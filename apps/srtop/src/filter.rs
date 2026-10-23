//! Bounded, deterministic command and process name filtering (PX-010; design
//! §§7.6, 8, 18.2–18.3, 22.6).
//!
//! Filter evaluation is strictly server-side: no filtering logic lives on the client,
//! no regex engine is used, and input text is never passed to a shell or subprocess.
//!
//! Three rules shape it:
//!
//! * **Bounded grammar and inert sanitization:** The input query is bounded to
//!   [`MAX_FILTER_QUERY_BYTES`] (512 bytes) on a UTF-8 character boundary. All
//!   control, format, bidi and unassigned characters are replaced with spaces.
//!   Separate OR terms are split on the literal `|` character, bounded to
//!   [`MAX_FILTER_TERMS`] (16 terms). All regex, glob, and shell metacharacters
//!   (`.*+?()[]{}^$\;`&><`) are treated as literal characters.
//! * **Unicode case-insensitive matching:** Matching uses Unicode lowercase
//!   folding (`char::to_lowercase`), matching against the process's sanitized
//!   display name and any available sanitized command text.
//! * **Monotonic generations:** Every query modification at the application
//!   layer allocates a strictly monotonic [`FilterGeneration`]. Stale or out-of-order
//!   evaluations cannot overwrite newer committed results.

use crate::projection::Row;
use crate::source::{DisplayName, ProcessRecord};

/// Maximum UTF-8 byte length of a filter query evaluated by the server.
pub const MAX_FILTER_QUERY_BYTES: usize = 512;

/// Maximum number of non-empty `|`-separated OR terms evaluated per query.
pub const MAX_FILTER_TERMS: usize = 16;

/// Maximum UTF-8 byte length of available command text on a record.
pub const MAX_COMMAND_TEXT_BYTES: usize = 512;

/// A strictly monotonic query generation identifier at the application layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FilterGeneration(pub u64);

impl FilterGeneration {
    /// Initial generation when no filter has been submitted.
    pub const INITIAL: Self = Self(0);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for FilterGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One non-empty fixed-string filter term within a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterTerm {
    /// The sanitized, trimmed display text of this term.
    pub display: String,
    /// Unicode lowercase-folded text used for linear substring matching.
    pub folded: String,
}

impl PartialEq<&str> for FilterTerm {
    fn eq(&self, other: &&str) -> bool {
        self.display == *other || self.folded == *other
    }
}

impl PartialEq<FilterTerm> for &str {
    fn eq(&self, other: &FilterTerm) -> bool {
        *self == other.display || *self == other.folded
    }
}

/// A parsed and bounded filter query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilterQuery {
    /// The sanitized, byte-bounded raw query string.
    raw: String,
    /// Parsed terms, up to [`MAX_FILTER_TERMS`].
    terms: Vec<FilterTerm>,
    /// Whether the raw query had to be truncated to [`MAX_FILTER_QUERY_BYTES`].
    truncated_bytes: bool,
    /// Non-empty terms beyond [`MAX_FILTER_TERMS`] that were omitted.
    omitted_terms: usize,
}

/// A staged filter query waiting to be committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFilter {
    pub generation: FilterGeneration,
    pub query: FilterQuery,
}

/// Performs Unicode case folding via `char::to_lowercase`.
pub fn fold_case(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// Collapses runs of whitespace within a string to a single space.
fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_whitespace = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_whitespace {
                result.push(' ');
                in_whitespace = true;
            }
        } else {
            result.push(c);
            in_whitespace = false;
        }
    }
    result
}

/// Sanitizes query bytes into an inert string, bounding at [`MAX_FILTER_QUERY_BYTES`].
pub fn sanitize_query(raw: &str) -> (String, bool) {
    let mut truncated = false;
    let end = if raw.len() > MAX_FILTER_QUERY_BYTES {
        truncated = true;
        let mut b = MAX_FILTER_QUERY_BYTES;
        while !raw.is_char_boundary(b) {
            b -= 1;
        }
        b
    } else {
        raw.len()
    };
    let slice = &raw[..end];
    let mut out = String::with_capacity(slice.len());
    for c in slice.chars() {
        if c == '|' {
            out.push('|');
        } else if DisplayName::is_space_separator(c) || DisplayName::is_unsafe(c) || c.is_control()
        {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    (out, truncated)
}

impl FilterQuery {
    /// Parses a raw filter input string into a bounded [`FilterQuery`].
    pub fn parse(raw: &str) -> Self {
        let (sanitized, truncated_bytes) = sanitize_query(raw);
        let mut terms = Vec::new();
        let mut omitted_terms = 0;

        for segment in sanitized.split('|') {
            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }
            let collapsed = collapse_whitespace(trimmed);
            if collapsed.is_empty() {
                continue;
            }
            if terms.len() < MAX_FILTER_TERMS {
                let folded = fold_case(&collapsed);
                terms.push(FilterTerm {
                    display: collapsed,
                    folded,
                });
            } else {
                omitted_terms += 1;
            }
        }

        Self {
            raw: sanitized,
            terms,
            truncated_bytes,
            omitted_terms,
        }
    }

    /// True when the query has no active search terms (matches all rows).
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The sanitized, bounded raw input text.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Alias for [`Self::raw`].
    pub fn raw_bounded(&self) -> &str {
        &self.raw
    }

    /// The active terms evaluated for this query.
    pub fn terms(&self) -> &[FilterTerm] {
        &self.terms
    }

    /// True if input bytes exceeded [`MAX_FILTER_QUERY_BYTES`] and were truncated.
    pub fn truncated_bytes(&self) -> bool {
        self.truncated_bytes
    }

    /// Count of non-empty `|` terms omitted beyond [`MAX_FILTER_TERMS`].
    pub fn omitted_terms(&self) -> usize {
        self.omitted_terms
    }

    /// Evaluates whether this query matches the given process name and optional command text.
    pub fn matches_name_and_command(&self, name: &str, command: Option<&str>) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let folded_name = fold_case(name);
        let folded_cmd = command.map(fold_case);
        self.terms.iter().any(|term| {
            folded_name.contains(&term.folded)
                || folded_cmd
                    .as_deref()
                    .is_some_and(|cmd| cmd.contains(&term.folded))
        })
    }

    /// Evaluates whether this query matches a [`ProcessRecord`].
    pub fn matches_record(&self, record: &ProcessRecord) -> bool {
        self.matches_name_and_command(record.display_name.as_str(), record.command_text())
    }

    /// Evaluates whether this query matches a projected [`Row`].
    pub(crate) fn matches_row(&self, row: &Row) -> bool {
        self.matches_name_and_command(row.display_name.as_str(), row.display_name.command_text())
    }

    /// Formats the pending status text when a new query generation has been staged.
    pub fn format_pending_status(
        &self,
        generation: FilterGeneration,
        committed_generation: FilterGeneration,
        shown: usize,
        total: usize,
    ) -> String {
        let pending_summary = if self.terms.is_empty() {
            "none".to_string()
        } else {
            self.terms
                .iter()
                .map(|t| format!("\"{}\"", t.display))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        format!(
            "Filter pending (gen {generation}): {pending_summary} · showing committed gen {committed_generation} ({shown} of {total} rows)"
        )
    }

    /// Formats the committed status text for `FILTER_STATUS`.
    pub fn format_committed_status(
        &self,
        generation: FilterGeneration,
        shown: usize,
        matched: usize,
        total: usize,
    ) -> String {
        if self.terms.is_empty() {
            if generation.get() == 0 {
                if shown == 0 {
                    "Filter: none · 0 rows".to_string()
                } else {
                    format!("Filter: none · showing all {shown} rows")
                }
            } else {
                format!("Committed filter (gen {generation}): none · showing all {shown} rows")
            }
        } else {
            let terms_display = self
                .terms
                .iter()
                .map(|t| format!("\"{}\"", t.display))
                .collect::<Vec<_>>()
                .join(" | ");
            let mut text = format!(
                "Committed filter (gen {generation}): {terms_display} · {matched} of {total} rows matched"
            );
            if shown < matched {
                text.push_str(&format!(" ({shown} shown)"));
            }
            if self.truncated_bytes || self.omitted_terms > 0 {
                text.push_str(" · query bounded");
            }
            text
        }
    }
}
