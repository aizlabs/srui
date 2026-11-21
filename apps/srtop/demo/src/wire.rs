//! A minimal replica client of one srtop session, over its socket or a
//! [`crate::relay::DelayRelay`] in front of it (PX-025).
//!
//! It speaks the runtime's protocol exactly as `apps/srtop/tests/continuity_test.rs`
//! does: `CLIENT_HELLO` or `CLIENT_RESUME`, one snapshot envelope, then every
//! commit applied to a [`SemanticStore`] as it arrives (§§12.1, 17–18). It
//! sends standard `TEXT_EDIT` events and nothing of its own.

use srui_process_explorer::MODEL;
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, ClientResume, SessionContinuity,
    SruiMessage, DEFAULT_MAX_FRAME_SIZE,
};
use srui_semantic_tree::{
    EditSeq, Event as DomainEvent, NodeId, PropertyRef, ResyncSnapshot, SemanticStore, Value,
};
use srui_sessiond::CORE_VERSION;
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

const PROFILE: &str = "org.srui.standard-widgets/1";

/// The length of the first frame in `buffer` (its varint prefix plus its
/// payload), once `buffer` holds all of it: `Ok(None)` while more is needed.
/// A declared payload longer than `max_frame` bytes (§26), a prefix longer
/// than ten bytes or one that does not fit 64 bits is an error at once, so a
/// hostile length is never buffered, and no sum can overflow.
pub fn frame_extent(buffer: &[u8], max_frame: usize) -> Result<Option<usize>> {
    let mut length: u64 = 0;
    for (index, byte) in buffer.iter().take(10).enumerate() {
        let bits = u64::from(byte & 0x7f);
        let shift = 7 * index as u32;
        // The tenth byte may carry only the 64th bit.
        if index == 9 && bits > 1 {
            return Err("a frame length prefix that does not fit 64 bits".into());
        }
        length |= bits << shift;
        if byte & 0x80 == 0 {
            let payload = usize::try_from(length)
                .ok()
                .filter(|payload| *payload <= max_frame)
                .ok_or_else(|| {
                    format!("a frame of {length} bytes exceeds the {max_frame}-byte limit (§26)")
                })?;
            let extent = (index + 1)
                .checked_add(payload)
                .ok_or("a frame length that overflows")?;
            return Ok((buffer.len() >= extent).then_some(extent));
        }
    }
    if buffer.len() >= 10 {
        return Err("a frame length prefix longer than ten bytes".into());
    }
    Ok(None)
}

/// How a server answered a `CLIENT_RESUME` (§18).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    ResumeOk { replay_from: u64 },
    Resync { continuity: SessionContinuity },
}

/// One replica of an srtop session.
pub struct Replica {
    stream: UnixStream,
    instance: Vec<u8>,
    buffer: Vec<u8>,
    pub store: SemanticStore,
    pub session_id: String,
    last_acked: u64,
    next_seq: u64,
    next_edit: u64,
    /// Every byte read from the server since the last attach.
    pub received: u64,
}

fn connect(socket: &Path) -> Result<UnixStream> {
    let stream = UnixStream::connect(socket)
        .map_err(|error| format!("connect {}: {error}", socket.display()))?;
    // Short reads made again until READ_DEADLINE: a blocked macOS AF_UNIX
    // reader can sleep through its peer's end of file (`relay::READ_POLL`).
    stream
        .set_read_timeout(Some(crate::relay::READ_POLL))
        .map_err(|error| error.to_string())?;
    Ok(stream)
}

/// The longest one read waits for the server.
const READ_DEADLINE: Duration = Duration::from_secs(15);

impl Replica {
    /// A fresh client: `CLIENT_HELLO`, then the welcome and its snapshot.
    pub fn hello(socket: &Path, instance: &[u8]) -> Result<Self> {
        let mut replica = Self {
            stream: connect(socket)?,
            instance: instance.to_vec(),
            buffer: Vec::new(),
            store: SemanticStore::new(),
            session_id: String::new(),
            last_acked: 0,
            next_seq: 1,
            next_edit: 1,
            received: 0,
        };
        replica.send(&SruiMessage {
            msg: Some(Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec![PROFILE.to_string()],
                client_instance_id: replica.instance.clone(),
                ..ClientHello::default()
            })),
        })?;
        let welcome = loop {
            if let Some(Msg::ServerWelcome(welcome)) = replica.next_message()?.msg {
                break welcome;
            }
        };
        replica.session_id = welcome.session_id;
        replica.take_snapshot(welcome.initial_revision)?;
        Ok(replica)
    }

    /// The warm reconnect of this client over a new transport: `CLIENT_RESUME`
    /// from the replica's revision and acknowledged frontier (§18).
    pub fn resume(&mut self, socket: &Path) -> Result<Decision> {
        self.stream = connect(socket)?;
        self.buffer.clear();
        self.received = 0;
        self.send(&SruiMessage {
            msg: Some(Msg::ClientResume(ClientResume {
                session_id: self.session_id.clone(),
                client_instance_id: self.instance.clone(),
                last_applied_revision: self.revision(),
                last_acked_event_seq: self.last_acked,
                core_version: CORE_VERSION.to_string(),
                profiles: vec![PROFILE.to_string()],
                ..ClientResume::default()
            })),
        })?;
        loop {
            match self.next_message()?.msg {
                Some(Msg::ServerResumeOk(ok)) => {
                    return Ok(Decision::ResumeOk {
                        replay_from: ok.replay_from_revision,
                    })
                }
                Some(Msg::ServerResyncRequired(resync)) => {
                    let continuity = SessionContinuity::try_from(resync.continuity)
                        .map_err(|_| "an unknown continuity".to_string())?;
                    if continuity == SessionContinuity::Replaced {
                        self.store = SemanticStore::new();
                        self.last_acked = 0;
                        self.next_seq = 1;
                        self.session_id = resync.session_id.clone();
                    }
                    self.take_snapshot(resync.snapshot_revision)?;
                    return Ok(Decision::Resync { continuity });
                }
                _ => {}
            }
        }
    }

    fn take_snapshot(&mut self, revision: u64) -> Result<()> {
        let wire = loop {
            if let Some(Msg::Transaction(transaction)) = self.next_raw()?.msg {
                break transaction;
            }
        };
        if wire.base_revision != 0 || wire.new_revision != revision {
            return Err(format!(
                "expected the snapshot at {revision}, got {} -> {}",
                wire.base_revision, wire.new_revision
            ));
        }
        let snapshot = ResyncSnapshot::try_from(wire).map_err(|error| error.to_string())?;
        self.store
            .replace_from_snapshot(&snapshot)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn revision(&self) -> u64 {
        self.store.revision().get()
    }

    /// Loses the transport without telling the session (§17).
    pub fn lose_transport(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    pub fn send(&mut self, message: &SruiMessage) -> Result<()> {
        let bytes = encode_framed(message).map_err(|error| error.to_string())?;
        self.stream
            .write_all(&bytes)
            .map_err(|error| format!("send: {error}"))
    }

    /// The next message, with every commit applied as it arrives. Event
    /// acknowledgements advance the acknowledged frontier.
    pub fn next_message(&mut self) -> Result<SruiMessage> {
        let message = self.next_raw()?;
        match &message.msg {
            Some(Msg::Transaction(transaction)) => {
                let (base, new) = (transaction.base_revision, transaction.new_revision);
                self.store
                    .apply_delivered_transaction(transaction.clone())
                    .map_err(|error| format!("commit {base} -> {new} did not apply: {error}"))?;
            }
            Some(Msg::ServerEventAck(ack)) => {
                self.last_acked = self.last_acked.max(ack.last_processed_event_seq);
            }
            _ => {}
        }
        Ok(message)
    }

    /// The next framed message, not applied. A frame whose declared length
    /// exceeds the §26 limit is refused before any of it is buffered.
    pub fn next_raw(&mut self) -> Result<SruiMessage> {
        loop {
            if let Some(extent) = frame_extent(&self.buffer, DEFAULT_MAX_FRAME_SIZE)? {
                let frame: Vec<u8> = self.buffer.drain(..extent).collect();
                return decode_framed(&frame).map_err(|error| error.to_string());
            }
            let mut chunk = [0_u8; 64 * 1024];
            let deadline = Instant::now() + READ_DEADLINE;
            let read = loop {
                match self.stream.read(&mut chunk) {
                    Ok(read) => break read,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) && Instant::now() < deadline => {}
                    Err(error) => return Err(format!("read: {error}")),
                }
            };
            if read == 0 {
                return Err("the server closed the connection".into());
            }
            self.received += read as u64;
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// Reads and applies messages until `done` holds, for at most `timeout`.
    pub fn until(
        &mut self,
        what: &str,
        timeout: Duration,
        done: impl Fn(&Self) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !done(self) {
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting for {what}"));
            }
            self.next_message()?;
        }
        Ok(())
    }

    /// Sends one standard `TEXT_EDIT` carrying the whole text of `node` and
    /// returns its event sequence; it does not wait for anything.
    pub fn send_text_edit(&mut self, node: NodeId, text: &str) -> Result<u64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let edit = EditSeq::new(self.next_edit).ok_or("edit sequence overflow")?;
        self.next_edit += 1;
        let event = DomainEvent::text_edit(
            seq,
            format!("px025-edit-{seq}"),
            self.revision(),
            node.get(),
            text,
            edit,
        )
        .with_client_instance_id(self.instance.clone())
        .to_wire();
        self.send(&SruiMessage {
            msg: Some(Msg::Event(event)),
        })?;
        Ok(seq)
    }

    /// The `TEXT` of `node`, if it has one.
    pub fn text(&self, node: NodeId) -> Option<String> {
        match self
            .store
            .get_node(node)
            .and_then(|node| node.get_property(PropertyRef::TEXT))
        {
            Some(Value::String(text)) => Some(text.clone()),
            _ => None,
        }
    }

    /// The process table's rows as text cells, in order.
    pub fn rows(&self) -> Vec<Vec<String>> {
        let Some(model) = self.store.get_model(MODEL) else {
            return Vec::new();
        };
        (0..model.item_count)
            .filter_map(|index| model.get_item_by_index(index))
            .map(|item| match &item.value {
                Value::List(cells) => cells
                    .iter()
                    .map(|cell| match cell {
                        Value::String(text) => text.clone(),
                        Value::UnsignedInt(number) => number.to_string(),
                        Value::SignedInt(number) => number.to_string(),
                        other => format!("{other:?}"),
                    })
                    .collect(),
                other => vec![format!("{other:?}")],
            })
            .collect()
    }

    /// The table's column headings.
    pub fn headings(&self) -> Vec<String> {
        match self
            .store
            .get_node(srui_process_explorer::TABLE)
            .and_then(|node| node.get_property(PropertyRef::COLUMNS))
        {
            Some(Value::List(headings)) => headings
                .iter()
                .map(|heading| match heading {
                    Value::String(text) => text.clone(),
                    other => format!("{other:?}"),
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}
