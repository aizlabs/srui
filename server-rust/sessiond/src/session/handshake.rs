//! Atomic client attach: optimistic catch-up export plus bounded outbound subscribe
//! (§15, §18, §18.1, §18.3, §20.2, §21).
//!
//! # Lock Order
//!
//! Bootstrap holds the [`SessionInner`](super::SessionInner) lock across
//! [`OutboundHub::subscribe`](crate::outbound::OutboundHub::subscribe) and
//! `OutboundHub::is_client_stale`, so the session lock is always the outermost of the two; the hub
//! itself nests `subscribers -> SubscriberState -> stale_clients`. Nothing may take a hub lock
//! before the session lock, or the two orders deadlock.

use std::collections::{HashMap, HashSet};

use crate::outbound::OutboundReceiver;
use srui_protocol::{
    ClientHello, ClientLimits, ClientResume, ServerResumeOk, ServerResyncRequired, ServerWelcome,
    SessionContinuity, Transaction, MAX_TERMINAL_RESUME_MAP_ENTRIES,
};
use srui_semantic_tree::{CapabilitySet, Profile, ResourceHash, SemanticStore};

use super::terminal::TerminalAttach;

use super::snapshot::export_snapshot_transaction;
use super::{Session, SessionError};

/// Core protocol version this build speaks (§15).
pub const CORE_VERSION: &str = "0.5.0";

/// Lock-free exports attempted before the bounded liveness fallback takes the session mutex.
const MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS: usize = 3;

/// Whether `requested` names a core version this build can serve (§15, §4 inv. 13).
///
/// Compatibility is decided on `major.minor`; the patch level is free. An absent field decodes to
/// the proto3 default `""`, which is indistinguishable from "omitted" on the wire, so it is
/// refused rather than treated as "unspecified, therefore fine": a default must never be the thing
/// that authorizes a session.
fn core_version_is_compatible(requested: &str) -> bool {
    fn major_minor(version: &str) -> Option<(&str, &str)> {
        let mut parts = version.split('.');
        let major = parts.next()?;
        let minor = parts.next()?;
        let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (numeric(major) && numeric(minor)).then_some((major, minor))
    }

    match (major_minor(requested), major_minor(CORE_VERSION)) {
        (Some(requested), Some(supported)) => requested == supported,
        _ => false,
    }
}

/// Result of an atomic fresh-client handshake bootstrap (§15, §18, §20.2).
#[derive(Debug)]
pub struct FreshClientBootstrap {
    /// Negotiated welcome parameters sent to the fresh client (§15).
    pub welcome: ServerWelcome,
    /// Catch-up snapshot transaction for populated sessions, or `None` if revision is 0 (§18).
    pub snapshot: Option<Transaction>,
    /// Bounded outbound receiver capturing every subsequent transaction committed to the session (§20.2).
    pub transactions: OutboundReceiver,
    /// Whether this client negotiated `org.srui.terminal/1`.
    pub terminal_negotiated: bool,
    /// Terminal catch-up captured before welcome so live bytes cannot fall through (§21.2).
    pub terminal: TerminalAttach,
}

/// Result of an atomic resume handshake bootstrap (§20.2, §21, §32.5).
#[derive(Debug)]
pub struct ResumeClientBootstrap {
    /// Replay or resync catch-up collected under the same lock as [`Self::transactions`].
    pub outcome: ResumeOutcome,
    /// Bounded outbound receiver capturing every subsequent transaction committed to the session (§20.2).
    pub transactions: OutboundReceiver,
    /// Whether this resume re-advertised and negotiated `org.srui.terminal/1`.
    pub terminal_negotiated: bool,
    /// Terminal catch-up captured before resume so live bytes cannot fall through (§21.2).
    pub terminal: TerminalAttach,
}

/// Outcome of a [`ClientResume`] handshake request.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    /// The exact requested session survived: sends `ServerResumeOk` followed by replay.
    Replay {
        welcome_msg: ServerResumeOk,
        replayed: Vec<Transaction>,
    },
    /// Full snapshot required, either for a same-session journal gap or a replaced incarnation.
    Resync {
        resync_msg: ServerResyncRequired,
        snapshot_transaction: Transaction,
    },
}

fn negotiate_client(
    inner: &super::SessionInner,
    core_version: &str,
    profiles: &[String],
) -> Result<CapabilitySet, SessionError> {
    // §15: `core_version` is part of the handshake, not decoration. Accepting an unknown core
    // version would let two peers that disagree about required semantics reach the data plane
    // (§4 inv. 13).
    if !core_version_is_compatible(core_version) {
        return Err(SessionError::UnsupportedCoreVersion {
            requested: core_version.to_string(),
            supported: CORE_VERSION.to_string(),
        });
    }

    let mut client_caps = CapabilitySet::new();
    for profile in profiles {
        if let Ok(profile) = Profile::parse(profile) {
            client_caps.insert(profile);
        }
    }
    let negotiated = inner.capabilities.negotiate(&client_caps)?;
    validate_extension_namespace_contract(inner)?;
    Ok(negotiated)
}

/// Validates the complete server-advertised namespace contract while `SessionInner` is locked.
///
/// Handshake responses carry the server's required and optional profile lists. An advertised
/// optional extension without a mapping would therefore become negotiated for any client that
/// offers it. Reject that configuration before snapshot export, subscription, or terminal attach
/// instead of sending a response whose numeric type references cannot be decoded (§6.4, §15).
fn validate_extension_namespace_contract(inner: &super::SessionInner) -> Result<(), SessionError> {
    let standard_widgets = Profile::standard_widgets_v1();
    let standard_uri = standard_widgets.name();
    let mut standard_mappings = inner
        .extension_namespaces
        .iter()
        .filter(|mapping| mapping.extension_uri == standard_uri);
    let Some(standard_mapping) = standard_mappings.next() else {
        return Err(SessionError::InvalidConfiguration(
            "the reserved standard widget namespace mapping is missing".to_string(),
        ));
    };
    if standard_mapping.namespace_id != 0 {
        return Err(SessionError::InvalidConfiguration(
            "the reserved standard widget namespace must use namespace_id 0".to_string(),
        ));
    }
    if standard_mappings.next().is_some() {
        return Err(SessionError::InvalidConfiguration(
            "the reserved standard widget namespace has more than one mapping".to_string(),
        ));
    }

    for profile in inner
        .capabilities
        .required
        .iter()
        .chain(inner.capabilities.optional.iter())
    {
        if profile.name() == standard_uri {
            if profile != &standard_widgets {
                return Err(SessionError::InvalidConfiguration(format!(
                    "unsupported standard widget profile {profile}; namespace_id 0 is reserved for org.srui.standard-widgets/1"
                )));
            }
            continue;
        }

        let extension_uri = profile.to_string();
        let mut mappings = inner
            .extension_namespaces
            .iter()
            .filter(|mapping| mapping.extension_uri == extension_uri);
        let Some(mapping) = mappings.next() else {
            return Err(SessionError::InvalidConfiguration(format!(
                "advertised profile {profile} has no extension namespace mapping"
            )));
        };
        if mapping.namespace_id == 0 {
            return Err(SessionError::InvalidConfiguration(format!(
                "advertised profile {profile} must use a nonzero extension namespace"
            )));
        }
        if mappings.next().is_some() {
            return Err(SessionError::InvalidConfiguration(format!(
                "advertised profile {profile} has more than one extension namespace mapping"
            )));
        }
    }

    let mut seen_ids = HashSet::new();
    let mut seen_uris = HashSet::new();
    for mapping in &inner.extension_namespaces {
        if mapping.extension_uri == standard_uri {
            if mapping.namespace_id != 0 {
                return Err(SessionError::InvalidConfiguration(
                    "the reserved standard widget namespace must use namespace_id 0".to_string(),
                ));
            }
        } else {
            if mapping.namespace_id == 0 {
                return Err(SessionError::InvalidConfiguration(format!(
                    "extension namespace {:?} must use a nonzero namespace_id",
                    mapping.extension_uri
                )));
            }
            let mapped_profile = Profile::parse(&mapping.extension_uri).map_err(|error| {
                SessionError::InvalidConfiguration(format!(
                    "extension namespace URI {:?} is not a versioned profile: {error}",
                    mapping.extension_uri
                ))
            })?;
            if mapped_profile.to_string() != mapping.extension_uri {
                return Err(SessionError::InvalidConfiguration(format!(
                    "extension namespace URI {:?} is not canonical; expected {:?}",
                    mapping.extension_uri,
                    mapped_profile.to_string()
                )));
            }
            if mapped_profile.name() == standard_uri {
                return Err(SessionError::InvalidConfiguration(
                    "standard widget profiles cannot have nonzero extension namespace aliases"
                        .to_string(),
                ));
            }
            if !inner.capabilities.required.contains(&mapped_profile)
                && !inner.capabilities.optional.contains(&mapped_profile)
            {
                return Err(SessionError::InvalidConfiguration(format!(
                    "extension namespace URI {:?} is not advertised as a required or optional profile",
                    mapping.extension_uri
                )));
            }
        }

        if !seen_ids.insert(mapping.namespace_id) {
            return Err(SessionError::InvalidConfiguration(format!(
                "duplicate extension namespace_id {}",
                mapping.namespace_id
            )));
        }
        if !seen_uris.insert(mapping.extension_uri.as_str()) {
            return Err(SessionError::InvalidConfiguration(format!(
                "duplicate extension_uri {:?} in extension namespace table",
                mapping.extension_uri
            )));
        }
    }

    Ok(())
}

fn negotiate_hello(
    inner: &super::SessionInner,
    hello: &ClientHello,
) -> Result<(ServerWelcome, Option<SemanticStore>, bool), SessionError> {
    // Refuse an unretainable identity before any per-client table copies it (§15, §26).
    validate_client_instance_id(&hello.client_instance_id)?;

    let negotiated = negotiate_client(inner, &hello.core_version, &hello.profiles)?;
    let terminal_negotiated = negotiated.contains(&Profile::terminal_v1());

    let initial_revision = inner.store.revision().get();
    // Advertise the effective (server ∩ client) resource ceiling so peers agree on §15/§26 limits.
    let mut limits = inner.limits;
    limits.max_resource_size =
        negotiated_max_resource_size(inner.limits.max_resource_size, hello.limits.as_ref()) as u32;
    let welcome = ServerWelcome {
        core_version: CORE_VERSION.to_string(),
        required_profiles: inner.capabilities.required.to_string_vec(),
        optional_profiles: inner.capabilities.optional.to_string_vec(),
        session_id: inner.session_id.clone(),
        initial_revision,
        extension_namespaces: inner.extension_namespaces.clone(),
        limits: Some(limits),
    };

    let store_clone = if initial_revision > 0 {
        Some(inner.store.clone_staging())
    } else {
        None
    };

    Ok((welcome, store_clone, terminal_negotiated))
}

/// Intersects the client's advertised `max_resource_size` with the server ceiling (§15, §26).
///
/// A missing or zero client value means "no client preference" and keeps the server default.
fn negotiated_max_resource_size(server: u32, client_limits: Option<&ClientLimits>) -> u64 {
    let server = u64::from(server);
    let client = client_limits
        .map(|limits| u64::from(limits.max_resource_size))
        .unwrap_or(0);
    if client == 0 {
        server
    } else {
        server.min(client)
    }
}

/// Parses a bounded set of verified client CAS hashes.
///
/// Malformed values are ignored so an advisory optimization cannot fail the handshake.
fn known_resource_hashes(raw_hashes: &[Vec<u8>], limit: usize) -> HashSet<ResourceHash> {
    raw_hashes
        .iter()
        .take(limit)
        .filter_map(|raw| {
            let bytes: [u8; 32] = raw.as_slice().try_into().ok()?;
            Some(ResourceHash::new(bytes))
        })
        .collect()
}

/// Bounds remembered per-client ceilings; `client_instance_id` is client-supplied (§15, §26).
pub const MAX_CLIENT_RESOURCE_CEILINGS: usize = 256;

/// Maximum accepted `client_instance_id` length, in bytes (§15, §26).
///
/// The identifier is client-supplied and is retained as a *key* in several long-lived per-client
/// tables: remembered resource ceilings, the stale-client record, and each subscriber's identity.
/// Those tables bound their entry count, not the size of a key, so an unbounded identifier lets
/// ordinary handshakes grow daemon memory toward the frame ceiling times the entry cap. 64 bytes
/// holds a UUID or a SHA-256 with room to spare.
pub const MAX_CLIENT_INSTANCE_ID_BYTES: usize = 64;

/// Refuses a `client_instance_id` too large to retain, before anything stores a copy of it.
fn validate_client_instance_id(client_instance_id: &[u8]) -> Result<(), SessionError> {
    if client_instance_id.len() > MAX_CLIENT_INSTANCE_ID_BYTES {
        return Err(SessionError::InvalidInput(format!(
            "client_instance_id is {} bytes; at most {} are accepted (§15, §26)",
            client_instance_id.len(),
            MAX_CLIENT_INSTANCE_ID_BYTES
        )));
    }
    Ok(())
}

fn remember_client_resource_ceiling(
    ceilings: &mut HashMap<Vec<u8>, u64>,
    client_instance_id: &[u8],
    max_resource_size: u64,
) {
    if ceilings.len() >= MAX_CLIENT_RESOURCE_CEILINGS && !ceilings.contains_key(client_instance_id)
    {
        // Drop an arbitrary entry to keep the table bounded.
        if let Some(key) = ceilings.keys().next().cloned() {
            ceilings.remove(&key);
        }
    }
    ceilings.insert(client_instance_id.to_vec(), max_resource_size);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResyncCause {
    ReplacedIncarnation,
    OutboundQueueOverflow,
    JournalGap,
}

impl ResyncCause {
    fn continuity(self) -> SessionContinuity {
        match self {
            Self::ReplacedIncarnation => SessionContinuity::Replaced,
            Self::OutboundQueueOverflow | Self::JournalGap => SessionContinuity::SameSession,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::ReplacedIncarnation => "requested session incarnation is no longer available",
            Self::OutboundQueueOverflow => {
                "outbound transaction queue overflowed; full state resynchronization required (§20.2)"
            }
            Self::JournalGap => "client revision outside retained journal window",
        }
    }
}

impl Session {
    fn subscribe_client_locked(
        &self,
        inner: &mut super::SessionInner,
        client_instance_id: &[u8],
        advertised_resource_hashes: &[Vec<u8>],
        max_resource_size: u64,
    ) -> Result<OutboundReceiver, SessionError> {
        let max_ops = inner.limits.max_transaction_operations as usize;
        let max_frame_size = inner.limits.max_frame_size as usize;
        remember_client_resource_ceiling(
            &mut inner.client_resource_ceilings,
            client_instance_id,
            max_resource_size,
        );
        let known_hashes = known_resource_hashes(
            advertised_resource_hashes,
            inner.resources.limits().max_entries,
        );
        let retained_resources = inner.resources.retained_entries();
        let transactions = self.outbound_hub.subscribe(
            client_instance_id.to_vec(),
            self.outbound_queue_capacity,
            max_ops,
            max_frame_size,
            max_resource_size,
        )?;
        // Seed while still holding SessionInner so a resource published between snapshot
        // creation and live subscription cannot be missed (SessionInner -> OutboundHub order).
        self.outbound_hub.seed_resources_excluding(
            &transactions,
            &retained_resources,
            &known_hashes,
        );
        Ok(transactions)
    }

    /// Atomically prepares a fresh client handshake and subscribes it to transactions (§15, §18, §20.2).
    ///
    /// Evaluates `ClientHello`, negotiates capabilities, constructs `ServerWelcome`, and
    /// clones authoritative state under the session lock. Snapshot serialization runs after
    /// releasing the lock; bootstrap then reacquires it and subscribes only if the captured
    /// revision is still current, retrying otherwise so catch-up and live delivery have no gap.
    ///
    /// # Revision-Zero Omission
    /// When `initial_revision == 0`, `snapshot` is `None` because an empty `0 -> 0` snapshot violates normal
    /// transaction invariants.
    ///
    /// # Errors
    /// Returns [`SessionError::Negotiation`] if capability negotiation fails against client profiles.
    /// Returns [`SessionError::LockPoisoned`] if an internal mutex is poisoned.
    /// Returns [`SessionError::OutboundClosed`] if the outbound hub has been closed.
    pub fn bootstrap_fresh_client(
        &self,
        hello: &ClientHello,
    ) -> Result<FreshClientBootstrap, SessionError> {
        self.bootstrap_fresh_client_with(hello, || {})
    }

    /// [`Self::bootstrap_fresh_client`] with a callback run immediately before subscription while
    /// the session lock is held, used by tests to interleave a commit with the catch-up clone.
    pub(crate) fn bootstrap_fresh_client_with<F>(
        &self,
        hello: &ClientHello,
        before_subscribe: F,
    ) -> Result<FreshClientBootstrap, SessionError>
    where
        F: FnOnce(),
    {
        self.bootstrap_fresh_client_with_exporter(
            hello,
            before_subscribe,
            export_snapshot_transaction,
        )
    }

    fn bootstrap_fresh_client_with_exporter<F, E>(
        &self,
        hello: &ClientHello,
        before_subscribe: F,
        mut export_snapshot: E,
    ) -> Result<FreshClientBootstrap, SessionError>
    where
        F: FnOnce(),
        E: FnMut(&SemanticStore) -> Result<Transaction, SessionError>,
    {
        let mut before_subscribe = Some(before_subscribe);
        let mut optimistic_attempts = 0usize;

        loop {
            let mut inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let (welcome, store_clone, terminal_negotiated) = negotiate_hello(&inner_guard, hello)?;

            let snapshot = if optimistic_attempts >= MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS {
                // Sustained commits cannot starve the synchronous handshake forever. This rare
                // fallback serializes the immutable clone while holding SessionInner, preserving
                // the no-gap boundary and still exporting before any subscriber is replaced.
                match store_clone {
                    Some(store) => Some(export_snapshot(&store).inspect_err(|error| {
                        tracing::error!(
                            %error,
                            "refusing to send an unrepresentable catch-up snapshot"
                        );
                    })?),
                    None => None,
                }
            } else {
                drop(inner_guard);

                // Normal path: full-store serialization leaves commits, events, and text edits
                // available while the immutable staging clone is exported.
                let snapshot_result = match store_clone {
                    Some(store) => export_snapshot(&store).map(Some).inspect_err(|error| {
                        tracing::error!(
                            %error,
                            "refusing to send an unrepresentable catch-up snapshot"
                        );
                    }),
                    None => Ok(None),
                };

                inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
                if inner_guard.store.revision().get() != welcome.initial_revision {
                    optimistic_attempts += 1;
                    drop(inner_guard);
                    continue;
                }
                snapshot_result?
            };

            before_subscribe
                .take()
                .expect("subscription callback runs once")();

            let max_resource_size = negotiated_max_resource_size(
                inner_guard.limits.max_resource_size,
                hello.limits.as_ref(),
            );
            let transactions = self.subscribe_client_locked(
                &mut inner_guard,
                &hello.client_instance_id,
                &hello.known_resource_hashes,
                max_resource_size,
            )?;
            // Capabilities are now fixed for the rest of this session's life: a later detach
            // must not reopen capability-changing operations (§15, §21).
            inner_guard.has_negotiated = true;
            drop(inner_guard);

            // Attach only after the catch-up export is committed so optimistic retries cannot
            // duplicate PTY listeners. Live bytes produced during welcome/snapshot send queue
            // on these subscriptions instead of falling through (§21.2).
            let terminal = self.attach_terminals(&HashMap::new(), false)?;

            return Ok(FreshClientBootstrap {
                welcome,
                snapshot,
                transactions,
                terminal_negotiated,
                terminal,
            });
        }
    }

    /// Atomically prepares a client resume and subscribes it to transactions (§18, §18.1, §20.2).
    ///
    /// Core/profile compatibility is validated before selecting or exporting any catch-up.
    /// Replaced session incarnations are evaluated first (§18.1). If the session incarnation
    /// matches but the client detached due to outbound queue overflow (§20.2), full state resync
    /// is forced. If neither applies and `last_applied_revision` is within the retained journal
    /// window, a replay is prepared; otherwise, same-session snapshot resync is returned.
    pub fn bootstrap_resume(
        &self,
        resume: &ClientResume,
    ) -> Result<ResumeClientBootstrap, SessionError> {
        self.bootstrap_resume_with(resume, || {})
    }

    /// [`Self::bootstrap_resume`] with a callback run immediately before subscription while the
    /// session lock is held, used by tests to interleave a commit with the replay collection.
    pub(crate) fn bootstrap_resume_with<F>(
        &self,
        resume: &ClientResume,
        before_subscribe: F,
    ) -> Result<ResumeClientBootstrap, SessionError>
    where
        F: FnOnce(),
    {
        self.bootstrap_resume_with_exporter(resume, before_subscribe, export_snapshot_transaction)
    }

    fn bootstrap_resume_with_exporter<F, E>(
        &self,
        resume: &ClientResume,
        before_subscribe: F,
        mut export_snapshot: E,
    ) -> Result<ResumeClientBootstrap, SessionError>
    where
        F: FnOnce(),
        E: FnMut(&SemanticStore) -> Result<Transaction, SessionError>,
    {
        #[allow(clippy::large_enum_variant)]
        enum ResumePlan {
            Replay {
                welcome_msg: ServerResumeOk,
                replayed: Vec<Transaction>,
            },
            Resync {
                session_id: String,
                snapshot_revision: u64,
                snapshot_transaction: Transaction,
                continuity: SessionContinuity,
                reason: String,
                last_processed_event_seq: u64,
                discarded_text_edits: Vec<srui_protocol::PendingTextEditRef>,
                required_profiles: Vec<String>,
                optional_profiles: Vec<String>,
                extension_namespaces: Vec<srui_protocol::ExtensionNamespaceMapping>,
                pending_text_edit_cancellation:
                    Option<super::text_edit::PendingTextEditCancellation>,
            },
        }

        validate_client_instance_id(&resume.client_instance_id)?;
        if resume.terminal_stream_offsets.len() > MAX_TERMINAL_RESUME_MAP_ENTRIES {
            return Err(SessionError::InvalidInput(format!(
                "terminal_stream_offsets has {} entries; at most {MAX_TERMINAL_RESUME_MAP_ENTRIES} are accepted (§21, §26)",
                resume.terminal_stream_offsets.len()
            )));
        }
        let pending_text_edits =
            Session::validate_pending_text_edit_refs(&resume.pending_text_edits)?;

        let determine_resync_cause = |inner_guard: &super::SessionInner| {
            if resume.session_id != inner_guard.session_id {
                Some(ResyncCause::ReplacedIncarnation)
            } else if self
                .outbound_hub
                .is_client_stale(&resume.client_instance_id)
            {
                Some(ResyncCause::OutboundQueueOverflow)
            } else if inner_guard
                .journal
                .iter_from(resume.last_applied_revision)
                .is_none()
            {
                Some(ResyncCause::JournalGap)
            } else {
                None
            }
        };

        let mut before_subscribe = Some(before_subscribe);
        let mut optimistic_attempts = 0usize;
        let (mut inner_guard, mut plan, terminal_negotiated) = loop {
            let inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let negotiated =
                negotiate_client(&inner_guard, &resume.core_version, &resume.profiles)?;
            let terminal_negotiated = negotiated.contains(&Profile::terminal_v1());
            let Some(cause) = determine_resync_cause(&inner_guard) else {
                let last_processed_event_seq = inner_guard
                    .dedupe
                    .last_contiguous_processed_seq(&resume.client_instance_id);
                let iter = inner_guard
                    .journal
                    .iter_from(resume.last_applied_revision)
                    .expect("resync cause already ruled out a journal gap");
                let plan = ResumePlan::Replay {
                    welcome_msg: ServerResumeOk {
                        session_id: inner_guard.session_id.clone(),
                        replay_from_revision: resume.last_applied_revision,
                        last_processed_event_seq,
                        required_profiles: inner_guard.capabilities.required.to_string_vec(),
                        optional_profiles: inner_guard.capabilities.optional.to_string_vec(),
                        extension_namespaces: inner_guard.extension_namespaces.clone(),
                    },
                    replayed: iter.cloned().collect(),
                };
                break (inner_guard, plan, terminal_negotiated);
            };

            let snapshot_revision = inner_guard.store.revision().get();
            let store_snapshot = inner_guard.store.clone_staging();

            let (inner_guard, snapshot_transaction) =
                if optimistic_attempts >= MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS {
                    // Bounded fallback: guarantee handshake progress under continuous commits. Export
                    // still precedes cancellation and subscription, so failure has no side effects.
                    let snapshot_transaction =
                        export_snapshot(&store_snapshot).inspect_err(|error| {
                            tracing::error!(
                                %error,
                                "refusing to send an unrepresentable resync snapshot"
                            );
                        })?;
                    // The cause can become less severe while the outbound stale marker is
                    // cleared, but the already-selected full resync remains safe. Do not restart
                    // the fallback and serialize the full store without a bound.
                    (inner_guard, snapshot_transaction)
                } else {
                    drop(inner_guard);

                    let snapshot_result = export_snapshot(&store_snapshot).inspect_err(|error| {
                        tracing::error!(
                            %error,
                            "refusing to send an unrepresentable resync snapshot"
                        );
                    });

                    let inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
                    if inner_guard.store.revision().get() != snapshot_revision
                        || determine_resync_cause(&inner_guard) != Some(cause)
                    {
                        optimistic_attempts += 1;
                        drop(inner_guard);
                        continue;
                    }
                    (inner_guard, snapshot_result?)
                };

            let pending_text_edit_cancellation = if matches!(
                cause,
                ResyncCause::OutboundQueueOverflow | ResyncCause::JournalGap
            ) {
                Some(Session::prepare_pending_text_edit_cancellation(
                    &inner_guard,
                    &resume.client_instance_id,
                    &pending_text_edits,
                ))
            } else {
                None
            };
            let last_processed_event_seq = pending_text_edit_cancellation.as_ref().map_or_else(
                || {
                    inner_guard
                        .dedupe
                        .last_contiguous_processed_seq(&resume.client_instance_id)
                },
                |cancellation| cancellation.last_processed_event_seq(&resume.client_instance_id),
            );
            let discarded_text_edits = pending_text_edit_cancellation
                .as_ref()
                .map_or_else(Vec::new, |cancellation| {
                    cancellation.discarded_text_edits().to_vec()
                });
            let plan = ResumePlan::Resync {
                session_id: inner_guard.session_id.clone(),
                snapshot_revision,
                snapshot_transaction,
                continuity: cause.continuity(),
                reason: cause.reason().to_string(),
                last_processed_event_seq,
                discarded_text_edits,
                required_profiles: inner_guard.capabilities.required.to_string_vec(),
                optional_profiles: inner_guard.capabilities.optional.to_string_vec(),
                extension_namespaces: inner_guard.extension_namespaces.clone(),
                pending_text_edit_cancellation,
            };
            break (inner_guard, plan, terminal_negotiated);
        };

        before_subscribe
            .take()
            .expect("subscription callback runs once")();

        let max_resource_size = if resume.limits.is_some() {
            negotiated_max_resource_size(
                inner_guard.limits.max_resource_size,
                resume.limits.as_ref(),
            )
        } else {
            inner_guard
                .client_resource_ceilings
                .get(&resume.client_instance_id)
                .copied()
                .unwrap_or_else(|| u64::from(inner_guard.limits.max_resource_size))
        };
        let transactions = self.subscribe_client_locked(
            &mut inner_guard,
            &resume.client_instance_id,
            &resume.known_resource_hashes,
            max_resource_size,
        )?;
        inner_guard.has_negotiated = true;
        if let ResumePlan::Resync {
            pending_text_edit_cancellation,
            ..
        } = &mut plan
        {
            if let Some(cancellation) = pending_text_edit_cancellation.take() {
                cancellation.commit(&mut inner_guard);
            }
        }
        let replaced = matches!(
            &plan,
            ResumePlan::Resync {
                continuity: SessionContinuity::Replaced,
                ..
            }
        );
        drop(inner_guard);

        let terminal = self.attach_terminals(&resume.terminal_stream_offsets, replaced)?;

        let outcome = match plan {
            ResumePlan::Replay {
                welcome_msg,
                replayed,
            } => ResumeOutcome::Replay {
                welcome_msg,
                replayed,
            },
            ResumePlan::Resync {
                session_id,
                snapshot_revision,
                snapshot_transaction,
                continuity,
                reason,
                last_processed_event_seq,
                discarded_text_edits,
                required_profiles,
                optional_profiles,
                extension_namespaces,
                pending_text_edit_cancellation: _,
            } => ResumeOutcome::Resync {
                resync_msg: ServerResyncRequired {
                    session_id,
                    snapshot_revision,
                    reason,
                    continuity: continuity as i32,
                    last_processed_event_seq,
                    discarded_text_edits,
                    required_profiles,
                    optional_profiles,
                    extension_namespaces,
                },
                snapshot_transaction,
            },
        };

        Ok(ResumeClientBootstrap {
            outcome,
            transactions,
            terminal_negotiated,
            terminal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_semantic_tree::{NodeId, Revision, ServerCapabilities, StoreLimits, TypeRef};
    use std::cell::Cell;
    use std::sync::{Arc, Barrier, TryLockError};
    use std::thread;

    fn sample_hello() -> ClientHello {
        ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        }
    }

    const UNMAPPED_PROFILE: &str = "org.example.unmapped/1";

    fn session_with_unmapped_optional_profile(session_id: &str) -> Session {
        let required =
            CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).expect("required");
        let optional = CapabilitySet::from_str_slice(&[UNMAPPED_PROFILE]).expect("optional");
        Session::with_capabilities(session_id, ServerCapabilities::new(required, optional))
    }

    fn session_with_unmapped_required_profile(session_id: &str) -> Session {
        let required =
            CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1", UNMAPPED_PROFILE])
                .expect("required");
        Session::with_capabilities(
            session_id,
            ServerCapabilities::new(required, CapabilitySet::new()),
        )
    }

    fn hello_offering_unmapped_profile() -> ClientHello {
        let mut hello = sample_hello();
        hello.profiles.push(UNMAPPED_PROFILE.to_string());
        hello
    }

    fn resume_offering_unmapped_profile(session_id: String) -> ClientResume {
        ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: hello_offering_unmapped_profile().profiles,
            session_id,
            client_instance_id: vec![7, 7],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }
    }

    fn assert_unmapped_profile_rejected<T>(result: Result<T, SessionError>) {
        match result {
            Err(SessionError::InvalidConfiguration(reason)) => assert_eq!(
                reason,
                format!("advertised profile {UNMAPPED_PROFILE} has no extension namespace mapping")
            ),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("unmapped advertised extension must be rejected"),
        }
    }

    #[test]
    fn default_standard_session_does_not_negotiate_terminal_without_a_terminal_node() {
        let session = Session::new("standard-only");
        let mut hello = sample_hello();
        hello.profiles.push(Profile::terminal_v1().to_string());

        let bootstrap = session
            .bootstrap_fresh_client(&hello)
            .expect("standard-only handshake");

        assert_eq!(
            bootstrap.welcome.required_profiles,
            vec!["org.srui.standard-widgets/1"]
        );
        assert!(bootstrap.welcome.optional_profiles.is_empty());
        assert!(!bootstrap.terminal_negotiated);
        assert!(bootstrap
            .welcome
            .extension_namespaces
            .iter()
            .all(|mapping| mapping.extension_uri != Profile::terminal_v1().to_string()));
    }

    #[test]
    fn fresh_handshake_rejects_unoffered_optional_extension_without_a_namespace() {
        let session = session_with_unmapped_optional_profile("invalid-fresh-optional");

        assert_unmapped_profile_rejected(session.bootstrap_fresh_client(&sample_hello()));
        assert!(!session.inner.lock().expect("session lock").has_negotiated);
    }

    #[test]
    fn fresh_handshake_rejects_required_extension_without_a_namespace() {
        let session = session_with_unmapped_required_profile("invalid-fresh-required");

        assert_unmapped_profile_rejected(
            session.bootstrap_fresh_client(&hello_offering_unmapped_profile()),
        );
        assert!(!session.inner.lock().expect("session lock").has_negotiated);
    }

    #[test]
    fn replay_resume_rejects_advertised_extension_without_a_namespace() {
        let session = session_with_unmapped_optional_profile("invalid-replay");
        let resume = resume_offering_unmapped_profile(session.session_id());

        assert_unmapped_profile_rejected(session.bootstrap_resume(&resume));
        assert!(!session.inner.lock().expect("session lock").has_negotiated);
    }

    #[test]
    fn resync_resume_rejects_advertised_extension_without_a_namespace() {
        let session = session_with_unmapped_optional_profile("invalid-resync");
        let resume = resume_offering_unmapped_profile("replaced-session".to_string());

        assert_unmapped_profile_rejected(session.bootstrap_resume(&resume));
        assert!(!session.inner.lock().expect("session lock").has_negotiated);
    }

    #[test]
    fn namespace_contract_rejects_malformed_mapping_tables() {
        let missing_standard = Session::new("missing-standard-namespace");
        missing_standard
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .clear();
        let missing_reason = {
            let inner = missing_standard.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("missing standard namespace must be rejected");
            };
            reason
        };
        assert!(missing_reason.contains("standard widget namespace mapping is missing"));

        let wrong_standard_id = Session::new("wrong-standard-namespace-id");
        wrong_standard_id
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces[0]
            .namespace_id = 1;
        let wrong_standard_id_reason = {
            let inner = wrong_standard_id.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("wrong standard namespace ID must be rejected");
            };
            reason
        };
        assert!(wrong_standard_id_reason.contains("must use namespace_id 0"));

        let duplicate_standard = Session::new("duplicate-standard-namespace");
        duplicate_standard
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: "org.srui.standard-widgets".to_string(),
                namespace_id: 1,
            });
        let duplicate_standard_reason = {
            let inner = duplicate_standard.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("duplicate standard namespace must be rejected");
            };
            reason
        };
        assert!(duplicate_standard_reason.contains("has more than one mapping"));

        let standard_v2_alias = Session::new("standard-v2-extension-alias");
        standard_v2_alias
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: "org.srui.standard-widgets/2".to_string(),
                namespace_id: 1,
            });
        let standard_v2_alias_reason = {
            let inner = standard_v2_alias.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("standard v2 extension alias must be rejected");
            };
            reason
        };
        assert!(standard_v2_alias_reason
            .contains("standard widget profiles cannot have nonzero extension namespace aliases"));

        let malformed_uri = Session::new("malformed-extension-uri");
        malformed_uri
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: "org.example.unversioned".to_string(),
                namespace_id: 1,
            });
        let malformed_reason = {
            let inner = malformed_uri.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("unversioned extension namespace must be rejected");
            };
            reason
        };
        assert!(malformed_reason.contains("is not a versioned profile"));

        let noncanonical_uri = Session::new("noncanonical-extension-uri");
        noncanonical_uri
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: " org.example.noncanonical/1 ".to_string(),
                namespace_id: 1,
            });
        let noncanonical_reason = {
            let inner = noncanonical_uri.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("noncanonical extension namespace must be rejected");
            };
            reason
        };
        assert!(noncanonical_reason.contains("is not canonical"));

        let orphan = Session::new("orphan-extension-namespace");
        orphan
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: "org.example.orphan/1".to_string(),
                namespace_id: 1,
            });
        let orphan_reason = {
            let inner = orphan.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("orphan extension namespace must be rejected");
            };
            reason
        };
        assert!(orphan_reason.contains("is not advertised as a required or optional profile"));

        let duplicate_id = Session::new("duplicate-namespace-id");
        let shared_namespace = duplicate_id
            .register_optional_extension_profile(
                Profile::parse("org.example.one/1").expect("first profile"),
            )
            .expect("first extension namespace");
        duplicate_id
            .register_optional_extension_profile(
                Profile::parse("org.example.two/1").expect("second profile"),
            )
            .expect("second extension namespace");
        duplicate_id
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .iter_mut()
            .find(|mapping| mapping.extension_uri == "org.example.two/1")
            .expect("second mapping")
            .namespace_id = shared_namespace;
        let duplicate_id_reason = {
            let inner = duplicate_id.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("duplicate extension namespace IDs must be rejected");
            };
            reason
        };
        assert!(duplicate_id_reason.contains("duplicate extension namespace_id 1"));

        let standard_alias = Session::new("standard-extension-alias");
        standard_alias
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: Profile::standard_widgets_v1().to_string(),
                namespace_id: 1,
            });
        let standard_alias_reason = {
            let inner = standard_alias.inner.lock().expect("session lock");
            let Err(SessionError::InvalidConfiguration(reason)) =
                validate_extension_namespace_contract(&inner)
            else {
                panic!("standard profile extension alias must be rejected");
            };
            reason
        };
        assert!(standard_alias_reason
            .contains("standard widget profiles cannot have nonzero extension namespace aliases"));
    }

    #[test]
    fn namespace_contract_rejects_zero_and_duplicate_extension_mappings() {
        let zero = session_with_unmapped_optional_profile("zero-namespace");
        zero.inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .push(srui_protocol::ExtensionNamespaceMapping {
                extension_uri: UNMAPPED_PROFILE.to_string(),
                namespace_id: 0,
            });
        let Err(SessionError::InvalidConfiguration(zero_reason)) =
            zero.bootstrap_fresh_client(&hello_offering_unmapped_profile())
        else {
            panic!("zero extension namespace must be rejected");
        };
        assert!(zero_reason.contains("must use a nonzero extension namespace"));

        let duplicate = session_with_unmapped_optional_profile("duplicate-namespace");
        duplicate
            .inner
            .lock()
            .expect("session lock")
            .extension_namespaces
            .extend([
                srui_protocol::ExtensionNamespaceMapping {
                    extension_uri: UNMAPPED_PROFILE.to_string(),
                    namespace_id: 1,
                },
                srui_protocol::ExtensionNamespaceMapping {
                    extension_uri: UNMAPPED_PROFILE.to_string(),
                    namespace_id: 2,
                },
            ]);
        let Err(SessionError::InvalidConfiguration(duplicate_reason)) =
            duplicate.bootstrap_fresh_client(&hello_offering_unmapped_profile())
        else {
            panic!("duplicate extension mappings must be rejected");
        };
        assert!(duplicate_reason.contains("more than one extension namespace mapping"));
    }

    fn empty_tx(base: u64, new_rev: u64) -> Transaction {
        Transaction {
            base_revision: base,
            new_revision: new_rev,
            priority: 1,
            operations: vec![],
        }
    }

    fn pending_text_edit_ref() -> srui_protocol::PendingTextEditRef {
        srui_protocol::PendingTextEditRef {
            node_id: 2,
            edit_seq: 1,
            event_seq: 1,
            event_id: "pending-edit".into(),
        }
    }

    fn install_unrepresentable_snapshot(session: &Session) {
        let limits = StoreLimits {
            max_transaction_operations: 1,
            ..StoreLimits::default()
        };
        let mut store = SemanticStore::with_limits_and_revision(limits, Revision::new(1));
        let surface = TypeRef::new(0, 1);
        store
            .create_node(NodeId::new(1), surface, None, None, [])
            .expect("first root");
        store
            .create_node(NodeId::new(2), surface, None, None, [])
            .expect("second root");
        session.inner.lock().expect("session lock").store = store;
    }

    #[test]
    fn replay_resume_validates_pending_text_edit_refs() {
        let session = Session::new("replay-pending-validation");
        let mut invalid_ref = pending_text_edit_ref();
        invalid_ref.event_id.clear();
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: vec![8, 1],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![invalid_ref],
        };

        let Err(SessionError::InvalidInput(reason)) = session.bootstrap_resume(&resume) else {
            panic!("replay resume must validate pending TEXT_EDIT refs");
        };
        assert!(reason.contains("missing event_id"));
    }

    #[test]
    fn replaced_resume_validates_pending_text_edit_ref_bound() {
        let session = Session::new("replacement-pending-validation");
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "replaced-session".into(),
            client_instance_id: vec![8, 2],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![
                pending_text_edit_ref();
                crate::session::MAX_TEXT_EDIT_STREAMS + 1
            ],
        };

        let Err(SessionError::InvalidInput(reason)) = session.bootstrap_resume(&resume) else {
            panic!("replacement resume must enforce the pending TEXT_EDIT ref bound");
        };
        assert!(reason.contains("at most"));
    }

    #[test]
    fn failed_resume_subscription_does_not_commit_pending_edit_cancellation() {
        let session = Session::with_outbound_queue_capacity("failed-resume-cancellation", 1);
        let client = vec![8, 9];
        let _receiver = session
            .subscribe_transactions(client.clone())
            .expect("subscribe client");
        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill outbound queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow outbound queue");
        assert!(session.outbound_hub.is_client_stale(&client));
        session.close_outbound();

        let pending = pending_text_edit_ref();
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: client.clone(),
            last_applied_revision: 2,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![pending.clone()],
        };

        assert!(matches!(
            session.bootstrap_resume(&resume),
            Err(SessionError::OutboundClosed)
        ));
        let inner = session.inner.lock().unwrap();
        assert!(
            !inner
                .dedupe
                .is_duplicate(&client, pending.event_id.as_slice()),
            "a failed handshake must not settle the pending edit"
        );
    }

    #[test]
    fn pending_edit_dedupe_conflict_does_not_fail_resume() {
        let session = Session::with_outbound_queue_capacity("resume-dedupe-conflict", 1);
        let client = vec![8, 10];
        let existing = srui_protocol::Event {
            client_instance_id: client.clone(),
            event_seq: 1,
            event_id: b"existing-activate".to_vec(),
            event_type: Some(TypeRef::EVENT_ACTIVATE.into()),
            ..Default::default()
        };
        {
            let mut inner = session.inner.lock().unwrap();
            assert!(matches!(
                inner.dedupe.admit_event(&existing).unwrap(),
                srui_event_dedupe::RecordOutcome::Fresh { .. }
            ));
            inner.dedupe.settle_event(
                &existing,
                srui_event_dedupe::EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 0,
                    reject_reason: String::new(),
                },
            );
        }

        let _receiver = session
            .subscribe_transactions(client.clone())
            .expect("subscribe client");
        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill outbound queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow outbound queue");
        let mut conflicting = pending_text_edit_ref();
        conflicting.event_seq = 1;
        conflicting.event_id = b"conflicting-text-edit".to_vec();
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: client.clone(),
            last_applied_revision: 2,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![conflicting.clone()],
        };

        let bootstrap = session
            .bootstrap_resume(&resume)
            .expect("dedupe conflict must not fail resume");
        let ResumeOutcome::Resync { resync_msg, .. } = bootstrap.outcome else {
            panic!("outbound-stale client must resync");
        };
        assert_eq!(resync_msg.discarded_text_edits, vec![conflicting]);
        let inner = session.inner.lock().unwrap();
        assert!(inner
            .dedupe
            .is_duplicate(&client, existing.event_id.as_slice()));
    }

    #[test]
    fn fresh_snapshot_failure_preserves_existing_subscriber() {
        let session = Session::new("fresh-snapshot-failure");
        let hello = sample_hello();
        let mut existing = session
            .subscribe_transactions(hello.client_instance_id.clone())
            .expect("existing subscriber");
        install_unrepresentable_snapshot(&session);

        let callback_reached = Cell::new(false);
        let result = session.bootstrap_fresh_client_with(&hello, || callback_reached.set(true));
        assert!(matches!(
            result,
            Err(SessionError::SnapshotUnrepresentable {
                limit: 1,
                actual: 2
            })
        ));
        assert!(
            !callback_reached.get(),
            "snapshot export must fail before the subscription boundary"
        );
        assert!(existing.termination().is_none());

        session.outbound_hub.publish(&empty_tx(1, 2));
        let received = existing
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .expect("existing receiver remains healthy")
            .expect("published transaction")
            .into_transaction()
            .expect("UI transaction");
        assert_eq!(received, empty_tx(1, 2));
    }

    #[test]
    fn replacement_snapshot_failure_preserves_existing_subscriber() {
        let session = Session::new("replacement-snapshot-failure");
        let client = vec![8, 3];
        let mut existing = session
            .subscribe_transactions(client.clone())
            .expect("existing subscriber");
        install_unrepresentable_snapshot(&session);
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "replaced-session".into(),
            client_instance_id: client,
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        let callback_reached = Cell::new(false);
        let result = session.bootstrap_resume_with(&resume, || callback_reached.set(true));
        assert!(matches!(
            result,
            Err(SessionError::SnapshotUnrepresentable {
                limit: 1,
                actual: 2
            })
        ));
        assert!(
            !callback_reached.get(),
            "snapshot export must fail before the subscription boundary"
        );
        assert!(existing.termination().is_none());

        session.outbound_hub.publish(&empty_tx(1, 2));
        let received = existing
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .expect("existing receiver remains healthy")
            .expect("published transaction")
            .into_transaction()
            .expect("UI transaction");
        assert_eq!(received, empty_tx(1, 2));
    }

    #[test]
    fn fresh_bootstrap_skips_verified_client_resources() {
        let session = Session::new("known-fresh");
        let published = session.publish_resource(b"already-cached").unwrap();
        let mut hello = sample_hello();
        hello.known_resource_hashes = vec![published.hash.0.to_vec()];

        let mut bootstrap = session.bootstrap_fresh_client(&hello).unwrap();
        assert!(bootstrap
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .unwrap()
            .is_none());
    }

    #[test]
    fn resume_bootstrap_skips_verified_client_resources() {
        let session = Session::new("known-resume");
        let published = session.publish_resource(b"already-cached").unwrap();
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: vec![3, 4],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![published.hash.0.to_vec()],
            pending_text_edits: vec![],
        };

        let mut bootstrap = session.bootstrap_resume(&resume).unwrap();
        assert!(bootstrap
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .unwrap()
            .is_none());
    }

    #[test]
    fn resume_bootstrap_uses_readvertised_resource_limit() {
        let session = Session::new("limited-resume");
        session.publish_resource(b"larger-than-eight").unwrap();
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: vec![5, 6],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: Some(ClientLimits {
                max_resource_size: 8,
                ..ClientLimits::default()
            }),
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        let mut bootstrap = session.bootstrap_resume(&resume).unwrap();
        assert!(bootstrap
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_replaced_incarnation_precedes_outbound_overflow_stale() {
        let session = Session::with_outbound_queue_capacity("live-incarnation", 1);
        let client = vec![9, 9];
        let _rx = session
            .subscribe_transactions(client.clone())
            .expect("subscribe");

        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow");
        assert!(session.outbound_hub().is_client_stale(&client));

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "old-incarnation".to_string(),
            client_instance_id: client,
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };
        let bootstrap = session.bootstrap_resume(&resume).expect("resume");
        match bootstrap.outcome {
            ResumeOutcome::Resync { resync_msg, .. } => {
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::Replaced)
                );
                assert!(resync_msg.reason.contains("incarnation"));
            }
            other => panic!("expected Replaced resync, got {other:?}"),
        }
    }

    #[test]
    fn test_overflow_stale_survives_resume_bootstrap_until_snapshot_ack() {
        let session = Session::with_outbound_queue_capacity("overflow-session", 1);
        let client = vec![4, 2];
        let _rx = session
            .subscribe_transactions(client.clone())
            .expect("subscribe");

        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow");
        assert!(session.outbound_hub().is_client_stale(&client));

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: client.clone(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };
        let bootstrap = session.bootstrap_resume(&resume).expect("resume");
        match bootstrap.outcome {
            ResumeOutcome::Resync { resync_msg, .. } => {
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::SameSession)
                );
                assert!(resync_msg
                    .reason
                    .contains("outbound transaction queue overflowed"));
            }
            other => panic!("expected overflow resync, got {other:?}"),
        }
        assert!(
            session.outbound_hub().is_client_stale(&client),
            "stale must remain until the snapshot write completes"
        );
        session.clear_stale_client(&client);
        assert!(!session.outbound_hub().is_client_stale(&client));
    }

    #[test]
    fn test_session_lifecycle() {
        let session = Session::new("test-session");
        assert_eq!(session.session_id(), "test-session");
        assert_eq!(session.current_revision(), 0);

        let hello = sample_hello();
        let bootstrap = session
            .bootstrap_fresh_client(&hello)
            .expect("hello negotiated");
        assert_eq!(bootstrap.welcome.session_id, "test-session");
        assert_eq!(bootstrap.welcome.initial_revision, 0);
        assert!(bootstrap.snapshot.is_none());

        let tx = Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        };

        let committed = session.commit_transaction(tx).expect("commit tx");
        assert_eq!(committed.new_revision, 1);
        assert_eq!(session.current_revision(), 1);

        let bootstrap_after = session
            .bootstrap_fresh_client(&hello)
            .expect("hello after commit");
        assert_eq!(bootstrap_after.welcome.initial_revision, 1);
        let snapshot = bootstrap_after.snapshot.expect("hello catch-up snapshot");
        assert_eq!(snapshot.base_revision, 0);
        assert_eq!(snapshot.new_revision, 1);

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "test-session".to_string(),
            client_instance_id: vec![1, 2],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        match session
            .bootstrap_resume(&resume)
            .expect("resume handled")
            .outcome
        {
            ResumeOutcome::Replay {
                welcome_msg,
                replayed,
            } => {
                assert_eq!(welcome_msg.replay_from_revision, 0);
                assert_eq!(welcome_msg.last_processed_event_seq, 0);
                assert_eq!(replayed.len(), 1);
                assert_eq!(replayed[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }
    }

    #[test]
    fn test_bootstrap_resume_resync_after_journal_eviction() {
        let session = Session::new("resync-test");

        for rev in 0..1025 {
            let tx = Transaction {
                base_revision: rev,
                new_revision: rev + 1,
                priority: 1,
                operations: vec![],
            };
            session.commit_transaction(tx).expect("commit tx");
        }
        assert_eq!(session.current_revision(), 1025);

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "resync-test".to_string(),
            client_instance_id: vec![1],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        match session
            .bootstrap_resume(&resume)
            .expect("resume handled")
            .outcome
        {
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } => {
                assert_eq!(resync_msg.session_id, "resync-test");
                assert_eq!(resync_msg.snapshot_revision, 1025);
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::SameSession)
                );
                assert_eq!(resync_msg.last_processed_event_seq, 0);
                assert_eq!(snapshot_transaction.new_revision, 1025);
            }
            ResumeOutcome::Replay { .. } => panic!("expected resync, got replay"),
        }
    }

    #[test]
    fn test_bootstrap_fresh_client() {
        let session = Session::new("test-bootstrap-session");
        let hello = sample_hello();

        let mut bootstrap0 = session
            .bootstrap_fresh_client(&hello)
            .expect("bootstrap rev0");
        assert_eq!(bootstrap0.welcome.session_id, "test-bootstrap-session");
        assert_eq!(bootstrap0.welcome.initial_revision, 0);
        assert!(bootstrap0.snapshot.is_none());

        let tx = Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        };
        session.commit_transaction(tx).expect("commit tx");

        let rec_tx = bootstrap0
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .expect("receive broadcast")
            .unwrap()
            .into_transaction()
            .expect("transaction");
        assert_eq!(rec_tx.new_revision, 1);

        let bootstrap1 = session
            .bootstrap_fresh_client(&hello)
            .expect("bootstrap rev1");
        assert_eq!(bootstrap1.welcome.initial_revision, 1);
        let snapshot = bootstrap1.snapshot.expect("snapshot for rev1");
        assert_eq!(snapshot.base_revision, 0);
        assert_eq!(snapshot.new_revision, 1);
    }

    fn seed_revision_one(session: &Session) {
        session
            .commit_transaction(Transaction {
                base_revision: 0,
                new_revision: 1,
                priority: 1,
                operations: vec![],
            })
            .expect("seed revision");
    }

    #[test]
    fn test_bootstrap_fresh_client_holds_catch_up_until_subscription() {
        let session = Arc::new(Session::new("test-bootstrap-no-gap"));
        seed_revision_one(&session);

        let hello = ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 8],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        };

        let snapshot_captured = Arc::new(Barrier::new(2));
        let allow_subscription = Arc::new(Barrier::new(2));
        let bootstrap_thread = {
            let session = Arc::clone(&session);
            let snapshot_captured = Arc::clone(&snapshot_captured);
            let allow_subscription = Arc::clone(&allow_subscription);
            thread::spawn(move || {
                session.bootstrap_fresh_client_with(&hello, || {
                    snapshot_captured.wait();
                    allow_subscription.wait();
                })
            })
        };

        snapshot_captured.wait();
        assert!(
            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
            "bootstrap must retain the authoritative state lock after catch-up capture"
        );

        let commit_started = Arc::new(Barrier::new(2));
        let commit_thread = {
            let session = Arc::clone(&session);
            let commit_started = Arc::clone(&commit_started);
            thread::spawn(move || {
                assert!(
                    matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                    "the competing commit must observe bootstrap holding the state lock"
                );
                commit_started.wait();
                session.commit_transaction(Transaction {
                    base_revision: 1,
                    new_revision: 2,
                    priority: 1,
                    operations: vec![],
                })
            })
        };

        commit_started.wait();
        allow_subscription.wait();

        let mut bootstrap = bootstrap_thread
            .join()
            .expect("bootstrap thread")
            .expect("bootstrap succeeds");
        commit_thread
            .join()
            .expect("commit thread")
            .expect("commit succeeds");

        let snapshot = bootstrap.snapshot.expect("revision-one snapshot");
        assert_eq!(snapshot.new_revision, 1);

        let streamed = bootstrap
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .expect("post-snapshot transaction")
            .unwrap()
            .into_transaction()
            .expect("transaction");
        assert_eq!(streamed.base_revision, snapshot.new_revision);
        assert_eq!(streamed.new_revision, snapshot.new_revision + 1);
    }

    #[test]
    fn test_bootstrap_resume_holds_catch_up_until_subscription() {
        let session = Arc::new(Session::new("test-resume-no-gap"));
        seed_revision_one(&session);

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "test-resume-no-gap".to_string(),
            client_instance_id: vec![7, 8],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        let catch_up_captured = Arc::new(Barrier::new(2));
        let allow_subscription = Arc::new(Barrier::new(2));
        let bootstrap_thread = {
            let session = Arc::clone(&session);
            let catch_up_captured = Arc::clone(&catch_up_captured);
            let allow_subscription = Arc::clone(&allow_subscription);
            thread::spawn(move || {
                session.bootstrap_resume_with(&resume, || {
                    catch_up_captured.wait();
                    allow_subscription.wait();
                })
            })
        };

        catch_up_captured.wait();
        assert!(
            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
            "resume bootstrap must retain the authoritative state lock after catch-up capture"
        );

        let commit_started = Arc::new(Barrier::new(2));
        let commit_thread = {
            let session = Arc::clone(&session);
            let commit_started = Arc::clone(&commit_started);
            thread::spawn(move || {
                assert!(
                    matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                    "the competing commit must observe resume bootstrap holding the state lock"
                );
                commit_started.wait();
                session.commit_transaction(Transaction {
                    base_revision: 1,
                    new_revision: 2,
                    priority: 1,
                    operations: vec![],
                })
            })
        };

        commit_started.wait();
        allow_subscription.wait();

        let mut bootstrap = bootstrap_thread
            .join()
            .expect("bootstrap thread")
            .expect("bootstrap succeeds");
        commit_thread
            .join()
            .expect("commit thread")
            .expect("commit succeeds");

        match bootstrap.outcome {
            ResumeOutcome::Replay { replayed, .. } => {
                assert_eq!(replayed.len(), 1);
                assert_eq!(replayed[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }

        let streamed = bootstrap
            .transactions
            .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
            .expect("post-replay transaction")
            .unwrap()
            .into_transaction()
            .expect("transaction");
        assert_eq!(streamed.base_revision, 1);
        assert_eq!(streamed.new_revision, 2);
    }

    #[test]
    fn fresh_snapshot_export_releases_lock_and_retries_an_advanced_revision() {
        let session = Session::new("fresh-export-outside-lock");
        seed_revision_one(&session);
        let hello = sample_hello();
        let injected_commit = Cell::new(false);

        let mut bootstrap = session
            .bootstrap_fresh_client_with_exporter(
                &hello,
                || {},
                |store_snapshot| {
                    assert!(
                        session.inner.try_lock().is_ok(),
                        "snapshot serialization must not hold the session lock"
                    );
                    if !injected_commit.replace(true) {
                        session
                            .commit_transaction(empty_tx(1, 2))
                            .expect("commit while first snapshot serializes");
                    }
                    export_snapshot_transaction(store_snapshot)
                },
            )
            .expect("fresh bootstrap");

        assert!(injected_commit.get());
        let snapshot = bootstrap.snapshot.expect("snapshot");
        assert_eq!(snapshot.new_revision, 2);
        assert_eq!(bootstrap.welcome.initial_revision, 2);
        assert!(
            bootstrap
                .transactions
                .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
                .expect("inspect live queue")
                .is_none(),
            "the commit injected before subscription must be represented by the retried snapshot"
        );
    }

    #[test]
    fn fresh_snapshot_export_falls_back_after_bounded_contention() {
        let session = Session::new("fresh-export-bounded-fallback");
        seed_revision_one(&session);
        let hello = sample_hello();
        let export_calls = Cell::new(0usize);

        let bootstrap = session
            .bootstrap_fresh_client_with_exporter(
                &hello,
                || {},
                |store_snapshot| {
                    let call = export_calls.get();
                    export_calls.set(call + 1);
                    if call < MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS {
                        assert!(
                            session.inner.try_lock().is_ok(),
                            "optimistic export must not hold the session lock"
                        );
                        let revision = store_snapshot.revision().get();
                        session
                            .commit_transaction(empty_tx(revision, revision + 1))
                            .expect("advance revision during optimistic export");
                    } else {
                        assert!(
                            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                            "bounded fallback must retain the lock through serialization"
                        );
                    }
                    export_snapshot_transaction(store_snapshot)
                },
            )
            .expect("bounded fallback completes fresh bootstrap");

        assert_eq!(
            export_calls.get(),
            MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS + 1
        );
        assert_eq!(
            bootstrap.snapshot.expect("snapshot").new_revision,
            1 + MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS as u64
        );
    }

    #[test]
    fn resume_snapshot_export_releases_lock_and_retries_an_advanced_revision() {
        let session = Session::new("resume-export-outside-lock");
        seed_revision_one(&session);
        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: "replaced-incarnation".to_string(),
            client_instance_id: vec![4, 5],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };
        let injected_commit = Cell::new(false);

        let mut bootstrap = session
            .bootstrap_resume_with_exporter(
                &resume,
                || {},
                |store_snapshot| {
                    assert!(
                        session.inner.try_lock().is_ok(),
                        "snapshot serialization must not hold the session lock"
                    );
                    if !injected_commit.replace(true) {
                        session
                            .commit_transaction(empty_tx(1, 2))
                            .expect("commit while first resync snapshot serializes");
                    }
                    export_snapshot_transaction(store_snapshot)
                },
            )
            .expect("resume bootstrap");

        assert!(injected_commit.get());
        match bootstrap.outcome {
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } => {
                assert_eq!(resync_msg.snapshot_revision, 2);
                assert_eq!(snapshot_transaction.new_revision, 2);
            }
            ResumeOutcome::Replay { .. } => panic!("expected resync"),
        }
        assert!(
            bootstrap
                .transactions
                .try_recv_class(crate::outbound::LogicalChannelClass::Ui)
                .expect("inspect live queue")
                .is_none(),
            "the commit injected before subscription must be represented by the retried snapshot"
        );
    }

    #[test]
    fn resume_snapshot_export_falls_back_after_bounded_contention() {
        let session = Session::with_outbound_queue_capacity("resume-export-bounded-fallback", 1);
        seed_revision_one(&session);
        let client = vec![6, 7];
        let _receiver = session
            .subscribe_transactions(client.clone())
            .expect("subscribe client");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("fill outbound queue");
        session
            .commit_transaction(empty_tx(2, 3))
            .expect("overflow outbound queue");
        assert!(session.outbound_hub.is_client_stale(&client));

        let resume = ClientResume {
            core_version: CORE_VERSION.to_string(),
            profiles: sample_hello().profiles,
            session_id: session.session_id(),
            client_instance_id: client.clone(),
            last_applied_revision: 3,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };
        let export_calls = Cell::new(0usize);

        let bootstrap = session
            .bootstrap_resume_with_exporter(
                &resume,
                || {},
                |store_snapshot| {
                    let call = export_calls.get();
                    export_calls.set(call + 1);
                    if call < MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS {
                        assert!(
                            session.inner.try_lock().is_ok(),
                            "optimistic export must not hold the session lock"
                        );
                        let revision = store_snapshot.revision().get();
                        session
                            .commit_transaction(empty_tx(revision, revision + 1))
                            .expect("advance revision during optimistic resync export");
                    } else {
                        assert!(
                            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                            "bounded fallback must retain the lock through serialization"
                        );
                        session.clear_stale_client(&client);
                    }
                    export_snapshot_transaction(store_snapshot)
                },
            )
            .expect("bounded fallback completes resume bootstrap");

        assert_eq!(
            export_calls.get(),
            MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS + 1
        );
        match bootstrap.outcome {
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } => {
                let expected_revision = 3 + MAX_OPTIMISTIC_SNAPSHOT_EXPORT_ATTEMPTS as u64;
                assert_eq!(resync_msg.snapshot_revision, expected_revision);
                assert_eq!(snapshot_transaction.new_revision, expected_revision);
            }
            ResumeOutcome::Replay { .. } => panic!("fallback must finish the selected resync"),
        }
    }
}
