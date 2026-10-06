//! PX-008, the R0 release gate: a frozen source polled by the production refresh
//! loop across seconds of real ticks sends a synchronized client no app UI
//! mutation (D1 §4 invariant 12, §12.1, §12.2, §19.2).
//!
//! The client is real: a `ClientHello` over a connection served by the runtime's
//! own `handle_connection`, exactly as `main` serves every accepted socket. The
//! loop is real: `refresh::poll`, the function `main` spawns for every collecting
//! mode, on a multi-threaded runtime with the real clock, at the shortest
//! interval the binary accepts. Only the source is wrapped, to count the samples
//! the loop takes; every sample is `FakeProcessSource`'s own.
//!
//! What counts is defined by the runtime, not by this test: every message the
//! client receives is classed by `logical_class_for_server_envelope`, the
//! function the session's outbound scheduler classes its own envelopes with
//! (§19.2). An app UI mutation is a `Transaction`, the `Ui` class and the only
//! message that changes a client's replica (§12.1). Control traffic is the
//! `Control` class — `ServerWelcome`, `ServerResumeOk`, `ServerResyncRequired`,
//! `ServerHandshakeRefused` and `ServerEventAck` — which is session management,
//! never a change to the UI, and which §4 invariant 12 allows an idle UI to send.
//! It is counted and reported, not asserted away. Resource and terminal traffic
//! would be application traffic too; none may appear either.
use srui_process_explorer::refresh::{poll, MIN_REFRESH_INTERVAL};
use srui_process_explorer::source::{FakeProcessSource, ProcessSnapshot, ProcessSource};
use srui_process_explorer::{start_from_source, update_title, FIXTURE_TITLE, HEADING, SURFACE};
use srui_protocol::{
    decode_framed, encode_framed, operation, srui_message::Msg, ClientHello, SruiMessage,
};
use srui_sessiond::{
    handle_connection, logical_class_for_server_envelope, LogicalChannelClass, Session,
    CORE_VERSION,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// How much real time the samples taken after synchronization must span. Well
/// over one second, so a figure derived from the wall clock — the freshness
/// line's sample time, say — would have changed at least twice on the way.
const SPAN: Duration = Duration::from_secs(3);

/// The fake source, reporting the wall-clock time of every sample it hands the
/// refresh loop. The snapshot itself is untouched.
struct Counted {
    inner: FakeProcessSource,
    samples: mpsc::UnboundedSender<SystemTime>,
}

impl ProcessSource for Counted {
    fn status_text(&self) -> &str {
        self.inner.status_text()
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let snapshot = self.inner.snapshot();
        // The receiver outlives the loop; a closed channel only means the test
        // has already finished observing.
        let _ = self.samples.send(SystemTime::now());
        snapshot
    }
}

/// Length-delimited SRUI messages (§16) read from the client's end of a
/// connection, each with the number of bytes it occupied on the wire.
struct Frames<R> {
    io: R,
    buffer: Vec<u8>,
}

impl<R: AsyncRead + Unpin> Frames<R> {
    async fn next(&mut self) -> Option<(SruiMessage, usize)> {
        loop {
            if let Some((payload, prefix)) = length_prefix(&self.buffer) {
                let framed = prefix + payload;
                if self.buffer.len() >= framed {
                    let frame: Vec<u8> = self.buffer.drain(..framed).collect();
                    let message = decode_framed::<SruiMessage>(&frame)
                        .expect("every frame the server writes decodes");
                    return Some((message, framed));
                }
            }
            let mut chunk = [0_u8; 64 * 1024];
            match self.io.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
            }
        }
    }
}

/// The payload length a frame's varint prefix declares, and the prefix's own
/// length, once the whole prefix has arrived.
fn length_prefix(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0_usize;
    for (index, byte) in bytes.iter().take(10).enumerate() {
        value |= usize::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

fn kind(message: &SruiMessage) -> &'static str {
    match &message.msg {
        Some(Msg::ServerWelcome(_)) => "ServerWelcome",
        Some(Msg::ServerResumeOk(_)) => "ServerResumeOk",
        Some(Msg::ServerResyncRequired(_)) => "ServerResyncRequired",
        Some(Msg::ServerHandshakeRefused(_)) => "ServerHandshakeRefused",
        Some(Msg::ServerEventAck(_)) => "ServerEventAck",
        Some(Msg::Transaction(_)) => "Transaction",
        Some(Msg::ResourceMetadata(_)) => "ResourceMetadata",
        Some(Msg::ResourceChunk(_)) => "ResourceChunk",
        Some(Msg::TerminalData(_)) => "TerminalData",
        Some(Msg::TerminalResyncRequired(_)) => "TerminalResyncRequired",
        Some(_) => "client-originated message",
        None => "empty envelope",
    }
}

/// The operations a transaction carries, by kind, in order.
fn operation_kinds(message: &SruiMessage) -> Vec<&'static str> {
    let Some(Msg::Transaction(transaction)) = &message.msg else {
        return Vec::new();
    };
    transaction
        .operations
        .iter()
        .map(|operation| match &operation.op {
            Some(operation::Op::SetProperty(_)) => "SET_PROPERTY",
            Some(operation::Op::ClearProperty(_)) => "CLEAR_PROPERTY",
            Some(operation::Op::CreateNode(_)) => "CREATE_NODE",
            Some(operation::Op::CreateModel(_)) => "CREATE_MODEL",
            Some(operation::Op::ModelInsert(_)) => "MODEL_INSERT",
            Some(operation::Op::ModelUpdate(_)) => "MODEL_UPDATE",
            Some(operation::Op::ModelDelete(_)) => "MODEL_DELETE",
            Some(_) => "other",
            None => "empty",
        })
        .collect()
}

fn revisions(message: &SruiMessage) -> Option<(u64, u64)> {
    match &message.msg {
        Some(Msg::Transaction(transaction)) => {
            Some((transaction.base_revision, transaction.new_revision))
        }
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frozen_fake_source_sends_a_synchronized_client_no_app_ui_mutation_across_seconds_of_ticks(
) {
    let session = Arc::new(Session::mint());
    let (samples, mut sampled) = mpsc::unbounded_channel();
    let mut source = Counted {
        inner: FakeProcessSource,
        samples,
    };
    // As `main` does: publish the first snapshot, then poll the same source.
    let (view, _) = start_from_source(&session, &mut source).expect("the fake source publishes");
    let shutdown = CancellationToken::new();
    let polling = tokio::spawn(poll(
        view,
        source,
        Arc::clone(&session),
        MIN_REFRESH_INTERVAL,
        shutdown.child_token(),
    ));

    // A fresh client, served by the runtime's own connection handler.
    let (client, server) = tokio::io::duplex(1 << 20);
    let serving = tokio::spawn(handle_connection(
        server,
        Arc::clone(&session),
        shutdown.child_token(),
    ));
    let (read, mut write) = tokio::io::split(client);
    let hello = SruiMessage {
        msg: Some(Msg::ClientHello(ClientHello {
            core_version: CORE_VERSION.to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            client_instance_id: vec![0x50, 0x58, 0x08],
            ..ClientHello::default()
        })),
    };
    write
        .write_all(&encode_framed(&hello).expect("a hello encodes"))
        .await
        .expect("the hello is written");
    let mut frames = Frames {
        io: read,
        buffer: Vec::new(),
    };

    let observed = tokio::time::timeout(Duration::from_secs(60), async {
        // Synchronized: the welcome, then the catch-up snapshot that brings the
        // replica to the session's revision.
        let (welcome, _) = frames.next().await.expect("the session answers");
        assert_eq!(kind(&welcome), "ServerWelcome");
        let synchronized = loop {
            let (message, _) = frames.next().await.expect("the snapshot arrives");
            if let Some((0, revision)) = revisions(&message) {
                break revision;
            }
        };
        assert_eq!(synchronized, session.current_revision());

        // Everything the client receives from here on, in order, on its own task.
        let (received, mut inbox) = mpsc::unbounded_channel();
        let reading = tokio::spawn(async move {
            while let Some(frame) = frames.next().await {
                if received.send(frame).is_err() {
                    break;
                }
            }
        });

        // Let the production loop sample the frozen source for well over a
        // second of real time: no sleep, the loop's own samples are the clock.
        while sampled.try_recv().is_ok() {}
        let first = sampled.recv().await.expect("the loop samples");
        let mut ticks = 1_usize;
        let span = loop {
            let at = sampled.recv().await.expect("the loop keeps sampling");
            ticks += 1;
            let span = at.duration_since(first).unwrap_or_default();
            if span >= SPAN {
                break span;
            }
        };
        assert_eq!(
            session.current_revision(),
            synchronized,
            "a frozen source committed a transaction during {ticks} ticks"
        );

        // A canary closes the observation: one known mutation, committed after
        // the window. The connection delivers transactions in commit order, so
        // every message that reaches the client before the canary was sent
        // during the window, and the canary proves the client was still
        // receiving.
        update_title(&session, FIXTURE_TITLE).expect("the canary commits");
        let mut before_canary = Vec::new();
        let canary = loop {
            let (message, bytes) = inbox.recv().await.expect("the canary arrives");
            if revisions(&message).is_some() {
                break (message, bytes);
            }
            before_canary.push((message, bytes));
        };
        shutdown.cancel();
        reading.abort();
        (synchronized, ticks, span, before_canary, canary)
    })
    .await
    .expect("the observation finishes");
    let _ = polling.await;
    let _ = serving.await;
    drop(write);

    let (synchronized, ticks, span, before_canary, (canary, canary_bytes)) = observed;
    // No app UI mutation, and nothing else that is not control traffic, reached
    // the synchronized client in the window.
    let application: Vec<String> = before_canary
        .iter()
        .filter(|(message, _)| {
            logical_class_for_server_envelope(message) != Some(LogicalChannelClass::Control)
        })
        .map(|(message, bytes)| format!("{} ({bytes} B)", kind(message)))
        .collect();
    assert!(
        application.is_empty(),
        "application traffic after synchronization: {application:?}"
    );
    let control: Vec<String> = before_canary
        .iter()
        .map(|(message, bytes)| format!("{} ({bytes} B)", kind(message)))
        .collect();
    // The first transaction after synchronization is the canary itself: built
    // on the synchronized revision, two scalar properties, nothing in between.
    assert_eq!(
        logical_class_for_server_envelope(&canary),
        Some(LogicalChannelClass::Ui)
    );
    assert_eq!(
        revisions(&canary),
        Some((synchronized, synchronized + 1)),
        "the canary is the first transaction after synchronization"
    );
    assert_eq!(operation_kinds(&canary), ["SET_PROPERTY", "SET_PROPERTY"]);
    let Some(Msg::Transaction(transaction)) = &canary.msg else {
        unreachable!("the canary is a transaction");
    };
    let targets: Vec<u64> = transaction
        .operations
        .iter()
        .filter_map(|operation| match &operation.op {
            Some(operation::Op::SetProperty(set)) => Some(set.node_id),
            _ => None,
        })
        .collect();
    assert_eq!(targets, [SURFACE.get(), HEADING.get()]);
    assert_eq!(session.current_revision(), synchronized + 1);
    assert!(ticks >= 2 && span >= SPAN);
    println!(
        "PX-008 frozen-source evidence: interval={}ms ticks_after_sync={ticks} \
         span={:.3}s app_ui_mutations_after_sync=0 control_after_sync={} {control:?} \
         canary=Transaction {}->{} {:?} ({canary_bytes} B)",
        MIN_REFRESH_INTERVAL.as_millis(),
        span.as_secs_f64(),
        control.len(),
        synchronized,
        synchronized + 1,
        operation_kinds(&canary),
    );
}
