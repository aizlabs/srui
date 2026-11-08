//! Find-next/find-previous and jump to PID (PX-011; design §§6.2–6.3, 7.6, 8,
//! 12.1, 22.5, 29).
//!
//! Search *locates* rows; it never hides one. It is server-side application
//! state, separate from the committed filter (PX-010), and four rules shape it:
//!
//! * **Same query rules as the filter.** A search query is parsed by
//!   [`FilterQuery::parse`] and evaluated by the filter's own matcher: the same
//!   512-byte cap, 16-term cap, inert sanitization, Unicode case folding and `|`
//!   OR terms, and every edit allocates a strictly monotonic
//!   [`FilterGeneration`]. No regex engine and no shell are involved.
//! * **The whole published collection.** Matches are counted over every row the
//!   server publishes to the model, in published order — never over the rows a
//!   client happens to have cached — and rows that match but are not published
//!   (hidden by the committed filter, or beyond the published-row ceiling) are
//!   counted and stated rather than silently ignored.
//! * **Identity, not position.** The current match is a target held by item ID
//!   ([`ItemId`], owned by one process-instance key), never by PID, row index or
//!   name. A PID jump resolves the typed number to the row the latest scan
//!   confirmed with that PID, and then holds that row's item ID.
//! * **No automatic retargeting.** When the target stops being published — its
//!   process ended, the filter hid it, or the ceiling dropped it — the search
//!   says so and stays on no other process, even when a new process now has the
//!   same PID. Only an explicit action (a new query, Next/Previous, or Go to PID)
//!   moves the target. If the very same item ID is published again, the target
//!   is that same process instance again, which is not a retarget.
//!
//! Matches that change between samples are re-counted on every publication:
//! the position ("match i of n", "row r of R") is recomputed against the rows
//! the client is about to hold, and Next/Previous from a target that is no
//! longer shown continue from the place it last held.
//!
//! The generic client's native table selection is local presentation state
//! (§22.5) and the protocol has no server-driven selection or reveal facility,
//! so the current match is published as semantic text on the search status line
//! rather than as a native highlight (see the PX-011 completion note).

use crate::filter::{FilterGeneration, FilterQuery};
use crate::projection::Row;
use crate::source::Observed;
use srui_sdk::ItemId;
use std::collections::HashSet;

/// The most bytes of a PID field's text the server keeps. A PID is at most ten
/// digits; the rest is room for surrounding spaces.
pub const MAX_PID_INPUT_BYTES: usize = 32;

/// The most bytes the search status line publishes.
pub const MAX_SEARCH_STATUS_BYTES: usize = 1024;

/// The most bytes of the query's own terms the status echoes.
const MAX_QUERY_ECHO_BYTES: usize = 256;

/// The most bytes of a process's display name the status echoes.
const MAX_NAME_ECHO_BYTES: usize = 96;

/// The status of a collecting shell before any search.
pub const IDLE_STATUS: &str = "Search: none · search never hides rows";

/// The status of a shell that collects nothing.
pub const UNCOLLECTED_STATUS: &str = "Search disabled: process collection not started";

/// One Next or Previous click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStep {
    Next,
    Previous,
}

/// What explicit action put the target where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Query,
    Pid(u32),
}

/// The current match: one published row, held by its item ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    item_id: ItemId,
    /// The row's PID and bounded name when it was last published, so a target
    /// that is no longer shown can still be named.
    pid: Option<u32>,
    name: String,
    /// The row index it last held, where Next/Previous continue from once it
    /// is no longer shown.
    anchor: usize,
    origin: Origin,
}

/// The server's search state (PX-011). Bounded: one parsed query (bounded by
/// the filter's caps), at most [`MAX_PID_INPUT_BYTES`] of PID text, and one
/// target.
#[derive(Debug, Clone, Default)]
pub struct Search {
    query: FilterQuery,
    generation: FilterGeneration,
    pid_input: String,
    target: Option<Target>,
    /// The PID of the last Go to PID that selected nothing.
    unresolved_pid: Option<u32>,
}

/// What the search controls publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchView {
    pub(crate) status: String,
    pub(crate) previous: bool,
    pub(crate) next: bool,
    pub(crate) go: bool,
}

impl SearchView {
    /// What [`crate::initialize_rows`] publishes for the search controls.
    pub(crate) fn initial(collecting: bool) -> Self {
        Self {
            status: if collecting {
                IDLE_STATUS
            } else {
                UNCOLLECTED_STATUS
            }
            .to_string(),
            previous: false,
            next: false,
            go: false,
        }
    }
}

/// The PID `text` names: one to ten ASCII digits, surrounding spaces ignored,
/// within `u32`. Anything else — signs, separators, letters — is not a PID.
pub fn parse_pid(text: &str) -> Option<u32> {
    let digits = text.trim();
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn known_pid(row: &Row) -> Option<u32> {
    match row.key.pid {
        Observed::Known(pid) => Some(pid),
        Observed::Missing(_) => None,
    }
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        one
    } else {
        many
    }
}

impl Target {
    fn of(rows: &[Row], index: usize, origin: Origin) -> Self {
        let row = &rows[index];
        Self {
            item_id: row.item_id,
            pid: known_pid(row),
            name: crate::bounded(row.display_name.as_str(), MAX_NAME_ECHO_BYTES),
            anchor: index,
            origin,
        }
    }

    fn describe(&self) -> String {
        match self.pid {
            Some(pid) => format!("PID {pid} · {}", self.name),
            None => format!("{} (PID unavailable)", self.name),
        }
    }
}

impl Search {
    /// The active search query.
    pub fn query(&self) -> &FilterQuery {
        &self.query
    }

    /// The generation of the active search query; every edit allocates the
    /// next one.
    pub fn generation(&self) -> FilterGeneration {
        self.generation
    }

    /// The PID field's text as the server holds it.
    pub fn pid_input(&self) -> &str {
        &self.pid_input
    }

    /// The item ID of the current match, if one is held.
    pub fn target(&self) -> Option<ItemId> {
        self.target.as_ref().map(|target| target.item_id)
    }

    /// Indices of the rows of `rows` that match the query, in published order.
    fn matches(&self, rows: &[Row]) -> Vec<usize> {
        if self.query.is_empty() {
            return Vec::new();
        }
        rows.iter()
            .enumerate()
            .filter(|(_, row)| self.query.matches_row(row))
            .map(|(index, _)| index)
            .collect()
    }

    /// Replaces the query with `raw` under a new generation, and targets the
    /// first match in published order — an explicit new search. An empty query
    /// holds no target.
    pub(crate) fn set_query(&mut self, raw: &str, rows: &[Row]) -> FilterGeneration {
        self.generation = self.generation.next();
        self.query = FilterQuery::parse(raw);
        self.unresolved_pid = None;
        self.target = self
            .matches(rows)
            .first()
            .map(|&index| Target::of(rows, index, Origin::Query));
        self.generation
    }

    /// Moves the target to the next or previous match, wrapping around. From a
    /// target still shown, the step is relative to its row; from one no longer
    /// shown, relative to the place it last held; with no target, the first or
    /// last match. Without a query or a match nothing moves.
    pub(crate) fn step(&mut self, step: SearchStep, rows: &[Row]) {
        let matches = self.matches(rows);
        let (Some(&first), Some(&last)) = (matches.first(), matches.last()) else {
            return;
        };
        let place = self.target.as_ref().map(|target| {
            match rows.iter().position(|row| row.item_id == target.item_id) {
                Some(index) => (index, true),
                None => (target.anchor, false),
            }
        });
        let chosen = match (step, place) {
            (SearchStep::Next, None) => first,
            (SearchStep::Previous, None) => last,
            (SearchStep::Next, Some((at, shown))) => matches
                .iter()
                .copied()
                .find(|&index| if shown { index > at } else { index >= at })
                .unwrap_or(first),
            (SearchStep::Previous, Some((at, _))) => matches
                .iter()
                .rev()
                .copied()
                .find(|&index| index < at)
                .unwrap_or(last),
        };
        self.unresolved_pid = None;
        self.target = Some(Target::of(rows, chosen, Origin::Query));
    }

    /// Keeps the PID field's text, bounded to [`MAX_PID_INPUT_BYTES`] on a
    /// character boundary. Typing never moves the target.
    pub(crate) fn set_pid_input(&mut self, text: &str) -> &str {
        let mut end = text.len().min(MAX_PID_INPUT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.pid_input = text[..end].to_string();
        &self.pid_input
    }

    /// Resolves the PID field to the published row the latest scan confirmed
    /// with that PID — an explicit new search — and targets it. A row shown
    /// only because an earlier scan saw it (`unconfirmed`) is not the currently
    /// observed instance and is not selected. When nothing is selected the
    /// target is cleared and the status explains why.
    pub(crate) fn go_to_pid(&mut self, rows: &[Row], unconfirmed: &HashSet<ItemId>) {
        let Some(pid) = parse_pid(&self.pid_input) else {
            return;
        };
        match rows
            .iter()
            .position(|row| known_pid(row) == Some(pid) && !unconfirmed.contains(&row.item_id))
        {
            Some(index) => {
                self.unresolved_pid = None;
                self.target = Some(Target::of(rows, index, Origin::Pid(pid)));
            }
            None => {
                self.unresolved_pid = Some(pid);
                self.target = None;
            }
        }
    }

    /// Brings the target's last-known place, PID and name up to the rows the
    /// client now holds. A target that is no longer shown is left exactly as it
    /// was: nothing here ever chooses another row.
    pub(crate) fn observe(&mut self, rows: &[Row]) {
        if let Some(target) = &mut self.target {
            if let Some(index) = rows.iter().position(|row| row.item_id == target.item_id) {
                let origin = target.origin;
                *target = Target::of(rows, index, origin);
            }
        }
    }

    fn is_idle(&self) -> bool {
        self.query.is_empty()
            && self.target.is_none()
            && self.unresolved_pid.is_none()
            && self.pid_input.trim().is_empty()
    }

    /// What the search controls publish when the client holds `rows`, out of
    /// the server's `unfiltered` collection, of which `unconfirmed` were not
    /// confirmed by the latest scan. A pure function of the search state and
    /// those rows.
    pub(crate) fn view(
        &self,
        rows: &[Row],
        unfiltered: &[Row],
        unconfirmed: &HashSet<ItemId>,
    ) -> SearchView {
        let go = parse_pid(&self.pid_input).is_some();
        if self.is_idle() {
            return SearchView {
                status: IDLE_STATUS.to_string(),
                previous: false,
                next: false,
                go,
            };
        }
        let shown = rows.len();
        let shown_rows = plural(shown, "shown row", "shown rows");
        let mut clauses = Vec::new();
        let mut matches = Vec::new();
        if !self.query.is_empty() {
            // One pass over the whole server-side collection; the published
            // rows are a subset of it, so their matches are read off the set.
            let all: HashSet<ItemId> = unfiltered
                .iter()
                .filter(|row| self.query.matches_row(row))
                .map(|row| row.item_id)
                .collect();
            matches = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| all.contains(&row.item_id))
                .map(|(index, _)| index)
                .collect();
            let terms = self
                .query
                .terms()
                .iter()
                .map(|term| format!("\"{}\"", term.display))
                .collect::<Vec<_>>()
                .join(" | ");
            let terms = crate::bounded(&terms, MAX_QUERY_ECHO_BYTES);
            if matches.is_empty() {
                clauses.push(format!(
                    "no match for {terms} in {shown} {shown_rows}; search hides no rows"
                ));
            } else {
                let found = matches.len();
                clauses.push(format!(
                    "{terms}: {found} {} in {shown} {shown_rows}",
                    plural(found, "match", "matches")
                ));
            }
            let hidden = all.len().saturating_sub(matches.len());
            if hidden > 0 {
                clauses.push(format!(
                    "{hidden} more {} not shown (filtered out or beyond the row limit)",
                    plural(hidden, "match", "matches")
                ));
            }
            if self.query.truncated_bytes() || self.query.omitted_terms() > 0 {
                clauses.push("query bounded".to_string());
            }
        }
        match &self.target {
            Some(target) => match rows.iter().position(|row| row.item_id == target.item_id) {
                Some(index) => {
                    let row = &rows[index];
                    let name = crate::bounded(row.display_name.as_str(), MAX_NAME_ECHO_BYTES);
                    let pid = known_pid(row)
                        .map_or_else(|| "unavailable".to_string(), |pid| pid.to_string());
                    let at = index + 1;
                    clauses.push(
                        match (matches.iter().position(|&m| m == index), target.origin) {
                            (Some(nth), _) => format!(
                                "match {} of {} at row {at} of {shown} · PID {pid} · {name}",
                                nth + 1,
                                matches.len()
                            ),
                            (None, Origin::Pid(_)) => {
                                format!("PID {pid} found at row {at} of {shown} · {name}")
                            }
                            (None, Origin::Query) => format!(
                                "row {at} of {shown} no longer matches · PID {pid} · {name}"
                            ),
                        },
                    );
                }
                None => {
                    let mut clause = format!(
                        "{} is no longer shown; the search moved to no other process",
                        target.describe()
                    );
                    if let Origin::Pid(pid) = target.origin {
                        if rows.iter().any(|row| {
                            known_pid(row) == Some(pid)
                                && row.item_id != target.item_id
                                && !unconfirmed.contains(&row.item_id)
                        }) {
                            clause.push_str(&format!(
                                " · a different process now has PID {pid}: Go to PID selects it"
                            ));
                        }
                    }
                    clauses.push(clause);
                }
            },
            None => {
                if let Some(pid) = self.unresolved_pid {
                    let with_pid = |row: &&Row| known_pid(row) == Some(pid);
                    clauses.push(if rows.iter().filter(with_pid).any(|row| !unconfirmed.contains(&row.item_id)) {
                        format!("PID {pid} was not shown when searched and is now: Go to PID selects it")
                    } else if rows.iter().any(|row| with_pid(&row)) {
                        format!("PID {pid} is shown only from an earlier scan and was not selected")
                    } else if unfiltered.iter().any(|row| with_pid(&row)) {
                        format!("PID {pid} is not shown: filtered out or beyond the row limit")
                    } else {
                        format!("no shown process has PID {pid}")
                    });
                }
            }
        }
        if !self.pid_input.trim().is_empty() && !go {
            clauses.push("PID field: digits only".to_string());
        }
        let status = if clauses.is_empty() {
            IDLE_STATUS.to_string()
        } else {
            crate::bounded(
                &format!("Search: {}", clauses.join(" · ")),
                MAX_SEARCH_STATUS_BYTES,
            )
        };
        let any = !matches.is_empty();
        SearchView {
            status,
            previous: any,
            next: any,
            go,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pid_is_one_to_ten_ascii_digits_within_u32() {
        assert_eq!(parse_pid("4102"), Some(4102));
        assert_eq!(parse_pid("  17 "), Some(17));
        assert_eq!(parse_pid("0"), Some(0));
        assert_eq!(parse_pid("4294967295"), Some(u32::MAX));
        for not_a_pid in [
            "",
            "   ",
            "4294967296",
            "00000000001",
            "-1",
            "+1",
            "1 2",
            "0x10",
            "١٢",
            "12a",
            "4102;kill",
        ] {
            assert_eq!(parse_pid(not_a_pid), None, "{not_a_pid:?}");
        }
    }

    #[test]
    fn the_pid_field_is_bounded_on_a_character_boundary() {
        let mut search = Search::default();
        let long = "é".repeat(MAX_PID_INPUT_BYTES);
        let kept = search.set_pid_input(&long).to_string();
        assert!(kept.len() <= MAX_PID_INPUT_BYTES);
        assert!(long.starts_with(&kept));
        assert_eq!(search.pid_input(), kept);
    }
}
