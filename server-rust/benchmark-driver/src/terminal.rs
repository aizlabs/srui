use crate::report::{metric, p50, percentile, push_timing_distributions, Assertion, Section};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use srui_protocol::TerminalResyncReason;
use srui_pty::{
    PTYManager, SubscribeSnapshot, TerminalEvent, TerminalSpec, MAX_TERMINAL_OUTPUT_FRAME_BYTES,
};
use srui_semantic_tree::NodeId;
use std::collections::BTreeMap;
use std::io::Read;
use std::time::{Duration, Instant};

const TERMINAL_LINE: &[u8] = b"\x1b[32mbenchmark output\x1b[0m\r\n";
const TERMINAL_LINES: usize = 256;

fn terminal_script(lines: usize) -> String {
    format!(
        "stty raw -echo; i=0; while [ \"$i\" -lt {lines} ]; do \
         printf '\\033[32mbenchmark output\\033[0m\\r\\n'; i=$((i + 1)); done"
    )
}

fn pty_roundtrip(payload: &[u8], script: &str) -> Result<(f64, Vec<u8>, bool), String> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(script);
    let start = Instant::now();
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut received = Vec::with_capacity(payload.len());
    reader
        .read_to_end(&mut received)
        .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    Ok((elapsed, received, status.success()))
}

fn terminal_spec(script: &str, ring_capacity: usize) -> TerminalSpec {
    TerminalSpec {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        ring_capacity,
        ..TerminalSpec::default()
    }
}

async fn wait_for_terminal_exit(manager: &PTYManager, id: NodeId) -> Result<bool, String> {
    tokio::time::timeout(Duration::from_secs(5), manager.benchmark_wait_for_exit(id))
        .await
        .map_err(|_| format!("timed out waiting for terminal {id:?} process exit"))?
        .map_err(|error| error.to_string())
}

async fn embedded_pty_roundtrip(
    payload: &[u8],
    script: &str,
) -> Result<(f64, Vec<u8>, usize, bool, bool), String> {
    let manager = PTYManager::default();
    let id = NodeId::new(1);
    let start = Instant::now();
    manager
        .spawn(id, terminal_spec(script, payload.len() * 2))
        .map_err(|error| error.to_string())?;
    let exit_success = wait_for_terminal_exit(&manager, id).await?;
    let final_offset = manager
        .offsets(id)
        .ok_or_else(|| "embedded PTY disappeared before final offset sampling".to_string())?
        .1;
    let outcome = manager
        .subscribe(id, 0)
        .map_err(|error| error.to_string())?;
    let mut received = Vec::with_capacity(payload.len());
    let mut expected_offset = 0_u64;
    let mut offsets_exact = true;
    let frames = match outcome.snapshot {
        SubscribeSnapshot::Replay { frames } => frames,
        SubscribeSnapshot::Resync { .. } => {
            manager.shutdown();
            return Ok((
                start.elapsed().as_secs_f64() * 1_000.0,
                received,
                0,
                false,
                exit_success,
            ));
        }
    };
    for frame in &frames {
        offsets_exact &= frame.stream_id == id.get()
            && frame.byte_offset == expected_offset
            && !frame.data.is_empty()
            && frame.data.len() <= MAX_TERMINAL_OUTPUT_FRAME_BYTES;
        expected_offset = expected_offset.saturating_add(frame.data.len() as u64);
        received.extend_from_slice(&frame.data);
    }
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    manager.shutdown();
    Ok((
        elapsed,
        received,
        frames.len(),
        offsets_exact && final_offset == payload.len() as u64,
        exit_success,
    ))
}

pub(crate) async fn terminal(iterations: usize) -> Result<Section, String> {
    let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
    let script = terminal_script(TERMINAL_LINES);
    let sample_count = iterations.min(20);
    let mut standalone_ms = Vec::with_capacity(sample_count);
    let mut embedded_ms = Vec::with_capacity(sample_count);
    let mut exact_payloads = true;
    let mut standalone_exit_success = true;
    let mut embedded_exit_success = true;
    let mut offsets_exact = true;
    let mut embedded_frame_counts = Vec::with_capacity(sample_count);

    for _ in 0..sample_count {
        let (elapsed, received, exited_successfully) = pty_roundtrip(&payload, &script)?;
        standalone_ms.push(elapsed);
        exact_payloads &= received == payload;
        standalone_exit_success &= exited_successfully;

        let (elapsed, received, frame_count, sample_offsets_exact, sample_exit_success) =
            embedded_pty_roundtrip(&payload, &script).await?;
        embedded_ms.push(elapsed);
        exact_payloads &= received == payload;
        offsets_exact &= sample_offsets_exact;
        embedded_exit_success &= sample_exit_success;
        embedded_frame_counts.push(frame_count as f64);
    }

    // Exhaust the ring through a real PTY stream, then reconnect through PTYManager::subscribe.
    // The public subscription maps the retention gap to one explicit RETENTION_LOSS event.
    let exhaustion_manager = PTYManager::default();
    let exhaustion_id = NodeId::new(2);
    exhaustion_manager
        .spawn(exhaustion_id, terminal_spec(&script, 1_024))
        .map_err(|error| error.to_string())?;
    let exhaustion_exit_success =
        wait_for_terminal_exit(&exhaustion_manager, exhaustion_id).await?;
    let start = Instant::now();
    let exhausted = exhaustion_manager
        .subscribe(exhaustion_id, 0)
        .map_err(|error| error.to_string())?;
    let exhaustion_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let exhaustion_detected = matches!(
        &exhausted.snapshot,
        SubscribeSnapshot::Resync {
            requested_offset: 0,
            retained_from_offset,
            resume_at_offset,
            reason: TerminalResyncReason::RetentionLoss,
        } if *retained_from_offset > 0 && *resume_at_offset == payload.len() as u64
    ) && exhaustion_exit_success
        && matches!(
            exhausted.catch_up_events().as_slice(),
            [TerminalEvent::Resync(resync)]
                if resync.reason == TerminalResyncReason::RetentionLoss as i32
                    && resync.requested_offset == 0
                    && resync.retained_from_offset > 0
                    && resync.resume_at_offset == payload.len() as u64
        );
    exhaustion_manager.shutdown();

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
    push_timing_distributions(&mut metrics, timings);
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
        p50(embedded_frame_counts.clone()),
        "messages",
        "p50",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        percentile(embedded_frame_counts.clone(), 0.95),
        "messages",
        "p95",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        percentile(embedded_frame_counts.clone(), 0.99),
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
                detail: format!("{sample_count} child exit statuses checked"),
            },
            Assertion {
                id: "embedded_pty_eof_exact",
                name: "embedded terminal reaches natural successful EOF with no trailing bytes",
                passed: embedded_exit_success && offsets_exact,
                detail: format!(
                    "{sample_count} production PTY exit statuses checked after the final exact offset"
                ),
            },
            Assertion {
                id: "embedded_terminal_frame_bounds",
                name: "embedded terminal frames preserve exact offsets and bounds",
                passed: offsets_exact,
                detail: format!(
                    "{} samples aggregated; p50 {:.0} frames, each at most {MAX_TERMINAL_OUTPUT_FRAME_BYTES} bytes",
                    embedded_frame_counts.len(),
                    p50(embedded_frame_counts)
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
            "Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes. The embedded EOF boundary is event-driven and does not poll process state."
                .into(),
        ],
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_fixture_matches_cross_language_contract() {
        let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
        assert_eq!(payload.len(), 6_912);
        assert_eq!(
            &payload[..TERMINAL_LINE.len()],
            b"\x1b[32mbenchmark output\x1b[0m\r\n"
        );
    }
}
