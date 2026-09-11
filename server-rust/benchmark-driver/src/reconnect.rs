use crate::reconnect_wire::{
    wire_lost_ack_reconnect, wire_partial_event_reconnect, wire_partial_transaction_reconnect,
    wire_pre_receipt_disconnect,
};
use crate::report::{push_timing_distributions, Assertion, Section};
use bytes::BytesMut;
use srui_protocol::{srui_message, ClientResume, SessionContinuity, SruiCodec, SruiMessage};
use srui_resources::CHUNK_PAYLOAD_SIZE;
use srui_sdk::{Button, Surface, ACTIVATE};
use srui_semantic_tree::{Event as DomainEvent, NodeId};
use srui_sessiond::{
    EventOutcome, LogicalChannelClass, OutboundItem, OutboundReceiver, ResumeOutcome, Session,
    SessionConfig,
};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tokio_util::codec::{Decoder, Encoder};

async fn next_resource(receiver: &mut OutboundReceiver) -> Result<OutboundItem, String> {
    timeout(
        Duration::from_secs(2),
        receiver.recv_class(LogicalChannelClass::Resource),
    )
    .await
    .map_err(|_| "timed out waiting for a production resource frame".to_string())?
    .map_err(|error| error.to_string())
}

fn empty_wire_transaction(base_revision: u64) -> srui_protocol::Transaction {
    srui_protocol::Transaction {
        base_revision,
        new_revision: base_revision + 1,
        priority: 0,
        operations: Vec::new(),
    }
}

fn resume_request(session: &Session, client: &[u8], revision: u64) -> ClientResume {
    ClientResume {
        session_id: session.session_id(),
        client_instance_id: client.to_vec(),
        last_applied_revision: revision,
        ..ClientResume::default()
    }
}

fn partial_transaction_frame_is_buffered(
    transaction: &srui_protocol::Transaction,
) -> Result<bool, String> {
    let mut codec = SruiCodec::new();
    let mut complete = BytesMut::new();
    codec
        .encode(
            SruiMessage {
                msg: Some(srui_message::Msg::Transaction(transaction.clone())),
            },
            &mut complete,
        )
        .map_err(|error| error.to_string())?;
    let split = complete.len() / 2;
    let mut partial = BytesMut::from(&complete[..split]);
    let decoded = codec
        .decode(&mut partial)
        .map_err(|error| error.to_string())?;
    Ok(decoded.is_none() && partial.len() == split)
}

pub(crate) async fn reconnect(iterations: usize) -> Result<Section, String> {
    let mut timings: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    let mut resource_replay_correct = true;
    let mut transaction_replay_correct = true;
    let mut wire_transaction_replay_correct = true;
    let mut event_boundary_correct = true;
    let mut wire_pre_receipt_correct = true;
    let mut wire_partial_event_correct = true;
    let mut duplicate_correct = true;
    let mut wire_duplicate_correct = true;
    let mut retention_correct = true;
    let resource_payload: Vec<u8> = (0..(CHUNK_PAYLOAD_SIZE * 2 + 37))
        .map(|index| (index % 251) as u8)
        .collect();

    for sample in 0..iterations {
        // Interrupt an actual production resource lane after its first chunk. A replacement
        // subscription is seeded from the session CAS and must restart at metadata/offset zero.
        let resource_session = Session::new(format!("benchmark-resource-{sample}"));
        let published = resource_session
            .publish_resource(&resource_payload)
            .map_err(|error| error.to_string())?;
        let client = format!("resource-client-{sample}").into_bytes();
        let mut interrupted = resource_session
            .subscribe_transactions(client.clone())
            .map_err(|error| error.to_string())?;
        let metadata = next_resource(&mut interrupted).await?;
        let first_chunk = next_resource(&mut interrupted).await?;
        let interrupted_at_real_boundary = matches!(
            &metadata,
            OutboundItem::ResourceMetadata(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.encoded_length == resource_payload.len() as u64
        ) && matches!(
            &first_chunk,
            OutboundItem::ResourceChunk(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.byte_offset == 0
                    && value.data.len() == CHUNK_PAYLOAD_SIZE
        );
        drop(interrupted);

        let start = Instant::now();
        let mut resumed = resource_session
            .subscribe_transactions(client)
            .map_err(|error| error.to_string())?;
        let mut resumed_metadata = false;
        let mut resumed_bytes = Vec::with_capacity(resource_payload.len());
        let mut next_offset = 0_u64;
        while resumed_bytes.len() < resource_payload.len() {
            match next_resource(&mut resumed).await? {
                OutboundItem::ResourceMetadata(value) => {
                    resumed_metadata = value.resource_hash == published.hash.0.to_vec()
                        && value.encoded_length == resource_payload.len() as u64;
                }
                OutboundItem::ResourceChunk(value) => {
                    if value.resource_hash != published.hash.0.to_vec()
                        || value.byte_offset != next_offset
                    {
                        resource_replay_correct = false;
                    }
                    next_offset = next_offset.saturating_add(value.data.len() as u64);
                    resumed_bytes.extend_from_slice(&value.data);
                }
                OutboundItem::Transaction(_) => {
                    resource_replay_correct = false;
                }
            }
        }
        timings
            .entry("mid-resource reconnect and exact replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        resource_replay_correct &= interrupted_at_real_boundary
            && resumed_metadata
            && resumed_bytes == resource_payload
            && next_offset == resource_payload.len() as u64;

        // Feed half of a real length-delimited TRANSACTION through SruiCodec, detach before it
        // completes, then resume through Session::bootstrap_resume and recover the atomic commit.
        let transaction_session = Session::new(format!("benchmark-frame-{sample}"));
        let committed = transaction_session
            .commit_transaction(empty_wire_transaction(0))
            .map_err(|error| error.to_string())?;
        let attachment = transaction_session
            .attach()
            .ok_or_else(|| "transaction session refused attachment".to_string())?;
        let start = Instant::now();
        let partial_was_buffered = partial_transaction_frame_is_buffered(&committed)?;
        drop(attachment);
        let resumed_transaction = transaction_session
            .bootstrap_resume(&resume_request(
                &transaction_session,
                format!("frame-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("mid-transaction frame discard and atomic replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        transaction_replay_correct &= partial_was_buffered
            && transaction_session.is_detached()
            && matches!(
                resumed_transaction.outcome,
                ResumeOutcome::Replay { replayed, .. }
                    if replayed.as_slice() == std::slice::from_ref(&committed)
            );

        // Keep the direct admission/result-cache timings as component measurements. The
        // pre-receipt reconnect itself is measured separately over handle_connection.
        let event_session = Session::new(format!("benchmark-event-{sample}"));
        event_session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                Button::builder(NodeId::new(2))
                    .parent(NodeId::new(1))
                    .label("Run")
                    .create(ui)?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        let side_effects = Arc::new(AtomicUsize::new(0));
        let handler_side_effects = Arc::clone(&side_effects);
        event_session.on(NodeId::new(2), ACTIVATE, move |_, _| {
            handler_side_effects.fetch_add(1, Ordering::SeqCst);
        });
        let event = DomainEvent::activate(
            1,
            "benchmark-event",
            event_session.current_revision(),
            NodeId::new(2),
        )
        .with_client_instance_id(format!("event-client-{sample}").into_bytes())
        .to_wire();

        let pre_receipt_attachment = event_session
            .attach()
            .ok_or_else(|| "event session refused attachment".to_string())?;
        drop(pre_receipt_attachment);
        event_boundary_correct &=
            event_session.is_detached() && side_effects.load(Ordering::SeqCst) == 0;

        let start = Instant::now();
        let first = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("event receipt through settled side effect")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        event_boundary_correct &= matches!(first, EventOutcome::Processed { .. })
            && side_effects.load(Ordering::SeqCst) == 1;

        let start = Instant::now();
        let replay = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("in-process cached DUPLICATE response")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        duplicate_correct &= matches!(
            replay,
            EventOutcome::Duplicate {
                accepted: true,
                revision_after_effect: 1,
                last_processed_event_seq: 1,
                ..
            }
        ) && side_effects.load(Ordering::SeqCst) == 1;

        // Exercise both resume decisions through Session::bootstrap_resume, not the journal type.
        let retention_session = Session::with_config(
            format!("benchmark-retention-{sample}"),
            SessionConfig {
                journal_capacity: 4,
                ..SessionConfig::default()
            },
        );
        for base in 0..8 {
            retention_session
                .commit_transaction(empty_wire_transaction(base))
                .map_err(|error| error.to_string())?;
        }

        let start = Instant::now();
        let within = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("retained-client-{sample}").as_bytes(),
                6,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume within journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            within.outcome,
            ResumeOutcome::Replay { replayed, .. }
                if replayed.len() == 2
                    && replayed[0].base_revision == 6
                    && replayed[1].new_revision == 8
        );

        let start = Instant::now();
        let beyond = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("expired-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume beyond journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            beyond.outcome,
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } if resync_msg.continuity == SessionContinuity::SameSession as i32
                && resync_msg.snapshot_revision == 8
                && snapshot_transaction.new_revision == 8
        );
    }

    let wire_sample_count = iterations.min(50);
    for sample in 0..wire_sample_count {
        let (elapsed, correct) = wire_pre_receipt_disconnect(sample).await?;
        timings
            .entry("disconnect immediately before event receipt")
            .or_default()
            .push(elapsed);
        wire_pre_receipt_correct &= correct;

        let (elapsed, correct) = wire_lost_ack_reconnect(sample).await?;
        timings
            .entry("lost ACK wire reconnect through DUPLICATE acknowledgement")
            .or_default()
            .push(elapsed);
        wire_duplicate_correct &= correct;

        let (elapsed, correct) = wire_partial_transaction_reconnect(sample).await?;
        timings
            .entry("mid-transaction wire disconnect and exact atomic replay")
            .or_default()
            .push(elapsed);
        wire_transaction_replay_correct &= correct;

        let (elapsed, correct) = wire_partial_event_reconnect(sample).await?;
        timings
            .entry("partial EVENT disconnect and one processed replay")
            .or_default()
            .push(elapsed);
        wire_partial_event_correct &= correct;
    }

    let mut sample_counts = BTreeMap::new();
    for (name, values) in &timings {
        let group = match *name {
            "mid-resource reconnect and exact replay" => "rust.mid_resource",
            "mid-transaction frame discard and atomic replay" => "rust.mid_transaction_codec",
            "event receipt through settled side effect" => "rust.event_side_effect",
            "in-process cached DUPLICATE response" => "rust.cached_duplicate",
            "resume within journal retention" => "rust.resume_within_retention",
            "resume beyond journal retention" => "rust.resume_beyond_retention",
            "disconnect immediately before event receipt" => "rust.pre_receipt_wire",
            "lost ACK wire reconnect through DUPLICATE acknowledgement" => "rust.lost_ack_wire",
            "mid-transaction wire disconnect and exact atomic replay" => {
                "rust.mid_transaction_wire"
            }
            "partial EVENT disconnect and one processed replay" => "rust.partial_event_wire",
            unexpected => {
                return Err(format!(
                    "reconnect timing group has no sample-count identity: {unexpected}"
                ));
            }
        };
        if sample_counts.insert(group, values.len()).is_some() {
            return Err(format!(
                "duplicate reconnect sample-count identity: {group}"
            ));
        }
    }

    let mut metrics = Vec::new();
    push_timing_distributions(&mut metrics, timings)?;
    Ok(Section {
        id: "31.5",
        name: "Reconnect",
        sample_counts,
        metrics,
        assertions: vec![
            Assertion {
                id: "mid_resource_exact_restart",
                name: "mid-resource reconnect restarts production transfer at offset zero",
                passed: resource_replay_correct,
                detail: format!(
                    "{}-byte CAS object interrupted after one real chunk and reconstructed exactly",
                    resource_payload.len()
                ),
            },
            Assertion {
                id: "mid_transaction_wire_atomic",
                name: "mid-transaction wire disconnect exposes no partial state",
                passed: transaction_replay_correct && wire_transaction_replay_correct,
                detail: format!(
                    "{wire_sample_count} capacity-one handle_connection streams decoded no partial frame, left the replica at revision 0, then replayed exactly one atomic revision"
                ),
            },
            Assertion {
                id: "partial_event_wire_once",
                name: "pre-receipt and partial EVENT disconnects are inert before one processed replay",
                passed: event_boundary_correct
                    && wire_pre_receipt_correct
                    && wire_partial_event_correct,
                detail: format!(
                    "{wire_sample_count} pre-receipt and capacity-one partial-frame handle_connection reconnects each dispatched zero events on the interrupted connection, then decoded one Processed acknowledgement and one resulting transaction"
                ),
            },
            Assertion {
                id: "lost_ack_wire_duplicate_once",
                name: "lost ACK wire replay is DUPLICATE without a second side effect",
                passed: duplicate_correct && wire_duplicate_correct,
                detail: format!(
                    "{wire_sample_count} duplex reconnects decoded ServerEventAck::Duplicate with cached revision 2; after connection shutdown the handler count, revision, and store remained at one effect"
                ),
            },
            Assertion {
                id: "journal_retention_boundary",
                name: "journal retention boundary selects replay versus same-session resync",
                passed: retention_correct,
                detail: "revision 6 replayed 6→8; revision 0 produced SAME_SESSION snapshot at 8"
                    .into(),
            },
        ],
        notes: vec![
            "Wire negative proofs use decoded acknowledgements plus exact handler counters, revisions, and store state after bounded connection shutdown; they do not infer absence from short timeouts."
                .into(),
            "Superseded resume-attempt inertness is measured against the Task 23 client generation guard in the macOS driver."
                .into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_srui_transaction_frame_is_not_decoded() {
        let transaction = empty_wire_transaction(0);
        assert!(partial_transaction_frame_is_buffered(&transaction).unwrap());
    }
}
