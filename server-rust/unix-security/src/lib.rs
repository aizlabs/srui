//! Shared Unix-account and private-socket security boundary (§27).
//!
//! Implements the reference deployment's per-account rules: refuse effective UID 0, require the
//! runtime directory and socket to be owned by the effective UID with no group/other access, and
//! require the kernel-reported peer UID to match. Mapping the SSH-authenticated account onto that
//! effective UID is an OpenSSH/service-manager responsibility (§25); this crate only enforces the
//! local boundary once that mapping has happened.

use std::ffi::{CString, OsString};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
pub const PRIVATE_SOCKET_MODE: u32 = 0o600;

#[must_use]
pub fn effective_uid() -> u32 {
    // SAFETY: geteuid(2) has no arguments, cannot fail, and has no side effects.
    unsafe { libc::geteuid() }
}

pub fn require_unprivileged_uid(uid: u32, program: &str) -> io::Result<()> {
    if uid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{program} refuses to run as root; launch it as the SSH-authenticated user"),
        ));
    }
    Ok(())
}

#[must_use]
pub fn default_runtime_directory(uid: u32) -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("srui-{uid}")))
}

#[must_use]
pub fn default_socket_path(uid: u32) -> PathBuf {
    default_runtime_directory(uid).join("srui-sessiond.sock")
}

#[must_use]
pub fn default_named_socket_path(uid: u32, file_name: &str) -> PathBuf {
    default_runtime_directory(uid).join(file_name)
}

fn socket_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn path_cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} contains an interior NUL byte", path.display()),
        )
    })
}

fn component_cstring(component: &std::ffi::OsStr) -> io::Result<CString> {
    CString::new(component.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path contains an interior NUL byte",
        )
    })
}

fn validate_owner_and_mode(
    label: &str,
    expected_uid: u32,
    actual_uid: u32,
    mode: impl Into<u64>,
) -> io::Result<()> {
    if actual_uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{label} is owned by uid {actual_uid}, expected uid {expected_uid}"),
        ));
    }
    let mode = mode.into();
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{label} permissions {:04o} allow group/other access; use a per-user 0700 directory",
                mode & 0o7777
            ),
        ));
    }
    Ok(())
}

fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let encoded = path_cstring(path)?;
    // SAFETY: encoded is NUL-terminated; open returns either a new descriptor or -1.
    let raw_fd = unsafe {
        libc::open(
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw_fd is uniquely owned after a successful open(2).
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

fn open_directory_at(directory: RawFd, component: &CString) -> io::Result<OwnedFd> {
    // SAFETY: directory is live and component is a NUL-terminated single path component.
    let raw_fd = unsafe {
        libc::openat(
            directory,
            component.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )
    };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw_fd is uniquely owned after a successful openat(2).
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

/// `Path::parent()` returns `Some("")` for a single-component relative path such as `runtime`,
/// not `None`, so an empty parent has to be normalized to the working directory explicitly or the
/// missing-ancestor walk stalls on a cursor it can never take a file name from.
fn parent_or_current_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn resolve_intermediate_symlinks(path: &Path) -> io::Result<PathBuf> {
    if path == Path::new("/") {
        return Ok(path.to_path_buf());
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} contains '..'; pass a normalized private socket path",
                path.display()
            ),
        ));
    }

    // `.` (a bare relative socket name) and any other parent-less form already name an existing
    // directory. Canonicalizing it directly keeps the caller's diagnostic on the owner/mode check
    // that actually refuses it, instead of an opaque "does not name a directory".
    let Some(final_name) = path.file_name() else {
        return std::fs::canonicalize(path);
    };
    let mut cursor = parent_or_current_dir(path);
    let mut missing: Vec<OsString> = Vec::new();
    let canonical_prefix = loop {
        match std::fs::symlink_metadata(&cursor) {
            Ok(_) => break std::fs::canonicalize(&cursor)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = cursor.file_name().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no existing ancestor for {}", path.display()),
                    )
                })?;
                missing.push(name.to_os_string());
                cursor = parent_or_current_dir(&cursor);
            }
            Err(error) => return Err(error),
        }
    };

    let mut resolved = canonical_prefix;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    resolved.push(final_name);
    Ok(resolved)
}

fn open_directory_tree(path: &Path, create_missing: bool) -> io::Result<OwnedFd> {
    // Resolve symlinks only in existing ancestors. The final private directory component is still
    // opened with O_NOFOLLOW, preserving the security boundary while supporting macOS TMPDIR and
    // operator paths whose trusted prefix contains a symlink.
    let resolved_path = resolve_intermediate_symlinks(path)?;
    let mut current = open_directory(if resolved_path.is_absolute() {
        Path::new("/")
    } else {
        Path::new(".")
    })?;

    for component in resolved_path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => component_cstring(name)?,
            Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{} contains '..'; pass a normalized private socket path",
                        path.display()
                    ),
                ));
            }
            Component::Prefix(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "unsupported Unix socket path prefix",
                ));
            }
        };

        let next = match open_directory_at(current.as_raw_fd(), &name) {
            Ok(fd) => fd,
            Err(error) if create_missing && error.kind() == io::ErrorKind::NotFound => {
                // SAFETY: current and name identify the exact parent inode and one component.
                let mkdir_result = unsafe {
                    libc::mkdirat(
                        current.as_raw_fd(),
                        name.as_ptr(),
                        PRIVATE_DIRECTORY_MODE as libc::mode_t,
                    )
                };
                if mkdir_result != 0 {
                    let mkdir_error = io::Error::last_os_error();
                    if mkdir_error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(mkdir_error);
                    }
                }
                open_directory_at(current.as_raw_fd(), &name)?
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "cannot open private runtime directory {} without following symlinks \
                         ({error}); point --socket at a per-user directory such as \
                         $XDG_RUNTIME_DIR or $TMPDIR/srui-<uid>/, whose final component is a real \
                         0700 directory rather than a symlink",
                        path.display()
                    ),
                ));
            }
        };
        current = next;
    }

    Ok(current)
}

fn validate_directory_fd(fd: &OwnedFd, path: &Path, uid: u32) -> io::Result<()> {
    let mut stat = MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fd is live and stat points to sufficient writable storage for fstat(2).
    if unsafe { libc::fstat(fd.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat(2) initialized stat.
    let stat = unsafe { stat.assume_init() };
    validate_owner_and_mode(
        &format!("runtime directory {}", path.display()),
        uid,
        stat.st_uid,
        stat.st_mode,
    )
}

/// Identity of a socket inode reached relative to a verified private directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketIdentity {
    device: libc::dev_t,
    inode: libc::ino_t,
}

/// Retained authority for exactly one socket name inside an opened private directory.
///
/// Security-sensitive mode, identity, and removal operations use this descriptor rather than
/// resolving the parent path again.
#[derive(Debug)]
pub struct PrivateSocketParent {
    directory: OwnedFd,
    parent_path: PathBuf,
    socket_path: PathBuf,
    socket_name: CString,
    uid: u32,
}

impl PrivateSocketParent {
    fn open(path: &Path, uid: u32, create_missing: bool) -> io::Result<Self> {
        let parent_path = socket_parent(path).to_path_buf();
        let directory = open_directory_tree(&parent_path, create_missing)?;
        validate_directory_fd(&directory, &parent_path, uid)?;
        let file_name = path.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} does not name a Unix socket file", path.display()),
            )
        })?;
        Ok(Self {
            directory,
            parent_path,
            socket_path: path.to_path_buf(),
            socket_name: component_cstring(file_name)?,
            uid,
        })
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Stats the socket name relative to the retained directory descriptor, never by path and
    /// never through a symlink.
    fn stat_socket_entry(&self) -> io::Result<Option<libc::stat>> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: the descriptor and name are live; AT_SYMLINK_NOFOLLOW inspects the exact entry.
        let result = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                self.socket_name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error);
        }
        // SAFETY: successful fstatat(2) initialized stat.
        Ok(Some(unsafe { stat.assume_init() }))
    }

    /// Requires the entry to exist, be a socket, and be owned by this account. Deliberately does
    /// *not* check the mode: it runs before [`Self::secure_bound_socket`] narrows a freshly bound
    /// endpoint, where the umask-derived mode is still whatever `bind(2)` produced.
    fn require_own_socket(&self) -> io::Result<libc::stat> {
        let stat = self.stat_socket_entry()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not present", self.socket_path.display()),
            )
        })?;
        if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} exists and is not a Unix socket",
                    self.socket_path.display()
                ),
            ));
        }
        if stat.st_uid != self.uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "Unix socket {} is owned by uid {}, expected uid {}",
                    self.socket_path.display(),
                    stat.st_uid,
                    self.uid
                ),
            ));
        }
        Ok(stat)
    }

    pub fn socket_identity(&self) -> io::Result<Option<SocketIdentity>> {
        let Some(stat) = self.stat_socket_entry()? else {
            return Ok(None);
        };
        if stat.st_mode & libc::S_IFMT != libc::S_IFSOCK {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "{} exists and is not a Unix socket",
                    self.socket_path.display()
                ),
            ));
        }
        Ok(Some(SocketIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        }))
    }

    pub fn remove_socket(&self) -> io::Result<()> {
        // SAFETY: the descriptor and NUL-terminated single-component name remain live.
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), self.socket_name.as_ptr(), 0) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Re-validates the parent inode through the retained descriptor and then the socket entry's
    /// type, owner, and mode. Every check is descriptor-relative, so none of them can be answered
    /// by a path that was re-pointed after [`prepare_private_socket_parent`] returned.
    pub fn validate_socket(&self) -> io::Result<SocketIdentity> {
        validate_directory_fd(&self.directory, &self.parent_path, self.uid)?;
        let stat = self.require_own_socket()?;
        validate_owner_and_mode(
            &format!("Unix socket {}", self.socket_path.display()),
            self.uid,
            stat.st_uid,
            stat.st_mode,
        )?;
        Ok(SocketIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }

    /// Narrows a freshly bound endpoint to 0600 and then validates it.
    ///
    /// `fchmodat` follows symlinks and cannot be told not to: `AT_SYMLINK_NOFOLLOW` is not
    /// portable for it (Linux returns `ENOTSUP`), so a symlink planted under this name would
    /// otherwise have its *target* relaxed to 0600. [`Self::require_own_socket`] therefore proves
    /// the entry is this account's own socket before the mode is touched, and the full validation
    /// afterwards fails closed if anything replaced it in between. The parent is 0700 and owned by
    /// `uid`, so no other account can create that entry in the first place.
    pub fn secure_bound_socket(&self) -> io::Result<SocketIdentity> {
        self.require_own_socket()?;
        // SAFETY: descriptor and name identify the entry inside the retained private directory.
        if unsafe {
            libc::fchmodat(
                self.directory.as_raw_fd(),
                self.socket_name.as_ptr(),
                PRIVATE_SOCKET_MODE as libc::mode_t,
                0,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        self.validate_socket()
    }

    /// Binds inside the private parent, then immediately restricts and descriptor-validates the
    /// endpoint. The parent itself is 0700 and owned by `uid`, so no other account can reach the
    /// socket during the window between `bind(2)` and the 0600 narrowing; a failure in either step
    /// unlinks the half-published endpoint rather than leaving it listening (§4 inv. 13).
    pub fn bind(&self) -> io::Result<std::os::unix::net::UnixListener> {
        let listener = std::os::unix::net::UnixListener::bind(&self.socket_path)?;
        if let Err(error) = self.secure_bound_socket() {
            drop(listener);
            let _ = self.remove_socket();
            return Err(error);
        }
        if let Err(error) = listener.set_nonblocking(true) {
            drop(listener);
            let _ = self.remove_socket();
            return Err(error);
        }
        Ok(listener)
    }
}

/// Opens or recursively creates the socket parent without following symlinks and returns a
/// capability that callers retain through bind, validation, and cleanup.
pub fn prepare_private_socket_parent(path: &Path, uid: u32) -> io::Result<PrivateSocketParent> {
    PrivateSocketParent::open(path, uid, true)
}

pub fn validate_private_directory(path: &Path, uid: u32) -> io::Result<()> {
    let directory = open_directory_tree(path, false)?;
    validate_directory_fd(&directory, path, uid)
}

pub fn validate_private_socket(path: &Path, uid: u32) -> io::Result<()> {
    PrivateSocketParent::open(path, uid, false)?
        .validate_socket()
        .map(drop)
}
pub fn peer_effective_uid<T: AsRawFd + ?Sized>(stream: &T) -> io::Result<u32> {
    let fd = stream.as_raw_fd();

    #[cfg(target_os = "linux")]
    {
        let mut credentials = MaybeUninit::<libc::ucred>::zeroed();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: fd is a live Unix-stream descriptor; the output points to a correctly sized
        // ucred buffer and length for the duration of getsockopt(2).
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful getsockopt initialized the ucred output buffer.
        return Ok(unsafe { credentials.assume_init() }.uid);
    }

    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        let mut peer_uid: libc::uid_t = 0;
        let mut peer_gid: libc::gid_t = 0;
        // SAFETY: fd is live and both output pointers remain valid for getpeereid(3).
        if unsafe { libc::getpeereid(fd, &mut peer_uid, &mut peer_gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok(peer_uid);
    }

    #[allow(unreachable_code)]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "this platform does not expose Unix peer credentials",
    ))
}

pub fn require_same_uid(expected_uid: u32, peer_uid: u32) -> io::Result<()> {
    if peer_uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Unix peer uid {peer_uid} does not match session uid {expected_uid}"),
        ));
    }
    Ok(())
}

pub fn validate_peer<T: AsRawFd + ?Sized>(stream: &T, expected_uid: u32) -> io::Result<()> {
    require_same_uid(expected_uid, peer_effective_uid(stream)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    /// Serializes the tests that read or replace the process-wide working directory. Cargo runs
    /// tests in threads of one process, so a concurrent `set_current_dir` would otherwise change
    /// what a relative socket path resolves to underneath another test.
    static WORKING_DIRECTORY: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_working_directory() -> std::sync::MutexGuard<'static, ()> {
        WORKING_DIRECTORY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn unique_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "srui-unix-security-{}-{label}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn root_and_cross_account_peers_are_rejected() {
        assert_eq!(
            require_unprivileged_uid(0, "test")
                .expect_err("root must fail")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(require_unprivileged_uid(501, "test").is_ok());
        assert!(require_same_uid(501, 502).is_err());
        assert!(require_same_uid(501, 501).is_ok());
    }

    #[test]
    fn private_parent_is_created_atomically_and_final_symlinks_are_rejected() {
        let uid = effective_uid();
        let runtime = unique_path("runtime");
        let socket = runtime.join("session.sock");
        let parent = prepare_private_socket_parent(&socket, uid).expect("create private runtime");
        let mode = std::fs::metadata(&runtime)
            .expect("runtime metadata")
            .mode()
            & 0o777;
        assert_eq!(mode, PRIVATE_DIRECTORY_MODE);

        let target = unique_path("target");
        std::fs::create_dir(&target).expect("create symlink target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700))
            .expect("secure symlink target");
        let link = unique_path("link");
        symlink(&target, &link).expect("create runtime symlink");
        assert!(prepare_private_socket_parent(&link.join("session.sock"), uid).is_err());

        let nested_runtime = unique_path("nested");
        let nested_socket = nested_runtime.join("one/two/session.sock");
        let nested_parent = prepare_private_socket_parent(&nested_socket, uid)
            .expect("recursively create missing private ancestors");
        assert_eq!(
            std::fs::metadata(nested_socket.parent().unwrap())
                .expect("nested parent metadata")
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        drop(nested_parent);
        std::fs::remove_dir(nested_runtime.join("one/two")).expect("remove nested leaf");
        std::fs::remove_dir(nested_runtime.join("one")).expect("remove nested parent");
        std::fs::remove_dir(&nested_runtime).expect("remove nested runtime");

        drop(parent);
        std::fs::remove_file(&link).expect("remove symlink");
        std::fs::remove_dir(&target).expect("remove target");
        std::fs::remove_dir(&runtime).expect("remove runtime");
    }

    #[test]
    fn bound_socket_and_kernel_peer_credentials_are_validated() {
        let uid = effective_uid();
        let runtime = unique_path("socket");
        let socket = runtime.join("session.sock");
        let parent = prepare_private_socket_parent(&socket, uid).expect("create private runtime");
        let _listener = parent.bind().expect("bind and secure socket");
        parent.validate_socket().expect("validate bound socket");

        let mode = std::fs::symlink_metadata(&socket)
            .expect("socket metadata")
            .mode()
            & 0o777;
        assert_eq!(mode, PRIVATE_SOCKET_MODE);

        let (peer, _other) = UnixStream::pair().expect("Unix socket pair");
        assert_eq!(peer_effective_uid(&peer).expect("peer uid"), uid);
        validate_peer(&peer, uid).expect("same account peer accepted");
        // The descriptor-based path must refuse a foreign account, not just `require_same_uid`:
        // this is the exact call every accept loop makes (§25, §27).
        assert_eq!(
            validate_peer(&peer, uid.wrapping_add(1))
                .expect_err("cross-account peer must be refused")
                .kind(),
            io::ErrorKind::PermissionDenied
        );

        std::fs::remove_file(&socket).expect("remove socket");
        std::fs::remove_dir(&runtime).expect("remove runtime");
    }

    /// A shared or symlinked parent must fail with a diagnostic that names the remedy, and a
    /// parent-less relative path must reach the owner/mode check rather than an opaque error.
    #[test]
    fn refused_socket_parents_explain_the_required_layout() {
        let uid = effective_uid();

        let shared = unique_path("shared");
        std::fs::create_dir(&shared).expect("create shared directory");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755))
            .expect("widen shared directory");
        let shared_error = prepare_private_socket_parent(&shared.join("session.sock"), uid)
            .expect_err("group/other-accessible parent must be refused");
        assert_eq!(shared_error.kind(), io::ErrorKind::PermissionDenied);
        assert!(
            shared_error.to_string().contains("per-user 0700 directory"),
            "unhelpful diagnostic: {shared_error}"
        );
        std::fs::remove_dir(&shared).expect("remove shared directory");

        let target = unique_path("symlink-target");
        std::fs::create_dir(&target).expect("create symlink target");
        std::fs::set_permissions(
            &target,
            std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
        )
        .expect("restrict symlink target");
        let link = unique_path("symlink-parent");
        symlink(&target, &link).expect("create parent symlink");
        let symlink_error = prepare_private_socket_parent(&link.join("session.sock"), uid)
            .expect_err("symlinked final parent must be refused");
        assert!(
            symlink_error
                .to_string()
                .contains("without following symlinks")
                && symlink_error.to_string().contains("$XDG_RUNTIME_DIR"),
            "unhelpful diagnostic: {symlink_error}"
        );
        std::fs::remove_file(&link).expect("remove parent symlink");
        std::fs::remove_dir(&target).expect("remove symlink target");

        // `bare.sock` has no parent component; it must be judged on the working directory's own
        // ownership and mode, which is a diagnosis the operator can act on.
        let _working_directory = lock_working_directory();
        let relative = prepare_private_socket_parent(Path::new("bare.sock"), uid);
        match relative {
            Ok(parent) => assert_eq!(
                std::fs::metadata(parent.socket_path().parent().unwrap_or(Path::new(".")))
                    .expect("cwd metadata")
                    .mode()
                    & 0o077,
                0,
                "a relative parent is only accepted when it is genuinely private"
            ),
            Err(error) => assert!(
                error.to_string().contains("per-user 0700 directory"),
                "unhelpful diagnostic for a relative socket path: {error}"
            ),
        }
    }

    /// A relative socket path whose own parent components do not exist yet must be created under
    /// the working directory. `Path::parent()` yields `Some("")` rather than `None` there, so the
    /// missing-ancestor walk needs an explicit normalization to `.` (§27).
    #[test]
    fn relative_socket_parents_are_created_under_the_working_directory() {
        let uid = effective_uid();
        let sandbox = unique_path("relative-cwd");
        std::fs::create_dir(&sandbox).expect("create sandbox");
        std::fs::set_permissions(
            &sandbox,
            std::fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
        )
        .expect("restrict sandbox");
        let _working_directory = lock_working_directory();
        let restore = std::env::current_dir().expect("current dir");
        std::env::set_current_dir(&sandbox).expect("enter sandbox");

        let single = prepare_private_socket_parent(Path::new("runtime/session.sock"), uid)
            .expect("single missing relative component is created");
        assert_eq!(
            std::fs::metadata("runtime")
                .expect("runtime metadata")
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        drop(single);

        let nested = prepare_private_socket_parent(Path::new("deep/a/b/session.sock"), uid)
            .expect("several missing relative components are created");
        assert_eq!(
            std::fs::metadata("deep/a/b")
                .expect("nested metadata")
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        drop(nested);

        std::env::set_current_dir(&restore).expect("restore working directory");
        std::fs::remove_dir_all(&sandbox).expect("remove sandbox");
    }
}
