//! Ergonomic Session API, transaction coordination, and in-process event dispatching (§6.1, §7.6, §7.7, §12.1, §12.2, §13, §27, §29).
//!
//! # Architecture & Design Invariants
//!
//! - **§29 Reference Server SDK Contract**: The application developer interacts with high-level
//!   semantic transactions (`session.transaction(|ui| { ... })`) and event handlers (`session.on(node, EVENT, handler)`).
//!   The SDK automatically manages transaction lifecycle, speculative staging, atomicity, revision advancement,
//!   and event validation without exposing base/new revision bookkeeping to the caller.
//! - **§12.1 Atomic Transactions & Revision Advancement**: Transactions are strictly all-or-nothing.
//!   A transaction operates on a private speculative staging store. If the transaction closure returns an error
//!   or panics, all mutations are discarded and the store is left in its exact pre-transaction state without side effects.
//!   Upon successful completion, mutations are committed atomically, advancing the store's revision by exactly one (`base + 1`).
//! - **§12.2 Commits Are Not Frames**: Commits establish semantic state-consistency boundaries, not render frames or display ticks.
//! - **§7.7 Semantic Input Routing & §27 Server Validation**: Incoming semantic events are validated against the
//!   authoritative store (verifying target node existence, interactive enabled state, and revision freshness)
//!   before dispatching to registered in-process handlers.
//! - **Lock-Safety & Reentrancy**: Handler invocations are executed outside internal mutex locks so handlers can freely
//!   execute nested transactions (`session.transaction(...)`) or dispatch further events without risk of deadlock.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};

use srui_semantic_tree::{
    Event, EventValidationError, Node, NodeId, Operation, PropertyRef, Revision,
    SemanticStore, StoreError, TxnError, TypeRef, Value,
};
use thiserror::Error;

/// Errors produced by the SRUI SDK Session and Transaction subsystem.
#[derive(Debug, Error)]
pub enum SdkError {
    /// Semantic store operation failure (§13, §26).
    #[error("store error: {0}")]
    Store(#[from] StoreError),

    /// Transaction lifecycle or limit violation (§12.1, §26).
    #[error("transaction error: {0}")]
    Transaction(#[from] TxnError),

    /// Event semantic validation failure (§7.7, §27).
    #[error("event validation error: {0}")]
    EventValidation(#[from] EventValidationError),

    /// Error returned by a user transaction closure.
    #[error("transaction closure failed: {0}")]
    User(String),

    /// Internal mutex lock was poisoned.
    #[error("session mutex lock poisoned")]
    LockPoisoned,

    /// Transaction closure panicked during execution.
    #[error("transaction panicked: {0}")]
    Panicked(String),
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Transactional UI context passed into [`Session::transaction`] closures (§12.1, §29).
///
/// `UiTransaction` wraps a speculative staging copy of the [`SemanticStore`].
/// Because it implements [`Deref<Target = SemanticStore>`] and [`DerefMut<Target = SemanticStore>`],
/// any typed widget builder or mutator (from Task 9's typed widget layer) can be called directly
/// on `ui` (e.g. `Button::builder(id).create(ui)`, `btn.set_label(ui, "...")`).
///
/// It also provides §29 convenience helpers such as [`UiTransaction::set`], [`UiTransaction::clear`],
/// and [`UiTransaction::delete`].
#[derive(Debug)]
pub struct UiTransaction {
    staged: SemanticStore,
    op_count: usize,
    max_ops: usize,
}

impl UiTransaction {
    /// Constructs a new `UiTransaction` wrapping a staging store.
    pub(crate) fn new(staged: SemanticStore, max_ops: usize) -> Self {
        Self {
            staged,
            op_count: 0,
            max_ops,
        }
    }

    /// Records an operation, enforcing the `max_transaction_operations` limit (§26).
    fn record_op(&mut self) -> Result<(), StoreError> {
        self.op_count += 1;
        if self.op_count > self.max_ops {
            return Err(StoreError::OperationError(format!(
                "maximum transaction operations exceeded (limit: {}, actual: {})",
                self.max_ops, self.op_count
            )));
        }
        Ok(())
    }

    /// Returns the number of mutations performed within this transaction.
    #[inline]
    pub fn op_count(&self) -> usize {
        self.op_count
    }

    /// Consumes this context and returns the staging store.
    pub(crate) fn into_staged(self) -> SemanticStore {
        self.staged
    }

    /// Sets a property on a node within the active transaction (§13 SET_PROPERTY, §29).
    ///
    /// # Example
    ///
    /// ```rust
    /// # use srui_sdk::*;
    /// # let session = Session::new("test");
    /// # let btn = NodeId::new(1);
    /// # session.transaction(|ui| {
    /// #     Surface::builder(btn).create(ui)?;
    /// ui.set(btn, LABEL, "Submit")?;
    /// #     Ok(())
    /// # }).unwrap();
    /// ```
    pub fn set(
        &mut self,
        node: impl Into<NodeId>,
        prop: PropertyRef,
        val: impl Into<Value>,
    ) -> Result<Option<Value>, StoreError> {
        self.record_op()?;
        self.staged.set_property(node.into(), prop, val.into())
    }

    /// Clears a property from a node within the active transaction (§13 CLEAR_PROPERTY, §29).
    pub fn clear(
        &mut self,
        node: impl Into<NodeId>,
        prop: PropertyRef,
    ) -> Result<Option<Value>, StoreError> {
        self.record_op()?;
        self.staged.clear_property(node.into(), prop)
    }

    /// Creates a new node in the graph within the active transaction (§13 CREATE_NODE).
    pub fn create_node(
        &mut self,
        id: impl Into<NodeId>,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Result<(), StoreError> {
        self.record_op()?;
        self.staged
            .create_node(id.into(), node_type, parent_id, child_index, properties)
    }

    /// Deletes a node and all of its descendants within the active transaction (§13 DELETE_NODE).
    pub fn delete(&mut self, node: impl Into<NodeId>) -> Result<Vec<NodeId>, StoreError> {
        self.record_op()?;
        self.staged.delete_node(node.into())
    }

    /// Moves a node to a new parent and/or child index within the active transaction (§13 MOVE_NODE).
    pub fn move_node(
        &mut self,
        node: impl Into<NodeId>,
        new_parent_id: Option<NodeId>,
        new_child_index: Option<usize>,
    ) -> Result<(), StoreError> {
        self.record_op()?;
        self.staged
            .move_node(node.into(), new_parent_id, new_child_index)
    }

    /// Reorders the children of a parent node within the active transaction (§13 REORDER_CHILDREN).
    pub fn reorder_children(
        &mut self,
        parent: impl Into<NodeId>,
        new_order: &[NodeId],
    ) -> Result<(), StoreError> {
        self.record_op()?;
        self.staged.reorder_children(parent.into(), new_order)
    }

    /// Applies a low-level mutation [`Operation`] to the staging store (§13).
    pub fn apply_op(&mut self, op: &Operation) -> Result<(), StoreError> {
        self.record_op()?;
        op.apply(&mut self.staged)
    }
}

impl Deref for UiTransaction {
    type Target = SemanticStore;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.staged
    }
}

impl DerefMut for UiTransaction {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.staged
    }
}

/// Type alias for event handler callbacks (§29).
pub type HandlerFn = Arc<dyn Fn(&Session, &Event) + Send + Sync + 'static>;

struct SessionInner {
    session_id: String,
    store: SemanticStore,
    handlers: HashMap<(NodeId, TypeRef), Vec<HandlerFn>>,
}

impl std::fmt::Debug for SessionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInner")
            .field("session_id", &self.session_id)
            .field("store", &self.store)
            .field("handler_count", &self.handlers.len())
            .finish()
    }
}

/// Authoritative server-side semantic session controller (§6.1, §6.3, §12.1, §27, §29).
///
/// `Session` provides the primary application-facing API for SRUI applications:
/// - Opening atomic mutation transactions: [`Session::transaction`]
/// - Registering semantic event listeners: [`Session::on`]
/// - In-process event delivery: [`Session::dispatch`]
/// - Querying committed semantic state: [`Session::with_store`], [`Session::current_revision`], [`Session::get_node`]
///
/// `Session` is cheaply cloneable (`Arc`-backed) and thread-safe (`Send + Sync`).
#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<Mutex<SessionInner>>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new("default")
    }
}

impl Session {
    /// Constructs a new semantic session with the given session ID and default limits (§6.1, §26).
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_custom_store(session_id, SemanticStore::new())
    }

    /// Constructs a new semantic session with a pre-configured [`SemanticStore`].
    pub fn with_custom_store(session_id: impl Into<String>, store: SemanticStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionInner {
                session_id: session_id.into(),
                store,
                handlers: HashMap::new(),
            })),
        }
    }

    /// Returns the session ID string (§6.1).
    pub fn session_id(&self) -> String {
        let guard = lock_or_recover(&self.inner);
        guard.session_id.clone()
    }

    /// Returns the current committed revision of the session graph (§12.1).
    pub fn current_revision(&self) -> Revision {
        let guard = lock_or_recover(&self.inner);
        guard.store.revision()
    }

    /// Returns the total number of active nodes currently in the store (§6.2).
    pub fn node_count(&self) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard.store.node_count()
    }

    /// Returns `true` if an active node exists with the given ID.
    pub fn contains_node(&self, node: impl Into<NodeId>) -> bool {
        let guard = lock_or_recover(&self.inner);
        guard.store.contains_node(node.into())
    }

    /// Returns a clone of the node with the given ID, if it exists in the store.
    pub fn get_node(&self, node: impl Into<NodeId>) -> Option<Node> {
        let guard = lock_or_recover(&self.inner);
        guard.store.get_node(node.into()).cloned()
    }

    /// Returns a list of top-level root node IDs.
    pub fn root_ids(&self) -> Vec<NodeId> {
        let guard = lock_or_recover(&self.inner);
        guard.store.root_ids().to_vec()
    }

    /// Executes a read-only query closure against the committed [`SemanticStore`].
    pub fn with_store<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&SemanticStore) -> T,
    {
        let guard = lock_or_recover(&self.inner);
        f(&guard.store)
    }

    /// Opens an atomic semantic transaction advancing the graph from revision `N` to `N + 1` (§12.1, §29).
    ///
    /// The closure `f(&mut ui)` receives a [`UiTransaction`] context, allowing any typed widget builder
    /// or mutator to be invoked.
    ///
    /// # Transaction Semantics
    ///
    /// 1. A private speculative staging store is opened.
    /// 2. The closure `f(&mut ui)` performs mutations via typed widget mutators or `ui.set(...)` methods.
    /// 3. If `f` returns `Ok(val)`:
    ///    - The transaction is committed atomically.
    ///    - The store's revision advances by exactly 1 (`base_revision.next()`).
    ///    - `Ok(val)` is returned.
    /// 4. If `f` returns `Err(e)`:
    ///    - All staged mutations are discarded.
    ///    - The store remains in its exact pre-transaction state without side effects.
    ///    - `Err(SdkError::Store(...))` is returned.
    /// 5. If `f` panics:
    ///    - The panic is caught via unwind safety, discarding all staged mutations.
    ///    - The store remains in its exact pre-transaction state and revision.
    ///    - `Err(SdkError::Panicked(...))` is returned.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use srui_sdk::*;
    /// let session = Session::new("test");
    /// let progress = NodeId::new(1);
    /// let status = NodeId::new(2);
    ///
    /// session.transaction(|ui| {
    ///     Progress::builder(progress).create(ui)?;
    ///     Text::builder(status).create(ui)?;
    ///     ui.set(progress, VALUE, 0.72)?;
    ///     ui.set(status, TEXT, "Running tests")?;
    ///     Ok(())
    /// }).expect("transaction failed");
    ///
    /// assert_eq!(session.current_revision().get(), 1);
    /// ```
    /// # Deadlock / Reentrancy Notice
    ///
    /// The session lock is held for the duration of the transaction closure to guarantee atomic staging.
    /// Transactions are therefore non-reentrant: calling `session.transaction` from within another
    /// `session.transaction` closure on the same session will result in a deadlock.
    /// Handlers registered via [`Session::on`] are executed outside the lock and can safely call `session.transaction`.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, SdkError>
    where
        F: FnOnce(&mut UiTransaction) -> Result<T, StoreError>,
    {
        let mut guard = self.inner.lock().map_err(|_| SdkError::LockPoisoned)?;
        let base_revision = guard.store.revision();
        let max_ops = guard.store.limits().max_transaction_operations;
        let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut ui)));

        match result {
            Ok(Ok(val)) => {
                let new_rev = base_revision.next();
                guard.store.commit_staging(ui.into_staged(), new_rev);
                Ok(val)
            }
            Ok(Err(store_err)) => {
                // Staged mutations dropped; store remains untouched at base_revision
                Err(SdkError::Store(store_err))
            }
            Err(panic_payload) => {
                let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Err(SdkError::Panicked(panic_msg))
            }
        }
    }

    /// Opens an atomic semantic transaction with a custom error type (§12.1, §29).
    pub fn transaction_custom<T, E, F>(&self, f: F) -> Result<T, SdkError>
    where
        F: FnOnce(&mut UiTransaction) -> Result<T, E>,
        E: std::fmt::Display,
    {
        let mut guard = self.inner.lock().map_err(|_| SdkError::LockPoisoned)?;
        let base_revision = guard.store.revision();
        let max_ops = guard.store.limits().max_transaction_operations;
        let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);

        // Execute closure inside catch_unwind to ensure atomicity even if the closure panics
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut ui)));

        match result {
            Ok(Ok(val)) => {
                let new_rev = base_revision.next();
                guard.store.commit_staging(ui.into_staged(), new_rev);
                Ok(val)
            }
            Ok(Err(user_err)) => {
                // Staged mutations dropped; store remains untouched at base_revision
                Err(SdkError::User(user_err.to_string()))
            }
            Err(panic_payload) => {
                // Panicked closure: staged mutations dropped; store remains untouched
                let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                Err(SdkError::Panicked(panic_msg))
            }
        }
    }

    /// Convenience wrapper for running an infallible transaction closure (§12.1, §29).
    pub fn transaction_sync<T, F>(&self, f: F) -> Result<T, SdkError>
    where
        F: FnOnce(&mut UiTransaction) -> T,
    {
        self.transaction(|ui| Ok(f(ui)))
    }

    /// Registers an in-process semantic event handler for the given node and event type (§7.6, §29).
    ///
    /// When a matching [`Event`] is delivered via [`Session::dispatch`], `handler(&Session, &Event)`
    /// is invoked. Handlers are executed without holding internal mutex locks, enabling handlers
    /// to safely open new transactions or dispatch further events.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use srui_sdk::*;
    /// let session = Session::new("test");
    /// let button = NodeId::new(1);
    ///
    /// session.transaction(|ui| {
    ///     Button::builder(button).label("Approve").create(ui)?;
    ///     Ok(())
    /// }).unwrap();
    ///
    /// session.on(button, ACTIVATE, |ctx, event| {
    ///     ctx.transaction(|ui| {
    ///         ui.set(event.node_id, LABEL, "Approved!")?;
    ///         Ok(())
    ///     }).unwrap();
    /// });
    /// ```
    pub fn on<F>(&self, node: impl Into<NodeId>, event_type: TypeRef, handler: F)
    where
        F: Fn(&Session, &Event) + Send + Sync + 'static,
    {
        let mut guard = lock_or_recover(&self.inner);
        guard
            .handlers
            .entry((node.into(), event_type))
            .or_default()
            .push(Arc::new(handler));
    }

    /// Returns the number of registered handlers for a specific node and event type.
    pub fn handler_count(&self, node: impl Into<NodeId>, event_type: TypeRef) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard
            .handlers
            .get(&(node.into(), event_type))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Clears all registered event handlers from this session.
    pub fn clear_handlers(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.handlers.clear();
    }

    /// Delivers an incoming semantic event to the session (§7.7, §27, §29).
    ///
    /// # Execution Flow
    ///
    /// 1. Validates the event against the current graph:
    ///    - Verifies the target `node_id` exists in the store ([`EventValidationError::NodeNotFound`]).
    ///    - Verifies the target node is interactive / not disabled ([`EventValidationError::NodeDisabled`]).
    ///    - Verifies `observed_revision <= current_revision` ([`EventValidationError::FutureRevision`]).
    /// 2. Locates matching registered handlers for `(event.node_id, event.event_type)`.
    /// 3. Releases the internal session lock.
    /// 4. Invokes matching handlers sequentially with `(&Session, &Event)`.
    ///
    /// Returns `Ok(usize)` indicating the number of matching handlers executed, or `Err(SdkError)`
    /// if event validation failed.
    pub fn dispatch(&self, event: impl Into<Event>) -> Result<usize, SdkError> {
        let event = event.into();

        // 1. Validate event and extract handlers under lock
        let matching_handlers = {
            let guard = self.inner.lock().map_err(|_| SdkError::LockPoisoned)?;
            event.validate(&guard.store)?;

            guard
                .handlers
                .get(&(event.node_id, event.event_type))
                .cloned()
                .unwrap_or_default()
        }; // Lock released here!

        // 2. Invoke matching handlers outside lock
        let count = matching_handlers.len();
        for handler in matching_handlers {
            handler(self, &event);
        }

        Ok(count)
    }
}

#[cfg(test)]
impl Session {
    fn poison_lock_for_test(&self) {
        let inner = Arc::clone(&self.inner);
        let _ = std::panic::catch_unwind(|| {
            let _guard = inner.lock().unwrap();
            panic!("test lock poison");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_getters_survive_poisoned_lock() {
        let session = Session::new("poison-test");
        session.poison_lock_for_test();
        assert_eq!(session.session_id(), "poison-test");
        assert_eq!(session.current_revision().get(), 0);
        assert_eq!(session.node_count(), 0);
    }
}
