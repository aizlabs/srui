//! Authoritative `TEXT_EDIT` processing, per-editor sequence tracking, and application policy
//! (§7.6, §18.3, §22.6, §26, §27).
//!
//! Policy runs *outside* the session mutex. A per-node generation reserved before the policy call
//! is rechecked before commit so an older concurrent validator cannot overwrite a newer edit.

use std::collections::HashMap;
use std::sync::Arc;

use srui_event_dedupe::{EventOutcomeRecord, RecordOutcome};
use srui_protocol::Event as WireEvent;
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    AuthoritativeCommit, ClientInstanceId, EditSeq, Event as DomainEvent, EventId,
    EventValidationError, NodeId, PropertyRef, Revision, StandardValidationState, StoreError,
    TypeRef, Value,
};

use super::{
    bound_diagnostic_string, lock_or_recover, EventOutcome, Session, SessionError, SessionInner,
};

/// Default cap on tracked `(client_instance_id, node_id)` editor streams (§26).
///
/// New keys are refused rather than evicting monotonicity state.
pub const MAX_TEXT_EDIT_STREAMS: usize = 1024;

/// Submitted `TEXT_EDIT` presented to application policy (§22.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEditRequest {
    pub client_instance_id: ClientInstanceId,
    pub event_id: EventId,
    pub event_seq: u64,
    pub edit_seq: EditSeq,
    pub node_id: NodeId,
    pub value: String,
    pub observed_revision: Revision,
}

/// Application decision for one submitted text edit (§22.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEditDecision {
    /// Publish the submitted `.value` and `validation_state = valid`.
    Accept,
    /// Publish a policy-supplied value and validation state; acknowledge processed.
    Correct {
        value: String,
        validation: StandardValidationState,
    },
    /// Publish the current or supplied authoritative value plus error validation; acknowledge rejected.
    Reject {
        value: Option<String>,
        reason: String,
    },
}

pub type TextEditPolicy =
    Arc<dyn Fn(&Session, &TextEditRequest) -> TextEditDecision + Send + Sync + 'static>;

#[derive(Debug, Clone)]
struct EditorStream {
    last_terminal_edit_seq: u64,
    generation: u64,
}

/// Bounded per-`(client_instance_id, node_id)` editor sequence table (§18.3, §26).
#[derive(Debug, Clone)]
pub struct TextEditTracker {
    streams: HashMap<(Vec<u8>, u64), EditorStream>,
    max_streams: usize,
}

impl Default for TextEditTracker {
    fn default() -> Self {
        Self::new(MAX_TEXT_EDIT_STREAMS)
    }
}

impl TextEditTracker {
    #[must_use]
    pub fn new(max_streams: usize) -> Self {
        Self {
            streams: HashMap::new(),
            max_streams: max_streams.max(1),
        }
    }

    /// Client-controlled identifier bytes retained as map keys (§15, §26).
    #[must_use]
    pub fn retained_client_id_bytes(&self) -> usize {
        self.streams.keys().map(|(client, _)| client.len()).sum()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }

    /// Reserves a generation for a sequence strictly above the terminal watermark.
    pub fn reserve(
        &mut self,
        client_instance_id: &[u8],
        node_id: NodeId,
        edit_seq: EditSeq,
    ) -> Result<u64, EventValidationError> {
        let key = (client_instance_id.to_vec(), node_id.get());
        if let Some(stream) = self.streams.get_mut(&key) {
            if edit_seq.get() <= stream.last_terminal_edit_seq {
                return Err(EventValidationError::StaleEditSeq {
                    observed: edit_seq.get(),
                    watermark: stream.last_terminal_edit_seq,
                });
            }
            stream.generation = stream.generation.saturating_add(1);
            if stream.generation == 0 {
                stream.generation = u64::MAX;
            }
            return Ok(stream.generation);
        }
        if self.streams.len() >= self.max_streams {
            return Err(EventValidationError::TextTrackerFull {
                limit: self.max_streams,
            });
        }
        self.streams.insert(
            key,
            EditorStream {
                last_terminal_edit_seq: 0,
                generation: 1,
            },
        );
        Ok(1)
    }

    #[must_use]
    pub fn generation_of(&self, client_instance_id: &[u8], node_id: NodeId) -> Option<u64> {
        self.streams
            .get(&(client_instance_id.to_vec(), node_id.get()))
            .map(|s| s.generation)
    }

    /// Raises the terminal watermark for an existing stream. Does not insert new keys:
    /// canceled-edit refs for unknown or deleted nodes must not consume tracker capacity (§18.3, §26).
    pub fn mark_terminal(&mut self, client_instance_id: &[u8], node_id: NodeId, edit_seq: EditSeq) {
        let key = (client_instance_id.to_vec(), node_id.get());
        if let Some(stream) = self.streams.get_mut(&key) {
            stream.last_terminal_edit_seq = stream.last_terminal_edit_seq.max(edit_seq.get());
        }
    }

    pub fn reclaim_node(&mut self, node_id: NodeId) {
        let nid = node_id.get();
        self.streams.retain(|(_, stored), _| *stored != nid);
    }

    pub fn reclaim_nodes<I>(&mut self, node_ids: I)
    where
        I: IntoIterator<Item = NodeId>,
    {
        let doomed: std::collections::HashSet<u64> =
            node_ids.into_iter().map(NodeId::get).collect();
        if doomed.is_empty() {
            return;
        }
        self.streams.retain(|(_, nid), _| !doomed.contains(nid));
    }
}

pub(crate) fn is_standard_text_edit(event: &WireEvent) -> bool {
    event
        .event_type
        .as_ref()
        .is_some_and(|ty| ty.namespace_id == 0 && ty.local_id == TypeRef::EVENT_TEXT_EDIT.local_id)
}

pub(crate) fn is_editor_type(ty: TypeRef) -> bool {
    ty == TypeRef::TEXT_INPUT || ty == TypeRef::TEXT_AREA
}

impl Session {
    /// Registers the application policy for `TEXT_EDIT` events (§22.6).
    ///
    /// An absent policy accepts every structurally valid edit.
    pub fn on_text_edit<F>(&self, policy: F)
    where
        F: Fn(&Session, &TextEditRequest) -> TextEditDecision + Send + Sync + 'static,
    {
        let mut guard = lock_or_recover(&self.inner);
        guard.text_edit_policy = Some(Arc::new(policy));
    }

    /// Clears the registered text-edit policy, restoring default accept.
    pub fn clear_text_edit_policy(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.text_edit_policy = None;
    }

    pub(crate) fn process_text_edit(
        &self,
        event: &WireEvent,
    ) -> Result<EventOutcome, SessionError> {
        let domain = DomainEvent::try_from(event.clone())
            .map_err(|err| SessionError::InvalidInput(format!("malformed TEXT_EDIT: {err}")))?;

        let prepared = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

            match guard.dedupe.admit_event(event)? {
                RecordOutcome::Duplicate {
                    prior,
                    last_processed_event_seq,
                } => {
                    return Ok(EventOutcome::Duplicate {
                        accepted: prior.accepted,
                        revision_after_effect: prior.revision_after_effect,
                        last_processed_event_seq,
                        reject_reason: prior.reject_reason,
                    });
                }
                RecordOutcome::Pending {
                    last_processed_event_seq,
                } => {
                    return Ok(EventOutcome::Pending {
                        last_processed_event_seq,
                    });
                }
                RecordOutcome::Fresh { .. } => {}
            }

            if let Err(error) = validate_text_edit(&guard, &domain) {
                return Ok(reject_admitted(&mut guard, event, error));
            }

            let edit_seq = domain.edit_seq.expect("validated");
            let client_bytes = event.client_instance_id.clone();
            match guard
                .text_edit_tracker
                .reserve(&client_bytes, domain.node_id, edit_seq)
            {
                Ok(generation) => PreparedTextEdit {
                    request: TextEditRequest {
                        client_instance_id: domain
                            .client_instance_id
                            .clone()
                            .unwrap_or_else(|| ClientInstanceId::new(client_bytes.clone())),
                        event_id: domain.event_id.clone(),
                        event_seq: domain.event_seq,
                        edit_seq,
                        node_id: domain.node_id,
                        value: domain.text_arg().unwrap_or_default().to_string(),
                        observed_revision: domain.observed_revision,
                    },
                    generation,
                    policy: guard.text_edit_policy.clone(),
                },
                Err(error) => return Ok(reject_admitted(&mut guard, event, error)),
            }
        };

        let decision = if let Some(policy) = prepared.policy {
            let dispatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                policy(self, &prepared.request)
            }));
            match dispatch {
                Ok(decision) => decision,
                Err(panic_payload) => {
                    let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
                    guard.dedupe.abandon_event(event);
                    drop(guard);
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    return Err(SessionError::Panicked(panic_msg));
                }
            }
        } else {
            TextEditDecision::Accept
        };

        self.commit_text_edit_decision(event, &prepared.request, prepared.generation, decision)
    }

    fn commit_text_edit_decision(
        &self,
        event: &WireEvent,
        request: &TextEditRequest,
        reserved_generation: u64,
        decision: TextEditDecision,
    ) -> Result<EventOutcome, SessionError> {
        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

        if !guard
            .dedupe
            .is_in_flight(&event.client_instance_id, &event.event_id)
        {
            if let Some(prior) = guard
                .dedupe
                .settled_outcome(&event.client_instance_id, &event.event_id)
            {
                let last_processed_event_seq = guard
                    .dedupe
                    .last_contiguous_processed_seq(&event.client_instance_id);
                return Ok(EventOutcome::Duplicate {
                    accepted: prior.accepted,
                    revision_after_effect: prior.revision_after_effect,
                    last_processed_event_seq,
                    reject_reason: prior.reject_reason,
                });
            }
            return Ok(EventOutcome::Pending {
                last_processed_event_seq: guard
                    .dedupe
                    .last_contiguous_processed_seq(&event.client_instance_id),
            });
        }

        let current_generation = guard
            .text_edit_tracker
            .generation_of(&event.client_instance_id, request.node_id)
            .unwrap_or(0);
        if current_generation != reserved_generation {
            let error = EventValidationError::StaleEditSeq {
                observed: request.edit_seq.get(),
                watermark: current_generation,
            };
            return Ok(reject_admitted(&mut guard, event, error));
        }

        if let Err(error) = revalidate_editor_for_commit(&guard, request.node_id) {
            return Ok(reject_admitted(&mut guard, event, error));
        }

        let current_value = current_editor_value(&guard, request.node_id);
        let (publish_value, validation, accepted, reject_reason) = match decision {
            TextEditDecision::Accept => (
                request.value.clone(),
                StandardValidationState::Valid,
                true,
                String::new(),
            ),
            TextEditDecision::Correct { value, validation } => {
                (value, validation, true, String::new())
            }
            TextEditDecision::Reject { value, reason } => (
                value.unwrap_or(current_value),
                StandardValidationState::Error,
                false,
                reason,
            ),
        };

        let max_string_length = guard.store.limits().max_string_length;
        if publish_value.len() > max_string_length {
            let error = EventValidationError::StringTooLong {
                length: publish_value.len(),
                limit: max_string_length,
            };
            return Ok(reject_admitted(&mut guard, event, error));
        }

        let reject_reason = bound_diagnostic_string(reject_reason, max_string_length);
        let base_revision = guard.store.revision();
        let max_ops = guard.store.limits().max_transaction_operations;
        let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);
        if let Err(error) = ui.set(
            request.node_id,
            PropertyRef::VALUE,
            Value::String(publish_value),
        ) {
            return Ok(reject_store(&mut guard, event, error));
        }
        if let Err(error) = ui.set(
            request.node_id,
            PropertyRef::VALIDATION_STATE,
            Value::EnumToken(validation.into()),
        ) {
            return Ok(reject_store(&mut guard, event, error));
        }
        let (staged, ops) = ui.into_staged_and_ops();
        let commit = AuthoritativeCommit::new(base_revision, ops);
        let permit = match guard.journal.prepare(&commit) {
            Ok(permit) => permit,
            Err(error) => {
                return Ok(reject_admitted(
                    &mut guard,
                    event,
                    EventValidationError::PolicyRejected(error.to_string()),
                ));
            }
        };
        let tx_wire = permit.transaction().clone();
        guard.store.commit_staging(staged, commit.new_revision());
        guard.journal.append(permit);
        guard.text_edit_tracker.mark_terminal(
            &event.client_instance_id,
            request.node_id,
            request.edit_seq,
        );
        self.publish_committed(&tx_wire);

        let revision_after_effect = guard.store.revision().get();
        let last_processed_event_seq = guard.dedupe.settle_event(
            event,
            EventOutcomeRecord {
                accepted,
                revision_after_effect,
                reject_reason: reject_reason.clone(),
            },
        );

        if accepted {
            Ok(EventOutcome::Processed {
                revision_after_effect,
                last_processed_event_seq,
            })
        } else {
            Ok(EventOutcome::Rejected {
                error: EventValidationError::PolicyRejected(reject_reason),
                revision_after_effect,
                last_processed_event_seq,
            })
        }
    }

    pub(crate) fn cancel_pending_text_edits(
        inner: &mut SessionInner,
        client_instance_id: &[u8],
        refs: &[srui_protocol::PendingTextEditRef],
    ) -> Result<Vec<srui_protocol::PendingTextEditRef>, SessionError> {
        let revision_after_effect = inner.store.revision().get();
        let max_string_length = inner.store.limits().max_string_length;
        let outcome = EventOutcomeRecord {
            accepted: false,
            revision_after_effect,
            reject_reason: bound_diagnostic_string(
                "canceled on same-session resync".to_string(),
                max_string_length,
            ),
        };

        let mut discarded = Vec::with_capacity(refs.len());
        for reference in refs {
            if reference.event_id.is_empty() || reference.event_seq == 0 {
                return Err(SessionError::InvalidInput(
                    "pending TEXT_EDIT ref is missing event_id or event_seq".into(),
                ));
            }
            let Some(edit_seq) = EditSeq::new(reference.edit_seq) else {
                return Err(SessionError::InvalidInput(
                    "pending TEXT_EDIT ref requires a positive edit_seq".into(),
                ));
            };
            inner.dedupe.settle_canceled_text_event(
                client_instance_id,
                &reference.event_id,
                reference.event_seq,
                outcome.clone(),
            )?;
            inner.text_edit_tracker.mark_terminal(
                client_instance_id,
                NodeId::new(reference.node_id),
                edit_seq,
            );
            discarded.push(reference.clone());
        }
        Ok(discarded)
    }
}

struct PreparedTextEdit {
    request: TextEditRequest,
    generation: u64,
    policy: Option<TextEditPolicy>,
}

fn validate_text_edit(
    inner: &SessionInner,
    event: &DomainEvent,
) -> Result<(), EventValidationError> {
    let Some(_edit_seq) = event.edit_seq else {
        return Err(EventValidationError::InvalidEditSeq);
    };

    event.validate_observed_revision(inner.store.revision())?;
    let node = event.validate_node_interactive(&inner.store)?;
    if !is_editor_type(node.node_type) {
        return Err(EventValidationError::UnsupportedNodeType(node.node_type));
    }
    if let Some(Value::Bool(true)) = node.get_property(PropertyRef::READ_ONLY) {
        return Err(EventValidationError::NodeReadOnly(event.node_id));
    }
    let Some(text) = event.text_arg() else {
        return Err(EventValidationError::MissingArgument(PropertyRef::TEXT));
    };
    let limit = inner.store.limits().max_string_length;
    if text.len() > limit {
        return Err(EventValidationError::StringTooLong {
            length: text.len(),
            limit,
        });
    }
    Ok(())
}

fn revalidate_editor_for_commit(
    inner: &SessionInner,
    node_id: NodeId,
) -> Result<(), EventValidationError> {
    let node = inner
        .store
        .get_node(node_id)
        .ok_or(EventValidationError::NodeNotFound(node_id))?;
    if !is_editor_type(node.node_type) {
        return Err(EventValidationError::UnsupportedNodeType(node.node_type));
    }
    if let Some(Value::Bool(true)) = node.get_property(PropertyRef::READ_ONLY) {
        return Err(EventValidationError::NodeReadOnly(node_id));
    }
    Ok(())
}

fn reject_store(inner: &mut SessionInner, event: &WireEvent, error: StoreError) -> EventOutcome {
    let mapped = match error {
        StoreError::NodeNotFound(id) => EventValidationError::NodeNotFound(id),
        other => EventValidationError::PolicyRejected(other.to_string()),
    };
    reject_admitted(inner, event, mapped)
}

fn current_editor_value(inner: &SessionInner, node_id: NodeId) -> String {
    inner
        .store
        .get_node(node_id)
        .and_then(|node| {
            node.get_property(PropertyRef::VALUE)
                .and_then(Value::as_string)
                .or_else(|| {
                    node.get_property(PropertyRef::TEXT)
                        .and_then(Value::as_string)
                })
        })
        .unwrap_or("")
        .to_string()
}

fn reject_admitted(
    inner: &mut SessionInner,
    event: &WireEvent,
    error: EventValidationError,
) -> EventOutcome {
    let max_string_length = inner.store.limits().max_string_length;
    let revision_after_effect = inner.store.revision().get();
    let last_processed_event_seq = inner.dedupe.settle_event(
        event,
        EventOutcomeRecord {
            accepted: false,
            revision_after_effect,
            reject_reason: bound_diagnostic_string(error.to_string(), max_string_length),
        },
    );
    EventOutcome::Rejected {
        error,
        revision_after_effect,
        last_processed_event_seq,
    }
}

pub(crate) fn deleted_subtree_ids(
    store: &srui_semantic_tree::SemanticStore,
    ops: &[srui_semantic_tree::Operation],
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for op in ops {
        if let srui_semantic_tree::Operation::DeleteNode { id } = op {
            collect_subtree(store, *id, &mut ids, &mut seen);
        }
    }
    ids
}

pub(crate) fn deleted_subtree_ids_from_wire(
    store: &srui_semantic_tree::SemanticStore,
    tx: &srui_protocol::Transaction,
) -> Vec<NodeId> {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for op in &tx.operations {
        if let Some(srui_protocol::operation::Op::DeleteNode(ref del)) = op.op {
            collect_subtree(store, NodeId::new(del.node_id), &mut ids, &mut seen);
        }
    }
    ids
}

fn collect_subtree(
    store: &srui_semantic_tree::SemanticStore,
    id: NodeId,
    out: &mut Vec<NodeId>,
    seen: &mut std::collections::HashSet<u64>,
) {
    if !seen.insert(id.get()) {
        return;
    }
    out.push(id);
    if let Some(node) = store.get_node(id) {
        let children = node.ordered_children.clone();
        for child in children {
            collect_subtree(store, child, out, seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64) -> NodeId {
        NodeId::new(id)
    }

    fn seq(n: u64) -> EditSeq {
        EditSeq::new(n).expect("positive")
    }

    #[test]
    fn tracker_accepts_gaps_and_rejects_stale_sequences() {
        let mut tracker = TextEditTracker::new(8);
        assert_eq!(tracker.reserve(b"c", node(1), seq(1)).unwrap(), 1);
        tracker.mark_terminal(b"c", node(1), seq(1));
        assert_eq!(tracker.reserve(b"c", node(1), seq(3)).unwrap(), 2);
        tracker.mark_terminal(b"c", node(1), seq(3));
        assert!(matches!(
            tracker.reserve(b"c", node(1), seq(2)),
            Err(EventValidationError::StaleEditSeq {
                observed: 2,
                watermark: 3
            })
        ));
    }

    #[test]
    fn tracker_refuses_new_keys_when_full() {
        let mut tracker = TextEditTracker::new(2);
        tracker.reserve(b"c", node(1), seq(1)).unwrap();
        tracker.reserve(b"c", node(2), seq(1)).unwrap();
        assert!(matches!(
            tracker.reserve(b"c", node(3), seq(1)),
            Err(EventValidationError::TextTrackerFull { limit: 2 })
        ));
        assert!(tracker.reserve(b"c", node(1), seq(2)).is_ok());
    }

    #[test]
    fn mark_terminal_does_not_insert_a_new_stream() {
        let mut tracker = TextEditTracker::new(8);
        tracker.mark_terminal(b"c", node(99), seq(4));
        assert!(tracker.is_empty());
        tracker.reserve(b"c", node(1), seq(1)).unwrap();
        tracker.mark_terminal(b"c", node(1), seq(3));
        assert_eq!(tracker.len(), 1);
        assert!(matches!(
            tracker.reserve(b"c", node(1), seq(2)),
            Err(EventValidationError::StaleEditSeq {
                observed: 2,
                watermark: 3
            })
        ));
    }

    #[test]
    fn tracker_reclaims_deleted_nodes_and_counts_client_id_bytes() {
        let mut tracker = TextEditTracker::new(8);
        tracker.reserve(b"alice", node(1), seq(1)).unwrap();
        tracker.reserve(b"bob", node(1), seq(1)).unwrap();
        tracker.reserve(b"alice", node(2), seq(1)).unwrap();
        assert_eq!(tracker.len(), 3);
        assert_eq!(tracker.retained_client_id_bytes(), 5 + 3 + 5);
        tracker.reclaim_node(node(1));
        assert_eq!(tracker.len(), 1);
        assert_eq!(tracker.retained_client_id_bytes(), 5);
    }
}
