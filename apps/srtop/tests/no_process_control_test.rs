//! PX-008, the R0 release gate: a repeatable audit against srtop gaining a way to
//! signal or reconfigure a process (D1 §4 invariants 1 and 18, §7.7, §24; D2
//! T21). It runs on every `cargo test`, so it constrains every later change.
//!
//! It is a tripwire against introducing process control by accident, read from
//! tokens: it rejects the classes of construct listed here, each backed by a
//! self-test below. It is not a proof against deliberate evasion; a construct it
//! does not recognise passes it.
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
//!   symbolic link may stand there. Each file is tokenized with proc-macro2, so
//!   comments, string, raw-string, byte-string and character literals can
//!   neither hide code nor be mistaken for it, and an item or statement gated by
//!   `#[cfg(test)]` is set aside token-exactly (`cfg(test)` exactly; any other
//!   cfg is read). The production tokens may not name: process creation or
//!   control (`Command`, `Child`, `kill`, `ptrace`, `setpriority`,
//!   `sched_setaffinity`, `setrlimit`, …); FFI, assembly, raw system calls or
//!   dynamic loading (`unsafe`, `extern`, `asm!`, `libc`, `nix`, `syscall`,
//!   `dlopen`, …); outbound sockets (`UnixStream`, `TcpStream`, `UdpSocket`,
//!   `connect`); the runtime's PTY facility (`pty`, `TerminalSpec`, …); any
//!   handler for client input except the sort controls' — `on` in method or
//!   path form is refused unless the call names exactly one of the four
//!   sort-button node constants as its first argument and `ACTIVATE` as its
//!   second (PX-009's standard semantic controls), or, in the same exact shape,
//!   a column chooser button (PX-068), a search button with `ACTIVATE` or the
//!   filter, search or PID field with `TEXT_EDIT` (PX-010, PX-011), a user-filter
//!   button with `ACTIVATE` or the user or PID-set field with `TEXT_EDIT` (PX-012), or
//!   exactly `TABLE` with `SELECTION_CHANGED`, the inspector's one selection input
//!   (PX-013), or a sampling button with `ACTIVATE` or the interval field with
//!   `TEXT_EDIT` (PX-015), or exactly `FOLLOW_TOGGLE` with `VALUE_CHANGED`
//!   (PX-017), or exactly `VIEW_TOGGLE` with `VALUE_CHANGED`, `TREE` with
//!   `SELECTION_CHANGED` or `TREE` with `EXPANSION_CHANGED` (PX-018), or
//!   exactly `USER_THREADS_TOGGLE` or `KERNEL_THREADS_TOGGLE` with
//!   `VALUE_CHANGED` (PX-019), or a tag button (`TAG_TOGGLE`,
//!   `TAG_DESCENDANTS`, `TAGS_CLEAR`) with `ACTIVATE` (PX-020), or an
//!   action button (`ACTION_PREPARE`, `ACTION_CANCEL`) with `ACTIVATE`
//!   (PX-028), or a saved views button (`SAVED_VIEW_SLOT_1`…`_8`,
//!   `SAVED_VIEW_CREATE`, `SAVED_VIEW_RENAME`, `SAVED_VIEW_DELETE`,
//!   `SAVED_VIEWS_RESTORE`) with `ACTIVATE` or `SAVED_VIEW_NAME_INPUT` with
//!   `TEXT_EDIT` (PX-069), or a meter layout button (`METER_TOGGLE_*`,
//!   `METER_EARLIER_*`, `METER_LATER_*`, `METER_MODE_*`, `METERS_HEADER`,
//!   `METERS_RESTORE`) with `ACTIVATE` (PX-070), or, in `src/lib.rs` only, exactly `panel_confirm` with
//!   `ACTIVATE`, the confirmation panels' Confirm handlers (PX-030), whose
//!   gate reaches one of the two production executors pinned below;
//!   `on_result`, `on_text_edit`, `on_expansion_changed`
//!   and a model range provider unconditionally; `include`, called or
//!   renamed; or a macro definition (`macro_rules!`), whose expansion can
//!   assemble tokens this audit never sees together. Attributes come from an
//!   allowlist — `doc`, `derive` of the standard derives, `cfg`, `must_use`, the
//!   lint levels, `inline` and `tokio::main` — and `cfg_attr` is read through,
//!   at any depth, so `#[path]`, `#[link]`, `#[export_name]` or `#[no_mangle]`
//!   is refused bare or wrapped. Files are opened only through an allowlist —
//!   `File::open` and the `std::fs` read functions srtop uses — and the ways to
//!   obtain an `OpenOptions` (the only write, append, create or truncate
//!   builder) that these rules know are refused: `OpenOptions`, `File::options`,
//!   `.options()`, any other `File::` or `fs::` item, a grouped, glob or renamed
//!   import of one, a type alias of `File`. A file's write, resize and
//!   permission methods are refused as methods and as paths
//!   (`std::io::Write::write_all(&mut f, ..)`).
//! * **The one file that writes (PX-069).** `src/settings_file.rs`, and no
//!   other file, may name `OpenOptions`, `OpenOptionsExt`, `DirBuilder`,
//!   `DirBuilderExt`, `fs::rename` and `fs::remove_file`, and call `.write`
//!   and `.write_all`: the per-user settings store, which replaces the
//!   settings file and its backup through fresh temporary files renamed into
//!   place. Its writes are pinned ([`settings_write_violations`]): exactly
//!   one `OpenOptions::new()` and one `create_new(true)`, and no `truncate`,
//!   `append`, `.create(true)`, `custom_flags`, `clone`, `clone_from` or
//!   `to_owned`. Every other rule holds there as everywhere; no file but it
//!   may write, and only `src/main.rs` constructs a `SettingsStore`, once,
//!   from its command line or the per-user default path
//!   ([`settings_store_constructions`]). Another file may not hold an `impl`
//!   block for it or a struct literal of it, and the settings module holds
//!   exactly one literal, in `fn new`, no `Self::new` and no trait `impl`
//!   for it ([`settings_store_impl_violations`]). These are token rules: a
//!   construction they do not recognise passes them. By design the module's
//!   own `fs::rename` and `fs::remove_file` may name any path; that file is
//!   the one place such a call can be, and it is reviewed with this test.
//!   The only signal API is receiving SIGINT, SIGTERM and the
//!   `--smoke-fixture` SIGUSR1.
//! * **The one raw system-call allowance (PX-029).** `src/handle.rs`, and no
//!   other file, may name `unsafe`, `libc` and `syscall`, and only as
//!   `pidfd_open` and `pidfd_send_signal` need them: `libc::` followed by
//!   one of [`HANDLE_LIBC_ITEMS`], `libc::syscall(` only with
//!   `libc::SYS_pidfd_open` or `libc::SYS_pidfd_send_signal` as its first
//!   argument, and `unsafe` only as a block whose tokens are exactly one of
//!   [`HANDLE_UNSAFE_BLOCKS`]; the signal number is pinned too
//!   ([`HANDLE_SIGNAL_PINS`]: `Terminate` is `libc::SIGTERM`, the only
//!   `libc::SIG..` named). `kill`, `extern`, every other `libc` item and
//!   every other rule hold there as everywhere. No other production file may
//!   name the module or its public items ([`HANDLE_ITEMS`]) but the one
//!   termination executor (PX-030).
//! * **The one executor that can reach a process (PX-030).**
//!   `src/terminate.rs` alone may name the handle module, and only the
//!   items [`EXECUTOR_HANDLE_ITEMS`] (not its system-call seam); all of
//!   production holds exactly one handle table construction
//!   ([`EXECUTOR_TABLE`]) and exactly one settlement ([`EXECUTOR_SETTLE`],
//!   SIGTERM through the handle it acquired), both in that file; `main`
//!   builds that executor exactly once; and the production `ProcessActions`
//!   are exactly [`PRODUCTION_EXECUTORS`]. Every other rule — `kill`,
//!   `libc`, `unsafe`, `syscall` — holds in that file as everywhere: the
//!   handle module stays the only system-call site, and nothing falls back to
//!   a PID-addressed signal.
//! * **Binary imports.** `nm` lists the symbols the built `srtop` imports. None
//!   may create a process, deliver a targeted signal other than `kill`, trace a
//!   process, or change one's priority, affinity, limits, group or session. The
//!   binary does import `kill`, `waitpid` and `waitid` (and `syscall` on Linux);
//!   this check does not establish which code reaches them.
//!
//! What none of this can see is listed in the R0 release record: any construct
//! the token audit does not recognise — among them a write through a `File`
//! handle by `write!` or a generic writer such as `io::copy`, which tokens cannot
//! tell from formatting into a `String` (inert while the only open allowed is
//! read-only, outside the settings store's fresh temporary files); code that procedural macros or the dependencies' own macros
//! generate; code inside the allowed dependencies beyond the symbols they
//! import; the arguments of a raw system call a dependency makes; any binary but
//! the debug build `cargo test` links; Cargo configuration outside the package
//! (`.cargo/config.toml`, `RUSTFLAGS`); and what another program does to the
//! same processes.
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
    // a macro definition: its expansion can assemble tokens this audit never
    // sees together (`$h[path = ".."]` from `#`, `std::fs::$f` from `write`)
    (
        "macro_rules",
        "defines a macro, whose expansion the audit cannot read",
    ),
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
    // PX-018: the tree's disclosure reports reach srtop only through the one
    // exact `on(TREE, EXPANSION_CHANGED, ..)` allowance.
    ("on_expansion_changed", "installs a client event handler"),
    ("register_handler", "installs a client event handler"),
    ("ModelRangeProvider", "answers client range requests"),
    ("ModelRangeRequestInbox", "answers client range requests"),
    ("run_model_range_worker", "answers client range requests"),
];

/// Methods production code may not call: the `OpenOptions` builder and the
/// writes, resizes and permission changes of an open file, as a method
/// (`file.write_all(..)`) or as a path (`std::io::Write::write_all(&mut file,
/// ..)`, called or taken as a value; a macro such as `std::write!` is not one).
/// `append`, `truncate` and `create` are also `OpenOptions` builders, but srtop
/// calls them on vectors and SDK builders; they are reachable on an
/// `OpenOptions` only after one of the refused ways to obtain it. A write
/// through a `File` by `write!` or a generic writer (`io::copy`) is not seen:
/// tokens cannot tell a `File` from a `String`, and the only open allowed is
/// read-only.
const FORBIDDEN_METHODS: &[&str] = &[
    "options",
    "write",
    "write_all",
    "write_at",
    "write_vectored",
    "write_fmt",
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

/// The attributes production code may carry, by path; every other one is
/// refused: `path`, `link`, `link_name`, `link_section`, `no_mangle`,
/// `export_name`, `used`, `unsafe(..)`, crate-level attributes, and every
/// attribute macro but `tokio::main`. srtop's production code carries `doc` (its
/// doc comments), `derive`, `must_use` and `tokio::main`; `cfg`, the lint levels
/// and `inline` change nothing it could use to reach a process. `cfg_attr` is
/// read through: every attribute it applies, at any depth, must be allowed too.
const ALLOWED_ATTRIBUTES: &[&str] = &[
    "doc",
    "derive",
    "must_use",
    "tokio::main",
    "cfg",
    "cfg_attr",
    "allow",
    "expect",
    "warn",
    "deny",
    "forbid",
    "inline",
];

/// The derives production code may use: the standard library's, which
/// implement only the trait they name.
const ALLOWED_DERIVES: &[&str] = &[
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
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
    // PX-029: the two pidfd system calls of `src/handle.rs`, the only file
    // the token rules let name it; already locked through srui-unix-security.
    "libc",
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
/// attributed: this test does not establish which code reaches them. Since
/// PX-029 srtop's own `src/handle.rs` calls `syscall` on Linux (for
/// `pidfd_open` and `pidfd_send_signal` only, as the token rules pin); since
/// PX-030 the binary reaches it through the termination executor of
/// `--actions terminate`.
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
/// `extra` are the names the file's own allowance adds (PX-069).
fn check_fs_group(group: &[Tok], extra: &[&str], report: &mut dyn FnMut(usize, &str, String)) {
    for (i, tok) in group.iter().enumerate() {
        match tok {
            Tok::Ident(name, _, line) if name == "as" => {
                report(*line, "fs-import", "a renamed import from fs".into())
            }
            Tok::Ident(name, _, line) => {
                let renamed_to = ident(i.checked_sub(1).and_then(|p| group.get(p))) == Some("as");
                if !renamed_to
                    && name != "self"
                    && !FS_ALLOWED.contains(&name.as_str())
                    && !extra.contains(&name.as_str())
                {
                    report(*line, "fs-import", format!("fs::{{{name}}} is not a read"));
                }
            }
            Tok::Punct('*', line) => report(*line, "fs-import", "a glob import from fs".into()),
            Tok::Group(_, inner, _) => check_fs_group(inner, extra, report),
            _ => {}
        }
    }
}

/// The one file that may write (PX-069): the per-user settings file's store,
/// which replaces the settings file and its backup through fresh temporary
/// files renamed into place.
const SETTINGS_MODULE: &str = "src/settings_file.rs";

/// The identifiers only [`SETTINGS_MODULE`] may name: the builder that opens
/// its temporary files and its mode extension.
const SETTINGS_IDENTS: &[&str] = &["OpenOptions", "OpenOptionsExt"];

/// The methods only [`SETTINGS_MODULE`] may call: `OpenOptions::write`, and
/// `write_all` of a temporary file it created.
const SETTINGS_METHODS: &[&str] = &["write", "write_all"];

/// The `std::fs` and `std::os::unix::fs` items only [`SETTINGS_MODULE`] may
/// name besides the reads: creating its directory (mode 0700), opening a
/// temporary file, renaming it into place, and removing a temporary file.
const SETTINGS_FS_ALLOWED: &[&str] = &[
    "OpenOptions",
    "OpenOptionsExt",
    "DirBuilder",
    "DirBuilderExt",
    "rename",
    "remove_file",
];

/// Whether the token after `level[i]` is a parenthesized group holding
/// exactly the identifier `true`.
fn called_with_true(level: &[Tok], i: usize) -> bool {
    matches!(
        level.get(i + 1),
        Some(Tok::Group(Delimiter::Parenthesis, inner, _))
            if matches!(inner.as_slice(), [Tok::Ident(value, _, _)] if value == "true")
    )
}

/// How [`SETTINGS_MODULE`]'s writes are pinned (PX-069): exactly one
/// `OpenOptions::new()` and exactly one `create_new`, called with exactly
/// `true` (so every file it opens for writing is a fresh one, never an
/// existing file or a link); no `truncate`, `append`, `.create(true)` or
/// `custom_flags` (`O_TRUNC`, `O_APPEND`), which would open an existing file
/// to rewrite it in place; and no `clone`, `clone_from` or `to_owned` of
/// anything, so the one pinned builder cannot be copied and reconfigured.
fn settings_write_violations(source: &str) -> Vec<String> {
    fn walk(level: &[Tok], counts: &mut (usize, usize), found: &mut Vec<String>) {
        for (i, tok) in level.iter().enumerate() {
            let prev = i.checked_sub(1).and_then(|p| level.get(p));
            match tok {
                Tok::Ident(name, _, line) => match name.as_str() {
                    "truncate" | "append" | "custom_flags" => {
                        found.push(format!("line {line}: `{name}` rewrites a file in place"))
                    }
                    "clone" | "clone_from" | "to_owned"
                        if is_punct(prev, '.') || is_path_sep(prev) =>
                    {
                        found.push(format!(
                            "line {line}: `{name}` could copy the pinned file builder"
                        ))
                    }
                    "Clone" if is_path_sep(level.get(i + 1)) => found.push(format!(
                        "line {line}: `Clone::` could copy the pinned file builder"
                    )),
                    "create_new" if called_with_true(level, i) => counts.1 += 1,
                    "create_new" => found.push(format!(
                        "line {line}: `create_new` not called with exactly `true`"
                    )),
                    "create" if is_punct(i.checked_sub(1).and_then(|p| level.get(p)), '.') => {
                        if let Some(Tok::Group(Delimiter::Parenthesis, inner, _)) = level.get(i + 1)
                        {
                            if matches!(inner.as_slice(), [Tok::Ident(value, _, _)] if value == "true")
                            {
                                found.push(format!(
                                    "line {line}: `.create(true)` opens an existing file"
                                ));
                            }
                        }
                    }
                    "OpenOptions"
                        if is_path_sep(level.get(i + 1))
                            && ident(level.get(i + 2)) == Some("new") =>
                    {
                        counts.0 += 1
                    }
                    _ => {}
                },
                Tok::Group(_, inner, _) => walk(inner, counts, found),
                _ => {}
            }
        }
    }
    let stream = match TokenStream::from_str(source) {
        Ok(stream) => stream,
        Err(error) => return vec![format!("does not tokenize: {error}")],
    };
    let tokens = production(&lower(stream));
    let mut counts = (0, 0);
    let mut found = Vec::new();
    walk(&tokens, &mut counts, &mut found);
    if counts.0 != 1 {
        found.push(format!(
            "{} `OpenOptions::new()`, not exactly one",
            counts.0
        ));
    }
    if counts.1 != 1 {
        found.push(format!("{} `create_new(true)`, not exactly one", counts.1));
    }
    found
}

/// The one file that may construct a settings store (PX-069): `main`, from
/// `--settings-path` or the per-user default path, never from client input.
const SETTINGS_STORE_CONSTRUCTOR: &str = "src/main.rs";

/// Every way production file `file` breaks the constructor pin: a
/// construction anywhere but [`SETTINGS_STORE_CONSTRUCTOR`], more than one
/// there, or a renamed or aliased type.
fn settings_store_violations(file: &str, source: &str) -> Vec<String> {
    let (constructions, hidden) = settings_store_constructions(source);
    let mut found: Vec<String> = hidden
        .into_iter()
        .map(|violation| format!("{file}: [settings-store] {violation}"))
        .collect();
    let allowed = usize::from(file == SETTINGS_STORE_CONSTRUCTOR);
    if constructions != allowed {
        found.push(format!(
            "{file}: [settings-store] {constructions} `SettingsStore::new`, {allowed} allowed"
        ));
    }
    found.extend(
        settings_store_impl_violations(file == SETTINGS_MODULE, source)
            .into_iter()
            .map(|violation| format!("{file}: [settings-store] {violation}")),
    );
    found
}

/// The other ways to make a `SettingsStore` (PX-069 verification round 2,
/// V2-3): an `impl` block for it in another file, whose `Self::new` the
/// constructor count cannot see (B1); a struct literal anywhere but the body
/// of `fn new` in the settings module, which could name any path (B2); a
/// trait implementation such as `Default`, even there (B3); and `Self::new`
/// in the settings module. The settings module holds exactly one literal,
/// in `fn new`, and only the one inherent `impl SettingsStore` block.
fn settings_store_impl_violations(settings_module: bool, source: &str) -> Vec<String> {
    struct Walk {
        settings_module: bool,
        literals: usize,
        in_new: usize,
        found: Vec<String>,
    }
    fn walk(level: &[Tok], inside_new: bool, state: &mut Walk) {
        let mut next_body_is_new = false;
        // Between `fn` and its body: a return type followed by the body
        // (`-> Self { .. }`, `-> &SettingsStore { .. }`) is no literal.
        let mut in_signature = false;
        // Likewise between `impl` and its block (`impl a::SettingsStore {`).
        let mut in_impl_header = false;
        for (i, tok) in level.iter().enumerate() {
            let prev = i.checked_sub(1).and_then(|p| level.get(p));
            match tok {
                Tok::Ident(name, raw, line) if name == "impl" && !*raw => {
                    in_impl_header = true;
                    // The header runs to the block it opens.
                    let end = level[i..]
                        .iter()
                        .position(|tok| matches!(tok, Tok::Group(Delimiter::Brace, ..)))
                        .map_or(level.len(), |offset| i + offset);
                    let header = &level[i + 1..end];
                    if contains_ident(header, &["SettingsStore"]) {
                        let inherent =
                            matches!(header, [Tok::Ident(only, _, _)] if only == "SettingsStore");
                        if !state.settings_module {
                            state.found.push(format!(
                                "line {line}: an `impl` block for `SettingsStore` outside the settings module"
                            ));
                        } else if !inherent {
                            state.found.push(format!(
                                "line {line}: a trait implemented for `SettingsStore`"
                            ));
                        }
                    }
                }
                Tok::Ident(name, _, _) if name == "fn" => {
                    next_body_is_new = ident(level.get(i + 1)) == Some("new");
                    in_signature = true;
                }
                Tok::Ident(name, _, line)
                    if (name == "SettingsStore" || name == "Self")
                        && matches!(level.get(i + 1), Some(Tok::Group(Delimiter::Brace, ..)))
                        && !in_signature
                        && !in_impl_header
                        && !matches!(ident(prev), Some("struct" | "impl" | "for")) =>
                {
                    if name == "SettingsStore" || state.settings_module {
                        state.literals += 1;
                        state.in_new += usize::from(inside_new);
                    }
                    if !state.settings_module && name == "SettingsStore" {
                        state.found.push(format!(
                            "line {line}: a `SettingsStore` literal outside the settings module"
                        ));
                    }
                }
                Tok::Ident(name, _, line)
                    if state.settings_module
                        && name == "Self"
                        && is_path_sep(level.get(i + 1))
                        && ident(level.get(i + 2)) == Some("new") =>
                {
                    state
                        .found
                        .push(format!("line {line}: `Self::new` in the settings module"));
                }
                Tok::Group(Delimiter::Brace, inner, _) => {
                    in_signature = false;
                    in_impl_header = false;
                    let body_is_new = std::mem::take(&mut next_body_is_new);
                    walk(inner, inside_new || body_is_new, state);
                }
                Tok::Group(_, inner, _) => walk(inner, inside_new, state),
                _ => {}
            }
        }
    }
    let Ok(stream) = TokenStream::from_str(source) else {
        return vec!["does not tokenize".into()];
    };
    let tokens = production(&lower(stream));
    let mut state = Walk {
        settings_module,
        literals: 0,
        in_new: 0,
        found: Vec::new(),
    };
    walk(&tokens, false, &mut state);
    if settings_module && (state.literals != 1 || state.in_new != 1) {
        state.found.push(format!(
            "{} store literals, {} of them in `fn new`: exactly one, there, is allowed",
            state.literals, state.in_new
        ));
    }
    state.found
}

/// How many times a production file constructs a `SettingsStore`
/// (`SettingsStore::new`, `<SettingsStore>::new`), and every way it could
/// hide one: the type renamed on import or aliased.
fn settings_store_constructions(source: &str) -> (usize, Vec<String>) {
    fn walk(level: &[Tok], count: &mut usize, found: &mut Vec<String>) {
        for (i, tok) in level.iter().enumerate() {
            match tok {
                Tok::Ident(name, _, line) if name == "SettingsStore" => {
                    let next = level.get(i + 1);
                    let member = if is_path_sep(next) {
                        ident(level.get(i + 2))
                    } else if is_punct(next, '>') && is_path_sep(level.get(i + 2)) {
                        ident(level.get(i + 3))
                    } else {
                        None
                    };
                    if member == Some("new") {
                        *count += 1;
                    }
                    if ident(next) == Some("as") {
                        found.push(format!("line {line}: `SettingsStore` renamed"));
                    }
                }
                Tok::Ident(name, raw, line) if name == "type" && !*raw => {
                    let end = level[i..]
                        .iter()
                        .position(|tok| matches!(tok, Tok::Punct(';', _)))
                        .map_or(level.len(), |offset| i + offset);
                    if contains_ident(&level[i + 1..end], &["SettingsStore"]) {
                        found.push(format!("line {line}: a type alias of `SettingsStore`"));
                    }
                }
                Tok::Group(_, inner, _) => walk(inner, count, found),
                _ => {}
            }
        }
    }
    let Ok(stream) = TokenStream::from_str(source) else {
        return (0, vec!["does not tokenize".into()]);
    };
    let tokens = production(&lower(stream));
    let (mut count, mut found) = (0, Vec::new());
    walk(&tokens, &mut count, &mut found);
    (count, found)
}

fn contains_ident(level: &[Tok], names: &[&str]) -> bool {
    level.iter().any(|tok| match tok {
        Tok::Ident(name, _, _) => names.contains(&name.as_str()),
        Tok::Group(_, inner, _) => contains_ident(inner, names),
        _ => false,
    })
}

/// An attribute's path (`doc`, `tokio::main`) and where its arguments start. The
/// path is empty when the attribute does not start with an identifier, as
/// `#[$name = ".."]` in a macro body.
fn attribute_path(attribute: &[Tok]) -> (String, usize) {
    let mut path = Vec::new();
    let mut k = 0;
    while let Some(Tok::Ident(name, _, _)) = attribute.get(k) {
        path.push(name.as_str());
        k += 1;
        if is_path_sep(attribute.get(k)) && matches!(attribute.get(k + 1), Some(Tok::Ident(..))) {
            k += 1;
        } else {
            break;
        }
    }
    (path.join("::"), k)
}

/// One attribute, the inside of `#[..]` or `#![..]`, against the allowlist. A
/// `cfg_attr` is read through at any depth: every attribute it applies must be
/// allowed too, so `#[cfg_attr(unix, path = "..")]` is refused as `#[path]` is.
fn check_attribute(
    attribute: &[Tok],
    line: usize,
    applied_by_cfg_attr: bool,
    report: &mut dyn FnMut(usize, &str, String),
) {
    let (path, start) = attribute_path(attribute);
    let context = if applied_by_cfg_attr {
        " (applied by `cfg_attr`)"
    } else {
        ""
    };
    if !ALLOWED_ATTRIBUTES.contains(&path.as_str()) {
        let shown = if path.is_empty() { ".." } else { &path };
        report(
            line,
            "attribute",
            format!("`#[{shown}]`{context} is not an allowed attribute"),
        );
        return;
    }
    let list = match &attribute[start..] {
        [Tok::Group(Delimiter::Parenthesis, list, _)] => Some(list.as_slice()),
        _ => None,
    };
    let comma = |tok: &Tok| matches!(tok, Tok::Punct(',', _));
    match (path.as_str(), list) {
        // `cfg_attr(predicate, attribute, ..)`: every attribute after the predicate
        ("cfg_attr", Some(list)) => {
            for applied in list.split(comma).skip(1) {
                if !applied.is_empty() {
                    check_attribute(applied, line, true, report);
                }
            }
        }
        ("derive", Some(list)) => {
            for derive in list.split(comma) {
                match derive {
                    [] => {}
                    [Tok::Ident(name, _, _)] if ALLOWED_DERIVES.contains(&name.as_str()) => {}
                    _ => report(
                        line,
                        "attribute",
                        format!(
                            "`derive({})`{context} is not a standard derive",
                            ident(derive.first()).unwrap_or("..")
                        ),
                    ),
                }
            }
        }
        ("cfg_attr" | "derive", None) => {
            report(line, "attribute", format!("a malformed `{path}`{context}"))
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// PX-029: the process handle module's two system calls
// ---------------------------------------------------------------------------

/// The one production file that may make a raw system call (PX-029).
const HANDLE_MODULE: &str = "src/handle.rs";

/// The `libc` items the handle module may name: the call, the two call
/// numbers, the one signal and two argument types. Every other one —
/// `kill`, `SYS_kill`, `setpriority` — is refused there as everywhere.
const HANDLE_LIBC_ITEMS: &[&str] = &[
    "syscall",
    "SYS_pidfd_open",
    "SYS_pidfd_send_signal",
    "SIGTERM",
    "pid_t",
    "c_uint",
];

/// The calls `libc::syscall` may make in the handle module, named as its
/// first argument. A number (`libc::syscall(62, ..)`) or another name is
/// refused.
const HANDLE_SYSCALLS: &[&str] = &["SYS_pidfd_open", "SYS_pidfd_send_signal"];

/// The handle module's `unsafe` blocks, token for token
/// ([`canonical`]): `pidfd_open`, taking ownership of the descriptor it
/// returns, and `pidfd_send_signal` with a NULL `siginfo_t`. Any other
/// `unsafe` — another block, a changed argument, an `unsafe fn` or
/// `impl` — is refused until it is reviewed and pinned here.
const HANDLE_UNSAFE_BLOCKS: &[&str] = &[
    "libc : : syscall ( libc : : SYS_pidfd_open , pid , 0 as libc : : c_uint )",
    "OwnedFd : : from_raw_fd ( fd )",
    "libc : : syscall ( libc : : SYS_pidfd_send_signal , fd , number , std : : ptr : : null : : < u8 > (  ) , 0 as libc : : c_uint , )",
];

/// The handle module's one signal mapping and its one use, token for token
/// ([`canonical`]): `Terminate` is `libc::SIGTERM`, and the number the pinned
/// `pidfd_send_signal` block sends is exactly that. With
/// [`handle_signal_violations`], no other number (`=> 9`), no other signal
/// (`libc::SIGKILL`) and no shadowing `let number` can reach the call.
const HANDLE_SIGNAL_PINS: &[&str] = &[
    "fn number ( self ) - > i32 { match self { Self : : Terminate = > libc : : SIGTERM , } }",
    "let number = signal . number (  ) ;",
];

/// The handle module's public items. No other production file may name
/// them, or the module itself as a path, but [`EXECUTOR_MODULE`], and that
/// one only [`EXECUTOR_HANDLE_ITEMS`] (PX-030).
const HANDLE_ITEMS: &[&str] = &[
    "ProcessHandles",
    "PidfdSystem",
    "Pidfd",
    "LivePidfds",
    "HandleSignal",
    "SignalOutcome",
    "AcquireError",
    "HandleId",
];

// ---------------------------------------------------------------------------
// PX-030: the one executor that reaches a process handle
// ---------------------------------------------------------------------------

/// The one production file besides the handle module that may name it: the
/// termination executor.
const EXECUTOR_MODULE: &str = "src/terminate.rs";

/// What the executor module may name of the handle module: the table, its
/// handle IDs, the one signal and the two result types. Not the system-call
/// seam (`PidfdSystem`, `Pidfd`, `LivePidfds`): production code uses only the
/// host's, through `ProcessHandles::live`.
const EXECUTOR_HANDLE_ITEMS: &[&str] = &[
    "ProcessHandles",
    "HandleSignal",
    "SignalOutcome",
    "AcquireError",
    "HandleId",
];

/// The one handle settlement in production code, token for token
/// ([`render`]): the handle just acquired, with SIGTERM. In
/// [`EXECUTOR_MODULE`], and nowhere else.
const EXECUTOR_SETTLE: &str = ". settle ( id , HandleSignal : : Terminate )";

/// The one handle table production code builds: PX-029 requires exactly one
/// table, since a `HandleId` is minted per table and `settle` takes no key.
/// In [`EXECUTOR_MODULE`], and nowhere else.
const EXECUTOR_TABLE: &str = "ProcessHandles : : live (  )";

/// The one construction of the termination executor in production code, in
/// `src/main.rs`.
const EXECUTOR_CONSTRUCTION: (&str, &str) = ("src/main.rs", "PidfdTerminator : : live (  )");

/// The production action executors (PX-028, PX-030) and the file each one is
/// implemented in.
const PRODUCTION_EXECUTORS: &[(&str, &str)] = &[
    ("DryRunExecutor", "src/actions.rs"),
    ("PidfdTerminator", EXECUTOR_MODULE),
];

/// The confirmation panels' Confirm handlers (PX-030): one registration per
/// panel slot, made in a loop in `install_actions`, whose node argument is
/// exactly the identifier `panel_confirm` with `ACTIVATE`, in this file only.
const PANEL_CONFIRM: (&str, &str) = ("src/lib.rs", "panel_confirm");

/// The production tokens of `level` as one line: identifiers and
/// punctuation by their text, `::` as `: :` (as [`canonical`] prints it),
/// groups in their delimiters, every literal as `<literal>`.
fn render(level: &[Tok]) -> String {
    level
        .iter()
        .map(|tok| match tok {
            Tok::Ident(name, _, _) => name.clone(),
            Tok::Punct(ch, _) => ch.to_string(),
            Tok::PathSep(_) => ": :".to_string(),
            Tok::Literal(_) => "<literal>".to_string(),
            Tok::Group(delimiter, inner, _) => {
                let inner = render(inner);
                match delimiter {
                    Delimiter::Parenthesis => format!("( {inner} )"),
                    Delimiter::Brace => format!("{{ {inner} }}"),
                    Delimiter::Bracket => format!("[ {inner} ]"),
                    Delimiter::None => inner,
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How the production tokens of one file reach the handle table: every
/// `settle` called as a method or a path, and every associated function of
/// `ProcessHandles` or `PidfdTerminator` (`::live`, `::new`, ...) named,
/// each as the rendered text around it, so a pinned one can be told from any
/// other.
#[derive(Debug, Default, PartialEq, Eq)]
struct HandleReach {
    settles: Vec<String>,
    tables: Vec<String>,
    executors: Vec<String>,
}

fn handle_reach(source: &str) -> HandleReach {
    fn walk(level: &[Tok], reach: &mut HandleReach) {
        for (i, tok) in level.iter().enumerate() {
            let prev = i.checked_sub(1).and_then(|p| level.get(p));
            match tok {
                Tok::Ident(name, _, _)
                    if name == "settle" && (is_punct(prev, '.') || is_path_sep(prev)) =>
                {
                    let start = i - 1;
                    reach
                        .settles
                        .push(render(&level[start..(i + 2).min(level.len())]));
                }
                Tok::Ident(name, _, _)
                    if (name == "ProcessHandles" || name == "PidfdTerminator")
                        && is_path_sep(level.get(i + 1)) =>
                {
                    let end = (i + 4).min(level.len());
                    let text = render(&level[i..end]);
                    // Any associated function, called or taken as a value,
                    // is a way to build one.
                    if ident(level.get(i + 2))
                        .is_some_and(|member| member.starts_with(|c: char| c.is_ascii_lowercase()))
                    {
                        if name == "ProcessHandles" {
                            reach.tables.push(text);
                        } else {
                            reach.executors.push(text);
                        }
                    }
                }
                Tok::Group(_, inner, _) => walk(inner, reach),
                _ => {}
            }
        }
    }
    let stream = TokenStream::from_str(source).expect("a source file tokenizes");
    let mut reach = HandleReach::default();
    walk(&production(&lower(stream)), &mut reach);
    reach
}

/// Every way `files` (path, source) depart from the one table, the one
/// settlement and the one executor construction PX-030 allows.
fn handle_reach_violations(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    let (mut settles, mut tables, mut executors) = (0, 0, 0);
    for (file, source) in files {
        let reach = handle_reach(source);
        for settle in &reach.settles {
            settles += 1;
            if file != EXECUTOR_MODULE || settle != EXECUTOR_SETTLE {
                found.push(format!(
                    "{file}: [settle] `{settle}` is not the one settlement"
                ));
            }
        }
        for table in &reach.tables {
            tables += 1;
            if file != EXECUTOR_MODULE || table != EXECUTOR_TABLE {
                found.push(format!(
                    "{file}: [table] `{table}` builds another handle table"
                ));
            }
        }
        for executor in &reach.executors {
            executors += 1;
            if (file.as_str(), executor.as_str()) != EXECUTOR_CONSTRUCTION {
                found.push(format!(
                    "{file}: [executor] `{executor}` builds another terminator"
                ));
            }
        }
    }
    for (what, count) in [
        ("settlement", settles),
        ("handle table", tables),
        ("terminator construction", executors),
    ] {
        if count != 1 {
            found.push(format!(
                "[count] {count} {what}s in production code, not exactly one"
            ));
        }
    }
    found
}

/// Whether an `on(..)` argument list is a confirmation panel's Confirm
/// registration (PX-030): exactly the identifier `panel_confirm`, then
/// exactly `ACTIVATE`. The file is checked by the caller.
fn is_panel_confirm_activation(tokens: &[Tok]) -> bool {
    matches!(
        tokens,
        [Tok::Ident(node, false, _), Tok::Punct(',', _), Tok::Ident(event, false, _), Tok::Punct(',', _), ..]
            if node == PANEL_CONFIRM.1 && event == "ACTIVATE"
    )
}

/// Whether a parenthesized argument list starts with one of
/// [`HANDLE_SYSCALLS`], as `libc::SYS_pidfd_open,`.
fn is_allowed_syscall_call(arguments: Option<&Tok>) -> bool {
    match arguments {
        Some(Tok::Group(Delimiter::Parenthesis, tokens, _)) => matches!(
            tokens.as_slice(),
            [Tok::Ident(crate_name, false, _), Tok::PathSep(_), Tok::Ident(call, false, _), Tok::Punct(',', _), ..]
                if crate_name == "libc" && HANDLE_SYSCALLS.contains(&call.as_str())
        ),
        _ => false,
    }
}

/// Whether the forbidden identifier at `level[i]` is one the handle module
/// may use there: `unsafe` opening a block (its tokens are checked by
/// [`handle_unsafe_violations`]), `libc::` an allowed item, and `syscall`
/// only as `libc::syscall(libc::SYS_pidfd_.., ..)`.
fn handle_module_allows(level: &[Tok], i: usize) -> bool {
    let prev = i.checked_sub(1).and_then(|p| level.get(p));
    match ident(level.get(i)) {
        Some("unsafe") => matches!(level.get(i + 1), Some(Tok::Group(Delimiter::Brace, ..))),
        Some("libc") => {
            !is_path_sep(prev)
                && is_path_sep(level.get(i + 1))
                && match ident(level.get(i + 2)) {
                    Some("syscall") => is_allowed_syscall_call(level.get(i + 3)),
                    Some(item) => HANDLE_LIBC_ITEMS.contains(&item),
                    None => false,
                }
        }
        Some("syscall") => {
            is_path_sep(prev)
                && ident(i.checked_sub(2).and_then(|p| level.get(p))) == Some("libc")
                && is_allowed_syscall_call(level.get(i + 1))
        }
        _ => false,
    }
}

/// A token stream as one canonical line: every token's text, single spaces
/// between them, groups in their delimiters. Literals keep their text, so
/// `0` and `9` differ.
fn canonical(stream: TokenStream) -> String {
    stream
        .into_iter()
        .map(|tree| match tree {
            TokenTree::Group(group) => {
                let inner = canonical(group.stream());
                match group.delimiter() {
                    Delimiter::Parenthesis => format!("( {inner} )"),
                    Delimiter::Brace => format!("{{ {inner} }}"),
                    Delimiter::Bracket => format!("[ {inner} ]"),
                    Delimiter::None => inner,
                }
            }
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every way the handle module's `unsafe` departs from
/// [`HANDLE_UNSAFE_BLOCKS`], over all of its tokens (test code included): an
/// `unsafe` that does not open a block, a block not pinned there, and a
/// pinned block that is gone (so the list stays exact).
fn handle_unsafe_violations(source: &str) -> Vec<String> {
    fn walk(stream: TokenStream, unsafes: &mut usize, blocks: &mut Vec<String>) {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        for (i, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Ident(name) if name == "unsafe" => {
                    *unsafes += 1;
                    if let Some(TokenTree::Group(group)) = trees.get(i + 1) {
                        if group.delimiter() == Delimiter::Brace {
                            blocks.push(canonical(group.stream()));
                        }
                    }
                }
                TokenTree::Group(group) => walk(group.stream(), unsafes, blocks),
                _ => {}
            }
        }
    }
    let stream = match TokenStream::from_str(source) {
        Ok(stream) => stream,
        Err(error) => return vec![format!("does not tokenize: {error}")],
    };
    let (mut unsafes, mut blocks) = (0, Vec::new());
    walk(stream, &mut unsafes, &mut blocks);
    let mut found = Vec::new();
    if unsafes != blocks.len() {
        found.push(format!(
            "{} `unsafe` that open no block (an unsafe fn, impl, trait or extern)",
            unsafes - blocks.len()
        ));
    }
    let mut pinned: Vec<&str> = HANDLE_UNSAFE_BLOCKS.to_vec();
    for block in &blocks {
        match pinned.iter().position(|allowed| allowed == block) {
            Some(at) => {
                pinned.remove(at);
            }
            None => found.push(format!("an `unsafe` block that is not pinned: {block}")),
        }
    }
    for missing in pinned {
        found.push(format!("a pinned `unsafe` block is gone: {missing}"));
    }
    found
}

/// Every way the handle module's signal mapping departs from
/// [`HANDLE_SIGNAL_PINS`]: each pin must occur exactly once, `let number`
/// exactly once (no shadowing), and `libc::SIG..` exactly once (the pinned
/// `SIGTERM`), over all of the module's tokens.
fn handle_signal_violations(source: &str) -> Vec<String> {
    let stream = match TokenStream::from_str(source) {
        Ok(stream) => stream,
        Err(error) => return vec![format!("does not tokenize: {error}")],
    };
    let text = format!(" {} ", canonical(stream));
    let count = |needle: &str| text.matches(&format!(" {needle} ")).count();
    let mut found = Vec::new();
    for pin in HANDLE_SIGNAL_PINS {
        let seen = count(pin);
        if seen != 1 {
            found.push(format!(
                "the pinned signal mapping occurs {seen} times: {pin}"
            ));
        }
    }
    let lets = count("let number");
    if lets != 1 {
        found.push(format!("`let number` occurs {lets} times"));
    }
    let signals = text.matches(" libc : : SIG").count();
    if signals != 1 {
        found.push(format!(
            "{signals} `libc::SIG..` names, not the one pinned `SIGTERM`"
        ));
    }
    found
}

fn scan(level: &[Tok], file: &str, report: &mut dyn FnMut(usize, &str, String)) {
    let handle_module = file == HANDLE_MODULE;
    let executor_module = file == EXECUTOR_MODULE;
    // PX-069: the settings store's writes, in that file alone.
    let settings_module = file == SETTINGS_MODULE;
    let fs_extra: &[&str] = if settings_module {
        SETTINGS_FS_ALLOWED
    } else {
        &[]
    };
    for (i, tok) in level.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| level.get(p));
        let next = level.get(i + 1);
        let after = level.get(i + 2);
        match tok {
            Tok::Ident(name, raw, line) => {
                let line = *line;
                if let Some((_, why)) = FORBIDDEN_IDENTS.iter().find(|(word, _)| word == name) {
                    if !(handle_module && handle_module_allows(level, i))
                        && !(settings_module && SETTINGS_IDENTS.contains(&name.as_str()))
                    {
                        report(line, "ident", format!("`{name}`: {why}"));
                    }
                }
                // PX-029: the handle module is reached by no other file but
                // the termination executor, and that one names only its
                // table, IDs, signal and results (PX-030).
                if !handle_module {
                    if HANDLE_ITEMS.contains(&name.as_str())
                        && !(executor_module && EXECUTOR_HANDLE_ITEMS.contains(&name.as_str()))
                    {
                        report(
                            line,
                            "handle",
                            format!("`{name}`: the process handle module is not wired here"),
                        );
                    }
                    if name == "handle"
                        && !executor_module
                        && (is_path_sep(prev) || is_path_sep(next) || ident(next) == Some("as"))
                    {
                        report(
                            line,
                            "handle",
                            "a path to the process handle module, which is not wired".into(),
                        );
                    }
                }
                // `.name(` or `.name::<T>(`, and the path form in any position
                // (`std::io::Write::write_all(&mut file, ..)`, or taken as a
                // value) but a macro (`std::write!`); `fs::` and `File::` paths
                // have their own rules below
                if FORBIDDEN_METHODS.contains(&name.as_str())
                    && !(settings_module && SETTINGS_METHODS.contains(&name.as_str()))
                {
                    let owner = i.checked_sub(2).and_then(|p| ident(level.get(p)));
                    if is_punct(prev, '.')
                        && (matches!(next, Some(Tok::Group(Delimiter::Parenthesis, ..)))
                            || is_path_sep(next))
                    {
                        report(
                            line,
                            "method",
                            format!("`.{name}(..)` writes or opens for writing"),
                        );
                    } else if is_path_sep(prev)
                        && !is_punct(next, '!')
                        && !matches!(owner, Some("fs" | "File"))
                    {
                        report(
                            line,
                            "method",
                            format!("`::{name}` writes or opens for writing"),
                        );
                    }
                }
                match name.as_str() {
                    // `x.on(..)` and `Session::on(&session, ..)`: a client event
                    // handler. The sort controls' four ACTIVATE registrations
                    // (PX-009), the column chooser's (PX-068), the search
                    // buttons' (PX-011), the user-filter buttons' (PX-012),
                    // and the filter, search, PID, user and PID-set fields'
                    // TEXT_EDIT registrations (PX-010, PX-011, PX-012) and
                    // the table's one SELECTION_CHANGED registration (PX-013)
                    // are the allowances: the first argument must be one of
                    // those button constants with `ACTIVATE`, one of those
                    // input constants with `TEXT_EDIT`, or `TABLE` with
                    // `SELECTION_CHANGED`, so a handler on any other node,
                    // under any other event type, or through a computed
                    // argument stays refused exactly as before.
                    "on" if is_punct(prev, '.') || is_path_sep(prev) => {
                        let allowed = match next {
                            Some(Tok::Group(Delimiter::Parenthesis, tokens, _)) => {
                                is_sort_button_activation(tokens)
                                    || is_filter_input_text_edit(tokens)
                                    || is_table_selection(tokens)
                                    || is_follow_toggle(tokens)
                                    || is_view_or_tree_input(tokens)
                                    || (file == PANEL_CONFIRM.0
                                        && is_panel_confirm_activation(tokens))
                            }
                            _ => false,
                        };
                        if !allowed {
                            report(
                                line,
                                "handler",
                                "`on`: installs a client event handler".into(),
                            );
                        }
                    }
                    "fs" => {
                        if is_path_sep(next) {
                            match after {
                                Some(Tok::Ident(member, _, _))
                                    if !FS_ALLOWED.contains(&member.as_str())
                                        && !fs_extra.contains(&member.as_str()) =>
                                {
                                    report(line, "fs", format!("`fs::{member}` is not a read"))
                                }
                                Some(Tok::Group(Delimiter::Brace, group, _)) => {
                                    check_fs_group(group, fs_extra, report)
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
                    // `include!`, and `include` imported under another name
                    "include" => report(
                        line,
                        "include",
                        "`include` brings in code from elsewhere".into(),
                    ),
                    _ => {}
                }
            }
            Tok::Punct('#', line) => {
                let attribute = if is_punct(next, '!') { after } else { next };
                if let Some(Tok::Group(Delimiter::Bracket, inner, _)) = attribute {
                    check_attribute(inner, *line, false, report);
                }
            }
            Tok::Group(_, inner, _) => scan(inner, file, report),
            _ => {}
        }
    }
}

/// The four node constants whose `ACTIVATE` handlers are the audit's one
/// client-input allowance: the sort buttons PX-009 installs.
const SORT_BUTTON_CONSTANTS: &[&str] = &["SORT_PID", "SORT_NAME", "SORT_RESIDENT", "SORT_CPU"];

/// The column chooser's buttons (PX-068), whose `ACTIVATE` handlers re-lay out
/// the published table and, like the sort buttons', touch no process.
const COLUMN_CHOOSER_CONSTANTS: &[&str] = &[
    "COLUMN_TOGGLE_PID",
    "COLUMN_EARLIER_PID",
    "COLUMN_LATER_PID",
    "COLUMN_TOGGLE_NAME",
    "COLUMN_EARLIER_NAME",
    "COLUMN_LATER_NAME",
    "COLUMN_TOGGLE_RESIDENT",
    "COLUMN_EARLIER_RESIDENT",
    "COLUMN_LATER_RESIDENT",
    "COLUMN_TOGGLE_CPU",
    "COLUMN_EARLIER_CPU",
    "COLUMN_LATER_CPU",
    "COLUMNS_RESTORE",
];

/// The search buttons (PX-011), whose `ACTIVATE` handlers move the server's
/// current match or resolve the PID field the server kept, and touch no
/// process.
const SEARCH_BUTTON_CONSTANTS: &[&str] = &["SEARCH_PREVIOUS", "SEARCH_NEXT", "PID_GO"];

/// The user filter's buttons (PX-012), whose `ACTIVATE` handlers choose which
/// owners' rows are shown, and touch no process.
const OWNER_BUTTON_CONSTANTS: &[&str] = &["OWNER_MINE", "OWNER_ALL"];

/// The sampling buttons (PX-015), whose `ACTIVATE` handlers pause or resume
/// this app's published view, ask the refresh loop for one read-only sample,
/// or apply the interval field the server kept, and touch no process.
/// The tag buttons (PX-020), whose `ACTIVATE` handlers tag or untag the
/// process the server already resolved as selected, tag it and the
/// descendants the latest sample shows under it, or clear every tag. A tag
/// only marks published rows: none signals, opens for control or
/// reconfigures a process.
const TAG_BUTTON_CONSTANTS: &[&str] = &["TAG_TOGGLE", "TAG_DESCENDANTS", "TAGS_CLEAR"];

const SAMPLING_BUTTON_CONSTANTS: &[&str] =
    &["SAMPLING_PAUSE", "SAMPLING_REFRESH", "INTERVAL_APPLY"];

/// The fixed action buttons (PX-028), whose `ACTIVATE` handlers prepare an
/// intent for the process the server resolved as selected or cancel it. A
/// prepared intent is confirmed through its own panel's Confirm (PX-030,
/// [`PANEL_CONFIRM`]); the generic Confirm `ACTION_CONFIRM` is gone.
const ACTION_BUTTON_CONSTANTS: &[&str] = &["ACTION_PREPARE", "ACTION_CANCEL"];

/// The saved views' buttons (PX-069), whose `ACTIVATE` handlers apply the
/// view the server holds in a slot through the presentation paths above,
/// save the presentation as a view, or rename, delete or restore the saved
/// views. None signals, opens for control or reconfigures a process; Rename
/// and Delete never reach the process list or the action gate.
const SAVED_VIEW_BUTTON_CONSTANTS: &[&str] = &[
    "SAVED_VIEW_SLOT_1",
    "SAVED_VIEW_SLOT_2",
    "SAVED_VIEW_SLOT_3",
    "SAVED_VIEW_SLOT_4",
    "SAVED_VIEW_SLOT_5",
    "SAVED_VIEW_SLOT_6",
    "SAVED_VIEW_SLOT_7",
    "SAVED_VIEW_SLOT_8",
    "SAVED_VIEW_CREATE",
    "SAVED_VIEW_RENAME",
    "SAVED_VIEW_DELETE",
    "SAVED_VIEWS_RESTORE",
];

/// The meter layout buttons (PX-070), whose `ACTIVATE` handlers re-arrange
/// the system summary's structure and, like the column chooser's, touch no
/// row and no process.
const METER_BUTTON_CONSTANTS: &[&str] = &[
    "METER_TOGGLE_CPU",
    "METER_EARLIER_CPU",
    "METER_LATER_CPU",
    "METER_MODE_CPU",
    "METER_TOGGLE_MEMORY",
    "METER_EARLIER_MEMORY",
    "METER_LATER_MEMORY",
    "METER_MODE_MEMORY",
    "METER_TOGGLE_SWAP",
    "METER_EARLIER_SWAP",
    "METER_LATER_SWAP",
    "METER_MODE_SWAP",
    "METER_TOGGLE_LOAD",
    "METER_EARLIER_LOAD",
    "METER_LATER_LOAD",
    "METER_TOGGLE_UPTIME",
    "METER_EARLIER_UPTIME",
    "METER_LATER_UPTIME",
    "METER_TOGGLE_PROCESSES",
    "METER_EARLIER_PROCESSES",
    "METER_LATER_PROCESSES",
    "METERS_HEADER",
    "METERS_RESTORE",
];

/// The text inputs whose `TEXT_EDIT` handlers are allowed: the filter input
/// (PX-010), the search and PID fields (PX-011), and the user and PID-set
/// fields (PX-012), and the sampling interval field (PX-015), and the saved
/// views' name field (PX-069).
const TEXT_INPUT_CONSTANTS: &[&str] = &[
    "FILTER_INPUT",
    "SEARCH_INPUT",
    "PID_INPUT",
    "OWNER_INPUT",
    "PID_SET_INPUT",
    "INTERVAL_INPUT",
    "SAVED_VIEW_NAME_INPUT",
];

/// Whether an `on(..)` argument list is one of the sort buttons' `ACTIVATE`
/// registrations: the first argument exactly a sort-button node constant, the
/// second exactly `ACTIVATE` (PX-009). A column chooser button constant is
/// accepted in the same exact shape (PX-068). A computed first argument,
/// another node, another event type, or a handler taken as a value is not the
/// allowance and stays refused.
fn is_sort_button_activation(tokens: &[Tok]) -> bool {
    let mut arguments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Tok::Punct(',', _) => arguments.push(std::mem::take(&mut current)),
            Tok::Ident(name, _, _) => current.push(name.clone()),
            // Anything else — an expression, a reference, a nested group —
            // makes the argument one this rule cannot name.
            _ => current.push('\u{0}'.to_string()),
        }
    }
    arguments.push(current);
    // Each argument must be exactly one plain identifier to be nameable here;
    // anything composite — a reference, an expression, a nested group — fails.
    let bare = |argument: Option<&Vec<String>>| -> Option<String> {
        match argument?.as_slice() {
            [only] if !only.is_empty() => Some(only.clone()),
            _ => None,
        }
    };
    let node = bare(arguments.first());
    let event = bare(arguments.get(1));
    matches!(
        node.as_deref(),
        Some(name) if SORT_BUTTON_CONSTANTS.contains(&name)
            || COLUMN_CHOOSER_CONSTANTS.contains(&name)
            || SEARCH_BUTTON_CONSTANTS.contains(&name)
            || OWNER_BUTTON_CONSTANTS.contains(&name)
            || SAMPLING_BUTTON_CONSTANTS.contains(&name)
            || TAG_BUTTON_CONSTANTS.contains(&name)
            || ACTION_BUTTON_CONSTANTS.contains(&name)
            || SAVED_VIEW_BUTTON_CONSTANTS.contains(&name)
            || METER_BUTTON_CONSTANTS.contains(&name)
    ) && event.as_deref() == Some("ACTIVATE")
}

/// Whether an `on(..)` argument list is a text input's `TEXT_EDIT`
/// registration: the first argument exactly `FILTER_INPUT` (PX-010) or the
/// search or PID field's constant (PX-011), the second exactly `TEXT_EDIT`.
fn is_filter_input_text_edit(tokens: &[Tok]) -> bool {
    let mut arguments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Tok::Punct(',', _) => arguments.push(std::mem::take(&mut current)),
            Tok::Ident(name, _, _) => current.push(name.clone()),
            _ => current.push('\u{0}'.to_string()),
        }
    }
    arguments.push(current);
    let bare = |argument: Option<&Vec<String>>| -> Option<String> {
        match argument?.as_slice() {
            [only] if !only.is_empty() => Some(only.clone()),
            _ => None,
        }
    };
    let node = bare(arguments.first());
    let event = bare(arguments.get(1));
    matches!(node.as_deref(), Some(name) if TEXT_INPUT_CONSTANTS.contains(&name))
        && event.as_deref() == Some("TEXT_EDIT")
}

/// The one selection input srtop handles (PX-013): the process table's
/// `SELECTION_CHANGED`, whose handler resolves the event's `ItemId` against
/// the published rows and describes that row in the inspector. It reads
/// details and never signals, opens for control or reconfigures a process.
const TABLE_SELECTION: (&str, &str) = ("TABLE", "SELECTION_CHANGED");

/// Whether an `on(..)` argument list is exactly the table's
/// `SELECTION_CHANGED` registration: the first argument the bare constant
/// `TABLE`, the second the bare constant `SELECTION_CHANGED`. Any other node,
/// any other event type, a path or a computed argument is not the allowance.
fn is_table_selection(tokens: &[Tok]) -> bool {
    let mut arguments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Tok::Punct(',', _) => arguments.push(std::mem::take(&mut current)),
            Tok::Ident(name, _, _) => current.push(name.clone()),
            _ => current.push('\u{0}'.to_string()),
        }
    }
    arguments.push(current);
    let bare = |argument: Option<&Vec<String>>| -> Option<String> {
        match argument?.as_slice() {
            [only] if !only.is_empty() => Some(only.clone()),
            _ => None,
        }
    };
    bare(arguments.first()).as_deref() == Some(TABLE_SELECTION.0)
        && bare(arguments.get(1)).as_deref() == Some(TABLE_SELECTION.1)
}

/// The follow toggle (PX-017), whose `VALUE_CHANGED` handler turns follow on
/// or off for the process the server already resolved as selected. It names
/// the followed row's item ID in the table's `keep_visible` hint and never
/// signals, opens for control or reconfigures a process.
const FOLLOW_TOGGLE_CHANGE: (&str, &str) = ("FOLLOW_TOGGLE", "VALUE_CHANGED");

/// Whether an `on(..)` argument list is exactly the follow toggle's
/// `VALUE_CHANGED` registration: the bare constants `FOLLOW_TOGGLE` and
/// `VALUE_CHANGED`. Any other node, event type, path or computed argument is
/// not the allowance.
fn is_follow_toggle(tokens: &[Tok]) -> bool {
    let mut arguments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Tok::Punct(',', _) => arguments.push(std::mem::take(&mut current)),
            Tok::Ident(name, _, _) => current.push(name.clone()),
            _ => current.push('\u{0}'.to_string()),
        }
    }
    arguments.push(current);
    let bare = |argument: Option<&Vec<String>>| -> Option<String> {
        match argument?.as_slice() {
            [only] if !only.is_empty() => Some(only.clone()),
            _ => None,
        }
    };
    bare(arguments.first()).as_deref() == Some(FOLLOW_TOGGLE_CHANGE.0)
        && bare(arguments.get(1)).as_deref() == Some(FOLLOW_TOGGLE_CHANGE.1)
}

/// The view switch and the process tree (PX-018): the switch's
/// `VALUE_CHANGED`, which republishes the same rows in the other collection,
/// and the tree's `SELECTION_CHANGED` and `EXPANSION_CHANGED`, which resolve
/// one published item ID for the inspector and record what the client
/// already expanded or collapsed; and the thread controls' `VALUE_CHANGED`
/// (PX-019), whose boolean chooses which tasks the collection holds and
/// whether the next sample reads threads. None signals, opens for control or
/// reconfigures a process.
const VIEW_AND_TREE_INPUTS: &[(&str, &str)] = &[
    ("VIEW_TOGGLE", "VALUE_CHANGED"),
    ("TREE", "SELECTION_CHANGED"),
    ("TREE", "EXPANSION_CHANGED"),
    ("USER_THREADS_TOGGLE", "VALUE_CHANGED"),
    ("KERNEL_THREADS_TOGGLE", "VALUE_CHANGED"),
];

/// Whether an `on(..)` argument list is exactly one of
/// [`VIEW_AND_TREE_INPUTS`]: both arguments bare constants, in that pair.
/// Any other node, event type, path or computed argument is not the
/// allowance.
fn is_view_or_tree_input(tokens: &[Tok]) -> bool {
    let mut arguments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for token in tokens {
        match token {
            Tok::Punct(',', _) => arguments.push(std::mem::take(&mut current)),
            Tok::Ident(name, _, _) => current.push(name.clone()),
            _ => current.push('\u{0}'.to_string()),
        }
    }
    arguments.push(current);
    let bare = |argument: Option<&Vec<String>>| -> Option<String> {
        match argument?.as_slice() {
            [only] if !only.is_empty() => Some(only.clone()),
            _ => None,
        }
    };
    let (Some(node), Some(event)) = (bare(arguments.first()), bare(arguments.get(1))) else {
        return false;
    };
    VIEW_AND_TREE_INPUTS
        .iter()
        .any(|(allowed_node, allowed_event)| node == *allowed_node && event == *allowed_event)
}

/// Every violation in the production tokens of one source file.
fn token_violations(file: &str, source: &str) -> Vec<String> {
    let stream = match TokenStream::from_str(source) {
        Ok(stream) => stream,
        Err(error) => return vec![format!("{file}: does not tokenize: {error}")],
    };
    let tokens = production(&lower(stream));
    let mut found = Vec::new();
    scan(&tokens, file, &mut |line, rule, what| {
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

/// The paths of the attributes the production tokens of one source file carry.
fn production_attributes(source: &str, paths: &mut BTreeSet<String>) {
    fn walk(level: &[Tok], paths: &mut BTreeSet<String>) {
        for (i, tok) in level.iter().enumerate() {
            match tok {
                Tok::Punct('#', _) => {
                    let at = if is_punct(level.get(i + 1), '!') {
                        i + 2
                    } else {
                        i + 1
                    };
                    if let Some(Tok::Group(Delimiter::Bracket, attribute, _)) = level.get(at) {
                        paths.insert(attribute_path(attribute).0);
                    }
                }
                Tok::Group(_, inner, _) => walk(inner, paths),
                _ => {}
            }
        }
    }
    walk(
        &production(&lower(
            TokenStream::from_str(source).expect("a source that tokenizes"),
        )),
        paths,
    );
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

/// Each construct listed here is reported under its rule, including each one that
/// got past an earlier version of this test (review probes P1–P7, verifier
/// mutants N1–N8, round-two findings V2-1 and V2-2); look-alikes srtop really
/// uses are not, its attributes among them; comments and literals of every kind
/// neither hide code nor count as code.
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
        // the same methods as paths: called (UFCS) or taken as a value (V2-2)
        (
            "V2-2 UFCS write_all",
            "let mut f = std::fs::File::open(p)?;\nstd::io::Write::write_all(&mut f, b\"HACKED\")?;",
            "method",
        ),
        ("UFCS write", "Write::write(&mut f, b\"1\")?;", "method"),
        (
            "qualified UFCS",
            "<std::fs::File as std::io::Write>::write_all(&mut f, b\"1\")?;",
            "method",
        ),
        ("write_all as a value", "let w = std::io::Write::write_all;", "method"),
        ("write_fmt", "f.write_fmt(format_args!(\"1\"))?;", "method"),
        (
            "UFCS write_fmt",
            "io::Write::write_fmt(&mut f, format_args!(\"1\"))?;",
            "method",
        ),
        (
            "UFCS write_at",
            "std::os::unix::fs::FileExt::write_at(&f, b\"1\", 0)?;",
            "method",
        ),
        ("UFCS set_len", "std::fs::File::set_len(&f, 0)?;", "file"),
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
        // any attribute outside the allowlist, also where `cfg_attr` applies it (V2-1)
        (
            "V2-1 cfg_attr(not(test), path)",
            "#[cfg_attr(not(test), path = \"../evil.rs\")]\nmod evil;",
            "attribute",
        ),
        (
            "V2-1 cfg_attr(unix, path)",
            "#[cfg_attr(unix, path = \"../evil.rs\")]\nmod evil;",
            "attribute",
        ),
        (
            "nested cfg_attr",
            "#[cfg_attr(unix, cfg_attr(not(test), path = \"../evil.rs\"))]\nmod evil;",
            "attribute",
        ),
        (
            "cfg_attr path after an allowed attribute",
            "#[cfg_attr(unix, allow(dead_code), path = \"../evil.rs\")]\nmod evil;",
            "attribute",
        ),
        (
            "cfg_attr link",
            "#[cfg_attr(unix, link(name = \"c\"))]\nextern \"C\" {}",
            "attribute",
        ),
        (
            "cfg_attr export_name",
            "#[cfg_attr(unix, export_name = \"kill\")]\npub fn hook() {}",
            "attribute",
        ),
        (
            "cfg_attr no_mangle",
            "#[cfg_attr(unix, no_mangle)]\npub fn hook() {}",
            "attribute",
        ),
        (
            "cfg_attr link_section",
            "#[cfg_attr(unix, link_section = \".init_array\")]\npub static HOOK: fn() = hook;",
            "attribute",
        ),
        ("inner cfg_attr", "#![cfg_attr(not(test), no_main)]", "attribute"),
        ("#[unsafe(no_mangle)]", "#[unsafe(no_mangle)]\npub fn hook() {}", "attribute"),
        ("#[used]", "#[used]\nstatic HOOK: u8 = 0;", "attribute"),
        ("an attribute macro", "#[evil::rewrite]\nfn f() {}", "attribute"),
        ("a derive macro", "#[derive(Debug, Spawner)]\nstruct S;", "attribute"),
        ("an assembled attribute", "#[$name = \"../evil.rs\"]\nmod evil;", "attribute"),
        ("a malformed cfg_attr", "#[cfg_attr]\nmod m {}", "attribute"),
        // the same, assembled by a macro or brought in under another name
        (
            "a macro assembling an attribute",
            "macro_rules! hashed { ($h:tt) => { $h[path = \"../evil.rs\"] mod evil; } }\nhashed!(#);",
            "ident",
        ),
        (
            "a macro assembling a path",
            "macro_rules! call { ($f:ident) => { std::fs::$f(\"x\", \"y\") } }\ncall!(write);",
            "ident",
        ),
        (
            "include! renamed",
            "use std::include as inc;\nconst CODE: &str = inc!(\"../outside.rs\");",
            "include",
        ),
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
        // client input reaching application code (N8). The sort buttons'
        // ACTIVATE registrations are the one allowance, and only in the exact
        // shape PX-009 installs them: a bare sort-button constant, then
        // ACTIVATE. Everything else — another node, another event type, a
        // computed argument — is still a handler this audit refuses.
        ("session.on", "session.on(TABLE, ACTIVATE, |_, _| {});", "handler"),
        (
            "Session::on as a plain call (N8)",
            "Session::on(&session, TABLE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        ("on as a value", "let install = Session::on;", "handler"),
        (
            "a handler on another node",
            "session.on(KILL_BUTTON, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a sort button under another event",
            "session.on(SORT_PID, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a filter input under another event",
            "session.on(FILTER_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed filter handler target",
            "session.on(inputs[i], TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "a computed sort handler target",
            "session.on(buttons[i], ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a column chooser button under another event",
            "session.on(COLUMN_TOGGLE_CPU, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "a computed column chooser target",
            "session.on(COLUMN_CHOOSER_BUTTONS[i].0, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a sort button by path with a receiver",
            "Session::on(&session, SORT_PID, ACTIVATE, |_, _| {});",
            "handler",
        ),
        // PX-011: the search controls are allowed only in their exact shape.
        (
            "a search button under another event",
            "session.on(SEARCH_NEXT, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the PID button under a selection event",
            "session.on(PID_GO, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the PID field under ACTIVATE",
            "session.on(PID_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the search field under ACTIVATE",
            "session.on(SEARCH_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed search target",
            "session.on(search_buttons[i], ACTIVATE, |_, _| {});",
            "handler",
        ),
        // PX-012: the user-filter controls are allowed only in their exact shape.
        (
            "a user-filter button under another event",
            "session.on(OWNER_MINE, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the all-users button under a selection event",
            "session.on(OWNER_ALL, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the user field under ACTIVATE",
            "session.on(OWNER_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the PID-set field under ACTIVATE",
            "session.on(PID_SET_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed user-filter target",
            "session.on(owner_controls[i], TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        // PX-015: the sampling controls are allowed only in their exact shape.
        (
            "the pause button under a text edit",
            "session.on(SAMPLING_PAUSE, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the refresh button under a selection event",
            "session.on(SAMPLING_REFRESH, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the apply-interval button under a value change",
            "session.on(INTERVAL_APPLY, VALUE_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the interval field under ACTIVATE",
            "session.on(INTERVAL_INPUT, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the sampling status as a button",
            "session.on(SAMPLING_STATUS, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the sampling row as a button",
            "session.on(SAMPLING_ROW, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed sampling target",
            "session.on(sampling_buttons[i], ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a sampling button by path",
            "session.on(crate::SAMPLING_PAUSE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        // PX-028: the action buttons' ACTIVATE is allowed only in its exact
        // shape; their status, another event type or a computed target is not.
        (
            "an action button under a value change",
            "session.on(ACTION_CONFIRM, VALUE_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the actions status as a button",
            "session.on(ACTIONS_STATUS, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the actions row as a button",
            "session.on(ACTIONS_ROW, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed action target",
            "session.on(action_buttons[i], ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "an action button by path",
            "session.on(crate::ACTION_PREPARE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        // PX-013: the table's SELECTION_CHANGED is allowed only in its exact
        // shape: the bare `TABLE` and the bare `SELECTION_CHANGED`.
        (
            "the table under a text edit",
            "session.on(TABLE, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the table under another collection event",
            "session.on(TABLE, EXPANSION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a selection handler on another node",
            "session.on(MODEL, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a selection handler on a button",
            "session.on(SEARCH_NEXT, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a computed selection target",
            "session.on(tables[i], SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a path-qualified table",
            "session.on(crate::TABLE, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a computed selection event type",
            "session.on(TABLE, events[i], |_, _| {});",
            "handler",
        ),
        (
            "the table selection by path with a receiver",
            "Session::on(&session, TABLE, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        // PX-018: the view switch's VALUE_CHANGED and the tree's
        // SELECTION_CHANGED and EXPANSION_CHANGED are allowed only in their
        // exact shapes.
        (
            "the view switch under ACTIVATE",
            "session.on(VIEW_TOGGLE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the view status as a toggle",
            "session.on(VIEW_STATUS, VALUE_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the tree under a text edit",
            "session.on(TREE, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the tree under a value change",
            "session.on(TREE, VALUE_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the tree's model as a tree",
            "session.on(TREE_MODEL, EXPANSION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a path-qualified tree",
            "session.on(crate::TREE, EXPANSION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a computed tree target",
            "session.on(trees[i], SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "a computed view switch event",
            "session.on(VIEW_TOGGLE, events[i], |_, _| {});",
            "handler",
        ),
        // PX-020: the tag buttons are allowed only in their exact shape.
        (
            "a tag button under a value change",
            "session.on(TAG_TOGGLE, VALUE_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the descendants button under a selection event",
            "session.on(TAG_DESCENDANTS, SELECTION_CHANGED, |_, _| {});",
            "handler",
        ),
        (
            "the clear button under a text edit",
            "session.on(TAGS_CLEAR, TEXT_EDIT, |_, _| {});",
            "handler",
        ),
        (
            "the tags status as a button",
            "session.on(TAGS_STATUS, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "the tags row as a button",
            "session.on(TAGS_ROW, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a computed tag target",
            "session.on(tag_buttons[i], ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a tag button by path",
            "session.on(crate::TAG_TOGGLE, ACTIVATE, |_, _| {});",
            "handler",
        ),
        (
            "a tag button under a computed event",
            "session.on(TAGS_CLEAR, events[i], |_, _| {});",
            "handler",
        ),
        (
            "the SDK's disclosure handler",
            "tree.on_expansion_changed(&session, |_, _| {});",
            "ident",
        ),
        (
            "a table selection through on_result",
            "session.on_result(TABLE, SELECTION_CHANGED, |_, _| Ok(()));",
            "ident",
        ),
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
        // the sort controls' ACTIVATE registrations, exactly as PX-009 installs
        // them: the audit's one client-input allowance
        "session.on(SORT_PID, ACTIVATE, move |session, _| { sort_clicked(&pid, session, ProcessField::Pid); });",
        "session.on(SORT_NAME, ACTIVATE, move |session, _| { sort_clicked(&name, session, ProcessField::Name); });",
        "session.on(SORT_RESIDENT, ACTIVATE, move |session, _| { sort_clicked(&resident, session, ProcessField::Resident); });",
        "session.on(SORT_CPU, ACTIVATE, move |session, _| { sort_clicked(&cpu, session, ProcessField::Cpu); });",
        // the column chooser's ACTIVATE registrations, as PX-068 installs them
        "session.on(COLUMN_TOGGLE_PID, ACTIVATE, handler(Toggle(Pid)));",
        "session.on(COLUMNS_RESTORE, ACTIVATE, handler(RestoreDefaults));",
        // the filter input's TEXT_EDIT registration (PX-010)
        "session.on(FILTER_INPUT, TEXT_EDIT, move |session, event| { refresh::filter_edited(&view, session, event); });",
        // the search controls' registrations, as PX-011 installs them
        "session.on(SEARCH_INPUT, TEXT_EDIT, move |session, event| { refresh::search_edited(&edited, session, event); });",
        "session.on(SEARCH_PREVIOUS, ACTIVATE, move |session, _| { refresh::search_stepped(&previous, session, search::SearchStep::Previous); });",
        "session.on(SEARCH_NEXT, ACTIVATE, move |session, _| { refresh::search_stepped(&next, session, search::SearchStep::Next); });",
        "session.on(PID_INPUT, TEXT_EDIT, move |session, event| { refresh::pid_edited(&pid, session, event); });",
        "session.on(PID_GO, ACTIVATE, move |session, _| { refresh::pid_requested(&go, session); });",
        // the user filter's registrations, as PX-012 installs them
        "session.on(OWNER_MINE, ACTIVATE, move |session, _| { refresh::owner_chosen(&mine, session, owner::OwnerChoice::Current); });",
        "session.on(OWNER_ALL, ACTIVATE, move |session, _| { refresh::owner_chosen(&all, session, owner::OwnerChoice::AllVisible); });",
        "session.on(OWNER_INPUT, TEXT_EDIT, move |session, event| { refresh::owner_edited(&user, session, event); });",
        "session.on(PID_SET_INPUT, TEXT_EDIT, move |session, event| { refresh::pid_set_edited(&pids, session, event); });",
        // the inspector's one selection input, as PX-013 installs it
        "session.on(TABLE, SELECTION_CHANGED, move |session, event| { refresh::selection_changed(&selected, session, event); });",
        // the sampling controls' registrations, as PX-015 installs them
        "session.on(SAMPLING_PAUSE, ACTIVATE, move |session, _| { refresh::pause_toggled(&pause, session); });",
        "session.on(SAMPLING_REFRESH, ACTIVATE, move |session, _| { refresh::refresh_requested(&now, session); });",
        "session.on(INTERVAL_INPUT, TEXT_EDIT, move |session, event| { refresh::interval_edited(&interval, session, event); });",
        "session.on(INTERVAL_APPLY, ACTIVATE, move |session, _| { refresh::interval_requested(&apply, session); });",
        // the tag buttons' registrations, as PX-020 installs them
        "session.on(TAG_TOGGLE, ACTIVATE, move |session, _| { refresh::tag_requested(&toggle, session, refresh::TagRequest::Toggle); });",
        "session.on(TAG_DESCENDANTS, ACTIVATE, move |session, _| { refresh::tag_requested(&descendants, session, refresh::TagRequest::Descendants); });",
        "session.on(TAGS_CLEAR, ACTIVATE, move |session, _| { refresh::tag_requested(&clear, session, refresh::TagRequest::Clear); });",
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
        "use std::fmt::Write as _;",
        "let _ = std::write!(text, \"{x}\"); let _ = writeln!(text);",
        // the attributes srtop's production code carries
        "//! A crate doc.\n/// A doc.\n#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]\npub struct Row;",
        "#[must_use]\npub fn kept() -> u8 { 0 }",
        "#[must_use = \"the hook is removed as soon as this guard is dropped\"]\npub struct Guard;",
        "#[tokio::main]\nasync fn main() {}",
        // and other harmless ones, also where `cfg_attr` applies them
        "#[cfg(target_os = \"linux\")]\nfn linux_only() {}",
        "#[allow(clippy::too_many_lines)]\n#[expect(dead_code)]\n#[inline]\nfn f() {}",
        "#[cfg_attr(test, derive(PartialOrd, Ord))]\n#[cfg_attr(unix, allow(dead_code), must_use)]\nstruct T;",
        "#[cfg_attr(all(unix, not(test)), cfg_attr(target_os = \"linux\", inline))]\nfn g() {}",
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
            "N2 rustix (libc is allowed since PX-029, for src/handle.rs only)",
            with(&dependency("rustix", "null", "null", "null")),
            good_targets.clone(),
            manifest.to_string(),
            "normal dependency rustix",
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
    assert!(
        files.iter().any(|path| path.ends_with(HANDLE_MODULE)),
        "the pinned unsafe blocks are checked against the handle module that holds them"
    );
    assert!(
        files.iter().any(|path| path.ends_with(SETTINGS_MODULE)),
        "the pinned settings writes are checked against the module that holds them"
    );
    let mut found = Vec::new();
    let mut receivers = 0;
    let mut attributes = BTreeSet::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("a readable source file");
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        found.extend(token_violations(&name, &text));
        if name == SETTINGS_MODULE {
            found.extend(
                settings_write_violations(&text)
                    .into_iter()
                    .map(|violation| format!("{name}: [settings] {violation}")),
            );
        }
        found.extend(settings_store_violations(&name, &text));
        if name == HANDLE_MODULE {
            found.extend(
                handle_unsafe_violations(&text)
                    .into_iter()
                    .map(|violation| format!("{name}: [unsafe] {violation}")),
            );
            found.extend(
                handle_signal_violations(&text)
                    .into_iter()
                    .map(|violation| format!("{name}: [signal] {violation}")),
            );
        }
        receivers += signal_receivers(&text);
        production_attributes(&text, &mut attributes);
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
         {RECEIVED_SIGNALS:?}; file access through {FILE_ALLOWED:?} and fs {FS_ALLOWED:?} only; \
         attributes {attributes:?}",
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

/// Every `impl <trait> for <type>` in the production tokens of `source`
/// whose trait path ends in `ProcessActions`: the implementing type's first
/// identifier, or `?` when it is not a plain one.
fn process_actions_implementations(source: &str) -> Vec<String> {
    fn walk(level: &[Tok], found: &mut Vec<String>) {
        for (i, tok) in level.iter().enumerate() {
            match tok {
                Tok::Ident(name, _, _) if name == "ProcessActions" => {
                    let implemented = level[..i]
                        .iter()
                        .rev()
                        .take_while(|tok| !matches!(tok, Tok::Group(..) | Tok::Punct(';', _)))
                        .any(|tok| matches!(tok, Tok::Ident(name, _, _) if name == "impl"));
                    if implemented && ident(level.get(i + 1)) == Some("for") {
                        found.push(ident(level.get(i + 2)).unwrap_or("?").to_string());
                    }
                }
                Tok::Group(_, inner, _) => walk(inner, found),
                _ => {}
            }
        }
    }
    let stream = TokenStream::from_str(source).expect("a source file tokenizes");
    let mut found = Vec::new();
    walk(&production(&lower(stream)), &mut found);
    found
}

/// PX-029: the handle module's allowance is exactly its two system calls.
/// Each construct below is refused in `src/handle.rs` itself, and what the
/// module really writes passes there and is refused in any other file.
/// PX-069: the settings store is the one file that may write, only through a
/// fresh temporary file (`create_new`) renamed into place; the same
/// constructs anywhere else, and every other rule there, stay refused.
#[test]
fn the_settings_allowance_is_exactly_the_settings_modules_atomic_writes() {
    let writer = "use std::fs::{self, File, OpenOptions};\n\
                  use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};\n\
                  fn replace(temporary: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<()> {\n\
                      fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;\n\
                      let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(temporary)?;\n\
                      file.write_all(bytes)?; file.sync_all()?;\n\
                      let _ = fs::remove_file(temporary);\n\
                      fs::rename(temporary, target)\n\
                  }";
    assert_eq!(
        token_violations(SETTINGS_MODULE, writer),
        Vec::<String>::new()
    );
    assert_eq!(settings_write_violations(writer), Vec::<String>::new());
    // The same writer in any other file is refused, rule by rule.
    let elsewhere = token_violations("src/refresh.rs", writer);
    for rule in ["[ident]", "[method]", "[fs]", "[fs-import]"] {
        assert!(
            elsewhere.iter().any(|violation| violation.contains(rule)),
            "{rule} not reported outside the settings module: {elsewhere:?}"
        );
    }
    // Inside the settings module, an in-place rewrite and a second open are
    // refused by the pin, and every other rule still holds.
    for (name, snippet) in [
        (
            "truncate",
            writer.replace(".create_new(true)", ".create_new(true).truncate(true)"),
        ),
        (
            "append",
            writer.replace(".create_new(true)", ".create_new(true).append(true)"),
        ),
        (
            "create(true)",
            writer.replace(".create_new(true)", ".create(true)"),
        ),
        (
            "a second OpenOptions",
            format!("{writer}\nfn again() {{ let _ = OpenOptions::new().write(true).open(p); }}"),
        ),
        // PX-069 verification round 1, F3.
        (
            "create_new(false)",
            writer.replace(".create_new(true)", ".create_new(false)"),
        ),
        (
            "create_new(computed)",
            writer.replace(".create_new(true)", ".create_new(fresh)"),
        ),
        (
            "a cloned builder with custom_flags(O_TRUNC)",
            format!(
                "{writer}\nfn v2(path: &Path, options: &OpenOptions) {{ let mut again = options.clone(); again.custom_flags(0o1000); let _ = again.open(path); }}"
            ),
        ),
        (
            "custom_flags alone",
            writer.replace(".mode(0o600)", ".mode(0o600).custom_flags(0o2000)"),
        ),
        (
            "a builder copied by to_owned",
            format!("{writer}\nfn v2(o: &OpenOptions) {{ let _ = o.to_owned().open(p); }}"),
        ),
        (
            "a builder copied by Clone::clone",
            format!("{writer}\nfn v2(o: &OpenOptions) {{ let _ = Clone::clone(o).open(p); }}"),
        ),
    ] {
        assert!(
            !settings_write_violations(&snippet).is_empty(),
            "{name} passed the settings pin"
        );
    }
    for (name, snippet, rule) in [
        (
            "Command",
            "let _ = std::process::Command::new(\"x\");",
            "ident",
        ),
        ("kill", "kill(1, 9);", "ident"),
        ("fs::write", "std::fs::write(p, b);", "fs"),
        ("File::create", "let _ = File::create(p);", "file"),
        ("set_permissions", "file.set_permissions(mode);", "method"),
        // V1 of verification round 1: an in-place rewrite through `File::options`.
        (
            "File::options",
            "if let Ok(mut file) = File::options().write(true).open(path) { let _ = file.write_all(b\"x\"); }",
            "file",
        ),
        (
            "a handler",
            "session.on(TABLE, ACTIVATE, |_, _| {});",
            "handler",
        ),
    ] {
        let found = token_violations(SETTINGS_MODULE, snippet);
        assert!(
            found
                .iter()
                .any(|violation| violation.contains(&format!("[{rule}]"))),
            "{name} was not reported in the settings module: {found:?}"
        );
    }
    // The saved views' registrations are the exact shapes PX-069 installs.
    for innocent in [
        "session.on(SAVED_VIEW_SLOT_1, ACTIVATE, slot(0));",
        "session.on(SAVED_VIEW_SLOT_8, ACTIVATE, slot(7));",
        "session.on(SAVED_VIEW_RENAME, ACTIVATE, move |session, _| { rename(session); });",
        "session.on(SAVED_VIEW_NAME_INPUT, TEXT_EDIT, move |session, event| { named(session, event); });",
    ] {
        assert_eq!(
            token_violations("src/lib.rs", innocent),
            Vec::<String>::new(),
            "{innocent}"
        );
    }
    // Only `main` constructs a store (F3 of verification round 1, W1): every
    // form of a construction is counted, and a renamed or aliased type is
    // reported, so another file cannot write at a path of its choosing.
    for (snippet, constructions) in [
        (
            "let store = crate::settings_file::SettingsStore::new(path_from_client);",
            1,
        ),
        ("let store = <SettingsStore>::new(path);", 1),
        (
            "let a = SettingsStore::new(p); let b = SettingsStore::new(q);",
            2,
        ),
        (
            "fn f(store: &SettingsStore) { store.save(&settings).ok(); }",
            0,
        ),
    ] {
        assert_eq!(
            settings_store_constructions(snippet),
            (constructions, Vec::new()),
            "{snippet}"
        );
    }
    for hidden in [
        "use crate::settings_file::SettingsStore as Store;",
        "type Store = crate::settings_file::SettingsStore;",
    ] {
        assert!(
            !settings_store_constructions(hidden).1.is_empty(),
            "{hidden}"
        );
    }
    // The per-file policy: one construction in `main`, none elsewhere.
    let one = "let store = SettingsStore::new(path);";
    assert_eq!(
        settings_store_violations(SETTINGS_STORE_CONSTRUCTOR, one),
        Vec::<String>::new()
    );
    assert!(!settings_store_violations("src/zz_verify1_store_bypass.rs", one).is_empty());
    assert!(!settings_store_violations(SETTINGS_MODULE, one).is_empty());
    assert!(
        !settings_store_violations(SETTINGS_STORE_CONSTRUCTOR, &format!("{one}\n{one}")).is_empty()
    );
    assert!(!settings_store_violations(SETTINGS_STORE_CONSTRUCTOR, "fn f() {}").is_empty());
    // Verification round 2, V2-3: the other ways to make a store.
    let module = "pub struct SettingsStore { path: PathBuf, interrupt: Option<SaveStep> }\n\
                  impl SettingsStore {\n\
                      pub fn new(path: impl Into<PathBuf>) -> Self { Self { path: path.into(), interrupt: None } }\n\
                      pub fn interrupted_at(mut self, step: SaveStep) -> Self { self.interrupt = Some(step); self }\n\
                  }";
    assert_eq!(
        settings_store_violations(SETTINGS_MODULE, module),
        Vec::<String>::new()
    );
    for (name, file, snippet) in [
        (
            "B1 an impl block in another file",
            "src/zz_verify2_b1_impl_block.rs",
            "impl crate::settings_file::SettingsStore { pub fn verify2_at(path: PathBuf) -> Self { Self::new(path) } }".to_string(),
        ),
        (
            "B1 an impl block alone in another file",
            "src/zz.rs",
            "impl SettingsStore { pub fn at(path: PathBuf) -> Self { todo!() } }".to_string(),
        ),
        (
            "B3 a trait impl alone in the settings module",
            SETTINGS_MODULE,
            format!("{module}\nimpl Default for SettingsStore {{ fn default() -> Self {{ todo!() }} }}"),
        ),
        (
            "B1 a trait impl in another file",
            "src/zz.rs",
            "impl From<PathBuf> for SettingsStore { fn from(p: PathBuf) -> Self { todo!() } }".to_string(),
        ),
        (
            "B2 a second literal in the settings module",
            SETTINGS_MODULE,
            format!("{module}\npub fn verify2_put(target: &Path, bytes: &[u8]) -> Result<(), SaveError> {{ let store = SettingsStore {{ path: target.to_path_buf(), interrupt: None }}; store.replace(target, bytes, None) }}"),
        ),
        (
            "B2 a literal outside fn new",
            SETTINGS_MODULE,
            module.replace(
                "pub fn new(path: impl Into<PathBuf>) -> Self { Self { path: path.into(), interrupt: None } }",
                "pub fn at(path: impl Into<PathBuf>) -> Self { Self { path: path.into(), interrupt: None } }",
            ),
        ),
        (
            "B3 impl Default",
            SETTINGS_MODULE,
            format!("{module}\nimpl Default for SettingsStore {{ fn default() -> Self {{ Self::new(\"/tmp/anything\") }} }}"),
        ),
        (
            "B3 Self::new in the settings module",
            SETTINGS_MODULE,
            format!("{module}\nimpl SettingsStore {{ pub fn tmp() -> Self {{ Self::new(\"/tmp/x\") }} }}"),
        ),
        (
            "a literal in another file",
            "src/zz.rs",
            "let s = SettingsStore { path, interrupt: None };".to_string(),
        ),
        (
            "B5 the constructor taken as a value",
            "src/zz.rs",
            "pub fn f(p: Option<PathBuf>) { let _ = p.map(SettingsStore::new); }".to_string(),
        ),
        (
            "B6 a re-export under the same name",
            "src/zz.rs",
            "pub use crate::settings_file::SettingsStore as SettingsStore;".to_string(),
        ),
    ] {
        assert!(
            !settings_store_violations(file, &snippet).is_empty(),
            "{name} passed the store pin"
        );
    }
    for bypass in [
        "session.on(SAVED_VIEW_SLOT_1, VALUE_CHANGED, slot(0));",
        "session.on(SAVED_VIEW_NAME_INPUT, ACTIVATE, |_, _| {});",
        "session.on(SAVED_VIEW_SLOTS[0], ACTIVATE, slot(0));",
        "session.on(slot_node(3), ACTIVATE, slot(3));",
    ] {
        assert!(reported(bypass, "handler"), "{bypass}");
    }
}

#[test]
fn the_pidfd_allowance_is_exactly_the_handle_modules_two_system_calls() {
    let in_handle = |snippet: &str| token_violations(HANDLE_MODULE, snippet);
    for (name, snippet) in [
        ("libc::kill", "let _ = libc::kill(pid, 15);"),
        (
            "libc::kill in an unsafe block",
            "let _ = unsafe { libc::kill(pid, libc::SIGTERM) };",
        ),
        (
            "a leading-path libc::kill",
            "let _ = ::libc::kill(pid, 15);",
        ),
        ("kill imported", "use libc::kill;"),
        ("libc renamed", "use libc as c;"),
        ("libc imported bare", "use libc;"),
        ("libc glob", "use libc::*;"),
        ("libc grouped", "use libc::{syscall, SIGTERM};"),
        ("syscall renamed", "use libc::syscall as raw;"),
        ("syscall as a value", "let call = libc::syscall;"),
        ("syscall by number", "let _ = libc::syscall(62, pid, 9);"),
        (
            "syscall SYS_kill",
            "let _ = libc::syscall(libc::SYS_kill, pid, 9);",
        ),
        (
            "syscall SYS_tgkill",
            "let _ = libc::syscall(libc::SYS_tgkill, pid, pid, 9);",
        ),
        (
            "syscall with a computed number",
            "let _ = libc::syscall(number, pid, 9);",
        ),
        (
            "syscall unqualified",
            "let _ = syscall(libc::SYS_pidfd_open, pid, 0);",
        ),
        ("another libc item", "let _ = libc::SIGKILL;"),
        (
            "libc::setpriority",
            "libc::setpriority(libc::PRIO_PROCESS, 0, 19);",
        ),
        (
            "an extern block",
            "extern \"C\" { fn kill(pid: i32, sig: i32) -> i32; }",
        ),
        (
            "nix kill",
            "let _ = nix::sys::signal::kill(Pid::from_raw(pid), None);",
        ),
        (
            "a kill command",
            "std::process::Command::new(\"kill\").arg(pid).status();",
        ),
        ("an unsafe fn", "unsafe fn f() {}"),
    ] {
        assert!(
            in_handle(snippet).iter().any(|v| v.contains("[ident]")),
            "{name} is refused in the handle module: {:?}",
            in_handle(snippet)
        );
    }

    // Each layer refuses `libc::kill` on its own: the `libc` item rule and
    // the `kill` rule, so widening either one alone is still caught here.
    for snippet in [
        "let _ = libc::kill(pid, 15);",
        "let _ = unsafe { libc::kill(pid, libc::SIGTERM) };",
    ] {
        let found = in_handle(snippet);
        for word in ["`libc`", "`kill`"] {
            assert!(
                found.iter().any(|v| v.contains(word)),
                "{word} is refused in {snippet}: {found:?}"
            );
        }
    }

    // What the module writes passes there, and nowhere else.
    let real = "let pid = libc::pid_t::try_from(pid);\n\
                let result = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) };\n\
                let fd = unsafe { OwnedFd::from_raw_fd(fd) };\n\
                let number = libc::SIGTERM;\n\
                let result = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, number, std::ptr::null::<u8>(), 0 as libc::c_uint) };";
    assert_eq!(in_handle(real), Vec::<String>::new());
    for elsewhere in [
        "src/actions.rs",
        // PX-030: the termination executor names the handle module, and
        // still makes no system call of its own.
        EXECUTOR_MODULE,
        "src/handle/inner.rs",
        "handle.rs",
        "snippet.rs",
    ] {
        let found = token_violations(elsewhere, real);
        for word in ["unsafe", "libc", "syscall"] {
            assert!(
                found.iter().any(|v| v.contains(&format!("`{word}`"))),
                "{word} is refused in {elsewhere}: {found:?}"
            );
        }
    }

    // Every `unsafe` is pinned token for token.
    let module = std::fs::read_to_string(crate_root().join(HANDLE_MODULE)).unwrap();
    assert_eq!(handle_unsafe_violations(&module), Vec::<String>::new());
    for (name, extra) in [
        (
            "another block",
            "fn f() { unsafe { std::ptr::write(p, 1) } }",
        ),
        (
            "a changed argument",
            "fn f() { unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 1 as libc::c_uint) } }",
        ),
        ("an unsafe fn", "unsafe fn f() {}"),
        ("an unsafe impl", "unsafe impl Send for Handle {}"),
        (
            "an unsafe extern",
            "unsafe extern \"C\" { fn kill(pid: i32, sig: i32) -> i32; }",
        ),
        (
            "a pinned block twice",
            "fn f() { unsafe { OwnedFd::from_raw_fd(fd) } }",
        ),
        (
            "a pinned block in test code",
            "#[cfg(test)] mod t { fn f() { unsafe { OwnedFd::from_raw_fd(fd) } } }",
        ),
    ] {
        assert!(
            !handle_unsafe_violations(&format!("{module}\n{extra}")).is_empty(),
            "{name} is refused"
        );
    }
    assert!(
        handle_unsafe_violations("fn f() {}")
            .iter()
            .any(|v| v.contains("is gone")),
        "a pinned block that is gone is reported, so the pins stay exact"
    );

    // The signal sent is pinned: `Terminate` is `SIGTERM`, and nothing else
    // reaches the call.
    assert_eq!(handle_signal_violations(&module), Vec::<String>::new());
    for (name, from, to) in [
        (
            "a number",
            "Self::Terminate => libc::SIGTERM,",
            "Self::Terminate => 9,",
        ),
        (
            "SIGKILL",
            "Self::Terminate => libc::SIGTERM,",
            "Self::Terminate => libc::SIGKILL,",
        ),
        (
            "a number at the call",
            "let number = signal.number();",
            "let number = 9;",
        ),
        (
            "an offset at the call",
            "let number = signal.number();",
            "let number = signal.number() + 0;",
        ),
        (
            "a shadowing number",
            "let number = signal.number();",
            "let number = signal.number();\nlet number = 9;",
        ),
        (
            "a second mapping",
            "Self::Terminate => libc::SIGTERM,",
            "Self::Terminate => libc::SIGTERM,\n}\n}\nfn kill_number() -> i32 { libc::SIGKILL",
        ),
    ] {
        assert_eq!(
            module.matches(from).count(),
            1,
            "{name}: the anchor is in the module"
        );
        assert!(
            !handle_signal_violations(&module.replacen(from, to, 1)).is_empty(),
            "{name} is refused"
        );
    }
    assert!(
        in_handle("pub fn number(self) -> i32 { match self { Self::Terminate => libc::SIGKILL } }")
            .iter()
            .any(|v| v.contains("`libc`")),
        "SIGKILL is not a libc item the handle module may name"
    );

    // No other production file reaches the module.
    for snippet in [
        "use crate::handle::ProcessHandles;",
        "let table = crate::handle::ProcessHandles::live();",
        "use crate::handle;\nfn f() { handle::LivePidfds; }",
        "use super::handle as h;",
        "pub use handle::*;",
        "fn f(table: &mut ProcessHandles) {}",
        "let _ = HandleSignal::Terminate;",
        "impl PidfdSystem for Mine {}",
    ] {
        assert!(
            token_violations("src/actions.rs", snippet)
                .iter()
                .any(|v| v.contains("[handle]")),
            "{snippet} is refused outside the handle module"
        );
    }
    for innocent in [
        "pub mod handle;",
        "fn fd_link(handle: &File) -> PathBuf { todo!() }",
        "let fd = handle.as_raw_fd();",
        "Execution::Unsupported",
    ] {
        assert_eq!(
            token_violations("src/lib.rs", innocent),
            Vec::<String>::new(),
            "{innocent}"
        );
    }
}

/// Every production source file, by its path under the crate root.
fn production_files() -> Vec<(String, String)> {
    let root = crate_root();
    let (mut files, mut links) = (Vec::new(), Vec::new());
    rust_files(&root.join("src"), &mut files, &mut links);
    assert!(links.is_empty(), "{links:?}");
    files
        .iter()
        .map(|path| {
            (
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
                std::fs::read_to_string(path).expect("a readable source file"),
            )
        })
        .collect()
}

/// Every production `ProcessActions` implementation as (type, file).
fn production_executors(files: &[(String, String)]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = files
        .iter()
        .flat_map(|(file, text)| {
            process_actions_implementations(text)
                .into_iter()
                .map(move |name| (name, file.clone()))
        })
        .collect();
    found.sort();
    found
}

/// PX-028, PX-030: the action gate reaches a process only through a
/// `ProcessActions` executor, and production code has exactly two: the
/// dry-run executor, which sends nothing, and the PID file descriptor
/// terminator, the one that can, each in its own file. A third fails here
/// until this rule names it, under review. PX-029's process handles are not
/// an executor and implement no `ProcessActions`.
#[test]
fn the_production_action_executors_are_the_dry_run_executor_and_the_pidfd_terminator() {
    assert_eq!(
        process_actions_implementations(
            "impl ProcessActions for DryRunExecutor {}\n\
             impl crate::actions::ProcessActions for Other {}\n\
             #[cfg(test)] impl ProcessActions for Recording {}\n\
             fn f(x: &dyn ProcessActions) {}"
        ),
        vec!["DryRunExecutor".to_string(), "Other".to_string()],
        "the rule sees plain and path-qualified implementations and skips test code"
    );
    let mut expected: Vec<(String, String)> = PRODUCTION_EXECUTORS
        .iter()
        .map(|(name, file)| (name.to_string(), file.to_string()))
        .collect();
    expected.sort();
    assert_eq!(production_executors(&production_files()), expected);
    // The terminator implemented anywhere else is another executor.
    let moved = vec![(
        "src/actions.rs".to_string(),
        "impl ProcessActions for PidfdTerminator {}".to_string(),
    )];
    assert_ne!(production_executors(&moved), expected[1..].to_vec());
}

/// PX-030: the termination executor is the one production file that reaches
/// the handle module besides the module itself, and it reaches it in exactly
/// one way: one table, one settlement with SIGTERM, built once by `main`.
/// Each departure below is refused, and the module's system-call rules still
/// hold there.
#[test]
fn the_termination_executor_is_the_one_reach_into_the_handle_module() {
    let files = production_files();
    assert_eq!(handle_reach_violations(&files), Vec::<String>::new());
    let executor = files
        .iter()
        .find(|(file, _)| file == EXECUTOR_MODULE)
        .expect("the termination executor exists")
        .1
        .clone();
    let with = |file: &str, extra: &str| -> Vec<(String, String)> {
        let mut changed = files.clone();
        match changed.iter_mut().find(|(name, _)| name == file) {
            Some((_, text)) => text.push_str(extra),
            None => changed.push((file.to_string(), extra.to_string())),
        }
        changed
    };
    for (name, file, extra, rule) in [
        (
            "a second settlement, identical",
            EXECUTOR_MODULE,
            "\nfn again(t: &mut ProcessHandles, id: HandleId) { t.settle(id, HandleSignal::Terminate); }",
            "[count]",
        ),
        (
            "a settlement elsewhere",
            "src/actions.rs",
            "\nfn f() { table.settle(id, HandleSignal::Terminate); }",
            "[settle]",
        ),
        (
            "a settlement of another handle",
            EXECUTOR_MODULE,
            "\nfn f() { table.settle(other, HandleSignal::Terminate); }",
            "[settle]",
        ),
        (
            "a settlement by path",
            EXECUTOR_MODULE,
            "\nfn f() { ProcessHandles::settle(&mut table, id, HandleSignal::Terminate); }",
            "[settle]",
        ),
        (
            "a second table in main",
            "src/main.rs",
            "\nfn f() { let _ = ProcessHandles::live(); }",
            "[table]",
        ),
        (
            "a second table, identical",
            EXECUTOR_MODULE,
            "\nfn f() -> ProcessHandles { ProcessHandles::live() }",
            "[count]",
        ),
        (
            "a table over another system",
            EXECUTOR_MODULE,
            "\nfn f() { let _ = ProcessHandles::new(system, clock, source); }",
            "[table]",
        ),
        (
            "a table constructor taken as a value",
            EXECUTOR_MODULE,
            "\nfn f() { let make = ProcessHandles::live; }",
            "[table]",
        ),
        (
            "a second terminator in main",
            "src/main.rs",
            "\nfn f() { let _ = PidfdTerminator::live(); }",
            "[count]",
        ),
        (
            "a terminator built elsewhere",
            "src/lib.rs",
            "\nfn f() { let _ = PidfdTerminator::with_handles(table); }",
            "[executor]",
        ),
    ] {
        let found = handle_reach_violations(&with(file, extra));
        assert!(
            found.iter().any(|violation| violation.contains(rule)),
            "{name} is refused as {rule}: {found:?}"
        );
    }
    // Without the settlement, the count is wrong too: the rule is exact.
    let removed = executor.replace(EXECUTOR_SETTLE_SOURCE, "");
    assert_ne!(removed, executor, "the anchor is in the module");
    let mut without = files.clone();
    without
        .iter_mut()
        .find(|(file, _)| file == EXECUTOR_MODULE)
        .unwrap()
        .1 = removed;
    assert!(handle_reach_violations(&without)
        .iter()
        .any(|violation| violation.contains("0 settlements")));

    // In the executor module: the handle items it needs pass; the seam, and
    // every system-call, signal and process-control construct, are refused.
    assert_eq!(
        token_violations(
            EXECUTOR_MODULE,
            "use crate::handle::{AcquireError, HandleId, HandleSignal, ProcessHandles, SignalOutcome};"
        ),
        Vec::<String>::new()
    );
    assert!(token_violations(
        "src/actions.rs",
        "use crate::handle::{AcquireError, HandleId, HandleSignal, ProcessHandles, SignalOutcome};"
    )
    .iter()
    .any(|v| v.contains("[handle]")));
    for snippet in [
        "fn f(system: &dyn PidfdSystem) {}",
        "let _ = LivePidfds;",
        "impl Pidfd for Mine {}",
    ] {
        assert!(
            token_violations(EXECUTOR_MODULE, snippet)
                .iter()
                .any(|v| v.contains("[handle]")),
            "{snippet} is refused in the executor module"
        );
    }
    for snippet in [
        "let _ = libc::kill(pid, 15);",
        "let _ = kill(pid, 15);",
        "let _ = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, fd, 15, 0, 0) };",
        "let _ = libc::syscall(libc::SYS_pidfd_open, pid, 0);",
        "let _ = nix::sys::signal::kill(Pid::from_raw(pid), None);",
        "std::process::Command::new(\"kill\").arg(pid).status();",
    ] {
        assert!(
            token_violations(EXECUTOR_MODULE, snippet)
                .iter()
                .any(|v| v.contains("[ident]")),
            "{snippet} is refused in the executor module"
        );
    }

    // The panels' Confirm handlers: exactly `on(panel_confirm, ACTIVATE, ..)`,
    // in `src/lib.rs` only.
    let registration = "session.on(panel_confirm, ACTIVATE, move |session, event| { actions::confirm_requested(&gate, &view, session, event, slot); });";
    assert_eq!(
        token_violations(PANEL_CONFIRM.0, registration),
        Vec::<String>::new()
    );
    for (file, snippet) in [
        ("src/actions.rs", registration),
        (EXECUTOR_MODULE, registration),
        (
            PANEL_CONFIRM.0,
            "session.on(panel_confirm, VALUE_CHANGED, |_, _| {});",
        ),
        (
            PANEL_CONFIRM.0,
            "session.on(panel_confirm + 1, ACTIVATE, |_, _| {});",
        ),
        (PANEL_CONFIRM.0, "session.on(confirm, ACTIVATE, |_, _| {});"),
        (
            PANEL_CONFIRM.0,
            "session.on(ids.confirm, ACTIVATE, |_, _| {});",
        ),
        (
            PANEL_CONFIRM.0,
            "session.on(ACTION_CONFIRM, ACTIVATE, |_, _| {});",
        ),
    ] {
        assert!(
            token_violations(file, snippet)
                .iter()
                .any(|v| v.contains("[handler]")),
            "{snippet} in {file} is refused"
        );
    }
}

/// The one settlement as the executor module writes it.
const EXECUTOR_SETTLE_SOURCE: &str = "handles.settle(id, HandleSignal::Terminate)";

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
