//! Authoritative `TEXT_EDIT` processing, per-editor sequence tracking, and application policy
//! (§7.6, §18.3, §22.6, §26, §27).
//!
//! Policy runs *outside* the session mutex. A per-node generation reserved before the policy call
//! is rechecked before commit so an older concurrent validator cannot overwrite a newer edit.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, MutexGuard};

use srui_event_dedupe::{EventDeduplicator, EventOutcomeRecord, RecordOutcome};
use srui_protocol::Event as WireEvent;
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    AuthoritativeCommit, ClientInstanceId, EditSeq, Event as DomainEvent, EventId,
    EventValidationError, NodeId, PropertyRef, Revision, SemanticStore, StandardValidationState,
    StoreError, TypeRef, Value, MAX_EVENT_ID_BYTES,
};

use super::{
    bound_diagnostic_string, bounded_rejected_event_id, lock_or_recover, oversized_event_dedupe_id,
    panic_payload_message, EventOutcome, HandlerDispatchKind, RegisteredHandler, Session,
    SessionError, SessionInner,
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

    fn is_at_capacity(&self) -> bool {
        self.stream_count >= self.max_streams
    }

    /// Reclaims terminal editor streams only after all event-sequence history for the client has
    /// aged out.
    ///
    /// An evicted full receive window may leave a retained contiguous frontier. That frontier lets
    /// the same client resume at its next `event_seq`, so its edit-sequence watermarks must remain
    /// coupled to it; otherwise a returning client could overwrite newer text with a lower
    /// `edit_seq`. Both the full windows and retained frontiers are bounded by the deduplicator,
    /// so sustained identity churn eventually makes genuinely departed clients reclaimable.
    ///
    /// A client with an in-flight policy or committed handler remains pinned defensively, even
    /// though the deduplicator itself will not evict a window with an unsettled event.
    fn reclaim_departed_clients(&mut self, dedupe: &EventDeduplicator) {
        let mut removed = 0;
        self.streams.retain(|client_instance_id, client| {
            let pinned = !client.committed_handler_edits.is_empty()
                || client
                    .editor_streams
                    .values()
                    .any(|stream| !stream.in_flight_edit_seqs.is_empty());
            let has_event_sequence_history = dedupe.has_client_window(client_instance_id)
                || dedupe.last_contiguous_processed_seq(client_instance_id) != 0;
            if has_event_sequence_history || pinned {
                true
            } else {
                removed += client.editor_streams.len();
                false
            }
        });
        self.stream_count -= removed;
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

    pub(super) fn finish_committed_handler_dispatch(&mut self, event: &WireEvent) {
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

    /// Reclaims streams only for nodes absent from the committed post-transaction store.
    ///
    /// Scanning is bounded by `max_streams`; final membership, rather than the pre-commit tree,
    /// handles transactions that reparent nodes into or out of a deleted subtree.
    pub(super) fn reclaim_missing_nodes(&mut self, store: &SemanticStore) {
        let mut removed = 0;
        self.streams.retain(|_, client| {
            let before = client.editor_streams.len();
            client
                .editor_streams
                .retain(|node_id, _| store.contains_node(NodeId::new(*node_id)));
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
    pub(crate) fn process_admitted_text_edit(
        &self,
        event: &WireEvent,
        domain: DomainEvent,
        mut guard: MutexGuard<'_, SessionInner>,
    ) -> Result<EventOutcome, SessionError> {
        let edit_seq = match validate_text_edit(&guard, &domain) {
            Ok(seq) => seq,
            Err(error) => return Ok(Self::settle_rejected_event(&mut guard, event, error)),
        };
        let client_bytes = event.client_instance_id.clone();
        if guard.text_edit_tracker.is_at_capacity() {
            let SessionInner {
                dedupe,
                text_edit_tracker,
                ..
            } = &mut *guard;
            text_edit_tracker.reclaim_departed_clients(dedupe);
        }
        let prepared =
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
                                .unwrap_or_else(|| ClientInstanceId::new(client_bytes)),
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
                Err(error) => return Ok(Self::settle_rejected_event(&mut guard, event, error)),
            };
        drop(guard);

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

        match self.commit_text_edit_decision(
            event,
            &prepared.request,
            prepared.generation,
            decision,
        )? {
            TextEditCommit::Settled(outcome) => Ok(outcome),
            TextEditCommit::DispatchAccepted(transaction) => self.dispatch_admitted_event(
                event,
                &prepared.handlers,
                HandlerDispatchKind::CommittedTextEdit,
                Some(&transaction),
            ),
        }
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
    ) -> Result<TextEditCommit, SessionError> {
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
                return Ok(TextEditCommit::Settled(EventOutcome::Duplicate {
                    accepted: prior.accepted,
                    revision_after_effect: prior.revision_after_effect,
                    last_processed_event_seq,
                    reject_reason: prior.reject_reason,
                }));
            }
            return Ok(TextEditCommit::Settled(EventOutcome::Pending {
                last_processed_event_seq: guard
                    .dedupe
                    .last_contiguous_processed_seq(&event.client_instance_id),
            }));
        }

        if let Err(error) = revalidate_editor_for_commit(&guard, request.node_id) {
            return Ok(TextEditCommit::Settled(reject_reserved(
                &mut guard, event, request, error,
            )));
        }

        let current_generation = guard
            .text_edit_tracker
            .generation_of(&event.client_instance_id, request.node_id)
            .unwrap_or(0);
        if current_generation != reserved_generation {
            return Ok(TextEditCommit::Settled(reject_reserved(
                &mut guard,
                event,
                request,
                EventValidationError::SupersededGeneration,
            )));
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
            return Ok(TextEditCommit::Settled(reject_reserved(
                &mut guard, event, request, error,
            )));
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
            return Ok(TextEditCommit::Settled(reject_reserved_store(
                &mut guard, event, request, error,
            )));
        }
        if let Err(error) = ui.set(
            request.node_id,
            PropertyRef::VALIDATION_STATE,
            Value::EnumToken(validation.into()),
        ) {
            return Ok(TextEditCommit::Settled(reject_reserved_store(
                &mut guard, event, request, error,
            )));
        }
        let (staged, ops) = ui.into_staged_and_ops();
        let commit = AuthoritativeCommit::new(base_revision, ops);
        let permit = match guard.journal.prepare(&commit) {
            Ok(permit) => permit,
            Err(error) => {
                return Ok(TextEditCommit::Settled(reject_reserved(
                    &mut guard,
                    event,
                    request,
                    EventValidationError::PolicyRejected(error.to_string()),
                )));
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
            return Ok(TextEditCommit::Settled(EventOutcome::Rejected {
                error: EventValidationError::PolicyRejected(reject_reason),
                revision_after_effect,
                last_processed_event_seq,
            }));
        }

        // Keep the admitted identity in flight until registered handlers finish. The shared
        // dispatch path samples the post-handler revision and always removes this marker.
        Ok(TextEditCommit::DispatchAccepted(tx_wire))
    }

    pub(crate) fn validate_pending_text_edit_refs(
        refs: &[srui_protocol::PendingTextEditRef],
    ) -> Result<Vec<ValidatedPendingTextEditRef>, SessionError> {
        if refs.len() > MAX_TEXT_EDIT_STREAMS {
            return Err(SessionError::InvalidInput(format!(
                "pending_text_edits has {} entries; at most {MAX_TEXT_EDIT_STREAMS} are accepted (§18.3, §26)",
                refs.len()
            )));
        }

        let mut validated = Vec::with_capacity(refs.len());
        let mut seen_dedupe_ids: HashSet<Vec<u8>> = HashSet::with_capacity(refs.len());
        let mut seen_response_ids: HashSet<Vec<u8>> = HashSet::with_capacity(refs.len());
        for reference in refs {
            if reference.event_id.is_empty() || reference.event_seq == 0 {
                return Err(SessionError::InvalidInput(
                    "pending TEXT_EDIT ref is missing event_id or event_seq".into(),
                ));
            }
            let oversized = reference.event_id.len() > MAX_EVENT_ID_BYTES;
            let dedupe_event_id = if oversized {
                tracing::warn!(
                    event_seq = reference.event_seq,
                    actual = reference.event_id.len(),
                    limit = MAX_EVENT_ID_BYTES,
                    "settling oversized pending TEXT_EDIT event_id through a bounded identity"
                );
                oversized_event_dedupe_id(reference.event_seq, &reference.event_id)
            } else {
                reference.event_id.clone()
            };
            let response_event_id = if oversized {
                bounded_rejected_event_id(reference.event_seq, &reference.event_id)
            } else {
                reference.event_id.clone()
            };

            // Check the identities the server will actually retain and return, not just the raw
            // bytes. Both normalized forms are digest-bound, so distinct oversized values stay
            // distinct; this guard catches a genuine repeat, whose intended settlement is
            // ambiguous and would desynchronize the echoed discard list from the client's assigned
            // set. Malformed input fails explicitly rather than being silently coalesced
            // (§4 inv. 13).
            if !seen_dedupe_ids.insert(dedupe_event_id.clone())
                || !seen_response_ids.insert(response_event_id.clone())
            {
                return Err(SessionError::InvalidInput(
                    "pending_text_edits repeats a normalized event_id".into(),
                ));
            }
            let edit_seq = EditSeq::new(reference.edit_seq).ok_or_else(|| {
                SessionError::InvalidInput(
                    "pending TEXT_EDIT ref requires a positive edit_seq".into(),
                )
            })?;
            let mut bounded_reference = reference.clone();
            bounded_reference.event_id = response_event_id;
            validated.push(ValidatedPendingTextEditRef {
                reference: bounded_reference,
                dedupe_event_id,
                edit_seq,
            });
        }
        Ok(validated)
    }
    pub(crate) fn prepare_pending_text_edit_cancellation(
        inner: &SessionInner,
        client_instance_id: &[u8],
        refs: &[ValidatedPendingTextEditRef],
    ) -> PendingTextEditCancellation {
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

        // Stage cancellation until subscription succeeds. Never derive an editor watermark from
        // the resume payload: node_id/edit_seq are peer-controlled and the dedupe identity does
        // not authenticate either field. An already in-flight edit is allowed to finish; its
        // eventual transaction follows the snapshot and converges the client authoritatively.
        let mut staged_dedupe = inner.dedupe.clone();
        let mut discarded = Vec::with_capacity(refs.len());
        for validated in refs {
            let reference = &validated.reference;
            let placeholder = WireEvent {
                client_instance_id: client_instance_id.to_vec(),
                event_seq: reference.event_seq,
                event_id: validated.dedupe_event_id.clone(),
                node_id: reference.node_id,
                event_type: Some(TypeRef::EVENT_TEXT_EDIT.into()),
                edit_seq: validated.edit_seq.get(),
                ..Default::default()
            };

            match staged_dedupe.admit_event(&placeholder) {
                Ok(RecordOutcome::Fresh { .. }) => {
                    staged_dedupe.settle_event(&placeholder, outcome.clone());
                }
                Ok(RecordOutcome::Duplicate { .. }) => {}
                Ok(RecordOutcome::Pending { .. }) => {
                    if matches!(
                        inner
                            .text_edit_tracker
                            .committed_handler_identity_matches(client_instance_id, reference),
                        Some(false)
                    ) {
                        tracing::warn!(
                            event_seq = reference.event_seq,
                            "ignoring mismatched pending TEXT_EDIT resume identity"
                        );
                    }
                }
                Err(error) => {
                    // A bad replay reference must not make every subsequent resume fail. Echo its
                    // bounded rejection marker, but leave the authoritative dedupe entry untouched.
                    tracing::warn!(
                        %error,
                        event_seq = reference.event_seq,
                        "ignoring invalid pending TEXT_EDIT resume identity"
                    );
                }
            }
            discarded.push(reference.clone());
        }

        PendingTextEditCancellation {
            staged_dedupe,
            discarded,
        }
    }
}

struct PreparedTextEdit {
    request: TextEditRequest,
    generation: u64,
    policy: Option<TextEditPolicy>,
    handlers: Vec<RegisteredHandler>,
}

enum TextEditCommit {
    Settled(EventOutcome),
    DispatchAccepted(srui_protocol::Transaction),
}

#[derive(Debug)]
pub(crate) struct ValidatedPendingTextEditRef {
    reference: srui_protocol::PendingTextEditRef,
    dedupe_event_id: Vec<u8>,
    edit_seq: EditSeq,
}

pub(crate) struct PendingTextEditCancellation {
    staged_dedupe: EventDeduplicator,
    discarded: Vec<srui_protocol::PendingTextEditRef>,
}

impl PendingTextEditCancellation {
    pub(crate) fn last_processed_event_seq(&self, client_instance_id: &[u8]) -> u64 {
        self.staged_dedupe
            .last_contiguous_processed_seq(client_instance_id)
    }

    pub(crate) fn discarded_text_edits(&self) -> &[srui_protocol::PendingTextEditRef] {
        &self.discarded
    }

    pub(crate) fn commit(self, inner: &mut SessionInner) {
        inner.dedupe = self.staged_dedupe;
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
    Session::settle_rejected_event(inner, event, error)
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
    fn pending_resume_ids_are_bounded_and_stay_distinct_after_normalization() {
        let refs = [
            srui_protocol::PendingTextEditRef {
                event_id: vec![b'a'; MAX_EVENT_ID_BYTES + 1],
                event_seq: 7,
                node_id: 1,
                edit_seq: 1,
            },
            srui_protocol::PendingTextEditRef {
                event_id: vec![b'b'; MAX_EVENT_ID_BYTES + 2],
                event_seq: 7,
                node_id: 1,
                edit_seq: 2,
            },
        ];

        let validated = Session::validate_pending_text_edit_refs(&refs)
            .expect("distinct oversized identifiers are each settled through their own marker");
        assert_eq!(validated.len(), 2);
        for (validated, original) in validated.iter().zip(refs.iter()) {
            // Neither the retained nor the echoed identity may carry the peer's bytes...
            assert!(validated.reference.event_id.len() <= MAX_EVENT_ID_BYTES);
            assert_ne!(validated.reference.event_id, original.event_id);
            assert_eq!(validated.dedupe_event_id.len(), MAX_EVENT_ID_BYTES + 1);
        }
        // ...and two different oversized identifiers at one sequence must not collapse onto one
        // internal identity or one echoed discard entry (§4 inv. 13).
        assert_ne!(validated[0].dedupe_event_id, validated[1].dedupe_event_id);
        assert_ne!(
            validated[0].reference.event_id,
            validated[1].reference.event_id
        );

        // A genuine repeat of one identifier is still ambiguous and still fails explicitly.
        let repeated = [refs[0].clone(), refs[0].clone()];
        let error = Session::validate_pending_text_edit_refs(&repeated)
            .expect_err("a repeated oversized identifier must fail explicitly");
        assert!(matches!(
            error,
            SessionError::InvalidInput(message)
                if message.contains("normalized event_id")
        ));
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
    fn processing_retains_edit_watermark_while_event_frontier_can_resume() {
        use srui_sdk::{Surface, TextInput};

        let session = Session::new("tracker-frontier-coupling");
        session
            .transaction(|ui| {
                Surface::builder(1).create(ui)?;
                TextInput::builder(2).parent(1).value("").create(ui)?;
                TextInput::builder(3).parent(1).value("").create(ui)?;
                Ok(())
            })
            .expect("seed editors");
        {
            let mut inner = session.inner.lock().unwrap();
            inner.text_edit_tracker = TextEditTracker::new(1);
            inner.dedupe = EventDeduplicator::with_limits(8, 1);
        }

        let veteran = DomainEvent::text_edit(1, "veteran-1", 1u64, 2, "newer", seq(10))
            .with_client_instance_id(b"veteran".to_vec())
            .to_wire();
        assert!(matches!(
            session.process_event(&veteran),
            Ok(EventOutcome::Processed { .. })
        ));

        let newcomer = DomainEvent::text_edit(1, "newcomer-1", 2u64, 3, "other", seq(1))
            .with_client_instance_id(b"newcomer".to_vec())
            .to_wire();
        assert!(matches!(
            session.process_event(&newcomer),
            Ok(EventOutcome::Rejected {
                error: EventValidationError::TextTrackerFull { limit: 1 },
                ..
            })
        ));

        {
            let inner = session.inner.lock().unwrap();
            assert!(
                !inner.dedupe.has_client_window(b"veteran"),
                "the veteran's full receive window was evicted"
            );
            assert_eq!(
                inner.dedupe.last_contiguous_processed_seq(b"veteran"),
                1,
                "the resumable event frontier remains"
            );
            assert_eq!(
                inner
                    .text_edit_tracker
                    .last_terminal_of(b"veteran", node(2)),
                Some(10),
                "the edit watermark must outlive the full receive window"
            );
        }

        let stale_return = DomainEvent::text_edit(2, "veteran-2", 2u64, 2, "stale", seq(9))
            .with_client_instance_id(b"veteran".to_vec())
            .to_wire();
        assert!(matches!(
            session.process_event(&stale_return),
            Ok(EventOutcome::Rejected {
                error: EventValidationError::StaleEditSeq {
                    observed: 9,
                    watermark: 10,
                },
                ..
            })
        ));
    }

    #[test]
    fn processing_reclaims_client_after_window_and_frontier_age_out() {
        use srui_sdk::{Surface, TextInput};

        let session = Session::new("tracker-client-reclamation");
        session
            .transaction(|ui| {
                Surface::builder(1).create(ui)?;
                TextInput::builder(2).parent(1).value("").create(ui)?;
                TextInput::builder(3).parent(1).value("").create(ui)?;
                Ok(())
            })
            .expect("seed editors");
        {
            let mut inner = session.inner.lock().unwrap();
            inner.text_edit_tracker = TextEditTracker::new(1);
            inner.dedupe = EventDeduplicator::with_limits(8, 1);
        }

        let departed_event =
            DomainEvent::text_edit(1, "departed-event", 1u64, 2, "departed", seq(1))
                .with_client_instance_id(b"departed".to_vec())
                .to_wire();
        assert!(matches!(
            session.process_event(&departed_event),
            Ok(EventOutcome::Processed { .. })
        ));

        let bridge_event = DomainEvent::text_edit(1, "bridge-event", 2u64, 3, "bridge", seq(1))
            .with_client_instance_id(b"bridge".to_vec())
            .to_wire();
        assert!(matches!(
            session.process_event(&bridge_event),
            Ok(EventOutcome::Rejected {
                error: EventValidationError::TextTrackerFull { limit: 1 },
                ..
            })
        ));

        let current_event = DomainEvent::text_edit(1, "current-event", 2u64, 3, "current", seq(1))
            .with_client_instance_id(b"current".to_vec())
            .to_wire();
        assert!(matches!(
            session.process_event(&current_event),
            Ok(EventOutcome::Processed { .. })
        ));

        let inner = session.inner.lock().unwrap();
        assert_eq!(inner.text_edit_tracker.len(), 1);
        assert_eq!(
            inner
                .text_edit_tracker
                .last_terminal_of(b"departed", node(2)),
            None
        );
        assert_eq!(
            inner
                .text_edit_tracker
                .last_terminal_of(b"current", node(3)),
            Some(1)
        );
    }

    #[test]
    fn pending_text_edit_cancellation_is_staged_and_conflicts_are_nonfatal() {
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

        let validated =
            Session::validate_pending_text_edit_refs(&refs).expect("structurally valid refs");
        let cancellation =
            Session::prepare_pending_text_edit_cancellation(&inner, client, &validated);
        assert_eq!(cancellation.discarded_text_edits(), refs);
        assert!(!inner.dedupe.is_duplicate(client, b"text-1"));
        assert!(inner.dedupe.is_in_flight(client, b"shared"));

        cancellation.commit(&mut inner);
        assert!(inner.dedupe.is_duplicate(client, b"text-1"));
        assert!(inner.dedupe.is_in_flight(client, b"shared"));
        assert_eq!(inner.dedupe.last_contiguous_processed_seq(client), 1);
    }

    #[test]
    fn pending_text_edit_cancellation_never_trusts_resume_watermarks() {
        let session = Session::new("cancel-untrusted-watermark");
        let client = b"client";
        let mut inner = session.inner.lock().unwrap();
        inner
            .text_edit_tracker
            .reserve(client, node(7), seq(1))
            .unwrap();
        inner
            .text_edit_tracker
            .mark_terminal(client, node(7), seq(1));

        let refs = vec![srui_protocol::PendingTextEditRef {
            event_id: b"forged-watermark".to_vec(),
            event_seq: 1,
            node_id: 7,
            edit_seq: u64::MAX,
        }];
        let validated =
            Session::validate_pending_text_edit_refs(&refs).expect("structurally valid refs");
        let cancellation =
            Session::prepare_pending_text_edit_cancellation(&inner, client, &validated);
        cancellation.commit(&mut inner);

        assert_eq!(
            inner.text_edit_tracker.last_terminal_of(client, node(7)),
            Some(1)
        );
        assert!(inner
            .text_edit_tracker
            .reserve(client, node(7), seq(2))
            .is_ok());
    }

    #[test]
    fn duplicate_pending_text_edit_event_ids_are_rejected() {
        let duplicated = srui_protocol::PendingTextEditRef {
            event_id: b"repeated".to_vec(),
            event_seq: 1,
            node_id: 7,
            edit_seq: 1,
        };
        let refs = vec![
            duplicated.clone(),
            srui_protocol::PendingTextEditRef {
                event_seq: 2,
                node_id: 8,
                edit_seq: 2,
                ..duplicated
            },
        ];

        match Session::validate_pending_text_edit_refs(&refs) {
            Err(SessionError::InvalidInput(message)) => {
                assert!(
                    message.contains("event_id"),
                    "unexpected message: {message}"
                );
            }
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("a repeated event_id is malformed"),
        }
    }
}
