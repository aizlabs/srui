use crate::report::{metric, p50, percentile, push_timing_distributions, Assertion, Section};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use srui_protocol::{TerminalData, TerminalResyncReason};
use srui_pty::{
    PTYManager, SubscribeOutcome, SubscribeSnapshot, TerminalEvent, TerminalSpec,
    MAX_TERMINAL_OUTPUT_FRAME_BYTES,
};
use srui_semantic_tree::NodeId;
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const TERMINAL_LINE: &[u8] = b"\x1b[32mbenchmark output\x1b[0m\r\n";
const TERMINAL_LINES: usize = 256;
const PTY_TIMEOUT: Duration = Duration::from_secs(5);
const PTY_READ_CHUNK: usize = 8_192;

fn terminal_script(lines: usize) -> String {
    format!(
        "stty raw -echo; i=0; while [ \"$i\" -lt {lines} ]; do \
         printf '\\033[32mbenchmark output\\033[0m\\r\\n'; i=$((i + 1)); done"
    )
}

#[cfg(unix)]
fn standalone_signal_target(pid: Option<u32>) -> Option<libc::pid_t> {
    pid.and_then(|pid| libc::pid_t::try_from(pid).ok())
        .filter(|pid| *pid > 1)
}

fn signal_standalone_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = standalone_signal_target(pid) {
        // SAFETY: portable-pty's Unix backend calls setsid before exec, so this validated
        // child PID is also its process-group ID. A representable PID greater than one
        // excludes kill(2)'s caller-group, broadcast, and system-process special cases.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGHUP);
            let _ = libc::kill(-pid, libc::SIGKILL);
            let _ = libc::kill(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn cleanup_standalone_child(
    child: &mut (dyn portable_pty::Child + Send + Sync),
    pid: Option<u32>,
    reader_thread: Option<JoinHandle<()>>,
    wait_for_natural_exit: bool,
    deadline: Instant,
) -> Result<bool, String> {
    let mut exit_success = None;
    let mut cleanup_error = None;
    if wait_for_natural_exit {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    exit_success = Some(status.success());
                    break;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => break,
                Err(error) => {
                    cleanup_error = Some(error.to_string());
                    break;
                }
            }
        }
    }

    if exit_success.is_none() {
        signal_standalone_process_group(pid);
        let _ = child.kill();
        match child.wait() {
            Ok(status) => exit_success = Some(status.success()),
            Err(error) if cleanup_error.is_none() => cleanup_error = Some(error.to_string()),
            Err(_) => {}
        }
    }

    if let Some(reader_thread) = reader_thread {
        let reader_deadline = Instant::now() + Duration::from_millis(100);
        while !reader_thread.is_finished() && Instant::now() < reader_deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        if reader_thread.is_finished() {
            if reader_thread.join().is_err() && cleanup_error.is_none() {
                cleanup_error = Some("standalone PTY reader thread panicked".to_string());
            }
        } else {
            // Rust cannot safely cancel a blocked reader thread. Never turn the
            // five-second benchmark watchdog into an unbounded JoinHandle wait:
            // the already-signalled process owns the slave side, and dropping
            // this handle detaches the failed reader while the caller reports.
            signal_standalone_process_group(pid);
            if cleanup_error.is_none() {
                cleanup_error =
                    Some("standalone PTY reader did not stop after cleanup".to_string());
            }
        }
    }

    if let Some(error) = cleanup_error {
        Err(error)
    } else {
        Ok(exit_success.unwrap_or(false))
    }
}

fn standalone_pty_roundtrip_blocking(
    payload: &[u8],
    script: &str,
) -> Result<(f64, Vec<u8>, bool), String> {
    let system = native_pty_system();
    let start = Instant::now();
    let deadline = start + PTY_TIMEOUT;
    let pair = system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(script);
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;
    let child_pid = child.process_id();
    drop(pair.slave);
    drop(pair.master);

    let (send, receive) = mpsc::channel::<Result<Vec<u8>, String>>();
    let reader_thread = std::thread::Builder::new()
        .name("srui-benchmark-standalone-pty-reader".to_string())
        .spawn(move || {
            let mut reader = reader;
            let mut buffer = vec![0_u8; PTY_READ_CHUNK];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if send.send(Ok(buffer[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    #[cfg(unix)]
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                    Err(error) => {
                        let _ = send.send(Err(error.to_string()));
                        break;
                    }
                }
            }
        });

    let mut received = Vec::with_capacity(payload.len());
    let capture = match reader_thread.as_ref() {
        Err(error) => Err(format!("failed to spawn standalone PTY reader: {error}")),
        Ok(_) => {
            let mut boundary_elapsed = None;
            loop {
                let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                    break Err(format!(
                        "standalone PTY timed out before bounded natural EOF at {} of {} bytes",
                        received.len(),
                        payload.len()
                    ));
                };
                match receive.recv_timeout(remaining) {
                    Ok(Ok(chunk)) => {
                        if received.len() + chunk.len() > payload.len() {
                            break Err(format!(
                                "standalone PTY exceeded exact {}-byte payload",
                                payload.len()
                            ));
                        }
                        received.extend_from_slice(&chunk);
                        if received.len() == payload.len() && boundary_elapsed.is_none() {
                            boundary_elapsed = Some(start.elapsed().as_secs_f64() * 1_000.0);
                        }
                    }
                    Ok(Err(error)) => {
                        break Err(format!(
                            "standalone PTY reader failed before natural EOF: {error}"
                        ));
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        break Err(format!(
                            "standalone PTY timed out before bounded natural EOF at {} of {} bytes",
                            received.len(),
                            payload.len()
                        ));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        break boundary_elapsed.ok_or_else(|| {
                            format!(
                                "standalone PTY reached natural EOF at {} of {} bytes",
                                received.len(),
                                payload.len()
                            )
                        });
                    }
                }
            }
        }
    };

    let reader_thread = reader_thread.ok();
    let exit_success = cleanup_standalone_child(
        child.as_mut(),
        child_pid,
        reader_thread,
        capture.is_ok(),
        deadline,
    )?;
    let elapsed = capture?;
    Ok((elapsed, received, exit_success))
}

async fn standalone_pty_roundtrip(
    payload: Vec<u8>,
    script: String,
) -> Result<(f64, Vec<u8>, bool), String> {
    tokio::task::spawn_blocking(move || standalone_pty_roundtrip_blocking(&payload, &script))
        .await
        .map_err(|error| format!("standalone PTY worker failed: {error}"))?
}

fn terminal_spec(script: &str, ring_capacity: usize) -> TerminalSpec {
    TerminalSpec {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        ring_capacity,
        ..TerminalSpec::default()
    }
}

fn append_frame(
    frame: &TerminalData,
    id: NodeId,
    payload_len: usize,
    received: &mut Vec<u8>,
    expected_offset: &mut u64,
    frame_count: &mut usize,
) -> Result<(), String> {
    if frame.stream_id != id.get()
        || frame.byte_offset != *expected_offset
        || frame.data.is_empty()
        || frame.data.len() > MAX_TERMINAL_OUTPUT_FRAME_BYTES
    {
        return Err("embedded PTY emitted an invalid frame boundary or offset".to_string());
    }
    if received.len() + frame.data.len() > payload_len {
        return Err(format!(
            "embedded PTY exceeded exact {payload_len}-byte boundary"
        ));
    }
    *expected_offset = expected_offset.saturating_add(frame.data.len() as u64);
    received.extend_from_slice(&frame.data);
    *frame_count += 1;
    Ok(())
}

async fn embedded_pty_roundtrip(
    payload: &[u8],
    script: &str,
) -> Result<(f64, Vec<u8>, usize, bool), String> {
    let manager = PTYManager::default();
    let id = NodeId::new(1);
    let result = async {
        let start = Instant::now();
        manager
            .spawn(id, terminal_spec(script, payload.len() * 2))
            .map_err(|error| error.to_string())?;
        let outcome = manager
            .subscribe(id, 0)
            .map_err(|error| error.to_string())?;
        let SubscribeOutcome {
            snapshot,
            mut subscription,
            ..
        } = outcome;
        let mut received = Vec::with_capacity(payload.len());
        let mut expected_offset = 0_u64;
        let mut frame_count = 0_usize;
        match snapshot {
            SubscribeSnapshot::Replay { frames } => {
                for frame in frames {
                    append_frame(
                        &frame,
                        id,
                        payload.len(),
                        &mut received,
                        &mut expected_offset,
                        &mut frame_count,
                    )?;
                }
            }
            SubscribeSnapshot::Resync { .. } => {
                return Err("embedded PTY resynced before exact capture".to_string());
            }
        }

        let mut boundary_elapsed =
            (received.len() == payload.len()).then(|| start.elapsed().as_secs_f64() * 1_000.0);
        let mut offsets_exact_at_boundary = boundary_elapsed.is_some()
            && manager.offsets(id) == Some((0, payload.len() as u64))
            && subscription.cursor() == payload.len() as u64
            && expected_offset == payload.len() as u64;
        let deadline = tokio::time::Instant::now() + PTY_TIMEOUT;
        loop {
            let events = tokio::time::timeout_at(deadline, subscription.recv())
                .await
                .map_err(|_| {
                    format!(
                        "embedded PTY timed out before bounded natural EOF at {} of {} bytes",
                        received.len(),
                        payload.len()
                    )
                })?;
            if events.is_empty() {
                break;
            }
            for event in events {
                match event {
                    TerminalEvent::Data(frame) => {
                        append_frame(
                            &frame,
                            id,
                            payload.len(),
                            &mut received,
                            &mut expected_offset,
                            &mut frame_count,
                        )?;
                        if received.len() == payload.len() && boundary_elapsed.is_none() {
                            boundary_elapsed = Some(start.elapsed().as_secs_f64() * 1_000.0);
                            offsets_exact_at_boundary = manager.offsets(id)
                                == Some((0, payload.len() as u64))
                                && subscription.cursor() == payload.len() as u64
                                && expected_offset == payload.len() as u64;
                        }
                    }
                    TerminalEvent::Resync(_) => {
                        return Err("embedded PTY lost retained output during exact capture".into());
                    }
                }
            }
        }

        let elapsed = boundary_elapsed.ok_or_else(|| {
            format!(
                "embedded PTY reached natural EOF at {} of {} bytes",
                received.len(),
                payload.len()
            )
        })?;
        let offsets_exact = offsets_exact_at_boundary
            && subscription.cursor() == payload.len() as u64
            && expected_offset == payload.len() as u64;
        Ok((elapsed, received, frame_count, offsets_exact))
    }
    .await;
    // PTYManager::drop also calls shutdown(), so cancellation or unwinding before this
    // normal-path cleanup still kills and reaps every managed child.
    manager.shutdown();
    result
}

async fn ring_exhaustion(payload: &[u8], script: &str) -> Result<(f64, bool), String> {
    let manager = PTYManager::default();
    let id = NodeId::new(2);
    let result = async {
        manager
            .spawn(id, terminal_spec(script, 1_024))
            .map_err(|error| error.to_string())?;
        let SubscribeOutcome {
            mut subscription, ..
        } = manager
            .subscribe(id, 0)
            .map_err(|error| error.to_string())?;
        let deadline = tokio::time::Instant::now() + PTY_TIMEOUT;
        while subscription.cursor() < payload.len() as u64 {
            let events = tokio::time::timeout_at(deadline, subscription.recv())
                .await
                .map_err(|_| {
                    format!(
                        "ring exhaustion timed out before exact {}-byte offset",
                        payload.len()
                    )
                })?;
            if events.is_empty() {
                return Err(format!(
                    "ring exhaustion stream closed at offset {} of {}",
                    subscription.cursor(),
                    payload.len()
                ));
            }
        }
        if subscription.cursor() != payload.len() as u64
            || manager.offsets(id).map(|offsets| offsets.1) != Some(payload.len() as u64)
        {
            return Err(format!(
                "ring exhaustion crossed expected final offset {}",
                payload.len()
            ));
        }

        let start = Instant::now();
        let exhausted = manager
            .subscribe(id, 0)
            .map_err(|error| error.to_string())?;
        let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
        let detected = matches!(
            &exhausted.snapshot,
            SubscribeSnapshot::Resync {
                requested_offset: 0,
                retained_from_offset,
                resume_at_offset,
                reason: TerminalResyncReason::RetentionLoss,
            } if *retained_from_offset > 0 && *resume_at_offset == payload.len() as u64
        ) && matches!(
            exhausted.catch_up_events().as_slice(),
            [TerminalEvent::Resync(resync)]
                if resync.reason == TerminalResyncReason::RetentionLoss as i32
                    && resync.requested_offset == 0
                    && resync.retained_from_offset > 0
                    && resync.resume_at_offset == payload.len() as u64
        );
        Ok((elapsed, detected))
    }
    .await;
    // PTYManager::drop also calls shutdown(), so cancellation or unwinding before this
    // normal-path cleanup still kills and reaps every managed child.
    manager.shutdown();
    result
}

pub(crate) async fn terminal(iterations: usize) -> Result<Section, String> {
    let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
    let script = terminal_script(TERMINAL_LINES);
    let sample_count = iterations.min(20);
    let mut standalone_ms = Vec::with_capacity(sample_count);
    let mut embedded_ms = Vec::with_capacity(sample_count);
    let mut exact_payloads = true;
    let mut standalone_exit_success = true;
    let mut offsets_exact = true;
    let mut embedded_frame_counts = Vec::with_capacity(sample_count);

    for _ in 0..sample_count {
        let (elapsed, received, exited_successfully) =
            standalone_pty_roundtrip(payload.clone(), script.clone()).await?;
        standalone_ms.push(elapsed);
        exact_payloads &= received == payload;
        standalone_exit_success &= exited_successfully;

        let (elapsed, received, frame_count, sample_offsets_exact) =
            embedded_pty_roundtrip(&payload, &script).await?;
        embedded_ms.push(elapsed);
        exact_payloads &= received == payload;
        offsets_exact &= sample_offsets_exact;
        embedded_frame_counts.push(frame_count as f64);
    }

    let (exhaustion_ms, exhaustion_detected) = ring_exhaustion(&payload, &script).await?;

    let sample_counts = BTreeMap::from([
        ("rust.standalone_pty", standalone_ms.len()),
        ("rust.embedded_pty", embedded_ms.len()),
        ("rust.ring_exhaustion", 1),
    ]);
    let mut metrics = Vec::new();
    let mut timings = BTreeMap::new();
    timings.insert("standalone PTY exact ANSI interaction", standalone_ms);
    timings.insert(
        "embedded SRUI PTY exact ANSI capture and framing",
        embedded_ms,
    );
    push_timing_distributions(&mut metrics, timings)?;
    metrics.push(metric(
        "terminal reconnect retention-loss decision",
        exhaustion_ms,
        "ms",
        "sample",
    ));
    metrics.push(metric(
        "terminal payload",
        payload.len() as f64,
        "bytes",
        "exact",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        p50(embedded_frame_counts.clone())?,
        "messages",
        "p50",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        percentile(embedded_frame_counts.clone(), 0.95)?,
        "messages",
        "p95",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        percentile(embedded_frame_counts.clone(), 0.99)?,
        "messages",
        "p99",
    ));

    Ok(Section {
        id: "31.6",
        name: "Terminal",
        sample_counts,
        metrics,
        assertions: vec![
            Assertion {
                id: "pty_payload_identical",
                name: "standalone and embedded PTYs emit the identical ANSI byte stream",
                passed: exact_payloads,
                detail: format!("both paths compared all {} payload bytes", payload.len()),
            },
            Assertion {
                id: "standalone_pty_exit_success",
                name: "standalone terminal command exits successfully",
                passed: standalone_exit_success,
                detail: format!("{sample_count} bounded child exit statuses checked"),
            },
            Assertion {
                id: "embedded_pty_eof_exact",
                name: "embedded terminal reaches exact output and bounded natural EOF",
                passed: offsets_exact,
                detail: format!(
                    "{sample_count} production OutputRing captures reached exact byte offset {} and then natural EOF before explicit shutdown",
                    payload.len()
                ),
            },
            Assertion {
                id: "embedded_terminal_frame_bounds",
                name: "embedded terminal frames preserve exact offsets and bounds",
                passed: offsets_exact,
                detail: format!(
                    "{} samples aggregated; p50 {:.0} frames, each at most {MAX_TERMINAL_OUTPUT_FRAME_BYTES} bytes",
                    embedded_frame_counts.len(),
                    p50(embedded_frame_counts)?
                ),
            },
            Assertion {
                id: "terminal_ring_retention_loss",
                name: "reconnect ring-buffer exhaustion maps to RETENTION_LOSS",
                passed: exhaustion_detected,
                detail:
                    "PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired"
                        .into(),
            },
        ],
        notes: vec![
            "Both clocks start immediately before PTY allocation and stop when the identical exact ANSI byte count is first observed. Each path then drains to bounded natural EOF and rejects trailing bytes before reporting success. Standalone blocking I/O runs in spawn_blocking behind a five-second watchdog and one nonblocking-reader cleanup path; embedded observes production OutputRing frames and offsets through PTYManager subscriptions. Process teardown occurs after the timed boundary and EOF proof."
                .into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn signal_target_rejects_posix_special_and_unrepresentable_process_ids() {
        assert_eq!(standalone_signal_target(None), None);
        assert_eq!(standalone_signal_target(Some(0)), None);
        assert_eq!(standalone_signal_target(Some(1)), None);
        assert_eq!(standalone_signal_target(Some(u32::MAX)), None);
        assert_eq!(standalone_signal_target(Some(2)), Some(2));
    }

    #[test]
    fn terminal_fixture_matches_cross_language_contract() {
        let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
        assert_eq!(payload.len(), 6_912);
        assert_eq!(
            &payload[..TERMINAL_LINE.len()],
            b"\x1b[32mbenchmark output\x1b[0m\r\n"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn standalone_and_embedded_paths_capture_exact_fixture() {
        let payload = TERMINAL_LINE.repeat(4);
        let script = terminal_script(4);
        let (_, standalone, exited) = standalone_pty_roundtrip(payload.clone(), script.clone())
            .await
            .unwrap();
        let (_, embedded, _, exact_offsets) =
            embedded_pty_roundtrip(&payload, &script).await.unwrap();

        assert_eq!(standalone, payload);
        assert!(exited);
        assert_eq!(embedded, payload);
        assert!(exact_offsets);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn both_pty_paths_reject_trailing_bytes_after_the_expected_boundary() {
        let payload = TERMINAL_LINE.to_vec();
        let script = format!("{}; printf trailing-junk", terminal_script(1));

        let standalone = standalone_pty_roundtrip(payload.clone(), script.clone()).await;
        assert!(standalone.unwrap_err().contains("exceeded exact"));

        let embedded = embedded_pty_roundtrip(&payload, &script).await;
        assert!(embedded.unwrap_err().contains("exceeded exact"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn production_ring_exhaustion_reports_retention_loss() {
        let payload = TERMINAL_LINE.repeat(256);
        let script = terminal_script(256);
        let (_, detected) = ring_exhaustion(&payload, &script).await.unwrap();
        assert!(detected);
    }
}
