//! Extensible SRUI UI gallery (§5.2, §7.2, §7.3, §7.6, §7.7, §8, §12.1, §12.2, §14, §19.2, §23).
//!
//! # What this example is for
//!
//! One scrollable surface containing every node type the AppKit renderer can build today, a real
//! image delivered through the chunked resource path, a guided sequence of server-driven
//! mutations, and two live telemetry panels. It is the visual counterpart to the protocol tests:
//! if a semantic feature works, it is visible here.
//!
//! # Architecture & Protocol Invariants
//!
//! - **§5.2 Semantic state, not display remoting**: nothing here paints, encodes a frame, or
//!   measures a pixel. Every visible change is a property, model, or structural operation.
//! - **§12.1 Atomic transactions**: each scene change commits all-or-nothing. Built-in text edits
//!   commit authoritatively inside Session, then one instrumentation transaction reports that exact
//!   commit; ordinary gallery events remain self-describing in their mutation transaction.
//! - **§23 Incremental rendering**: scenes mutate existing nodes. `CREATE_NODE` after startup only
//!   ever appears in the structure scene, which creates one node and deletes it again.
//! - **§4 inv. 13 No silent degradation**: the gallery instantiates only node types the renderer
//!   implements, and labels the remaining interaction path that is not wired yet (tree expansion)
//!   in the UI rather than pretending it works.
//! - **§7.7 `action_key` is data**: keys are published for the client and for logs. Dispatch is by
//!   `(NodeId, TypeRef)` handler registration only; no key is ever parsed or executed.
//!
//! # Locking
//!
//! One mutex guards application state. The lock order is **state → session**, never the reverse.
//! Non-revision Session counters are read after taking the state lock and before a transaction
//! opens. The transaction's revisions are supplied from the actual Session commit critical section.

pub mod ids;
pub mod scenes;
pub mod stats;
pub mod trace;
pub mod ui;

use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;

use srui_protocol::{Event as WireEvent, Transaction as WireTransaction};
use srui_sdk::*;
use srui_semantic_tree::{Event as SemanticEvent, Transaction as SemanticTransaction};
use srui_sessiond::{Session, SessionError};

pub use scenes::{Scene, SceneContext, SCENES};
pub use stats::{Metrics, SessionFacts, SIZE_BUCKETS};
pub use trace::{Trigger, MAX_TRACE_ROWS};

/// The gallery image, published into the session resource store at startup (§14).
///
/// Embedded rather than read at runtime so the binary and the tests always agree on the bytes,
/// and so a working directory change cannot silently turn the hero image into a placeholder.
/// Provenance and licence: `assets/NOTICE.md`.
pub const GALLERY_IMAGE: &[u8] = include_bytes!("../assets/gallery.png");

/// Application state owned by the gallery server.
#[derive(Debug, Default)]
pub struct GalleryState {
    /// Scene currently applied to the graph.
    pub scene: Scene,
    /// Whether the autoplay timer is advancing scenes.
    pub autoplay: bool,
    /// Last list item the client selected, resolved against authoritative state (§8, §27).
    pub list_selection: Option<ItemId>,
    /// Last table item the client selected, resolved against authoritative state (§8, §27).
    pub table_selection: Option<ItemId>,
    /// Resource hash and transient-node allocator shared by every scene.
    pub scenes: SceneContext,
    /// Bounded protocol inspector log.
    pub trace: trace::TraceLog,
    /// Bounded traffic and latency accumulator.
    pub metrics: Metrics,
}

impl Clone for GalleryState {
    fn clone(&self) -> Self {
        Self {
            scene: self.scene,
            autoplay: self.autoplay,
            list_selection: self.list_selection,
            table_selection: self.table_selection,
            scenes: self.scenes.clone(),
            trace: self.trace.clone(),
            metrics: self.metrics.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.scene = source.scene;
        self.autoplay = source.autoplay;
        self.list_selection = source.list_selection;
        self.table_selection = source.table_selection;
        self.scenes.clone_from(&source.scenes);
        self.trace.clone_from(&source.trace);
        self.metrics.clone_from(&source.metrics);
    }
}

/// Reusable published and staging buffers for gallery-owned state.
#[derive(Debug)]
struct GalleryStates {
    published: GalleryState,
    staging: GalleryState,
}

/// Operations from a transaction committed before the gallery instrumentation transaction.
#[derive(Debug, Default)]
struct PriorTransaction {
    base_revision: Option<u64>,
    operations: Vec<Operation>,
}

impl PriorTransaction {
    fn observed(transaction: SemanticTransaction) -> Self {
        Self {
            base_revision: Some(transaction.base_revision.get()),
            operations: transaction.operations,
        }
    }
}

impl From<Vec<Operation>> for PriorTransaction {
    fn from(operations: Vec<Operation>) -> Self {
        Self {
            base_revision: None,
            operations,
        }
    }
}

/// The gallery application: a Session, deterministic state, and the handlers wired to it.
#[derive(Debug)]
pub struct GalleryApp {
    session: Arc<Session>,
    state: Mutex<GalleryStates>,
    autoplay_updates: tokio::sync::watch::Sender<bool>,
    image: Option<ResourceHash>,
}

impl GalleryApp {
    /// Publishes the gallery resource, builds the initial graph, and registers every handler.
    pub fn start(session: Arc<Session>) -> Result<Arc<Self>, SessionError> {
        let outcome = session.publish_resource(GALLERY_IMAGE)?;
        let image = outcome.hash;

        let base_revision = session.current_revision();
        let initial = ui::build_initial_ui(&session, Some(image), Scene::Baseline.label())?;

        let mut state = GalleryState {
            scenes: SceneContext::new(Some(image)),
            ..GalleryState::default()
        };
        state.metrics.observe_resource(GALLERY_IMAGE.len() as u64);
        state.metrics.observe_transaction(base_revision, &initial);

        let (autoplay_updates, _) = tokio::sync::watch::channel(state.autoplay);
        let staging = state.clone();
        let app = Arc::new(Self {
            session,
            state: Mutex::new(GalleryStates {
                published: state,
                staging,
            }),
            autoplay_updates,
            image: Some(image),
        });

        app.commit(
            Trigger::server(format!(
                "initial gallery graph · {} operations",
                initial.len()
            )),
            initial,
            None,
            |_, _| Ok(()),
        )?;

        app.register_handlers();
        Ok(app)
    }

    /// The underlying session.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Hash of the published gallery image, if publication succeeded.
    pub fn image(&self) -> Option<ResourceHash> {
        self.image
    }

    /// Runs `f` against authoritative application state.
    pub fn with_state<T>(&self, f: impl FnOnce(&GalleryState) -> T) -> T {
        let states = lock_or_recover(&self.state);
        f(&states.published)
    }

    /// Scene currently applied to the graph.
    pub fn scene(&self) -> Scene {
        lock_or_recover(&self.state).published.scene
    }

    /// Whether autoplay is advancing scenes.
    pub fn autoplay(&self) -> bool {
        lock_or_recover(&self.state).published.autoplay
    }

    /// Subscribes to autoplay changes; enabling always starts a fresh dwell interval.
    pub fn autoplay_updates(&self) -> tokio::sync::watch::Receiver<bool> {
        self.autoplay_updates.subscribe()
    }
    // =========================================================================
    // Transactions
    // =========================================================================

    /// Commits one transaction, appending the inspector rows and telemetry it implies.
    ///
    /// Prior operations may come from the bootstrap graph (trace-only) or an authoritative
    /// TEXT_EDIT carrying its exact base revision (trace and metrics).
    fn commit<F>(
        &self,
        trigger: Trigger,
        prior: impl Into<PriorTransaction>,
        started: Option<Instant>,
        f: F,
    ) -> Result<Vec<Operation>, SessionError>
    where
        F: FnOnce(&mut UiTransaction, &mut GalleryState) -> Result<(), StoreError>,
    {
        let mut states = lock_or_recover(&self.state);
        let facts = SessionFacts::capture(&self.session);
        let prior = prior.into();
        let GalleryStates { published, staging } = &mut *states;

        // Reuse staging allocations across commits. A StoreError or caught panic never swaps this
        // buffer into published, so gallery-owned state rolls back with the semantic graph.
        staging.clone_from(published);
        let result = self.session.transaction_with_result(|ui, revisions| {
            let facts = facts.with_transaction(revisions);
            f(ui, staging)?;

            if let Some(base_revision) = prior.base_revision {
                staging
                    .metrics
                    .observe_transaction(base_revision, &prior.operations);
            }

            // Snapshot before inspector writes so the inspector never describes itself.
            let mut described = prior.operations;
            described.extend_from_slice(ui.operations());
            staging.trace.record(ui, &trigger, &described)?;

            Text::set_text_for(ui, ids::INSPECT_LAST_EVENT, trigger.inspector_text())?;
            Text::set_text_for(
                ui,
                ids::INSPECT_REVISION,
                format!("Revision: {}", revisions.committed_revision),
            )?;

            if let (
                Some(started),
                Trigger::Event {
                    observed_revision, ..
                },
            ) = (started, &trigger)
            {
                let event_base_revision = prior.base_revision.unwrap_or(revisions.base_revision);
                let lag = event_base_revision.saturating_sub(*observed_revision);
                staging.metrics.observe_event(started.elapsed(), lag);
            }

            staging.metrics.render(ui, &facts)?;
            Ok(ui.operations().to_vec())
        })?;

        // Framed size is known only once the operation list is final. The panel therefore renders
        // prior commits, then this exact transaction is included in the next render.
        staging
            .metrics
            .observe_transaction(result.transaction.base_revision, &result.value);
        let autoplay_changed = published.autoplay != staging.autoplay;
        std::mem::swap(published, staging);
        let autoplay = published.autoplay;
        drop(states);

        if autoplay_changed {
            self.autoplay_updates.send_replace(autoplay);
        }

        Ok(result.value)
    }

    /// Commits a server-initiated transaction described by label.
    pub fn mutate<F>(&self, label: impl Into<String>, f: F) -> Result<Vec<Operation>, SessionError>
    where
        F: FnOnce(&mut UiTransaction, &mut GalleryState) -> Result<(), StoreError>,
    {
        self.commit(Trigger::server(label), Vec::new(), None, f)
    }

    // =========================================================================
    // Scene navigation
    // =========================================================================

    /// Applies `target`, reverting whatever scene is currently applied first.
    ///
    /// Reverting before applying is what makes the tour path-independent: the graph after this
    /// call depends only on `target`, never on the route taken to it.
    pub fn goto_scene(&self, target: Scene) -> Result<Vec<Operation>, SessionError> {
        self.goto_scene_traced(
            target,
            Trigger::server(format!("scene \u{2192} {}", target.name())),
            None,
        )
    }

    fn goto_scene_traced(
        &self,
        target: Scene,
        trigger: Trigger,
        started: Option<Instant>,
    ) -> Result<Vec<Operation>, SessionError> {
        self.commit(trigger, Vec::new(), started, move |ui, state| {
            Self::stage_scene(ui, state, target)
        })
    }

    fn step_scene_traced(
        &self,
        step: fn(Scene) -> Scene,
        trigger: Trigger,
        started: Option<Instant>,
    ) -> Result<Vec<Operation>, SessionError> {
        self.commit(trigger, Vec::new(), started, move |ui, state| {
            // Derive a relative target only after commit has acquired the state lock. Concurrent
            // Next/Previous actions must each advance from the state committed before them.
            let target = step(state.scene);
            Self::stage_scene(ui, state, target)
        })
    }

    fn stage_scene(
        ui: &mut UiTransaction,
        state: &mut GalleryState,
        target: Scene,
    ) -> Result<(), StoreError> {
        if state.scene == target {
            return Ok(());
        }
        state.scene.revert(ui, &mut state.scenes)?;
        target.apply(ui, &mut state.scenes)?;
        state.scene = target;
        Text::set_text_for(ui, ids::SCENE_LABEL, target.label())?;
        Text::set_text_for(ui, ids::INSPECT_SCENE, target.label())?;
        Ok(())
    }

    /// Advances one scene, wrapping back to the baseline.
    pub fn next_scene(&self) -> Result<Vec<Operation>, SessionError> {
        self.step_scene_traced(Scene::next, Trigger::server("next scene"), None)
    }

    /// Steps back one scene, wrapping to the last scene.
    pub fn previous_scene(&self) -> Result<Vec<Operation>, SessionError> {
        self.step_scene_traced(Scene::previous, Trigger::server("previous scene"), None)
    }

    /// Refreshes session-derived connection telemetry after an attach or detach transition.
    pub fn refresh_connection_telemetry(&self) -> Result<Vec<Operation>, SessionError> {
        self.mutate("connection lifecycle changed", |_, _| Ok(()))
    }

    /// Reverts the applied scene and restores every baseline value the gallery owns.
    ///
    /// Node, model, and item identities are preserved: this is a revert, not a rebuild.
    pub fn reset(&self) -> Result<Vec<Operation>, SessionError> {
        self.commit(
            Trigger::server("reset to baseline"),
            Vec::new(),
            None,
            Self::restore_baseline,
        )
    }

    fn restore_baseline(
        ui: &mut UiTransaction,
        state: &mut GalleryState,
    ) -> Result<(), StoreError> {
        state.scene.revert(ui, &mut state.scenes)?;
        state.scene = Scene::Baseline;
        state.autoplay = ui::BASELINE_AUTOPLAY;
        state.list_selection = None;
        state.table_selection = None;

        Text::set_text_for(ui, ids::SCENE_LABEL, Scene::Baseline.label())?;
        Text::set_text_for(ui, ids::INSPECT_SCENE, Scene::Baseline.label())?;
        Text::set_text_for(ui, ids::CTRL_STATUS, ui::BASELINE_CTRL_STATUS)?;
        Text::set_text_for(ui, ids::COLL_SELECTION, ui::BASELINE_COLL_SELECTION)?;
        Text::set_text_for(ui, ids::HERO_STATUS, ui::BASELINE_HERO_STATUS)?;
        for (node, value) in [
            (ids::TOGGLE_AUTOPLAY, ui::BASELINE_AUTOPLAY),
            (ids::TOGGLE_CHECKBOX, ui::BASELINE_TOGGLE_CHECKBOX),
            (ids::TOGGLE_SWITCH, ui::BASELINE_TOGGLE_SWITCH),
            (ids::TOGGLE_AUTOMATIC, ui::BASELINE_TOGGLE_AUTOMATIC),
        ] {
            Toggle::set_value_for(ui, node, value)?;
        }
        for (node, value) in [
            (ids::INPUT_PLAIN, ui::BASELINE_INPUT_PLAIN),
            (ids::INPUT_SEARCH, ui::BASELINE_INPUT_SEARCH),
            (ids::INPUT_SECURE, ui::BASELINE_INPUT_SECURE),
            (ids::INPUT_COMMAND, ui::BASELINE_INPUT_COMMAND),
            (ids::INPUT_INVALID, ui::BASELINE_INPUT_INVALID),
        ] {
            TextInput::set_value_for(ui, node, value)?;
            TextInput::set_validation_state_for(ui, node, ValidationState::Valid)?;
        }
        TextArea::set_value_for(ui, ids::TEXT_AREA, ui::BASELINE_TEXT_AREA)?;
        TextArea::set_validation_state_for(ui, ids::TEXT_AREA, ValidationState::Valid)?;
        Ok(())
    }

    /// Turns the autoplay timer on or off and echoes the authoritative value back to the client.
    pub fn set_autoplay(&self, enabled: bool) -> Result<Vec<Operation>, SessionError> {
        self.mutate(format!("autoplay \u{2192} {enabled}"), move |ui, state| {
            state.autoplay = enabled;
            Toggle::set_value_for(ui, ids::TOGGLE_AUTOPLAY, enabled)?;
            Ok(())
        })
    }

    // =========================================================================
    // Event handlers (§7.6, §7.7, §29)
    // =========================================================================

    fn register_handlers(self: &Arc<Self>) {
        for (node, scene_action) in [
            (ids::BTN_PREV, SceneAction::Previous),
            (ids::BTN_NEXT, SceneAction::Next),
            (ids::BTN_RESET, SceneAction::Reset),
        ] {
            let target: Weak<Self> = Arc::downgrade(self);
            self.session.on_result(node, ACTIVATE, move |_, event| {
                let app = target.upgrade().ok_or_else(handler_target_unavailable)?;
                app.on_scene_action(scene_action, event)
            });
        }

        let autoplay: Weak<Self> = Arc::downgrade(self);
        self.session
            .on_result(ids::TOGGLE_AUTOPLAY, VALUE_CHANGED, move |_, event| {
                let app = autoplay.upgrade().ok_or_else(handler_target_unavailable)?;
                app.on_autoplay_changed(event)
            });

        for (node, label) in [
            (ids::BTN_NORMAL, "Normal"),
            (ids::BTN_PRIMARY, "Primary"),
            (ids::BTN_DESTRUCTIVE, "Destructive"),
            (ids::BTN_QUIET, "Quiet"),
        ] {
            let target: Weak<Self> = Arc::downgrade(self);
            self.session.on_result(node, ACTIVATE, move |_, event| {
                let app = target.upgrade().ok_or_else(handler_target_unavailable)?;
                app.on_demo_button(label, event)
            });
        }

        for (node, label) in [
            (ids::TOGGLE_CHECKBOX, "Checkbox hint"),
            (ids::TOGGLE_SWITCH, "Switch hint"),
            (ids::TOGGLE_AUTOMATIC, "Automatic hint"),
        ] {
            let target: Weak<Self> = Arc::downgrade(self);
            self.session
                .on_result(node, VALUE_CHANGED, move |_, event| {
                    let app = target.upgrade().ok_or_else(handler_target_unavailable)?;
                    app.on_demo_toggle(node, label, event)
                });
        }

        for node in [
            ids::INPUT_PLAIN,
            ids::INPUT_SEARCH,
            ids::INPUT_SECURE,
            ids::INPUT_COMMAND,
            ids::INPUT_INVALID,
            ids::TEXT_AREA,
        ] {
            let target: Weak<Self> = Arc::downgrade(self);
            self.session
                .on_result_with_transaction(node, TEXT_EDIT, move |_, event, committed| {
                    let app = target.upgrade().ok_or_else(handler_target_unavailable)?;
                    let committed = committed.ok_or_else(|| {
                        SessionError::InvalidConfiguration(
                            "accepted gallery TEXT_EDIT did not carry its transaction".to_string(),
                        )
                    })?;
                    app.on_text_edit(event, committed)
                });
        }

        for (node, collection) in [
            (ids::LIST, Collection::List),
            (ids::TABLE, Collection::Table),
        ] {
            let target: Weak<Self> = Arc::downgrade(self);
            self.session
                .on_result(node, SELECTION_CHANGED, move |_, event| {
                    let app = target.upgrade().ok_or_else(handler_target_unavailable)?;
                    app.on_selection_changed(collection, event)
                });
        }
    }

    fn on_text_edit(
        &self,
        event: &WireEvent,
        committed: &WireTransaction,
    ) -> Result<(), SessionError> {
        let started = Instant::now();
        let transaction = SemanticTransaction::try_from(committed.clone())?;
        let edited_bytes = decode_semantic_event(event)
            .and_then(|event| event.text_arg().map(str::len))
            .unwrap_or(0);
        let trigger = event_trigger(
            event,
            format!("authoritative text edit · {edited_bytes} UTF-8 bytes"),
        );
        self.commit(
            trigger,
            PriorTransaction::observed(transaction),
            Some(started),
            |_, _| Ok(()),
        )
        .map(|_| ())
    }

    fn on_scene_action(&self, action: SceneAction, event: &WireEvent) -> Result<(), SessionError> {
        let started = Instant::now();
        let trigger = event_trigger(event, action.detail());
        let result = match action {
            SceneAction::Next => self.step_scene_traced(Scene::next, trigger, Some(started)),
            SceneAction::Previous => {
                self.step_scene_traced(Scene::previous, trigger, Some(started))
            }
            SceneAction::Reset => {
                self.commit(trigger, Vec::new(), Some(started), Self::restore_baseline)
            }
        };
        result.map(|_| ())
    }

    fn on_autoplay_changed(&self, event: &WireEvent) -> Result<(), SessionError> {
        let started = Instant::now();
        let enabled = decode_bool(event).ok_or_else(|| {
            SessionError::InvalidInput(
                "VALUE_CHANGED on the autoplay toggle requires a bool value".to_string(),
            )
        })?;
        let trigger = event_trigger(event, format!("value = {enabled}"));
        self.commit(trigger, Vec::new(), Some(started), move |ui, state| {
            state.autoplay = enabled;
            Toggle::set_value_for(ui, ids::TOGGLE_AUTOPLAY, enabled)?;
            Ok(())
        })
        .map(|_| ())
    }

    fn on_demo_button(&self, label: &'static str, event: &WireEvent) -> Result<(), SessionError> {
        let started = Instant::now();
        let trigger = event_trigger(event, format!("button \"{label}\""));
        let seq = event.event_seq;
        self.commit(trigger, Vec::new(), Some(started), move |ui, _| {
            Text::set_text_for(
                ui,
                ids::CTRL_STATUS,
                format!("ACTIVATE on the {label} button · client event seq {seq}"),
            )?;
            Ok(())
        })
        .map(|_| ())
    }

    fn on_demo_toggle(
        &self,
        node: NodeId,
        label: &'static str,
        event: &WireEvent,
    ) -> Result<(), SessionError> {
        let started = Instant::now();
        let value = decode_bool(event).ok_or_else(|| {
            SessionError::InvalidInput(format!(
                "VALUE_CHANGED on node {} requires a bool value",
                node.get()
            ))
        })?;
        let trigger = event_trigger(event, format!("value = {value}"));
        self.commit(trigger, Vec::new(), Some(started), move |ui, _| {
            // The server is authoritative: it echoes the value back rather than trusting that the
            // client's local view already matches (§7.7).
            Toggle::set_value_for(ui, node, value)?;
            Text::set_text_for(
                ui,
                ids::CTRL_STATUS,
                format!("VALUE_CHANGED on the {label} toggle · now {value}"),
            )?;
            Ok(())
        })
        .map(|_| ())
    }

    fn on_selection_changed(
        &self,
        collection: Collection,
        event: &WireEvent,
    ) -> Result<(), SessionError> {
        let started = Instant::now();
        let item = decode_item_id(event).ok_or_else(|| {
            SessionError::InvalidInput("SELECTION_CHANGED requires an item id value".to_string())
        })?;
        let model = collection.model();
        let trigger = event_trigger(event, format!("item {}", item.get()));

        self.commit(trigger, Vec::new(), Some(started), move |ui, state| {
            // Only the item id is trusted, and only if authoritative state still holds it. Row
            // text, index, and `action_key` from the client are never consulted (§7.7, §27).
            let resolved = ui
                .get_model(model)
                .and_then(|model| model.get_item_by_id(item))
                .map(|item| item.value.clone());

            let text = match (&resolved, collection) {
                (Some(value), Collection::List) => {
                    format!(
                        "List selection \u{b7} item {} \u{b7} {}",
                        item.get(),
                        summarize(value)
                    )
                }
                (Some(value), Collection::Table) => {
                    format!(
                        "Table selection \u{b7} item {} \u{b7} {}",
                        item.get(),
                        summarize(value)
                    )
                }
                (None, _) => format!(
                    "Selection refused \u{b7} item {} is not in authoritative state",
                    item.get()
                ),
            };

            if resolved.is_some() {
                match collection {
                    Collection::List => state.list_selection = Some(item),
                    Collection::Table => state.table_selection = Some(item),
                }
            }
            Text::set_text_for(ui, ids::COLL_SELECTION, text)?;
            Ok(())
        })
        .map(|_| ())
    }
}

/// Which toolbar button was activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SceneAction {
    Previous,
    Next,
    Reset,
}

impl SceneAction {
    fn detail(self) -> String {
        match self {
            Self::Previous => "previous scene".to_string(),
            Self::Next => "next scene".to_string(),
            Self::Reset => "reset".to_string(),
        }
    }
}

/// Which model-backed collection reported a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collection {
    List,
    Table,
}

impl Collection {
    fn model(self) -> ModelId {
        match self {
            Self::List => ids::LIST_MODEL,
            Self::Table => ids::TABLE_MODEL,
        }
    }
}

/// Renders a model item value for the selection status line.
fn summarize(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::List(cells) => cells
            .iter()
            .map(|cell| match cell {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(" \u{b7} "),
        other => other.to_string(),
    }
}

fn event_trigger(event: &WireEvent, detail: String) -> Trigger {
    Trigger::Event {
        node: NodeId::new(event.node_id),
        event_type: event
            .event_type
            .as_ref()
            .map(|type_ref| TypeRef::new(type_ref.namespace_id, type_ref.local_id))
            .unwrap_or_else(|| TypeRef::standard(0)),
        event_seq: event.event_seq,
        observed_revision: event.observed_revision,
        detail,
    }
}

fn decode_semantic_event(event: &WireEvent) -> Option<SemanticEvent> {
    SemanticEvent::try_from(event.clone()).ok()
}

fn decode_bool(event: &WireEvent) -> Option<bool> {
    match decode_semantic_event(event)?.value_arg() {
        Some(Value::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn decode_item_id(event: &WireEvent) -> Option<ItemId> {
    match decode_semantic_event(event)?.value_arg() {
        Some(Value::ItemId(item)) => Some(*item),
        Some(Value::UnsignedInt(raw)) => Some(ItemId::new(*raw)),
        _ => None,
    }
}

fn handler_target_unavailable() -> SessionError {
    SessionError::InvalidConfiguration("gallery event handler target is unavailable".to_string())
}

/// Recovers a poisoned mutex rather than propagating the panic.
///
/// A panic inside a transaction closure is already converted to `SessionError::Panicked` by the
/// session, so a poisoned application-state lock means state may be stale, never torn: refusing to
/// serve any further event would be a worse outcome than continuing.
pub fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
