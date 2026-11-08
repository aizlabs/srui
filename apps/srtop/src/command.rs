//! The selected process's command line and executable context (PX-014;
//! design §§7.2–7.3 `RichText`, 22.4–22.6, 26, 27).
//!
//! The inspector (PX-013) shows, for its one current target, the argument
//! vector the server's kernel reports, the executable and the working
//! directory. Six rules shape it:
//!
//! * **Read lazily, for one instance.** Nothing here is collected by a scan.
//!   The arguments, executable and working directory are read only by the
//!   inspector's detail worker, for the current target, and only after its
//!   record has been confirmed to be the target's own instance (its start time
//!   is the key's creation token, before *and* after the reads). They live in
//!   the inspector's current details and nowhere else: a new selection, an
//!   ended target or a dropped view discards them.
//! * **Raw bytes are identity; display text is not.** [`Arguments`] keeps each
//!   argument's bytes exactly as read, bounded, and its exact length. Display
//!   text is made from them by [`inert`] and is never written back: no filter,
//!   key or row is ever derived from it.
//! * **An argument vector, not a shell command.** Each argument is shown on its
//!   own line with its index and byte length (`argv[1] · 13 bytes · …`). No
//!   joined, quoted or "copy as shell command" form is produced, so nothing
//!   shown can be mistaken for something that would run as it reads.
//! * **Inert text.** Control characters (C0, DEL, C1), the Unicode format,
//!   line-separator and bidirectional-override characters listed in
//!   [`is_hidden`], and bytes that are not UTF-8 are shown as visible escapes
//!   (`\n`, `\x1b`, `\u{202e}`, `\xff`); a backslash is doubled, so an escape
//!   cannot be forged by an argument that spells one.
//! * **Bounded, and cut explicitly.** At most [`MAX_RETAINED_ARGUMENTS`]
//!   arguments and [`MAX_RETAINED_ARGUMENT_BYTES`] bytes are kept; the rest of
//!   the area is counted, up to [`MAX_ARGUMENT_SCAN_BYTES`], and never kept.
//!   Each argument shows at most [`MAX_ARGUMENT_DISPLAY_BYTES`], each path
//!   [`MAX_PATH_DISPLAY_BYTES`], and the whole view [`MAX_COMMAND_TEXT_BYTES`].
//!   Every cut says how much was not shown, in raw bytes, ahead of the text it
//!   cut, where an argument cannot forge it.
//! * **Paths are server text.** The executable and working directory are the
//!   server's own paths, shown as text. Nothing here, or in the client, resolves
//!   or opens them locally (§26: server paths are never interpreted as local
//!   paths).
//!
//! Neither the raw bytes nor the display text is ever logged: [`Sensitive`]'s
//! `Debug` shows only a length, so a derived `Debug` of any value holding one
//! (a [`crate::inspector::DetailRead`], say) cannot print an argument.
use crate::source::MissingReason;
use std::fmt::{self, Write as _};
use std::io::{self, Read};

/// The most arguments kept from one read. Arguments past it are counted.
pub const MAX_RETAINED_ARGUMENTS: usize = 256;
/// The most argument bytes kept from one read, separators not included.
/// Arguments that start past it are counted, not kept.
pub const MAX_RETAINED_ARGUMENT_BYTES: usize = 16 * 1024;
/// The most bytes of one argument area read at all: everything past the kept
/// bytes up to here is counted and dropped. The kernel's own ceiling on the
/// area is a quarter of the stack limit, 2 MiB under the common 8 MiB stack
/// (`execve(2)`); an area beyond this one is reported as "at least".
pub const MAX_ARGUMENT_SCAN_BYTES: u64 = 2 * 1024 * 1024;
/// The most bytes of a path kept: `PATH_MAX`, which the kernel's own
/// `readlink` of a `/proc/<pid>` link never exceeds.
pub const MAX_PATH_BYTES: usize = 4096;
/// The most display bytes one argument is shown with.
pub const MAX_ARGUMENT_DISPLAY_BYTES: usize = 1024;
/// The most display bytes one path is shown with.
pub const MAX_PATH_DISPLAY_BYTES: usize = 1024;
/// The most bytes the whole command-line view holds.
pub const MAX_COMMAND_TEXT_BYTES: usize = 8192;

/// The view's first word, whatever state it shows.
pub const COMMAND_HEAD: &str = "Command line";
/// What the view says with nothing selected.
pub const NO_SELECTION_COMMAND: &str = "Command line: no process selected";
/// What the uncollected shell's view says.
pub const UNCOLLECTED_COMMAND: &str = "Command line: process collection not started";
const EXECUTABLE_HEAD: &str = "Executable on the server";
const WORKING_DIRECTORY_HEAD: &str = "Working directory on the server";
/// Room kept for the line that counts arguments not shown.
const MORE_LINE_BYTES: usize = 160;
/// Room kept for the header line.
const HEADER_BYTES: usize = 320;
/// Room kept for one path line beyond its displayed path.
const PATH_LINE_BYTES: usize = MAX_PATH_DISPLAY_BYTES + 128;
/// The display bytes left for argument lines once the header, the paths and the
/// "more" line are provided for.
const ARGUMENT_LINES_BYTES: usize =
    MAX_COMMAND_TEXT_BYTES - HEADER_BYTES - MORE_LINE_BYTES - 2 * PATH_LINE_BYTES;
const _: () = assert!(ARGUMENT_LINES_BYTES >= 4 * (MAX_ARGUMENT_DISPLAY_BYTES + 64));

/// Bytes read from a process that must never be logged: `Debug` shows only
/// their length.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Sensitive(Vec<u8>);

impl Sensitive {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Sensitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes withheld>", self.0.len())
    }
}

/// One argument: its bytes as read, cut only by the retention bound, and its
/// full length.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Argument {
    retained: Sensitive,
    length: u64,
    /// Whether `length` is exact: false only for an argument the scan bound
    /// stopped counting.
    counted: bool,
}

impl Argument {
    /// The bytes kept, exactly as read: the whole argument unless
    /// [`Self::is_whole`] says otherwise.
    pub fn raw(&self) -> &[u8] {
        self.retained.bytes()
    }

    /// The argument's length in bytes, its terminating NUL not included.
    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn is_whole(&self) -> bool {
        self.counted && self.retained.0.len() as u64 == self.length
    }
}

/// A process's argument area as `/proc/<pid>/cmdline` reports it: the
/// arguments separated by NUL bytes (`proc_pid_cmdline(5)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Arguments {
    entries: Vec<Argument>,
    /// Arguments counted past the kept ones, and their bytes, NULs excluded.
    unretained: u64,
    unretained_bytes: u64,
    /// The area's size in bytes, NULs included, as counted.
    total_bytes: u64,
    /// Whether the count reached the end of the area. When it did not, every
    /// count is a lower bound.
    counted_to_end: bool,
    /// Whether the area ends with a NUL. A process that rewrote its arguments
    /// in place (`setproctitle`) may leave it without one, and the kernel then
    /// reports the rewritten text (`get_mm_cmdline`, fs/proc/base.c).
    terminated: bool,
}

#[derive(Default)]
struct Parser {
    entries: Vec<Argument>,
    current: Vec<u8>,
    current_length: u64,
    /// Whether an argument has begun and not yet ended.
    open: bool,
    /// Whether the open argument is kept.
    keeping: bool,
    kept_bytes: usize,
    unretained: u64,
    unretained_bytes: u64,
    total_bytes: u64,
    last_was_nul: bool,
}

impl Parser {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.total_bytes += 1;
            if !self.open {
                self.open = true;
                self.keeping = self.entries.len() < MAX_RETAINED_ARGUMENTS
                    && self.kept_bytes < MAX_RETAINED_ARGUMENT_BYTES;
            }
            if byte == 0 {
                self.close(true);
                self.last_was_nul = true;
            } else {
                self.current_length += 1;
                if self.keeping && self.kept_bytes < MAX_RETAINED_ARGUMENT_BYTES {
                    self.current.push(byte);
                    self.kept_bytes += 1;
                }
                self.last_was_nul = false;
            }
        }
    }

    fn close(&mut self, counted: bool) {
        if self.keeping {
            self.entries.push(Argument {
                retained: Sensitive(std::mem::take(&mut self.current)),
                length: self.current_length,
                counted,
            });
        } else {
            self.unretained += 1;
            self.unretained_bytes += self.current_length;
        }
        self.current_length = 0;
        self.open = false;
    }

    fn finish(mut self, counted_to_end: bool) -> Arguments {
        if self.open {
            self.close(counted_to_end);
        }
        Arguments {
            entries: self.entries,
            unretained: self.unretained,
            unretained_bytes: self.unretained_bytes,
            total_bytes: self.total_bytes,
            counted_to_end,
            terminated: self.last_was_nul,
        }
    }
}

impl Arguments {
    /// Reads one argument area from `reader`: the first
    /// [`MAX_RETAINED_ARGUMENTS`] arguments within
    /// [`MAX_RETAINED_ARGUMENT_BYTES`] are kept exactly, and the rest of the
    /// area is counted up to [`MAX_ARGUMENT_SCAN_BYTES`] through one fixed
    /// buffer, so memory is bounded whatever the area holds.
    pub fn read(mut reader: impl Read) -> io::Result<Self> {
        let mut parser = Parser::default();
        let mut buffer = [0u8; 8192];
        let mut scanned = 0u64;
        while scanned < MAX_ARGUMENT_SCAN_BYTES {
            let room = (MAX_ARGUMENT_SCAN_BYTES - scanned).min(buffer.len() as u64) as usize;
            let read = match reader.read(&mut buffer[..room]) {
                Ok(0) => return Ok(parser.finish(true)),
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            parser.feed(&buffer[..read]);
            scanned += read as u64;
        }
        // The scan bound is reached: one byte more says whether the area
        // goes on, and so whether the counts are exact.
        let mut probe = [0u8; 1];
        let more = loop {
            match reader.read(&mut probe) {
                Ok(read) => break read > 0,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        };
        Ok(parser.finish(!more))
    }

    /// The area `bytes` states, read exactly as [`Self::read`] reads one.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::read(bytes).expect("reading a byte slice cannot fail")
    }

    /// The arguments kept, in order.
    pub fn entries(&self) -> &[Argument] {
        &self.entries
    }

    /// Arguments counted but not kept.
    pub fn unretained(&self) -> u64 {
        self.unretained
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn counted_to_end(&self) -> bool {
        self.counted_to_end
    }

    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// Whether the area held nothing at all.
    pub fn is_empty(&self) -> bool {
        self.total_bytes == 0
    }

    fn count(&self) -> u64 {
        self.entries.len() as u64 + self.unretained
    }
}

/// Why a live process has no argument area, executable or working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Memoryless {
    /// It has exited and not been reaped: its memory and files are gone.
    Zombie,
    /// A kernel thread, which never had a user address space.
    KernelThread,
}

impl Memoryless {
    fn describe(self) -> &'static str {
        match self {
            Self::Zombie => "none: the process has exited and is not yet reaped (zombie)",
            Self::KernelThread => "none: a kernel thread has none",
        }
    }
}

/// What one read of the argument area found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArgumentsRead {
    Read(Arguments),
    /// The area is empty for a process that has one: it was started with no
    /// arguments at all.
    Empty,
    /// The process has no area.
    None(Memoryless),
    Missing(MissingReason),
}

/// A server path, as read, cut to [`MAX_PATH_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerPath {
    retained: Sensitive,
    length: usize,
}

impl ServerPath {
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            retained: Sensitive(bytes[..bytes.len().min(MAX_PATH_BYTES)].to_vec()),
            length: bytes.len(),
        }
    }

    /// The bytes kept, exactly as read.
    pub fn raw(&self) -> &[u8] {
        self.retained.bytes()
    }
}

/// What one read of a path link (`exe`, `cwd`) found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathRead {
    Read(ServerPath),
    None(Memoryless),
    Missing(MissingReason),
}

impl PathRead {
    /// A path read as `bytes`.
    pub fn server(bytes: &[u8]) -> Self {
        Self::Read(ServerPath::new(bytes))
    }
}

/// A process instance's command line and executable context.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandContext {
    pub arguments: ArgumentsRead,
    pub executable: PathRead,
    pub working_directory: PathRead,
}

impl CommandContext {
    /// Nothing could be read, for `reason`.
    pub fn missing(reason: MissingReason) -> Self {
        Self {
            arguments: ArgumentsRead::Missing(reason),
            executable: PathRead::Missing(reason),
            working_directory: PathRead::Missing(reason),
        }
    }

    /// A fixture's context: `argv` read as the kernel would report it, each
    /// argument NUL-terminated.
    pub fn fixture(argv: &[&[u8]], executable: PathRead, working_directory: PathRead) -> Self {
        let mut area = Vec::new();
        for argument in argv {
            area.extend_from_slice(argument);
            area.push(0);
        }
        let arguments = Arguments::from_bytes(&area);
        Self {
            arguments: if arguments.is_empty() {
                ArgumentsRead::Empty
            } else {
                ArgumentsRead::Read(arguments)
            },
            executable,
            working_directory,
        }
    }

    /// The view's text: a header, one line per argument shown, a line for the
    /// arguments not shown, and the two paths. Never more than
    /// [`MAX_COMMAND_TEXT_BYTES`].
    pub fn text(&self) -> String {
        let executable = path_line(EXECUTABLE_HEAD, &self.executable);
        let directory = path_line(WORKING_DIRECTORY_HEAD, &self.working_directory);
        let mut text = match &self.arguments {
            ArgumentsRead::Read(arguments) => arguments_text(arguments),
            ArgumentsRead::Empty => {
                format!("{COMMAND_HEAD}: empty: the process was started with no arguments")
            }
            ArgumentsRead::None(kind) => format!("{COMMAND_HEAD}: {}", kind.describe()),
            ArgumentsRead::Missing(reason) => format!("{COMMAND_HEAD}: {}", reason.describe()),
        };
        text.push('\n');
        text.push_str(&executable);
        text.push('\n');
        text.push_str(&directory);
        debug_assert!(text.len() <= MAX_COMMAND_TEXT_BYTES, "{}", text.len());
        crate::bounded(&text, MAX_COMMAND_TEXT_BYTES)
    }
}

fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 {
        one
    } else {
        many
    }
}

fn arguments_text(arguments: &Arguments) -> String {
    let at_least = if arguments.counted_to_end {
        ""
    } else {
        "at least "
    };
    let count = arguments.count();
    let mut text = format!(
        "{COMMAND_HEAD} (argv on the server, shown as text, never run): {at_least}{count} {}, {at_least}{} {}",
        plural(count, "argument", "arguments"),
        arguments.total_bytes,
        plural(arguments.total_bytes, "byte", "bytes"),
    );
    if !arguments.terminated && arguments.counted_to_end {
        text.push_str(
            " · not NUL-terminated: the process rewrote its argument area, so these may not be the arguments it started with",
        );
    }
    debug_assert!(text.len() <= HEADER_BYTES);
    let mut used = 0usize;
    let mut shown = 0usize;
    for (index, argument) in arguments.entries.iter().enumerate() {
        let line = argument_line(index, argument);
        if used + line.len() + 1 > ARGUMENT_LINES_BYTES {
            break;
        }
        text.push('\n');
        text.push_str(&line);
        used += line.len() + 1;
        shown += 1;
    }
    let hidden = &arguments.entries[shown..];
    let more = hidden.len() as u64 + arguments.unretained;
    if more > 0 {
        let bytes = hidden.iter().map(Argument::length).sum::<u64>() + arguments.unretained_bytes;
        let _ = write!(
            text,
            "\n… {at_least}{more} more {} not shown ({at_least}{bytes} {})",
            plural(more, "argument", "arguments"),
            plural(bytes, "byte", "bytes"),
        );
    }
    text
}

fn argument_line(index: usize, argument: &Argument) -> String {
    let (shown, consumed) = inert(argument.raw(), MAX_ARGUMENT_DISPLAY_BYTES);
    let at_least = if argument.counted { "" } else { "at least " };
    let length = argument.length;
    let unit = plural(length, "byte", "bytes");
    if length == 0 {
        format!("argv[{index}] · 0 bytes (empty)")
    } else if consumed as u64 == length && argument.counted {
        format!("argv[{index}] · {length} {unit} · {shown}")
    } else {
        format!("argv[{index}] · {at_least}{length} {unit}, first {consumed} shown · {shown}…")
    }
}

fn path_line(head: &str, path: &PathRead) -> String {
    match path {
        PathRead::Read(path) => {
            let (shown, consumed) = inert(path.raw(), MAX_PATH_DISPLAY_BYTES);
            if consumed == path.length {
                format!("{head}: {shown}")
            } else {
                format!(
                    "{head} ({} bytes, first {consumed} shown): {shown}…",
                    path.length
                )
            }
        }
        PathRead::None(kind) => format!("{head}: {}", kind.describe()),
        PathRead::Missing(reason) => format!("{head}: {}", reason.describe()),
    }
}

/// Whether `c` is a character [`inert`] shows as an escape although it is not
/// a control character: an invisible format character, a line or paragraph
/// separator, or a bidirectional mark, embedding, override or isolate — each
/// can hide, reorder or break the text around it.
pub fn is_hidden(c: char) -> bool {
    matches!(c,
        '\u{00AD}'
        | '\u{061C}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{E0000}'..='\u{E007F}')
}

fn escape(c: char) -> Option<String> {
    Some(match c {
        '\\' => "\\\\".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        c if c.is_ascii_control() => format!("\\x{:02x}", u32::from(c)),
        c if c.is_control() || is_hidden(c) => format!("\\u{{{:04x}}}", u32::from(c)),
        _ => return None,
    })
}

/// `raw` as inert display text of at most `limit` bytes, and how many bytes of
/// `raw` it shows.
///
/// UTF-8 is shown as text, except that control characters (C0, DEL and C1) and
/// the characters [`is_hidden`] names are shown as escapes, and a backslash is
/// doubled; a byte that is not UTF-8 is shown as `\xHH`. The text is for
/// display only: nothing reads it back as an argument or a path.
pub fn inert(raw: &[u8], limit: usize) -> (String, usize) {
    let mut shown = String::new();
    let mut consumed = 0usize;
    let mut rest = raw;
    while !rest.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(rest) {
            Ok(valid) => (valid, 0),
            Err(error) => (
                std::str::from_utf8(&rest[..error.valid_up_to()]).unwrap_or_default(),
                error
                    .error_len()
                    .unwrap_or(rest.len() - error.valid_up_to()),
            ),
        };
        for c in valid.chars() {
            let escaped = escape(c);
            let width = escaped.as_ref().map_or(c.len_utf8(), String::len);
            if shown.len() + width > limit {
                return (shown, consumed);
            }
            match escaped {
                Some(escaped) => shown.push_str(&escaped),
                None => shown.push(c),
            }
            consumed += c.len_utf8();
        }
        for byte in &rest[valid.len()..valid.len() + invalid] {
            if shown.len() + 4 > limit {
                return (shown, consumed);
            }
            let _ = write!(shown, "\\x{byte:02x}");
            consumed += 1;
        }
        rest = &rest[valid.len() + invalid..];
    }
    (shown, consumed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_hidden_and_invalid_bytes_are_visible_escapes_and_a_backslash_is_doubled() {
        let raw = b"a\"b' $(x) `y` ; rm -rf / # \\x1b \n\r\t\x1b[31m\x7f\xc2\x85\xe2\x80\xae\xe2\x80\xa8\xef\xbb\xbf\xff\xc3";
        let (shown, consumed) = inert(raw, usize::MAX);
        assert_eq!(consumed, raw.len());
        assert_eq!(
            shown,
            "a\"b' $(x) `y` ; rm -rf / # \\\\x1b \\n\\r\\t\\x1b[31m\\x7f\\u{0085}\\u{202e}\\u{2028}\\u{feff}\\xff\\xc3"
        );
        assert!(!shown.chars().any(|c| c.is_control() || is_hidden(c)));
        // Ordinary text, including other scripts and emoji, is untouched.
        let (plain, _) = inert("naïve 日本 🙂".as_bytes(), usize::MAX);
        assert_eq!(plain, "naïve 日本 🙂");
    }

    #[test]
    fn a_display_limit_cuts_at_a_whole_character_or_escape_and_counts_the_raw_bytes() {
        let (shown, consumed) = inert(b"ab\x1bcd", 5);
        assert_eq!(
            (shown.as_str(), consumed),
            ("ab", 2),
            "an escape is never split"
        );
        let (shown, consumed) = inert("aé".as_bytes(), 2);
        assert_eq!((shown.as_str(), consumed), ("a", 1));
        let (shown, consumed) = inert(b"\xff\xfe", 7);
        assert_eq!((shown.as_str(), consumed), ("\\xff", 1));
    }

    #[test]
    fn the_argument_area_is_split_at_nuls_and_kept_exactly() {
        let arguments = Arguments::from_bytes(b"prog\0-c\0\0a\nb\0");
        let raw: Vec<&[u8]> = arguments.entries().iter().map(Argument::raw).collect();
        assert_eq!(raw, [&b"prog"[..], b"-c", b"", b"a\nb"]);
        assert!(arguments.terminated() && arguments.counted_to_end());
        assert_eq!(arguments.total_bytes(), 13);
        assert!(arguments.entries().iter().all(Argument::is_whole));
        // Rewritten in place: one string, no final NUL.
        let rewritten = Arguments::from_bytes(b"nginx: worker process");
        assert!(!rewritten.terminated());
        assert_eq!(rewritten.entries().len(), 1);
        assert!(Arguments::from_bytes(b"").is_empty());
    }

    #[test]
    fn arguments_past_the_retention_bounds_are_counted_and_never_kept() {
        let mut area = Vec::new();
        for index in 0..MAX_RETAINED_ARGUMENTS + 10 {
            area.extend_from_slice(format!("arg{index}").as_bytes());
            area.push(0);
        }
        let many = Arguments::from_bytes(&area);
        assert_eq!(many.entries().len(), MAX_RETAINED_ARGUMENTS);
        assert_eq!(many.unretained(), 10);
        assert!(many.counted_to_end());

        let mut wide = vec![b'x'; MAX_RETAINED_ARGUMENT_BYTES + 100];
        wide.extend_from_slice(b"\0next\0");
        let wide = Arguments::from_bytes(&wide);
        assert_eq!(
            wide.entries().len(),
            1,
            "an argument past the kept bytes is counted"
        );
        assert_eq!(wide.entries()[0].raw().len(), MAX_RETAINED_ARGUMENT_BYTES);
        assert_eq!(
            wide.entries()[0].length(),
            (MAX_RETAINED_ARGUMENT_BYTES + 100) as u64
        );
        assert!(!wide.entries()[0].is_whole());
        assert_eq!(wide.unretained(), 1);

        let huge = vec![b'y'; MAX_ARGUMENT_SCAN_BYTES as usize + 10];
        let huge = Arguments::from_bytes(&huge);
        assert!(
            !huge.counted_to_end(),
            "past the scan bound, counts are lower bounds"
        );
        assert_eq!(huge.total_bytes(), MAX_ARGUMENT_SCAN_BYTES);
        let text = CommandContext {
            arguments: ArgumentsRead::Read(huge),
            executable: PathRead::Missing(MissingReason::Denied),
            working_directory: PathRead::Missing(MissingReason::Denied),
        }
        .text();
        assert!(
            text.contains("at least 1 argument, at least 2097152 bytes"),
            "{text}"
        );
        assert!(text.contains("argv[0] · at least 2097152 bytes, first 1024 shown · "));
    }

    #[test]
    fn the_view_is_bounded_and_every_cut_says_what_it_left_out() {
        let long = vec![b'z'; 5000];
        let wide = "\u{202e}".repeat(3000);
        let mut argv: Vec<&[u8]> = vec![b"prog", &long, wide.as_bytes()];
        let many: Vec<Vec<u8>> = (0..200).map(|i| vec![b'\x1b'; 40 + i]).collect();
        argv.extend(many.iter().map(Vec::as_slice));
        let long_path = vec![b'/'; 3000];
        let text = CommandContext::fixture(
            &argv,
            PathRead::server(&long_path),
            PathRead::server(b"/srv\n/x"),
        )
        .text();
        assert!(text.len() <= MAX_COMMAND_TEXT_BYTES, "{}", text.len());
        assert!(text.contains("argv[1] · 5000 bytes, first 1024 shown · zzz"));
        // Each override is shown as an 8-byte escape: 128 of them fit.
        assert!(text.contains("argv[2] · 9000 bytes, first 384 shown · \\u{202e}"));
        assert!(text.contains(" more arguments not shown ("), "{text}");
        assert!(text.contains("Executable on the server (3000 bytes, first 1024 shown): ///"));
        assert!(text.ends_with("Working directory on the server: /srv\\n/x"));
        assert_eq!(text.lines().count(), text.split('\n').count());
        // Every argument the view lists, plus those it counts, is every argument.
        let listed = text
            .lines()
            .filter(|line| line.starts_with("argv["))
            .count();
        let counted: usize = text
            .lines()
            .find_map(|line| line.strip_prefix("… "))
            .and_then(|rest| rest.split(' ').next())
            .and_then(|count| count.parse().ok())
            .unwrap();
        assert_eq!(listed + counted, 203);
    }

    #[test]
    fn sensitive_bytes_never_reach_a_debug_rendering() {
        let context = CommandContext::fixture(
            &[b"prog", b"--password=hunter2-SECRET"],
            PathRead::server(b"/home/secret-user"),
            PathRead::server(b"/home/secret-user"),
        );
        let debug = format!("{context:?} {context:#?}");
        assert!(
            !debug.contains("hunter2") && !debug.contains("secret-user"),
            "{debug}"
        );
        assert!(debug.contains("bytes withheld"));
        assert!(context.text().contains("--password=hunter2-SECRET"));
    }
}
