//! Unix-account security boundary for the per-user session daemon (§25, §27).

use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::UnixStream;

pub(crate) fn effective_uid() -> u32 {
    // SAFETY: geteuid(2) has no arguments, cannot fail, and has no side effects.
    unsafe { libc::geteuid() }
}

pub(crate) fn require_unprivileged_uid(uid: u32) -> io::Result<()> {
    if uid == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "srui-sessiond refuses to run as root; launch it as the SSH-authenticated user",
        ));
    }
    Ok(())
}

pub(crate) fn default_socket_path(uid: u32) -> PathBuf {
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

fn validate_owner_and_mode(
    label: &str,
    expected_uid: u32,
    actual_uid: u32,
    mode: u32,
) -> io::Result<()> {
    if actual_uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{label} is owned by uid {actual_uid}, expected uid {expected_uid}"),
        ));
    }
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

pub(crate) fn prepare_private_socket_parent(path: &Path, uid: u32) -> io::Result<()> {
    let parent = socket_parent(path);
    match std::fs::symlink_metadata(parent) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        Err(error) => return Err(error),
    }
    validate_private_directory(parent, uid)
}

pub(crate) fn validate_private_directory(path: &Path, uid: u32) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not a real directory", path.display()),
        ));
    }
    validate_owner_and_mode(
        &format!("runtime directory {}", path.display()),
        uid,
        metadata.uid(),
        metadata.mode(),
    )
}

pub(crate) fn validate_private_socket(path: &Path, uid: u32) -> io::Result<()> {
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

pub(crate) fn peer_effective_uid(stream: &UnixStream) -> io::Result<u32> {
    let fd = stream.as_raw_fd();

    #[cfg(target_os = "linux")]
    {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::zeroed();
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
        // SAFETY: a successful getsockopt initialized the ucred output buffer.
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
        // SAFETY: fd is a live Unix-stream descriptor and both output pointers remain valid.
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

pub(crate) fn require_same_uid(expected_uid: u32, peer_uid: u32) -> io::Result<()> {
    if peer_uid != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Unix peer uid {peer_uid} does not match session uid {expected_uid}"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_peer(stream: &UnixStream, expected_uid: u32) -> io::Result<()> {
    require_same_uid(expected_uid, peer_effective_uid(stream)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_cross_account_peers_are_rejected() {
        assert_eq!(
            require_unprivileged_uid(0)
                .expect_err("root must fail")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(require_unprivileged_uid(501).is_ok());
        assert_eq!(
            require_same_uid(501, 502)
                .expect_err("different account must fail")
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(require_same_uid(501, 501).is_ok());
    }

    #[test]
    fn group_or_other_access_is_rejected() {
        assert!(validate_owner_and_mode("runtime", 501, 501, 0o700).is_ok());
        assert!(validate_owner_and_mode("socket", 501, 501, 0o600).is_ok());
        assert!(validate_owner_and_mode("runtime", 501, 501, 0o750).is_err());
        assert!(validate_owner_and_mode("socket", 501, 501, 0o606).is_err());
        assert!(validate_owner_and_mode("socket", 501, 502, 0o600).is_err());
    }

    #[tokio::test]
    async fn kernel_reports_same_uid_for_a_local_socket_pair() {
        let (peer, _other) = UnixStream::pair().expect("Unix socket pair");
        assert_eq!(
            peer_effective_uid(&peer).expect("peer credentials"),
            effective_uid()
        );
        validate_peer(&peer, effective_uid()).expect("same account peer accepted");
    }
}
