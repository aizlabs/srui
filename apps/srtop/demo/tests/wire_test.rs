//! PX-025: the replica client's frame reader enforces the §26 frame-size
//! limit before it buffers a frame, and no declared length can overflow it.

use srtop_demo::wire::{frame_extent, Replica};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

/// The varint encoding of `value`.
fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            bytes.push(low);
            return bytes;
        }
        bytes.push(low | 0x80);
    }
}

#[test]
fn a_frame_is_accepted_at_the_limit_and_refused_one_byte_past_it() {
    let max = DEFAULT_MAX_FRAME_SIZE;
    // At the bound: a declared 16 MiB payload is waited for, not refused.
    let prefix = varint(max as u64);
    assert_eq!(frame_extent(&prefix, max), Ok(None));
    let mut whole = prefix.clone();
    whole.resize(prefix.len() + max, 0);
    assert_eq!(frame_extent(&whole, max), Ok(Some(prefix.len() + max)));
    // One past it: refused from the prefix alone, before any payload.
    let past = varint(max as u64 + 1);
    let refused = frame_extent(&past, max).unwrap_err();
    assert!(refused.contains("exceeds"), "{refused}");
    // Small frames as before; an incomplete prefix waits.
    assert_eq!(frame_extent(&[3, 1, 2, 3], max), Ok(Some(4)));
    assert_eq!(frame_extent(&[3, 1, 2], max), Ok(None));
    assert_eq!(frame_extent(&[0x80], max), Ok(None));
    assert_eq!(frame_extent(&[], max), Ok(None));
}

#[test]
fn an_overflowing_or_overlong_length_prefix_is_refused() {
    let max = DEFAULT_MAX_FRAME_SIZE;
    // u64::MAX: refused by the limit, and its sum with the prefix never formed.
    let huge = varint(u64::MAX);
    assert_eq!(huge.len(), 10);
    assert!(frame_extent(&huge, max).unwrap_err().contains("exceeds"));
    // A tenth byte carrying more than the 64th bit does not fit 64 bits.
    let mut too_wide = vec![0xff_u8; 9];
    too_wide.push(0x02);
    assert!(frame_extent(&too_wide, max)
        .unwrap_err()
        .contains("64 bits"));
    // Eleven continuation bytes: no varint is that long.
    assert!(frame_extent(&[0x80_u8; 11], max)
        .unwrap_err()
        .contains("longer than ten"));
    // Even with no limit, a length that overflows the sum is refused.
    let near = varint(usize::MAX as u64);
    assert!(frame_extent(&near, usize::MAX).is_err());
}

/// A server that declares a frame past the limit gets an error at once from
/// the replica, which reads nothing more.
#[test]
fn the_replica_refuses_a_server_that_declares_an_oversized_frame() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = PathBuf::from(format!(
        "/tmp/srtop-px025-wire-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut hello = [0_u8; 256];
        let _ = stream.read(&mut hello);
        stream
            .write_all(&varint(DEFAULT_MAX_FRAME_SIZE as u64 + 1))
            .unwrap();
        // Holds the connection open: the refusal must not wait for payload.
        let mut rest = [0_u8; 1];
        let _ = stream.read(&mut rest);
    });
    let refused = Replica::hello(&socket, b"px025-oversized")
        .err()
        .expect("refused");
    assert!(refused.contains("exceeds"), "{refused}");
    server.join().unwrap();
    let _ = fs::remove_dir_all(&directory);
}
