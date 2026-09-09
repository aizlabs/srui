//! One PTY stream: process, command worker, output ring, and subscribers (§21).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use srui_protocol::{
    TerminalData, TerminalResyncReason, TerminalResyncRequired, MAX_TERMINAL_COLUMNS,
    MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_OUTPUT_FRAME_BYTES, MAX_TERMINAL_PIXEL_DIMENSION,
    MAX_TERMINAL_ROWS,
};
use srui_semantic_tree::NodeId;
use thiserror::Error;
use tokio::sync::{mpsc, watch};

use crate::ring::{OutputRing, RingError};
use crate::spec::TerminalSpec;

const COMMAND_QUEUE_CAPACITY: usize = 64;
const PTY_READ_CHUNK: usize = 8192;

/// Serialized per-stream commands. Order is preserved by a single worker.
#[derive(Debug)]
pub enum StreamCommand {
    Input(Vec<u8>),
    Resize {
        columns: u32,
        rows: u32,
        pixel_width: u32,
        pixel_height: u32,
    },
    Close,
}

/// Events a cursor-based subscription may observe.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent {
    Data(TerminalData),
    Resync(TerminalResyncRequired),
}

#[derive(Debug, Error)]
pub enum TerminalStreamError {
    #[error("invalid terminal specification: {0}")]
    InvalidSpec(String),
    #[error("invalid terminal dimensions")]
    InvalidDimensions,
    #[error("terminal input exceeds {MAX_TERMINAL_INPUT_BYTES} bytes")]
    InputTooLarge,
    #[error("empty terminal input is forbidden")]
    EmptyInput,
    #[error("PTY command queue is full")]
    CommandQueueFull,
    #[error("PTY stream is closed")]
    Closed,
    #[error("failed to open PTY: {0}")]
    Pty(String),
    #[error("ring error: {0}")]
    Ring(#[from] RingError),
}

type ChildSlot = Arc<Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>>;
type CommandSender = Arc<Mutex<Option<mpsc::Sender<StreamCommand>>>>;

pub(crate) struct TerminalStream {
    pub id: NodeId,
    ring: Arc<Mutex<OutputRing>>,
    next_offset_watch: watch::Sender<u64>,
    command_tx: CommandSender,
    child: ChildSlot,
    child_pid: Arc<Mutex<Option<u32>>>,
    exit_success: Arc<Mutex<Option<bool>>>,
    reader_thread: Mutex<Option<JoinHandle<()>>>,
    command_thread: Mutex<Option<JoinHandle<()>>>,
}

impl TerminalStream {
    pub(crate) fn spawn(id: NodeId, spec: TerminalSpec) -> Result<Arc<Self>, TerminalStreamError> {
        spec.validate().map_err(TerminalStreamError::InvalidSpec)?;
        let size = pty_size(spec.columns, spec.rows, 0, 0)?;
        let system = NativePtySystem::default();
        static PTY_OPEN_LOCK: Mutex<()> = Mutex::new(());
        let _open_guard = PTY_OPEN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut pair = None;
        let mut last_err = None;
        for _ in 0..5 {
            match system.openpty(size) {
                Ok(p) => {
                    pair = Some(p);
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            }
        }
        let pair = pair.ok_or_else(|| {
            TerminalStreamError::Pty(last_err.map(|e| e.to_string()).unwrap_or_default())
        })?;

        let mut builder = CommandBuilder::new(spec.executable.as_os_str());
        for arg in &spec.args {
            builder.arg(arg);
        }
        if let Some(cwd) = &spec.working_directory {
            builder.cwd(cwd);
        }
        for (key, value) in spec.environment_with_defaults() {
            builder.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| TerminalStreamError::Pty(error.to_string()))?;
        drop(pair.slave);
        let child_pid = child.process_id();
        // portable-pty already calls setsid() in the child; this is a parent-side
        // best-effort so descendants forked after spawn inherit a stable pgid.
        become_process_group_leader(child_pid);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalStreamError::Pty(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalStreamError::Pty(error.to_string()))?;
        let master = pair.master;
        #[cfg(unix)]
        let child_pid = master
            .process_group_leader()
            .map(|pgid| pgid as u32)
            .or(child_pid);

        let child_pid = Arc::new(Mutex::new(child_pid));
        let exit_success = Arc::new(Mutex::new(None));
        let ring = Arc::new(Mutex::new(OutputRing::new(spec.ring_capacity)));
        let (next_offset_watch, _) = watch::channel(0_u64);
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let command_tx = Arc::new(Mutex::new(Some(command_tx)));
        let child = Arc::new(Mutex::new(Some(child)));

        let stream = Arc::new(Self {
            id,
            ring: Arc::clone(&ring),
            next_offset_watch: next_offset_watch.clone(),
            command_tx: Arc::clone(&command_tx),
            child: Arc::clone(&child),
            child_pid: Arc::clone(&child_pid),
            exit_success: Arc::clone(&exit_success),
            reader_thread: Mutex::new(None),
            command_thread: Mutex::new(None),
        });

        let reader_ring = Arc::clone(&ring);
        let reader_watch = next_offset_watch;
        let reader_child = Arc::clone(&child);
        let reader_child_pid = Arc::clone(&child_pid);
        let reader_exit_success = Arc::clone(&exit_success);
        let reader = thread::Builder::new()
            .name(format!("srui-pty-read-{}", id.get()))
            .spawn(move || {
                read_loop(
                    reader,
                    reader_ring,
                    reader_watch,
                    reader_child,
                    reader_child_pid,
                    reader_exit_success,
                    command_tx,
                )
            })
            .map_err(|error| TerminalStreamError::Pty(error.to_string()))?;
        *stream.reader_thread.lock().expect("reader thread slot") = Some(reader);

        let command = thread::Builder::new()
            .name(format!("srui-pty-cmd-{}", id.get()))
            .spawn(move || command_loop(command_rx, writer, master))
            .map_err(|error| TerminalStreamError::Pty(error.to_string()))?;
        *stream.command_thread.lock().expect("command thread slot") = Some(command);

        Ok(stream)
    }

    pub(crate) fn try_enqueue(&self, command: StreamCommand) -> Result<(), TerminalStreamError> {
        match &command {
            StreamCommand::Input(data) => {
                if data.is_empty() {
                    return Err(TerminalStreamError::EmptyInput);
                }
                if data.len() > MAX_TERMINAL_INPUT_BYTES {
                    return Err(TerminalStreamError::InputTooLarge);
                }
            }
            StreamCommand::Resize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            } => {
                validate_dimensions(*columns, *rows, *pixel_width, *pixel_height)?;
            }
            StreamCommand::Close => {}
        }
        let tx = self.command_tx.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tx) = tx.as_ref() else {
            return Err(TerminalStreamError::Closed);
        };
        tx.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => TerminalStreamError::CommandQueueFull,
            mpsc::error::TrySendError::Closed(_) => TerminalStreamError::Closed,
        })
    }

    #[cfg(test)]
    pub(crate) fn process_id(&self) -> Option<u32> {
        *self.child_pid.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn snapshot_offsets(&self) -> (u64, u64) {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        (ring.retained_start(), ring.next_offset())
    }

    pub(crate) fn exit_success(&self) -> Option<bool> {
        *self
            .exit_success
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn subscribe(
        self: &Arc<Self>,
        requested_offset: u64,
    ) -> (SubscribeSnapshot, crate::manager::TerminalSubscription) {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let retained_start = ring.retained_start();
        let cut = ring.next_offset();
        let snapshot = if requested_offset > cut {
            SubscribeSnapshot::Resync {
                requested_offset,
                retained_from_offset: retained_start,
                resume_at_offset: cut,
                reason: TerminalResyncReason::OffsetAhead,
            }
        } else if requested_offset < retained_start {
            SubscribeSnapshot::Resync {
                requested_offset,
                retained_from_offset: retained_start,
                resume_at_offset: cut,
                reason: TerminalResyncReason::RetentionLoss,
            }
        } else {
            let frames = ring
                .frame_range(requested_offset, cut)
                .unwrap_or_default()
                .into_iter()
                .map(|(byte_offset, data)| TerminalData {
                    stream_id: self.id.get(),
                    byte_offset,
                    data,
                })
                .collect();
            SubscribeSnapshot::Replay { frames }
        };
        drop(ring);

        let cursor = match &snapshot {
            SubscribeSnapshot::Replay { .. } => cut,
            SubscribeSnapshot::Resync {
                resume_at_offset, ..
            } => *resume_at_offset,
        };
        let subscription = crate::manager::TerminalSubscription {
            stream_id: self.id,
            ring: Arc::clone(&self.ring),
            watch: self.next_offset_watch.subscribe(),
            cursor,
        };
        (snapshot, subscription)
    }

    pub(crate) fn kill_and_reap(&self) {
        let run_teardown = || {
            // Drop the sender so `command_loop` unblocks even when the queue is full
            // and `Close` cannot be enqueued.
            drop(
                self.command_tx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take(),
            );
            let pid = self
                .child_pid
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            signal_child_tree(pid);
            if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = child.kill();
                if let Ok(status) = child.wait() {
                    *self
                        .exit_success
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(status.success());
                }
            }
            // Command thread owns the master PTY. Join it first so dropping the master
            // forces EOF/EIO on the reader if any leftover slave holders remain.
            if let Some(handle) = self
                .command_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = handle.join();
            }
            if let Some(handle) = self
                .reader_thread
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                let _ = handle.join();
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                tokio::task::block_in_place(run_teardown);
                return;
            }
        }
        run_teardown();
    }
}

#[derive(Debug)]
pub enum SubscribeSnapshot {
    Replay {
        frames: Vec<TerminalData>,
    },
    Resync {
        requested_offset: u64,
        retained_from_offset: u64,
        resume_at_offset: u64,
        reason: TerminalResyncReason,
    },
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    ring: Arc<Mutex<OutputRing>>,
    watch: watch::Sender<u64>,
    child: ChildSlot,
    child_pid: Arc<Mutex<Option<u32>>>,
    exit_success: Arc<Mutex<Option<bool>>>,
    command_tx: CommandSender,
) {
    let mut buf = vec![0_u8; PTY_READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut guard = ring.lock().unwrap_or_else(|e| e.into_inner());
                if guard.append(&buf[..n]).is_err() {
                    break;
                }
                let next = guard.next_offset();
                drop(guard);
                let _ = watch.send(next);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if let Some(mut child) = child.lock().unwrap_or_else(|e| e.into_inner()).take() {
        if let Ok(status) = child.wait() {
            *exit_success
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(status.success());
        }
        *child_pid.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    // Natural exit: close the command worker so it does not sit on blocking_recv
    // for the rest of the session.
    drop(command_tx.lock().unwrap_or_else(|e| e.into_inner()).take());
}

fn become_process_group_leader(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // Best-effort: descendants spawned after this inherit the new pgid.
        let _ = unsafe { libc::setpgid(pid as libc::pid_t, pid as libc::pid_t) };
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn signal_child_tree(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        let pid = pid as libc::pid_t;
        unsafe {
            let _ = libc::kill(-pid, libc::SIGHUP);
            let _ = libc::kill(-pid, libc::SIGKILL);
            let _ = libc::kill(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn command_loop(
    mut commands: mpsc::Receiver<StreamCommand>,
    mut writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
) {
    while let Some(command) = commands.blocking_recv() {
        match command {
            StreamCommand::Input(data) => {
                if writer.write_all(&data).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
            StreamCommand::Resize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            } => {
                if let Ok(size) = pty_size(columns, rows, pixel_width, pixel_height) {
                    let _ = master.resize(size);
                }
            }
            StreamCommand::Close => break,
        }
    }
}

pub(crate) fn validate_dimensions(
    columns: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
) -> Result<(), TerminalStreamError> {
    if columns == 0 || columns > MAX_TERMINAL_COLUMNS {
        return Err(TerminalStreamError::InvalidDimensions);
    }
    if rows == 0 || rows > MAX_TERMINAL_ROWS {
        return Err(TerminalStreamError::InvalidDimensions);
    }
    if pixel_width > MAX_TERMINAL_PIXEL_DIMENSION || pixel_height > MAX_TERMINAL_PIXEL_DIMENSION {
        return Err(TerminalStreamError::InvalidDimensions);
    }
    Ok(())
}

fn pty_size(
    columns: u32,
    rows: u32,
    pixel_width: u32,
    pixel_height: u32,
) -> Result<PtySize, TerminalStreamError> {
    validate_dimensions(columns, rows, pixel_width, pixel_height)?;
    Ok(PtySize {
        rows: u16::try_from(rows).map_err(|_| TerminalStreamError::InvalidDimensions)?,
        cols: u16::try_from(columns).map_err(|_| TerminalStreamError::InvalidDimensions)?,
        pixel_width: u16::try_from(pixel_width)
            .map_err(|_| TerminalStreamError::InvalidDimensions)?,
        pixel_height: u16::try_from(pixel_height)
            .map_err(|_| TerminalStreamError::InvalidDimensions)?,
    })
}

pub(crate) fn live_events_from_ring(
    stream_id: NodeId,
    ring: &OutputRing,
    cursor: &mut u64,
) -> Vec<TerminalEvent> {
    let mut events = Vec::new();
    if *cursor < ring.retained_start() {
        let resume = ring.next_offset();
        events.push(TerminalEvent::Resync(TerminalResyncRequired {
            stream_id: stream_id.get(),
            requested_offset: *cursor,
            retained_from_offset: ring.retained_start(),
            resume_at_offset: resume,
            reason: TerminalResyncReason::SubscriberFallbehind as i32,
        }));
        *cursor = resume;
        return events;
    }
    if *cursor >= ring.next_offset() {
        return events;
    }
    match ring.frame_range(*cursor, ring.next_offset()) {
        Ok(frames) => {
            for (byte_offset, data) in frames {
                if data.is_empty() {
                    continue;
                }
                let len = data.len() as u64;
                events.push(TerminalEvent::Data(TerminalData {
                    stream_id: stream_id.get(),
                    byte_offset,
                    data,
                }));
                *cursor = byte_offset.saturating_add(len);
            }
        }
        Err(_) => {
            let resume = ring.next_offset();
            events.push(TerminalEvent::Resync(TerminalResyncRequired {
                stream_id: stream_id.get(),
                requested_offset: *cursor,
                retained_from_offset: ring.retained_start(),
                resume_at_offset: resume,
                reason: TerminalResyncReason::SubscriberFallbehind as i32,
            }));
            *cursor = resume;
        }
    }
    let _ = MAX_TERMINAL_OUTPUT_FRAME_BYTES;
    events
}
