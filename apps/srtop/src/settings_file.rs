//! The per-user settings file: bounded reads, atomic replacement and one
//! backup (PX-069; design §§6.3, 26–27; D2 T32).
//!
//! This is the only module of srtop that writes a file, and the PX-008 audit
//! admits its write constructs here and nowhere else
//! (`tests/no_process_control_test.rs`, `SETTINGS_MODULE`). It writes exactly
//! three names, all in the directory of the configured settings path:
//!
//! * the settings file itself (`settings`), only ever replaced by `rename`;
//! * its backup (`settings.bak`): the previous valid file;
//! * this process's temporary files (`settings.tmp.<pid>` and
//!   `settings.bak.tmp.<pid>`), each opened `create_new` (so never through a
//!   symbolic link and never another process's), mode 0600, written in full,
//!   flushed (`sync_all`) and renamed into place, or removed on failure.
//!
//! **Lifecycle.** Nothing is created until a view is created, renamed or
//! deleted, the active view changes, or defaults are restored; reading never
//! writes. A save (1) re-reads the current file and, when it is valid and
//! this build may replace it, copies it to the backup atomically; a damaged
//! file never replaces the backup; (2) writes the new file to a temporary
//! name, flushes it, renames it over the settings file and flushes the
//! directory. A crash at any point leaves the settings file either whole and
//! old or whole and new, and the backup either the previous valid file or
//! unchanged. A temporary file a crash left behind is never read, and the
//! next save removes those older than [`STALE_TEMPORARY`]. The user resets
//! everything with "Restore default views" (which keeps the previous file as
//! the backup), or by deleting the `srtop` settings directory; srtop never
//! deletes the settings file or the backup itself.
//!
//! **Reading (§26).** A read takes at most
//! [`crate::settings::MAX_SETTINGS_FILE_BYTES`] plus one byte, of a regular
//! file only (a symbolic link, directory or FIFO is refused, never followed
//! or opened for reading). A damaged settings file falls back to a valid
//! backup. A file this build must not replace — a newer version, another
//! application's, another source's, unreadable, or not a regular file —
//! makes the store read-only: the defaults are shown, and every save is
//! refused, so the file is never overwritten.
//!
//! **Not covered.** Two srtop instances of the same user saving at once are
//! not serialized: the last rename wins, whole. The directory and file
//! permissions are set at creation (0700 and 0600) and not repaired later.

use crate::settings::{self, Parsed, Problem, Settings, MAX_SETTINGS_FILE_BYTES};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The settings file's name inside the settings directory.
pub const SETTINGS_FILE_NAME: &str = "settings";
/// The settings directory's name inside the user's configuration directory.
pub const SETTINGS_DIRECTORY_NAME: &str = "srtop";
/// How old a leftover temporary file must be before a save removes it: far
/// longer than any save takes, so another instance's save in progress is never
/// disturbed.
pub const STALE_TEMPORARY: Duration = Duration::from_secs(600);
/// The most directory entries a save examines for leftover temporary files.
pub const MAX_SWEPT_ENTRIES: usize = 256;

/// The default settings path for this user, from the environment:
/// `$XDG_CONFIG_HOME/srtop/settings` when that is set to an absolute path,
/// otherwise `~/Library/Application Support/srtop/settings` on macOS and
/// `~/.config/srtop/settings` elsewhere. `None` when neither can be formed.
pub fn default_path() -> Option<PathBuf> {
    default_path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        cfg!(target_os = "macos"),
    )
}

/// [`default_path`] from the two variables' values and the platform, so the
/// rule is tested without touching this process's environment.
pub fn default_path_from(
    config_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    macos: bool,
) -> Option<PathBuf> {
    let configured = config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let base = match configured {
        Some(base) => base,
        None => {
            let home = home.map(PathBuf::from).filter(|path| path.is_absolute())?;
            if macos {
                home.join("Library").join("Application Support")
            } else {
                home.join(".config")
            }
        }
    };
    Some(base.join(SETTINGS_DIRECTORY_NAME).join(SETTINGS_FILE_NAME))
}

/// Why the store will not replace the settings file. Every save is refused
/// while one holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOnly {
    /// Written by a newer srtop, which this build cannot read.
    NewerVersion(u32),
    /// Another application's file.
    ForeignApp,
    /// Saved for another process source.
    OtherSource,
    /// Present but unreadable (`std::io::ErrorKind`, by name).
    Unreadable(String),
    /// The settings path names a directory, link, FIFO or other non-file.
    NotAFile,
}

impl std::fmt::Display for ReadOnly {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewerVersion(version) => write!(
                formatter,
                "the settings file was written by a newer srtop (version {version})"
            ),
            Self::ForeignApp => formatter.write_str("the settings file is not srtop's"),
            Self::OtherSource => {
                formatter.write_str("the settings file was saved for another process source")
            }
            Self::Unreadable(kind) => write!(formatter, "the settings file is unreadable ({kind})"),
            Self::NotAFile => formatter.write_str("the settings path is not a regular file"),
        }
    }
}

/// Where the loaded settings came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// No settings file yet: the defaults, nothing written.
    Fresh,
    /// The settings file; `migrated_from` names an older version read.
    Primary { migrated_from: Option<u32> },
    /// The settings file was refused and the backup was valid.
    Backup { primary: Problem },
    /// The settings file was refused and no backup was usable: the defaults.
    Defaults {
        primary: Problem,
        backup: Option<Problem>,
    },
    /// The store is read-only: the defaults are shown.
    ReadOnly,
}

/// What [`SettingsStore::load`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub settings: Settings,
    pub origin: Origin,
    /// Set when every save will be refused.
    pub read_only: Option<ReadOnly>,
    /// Values dropped as incompatible ([`Parsed::notes`]).
    pub notes: Vec<String>,
}

/// Why a save did not replace the settings file. A failed save leaves the
/// settings file and the backup as they were, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The store is read-only for this reason.
    Refused(ReadOnly),
    /// The settings are outside the format's bounds.
    Invalid(Problem),
    /// A file operation failed (`std::io::ErrorKind`, by name).
    Io(String),
    /// The save stopped at the step a test asked for ([`SaveStep`]).
    Interrupted(SaveStep),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "{reason}"),
            Self::Invalid(problem) => write!(formatter, "the views are {problem}"),
            Self::Io(kind) => write!(formatter, "the settings file could not be written ({kind})"),
            Self::Interrupted(step) => write!(formatter, "interrupted at {step:?}"),
        }
    }
}

fn io(error: &std::io::Error) -> SaveError {
    SaveError::Io(format!("{:?}", error.kind()))
}

/// The steps of a save at which a crash is simulated (a test seam): the
/// save returns [`SaveError::Interrupted`] there and leaves the files as a
/// crash would, including a temporary file written halfway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStep {
    /// The backup was replaced; the settings file was not touched.
    BackupReplaced,
    /// Half the new file's bytes are in its temporary file.
    HalfWritten,
    /// The new file is whole in its temporary file, not yet renamed.
    Written,
    /// The new file was renamed into place; the directory was not flushed.
    Renamed,
}

/// What the settings path holds.
enum Found {
    Missing,
    Valid { parsed: Parsed, bytes: Vec<u8> },
    Refused(Problem),
}

/// The per-user settings file at one path.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
    interrupt: Option<SaveStep>,
}

impl SettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            interrupt: None,
        }
    }

    /// This store, every save of which stops at `step` as a crash would
    /// (a test seam: nothing in srtop sets it).
    #[must_use]
    pub fn interrupted_at(mut self, step: SaveStep) -> Self {
        self.interrupt = Some(step);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The backup's path: the settings path with `.bak` appended.
    pub fn backup_path(&self) -> PathBuf {
        self.sibling(".bak")
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(suffix);
        self.path.with_file_name(name)
    }

    fn temporary(&self, target: &Path) -> PathBuf {
        let mut name = target
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        name.push(format!(".tmp.{}", std::process::id()));
        target.with_file_name(name)
    }

    /// Reads the settings for `source`. Never writes.
    pub fn load(&self, source: &str) -> Loaded {
        let defaults = |origin, read_only| Loaded {
            settings: Settings::defaults(source),
            origin,
            read_only,
            notes: Vec::new(),
        };
        let primary = match classify(&self.path) {
            Found::Missing => return defaults(Origin::Fresh, None),
            Found::Valid { parsed, .. } if parsed.settings.source != source => {
                return defaults(Origin::ReadOnly, Some(ReadOnly::OtherSource));
            }
            Found::Valid { parsed, .. } => {
                return Loaded {
                    origin: Origin::Primary {
                        migrated_from: (parsed.version < settings::CURRENT_VERSION)
                            .then_some(parsed.version),
                    },
                    settings: parsed.settings,
                    read_only: None,
                    notes: parsed.notes,
                }
            }
            Found::Refused(problem) => problem,
        };
        if let Some(reason) = read_only(&primary) {
            return defaults(Origin::ReadOnly, Some(reason));
        }
        match classify(&self.backup_path()) {
            Found::Valid { parsed, .. } if parsed.settings.source == source => Loaded {
                settings: parsed.settings,
                origin: Origin::Backup { primary },
                read_only: None,
                notes: parsed.notes,
            },
            Found::Valid { .. } => defaults(
                Origin::Defaults {
                    primary,
                    backup: None,
                },
                None,
            ),
            Found::Missing => defaults(
                Origin::Defaults {
                    primary,
                    backup: None,
                },
                None,
            ),
            Found::Refused(backup) => defaults(
                Origin::Defaults {
                    primary,
                    backup: Some(backup),
                },
                None,
            ),
        }
    }

    /// Replaces the settings file with `settings`, keeping the file it
    /// replaces as the backup when that file is valid (see the module
    /// documentation for the steps).
    pub fn save(&self, settings: &Settings) -> Result<(), SaveError> {
        let text = settings.encode().map_err(SaveError::Invalid)?;
        let directory = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| io(&error))?;
        match classify(&self.path) {
            Found::Missing => {}
            Found::Valid { parsed, .. } if parsed.settings.source != settings.source => {
                return Err(SaveError::Refused(ReadOnly::OtherSource));
            }
            Found::Valid { bytes, .. } => {
                self.replace(&self.backup_path(), &bytes, None)?;
                self.stop_at(SaveStep::BackupReplaced)?;
            }
            Found::Refused(problem) => {
                if let Some(reason) = read_only(&problem) {
                    return Err(SaveError::Refused(reason));
                }
                // A damaged file is replaced, and never becomes the backup.
            }
        }
        self.replace(&self.path, text.as_bytes(), self.interrupt)?;
        self.stop_at(SaveStep::Renamed)?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| io(&error))?;
        self.sweep(directory);
        Ok(())
    }

    fn stop_at(&self, step: SaveStep) -> Result<(), SaveError> {
        if self.interrupt == Some(step) {
            return Err(SaveError::Interrupted(step));
        }
        Ok(())
    }

    /// Writes `bytes` to `target` through a fresh temporary file renamed over
    /// it. `interrupt` stops a primary write at [`SaveStep::HalfWritten`] or
    /// [`SaveStep::Written`].
    fn replace(
        &self,
        target: &Path,
        bytes: &[u8],
        interrupt: Option<SaveStep>,
    ) -> Result<(), SaveError> {
        let temporary = self.temporary(target);
        // A temporary of this process ID is a leftover of an earlier
        // process with the same ID, which can no longer be writing it.
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(&error)),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| io(&error))?;
        if interrupt == Some(SaveStep::HalfWritten) {
            let _ = file.write_all(&bytes[..bytes.len() / 2]);
            return Err(SaveError::Interrupted(SaveStep::HalfWritten));
        }
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(io(&error));
        }
        if interrupt == Some(SaveStep::Written) {
            return Err(SaveError::Interrupted(SaveStep::Written));
        }
        if let Err(error) = fs::rename(&temporary, target) {
            let _ = fs::remove_file(&temporary);
            return Err(io(&error));
        }
        Ok(())
    }

    /// Removes this settings file's temporary files older than
    /// [`STALE_TEMPORARY`], examining at most [`MAX_SWEPT_ENTRIES`] entries.
    /// Best effort: a failure here never fails the save that already
    /// succeeded.
    fn sweep(&self, directory: &Path) {
        let Some(name) = self.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let prefixes = [format!("{name}.tmp."), format!("{name}.bak.tmp.")];
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        let now = SystemTime::now();
        for entry in entries.take(MAX_SWEPT_ENTRIES).flatten() {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let leftover = prefixes.iter().any(|prefix| {
                file_name
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
            });
            if !leftover {
                continue;
            }
            let path = entry.path();
            let stale = fs::symlink_metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= STALE_TEMPORARY);
            if stale {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// The reason a refused file makes the store read-only, if it does.
fn read_only(problem: &Problem) -> Option<ReadOnly> {
    match problem {
        Problem::NewerVersion(version) => Some(ReadOnly::NewerVersion(*version)),
        Problem::ForeignApp => Some(ReadOnly::ForeignApp),
        Problem::Unreadable(kind) => Some(ReadOnly::Unreadable(kind.clone())),
        Problem::NotAFile => Some(ReadOnly::NotAFile),
        _ => None,
    }
}

/// Reads and parses the file at `path`, at most one byte past the bound, and
/// only when it is a regular file.
fn classify(path: &Path) -> Found {
    match read_bounded(path) {
        Ok(None) => Found::Missing,
        Ok(Some(bytes)) => match settings::parse(&bytes) {
            Ok(parsed) => Found::Valid { parsed, bytes },
            Err(problem) => Found::Refused(problem),
        },
        Err(problem) => Found::Refused(problem),
    }
}

fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, Problem> {
    let unreadable = |error: std::io::Error| Problem::Unreadable(format!("{:?}", error.kind()));
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    if !metadata.is_file() {
        return Err(Problem::NotAFile);
    }
    if metadata.len() > MAX_SETTINGS_FILE_BYTES as u64 {
        return Err(Problem::Oversized);
    }
    let file = File::open(path).map_err(unreadable)?;
    if !file.metadata().map_err(unreadable)?.is_file() {
        return Err(Problem::NotAFile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_SETTINGS_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() > MAX_SETTINGS_FILE_BYTES {
        return Err(Problem::Oversized);
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_path_follows_xdg_then_the_platform_and_needs_an_absolute_base() {
        let path = |xdg: Option<&str>, home: Option<&str>, macos| {
            default_path_from(xdg.map(Into::into), home.map(Into::into), macos)
        };
        assert_eq!(
            path(Some("/x/config"), Some("/home/a"), false),
            Some(PathBuf::from("/x/config/srtop/settings"))
        );
        assert_eq!(
            path(Some("relative"), Some("/home/a"), false),
            Some(PathBuf::from("/home/a/.config/srtop/settings"))
        );
        assert_eq!(
            path(None, Some("/Users/a"), true),
            Some(PathBuf::from(
                "/Users/a/Library/Application Support/srtop/settings"
            ))
        );
        assert_eq!(path(None, Some("relative"), false), None);
        assert_eq!(path(None, None, true), None);
    }
}
