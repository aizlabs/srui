//! Named saved views and their versioned, bounded settings format (PX-069;
//! design §§6.3, 7.3, 26–27; D2 T32).
//!
//! A saved view is what the server already owns about how the process list is
//! presented: the visible columns and their order (PX-068), the active sort
//! (PX-009), the command filter (PX-010), the user filter (PX-012), the table
//! or tree view (PX-018) and the thread controls (PX-019). It is authoritative
//! application state (§6.3), so it lives server-side, per user, next to the
//! application that owns it, never in the client. Nothing else is stored:
//!
//! * no endpoint, credential, host key or connection detail (those are the
//!   client's connection manager's, D2 T38);
//! * no window geometry or renderer appearance (local presentation state,
//!   §6.3);
//! * no process identity, selection, tag, search, PID set, pending action
//!   intent or confirmation. A [`SavedView`] has no field that could hold
//!   one, so none can be persisted as a preference (§§7.7, 27).
//! * no meter definitions: srtop has no meters yet (PX-070 adds them). A
//!   later version adds them through [`CURRENT_VERSION`] and a migration, not
//!   through a speculative field now.
//!
//! **The format.** UTF-8 text, one `key value` per line, every value escaped
//! ([`escape`]) so no byte outside printable ASCII, no space and no line break
//! appears inside one. The first line is `srtop-settings <version>`; then the
//! application (`app srtop`), the source the views were saved for (`source`,
//! the process source's identity, so a fixture's views never apply to the
//! live host or the other way round), the active view, and one
//! `view`…`end-view` block per view. The last line is `checksum` and the
//! FNV-1a 64 of every byte before it, so a truncated, partially written or
//! damaged file is refused whole rather than read as a shorter valid one. It
//! is an integrity check against accidents, not a signature: the file is the
//! user's own.
//!
//! **Versions and migration.** [`CURRENT_VERSION`] is 2. Version 1 is the
//! minimal layout record (columns and sort per view, no active view), kept
//! readable so the migration path is exercised from the first release: a
//! version 1 file is read, migrated in memory ([`Parsed::version`] says
//! from which version), and written back as version 2 only by the next save.
//! A version above [`CURRENT_VERSION`] is never interpreted past its first
//! line ([`Problem::NewerVersion`]); the store treats it as read-only and
//! never overwrites it.
//!
//! **Bounds (§26).** The whole file is at most [`MAX_SETTINGS_FILE_BYTES`],
//! [`MAX_LINES`] lines of at most [`MAX_LINE_BYTES`] each, [`MAX_VIEWS`]
//! views with names of at most [`MAX_VIEW_NAME_BYTES`], at most
//! [`MAX_COLUMN_ENTRIES`] column entries per view, a filter of at most
//! [`MAX_FILTER_QUERY_BYTES`] and a user field of at most
//! [`MAX_OWNER_INPUT_BYTES`]. Exceeding any bound refuses the file
//! ([`Problem`]); nothing is cut and then read as something else.
//!
//! **Compatibility.** Structure is strict: an unknown key, a missing key or a
//! malformed value refuses the file. Values that are well formed but that this
//! build cannot honour are dropped and explained ([`Parsed::notes`]): a column
//! or sort key the catalogue does not hold, a duplicate column. A view with no
//! recognised column gets the default layout. Nothing a file says can crash
//! the parser: every path returns a [`Problem`].

use crate::catalogue::{ColumnLayout, ProcessField, SortDirection, PROCESS_LIST};
use crate::filter::MAX_FILTER_QUERY_BYTES;
use crate::owner::MAX_OWNER_INPUT_BYTES;
use crate::sort::SortOrder;
use crate::source::DisplayName;
use crate::threads::ThreadSettings;
use std::fmt;

/// The first word of every settings file.
pub const MAGIC: &str = "srtop-settings";
/// The application a settings file belongs to.
pub const APP: &str = "srtop";
/// The version this build writes.
pub const CURRENT_VERSION: u32 = 2;
/// The oldest version this build reads (and migrates).
pub const OLDEST_VERSION: u32 = 1;
/// The most bytes a settings file may hold. Bounded reads stop one byte past
/// it, so an oversized file is refused without being read whole.
pub const MAX_SETTINGS_FILE_BYTES: usize = 64 * 1024;
/// The most bytes one line may hold, its line break excluded.
pub const MAX_LINE_BYTES: usize = 4096;
/// The most lines a settings file may hold.
pub const MAX_LINES: usize = 256;
/// The most saved views: one switcher button each.
pub const MAX_VIEWS: usize = 8;
/// The most bytes of a view's name, after trimming.
pub const MAX_VIEW_NAME_BYTES: usize = 64;
/// The most bytes of the source identity a file is scoped to.
pub const MAX_SOURCE_BYTES: usize = 512;
/// The most column entries one view may list, recognised or not.
pub const MAX_COLUMN_ENTRIES: usize = 32;
/// The most bytes of one column or sort key.
pub const MAX_KEY_BYTES: usize = 64;
/// The most compatibility notes kept from one read.
pub const MAX_NOTES: usize = 8;
/// The name of the one view of the default settings.
pub const DEFAULT_VIEW_NAME: &str = "Default";

const CHECKSUM: &str = "checksum";

/// A saved view's name: trimmed, non-empty, at most
/// [`MAX_VIEW_NAME_BYTES`], and free of control and invisible characters
/// ([`DisplayName::is_unsafe`]), so two names that render alike are never
/// told apart by a character nobody can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewName(String);

/// Why a text is not a view name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    TooLong,
    Unsafe,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "a view needs a name",
            Self::TooLong => "a view name is at most 64 bytes",
            Self::Unsafe => "a view name may not hold control or invisible characters",
        })
    }
}

impl ViewName {
    pub fn parse(text: &str) -> Result<Self, NameError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(NameError::Empty);
        }
        if text.len() > MAX_VIEW_NAME_BYTES {
            return Err(NameError::TooLong);
        }
        if text.chars().any(DisplayName::is_unsafe) {
            return Err(NameError::Unsafe);
        }
        Ok(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ViewName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The user filter a view restores (PX-012): every visible user, the
/// daemon's own account, or the user field's text (a UID or an account
/// name), re-parsed by the user filter when the view is applied. The PID set
/// is never saved: a PID is not a process identity across restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedOwner {
    AllVisible,
    Mine,
    Input(String),
}

/// One named view: everything it restores, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedView {
    pub name: ViewName,
    pub columns: ColumnLayout,
    /// `None` keeps whatever order is active when the view is applied: the
    /// view was saved before any sort was chosen.
    pub sort: Option<SortOrder>,
    /// The committed command filter's text (PX-010).
    pub filter: String,
    pub owner: SavedOwner,
    /// The process tree instead of the table (PX-018).
    pub tree: bool,
    pub threads: ThreadSettings,
}

impl SavedView {
    /// The one view of the default settings: the catalogued columns, PID
    /// ascending, no filter, every visible user, the table, and the default
    /// thread controls.
    pub fn default_view() -> Self {
        Self {
            name: ViewName(DEFAULT_VIEW_NAME.to_string()),
            columns: ColumnLayout::default(),
            sort: Some(SortOrder {
                field: ProcessField::Pid,
                direction: SortDirection::Ascending,
            }),
            filter: String::new(),
            owner: SavedOwner::AllVisible,
            tree: false,
            threads: ThreadSettings::default(),
        }
    }
}

/// Every saved view for one source, and which one is active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The process source these views were saved for.
    pub source: String,
    /// At least one, at most [`MAX_VIEWS`], names distinct.
    pub views: Vec<SavedView>,
    /// The view applied last, if it still exists. Deleting the active view
    /// leaves none active: no other view is applied in its place.
    pub active: Option<usize>,
}

impl Settings {
    /// The default settings: one [`SavedView::default_view`], active.
    pub fn defaults(source: &str) -> Self {
        Self {
            source: source.to_string(),
            views: vec![SavedView::default_view()],
            active: Some(0),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::defaults(&self.source)
    }

    pub fn active_view(&self) -> Option<&SavedView> {
        self.active.and_then(|index| self.views.get(index))
    }

    /// The index of the view named `name`.
    pub fn find(&self, name: &ViewName) -> Option<usize> {
        self.views.iter().position(|view| view.name == *name)
    }

    /// Whether these settings are within every bound the format holds a
    /// file to, so that what is written can always be read back.
    pub fn check(&self) -> Result<(), Problem> {
        if self.source.len() > MAX_SOURCE_BYTES {
            return Err(Problem::Bound("source"));
        }
        if self.views.is_empty() {
            return Err(Problem::Malformed {
                line: 0,
                what: "no view",
            });
        }
        if self.views.len() > MAX_VIEWS {
            return Err(Problem::Bound("views"));
        }
        if self.active.is_some_and(|active| active >= self.views.len()) {
            return Err(Problem::Malformed {
                line: 0,
                what: "active view out of range",
            });
        }
        for (index, view) in self.views.iter().enumerate() {
            if self.views[..index]
                .iter()
                .any(|other| other.name == view.name)
            {
                return Err(Problem::Malformed {
                    line: 0,
                    what: "duplicate view name",
                });
            }
            if view.filter.len() > MAX_FILTER_QUERY_BYTES {
                return Err(Problem::Bound("filter"));
            }
            if matches!(&view.owner, SavedOwner::Input(text) if text.len() > MAX_OWNER_INPUT_BYTES)
            {
                return Err(Problem::Bound("user field"));
            }
        }
        Ok(())
    }

    /// These settings as a current-version file. Settings that fail
    /// [`Self::check`] are refused rather than written.
    pub fn encode(&self) -> Result<String, Problem> {
        self.check()?;
        let mut body = format!("{MAGIC} {CURRENT_VERSION}\n");
        let mut line = |key: &str, value: &str| {
            body.push_str(key);
            body.push(' ');
            body.push_str(value);
            body.push('\n');
        };
        line("app", APP);
        line("source", &escape(&self.source));
        line(
            "active",
            &self
                .active
                .map_or_else(|| "none".to_string(), |active| active.to_string()),
        );
        for view in &self.views {
            line("view", &escape(view.name.as_str()));
            line(
                "columns",
                &view
                    .columns
                    .fields()
                    .iter()
                    .map(|field| field_key(*field))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            line("sort", &sort_value(view.sort));
            line("filter", &escape(&view.filter));
            line(
                "owner",
                &match &view.owner {
                    SavedOwner::AllVisible => "all".to_string(),
                    SavedOwner::Mine => "mine".to_string(),
                    SavedOwner::Input(text) => format!("input {}", escape(text)),
                },
            );
            line("tree", on_off(view.tree));
            line("user-threads", on_off(view.threads.user));
            line("kernel-threads", on_off(view.threads.kernel));
            line("end-view", "-");
        }
        let checksum = fnv1a64(body.as_bytes());
        body.push_str(&format!("{CHECKSUM} {checksum:016x}\n"));
        if body.len() > MAX_SETTINGS_FILE_BYTES {
            return Err(Problem::Oversized);
        }
        Ok(body)
    }
}

/// A column's key in a settings file: its catalogue key (PX-042), which
/// names the same metric in every build that has it.
pub fn field_key(field: ProcessField) -> &'static str {
    field.spec().key().as_str()
}

fn field_of(key: &str) -> Option<ProcessField> {
    PROCESS_LIST
        .into_iter()
        .find(|field| field_key(*field) == key)
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn sort_value(sort: Option<SortOrder>) -> String {
    match sort {
        None => "none".to_string(),
        Some(order) => format!(
            "{} {}",
            field_key(order.field),
            match order.direction {
                SortDirection::Ascending => "ascending",
                SortDirection::Descending => "descending",
            }
        ),
    }
}

/// FNV-1a, 64 bits: a fixed, dependency-free integrity check.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `text` with every byte outside printable ASCII, every space and every `%`
/// written as `%XX`, so a value never holds a separator or a line break.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_graphic() && byte != b'%' {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

/// The text [`escape`] wrote, or `None` for a value it cannot have written:
/// a raw byte outside printable ASCII, a bad `%XX`, or bytes that are not
/// UTF-8.
pub fn unescape(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            decoded.push(u8::try_from(high * 16 + low).ok()?);
            index += 3;
        } else if byte.is_ascii_graphic() {
            decoded.push(byte);
            index += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(decoded).ok()
}

/// Why a settings file was refused. Every refusal is whole: nothing of a
/// refused file is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// More than [`MAX_SETTINGS_FILE_BYTES`].
    Oversized,
    /// Not a settings file: no `srtop-settings` first line.
    NotSettings,
    /// No complete last line, or no checksum line: cut short.
    Truncated,
    /// The checksum does not match: written partly, or damaged.
    Checksum,
    /// A version this build does not know, newer than it. Never read past
    /// the first line, and never overwritten.
    NewerVersion(u32),
    /// A version that is not a number, or older than [`OLDEST_VERSION`].
    BadVersion,
    /// Another application's settings.
    ForeignApp,
    /// A line this version does not allow there.
    Malformed { line: usize, what: &'static str },
    /// A bound of the format exceeded (§26).
    Bound(&'static str),
    /// The file could not be read (`std::io::ErrorKind`, by name).
    Unreadable(String),
    /// The settings path names something other than a regular file.
    NotAFile,
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized => write!(
                formatter,
                "larger than {} KiB",
                MAX_SETTINGS_FILE_BYTES / 1024
            ),
            Self::NotSettings => formatter.write_str("not an srtop settings file"),
            Self::Truncated => formatter.write_str("cut short"),
            Self::Checksum => formatter.write_str("damaged or partly written"),
            Self::NewerVersion(version) => write!(
                formatter,
                "written by a newer srtop (version {version}; this build reads up to {CURRENT_VERSION})"
            ),
            Self::BadVersion => formatter.write_str("of an unknown version"),
            Self::ForeignApp => formatter.write_str("another application's settings"),
            Self::Malformed { line, what } => write!(formatter, "malformed at line {line}: {what}"),
            Self::Bound(what) => write!(formatter, "over the {what} limit"),
            Self::Unreadable(kind) => write!(formatter, "unreadable ({kind})"),
            Self::NotAFile => formatter.write_str("not a regular file"),
        }
    }
}

/// A settings file that was read: its settings in the current version's
/// shape, the version it was written in, and what was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub settings: Settings,
    /// The version the file was written in: below [`CURRENT_VERSION`] when
    /// it was migrated.
    pub version: u32,
    /// Values this build could not honour and dropped, at most
    /// [`MAX_NOTES`].
    pub notes: Vec<String>,
}

fn malformed(line: usize, what: &'static str) -> Problem {
    Problem::Malformed { line, what }
}

/// The version a settings file's first line declares, read from nothing
/// else: what decides whether the rest may be interpreted at all.
pub fn declared_version(bytes: &[u8]) -> Result<u32, Problem> {
    let first = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .filter(|first| first.len() <= MAX_LINE_BYTES)
        .ok_or(Problem::NotSettings)?;
    let first = std::str::from_utf8(first).map_err(|_| Problem::NotSettings)?;
    let version = first
        .strip_prefix(MAGIC)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or(Problem::NotSettings)?;
    if version.is_empty() || version.len() > 9 || !version.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Problem::BadVersion);
    }
    let version: u32 = version.parse().map_err(|_| Problem::BadVersion)?;
    if version > CURRENT_VERSION {
        return Err(Problem::NewerVersion(version));
    }
    if version < OLDEST_VERSION {
        return Err(Problem::BadVersion);
    }
    Ok(version)
}

/// Reads one settings file. Every refusal is a [`Problem`]; nothing panics.
pub fn parse(bytes: &[u8]) -> Result<Parsed, Problem> {
    if bytes.len() > MAX_SETTINGS_FILE_BYTES {
        return Err(Problem::Oversized);
    }
    let version = declared_version(bytes)?;
    if bytes.last() != Some(&b'\n') {
        return Err(Problem::Truncated);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| malformed(0, "not UTF-8"))?;
    let lines: Vec<&str> = text[..text.len() - 1].split('\n').collect();
    if lines.len() > MAX_LINES {
        return Err(Problem::Bound("lines"));
    }
    if lines.iter().any(|line| line.len() > MAX_LINE_BYTES) {
        return Err(Problem::Bound("line length"));
    }
    // The checksum line covers every byte before it.
    let last = lines.last().copied().unwrap_or_default();
    let Some(claimed) = last.strip_prefix("checksum ") else {
        return Err(Problem::Truncated);
    };
    if lines.len() < 2 {
        return Err(Problem::Truncated);
    }
    let covered = text.len() - 1 - last.len();
    let claimed = (claimed.len() == 16)
        .then(|| u64::from_str_radix(claimed, 16).ok())
        .flatten()
        .ok_or(Problem::Checksum)?;
    if claimed != fnv1a64(&bytes[..covered]) {
        return Err(Problem::Checksum);
    }
    Body::read(&lines[1..lines.len() - 1], version)
}

/// The lines between the first and the checksum, read in order.
struct Body<'a> {
    lines: &'a [&'a str],
    next: usize,
    version: u32,
    notes: Vec<String>,
}

impl<'a> Body<'a> {
    fn read(lines: &'a [&'a str], version: u32) -> Result<Parsed, Problem> {
        let mut body = Self {
            lines,
            next: 0,
            version,
            notes: Vec::new(),
        };
        let app = body.text("app")?;
        if app != APP {
            return Err(Problem::ForeignApp);
        }
        let source = body.text("source")?;
        if source.len() > MAX_SOURCE_BYTES {
            return Err(Problem::Bound("source"));
        }
        let active = if version >= 2 {
            let line = body.line_number();
            match body.value("active")? {
                "none" => None,
                index if index.len() <= 3 && index.bytes().all(|b| b.is_ascii_digit()) => Some(
                    index
                        .parse::<usize>()
                        .map_err(|_| malformed(line, "active"))?,
                ),
                _ => return Err(malformed(line, "active")),
            }
        } else {
            // Version 1 recorded no active view; the first is applied.
            Some(0)
        };
        let mut views = Vec::new();
        while body.next < body.lines.len() {
            if views.len() == MAX_VIEWS {
                return Err(Problem::Bound("views"));
            }
            views.push(body.view()?);
        }
        let settings = Settings {
            source,
            views,
            active,
        };
        settings.check().map_err(|problem| match problem {
            Problem::Malformed { what, .. } => malformed(0, what),
            other => other,
        })?;
        Ok(Parsed {
            settings,
            version,
            notes: body.notes,
        })
    }

    fn line_number(&self) -> usize {
        // The first line is line 1; the body starts at line 2.
        self.next + 2
    }

    fn note(&mut self, note: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(note);
        }
    }

    /// The next line, which must be `key value`; its value.
    fn value(&mut self, key: &'static str) -> Result<&'a str, Problem> {
        let line = self.line_number();
        let text = self.lines.get(self.next).ok_or(malformed(line, key))?;
        let value = text
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(' '))
            .ok_or(malformed(line, key))?;
        self.next += 1;
        Ok(value)
    }

    /// The next line's escaped value, decoded.
    fn text(&mut self, key: &'static str) -> Result<String, Problem> {
        let line = self.line_number();
        let value = self.value(key)?;
        unescape(value).ok_or(malformed(line, key))
    }

    fn on_off(&mut self, key: &'static str) -> Result<bool, Problem> {
        let line = self.line_number();
        match self.value(key)? {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err(malformed(line, key)),
        }
    }

    fn view(&mut self) -> Result<SavedView, Problem> {
        let line = self.line_number();
        let name = self.text("view")?;
        if name.trim().len() > MAX_VIEW_NAME_BYTES {
            return Err(Problem::Bound("view name"));
        }
        let name = ViewName::parse(&name).map_err(|_| malformed(line, "view name"))?;
        let columns = self.columns(&name)?;
        let sort = self.sort(&name)?;
        let mut view = SavedView {
            name,
            columns,
            sort,
            filter: String::new(),
            owner: SavedOwner::AllVisible,
            tree: false,
            threads: ThreadSettings::default(),
        };
        if self.version >= 2 {
            view.filter = self.text("filter")?;
            if view.filter.len() > MAX_FILTER_QUERY_BYTES {
                return Err(Problem::Bound("filter"));
            }
            view.owner = self.owner()?;
            view.tree = self.on_off("tree")?;
            view.threads = ThreadSettings {
                user: self.on_off("user-threads")?,
                kernel: self.on_off("kernel-threads")?,
            };
        }
        let line = self.line_number();
        if self.value("end-view")? != "-" {
            return Err(malformed(line, "end-view"));
        }
        Ok(view)
    }

    fn columns(&mut self, name: &ViewName) -> Result<ColumnLayout, Problem> {
        let line = self.line_number();
        let value = self.value("columns")?;
        let entries: Vec<&str> = value.split(',').collect();
        if entries.len() > MAX_COLUMN_ENTRIES {
            return Err(Problem::Bound("columns"));
        }
        let mut fields = Vec::new();
        for entry in entries {
            if !is_key(entry) {
                return Err(malformed(line, "columns"));
            }
            match field_of(entry) {
                Some(field) if fields.contains(&field) => {
                    self.note(format!(
                        "view “{name}”: column {entry} listed twice; kept once"
                    ));
                }
                Some(field) => fields.push(field),
                None => self.note(format!(
                    "view “{name}”: column {entry} is not in this srtop and was dropped"
                )),
            }
        }
        Ok(ColumnLayout::new(&fields).unwrap_or_else(|_| {
            self.note(format!(
                "view “{name}”: no recognised column; the default columns are shown"
            ));
            ColumnLayout::default()
        }))
    }

    fn sort(&mut self, name: &ViewName) -> Result<Option<SortOrder>, Problem> {
        let line = self.line_number();
        let value = self.value("sort")?;
        if value == "none" {
            return Ok(None);
        }
        let (key, direction) = value.split_once(' ').ok_or(malformed(line, "sort"))?;
        let direction = match direction {
            "ascending" => SortDirection::Ascending,
            "descending" => SortDirection::Descending,
            _ => return Err(malformed(line, "sort")),
        };
        if !is_key(key) {
            return Err(malformed(line, "sort"));
        }
        match field_of(key) {
            Some(field) => Ok(Some(SortOrder { field, direction })),
            None => {
                self.note(format!(
                    "view “{name}”: sort column {key} is not in this srtop; the order is kept"
                ));
                Ok(None)
            }
        }
    }

    fn owner(&mut self) -> Result<SavedOwner, Problem> {
        let line = self.line_number();
        let value = self.value("owner")?;
        match value {
            "all" => Ok(SavedOwner::AllVisible),
            "mine" => Ok(SavedOwner::Mine),
            _ => {
                let text = value
                    .strip_prefix("input ")
                    .and_then(unescape)
                    .ok_or(malformed(line, "owner"))?;
                if text.len() > MAX_OWNER_INPUT_BYTES {
                    return Err(Problem::Bound("user field"));
                }
                Ok(SavedOwner::Input(text))
            }
        }
    }
}

/// Whether `key` has the shape of a catalogue key: lowercase ASCII letters,
/// digits, `.`, `_` and `-`, at most [`MAX_KEY_BYTES`].
fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Settings {
        let mut settings = Settings::defaults("fixture-source");
        settings.views.push(SavedView {
            name: ViewName::parse("CPU hogs · ünïcode 100%").unwrap(),
            columns: ColumnLayout::new(&[ProcessField::Cpu, ProcessField::Name]).unwrap(),
            sort: Some(SortOrder {
                field: ProcessField::Cpu,
                direction: SortDirection::Descending,
            }),
            filter: "worker | helper %20\nnext".into(),
            owner: SavedOwner::Input("alice".into()),
            tree: true,
            threads: ThreadSettings {
                user: true,
                kernel: false,
            },
        });
        settings.active = Some(1);
        settings
    }

    #[test]
    fn settings_round_trip_through_the_current_format() {
        let settings = sample();
        let text = settings.encode().unwrap();
        assert!(text.starts_with("srtop-settings 2\napp srtop\n"));
        assert!(text.is_ascii(), "every value is escaped: {text}");
        let parsed = parse(text.as_bytes()).unwrap();
        assert_eq!(parsed.settings, settings);
        assert_eq!(parsed.version, CURRENT_VERSION);
        assert!(parsed.notes.is_empty());
    }

    #[test]
    fn every_truncation_and_every_flipped_byte_is_refused_without_panicking() {
        let text = sample().encode().unwrap();
        let bytes = text.as_bytes();
        for cut in 0..bytes.len() {
            assert!(
                parse(&bytes[..cut]).is_err(),
                "a file cut at byte {cut} of {} was accepted",
                bytes.len()
            );
        }
        // A deterministic sweep of single-byte damage: every position, a few
        // replacement bytes each.
        for position in 0..bytes.len() {
            for replacement in [b'0', b' ', b'\n', 0xff, b'%'] {
                if bytes[position] == replacement {
                    continue;
                }
                let mut damaged = bytes.to_vec();
                damaged[position] = replacement;
                if let Ok(parsed) = parse(&damaged) {
                    panic!("damage at {position} accepted: {parsed:?}");
                }
            }
        }
    }

    #[test]
    fn unknown_values_are_dropped_and_explained_while_structure_stays_strict() {
        let body = "srtop-settings 2\napp srtop\nsource s\nactive 0\nview V\n\
                    columns process.gpu,process.pid,process.pid\nsort process.gpu descending\n\
                    filter -\nowner all\ntree off\nuser-threads off\nkernel-threads on\nend-view -\n";
        let file = format!("{body}checksum {:016x}\n", fnv1a64(body.as_bytes()));
        let parsed = parse(file.as_bytes()).unwrap();
        let view = &parsed.settings.views[0];
        assert_eq!(view.columns.fields(), &[ProcessField::Pid]);
        assert_eq!(view.sort, None);
        assert_eq!(parsed.notes.len(), 3, "{:?}", parsed.notes);

        let unknown_key = body.replace("tree off", "tree off\nmeters cpu");
        let file = format!(
            "{unknown_key}checksum {:016x}\n",
            fnv1a64(unknown_key.as_bytes())
        );
        assert!(matches!(
            parse(file.as_bytes()),
            Err(Problem::Malformed { .. })
        ));
    }

    #[test]
    fn escaping_is_reversible_and_rejects_what_it_never_writes() {
        for text in ["", "plain", "a b", "100%", "line\nbreak", "ünï", "\u{0}"] {
            assert_eq!(unescape(&escape(text)).as_deref(), Some(text));
        }
        assert_eq!(unescape("a b"), None);
        assert_eq!(unescape("%4"), None);
        assert_eq!(unescape("%zz"), None);
        assert_eq!(unescape("%FF"), None, "not UTF-8");
    }

    #[test]
    fn view_names_are_bounded_trimmed_and_visible() {
        assert_eq!(ViewName::parse("  Mine  ").unwrap().as_str(), "Mine");
        assert_eq!(ViewName::parse("   "), Err(NameError::Empty));
        assert_eq!(
            ViewName::parse(&"x".repeat(MAX_VIEW_NAME_BYTES + 1)),
            Err(NameError::TooLong)
        );
        assert!(ViewName::parse(&"x".repeat(MAX_VIEW_NAME_BYTES)).is_ok());
        assert_eq!(ViewName::parse("a\u{202e}b"), Err(NameError::Unsafe));
        assert_eq!(ViewName::parse("a\u{7}b"), Err(NameError::Unsafe));
    }
}
