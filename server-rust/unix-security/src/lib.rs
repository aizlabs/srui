//! Shared Unix-account and private-socket security boundary (§25, §27).

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

    let final_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} does not name a directory", path.display()),
        )
    })?;
    let mut cursor = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
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
                cursor = cursor
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf();
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
                        "cannot open private runtime directory {} without following symlinks: {error}",
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

    pub fn socket_identity(&self) -> io::Result<Option<SocketIdentity>> {
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
        let stat = unsafe { stat.assume_init() };
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

    pub fn validate_socket(&self) -> io::Result<SocketIdentity> {
        validate_directory_fd(&self.directory, &self.parent_path, self.uid)?;
        let identity = self.socket_identity()?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not present", self.socket_path.display()),
            )
        })?;

        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: socket_identity already proved this descriptor-relative entry exists.
        if unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                self.socket_name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fstatat(2) initialized stat.
        let stat = unsafe { stat.assume_init() };
        validate_owner_and_mode(
            &format!("Unix socket {}", self.socket_path.display()),
            self.uid,
            stat.st_uid,
            stat.st_mode,
        )?;
        Ok(identity)
    }

    pub fn secure_bound_socket(&self) -> io::Result<SocketIdentity> {
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
    /// endpoint. The parent itself is 0700, so no other account can reach the socket during the
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

        std::fs::remove_file(&socket).expect("remove socket");
        std::fs::remove_dir(&runtime).expect("remove runtime");
    }
}
