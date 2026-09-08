//! Shared Unix-account and private-socket security boundary (§25, §27).

use std::ffi::CString;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
pub fn default_socket_path(uid: u32) -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("srui-{uid}")))
        .join("srui-sessiond.sock")
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
                "{label} permissions {:04o} allow group/other access",
                mode & 0o7777
            ),
        ));
    }
    Ok(())
}

/// Opens the exact directory without following a final symlink and validates the opened inode.
fn open_private_directory(path: &Path, uid: u32) -> io::Result<OwnedFd> {
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
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

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
    )?;
    Ok(fd)
}

/// Atomically creates the final runtime-directory component as 0700, or validates the existing
/// directory through an O_NOFOLLOW descriptor. Missing ancestors are an error.
pub fn prepare_private_socket_parent(path: &Path, uid: u32) -> io::Result<()> {
    let parent = socket_parent(path);
    let encoded = path_cstring(parent)?;
    // SAFETY: encoded is NUL-terminated. mkdir(2) atomically creates only the final component,
    // and mode 0700 cannot be broadened by umask.
    let result = unsafe { libc::mkdir(encoded.as_ptr(), 0o700) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    open_private_directory(parent, uid).map(drop)
}

pub fn validate_private_directory(path: &Path, uid: u32) -> io::Result<()> {
    open_private_directory(path, uid).map(drop)
}

pub fn validate_private_socket(path: &Path, uid: u32) -> io::Result<()> {
    validate_private_directory(socket_parent(path), uid)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a Unix socket", path.display()),
        ));
    }
    validate_owner_and_mode(
        &format!("Unix socket {}", path.display()),
        uid,
        metadata.uid(),
        metadata.mode(),
    )
}

/// Restricts a newly bound socket and then validates its owner, type, parent, and effective mode.
pub fn secure_bound_socket(path: &Path, uid: u32) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_SOCKET_MODE))?;
    validate_private_socket(path, uid)
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
    use std::os::unix::fs::symlink;
    use std::os::unix::net::{UnixListener, UnixStream};
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
        prepare_private_socket_parent(&socket, uid).expect("create private runtime");
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

        std::fs::remove_file(&link).expect("remove symlink");
        std::fs::remove_dir(&target).expect("remove target");
        std::fs::remove_dir(&runtime).expect("remove runtime");
    }

    #[test]
    fn bound_socket_and_kernel_peer_credentials_are_validated() {
        let uid = effective_uid();
        let runtime = unique_path("socket");
        let socket = runtime.join("session.sock");
        prepare_private_socket_parent(&socket, uid).expect("create private runtime");
        let _listener = UnixListener::bind(&socket).expect("bind socket");
        secure_bound_socket(&socket, uid).expect("secure bound socket");

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
