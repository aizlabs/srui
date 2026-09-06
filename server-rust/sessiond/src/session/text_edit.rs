//! Authoritative `TEXT_EDIT` processing, per-editor sequence tracking, and application policy
//! (§7.6, §18.3, §22.6, §26, §27).
//!
//! Policy runs *outside* the session mutex. A per-node generation reserved before the policy call
//! is rechecked before commit so an older concurrent validator cannot overwrite a newer edit.

use std::collections::{BTreeSet, HashMap};
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
    bound_diagnostic_string, lock_or_recover, EventOutcome, HandlerFn, Session, SessionError,
    SessionInner,
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

/// Application decision callback for one `TEXT_EDIT` (§22.6).
///
/// Invoked with the session mutex released (`async-no-lock-await`). The callback may read or
/// commit through `&Session` (for example to disable the editor). Nested `TEXT_EDIT` on the
/// same `(client, node)` bumps the stream generation; the outer in-flight commit then settles
/// as [`EventValidationError::SupersededGeneration`] instead of publishing.
pub type TextEditPolicy =
    Arc<dyn Fn(&Session, &TextEditRequest) -> TextEditDecision + Send + Sync + 'static>;

#[derive(Debug, Clone)]
struct EditorStream {
    last_terminal_edit_seq: u64,
    /// Reservations remain ordered so a policy panic can abandon one attempt without lowering
    /// the watermark below another concurrent in-flight edit (§18.3).
    in_flight_edit_seqs: BTreeSet<u64>,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommittedHandlerTextEdit {
    node_id: u64,
    edit_seq: u64,
}

impl CommittedHandlerTextEdit {
    fn from_event(event: &WireEvent) -> Self {
        Self {
            node_id: event.node_id,
            edit_seq: event.edit_seq,
        }
    }

    fn matches_pending(self, reference: &srui_protocol::PendingTextEditRef) -> bool {
        self.node_id == reference.node_id && self.edit_seq == reference.edit_seq
    }
}

#[derive(Debug, Clone, Default)]
struct ClientTextEditState {
    editor_streams: HashMap<u64, EditorStream>,
    /// Keyed by event_seq; event-id and event-type equality stay in the dedupe window, which
    /// already owns and bounds those peer-sized identity bytes. Values are fixed-size.
    committed_handler_edits: HashMap<u64, CommittedHandlerTextEdit>,
}

/// Bounded per-(client_instance_id, node_id) editor sequence table (§18.3, §26).
#[derive(Debug, Clone)]
pub struct TextEditTracker {
    /// One peer-sized client key owns both editor streams and fixed-size handler-phase records.
    /// Handler records correspond one-for-one with in-flight dedupe entries, so their count is
    /// bounded by the dedupe receive window without retaining another event-id copy.
    streams: HashMap<Vec<u8>, ClientTextEditState>,
    stream_count: usize,
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
            stream_count: 0,
            max_streams: max_streams.max(1),
        }
    }

    /// Client-controlled identifier bytes retained as map keys (§15, §26).
    #[must_use]
    pub fn retained_client_id_bytes(&self) -> usize {
        self.streams.keys().map(Vec::len).sum()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.stream_count
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
        if let Some(stream) = self
            .streams
            .get_mut(client_instance_id)
            .and_then(|client| client.editor_streams.get_mut(&node_id.get()))
        {
            let watermark = stream
                .in_flight_edit_seqs
                .last()
                .copied()
                .unwrap_or(stream.last_terminal_edit_seq)
                .max(stream.last_terminal_edit_seq);
            if edit_seq.get() <= watermark {
                return Err(EventValidationError::StaleEditSeq {
                    observed: edit_seq.get(),
                    watermark,
                });
            }
            stream.generation = stream
                .generation
                .checked_add(1)
                .ok_or(EventValidationError::GenerationOverflow)?;
            stream.in_flight_edit_seqs.insert(edit_seq.get());
            return Ok(stream.generation);
        }
        if self.stream_count >= self.max_streams {
            return Err(EventValidationError::TextTrackerFull {
                limit: self.max_streams,
            });
        }
        self.streams
            .entry(client_instance_id.to_vec())
            .or_default()
            .editor_streams
            .insert(
                node_id.get(),
                EditorStream {
                    last_terminal_edit_seq: 0,
                    in_flight_edit_seqs: BTreeSet::from([edit_seq.get()]),
                    generation: 1,
                },
            );
        self.stream_count += 1;
        Ok(1)
    }

    #[must_use]
    pub fn last_terminal_of(&self, client_instance_id: &[u8], node_id: NodeId) -> Option<u64> {
        self.streams
            .get(client_instance_id)
            .and_then(|client| client.editor_streams.get(&node_id.get()))
            .map(|stream| stream.last_terminal_edit_seq)
    }

    fn begin_committed_handler_dispatch(&mut self, event: &WireEvent) {
        let client = self
            .streams
            .get_mut(event.client_instance_id.as_slice())
            .expect("an accepted text edit retains its client tracker state");
        let replaced = client
            .committed_handler_edits
            .insert(event.event_seq, CommittedHandlerTextEdit::from_event(event));
        debug_assert!(
            replaced.is_none(),
            "dedupe prevents concurrent reuse of an in-flight event_seq"
        );
    }

    fn finish_committed_handler_dispatch(&mut self, event: &WireEvent) {
        let remove_client = self
            .streams
            .get_mut(event.client_instance_id.as_slice())
            .is_some_and(|client| {
                client.committed_handler_edits.remove(&event.event_seq);
                client.editor_streams.is_empty() && client.committed_handler_edits.is_empty()
            });
        if remove_client {
            self.streams.remove(event.client_instance_id.as_slice());
        }
    }

    /// Returns Some only for an accepted edit whose handlers are still running. The caller first
    /// validates (client_instance_id, event_id, event_seq, event_type) through the dedupe window;
    /// this fixed-size marker completes the exact comparison with (node_id, edit_seq).
    fn committed_handler_identity_matches(
        &self,
        client_instance_id: &[u8],
        reference: &srui_protocol::PendingTextEditRef,
    ) -> Option<bool> {
        self.streams
            .get(client_instance_id)
            .and_then(|client| client.committed_handler_edits.get(&reference.event_seq))
            .copied()
            .map(|identity| identity.matches_pending(reference))
    }

    #[must_use]
    pub fn generation_of(&self, client_instance_id: &[u8], node_id: NodeId) -> Option<u64> {
        self.streams
            .get(client_instance_id)
            .and_then(|client| client.editor_streams.get(&node_id.get()))
            .map(|stream| stream.generation)
    }

    /// Raises the terminal watermark for an existing stream. Does not insert new keys:
    /// canceled-edit refs for unknown or deleted nodes must not consume tracker capacity (§18.3, §26).
    pub fn mark_terminal(&mut self, client_instance_id: &[u8], node_id: NodeId, edit_seq: EditSeq) {
        if let Some(stream) = self
            .streams
            .get_mut(client_instance_id)
            .and_then(|client| client.editor_streams.get_mut(&node_id.get()))
        {
            stream.in_flight_edit_seqs.remove(&edit_seq.get());
            stream.last_terminal_edit_seq = stream.last_terminal_edit_seq.max(edit_seq.get());
        }
    }

    /// Releases a reservation whose policy panicked without making that edit terminal. Other
    /// concurrent reservations remain in the watermark, and generation numbers are never reused.
    pub fn abandon_reservation(
        &mut self,
        client_instance_id: &[u8],
        node_id: NodeId,
        edit_seq: EditSeq,
    ) {
        if let Some(stream) = self
            .streams
            .get_mut(client_instance_id)
            .and_then(|client| client.editor_streams.get_mut(&node_id.get()))
        {
            stream.in_flight_edit_seqs.remove(&edit_seq.get());
        }
    }

    pub fn reclaim_node(&mut self, node_id: NodeId) {
        let node_id = node_id.get();
        let mut removed = 0;
        self.streams.retain(|_, client| {
            removed += usize::from(client.editor_streams.remove(&node_id).is_some());
            !client.editor_streams.is_empty() || !client.committed_handler_edits.is_empty()
        });
        self.stream_count -= removed;
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
        let mut removed = 0;
        self.streams.retain(|_, client| {
            let before = client.editor_streams.len();
            client
                .editor_streams
                .retain(|node_id, _| !doomed.contains(node_id));
            removed += before - client.editor_streams.len();
            !client.editor_streams.is_empty() || !client.committed_handler_edits.is_empty()
        });
        self.stream_count -= removed;
    }
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
        domain: DomainEvent,
    ) -> Result<EventOutcome, SessionError> {
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

            let edit_seq = match validate_text_edit(&guard, &domain) {
                Ok(seq) => seq,
                Err(error) => return Ok(reject_admitted(&mut guard, event, error)),
            };
            let client_bytes = event.client_instance_id.clone();
            match guard
                .text_edit_tracker
                .reserve(&client_bytes, domain.node_id, edit_seq)
            {
                Ok(generation) => {
                    let handlers = guard
                        .handlers
                        .get(&(domain.node_id, domain.event_type))
                        .cloned()
                        .unwrap_or_default();
                    PreparedTextEdit {
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
                        handlers,
                    }
                }
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
                    return self.abandon_text_edit_after_panic(
                        event,
                        &prepared.request,
                        panic_payload,
                    );
                }
            }
        } else {
            TextEditDecision::Accept
        };

        self.commit_text_edit_decision(
            event,
            &prepared.request,
            prepared.generation,
            decision,
            &prepared.handlers,
        )
    }

    fn abandon_text_edit_after_panic(
        &self,
        event: &WireEvent,
        request: &TextEditRequest,
        panic_payload: Box<dyn std::any::Any + Send>,
    ) -> Result<EventOutcome, SessionError> {
        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        guard.text_edit_tracker.abandon_reservation(
            &event.client_instance_id,
            request.node_id,
            request.edit_seq,
        );
        guard.dedupe.abandon_event(event);
        drop(guard);

        Err(SessionError::Panicked(panic_payload_message(
            panic_payload.as_ref(),
        )))
    }

    fn commit_text_edit_decision(
        &self,
        event: &WireEvent,
        request: &TextEditRequest,
        reserved_generation: u64,
        decision: TextEditDecision,
        handlers: &[HandlerFn],
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

        if let Err(error) = revalidate_editor_for_commit(&guard, request.node_id) {
            return Ok(reject_reserved(&mut guard, event, request, error));
        }

        let current_generation = guard
            .text_edit_tracker
            .generation_of(&event.client_instance_id, request.node_id)
            .unwrap_or(0);
        if current_generation != reserved_generation {
            let error = EventValidationError::SupersededGeneration;
            return Ok(reject_reserved(&mut guard, event, request, error));
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
            return Ok(reject_reserved(&mut guard, event, request, error));
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
            return Ok(reject_reserved_store(&mut guard, event, request, error));
        }
        if let Err(error) = ui.set(
            request.node_id,
            PropertyRef::VALIDATION_STATE,
            Value::EnumToken(validation.into()),
        ) {
            return Ok(reject_reserved_store(&mut guard, event, request, error));
        }
        let (staged, ops) = ui.into_staged_and_ops();
        let commit = AuthoritativeCommit::new(base_revision, ops);
        let permit = match guard.journal.prepare(&commit) {
            Ok(permit) => permit,
            Err(error) => {
                return Ok(reject_reserved(
                    &mut guard,
                    event,
                    request,
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
        if accepted {
            guard
                .text_edit_tracker
                .begin_committed_handler_dispatch(event);
        }
        self.publish_committed(&tx_wire);

        if !accepted {
            let revision_after_effect = guard.store.revision().get();
            let last_processed_event_seq = guard.dedupe.settle_event(
                event,
                EventOutcomeRecord {
                    accepted: false,
                    revision_after_effect,
                    reject_reason: reject_reason.clone(),
                },
            );
            return Ok(EventOutcome::Rejected {
                error: EventValidationError::PolicyRejected(reject_reason),
                revision_after_effect,
                last_processed_event_seq,
            });
        }

        // Keep the admitted identity in flight until registered handlers finish. An accepted
        // TEXT_EDIT has the same ACK semantics as every other accepted event: the reported
        // revision includes transactions committed by its handlers.
        drop(guard);
        let dispatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for handler in handlers {
                handler(self, event);
            }
        }));

        // The authoritative text value is already committed even if a notification handler
        // panics. Settle it as accepted before surfacing the infrastructure failure so a resumed
        // client cannot replay the edit or re-enter the handler.
        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let revision_after_effect = guard.store.revision().get();
        let last_processed_event_seq = guard.dedupe.settle_event(
            event,
            EventOutcomeRecord {
                accepted: true,
                revision_after_effect,
                reject_reason: String::new(),
            },
        );
        guard
            .text_edit_tracker
            .finish_committed_handler_dispatch(event);
        drop(guard);

        if let Err(panic_payload) = dispatch {
            return Err(SessionError::Panicked(panic_payload_message(
                panic_payload.as_ref(),
            )));
        }

        Ok(EventOutcome::Processed {
            revision_after_effect,
            last_processed_event_seq,
        })
    }

    pub(crate) fn validate_pending_text_edit_refs(
        refs: &[srui_protocol::PendingTextEditRef],
    ) -> Result<(), SessionError> {
        if refs.len() > MAX_TEXT_EDIT_STREAMS {
            return Err(SessionError::InvalidInput(format!(
                "pending_text_edits has {} entries; at most {MAX_TEXT_EDIT_STREAMS} are accepted (§18.3, §26)",
                refs.len()
            )));
        }
        for reference in refs {
            if reference.event_id.is_empty() || reference.event_seq == 0 {
                return Err(SessionError::InvalidInput(
                    "pending TEXT_EDIT ref is missing event_id or event_seq".into(),
                ));
            }
            if EditSeq::new(reference.edit_seq).is_none() {
                return Err(SessionError::InvalidInput(
                    "pending TEXT_EDIT ref requires a positive edit_seq".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn cancel_pending_text_edits(
        inner: &mut SessionInner,
        client_instance_id: &[u8],
        refs: &[srui_protocol::PendingTextEditRef],
    ) -> Result<Vec<srui_protocol::PendingTextEditRef>, SessionError> {
        Self::validate_pending_text_edit_refs(refs)?;
        let validated: Vec<_> = refs
            .iter()
            .map(|reference| {
                EditSeq::new(reference.edit_seq)
                    .map(|edit_seq| (reference, edit_seq))
                    .ok_or_else(|| SessionError::InvalidInput("invalid edit_seq".into()))
            })
            .collect::<Result<_, _>>()?;

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

        // Stage the bounded receive-window and editor-watermark changes together. A later
        // identity conflict must not leave an earlier ref settled when the handshake fails.
        let mut staged_dedupe = inner.dedupe.clone();
        let mut staged_tracker = inner.text_edit_tracker.clone();
        let mut discarded = Vec::with_capacity(validated.len());
        for (reference, edit_seq) in validated {
            let placeholder = WireEvent {
                client_instance_id: client_instance_id.to_vec(),
                event_seq: reference.event_seq,
                event_id: reference.event_id.clone(),
                node_id: reference.node_id,
                event_type: Some(TypeRef::EVENT_TEXT_EDIT.into()),
                edit_seq: edit_seq.get(),
                ..Default::default()
            };
            // A duplicate accepted identity remains accepted, while unseen identities are
            // canceled and made terminal. An edit whose authoritative commit already landed but
            // whose registered handlers are still running must remain in flight: the resync still
            // tells the client to discard its local copy, and handler completion will cache the
            // accepted result at the true post-handler revision.
            // Admission validates the variable-sized event_id plus event_seq and event type
            // against the bounded dedupe record before the fixed-size handler marker is consulted.
            let (settle_as_canceled, mark_terminal) = match staged_dedupe
                .admit_event(&placeholder)?
            {
                RecordOutcome::Duplicate { .. } => (false, true),
                RecordOutcome::Fresh { .. } => (true, true),
                RecordOutcome::Pending { .. } => {
                    match staged_tracker
                        .committed_handler_identity_matches(client_instance_id, reference)
                    {
                        Some(true) => (false, false),
                        Some(false) => {
                            return Err(SessionError::InvalidInput(
                                    "pending TEXT_EDIT ref does not match committed in-handler event identity"
                                        .into(),
                                ));
                        }
                        None => (true, true),
                    }
                }
            };
            if settle_as_canceled {
                staged_dedupe.settle_event(&placeholder, outcome.clone());
            }
            if mark_terminal {
                staged_tracker.mark_terminal(
                    client_instance_id,
                    NodeId::new(reference.node_id),
                    edit_seq,
                );
            }
            discarded.push(reference.clone());
        }
        inner.dedupe = staged_dedupe;
        inner.text_edit_tracker = staged_tracker;
        Ok(discarded)
    }
}

struct PreparedTextEdit {
    request: TextEditRequest,
    generation: u64,
    policy: Option<TextEditPolicy>,
    handlers: Vec<HandlerFn>,
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn validate_text_edit(
    inner: &SessionInner,
    event: &DomainEvent,
) -> Result<EditSeq, EventValidationError> {
    let Some(edit_seq) = event.edit_seq else {
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
    Ok(edit_seq)
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
    if let Some(Value::Bool(false)) = node.get_property(PropertyRef::ENABLED) {
        return Err(EventValidationError::NodeDisabled(node_id));
    }
    if let Some(Value::Bool(true)) = node.get_property(PropertyRef::READ_ONLY) {
        return Err(EventValidationError::NodeReadOnly(node_id));
    }
    Ok(())
}

fn reject_reserved_store(
    inner: &mut SessionInner,
    event: &WireEvent,
    request: &TextEditRequest,
    error: StoreError,
) -> EventOutcome {
    let mapped = match error {
        StoreError::NodeNotFound(id) => EventValidationError::NodeNotFound(id),
        other => EventValidationError::PolicyRejected(other.to_string()),
    };
    reject_reserved(inner, event, request, mapped)
}

fn reject_reserved(
    inner: &mut SessionInner,
    event: &WireEvent,
    request: &TextEditRequest,
    error: EventValidationError,
) -> EventOutcome {
    inner.text_edit_tracker.mark_terminal(
        &event.client_instance_id,
        request.node_id,
        request.edit_seq,
    );
    reject_admitted(inner, event, error)
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
        assert_eq!(tracker.retained_client_id_bytes(), 5 + 3);
        tracker.reclaim_node(node(1));
        assert_eq!(tracker.len(), 1);
        assert_eq!(tracker.retained_client_id_bytes(), 5);
    }

    #[test]
    fn handler_phase_records_reuse_the_bounded_client_key() {
        let client = vec![b'x'; 4_096];
        let huge_event_id = vec![b'e'; 64 * 1_024];
        let mut tracker = TextEditTracker::new(8);

        tracker.reserve(&client, node(1), seq(1)).unwrap();
        tracker.mark_terminal(&client, node(1), seq(1));
        let first = WireEvent {
            client_instance_id: client.clone(),
            event_id: huge_event_id.clone(),
            event_seq: 1,
            node_id: 1,
            edit_seq: 1,
            ..Default::default()
        };
        tracker.begin_committed_handler_dispatch(&first);

        tracker.reserve(&client, node(2), seq(1)).unwrap();
        tracker.mark_terminal(&client, node(2), seq(1));
        let second = WireEvent {
            client_instance_id: client.clone(),
            event_id: huge_event_id,
            event_seq: 2,
            node_id: 2,
            edit_seq: 1,
            ..Default::default()
        };
        tracker.begin_committed_handler_dispatch(&second);

        assert_eq!(tracker.streams.len(), 1);
        assert_eq!(tracker.retained_client_id_bytes(), client.len());
        assert_eq!(
            tracker
                .streams
                .get(client.as_slice())
                .unwrap()
                .committed_handler_edits
                .len(),
            2
        );

        tracker.reclaim_nodes([node(1), node(2)]);
        assert_eq!(tracker.len(), 0);
        assert!(!tracker.is_empty());
        assert_eq!(tracker.retained_client_id_bytes(), client.len());

        tracker.finish_committed_handler_dispatch(&first);
        assert_eq!(tracker.retained_client_id_bytes(), client.len());
        tracker.finish_committed_handler_dispatch(&second);
        assert!(tracker.is_empty());
        assert_eq!(tracker.retained_client_id_bytes(), 0);
    }

    #[test]
    fn tracker_refuses_generation_overflow() {
        let mut tracker = TextEditTracker::new(8);
        tracker.reserve(b"c", node(1), seq(1)).unwrap();
        tracker
            .streams
            .get_mut(&b"c"[..])
            .unwrap()
            .editor_streams
            .get_mut(&1u64)
            .unwrap()
            .generation = u64::MAX;
        assert!(matches!(
            tracker.reserve(b"c", node(1), seq(2)),
            Err(EventValidationError::GenerationOverflow)
        ));
    }

    #[test]
    fn cancel_pending_text_edits_is_atomic_on_identity_conflict() {
        let session = Session::new("cancel-atomic");
        let client = b"client";
        let activate = WireEvent {
            client_instance_id: client.to_vec(),
            event_seq: 2,
            event_id: b"shared".to_vec(),
            event_type: Some(TypeRef::EVENT_ACTIVATE.into()),
            ..Default::default()
        };

        let mut inner = session.inner.lock().unwrap();
        assert!(matches!(
            inner.dedupe.admit_event(&activate).unwrap(),
            RecordOutcome::Fresh { .. }
        ));
        let refs = vec![
            srui_protocol::PendingTextEditRef {
                event_id: b"text-1".to_vec(),
                event_seq: 1,
                node_id: 1,
                edit_seq: 1,
            },
            srui_protocol::PendingTextEditRef {
                event_id: activate.event_id.clone(),
                event_seq: activate.event_seq,
                node_id: 1,
                edit_seq: 2,
            },
        ];

        assert!(Session::cancel_pending_text_edits(&mut inner, client, &refs).is_err());
        assert!(!inner.dedupe.is_duplicate(client, b"text-1"));
        assert!(inner.dedupe.is_in_flight(client, b"shared"));
        assert_eq!(inner.dedupe.last_contiguous_processed_seq(client), 0);
    }
}
