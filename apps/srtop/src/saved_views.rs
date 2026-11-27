//! Named saved views: the switcher and its controls (PX-069; design §§6.3,
//! 7.3, 7.6, 26–27; D2 T32).
//!
//! The controls are standard widgets in one `Row` under the shell's column,
//! created only when an instance has a settings store (`--settings-path`, or
//! the default path of `--live-source`): one `Button` per view slot
//! ([`MAX_VIEWS`]) labelled with the view's name, a "View name" `TextInput`,
//! the "Save as new view", "Rename active view", "Delete active view" and
//! "Restore default views" `Button`s, and a status `Text`. A button-based
//! switcher needs no Tabs widget (§7.3).
//!
//! Every decision is the server's. A slot's click names only its slot; the
//! view it applies is the one the server holds in that slot when the click is
//! handled, and an empty slot is disabled and collapsed, so event validation
//! refuses a click on it (§7.4). The name is the text the server kept for the
//! name field, never anything a click carries.
//!
//! **What each control may touch.** Applying a view (a slot, or Restore
//! default views) sets the process list's columns, sort, filters, view and
//! thread controls through the same server paths their own controls use, so
//! rows keep their item IDs, and any pending action intent is withdrawn or
//! kept exactly as those controls would (PX-030). Save as new view reads the
//! process list's presentation and changes nothing of it. Rename and Delete
//! never reach the process list or the action gate at all: their handlers
//! hold only these controls, so neither can change a selection, an identity
//! or an intent, nor execute an action. Deleting the active view leaves no
//! view active; it applies no other.
//!
//! A change is kept only when the settings file took it: a failed or refused
//! save leaves the views as they were and says why.

use crate::refresh::ProcessView;
use crate::settings::{
    NameError, SavedOwner, SavedView, Settings, ViewName, MAX_VIEWS, MAX_VIEW_NAME_BYTES,
};
use crate::settings_file::{Loaded, Origin, ReadOnly, SaveError, SettingsStore};
use crate::{
    bounded, refresh::ViewMode, SAVED_VIEWS_RESTORE, SAVED_VIEWS_ROW, SAVED_VIEWS_STATUS,
    SAVED_VIEW_CREATE, SAVED_VIEW_DELETE, SAVED_VIEW_NAME_INPUT, SAVED_VIEW_RENAME,
    SAVED_VIEW_SLOTS,
};
use srui_sdk::*;
use srui_sessiond::{Session, SessionError};
use std::sync::{Arc, Mutex};

/// The longest views status published, in encoded bytes.
pub const MAX_VIEWS_STATUS_BYTES: usize = 512;
/// The most bytes of the name field the server keeps.
pub const MAX_NAME_INPUT_BYTES: usize = 4 * MAX_VIEW_NAME_BYTES;

pub const NAME_LABEL: &str = "View name";
pub const NAME_PLACEHOLDER: &str = "Name for a new or renamed view";
pub const NAME_DESCRIPTION: &str =
    "The name Save as new view and Rename active view use: at most 64 bytes, no control or invisible characters.";
pub const CREATE_LABEL: &str = "Save as new view";
pub const RENAME_LABEL: &str = "Rename active view";
pub const DELETE_LABEL: &str = "Delete active view";
pub const RESTORE_LABEL: &str = "Restore default views";
pub const SLOT_DESCRIPTION: &str =
    "Applies this saved view: its columns, sort, filters, table or tree, and thread controls. It changes no process.";
pub const ROW_DESCRIPTION: &str =
    "Saved views, kept in this user's srtop settings on the server. Views hold presentation only: no process, selection or action.";

/// The switcher label of a view: its name, and whether it is active.
pub fn slot_label(name: &ViewName, active: bool) -> String {
    if active {
        format!("View: {name} (active)")
    } else {
        format!("View: {name}")
    }
}

/// The view `view` presents now, named `name`: its column layout, sort,
/// committed filter, user filter, view and thread controls. Nothing about a
/// process, the selection or an action is read.
pub fn capture(view: &ProcessView, name: ViewName) -> SavedView {
    let owner = view.owner_filter();
    let owner = if !owner.input().is_empty() {
        SavedOwner::Input(owner.input().to_string())
    } else if *owner.choice() == crate::owner::OwnerChoice::Current {
        SavedOwner::Mine
    } else {
        SavedOwner::AllVisible
    };
    SavedView {
        name,
        columns: view.column_layout().clone(),
        sort: view.sort(),
        filter: view.committed_filter().raw().to_string(),
        owner,
        tree: view.view_mode() == ViewMode::Tree,
        threads: view.thread_settings(),
    }
}

/// Applies `saved` to `view` through the process list's own server paths,
/// changing only what differs.
pub fn apply(
    view: &mut ProcessView,
    session: &Session,
    saved: &SavedView,
) -> Result<(), Box<dyn std::error::Error>> {
    if view.column_layout() != &saved.columns {
        view.set_columns(session, saved.columns.fields())?;
    }
    if let Some(order) = saved.sort.filter(|order| view.sort() != Some(*order)) {
        view.set_sort(session, order)?;
    }
    let owner = view.owner_filter();
    match &saved.owner {
        SavedOwner::AllVisible
            if !owner.input().is_empty()
                || *owner.choice() != crate::owner::OwnerChoice::AllVisible =>
        {
            view.choose_owner(session, crate::owner::OwnerChoice::AllVisible)?;
        }
        SavedOwner::Mine
            if !owner.input().is_empty()
                || *owner.choice() != crate::owner::OwnerChoice::Current =>
        {
            view.choose_owner(session, crate::owner::OwnerChoice::Current)?;
        }
        SavedOwner::Input(text) if owner.input() != text => {
            view.set_owner_input(session, text)?;
        }
        _ => {}
    }
    if view.committed_filter().raw() != saved.filter || view.pending_filter().is_some() {
        view.set_filter(session, &saved.filter)?;
    }
    if (view.view_mode() == ViewMode::Tree) != saved.tree {
        view.set_view(session, Some(saved.tree))?;
    }
    let threads = view.thread_settings();
    if threads.user != saved.threads.user {
        view.set_user_threads(session, Some(saved.threads.user))?;
    }
    if threads.kernel != saved.threads.kernel {
        view.set_kernel_threads(session, Some(saved.threads.kernel))?;
    }
    Ok(())
}

/// The controls as the client holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ControlsView {
    /// Per slot: label, enabled, shown.
    slots: Vec<(String, bool, bool)>,
    create: bool,
    rename: bool,
    delete: bool,
    restore: bool,
    status: String,
}

/// The saved views, their store and their controls.
pub struct SavedViews {
    store: SettingsStore,
    settings: Settings,
    read_only: Option<ReadOnly>,
    /// What the load found, until the next successful save.
    loaded_note: Option<String>,
    /// The outcome of the latest request.
    last: Option<String>,
    /// The name field's text as the server keeps it.
    name_input: String,
    /// Whether the controls exist.
    created: bool,
    published: Option<ControlsView>,
}

pub type SharedSavedViews = Arc<Mutex<SavedViews>>;

/// Locks the saved views. Lock order: the saved views, then the process
/// view, then the action gate.
pub fn lock_saved_views(views: &Mutex<SavedViews>) -> std::sync::MutexGuard<'_, SavedViews> {
    views
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn describe_load(loaded: &Loaded) -> Option<String> {
    let dropped = if loaded.notes.is_empty() {
        String::new()
    } else {
        format!(
            " · {} saved value{} this srtop does not support {} dropped: {}",
            loaded.notes.len(),
            if loaded.notes.len() == 1 { "" } else { "s" },
            if loaded.notes.len() == 1 {
                "was"
            } else {
                "were"
            },
            loaded.notes.join("; ")
        )
    };
    let note = match (&loaded.origin, &loaded.read_only) {
        (_, Some(reason)) => format!(
            "Not saving: {reason}; it is left unchanged and the default view is shown"
        ),
        (Origin::Fresh, _) => return None,
        (Origin::Primary { migrated_from: Some(version) }, _) => {
            format!("Settings from version {version} were upgraded; the next change saves them in the current version{dropped}")
        }
        (Origin::Primary { .. }, _) if dropped.is_empty() => return None,
        (Origin::Primary { .. }, _) => format!("Settings loaded{dropped}"),
        (Origin::Backup { primary }, _) => {
            format!("The settings file was {primary}; its backup was loaded{dropped}")
        }
        (Origin::Defaults { primary, .. }, _) => format!(
            "The settings file was {primary} and no valid backup was found; the default view is shown"
        ),
        (Origin::ReadOnly, None) => return None,
    };
    Some(note)
}

impl SavedViews {
    /// Loads the views for `source` from `store`. Nothing is written.
    pub fn open(store: SettingsStore, source: &str) -> Self {
        let loaded = store.load(source);
        Self {
            loaded_note: describe_load(&loaded),
            store,
            settings: loaded.settings,
            read_only: loaded.read_only,
            last: None,
            name_input: String::new(),
            created: false,
            published: None,
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn read_only(&self) -> Option<&ReadOnly> {
        self.read_only.as_ref()
    }

    pub fn store(&self) -> &SettingsStore {
        &self.store
    }

    /// The name field's text as the server keeps it.
    pub fn name_input(&self) -> &str {
        &self.name_input
    }

    /// The status as it was last published, or would be.
    pub fn status(&self) -> String {
        self.view().status
    }

    fn writable(&self) -> bool {
        self.read_only.is_none()
    }

    fn view(&self) -> ControlsView {
        let active = self.settings.active;
        let slots = (0..MAX_VIEWS)
            .map(|slot| match self.settings.views.get(slot) {
                Some(view) => (slot_label(&view.name, active == Some(slot)), true, true),
                None => (format!("Empty view slot {}", slot + 1), false, false),
            })
            .collect();
        let state = match self.settings.active_view() {
            Some(view) => format!(
                "Views: {} saved · active: {}",
                self.settings.views.len(),
                view.name
            ),
            None => format!(
                "Views: {} saved · no view active",
                self.settings.views.len()
            ),
        };
        let status = [Some(state), self.loaded_note.clone(), self.last.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        ControlsView {
            slots,
            create: self.writable() && self.settings.views.len() < MAX_VIEWS,
            rename: self.writable() && self.settings.active.is_some(),
            delete: self.writable()
                && self.settings.active.is_some()
                && self.settings.views.len() > 1,
            restore: self.writable() && !self.settings.is_default(),
            status: bounded(&status, MAX_VIEWS_STATUS_BYTES),
        }
    }

    /// Creates the controls under `parent`, in one transaction.
    pub fn create_controls(
        &mut self,
        session: &Session,
        parent: NodeId,
    ) -> Result<(), SessionError> {
        let target = self.view();
        session.transaction(|ui| {
            Row::builder(SAVED_VIEWS_ROW)
                .parent(parent)
                .spacing_role(SpacingRole::Normal)
                .accessible_description(ROW_DESCRIPTION)
                .create(ui)?;
            for (node, (label, enabled, shown)) in SAVED_VIEW_SLOTS.iter().zip(&target.slots) {
                Button::builder(*node)
                    .parent(SAVED_VIEWS_ROW)
                    .label(label.as_str())
                    .accessible_description(SLOT_DESCRIPTION)
                    .create(ui)?;
                ui.set(*node, ENABLED, *enabled)?;
                if !shown {
                    ui.set(*node, VISIBILITY, collapsed())?;
                }
            }
            TextInput::builder(SAVED_VIEW_NAME_INPUT)
                .parent(SAVED_VIEWS_ROW)
                .label(NAME_LABEL)
                .placeholder(NAME_PLACEHOLDER)
                .value("")
                .role(InputRole::Plain)
                .accessible_description(NAME_DESCRIPTION)
                .create(ui)?;
            ui.set(SAVED_VIEW_NAME_INPUT, ENABLED, self.writable())?;
            for (node, label, enabled) in [
                (SAVED_VIEW_CREATE, CREATE_LABEL, target.create),
                (SAVED_VIEW_RENAME, RENAME_LABEL, target.rename),
                (SAVED_VIEW_DELETE, DELETE_LABEL, target.delete),
                (SAVED_VIEWS_RESTORE, RESTORE_LABEL, target.restore),
            ] {
                Button::builder(node)
                    .parent(SAVED_VIEWS_ROW)
                    .label(label)
                    .create(ui)?;
                ui.set(node, ENABLED, enabled)?;
            }
            Text::builder(SAVED_VIEWS_STATUS)
                .parent(SAVED_VIEWS_ROW)
                .text(target.status.as_str())
                .role(TextRole::Status)
                .create(ui)?;
            Ok(())
        })?;
        self.created = true;
        self.published = Some(target);
        Ok(())
    }

    /// Publishes what changed in the controls, in one transaction.
    fn publish(&mut self, session: &Session) -> Result<(), SessionError> {
        if !self.created {
            return Ok(());
        }
        let target = self.view();
        let Some(published) = self.published.clone() else {
            return Ok(());
        };
        if published == target {
            return Ok(());
        }
        session.transaction(|ui| {
            for ((node, now), was) in SAVED_VIEW_SLOTS
                .iter()
                .zip(&target.slots)
                .zip(&published.slots)
            {
                if now.0 != was.0 {
                    ui.set(*node, LABEL, now.0.as_str())?;
                }
                if now.1 != was.1 {
                    ui.set(*node, ENABLED, now.1)?;
                }
                if now.2 != was.2 {
                    ui.set(
                        *node,
                        VISIBILITY,
                        if now.2 {
                            Value::from(EnumToken::from(Visibility::Visible))
                        } else {
                            collapsed()
                        },
                    )?;
                }
            }
            for (node, now, was) in [
                (SAVED_VIEW_CREATE, target.create, published.create),
                (SAVED_VIEW_RENAME, target.rename, published.rename),
                (SAVED_VIEW_DELETE, target.delete, published.delete),
                (SAVED_VIEWS_RESTORE, target.restore, published.restore),
            ] {
                if now != was {
                    ui.set(node, ENABLED, now)?;
                }
            }
            if target.status != published.status {
                ui.set(SAVED_VIEWS_STATUS, TEXT, target.status.as_str())?;
            }
            Ok(())
        })?;
        self.published = Some(target);
        Ok(())
    }

    /// Sets the name field to `text` in the store when it holds other text.
    fn publish_name(&mut self, session: &Session, text: &str) -> Result<(), SessionError> {
        self.name_input = text.to_string();
        if !self.created || field_value(session) == text {
            return Ok(());
        }
        session.transaction(|ui| {
            ui.set(SAVED_VIEW_NAME_INPUT, VALUE, text)?;
            Ok(())
        })?;
        Ok(())
    }

    /// The name field's new text, kept up to [`MAX_NAME_INPUT_BYTES`] on a
    /// character boundary; a longer name is refused when it is used.
    pub fn set_name_input(&mut self, session: &Session, text: &str) -> Result<(), SessionError> {
        let mut end = text.len().min(MAX_NAME_INPUT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.publish_name(session, &text[..end])
    }

    /// Saves `next` and keeps it when the settings file took it; otherwise
    /// keeps the views as they were and says why.
    fn commit(&mut self, next: Settings, done: String) -> bool {
        if let Some(reason) = &self.read_only {
            self.last = Some(format!("Not saved: {reason}"));
            return false;
        }
        match self.store.save(&next) {
            Ok(()) => {
                self.settings = next;
                self.loaded_note = None;
                self.last = Some(done);
                true
            }
            Err(error @ SaveError::NotFlushed(_)) => {
                // The file holds the new views; only its directory entry may
                // not have reached the disk yet.
                self.settings = next;
                self.loaded_note = None;
                self.last = Some(format!("{done}, {error}"));
                true
            }
            Err(error) => {
                if let SaveError::Refused(reason) = &error {
                    self.read_only = Some(reason.clone());
                }
                self.last = Some(format!("Not saved: {error}"));
                false
            }
        }
    }

    fn named(&mut self) -> Result<ViewName, String> {
        ViewName::parse(&self.name_input).map_err(|error| match error {
            NameError::Empty => "Type a view name first".to_string(),
            other => format!("Not saved: {other}"),
        })
    }

    /// Applies the view in `slot` to `view` and makes it the active view.
    pub fn apply_slot(
        &mut self,
        session: &Session,
        view: &mut ProcessView,
        slot: usize,
    ) -> Result<(), SessionError> {
        let Some(saved) = self.settings.views.get(slot).cloned() else {
            // An empty slot is disabled, so validation refuses its click;
            // nothing is applied if one arrives all the same.
            self.last = Some("That view slot is empty".into());
            return self.publish(session);
        };
        self.last = Some(match apply(view, session, &saved) {
            Ok(()) => format!("Applied view “{}”", saved.name),
            Err(error) => format!("View “{}” applied in part: {error}", saved.name),
        });
        if self.settings.active != Some(slot) {
            let applied = self.last.clone().unwrap_or_default();
            let mut next = self.settings.clone();
            next.active = Some(slot);
            if !self.commit(next, applied.clone()) {
                // The view is shown, but which one is active was not saved.
                self.last = Some(format!(
                    "{applied} · {}",
                    self.last.clone().unwrap_or_default()
                ));
            }
        }
        self.publish(session)
    }

    /// Applies the active view, when there is one: what a restarted srtop
    /// shows first. Writes nothing.
    pub fn apply_active(
        &mut self,
        session: &Session,
        view: &mut ProcessView,
    ) -> Result<(), SessionError> {
        if let Some(saved) = self.settings.active_view().cloned() {
            if let Err(error) = apply(view, session, &saved) {
                self.last = Some(format!("View “{}” applied in part: {error}", saved.name));
            }
        }
        self.publish(session)
    }

    /// Saves what `view` presents now as a new view named by the name field,
    /// and makes it active. `view` is only read.
    pub fn create(&mut self, session: &Session, view: &ProcessView) -> Result<(), SessionError> {
        match self.named() {
            Err(message) => self.last = Some(message),
            Ok(_) if self.settings.views.len() >= MAX_VIEWS => {
                self.last = Some(format!("Not saved: at most {MAX_VIEWS} views"));
            }
            Ok(name) if self.settings.find(&name).is_some() => {
                self.last = Some(format!("Not saved: a view named “{name}” exists"));
            }
            Ok(name) => {
                let mut next = self.settings.clone();
                next.views.push(capture(view, name.clone()));
                next.active = Some(next.views.len() - 1);
                if self.commit(next, format!("Saved view “{name}”")) {
                    self.publish_name(session, "")?;
                }
            }
        }
        self.publish(session)
    }

    /// Renames the active view to the name field's text. Touches nothing but
    /// the views.
    pub fn rename(&mut self, session: &Session) -> Result<(), SessionError> {
        let Some(active) = self.settings.active else {
            self.last = Some("No view is active to rename".into());
            return self.publish(session);
        };
        match self.named() {
            Err(message) => self.last = Some(message),
            Ok(name)
                if self
                    .settings
                    .find(&name)
                    .is_some_and(|other| other != active) =>
            {
                self.last = Some(format!("Not renamed: a view named “{name}” exists"));
            }
            Ok(name) => {
                let mut next = self.settings.clone();
                let was = std::mem::replace(&mut next.views[active].name, name.clone());
                if self.commit(next, format!("Renamed view “{was}” to “{name}”")) {
                    self.publish_name(session, "")?;
                }
            }
        }
        self.publish(session)
    }

    /// Deletes the active view; no view is active afterwards and nothing is
    /// applied. The last view cannot be deleted. Touches nothing but the
    /// views.
    pub fn delete(&mut self, session: &Session) -> Result<(), SessionError> {
        match self.settings.active {
            None => self.last = Some("No view is active to delete".into()),
            Some(_) if self.settings.views.len() <= 1 => {
                self.last = Some("Not deleted: the last view stays".into());
            }
            Some(active) => {
                let mut next = self.settings.clone();
                let removed = next.views.remove(active);
                next.active = None;
                self.commit(next, format!("Deleted view “{}”", removed.name));
            }
        }
        self.publish(session)
    }

    /// Replaces every view with the default view, saves it and applies it.
    pub fn restore_defaults(
        &mut self,
        session: &Session,
        view: &mut ProcessView,
    ) -> Result<(), SessionError> {
        let next = Settings::defaults(&self.settings.source);
        if self.commit(next, "Restored the default views".into()) {
            if let Some(saved) = self.settings.active_view().cloned() {
                if let Err(error) = apply(view, session, &saved) {
                    self.last = Some(format!("Default view applied in part: {error}"));
                }
            }
        }
        self.publish(session)
    }
}

fn collapsed() -> Value {
    Value::from(EnumToken::from(Visibility::Collapsed))
}

fn field_value(session: &Session) -> String {
    session.with_store(|store| {
        store
            .get_node(SAVED_VIEW_NAME_INPUT)
            .and_then(|node| node.get_property(VALUE))
            .and_then(Value::as_string)
            .unwrap_or("")
            .to_string()
    })
}
