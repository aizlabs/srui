//! Session-scoped PTY stream table (§21, §21.2).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use srui_protocol::TerminalResyncRequired;
use srui_semantic_tree::NodeId;
use thiserror::Error;
use tokio::sync::watch;

use crate::ring::OutputRing;
use crate::spec::TerminalSpec;
use crate::stream::{
    live_events_from_ring, StreamCommand, SubscribeSnapshot, TerminalEvent, TerminalStream,
    TerminalStreamError,
};

/// Configuration for [`PTYManager`].
#[derive(Debug, Clone)]
pub struct PTYManagerConfig {
    /// Maximum concurrently live streams.
    pub max_streams: usize,
}

impl Default for PTYManagerConfig {
    fn default() -> Self {
        Self { max_streams: 32 }
    }
}

#[derive(Debug, Error)]
pub enum PTYManagerError {
    #[error("maximum terminal stream count {limit} reached")]
    StreamLimit { limit: usize },
    #[error("terminal stream {0} already exists")]
    DuplicateStream(u64),
    #[error("unknown terminal stream {0}")]
    UnknownStream(u64),
    #[error(transparent)]
    Stream(#[from] TerminalStreamError),
}

/// Owns every live PTY for one semantic session.
pub struct PTYManager {
    inner: Mutex<Inner>,
    max_streams: usize,
}

struct Inner {
    streams: HashMap<NodeId, Arc<TerminalStream>>,
}

impl std::fmt::Debug for PTYManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .inner
            .lock()
            .map(|guard| guard.streams.len())
            .unwrap_or(0);
        f.debug_struct("PTYManager")
            .field("stream_count", &count)
            .field("max_streams", &self.max_streams)
            .finish()
    }
}

impl Default for PTYManager {
    fn default() -> Self {
        Self::new(PTYManagerConfig::default())
    }
}

impl PTYManager {
    #[must_use]
    pub fn new(config: PTYManagerConfig) -> Self {
        Self {
            inner: Mutex::new(Inner {
                streams: HashMap::new(),
            }),
            max_streams: config.max_streams.max(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Spawns a PTY bound to `id`. The stream stays alive across semantic detach.
    pub fn spawn(&self, id: NodeId, spec: TerminalSpec) -> Result<(), PTYManagerError> {
        let mut inner = self.lock();
        if inner.streams.contains_key(&id) {
            return Err(PTYManagerError::DuplicateStream(id.get()));
        }
        if inner.streams.len() >= self.max_streams {
            return Err(PTYManagerError::StreamLimit {
                limit: self.max_streams,
            });
        }
        let stream = TerminalStream::spawn(id, spec)?;
        inner.streams.insert(id, stream);
        Ok(())
    }

    pub fn input(&self, id: NodeId, data: Vec<u8>) -> Result<(), PTYManagerError> {
        self.stream(id)?.try_enqueue(StreamCommand::Input(data))?;
        Ok(())
    }

    pub fn resize(
        &self,
        id: NodeId,
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<(), PTYManagerError> {
        self.stream(id)?.try_enqueue(StreamCommand::Resize {
            columns,
            rows,
            pixel_width,
            pixel_height,
        })?;
        Ok(())
    }

    /// Captures a replay/live cut atomically for one client attachment.
    pub fn subscribe(
        &self,
        id: NodeId,
        requested_offset: u64,
    ) -> Result<SubscribeOutcome, PTYManagerError> {
        let stream = self.stream(id)?;
        let (snapshot, subscription) = stream.subscribe(requested_offset);
        Ok(SubscribeOutcome {
            stream_id: id,
            snapshot,
            subscription,
        })
    }

    /// Subscribes every live stream. Missing map entries start at offset 0.
    pub fn subscribe_all(
        &self,
        offsets: &HashMap<u64, u64>,
    ) -> Result<Vec<SubscribeOutcome>, PTYManagerError> {
        let streams: Vec<(NodeId, Arc<TerminalStream>)> = {
            let inner = self.lock();
            inner
                .streams
                .iter()
                .map(|(id, stream)| (*id, Arc::clone(stream)))
                .collect()
        };
        let mut outcomes = Vec::with_capacity(streams.len());
        for (id, stream) in streams {
            let requested = offsets.get(&id.get()).copied().unwrap_or(0);
            let (snapshot, subscription) = stream.subscribe(requested);
            outcomes.push(SubscribeOutcome {
                stream_id: id,
                snapshot,
                subscription,
            });
        }
        Ok(outcomes)
    }

    pub fn close(&self, id: NodeId) -> Result<(), PTYManagerError> {
        let stream = {
            let mut inner = self.lock();
            inner
                .streams
                .remove(&id)
                .ok_or(PTYManagerError::UnknownStream(id.get()))?
        };
        stream.kill_and_reap();
        Ok(())
    }

    pub fn close_many(&self, ids: impl IntoIterator<Item = NodeId>) {
        for id in ids {
            let _ = self.close(id);
        }
    }

    /// Kills and reaps every child. Used on session terminate/expiry.
    pub fn shutdown(&self) {
        let streams: Vec<Arc<TerminalStream>> = {
            let mut inner = self.lock();
            inner.streams.drain().map(|(_, stream)| stream).collect()
        };
        for stream in streams {
            stream.kill_and_reap();
        }
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.lock().streams.contains_key(&id)
    }

    pub fn live_stream_ids(&self) -> Vec<NodeId> {
        self.lock().streams.keys().copied().collect()
    }

    pub fn offsets(&self, id: NodeId) -> Option<(u64, u64)> {
        self.lock()
            .streams
            .get(&id)
            .map(|stream| stream.snapshot_offsets())
    }

    #[cfg(test)]
    pub(crate) fn process_id(&self, id: NodeId) -> Option<u32> {
        self.lock()
            .streams
            .get(&id)
            .and_then(|stream| stream.process_id())
    }

    fn stream(&self, id: NodeId) -> Result<Arc<TerminalStream>, PTYManagerError> {
        self.lock()
            .streams
            .get(&id)
            .cloned()
            .ok_or(PTYManagerError::UnknownStream(id.get()))
    }
}

impl Drop for PTYManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Result of capturing a subscription against the current ring.
pub struct SubscribeOutcome {
    pub stream_id: NodeId,
    pub snapshot: SubscribeSnapshot,
    pub subscription: TerminalSubscription,
}

impl std::fmt::Debug for SubscribeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscribeOutcome")
            .field("stream_id", &self.stream_id)
            .field("snapshot", &self.snapshot)
            .field("cursor", &self.subscription.cursor)
            .finish()
    }
}

impl SubscribeOutcome {
    /// Handshake catch-up frames. Replay uses `terminalNormal`; resync uses `terminalHigh`.
    pub fn catch_up_events(&self) -> Vec<TerminalEvent> {
        match &self.snapshot {
            SubscribeSnapshot::Replay { frames } => {
                frames.iter().cloned().map(TerminalEvent::Data).collect()
            }
            SubscribeSnapshot::Resync {
                requested_offset,
                retained_from_offset,
                resume_at_offset,
                reason,
            } => vec![TerminalEvent::Resync(TerminalResyncRequired {
                stream_id: self.stream_id.get(),
                requested_offset: *requested_offset,
                retained_from_offset: *retained_from_offset,
                resume_at_offset: *resume_at_offset,
                reason: *reason as i32,
            })],
        }
    }

    #[must_use]
    pub fn is_replay(&self) -> bool {
        matches!(self.snapshot, SubscribeSnapshot::Replay { .. })
    }
}

/// Cursor-based live subscription over one ring.
pub struct TerminalSubscription {
    pub(crate) stream_id: NodeId,
    pub(crate) ring: Arc<Mutex<OutputRing>>,
    pub(crate) watch: watch::Receiver<u64>,
    pub(crate) cursor: u64,
}

impl TerminalSubscription {
    #[must_use]
    pub fn stream_id(&self) -> NodeId {
        self.stream_id
    }

    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Non-blocking drain of newly retained bytes or a fall-behind resync.
    pub fn try_drain(&mut self) -> Vec<TerminalEvent> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        live_events_from_ring(self.stream_id, &ring, &mut self.cursor)
    }

    /// Waits until the ring advances past `cursor`, then drains.
    ///
    /// An **empty vector means the subscription producer has stopped and all retained bytes have
    /// been drained**, not a spurious wake. The reader thread owns the only sender, so channel
    /// closure is sticky after that thread exits -- whether because the PTY returned EOF, a
    /// terminal read failed, or output could not be appended to the ring. `recv` does not
    /// distinguish those causes. Callers must treat an empty result as terminal and stop polling:
    /// every later call also returns empty immediately. A non-empty result always carries at
    /// least one event.
    pub async fn recv(&mut self) -> Vec<TerminalEvent> {
        loop {
            let drained = self.try_drain();
            if !drained.is_empty() {
                return drained;
            }
            if self.watch.changed().await.is_err() {
                return Vec::new();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::spec::TerminalSpec;
    use srui_protocol::TerminalResyncReason;

    fn retained_bytes(manager: &PTYManager, id: NodeId) -> Vec<u8> {
        let Some((start, end)) = manager.offsets(id) else {
            return Vec::new();
        };
        if end <= start {
            return Vec::new();
        }
        let Ok(outcome) = manager.subscribe(id, start) else {
            return Vec::new();
        };
        outcome
            .catch_up_events()
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Data(data) => Some(data.data),
                TerminalEvent::Resync(_) => None,
            })
            .flatten()
            .collect()
    }

    fn wait_for_output(manager: &PTYManager, id: NodeId, needle: &[u8]) -> Vec<u8> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut last = Vec::new();
        while std::time::Instant::now() < deadline {
            last = retained_bytes(manager, id);
            if last.windows(needle.len()).any(|window| window == needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "timed out waiting for {:?}; retained {:?}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(&last)
        );
    }

    fn wait_for_bytes(manager: &PTYManager, id: NodeId, min_next_offset: u64) -> (u64, u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some((start, end)) = manager.offsets(id) {
                if end >= min_next_offset {
                    return (start, end);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for next_offset >= {min_next_offset}");
    }

    fn echo_spec(command: &str, ring_capacity: usize) -> TerminalSpec {
        TerminalSpec {
            executable: "/bin/sh".into(),
            args: vec!["-c".to_string(), command.to_string()],
            ring_capacity,
            ..TerminalSpec::default()
        }
    }

    #[test]
    fn spawn_shell_sends_sentinel_and_receives_output() {
        let manager = PTYManager::default();
        let id = NodeId::new(7);
        manager
            .spawn(
                id,
                echo_spec("printf 'SRUI_SENTINEL_OK\\n'; sleep 30", 64 * 1024),
            )
            .unwrap();
        let bytes = wait_for_output(&manager, id, b"SRUI_SENTINEL_OK");
        assert!(
            bytes
                .windows(b"SRUI_SENTINEL_OK".len())
                .any(|window| window == b"SRUI_SENTINEL_OK"),
            "sentinel missing from {:?}",
            String::from_utf8_lossy(&bytes)
        );
        manager.shutdown();
    }

    #[test]
    fn resize_is_visible_to_stty() {
        let manager = PTYManager::default();
        let id = NodeId::new(8);
        manager
            .spawn(
                id,
                TerminalSpec {
                    executable: "/bin/sh".into(),
                    args: vec!["-i".to_string()],
                    columns: 40,
                    rows: 12,
                    ring_capacity: 64 * 1024,
                    ..TerminalSpec::default()
                },
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        manager.resize(id, 91, 33, 0, 0).unwrap();
        std::thread::sleep(Duration::from_millis(40));
        manager.input(id, b"stty size\n".to_vec()).unwrap();
        let bytes = wait_for_output(&manager, id, b"33 91");
        assert!(
            bytes.windows(5).any(|w| w == b"33 91"),
            "stty size output missing 33 91 in {:?}",
            String::from_utf8_lossy(&bytes)
        );
        manager.shutdown();
    }

    #[test]
    fn subscribe_replays_from_mid_chunk() {
        let manager = PTYManager::default();
        let id = NodeId::new(9);
        manager
            .spawn(id, echo_spec("printf 'abcdefghij'; sleep 30", 512))
            .unwrap();
        let out = wait_for_output(&manager, id, b"abcdefghij");
        let abc_offset = out
            .windows(b"abcdefghij".len())
            .position(|w| w == b"abcdefghij")
            .unwrap() as u64;
        let (retained_start, _) = manager.offsets(id).unwrap();
        let target_offset = retained_start + abc_offset + 3;
        let outcome = manager.subscribe(id, target_offset).unwrap();
        let replay: Vec<u8> = outcome
            .catch_up_events()
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Data(data) => Some(data.data),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(
            replay.starts_with(b"defghij"),
            "mid-chunk replay was {:?}",
            String::from_utf8_lossy(&replay)
        );
        manager.shutdown();
    }

    #[test]
    fn offset_ahead_and_retention_resync() {
        let manager = PTYManager::default();
        let id = NodeId::new(10);
        manager
            .spawn(id, echo_spec("printf '0123456789ABCDEF'; sleep 30", 8))
            .unwrap();
        let (retained, next) = wait_for_bytes(&manager, id, 8);
        assert!(next >= 8);
        let ahead = manager.subscribe(id, next + 4).unwrap();
        match ahead.snapshot {
            SubscribeSnapshot::Resync { reason, .. } => {
                assert_eq!(reason, TerminalResyncReason::OffsetAhead);
            }
            other => panic!("expected offset-ahead resync, got {other:?}"),
        }
        if retained > 0 {
            let lost = manager.subscribe(id, 0).unwrap();
            match lost.snapshot {
                SubscribeSnapshot::Resync { reason, .. } => {
                    assert_eq!(reason, TerminalResyncReason::RetentionLoss);
                }
                other => panic!("expected retention resync, got {other:?}"),
            }
        }
        manager.shutdown();
    }

    #[test]
    fn slow_subscription_fallbehind_is_local() {
        let manager = PTYManager::default();
        let id = NodeId::new(11);
        manager
            .spawn(
                id,
                echo_spec("printf 'abcdefghijklmnopqrstuvwxyz'; sleep 30", 6),
            )
            .unwrap();
        wait_for_bytes(&manager, id, 6);
        let mut stale = manager.subscribe(id, 0).unwrap().subscription;
        stale.cursor = 0;
        wait_for_bytes(&manager, id, 26);
        let events = stale.try_drain();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Resync(r) if r.reason == TerminalResyncReason::SubscriberFallbehind as i32)),
            "expected subscriber fallbehind, got {events:?}"
        );
        manager.shutdown();
    }

    #[test]
    fn input_and_resize_are_fifo() {
        let manager = PTYManager::default();
        let id = NodeId::new(12);
        manager
            .spawn(
                id,
                TerminalSpec {
                    executable: "/bin/sh".into(),
                    args: vec!["-i".to_string()],
                    ring_capacity: 64 * 1024,
                    ..TerminalSpec::default()
                },
            )
            .unwrap();
        manager.resize(id, 80, 24, 0, 0).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        manager
            .input(id, b"printf 'SRUI_FIFO_A'; printf 'SRUI_FIFO_B'\n".to_vec())
            .unwrap();
        let bytes = wait_for_output(&manager, id, b"SRUI_FIFO_A");
        wait_for_output(&manager, id, b"SRUI_FIFO_B");
        let a = bytes
            .windows(b"SRUI_FIFO_A".len())
            .position(|window| window == b"SRUI_FIFO_A");
        let later = retained_bytes(&manager, id);
        let b = later
            .windows(b"SRUI_FIFO_B".len())
            .position(|window| window == b"SRUI_FIFO_B");
        assert!(
            a.is_some() && b.is_some() && a.unwrap() <= b.unwrap(),
            "FIFO violated in {:?}",
            String::from_utf8_lossy(&later)
        );
        manager.shutdown();
    }

    #[test]
    fn shutdown_reaps_child() {
        let manager = PTYManager::default();
        let id = NodeId::new(13);
        manager.spawn(id, echo_spec("sleep 60", 32)).unwrap();
        manager.shutdown();
        assert!(manager.live_stream_ids().is_empty());
    }

    #[test]
    fn shutdown_does_not_deadlock_when_command_queue_is_full() {
        let manager = PTYManager::default();
        let id = NodeId::new(21);
        manager.spawn(id, echo_spec("sleep 60", 32)).unwrap();
        for _ in 0..128 {
            let _ = manager.input(id, vec![b'x'; 1024]);
        }
        let started = std::time::Instant::now();
        manager.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "kill_and_reap deadlocked after {:?}",
            started.elapsed()
        );
        assert!(manager.live_stream_ids().is_empty());
    }

    #[test]
    fn shutdown_unblocks_when_background_job_holds_slave() {
        let manager = PTYManager::default();
        let id = NodeId::new(23);
        manager
            .spawn(id, echo_spec("sleep 120 & sleep 120", 32))
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        for _ in 0..128 {
            let _ = manager.input(id, vec![b'x'; 1024]);
        }
        let started = std::time::Instant::now();
        manager.shutdown();
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "leftover slave holder hung kill_and_reap after {:?}",
            started.elapsed()
        );
        assert!(manager.live_stream_ids().is_empty());
    }

    #[tokio::test]
    async fn recv_drains_then_reports_end_of_stream_after_natural_eof() {
        let manager = PTYManager::default();
        let id = NodeId::new(24);
        manager
            .spawn(id, echo_spec("printf 'SRUI_EOF_OK'; exit 0", 4_096))
            .unwrap();
        let SubscribeOutcome {
            mut subscription, ..
        } = manager.subscribe(id, 0).unwrap();

        let mut received = Vec::new();
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let events = subscription.recv().await;
                if events.is_empty() {
                    // Channel closure, not a spurious wake: the reader has exited.
                    return true;
                }
                for event in events {
                    if let TerminalEvent::Data(data) = event {
                        received.extend_from_slice(&data.data);
                    }
                }
            }
        })
        .await
        .expect("recv must report end of stream after natural EOF");

        assert!(ended);
        assert!(
            received
                .windows(b"SRUI_EOF_OK".len())
                .any(|window| window == b"SRUI_EOF_OK"),
            "end of stream arrived before the retained bytes: {:?}",
            String::from_utf8_lossy(&received)
        );
        // Terminal, and stays terminal: a caller that ignored the empty result would spin here.
        assert!(subscription.recv().await.is_empty());
        manager.shutdown();
    }

    #[test]
    fn natural_exit_reaps_the_child() {
        let manager = PTYManager::default();
        let id = NodeId::new(22);
        manager
            .spawn(id, echo_spec("printf 'SRUI_EXIT_OK'; exit 0", 32))
            .unwrap();
        let pid = manager
            .process_id(id)
            .expect("spawned child must expose a pid");
        wait_for_output(&manager, id, b"SRUI_EXIT_OK");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut reaped = false;
        while std::time::Instant::now() < deadline {
            if is_child_reaped(pid) {
                reaped = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        manager.close(id).unwrap();
        assert!(reaped, "child {pid} was not reaped by natural exit");
    }
    fn is_child_reaped(pid: u32) -> bool {
        #[cfg(unix)]
        {
            let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if res == -1 {
                let err = std::io::Error::last_os_error().raw_os_error();
                return err == Some(libc::ESRCH);
            }
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some(after) = stat.rsplit(')').next() {
                    if let Some(state) = after
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.chars().next())
                    {
                        return state != 'Z';
                    }
                }
            }
            false
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            true
        }
    }
}
