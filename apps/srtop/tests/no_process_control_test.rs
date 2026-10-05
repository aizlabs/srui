//! PX-008, the R0 release gate: a repeatable audit that keeps srtop from gaining a
//! way to signal or reconfigure a process (D1 §4 invariants 1 and 18, §7.7, §24;
//! D2 T21). It runs on every `cargo test`, so it constrains every later change.
//!
//! Three checks:
//!
//! * **Package shape, from Cargo.** `cargo metadata` lists every dependency the
//!   package declares — its real package name, whatever it is renamed to, of
//!   every kind and for every target — and every target. The normal
//!   dependencies are exactly tokio and the SRUI runtime; the only development
//!   dependency is the tokenizer this test uses; there is no build dependency,
//!   target-specific dependency, rename, `links`, `[patch]` or `[replace]`, no
//!   build script, and the library and binary are compiled from `src/`.
//! * **Sources, as Rust tokens.** Every `.rs` file under `src/` is read, and no
//!   symbolic link may stand there; with `#[path]` and `include!` refused below,
//!   that is every file the library and the binary compile. Each file is
//!   tokenized with proc-macro2, so comments, string, raw-string, byte-string
//!   and character literals can neither hide code nor be mistaken for it, and
//!   an item or statement gated by `#[cfg(test)]` is set aside token-exactly
//!   (`cfg(test)` exactly; any other cfg is read). The production tokens may not
//!   name: process creation or control (`Command`, `Child`, `kill`, `ptrace`,
//!   `setpriority`, `sched_setaffinity`, `setrlimit`, …); FFI, assembly, raw
//!   system calls or dynamic loading (`unsafe`, `extern`, `asm!`, `libc`,
//!   `nix`, `syscall`, `dlopen`, …); outbound sockets (`UnixStream`,
//!   `TcpStream`, `UdpSocket`, `connect`); the runtime's PTY facility (`pty`,
//!   `TerminalSpec`, …); or any handler for client input (`Session::on` in
//!   method or path form, `on_result`, `on_text_edit`, a model range
//!   provider). Files are opened only through an allowlist — `File::open` and
//!   the `std::fs` read functions srtop uses — and every way to obtain an
//!   `OpenOptions` (the only write, append, create or truncate builder) is
//!   refused: `OpenOptions`, `File::options`, `.options()`, any other `File::`
//!   or `fs::` item, a grouped, glob or renamed import of one, a type alias of
//!   `File`. `#[path]`, `#[link]`-style attributes and `include!` are refused.
//!   The only signal API is receiving SIGINT, SIGTERM and the `--smoke-fixture`
//!   SIGUSR1.
//! * **Binary imports.** `nm` lists the symbols the built `srtop` imports. None
//!   may create a process, deliver a targeted signal other than `kill`, trace a
//!   process, or change one's priority, affinity, limits, group or session. The
//!   binary does import `kill`, `waitpid` and `waitid` (and `syscall` on Linux);
//!   this check does not establish which code reaches them.
//!
//! What none of this can see is listed in the R0 release record: code inside the
//! allowed dependencies beyond the symbols they import, the arguments of a raw
//! system call a dependency makes, a release build, Cargo configuration outside
//! the package (`.cargo/config.toml`, `RUSTFLAGS`), and what another program
//! does to the same processes.
use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// Identifiers production code may not contain at all, in any position.
const FORBIDDEN_IDENTS: &[(&str, &str)] = &[
    // FFI, assembly, raw system calls, dynamic loading
    ("unsafe", "FFI or raw memory access"),
    ("extern", "FFI declarations or extern crates"),
    ("asm", "inline assembly"),
    ("global_asm", "inline assembly"),
    ("naked_asm", "inline assembly"),
    ("libc", "direct C library calls"),
    ("nix", "direct system calls"),
    ("rustix", "direct system calls"),
    ("syscall", "raw system calls"),
    ("dlopen", "dynamic loading"),
    ("dlsym", "dynamic loading"),
    ("libloading", "dynamic loading"),
    // creating, replacing or controlling a process
    ("Command", "spawns or configures a child process"),
    ("CommandExt", "spawns or replaces a process"),
    ("Child", "controls a spawned process"),
    ("fork", "creates a process"),
    ("vfork", "creates a process"),
    ("exec", "replaces a process image"),
    ("execv", "replaces a process image"),
    ("execve", "replaces a process image"),
    ("execvp", "replaces a process image"),
    ("posix_spawn", "creates a process"),
    ("kill", "signals a process"),
    ("killpg", "signals a process group"),
    ("tgkill", "signals a thread"),
    ("tkill", "signals a thread"),
    ("pidfd_open", "takes a process handle"),
    ("pidfd_send_signal", "signals a process"),
    ("sigqueue", "signals a process"),
    ("raise", "signals"),
    ("ptrace", "traces a process"),
    ("process_vm_writev", "writes another process's memory"),
    ("process_vm_readv", "reads another process's memory"),
    // reconfiguring a process
    ("setpriority", "re-prioritizes a process"),
    ("sched_setaffinity", "re-pins a process"),
    ("sched_setscheduler", "changes a scheduling policy"),
    ("sched_setparam", "changes a scheduling policy"),
    ("sched_setattr", "changes a scheduling policy"),
    ("setrlimit", "changes resource limits"),
    ("prlimit", "changes another process's limits"),
    ("ioprio_set", "changes I/O priority"),
    ("setpgid", "moves a process between groups"),
    ("setsid", "changes a session"),
    // writing files: every way to obtain the builder that opens one for writing
    ("OpenOptions", "opens a file for writing"),
    ("OpenOptionsExt", "opens a file for writing"),
    ("FileExt", "positional file writes"),
    // outbound sockets: a privileged daemon can control processes on request
    ("UnixStream", "connects to another program"),
    ("UnixDatagram", "connects to another program"),
    ("TcpStream", "connects to another program"),
    ("UdpSocket", "connects to another program"),
    ("connect", "connects to another program"),
    // the runtime's PTY facility, the one runtime path that spawns and signals
    ("pty", "the runtime's PTY children"),
    ("PTYManager", "the runtime's PTY children"),
    ("PTYManagerConfig", "the runtime's PTY children"),
    ("TerminalSpec", "spawns a PTY child"),
    ("create_terminal_node", "spawns a PTY child"),
    ("attach_terminals", "the runtime's PTY children"),
    ("srui_pty", "the runtime's PTY children"),
    // client input reaching application code
    ("on_result", "installs a client event handler"),
    (
        "on_result_with_transaction",
        "installs a client event handler",
    ),
    ("on_text_edit", "installs a client text-edit policy"),
    ("register_handler", "installs a client event handler"),
    ("ModelRangeProvider", "answers client range requests"),
    ("ModelRangeRequestInbox", "answers client range requests"),
    ("run_model_range_worker", "answers client range requests"),
];

/// Methods production code may not call: the `OpenOptions` builder and the
/// writes, resizes and permission changes of an open file. `append`, `truncate`
/// and `create` are also `OpenOptions` builders, but srtop calls them on vectors
/// and SDK builders; they are reachable on an `OpenOptions` only after one of the
/// refused ways to obtain it.
const FORBIDDEN_METHODS: &[&str] = &[
    "options",
    "write",
    "write_all",
    "write_at",
    "write_vectored",
    "set_len",
    "set_permissions",
    "set_times",
    "set_modified",
];

/// The `std::fs` and `std::os::unix::fs` items srtop may name: reads only.
const FS_ALLOWED: &[&str] = &[
    "File",
    "read_dir",
    "read_link",
    "metadata",
    "symlink_metadata",
    "MetadataExt",
];

/// The one `File` associated function srtop may call: a read-only open.
const FILE_ALLOWED: &[&str] = &["open"];

/// The signals `main` may receive. Sending one needs `kill` or `libc`.
const RECEIVED_SIGNALS: &[&str] = &["interrupt", "terminate", "user_defined1"];

/// Attributes that relocate code or link native symbols.
const FORBIDDEN_ATTRIBUTES: &[&str] = &[
    "path",
    "link",
    "link_name",
    "link_section",
    "no_mangle",
    "export_name",
];

/// The package's declared dependencies, by real package name.
const NORMAL_DEPENDENCIES: &[&str] = &[
    "srui-protocol",
    "srui-sdk",
    "srui-semantic-tree",
    "srui-sessiond",
    "srui-unix-security",
    "tokio",
    "tokio-util",
];
const DEV_DEPENDENCIES: &[&str] = &["proc-macro2"];

/// Imports that would let the binary create, signal, trace or reconfigure a
/// process. None may be present.
const FORBIDDEN_IMPORTS: &[&str] = &[
    // creating a process or replacing its image
    "fork",
    "vfork",
    "clone",
    "clone3",
    "execve",
    "execv",
    "execvp",
    "execvpe",
    "execl",
    "execlp",
    "execle",
    "fexecve",
    "posix_spawn",
    "posix_spawnp",
    "system",
    "popen",
    // targeted signals other than `kill`
    "killpg",
    "tgkill",
    "tkill",
    "pidfd_send_signal",
    "pidfd_open",
    "sigqueue",
    "pthread_kill",
    "raise",
    // tracing or another process's memory
    "ptrace",
    "process_vm_writev",
    "process_vm_readv",
    "task_for_pid",
    // reconfiguring a process
    "setpriority",
    "nice",
    "sched_setaffinity",
    "sched_setscheduler",
    "sched_setparam",
    "sched_setattr",
    "setrlimit",
    "setrlimit64",
    "prlimit",
    "prlimit64",
    "ioprio_set",
    "setpgid",
    "setsid",
    "setpgrp",
    "thread_policy_set",
    "task_policy_set",
    "setiopolicy_np",
    "proc_rlimit_control",
];

/// Process-related imports the binary has today. They are reported, not
/// attributed: this test does not establish which code reaches them.
const UNATTRIBUTED_IMPORTS: &[&str] = &["kill", "waitpid", "waitid", "syscall"];

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// One token of a source file, with `::` joined and groups nested.
#[derive(Debug, Clone)]
enum Tok {
    /// The identifier without any `r#`, whether it was raw, and its line.
    Ident(String, bool, usize),
    Punct(char, usize),
    PathSep(usize),
    Literal(usize),
    Group(Delimiter, Vec<Tok>, usize),
}

fn lower(stream: TokenStream) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut trees = stream.into_iter().peekable();
    while let Some(tree) = trees.next() {
        let line = tree.span().start().line;
        match tree {
            TokenTree::Ident(ident) => {
                let text = ident.to_string();
                let raw = text.starts_with("r#");
                out.push(Tok::Ident(
                    text.trim_start_matches("r#").to_string(),
                    raw,
                    line,
                ));
            }
            TokenTree::Punct(punct) => {
                let joint = punct.spacing() == Spacing::Joint;
                if punct.as_char() == ':' && joint {
                    if let Some(TokenTree::Punct(next)) = trees.peek() {
                        if next.as_char() == ':' {
                            trees.next();
                            out.push(Tok::PathSep(line));
                            continue;
                        }
                    }
                }
                out.push(Tok::Punct(punct.as_char(), line));
            }
            TokenTree::Literal(_) => out.push(Tok::Literal(line)),
            TokenTree::Group(group) => {
                out.push(Tok::Group(group.delimiter(), lower(group.stream()), line))
            }
        }
    }
    out
}

fn ident(tok: Option<&Tok>) -> Option<&str> {
    match tok {
        Some(Tok::Ident(name, _, _)) => Some(name),
        _ => None,
    }
}

fn is_punct(tok: Option<&Tok>, ch: char) -> bool {
    matches!(tok, Some(Tok::Punct(c, _)) if *c == ch)
}

fn is_path_sep(tok: Option<&Tok>) -> bool {
    matches!(tok, Some(Tok::PathSep(_)))
}

fn line_of(tok: &Tok) -> usize {
    match tok {
        Tok::Ident(_, _, line)
        | Tok::Punct(_, line)
        | Tok::PathSep(line)
        | Tok::Literal(line)
        | Tok::Group(_, _, line) => *line,
    }
}

/// `cfg(test)`, exactly: anything else (`cfg(not(test))`, `cfg(any(test, x))`)
/// is production code and is read.
fn is_cfg_test(attribute: &[Tok]) -> bool {
    match attribute {
        [Tok::Ident(cfg, false, _), Tok::Group(Delimiter::Parenthesis, inner, _)] => {
            cfg == "cfg"
                && matches!(inner.as_slice(), [Tok::Ident(test, false, _)] if test == "test")
        }
        _ => false,
    }
}

const ITEM_KEYWORDS: &[&str] = &[
    "fn",
    "mod",
    "impl",
    "struct",
    "enum",
    "trait",
    "union",
    "const",
    "static",
    "type",
    "use",
    "macro_rules",
];

/// The tokens production builds compile: every item or statement gated by
/// `#[cfg(test)]` removed. An item ends at its `;` or through its first
/// brace-delimited body; any other gated statement, field or element also ends
/// at a `,` of its own level, so a gate can only ever hide what it really gates
/// (and may leave a little test-only code to be read, never the reverse).
fn production(level: &[Tok]) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < level.len() {
        // `#![cfg(test)]` gates the rest of the enclosing file or module.
        if is_punct(level.get(i), '#') && is_punct(level.get(i + 1), '!') {
            if let Some(Tok::Group(Delimiter::Bracket, attribute, _)) = level.get(i + 2) {
                if is_cfg_test(attribute) {
                    break;
                }
            }
        }
        if is_punct(level.get(i), '#') {
            if let Some(Tok::Group(Delimiter::Bracket, attribute, _)) = level.get(i + 1) {
                if is_cfg_test(attribute) {
                    let mut j = i + 2;
                    // Further attributes of the same item.
                    while is_punct(level.get(j), '#')
                        && matches!(level.get(j + 1), Some(Tok::Group(Delimiter::Bracket, ..)))
                    {
                        j += 2;
                    }
                    let mut k = j;
                    // Visibility and qualifiers before an item keyword.
                    loop {
                        match level.get(k) {
                            Some(Tok::Ident(word, false, _))
                                if ["pub", "async", "default"].contains(&word.as_str()) =>
                            {
                                k += 1
                            }
                            Some(Tok::Group(Delimiter::Parenthesis, ..)) if k > j => k += 1,
                            _ => break,
                        }
                    }
                    let is_item = matches!(
                        level.get(k),
                        Some(Tok::Ident(word, false, _)) if ITEM_KEYWORDS.contains(&word.as_str())
                    );
                    while j < level.len() {
                        match &level[j] {
                            Tok::Punct(';', _) => {
                                j += 1;
                                break;
                            }
                            Tok::Punct(',', _) if !is_item => {
                                j += 1;
                                break;
                            }
                            Tok::Group(Delimiter::Brace, ..) => {
                                j += 1;
                                break;
                            }
                            _ => j += 1,
                        }
                    }
                    i = j;
                    continue;
                }
            }
        }
        out.push(match &level[i] {
            Tok::Group(delimiter, inner, line) => Tok::Group(*delimiter, production(inner), *line),
            other => other.clone(),
        });
        i += 1;
    }
    out
}

/// Every name a grouped import from `fs` brings in must be allowed, never
/// renamed and never a glob: `use std::fs::{metadata, write}` is refused.
fn check_fs_group(group: &[Tok], report: &mut dyn FnMut(usize, &str, String)) {
    for (i, tok) in group.iter().enumerate() {
        match tok {
            Tok::Ident(name, _, line) if name == "as" => {
                report(*line, "fs-import", "a renamed import from fs".into())
            }
            Tok::Ident(name, _, line) => {
                let renamed_to = ident(i.checked_sub(1).and_then(|p| group.get(p))) == Some("as");
                if !renamed_to && name != "self" && !FS_ALLOWED.contains(&name.as_str()) {
                    report(*line, "fs-import", format!("fs::{{{name}}} is not a read"));
                }
            }
            Tok::Punct('*', line) => report(*line, "fs-import", "a glob import from fs".into()),
            Tok::Group(_, inner, _) => check_fs_group(inner, report),
            _ => {}
        }
    }
}

fn contains_ident(level: &[Tok], names: &[&str]) -> bool {
    level.iter().any(|tok| match tok {
        Tok::Ident(name, _, _) => names.contains(&name.as_str()),
        Tok::Group(_, inner, _) => contains_ident(inner, names),
        _ => false,
    })
}

fn scan(level: &[Tok], report: &mut dyn FnMut(usize, &str, String)) {
    for (i, tok) in level.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| level.get(p));
        let next = level.get(i + 1);
        let after = level.get(i + 2);
        match tok {
            Tok::Ident(name, raw, line) => {
                let line = *line;
                if let Some((_, why)) = FORBIDDEN_IDENTS.iter().find(|(word, _)| word == name) {
                    report(line, "ident", format!("`{name}`: {why}"));
                }
                // `.name(` or `.name::<T>(`
                if is_punct(prev, '.')
                    && FORBIDDEN_METHODS.contains(&name.as_str())
                    && (matches!(next, Some(Tok::Group(Delimiter::Parenthesis, ..)))
                        || is_path_sep(next))
                {
                    report(
                        line,
                        "method",
                        format!("`.{name}(..)` writes or opens for writing"),
                    );
                }
                match name.as_str() {
                    // `x.on(..)` and `Session::on(&session, ..)`: a client event handler
                    "on" if is_punct(prev, '.') || is_path_sep(prev) => report(
                        line,
                        "handler",
                        "`on`: installs a client event handler".into(),
                    ),
                    "fs" => {
                        if is_path_sep(next) {
                            match after {
                                Some(Tok::Ident(member, _, _))
                                    if !FS_ALLOWED.contains(&member.as_str()) =>
                                {
                                    report(line, "fs", format!("`fs::{member}` is not a read"))
                                }
                                Some(Tok::Group(Delimiter::Brace, group, _)) => {
                                    check_fs_group(group, report)
                                }
                                Some(Tok::Punct('*', _)) => {
                                    report(line, "fs", "a glob import from fs".into())
                                }
                                _ => {}
                            }
                        }
                        if ident(next) == Some("as") {
                            report(line, "fs", "`fs` renamed".into());
                        }
                    }
                    "File" => {
                        // `File::x`, and the qualified `<File>::x`
                        let member = if is_path_sep(next) {
                            ident(after)
                        } else if is_punct(next, '>') && is_path_sep(after) {
                            ident(level.get(i + 3))
                        } else {
                            None
                        };
                        if let Some(member) = member {
                            if !FILE_ALLOWED.contains(&member) {
                                report(line, "file", format!("`File::{member}` is not a read"));
                            }
                        }
                        if ident(next) == Some("as") {
                            report(line, "file", "`File` renamed".into());
                        }
                    }
                    "SignalKind" => {
                        if is_path_sep(next) {
                            if let Some(kind) = ident(after) {
                                if !RECEIVED_SIGNALS.contains(&kind) {
                                    report(line, "signal", format!("`SignalKind::{kind}`"));
                                }
                            }
                        }
                    }
                    // `type F = std::fs::File;` would let `F::create` pass the `File` rule.
                    "type" if !*raw => {
                        let end = level[i..]
                            .iter()
                            .position(|tok| matches!(tok, Tok::Punct(';', _)))
                            .map_or(level.len(), |offset| i + offset);
                        if contains_ident(&level[i + 1..end], &["File", "fs", "OpenOptions"]) {
                            report(line, "type-alias", "a type alias of a file type".into());
                        }
                    }
                    "include" if is_punct(next, '!') => report(
                        line,
                        "include",
                        "`include!` brings in code from elsewhere".into(),
                    ),
                    _ => {}
                }
            }
            Tok::Punct('#', line) => {
                let attribute = if is_punct(next, '!') { after } else { next };
                if let Some(Tok::Group(Delimiter::Bracket, inner, _)) = attribute {
                    if let Some(name) = ident(inner.first()) {
                        if FORBIDDEN_ATTRIBUTES.contains(&name) {
                            report(*line, "attribute", format!("`#[{name}]`"));
                        }
                    }
                }
            }
            Tok::Group(_, inner, _) => scan(inner, report),
            _ => {}
        }
    }
}

/// Every violation in the production tokens of one source file.
fn token_violations(file: &str, source: &str) -> Vec<String> {
    let stream = match TokenStream::from_str(source) {
        Ok(stream) => stream,
        Err(error) => return vec![format!("{file}: does not tokenize: {error}")],
    };
    let tokens = production(&lower(stream));
    let mut found = Vec::new();
    scan(&tokens, &mut |line, rule, what| {
        found.push(format!("{file}:{line}: [{rule}] {what}"))
    });
    found
}

/// How many `SignalKind` receivers the production tokens name.
fn signal_receivers(source: &str) -> usize {
    fn count(level: &[Tok]) -> usize {
        level
            .iter()
            .map(|tok| match tok {
                Tok::Ident(name, _, _) if name == "SignalKind" => 1,
                Tok::Group(_, inner, _) => count(inner),
                _ => 0,
            })
            .sum()
    }
    count(&production(&lower(
        TokenStream::from_str(source).expect("a source that tokenizes"),
    )))
}

// ---------------------------------------------------------------------------
// cargo metadata
// ---------------------------------------------------------------------------

/// Just enough JSON for `cargo metadata`.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> Result<Json, String> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = parser.value()?;
        parser.skip_space();
        if parser.at != parser.bytes.len() {
            return Err(format!("trailing data at byte {}", parser.at));
        }
        Ok(value)
    }

    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map_or(&Json::Null, |(_, value)| value),
            _ => &Json::Null,
        }
    }

    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(text) => Some(text),
            _ => None,
        }
    }

    fn items(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => &[],
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl JsonParser<'_> {
    fn skip_space(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn eat(&mut self, literal: &str) -> bool {
        if self.bytes[self.at..].starts_with(literal.as_bytes()) {
            self.at += literal.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_space();
        match self.bytes.get(self.at) {
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.eat("]") {
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    self.skip_space();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("]") {
                        return Ok(Json::Array(items));
                    }
                    return Err(format!("expected , or ] at byte {}", self.at));
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut members = Vec::new();
                self.skip_space();
                if self.eat("}") {
                    return Ok(Json::Object(members));
                }
                loop {
                    self.skip_space();
                    let key = self.string()?;
                    self.skip_space();
                    if !self.eat(":") {
                        return Err(format!("expected : at byte {}", self.at));
                    }
                    members.push((key, self.value()?));
                    self.skip_space();
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("}") {
                        return Ok(Json::Object(members));
                    }
                    return Err(format!("expected , or }} at byte {}", self.at));
                }
            }
            Some(byte) if *byte == b'-' || byte.is_ascii_digit() => {
                let start = self.at;
                while self.at < self.bytes.len()
                    && matches!(
                        self.bytes[self.at],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.at += 1;
                }
                Ok(Json::Number(
                    String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned(),
                ))
            }
            _ => Err(format!("unexpected input at byte {}", self.at)),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if !self.eat("\"") {
            return Err(format!("expected a string at byte {}", self.at));
        }
        let mut out: Vec<u16> = Vec::new();
        let mut text = String::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return Err("unterminated string".into());
            };
            self.at += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let escape = *self.bytes.get(self.at).ok_or("unterminated escape")?;
                    self.at += 1;
                    let unit = match escape {
                        b'"' => u16::from(b'"'),
                        b'\\' => u16::from(b'\\'),
                        b'/' => u16::from(b'/'),
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => u16::from(b'\n'),
                        b'r' => u16::from(b'\r'),
                        b't' => u16::from(b'\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.at..self.at + 4)
                                .ok_or("short \\u escape")?;
                            self.at += 4;
                            u16::from_str_radix(&String::from_utf8_lossy(hex), 16)
                                .map_err(|error| error.to_string())?
                        }
                        other => return Err(format!("bad escape \\{}", other as char)),
                    };
                    out.push(unit);
                }
                _ => {
                    // Flush pending UTF-16 escapes, then copy this UTF-8 byte run.
                    if !out.is_empty() {
                        text.push_str(&String::from_utf16_lossy(&out));
                        out.clear();
                    }
                    let start = self.at - 1;
                    while self.at < self.bytes.len() && !matches!(self.bytes[self.at], b'"' | b'\\')
                    {
                        self.at += 1;
                    }
                    text.push_str(&String::from_utf8_lossy(&self.bytes[start..self.at]));
                }
            }
        }
        if !out.is_empty() {
            text.push_str(&String::from_utf16_lossy(&out));
        }
        Ok(text)
    }
}

/// Every way the package's declaration falls outside what this gate allows, from
/// `cargo metadata --no-deps` output and the manifest's own text.
fn package_violations(metadata: &str, manifest: &str, root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let document = match Json::parse(metadata) {
        Ok(document) => document,
        Err(error) => return vec![format!("cargo metadata did not parse: {error}")],
    };
    let Some(package) = document
        .get("packages")
        .items()
        .iter()
        .find(|package| package.get("name").str() == Some("srui-process-explorer"))
    else {
        return vec!["cargo metadata does not describe srui-process-explorer".into()];
    };
    if package.get("links") != &Json::Null {
        found.push("the package links a native library".into());
    }
    let mut normal = BTreeSet::new();
    for dependency in package.get("dependencies").items() {
        let name = dependency.get("name").str().unwrap_or("?").to_string();
        let kind = dependency.get("kind").str();
        if let Some(rename) = dependency.get("rename").str() {
            found.push(format!("dependency {name} is renamed to {rename}"));
        }
        if let Some(target) = dependency.get("target").str() {
            found.push(format!("dependency {name} is declared only for {target}"));
        }
        match kind {
            None => {
                if !NORMAL_DEPENDENCIES.contains(&name.as_str()) {
                    found.push(format!(
                        "normal dependency {name} is not the runtime or tokio"
                    ));
                }
                normal.insert(name);
            }
            Some("dev") if DEV_DEPENDENCIES.contains(&name.as_str()) => {}
            Some(kind) => found.push(format!("{kind} dependency {name} is not allowed")),
        }
    }
    let expected: BTreeSet<String> = NORMAL_DEPENDENCIES.iter().map(|n| n.to_string()).collect();
    for missing in expected.difference(&normal) {
        found.push(format!("normal dependency {missing} is missing"));
    }
    let src = root.join("src");
    for target in package.get("targets").items() {
        let kinds: Vec<&str> = target
            .get("kind")
            .items()
            .iter()
            .filter_map(Json::str)
            .collect();
        let name = target.get("name").str().unwrap_or("?");
        let path = PathBuf::from(target.get("src_path").str().unwrap_or(""));
        if kinds.contains(&"custom-build") {
            found.push(format!(
                "the package has a build script ({})",
                path.display()
            ));
        }
        let production = kinds.iter().any(|kind| {
            [
                "lib",
                "rlib",
                "dylib",
                "cdylib",
                "staticlib",
                "proc-macro",
                "bin",
            ]
            .contains(kind)
        });
        if production && !path.starts_with(&src) {
            found.push(format!(
                "target {name} is compiled from {} outside src/",
                path.display()
            ));
        }
    }
    for line in manifest.lines().map(str::trim) {
        if line.starts_with("[patch") || line.starts_with("[replace") {
            found.push(format!("the manifest overrides a dependency: {line}"));
        }
    }
    found
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `directory`, and every symbolic link there: a link would
/// let code the audit reads as `src/` live anywhere, so none is allowed.
fn rust_files(directory: &Path, files: &mut Vec<PathBuf>, links: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .expect("the source directory is readable")
        .map(|entry| entry.expect("a readable entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let kind = std::fs::symlink_metadata(&path)
            .expect("a readable entry")
            .file_type();
        if kind.is_symlink() {
            links.push(path);
        } else if kind.is_dir() {
            rust_files(&path, files, links);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// Self-tests: every bypass the reviews found, and the innocent look-alikes
// ---------------------------------------------------------------------------

fn reported(snippet: &str, rule: &str) -> bool {
    token_violations("snippet.rs", snippet)
        .iter()
        .any(|violation| violation.contains(&format!("[{rule}]")))
}

/// Every construct the source audit exists for is reported, including each one
/// that got past the first version of this test (review probes P1–P7, verifier
/// mutants N1–N8); look-alikes srtop really uses are not; comments and literals
/// of every kind neither hide code nor count as code.
#[test]
fn the_source_audit_reports_every_known_bypass() {
    let bypasses: &[(&str, &str, &str)] = &[
        // writing a file another way than the visible ones (P1, N4b)
        (
            "P1 File::options",
            "std::fs::File::options().write(true).open(format!(\"/proc/{target}/oom_score_adj\"))?.write_all(b\"1000\")?;",
            "file",
        ),
        (
            "P1 cgroup.kill",
            "std::fs::File::options().write(true).open(\"/sys/fs/cgroup/x/cgroup.kill\")?;",
            "method",
        ),
        ("the .options() method", "let opts = file_type.options();", "method"),
        ("OpenOptions", "let o = std::fs::OpenOptions::new();", "ident"),
        ("OpenOptionsExt", "use std::os::unix::fs::OpenOptionsExt;", "ident"),
        ("File::create", "let f = File::create(path)?;", "file"),
        ("File::create_new", "let f = std::fs::File::create_new(path)?;", "file"),
        ("qualified <File>::create", "let f = <std::fs::File>::create(path)?;", "file"),
        ("File renamed", "use std::fs::File as Handle;", "file"),
        ("type alias of File", "type Handle = std::fs::File;", "type-alias"),
        ("fs::write (N4a)", "std::fs::write(\"/proc/1/oom_score_adj\", \"1000\")?;", "fs"),
        ("tokio::fs::write", "tokio::fs::write(path, b\"1\").await?;", "fs"),
        ("fs::set_permissions", "fs::set_permissions(path, perms)?;", "fs"),
        ("std::os::unix::fs::chown", "std::os::unix::fs::chown(path, None, None)?;", "fs"),
        (
            "grouped import (N4c)",
            "use std::{fs::{metadata, write}, path::PathBuf};",
            "fs-import",
        ),
        ("nested grouped import", "use std::fs::{self, write as w};", "fs-import"),
        ("glob import", "use std::fs::*;", "fs"),
        ("fs renamed", "use std::fs as filesystem;", "fs"),
        ("fs renamed in a group", "use std::{fs as filesystem, io};", "fs"),
        ("file write_all", "handle.write_all(b\"1\")?;", "method"),
        ("file set_len", "handle.set_len(0)?;", "method"),
        // `//` inside a string literal no longer cuts the line (P2, P2b)
        (
            "P2 // in a string",
            "let _ = (\"https://\", std::fs::write(\"/proc/1/oom_score_adj\", \"1000\"));",
            "fs",
        ),
        (
            "P2b // hides Command",
            "let _ = (\"//\", std::process::Command::new(\"/bin/true\").status());",
            "ident",
        ),
        (
            "raw string then code",
            "let _ = (r#\"//\"#, std::process::Command::new(\"x\"));",
            "ident",
        ),
        (
            "char literal then code",
            "let _ = ('\"', std::fs::write(\"x\", \"y\"));",
            "fs",
        ),
        (
            "byte string then code",
            "let _ = (b\"//\", std::fs::write(\"x\", \"y\"));",
            "fs",
        ),
        // code that moves its source out of reach (P3, N7a, N7b, P7 via dep-info)
        ("P3 #[path]", "#[path = \"../probe_outside.rs\"]\nmod outside;", "attribute"),
        ("include!", "include!(\"../outside.rs\");", "include"),
        ("#[link]", "#[link(name = \"c\")]\nextern \"C\" {}", "attribute"),
        ("#[no_mangle]", "#[no_mangle]\npub fn kill_hook() {}", "attribute"),
        // a `#[cfg(test)]` inside a string literal gates nothing (P5)
        (
            "P5 cfg(test) in a string",
            "pub const NOTE: &str = \"\n#[cfg(test)]\n\";\nmod hidden { pub fn go() { std::fs::write(\"/proc/1/oom_score_adj\", \"1\").ok(); } }",
            "fs",
        ),
        // FFI, assembly, raw system calls, dynamic loading (N2, N5a, N5b)
        ("unsafe libc::kill (N2)", "let _ = unsafe { libc::kill(pid, 15) };", "ident"),
        (
            "extern block (N5a)",
            "extern \"C\" { fn setpriority(which: i32, who: u32, prio: i32) -> i32; }",
            "ident",
        ),
        (
            "libc::setpriority (N5b)",
            "libc::setpriority(libc::PRIO_PROCESS, 0, 19);",
            "ident",
        ),
        ("asm!", "std::arch::asm!(\"svc 0\");", "ident"),
        ("syscall", "let _ = syscall(62, pid, 9);", "ident"),
        ("dlopen", "let handle = dlopen(name, 1);", "ident"),
        // signals (N3, N6, P6)
        (
            "nix kill (N3)",
            "let _ = nix::sys::signal::kill(Pid::from_raw(pid), None);",
            "ident",
        ),
        (
            "renamed crate, kill as a value (N6)",
            "let send = ostools::sys::signal::kill;\nlet _ = send(Pid::from_raw(pid), None);",
            "ident",
        ),
        (
            "renamed crate, kill renamed (P6)",
            "use posix::sys::signal::{kill as deliver, Signal};",
            "ident",
        ),
        ("raw identifier r#kill", "let _ = posix::r#kill(pid, 15);", "ident"),
        ("receiving SIGHUP", "let hup = signal(SignalKind::hangup())?;", "signal"),
        ("process spawn (N1)", "std::process::Command::new(\"/bin/true\").status()?;", "ident"),
        ("setpriority", "setpriority(0, pid, 19);", "ident"),
        ("sched_setaffinity", "sched_setaffinity (pid, size, set);", "ident"),
        // the runtime's own process APIs (P4)
        (
            "P4 session.pty().spawn",
            "let _ = session.pty().spawn(NodeId::new(99), Default::default());",
            "ident",
        ),
        ("create_terminal_node", "session.create_terminal_node(id, spec)?;", "ident"),
        // client input reaching application code (N8)
        ("session.on", "session.on(TABLE, ACTIVATE, |_, _| {});", "handler"),
        (
            "Session::on as a plain call (N8)",
            "Session::on(&session, TABLE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        ("on as a value", "let install = Session::on;", "handler"),
        ("on_result", "session.on_result(TABLE, ACTIVATE, |_, _| Ok(()));", "ident"),
        ("model range provider", "impl ModelRangeProvider for Rows {}", "ident"),
        // outbound connections
        ("UnixStream::connect", "let s = UnixStream::connect(path).await?;", "ident"),
        ("TcpStream", "let s = std::net::TcpStream::connect(addr)?;", "ident"),
    ];
    let mut missed = Vec::new();
    for (name, snippet, rule) in bypasses {
        if !reported(snippet, rule) {
            missed.push(format!(
                "{name}: not reported as [{rule}]: {:?}",
                token_violations("snippet.rs", snippet)
            ));
        }
    }
    assert!(
        missed.is_empty(),
        "bypasses the audit misses:\n{}",
        missed.join("\n")
    );

    // What srtop really writes, and comments and literals of every kind.
    for innocent in [
        "// a comment may say kill(2), Command and fs::write",
        "/* and so may a block comment: std::fs::write(\"x\", \"y\") */",
        "/// a doc comment: `File::create` and `session.on(..)`\nfn documented() {}",
        "let url = \"https://example.org\";",
        "let quote = '\"';",
        "let raw = r#\"fs::write(path, kill)\"#;",
        "let bytes = b\"Command kill\";",
        "let [user, nice, system] = counts;",
        "let _ = write!(text, \"{byte:02x}\");",
        "let id = std::process::id();",
        "let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;",
        "connections.spawn(poll(view, source, session, interval, token));",
        "let sampled = tokio::task::spawn_blocking(move || source.snapshot()).await;",
        "let token = shutdown.child_token();",
        "let kind = std::fs::symlink_metadata(&path)?;",
        "use std::fs::File;",
        "use std::os::unix::fs::MetadataExt;",
        "let pin: Option<File> = None;",
        "fn open_directory(path: &Path) -> io::Result<File> { File::open(path.join(\".\")) }",
        "let entries = fs::read_dir(root)?; let target = fs::read_link(link)?; let m = fs::metadata(p)?;",
        "rows.truncate(kept); merged.append(&mut tail);",
        "Surface::builder(1).label(\"Processes\").create(ui)?;",
        "pub type SkippedRecords = PidLedger;",
        "pub fn system() -> SystemSample { todo!() }",
    ] {
        assert_eq!(
            token_violations("innocent.rs", innocent),
            Vec::<String>::new(),
            "{innocent}"
        );
    }

    // Test code is set aside token-exactly, in every form srtop uses; code after
    // a test module, and code a non-test cfg gates, is still read.
    let gated = "pub fn live() {}\n#[cfg(test)]\nmod tests {\n    fn t() { std::process::Command::new(\"x\"); }\n}\n\
                 #[cfg(test)]\npub(crate) fn tracked(&self) -> usize { kill(1) }\n\
                 fn f() { #[cfg(test)]\n fault::check(&path, \"open\")?; let x = Rec { base,\n #[cfg(test)]\n record }; }";
    assert_eq!(token_violations("gated.rs", gated), Vec::<String>::new());
    let after = "#[cfg(test)]\nmod tests {}\npub fn late() { std::fs::write(\"x\", \"y\").ok(); }";
    assert!(
        reported(after, "fs"),
        "code after a test module is production code"
    );
    let other_cfg = "#[cfg(not(test))]\nfn f() { kill(1, 9); }";
    assert!(reported(other_cfg, "ident"), "only cfg(test) is set aside");
    let element = "let v = Rec { #[cfg(test)] a: 1, b: std::fs::write(\"x\", \"y\") };";
    assert!(reported(element, "fs"), "a gated field gates only itself");
}

/// The package checks catch a dependency however it is declared (P6, N6), an
/// entry point moved out of `src/` (P7), a build script, an override, and a
/// symbolic link under `src/`.
#[test]
fn the_package_checks_report_every_known_bypass() {
    let root = Path::new("/repo/apps/srtop");
    let manifest = "[package]\nname = \"srui-process-explorer\"\n";
    let metadata = |dependencies: &str, targets: &str| {
        format!(
            "{{\"packages\":[{{\"name\":\"srui-process-explorer\",\"links\":null,\
             \"dependencies\":[{dependencies}],\"targets\":[{targets}]}}],\"version\":1}}"
        )
    };
    let dependency = |name: &str, rename: &str, kind: &str, target: &str| {
        format!(
            "{{\"name\":\"{name}\",\"rename\":{rename},\"kind\":{kind},\"target\":{target},\"optional\":false}}"
        )
    };
    let runtime: Vec<String> = NORMAL_DEPENDENCIES
        .iter()
        .map(|name| dependency(name, "null", "null", "null"))
        .collect();
    let target = |kind: &str, path: &str| {
        format!("{{\"kind\":[\"{kind}\"],\"name\":\"t\",\"src_path\":\"{path}\"}}")
    };
    let good_targets = [
        target("lib", "/repo/apps/srtop/src/lib.rs"),
        target("bin", "/repo/apps/srtop/src/main.rs"),
        target("test", "/repo/apps/srtop/tests/shutdown.rs"),
    ]
    .join(",");
    let with = |extra: &str| {
        let mut all = runtime.clone();
        if !extra.is_empty() {
            all.push(extra.to_string());
        }
        all.join(",")
    };
    assert_eq!(
        package_violations(&metadata(&with(""), &good_targets), manifest, root),
        Vec::<String>::new()
    );
    for (name, dependencies, targets, manifest_text, expected) in [
        (
            "P6 [dependencies.posix] package = nix",
            with(&dependency("nix", "\"posix\"", "null", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "renamed to posix",
        ),
        (
            "N6 [dependencies.ostools] package = nix",
            with(&dependency("nix", "\"ostools\"", "null", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "normal dependency nix",
        ),
        (
            "N2 libc",
            with(&dependency("libc", "null", "null", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "normal dependency libc",
        ),
        (
            "a target-specific dependency",
            with(&dependency("nix", "null", "null", "\"cfg(unix)\"")),
            good_targets.clone(),
            manifest.to_string(),
            "declared only for cfg(unix)",
        ),
        (
            "a build dependency",
            with(&dependency("cc", "null", "\"build\"", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "build dependency cc",
        ),
        (
            "another dev dependency",
            with(&dependency("nix", "null", "\"dev\"", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "dev dependency nix",
        ),
        (
            "P7 the binary moved out of src/",
            with(""),
            [
                target("lib", "/repo/apps/srtop/src/lib.rs"),
                target("bin", "/repo/apps/srtop/bin/srtop.rs"),
            ]
            .join(","),
            manifest.to_string(),
            "outside src/",
        ),
        (
            "a build script",
            with(""),
            [
                good_targets.clone(),
                target("custom-build", "/repo/apps/srtop/build.rs"),
            ]
            .join(","),
            manifest.to_string(),
            "build script",
        ),
        (
            "a [patch] of an allowed crate",
            with(""),
            good_targets.clone(),
            format!("{manifest}[patch.crates-io]\ntokio = {{ path = \"../tokio\" }}\n"),
            "overrides a dependency",
        ),
    ] {
        let found = package_violations(&metadata(&dependencies, &targets), &manifest_text, root);
        assert!(
            found.iter().any(|violation| violation.contains(expected)),
            "{name}: expected {expected:?} in {found:?}"
        );
    }

    // A symbolic link under src/ would let audited code live anywhere.
    let scratch = Path::new(env!("CARGO_TARGET_TMPDIR")).join("px008-audit-self-test");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("src/inner")).expect("a scratch tree");
    std::fs::write(scratch.join("src/lib.rs"), "mod inner;").expect("a scratch file");
    std::fs::write(scratch.join("src/inner/mod.rs"), "").expect("a scratch file");
    std::fs::write(scratch.join("outside.rs"), "").expect("a scratch file");
    std::os::unix::fs::symlink("../outside.rs", scratch.join("src/linked.rs"))
        .expect("a scratch link");
    let (mut files, mut links) = (Vec::new(), Vec::new());
    rust_files(&scratch.join("src"), &mut files, &mut links);
    assert_eq!(
        (files.len(), links),
        (2, vec![scratch.join("src/linked.rs")]),
        "a link under src/ is reported, real files are read"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

// ---------------------------------------------------------------------------
// The real package
// ---------------------------------------------------------------------------

#[test]
fn the_package_declares_only_the_runtime_and_builds_only_from_src() {
    let root = crate_root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| env!("CARGO").to_string());
    let output = Command::new(&cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("cargo metadata runs; this check is never skipped");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata = String::from_utf8(output.stdout).expect("cargo metadata is UTF-8");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("the manifest");
    let found = package_violations(&metadata, &manifest, &root);
    assert!(
        found.is_empty(),
        "package violations:\n{}",
        found.join("\n")
    );
    println!(
        "PX-008 package audit: dependencies {NORMAL_DEPENDENCIES:?} (dev {DEV_DEPENDENCIES:?}), \
         no rename, target-specific or build dependency, no [patch] or [replace], no build \
         script; library and binary compiled from src/"
    );
}

#[test]
fn no_production_source_can_signal_or_reconfigure_a_process() {
    let root = crate_root();
    let (mut files, mut links) = (Vec::new(), Vec::new());
    rust_files(&root.join("src"), &mut files, &mut links);
    assert!(
        links.is_empty(),
        "symbolic links under src/ would let audited code live elsewhere: {links:?}"
    );
    assert!(
        files.iter().any(|path| path.ends_with("src/main.rs")),
        "the audit must cover the binary's entry point"
    );
    let mut found = Vec::new();
    let mut receivers = 0;
    for path in &files {
        let text = std::fs::read_to_string(path).expect("a readable source file");
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        found.extend(token_violations(&name, &text));
        receivers += signal_receivers(&text);
    }
    assert!(
        found.is_empty(),
        "process control in production code:\n{}",
        found.join("\n")
    );
    // The exceptions, stated: `main` receives SIGINT and SIGTERM to remove its
    // own socket, and SIGUSR1 only under `--smoke-fixture`, to retitle the shell.
    assert_eq!(receivers, RECEIVED_SIGNALS.len());
    println!(
        "PX-008 source audit: {} files read as tokens, 0 violations; signal receivers \
         {RECEIVED_SIGNALS:?}; file access through {FILE_ALLOWED:?} and fs {FS_ALLOWED:?} only",
        files.len()
    );
}

#[test]
fn the_binary_imports_no_process_creation_targeted_signal_or_reconfiguration_function() {
    let binary = env!("CARGO_BIN_EXE_srtop");
    let arguments: &[&str] = if cfg!(target_os = "macos") {
        &["-u"]
    } else {
        &["-D", "--undefined-only"]
    };
    let output = Command::new("nm")
        .args(arguments)
        .arg(binary)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("nm is required for the import audit; it is never skipped");
    assert!(
        output.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let imports: BTreeSet<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().last())
        .map(|symbol| {
            let symbol = symbol.split('@').next().unwrap_or(symbol);
            let symbol = if cfg!(target_os = "macos") {
                symbol.strip_prefix('_').unwrap_or(symbol)
            } else {
                symbol
            };
            symbol.to_string()
        })
        .collect();
    assert!(
        imports.contains("read") && imports.len() > 20,
        "implausible import list from nm: {imports:?}"
    );
    let forbidden: Vec<&str> = FORBIDDEN_IMPORTS
        .iter()
        .copied()
        .filter(|symbol| imports.contains(*symbol))
        .collect();
    assert!(
        forbidden.is_empty(),
        "srtop imports process-control functions: {forbidden:?}"
    );
    let unattributed: Vec<&str> = UNATTRIBUTED_IMPORTS
        .iter()
        .copied()
        .filter(|symbol| imports.contains(*symbol))
        .collect();
    println!(
        "PX-008 import audit ({}): {} imported symbols; forbidden present: none; present and \
         not attributed by this test: {unattributed:?}; all: {imports:?}",
        std::env::consts::OS,
        imports.len()
    );
}

#[test]
fn line_numbers_point_at_the_violation() {
    let found = token_violations(
        "lines.rs",
        "fn a() {}\n\nfn b() { std::fs::write(\"x\", \"y\").ok(); }\n",
    );
    assert_eq!(
        found,
        vec!["lines.rs:3: [fs] `fs::write` is not a read".to_string()]
    );
    assert!(line_of(&Tok::Literal(7)) == 7);
}
