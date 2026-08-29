//! Wire-behaviour coverage over the real `handle_connection` path (§16, §18, §23, §26).

mod common;

use std::sync::Arc;

use common::{base_fixture, base_processes, MIB, UID};
use srui_example_process_monitor::testing::{record, snapshot};
use srui_example_process_monitor::*;
use srui_protocol::{
    decode_framed, encode_framed, operation::Op, srui_message, ClientHello, SruiMessage,
    Transaction,
};
use srui_sessiond::{handle_connection, Session};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio_util::sync::CancellationToken;

/// Reads one length-delimited SRUI frame, returning its total framed byte count and payload.
async fn read_frame(stream: &mut DuplexStream) -> (usize, SruiMessage) {
    let mut frame = Vec::new();
    let mut payload_len: u64 = 0;
    let mut shift = 0;
    loop {
        let byte = stream.read_u8().await.expect("read length prefix byte");
        frame.push(byte);
        payload_len |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    let mut payload = vec![0u8; payload_len as usize];
    stream.read_exact(&mut payload).await.expect("read payload");
    frame.extend_from_slice(&payload);

    let message: SruiMessage = decode_framed(&frame).expect("decode framed message");
    (frame.len(), message)
}

fn expect_transaction(message: SruiMessage) -> Transaction {
    match message.msg {
        Some(srui_message::Msg::Transaction(transaction)) => transaction,
        other => panic!("expected Transaction, got {other:?}"),
    }
}

fn wire_kinds(transaction: &Transaction) -> Vec<&'static str> {
    transaction
        .operations
        .iter()
        .map(|operation| match operation.op {
            Some(Op::CreateNode(_)) => "CREATE_NODE",
            Some(Op::DeleteNode(_)) => "DELETE_NODE",
            Some(Op::SetProperty(_)) => "SET_PROPERTY",
            Some(Op::ClearProperty(_)) => "CLEAR_PROPERTY",
            Some(Op::Commit(_)) => "COMMIT",
            Some(Op::CreateModel(_)) => "CREATE_MODEL",
            Some(Op::ModelInsert(_)) => "MODEL_INSERT",
            Some(Op::ModelDelete(_)) => "MODEL_DELETE",
            Some(Op::ModelUpdate(_)) => "MODEL_UPDATE",
            Some(Op::ModelResetRange(_)) => "MODEL_RESET_RANGE",
            Some(Op::MoveNode(_)) => "MOVE_NODE",
            Some(Op::ReorderChildren(_)) => "REORDER_CHILDREN",
            Some(Op::BatchPropertySet(_)) => "BATCH_PROPERTY_SET",
            None => "EMPTY",
        })
        .collect()
}

async fn connect(session: Arc<Session>, shutdown: CancellationToken) -> DuplexStream {
    let (mut client_io, server_io) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(async move {
        let _ = handle_connection(server_io, session, shutdown).await;
    });

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![9, 9, 9, 9],
            client_metadata: Default::default(),
        })),
    };
    client_io
        .write_all(&encode_framed(&hello).expect("encode hello"))
        .await
        .expect("send hello");
    client_io
}

#[tokio::test]
async fn fresh_hello_receives_welcome_then_the_complete_process_monitor_snapshot() {
    let fixture = base_fixture();
    let shutdown = CancellationToken::new();
    let mut client = connect(fixture.session.clone(), shutdown.clone()).await;

    let (_, welcome) = read_frame(&mut client).await;
    assert!(
        matches!(welcome.msg, Some(srui_message::Msg::ServerWelcome(_))),
        "first frame must be ServerWelcome"
    );

    let (_, snapshot_message) = read_frame(&mut client).await;
    let snapshot_transaction = expect_transaction(snapshot_message);
    let kinds = wire_kinds(&snapshot_transaction);

    assert_eq!(
        kinds.iter().filter(|kind| **kind == "CREATE_NODE").count(),
        10,
        "snapshot carries the full node tree"
    );
    assert_eq!(
        kinds.iter().filter(|kind| **kind == "CREATE_MODEL").count(),
        1,
        "snapshot carries the process model"
    );
    assert!(
        kinds
            .iter()
            .any(|kind| *kind == "MODEL_INSERT" || *kind == "MODEL_RESET_RANGE"),
        "snapshot carries the model items, got {kinds:?}"
    );

    shutdown.cancel();
}

#[tokio::test]
async fn wire_statistics_use_the_same_framing_path_as_the_live_connection() {
    let fixture = base_fixture();
    let shutdown = CancellationToken::new();
    let mut client = connect(fixture.session.clone(), shutdown.clone()).await;

    let (_, _welcome) = read_frame(&mut client).await;
    let (snapshot_bytes, snapshot_message) = read_frame(&mut client).await;
    let snapshot_transaction = expect_transaction(snapshot_message);

    let stats = measure_transaction(&snapshot_transaction);
    assert_eq!(
        stats.framed_bytes, snapshot_bytes,
        "wire statistics must measure the exact frame the client receives"
    );
    assert_eq!(stats.operations, snapshot_transaction.operations.len());

    shutdown.cancel();
}

#[tokio::test]
async fn a_quiet_tick_is_materially_smaller_than_the_initial_snapshot() {
    let fixture = base_fixture();
    let shutdown = CancellationToken::new();
    let mut client = connect(fixture.session.clone(), shutdown.clone()).await;

    let (_, _welcome) = read_frame(&mut client).await;
    let (snapshot_bytes, snapshot_message) = read_frame(&mut client).await;
    let snapshot_stats = measure_transaction(&expect_transaction(snapshot_message));

    // A quiet tick: global CPU moved, one process changed, nothing appeared or exited.
    let mut processes = base_processes();
    processes[1].cpu_percent = 9.5;
    fixture.source.publish(snapshot(31.0, processes));
    let monitor = fixture.monitor.clone();
    tokio::task::spawn_blocking(move || monitor.tick())
        .await
        .expect("join tick")
        .expect("tick commits");

    let (tick_bytes, tick_message) = read_frame(&mut client).await;
    let tick_transaction = expect_transaction(tick_message);
    let tick_stats = measure_transaction(&tick_transaction);

    assert_eq!(tick_stats.framed_bytes, tick_bytes);
    assert!(
        tick_bytes * 4 < snapshot_bytes,
        "steady-state tick ({tick_bytes} B) must be materially smaller than the snapshot ({snapshot_bytes} B)"
    );
    assert_eq!(tick_stats.model_update, 1);
    assert_eq!(tick_stats.set_property, 2);
    assert_eq!(tick_stats.other, 0);
    println!("snapshot={snapshot_stats} tick={tick_stats}");

    shutdown.cancel();
}

#[tokio::test]
async fn tick_transactions_contain_only_allowed_property_and_model_operations() {
    let fixture = base_fixture();
    let shutdown = CancellationToken::new();
    let mut client = connect(fixture.session.clone(), shutdown.clone()).await;

    let (_, _welcome) = read_frame(&mut client).await;
    let (_, _snapshot) = read_frame(&mut client).await;

    // Insert, update and delete in a single sample.
    let mut processes = base_processes();
    processes.retain(|process| process.key.pid != 10);
    processes[0].cpu_percent = 51.0;
    processes.push(record(90, 4_000, "newcomer", 1.0, MIB, Some(UID)));
    fixture.source.publish(snapshot(44.0, processes));
    let monitor = fixture.monitor.clone();
    tokio::task::spawn_blocking(move || monitor.tick())
        .await
        .expect("join tick")
        .expect("tick commits");

    let (_, tick_message) = read_frame(&mut client).await;
    let tick_transaction = expect_transaction(tick_message);
    for kind in wire_kinds(&tick_transaction) {
        assert!(
            matches!(
                kind,
                "SET_PROPERTY" | "MODEL_INSERT" | "MODEL_UPDATE" | "MODEL_DELETE"
            ),
            "forbidden steady-state wire operation: {kind}"
        );
    }

    shutdown.cancel();
}
