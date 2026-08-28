//! Minimal Counter application demonstrating the SRUI Server SDK (§7.1–§7.7, §29).
//!
//! Features:
//! - Top-level `Surface` containing a `Text` display, a `Progress` indicator, and an increment `Button`.
//! - In-process event handling: clicking the button (via `ACTIVATE` event) triggers `session.transaction`,
//!   updating the counter text and progress indicator atomically.

use srui_sdk::*;

/// Minimal counter application built with the SRUI Server SDK (§29).
#[derive(Debug, Clone)]
pub struct CounterApp {
    session: Session,
    surface_id: NodeId,
    text_id: NodeId,
    progress_id: NodeId,
    button_id: NodeId,
}

impl CounterApp {
    /// Initializes a new `CounterApp` with initial UI state in revision 1.
    pub fn new() -> Result<Self, SdkError> {
        let session = Session::new("counter-app-session");
        let surface_id = NodeId::new(1);
        let text_id = NodeId::new(2);
        let progress_id = NodeId::new(3);
        let button_id = NodeId::new(4);

        // Initial UI construction within a single atomic transaction (§12.1, §29)
        session.transaction(|ui| {
            Surface::builder(surface_id)
                .label("Counter Application")
                .create(ui)?;

            Text::builder(text_id)
                .parent(surface_id)
                .text("Count: 0")
                .role(TextRole::Heading)
                .create(ui)?;

            Progress::builder(progress_id)
                .parent(surface_id)
                .value(0.0)
                .value_description("0 / 100")
                .create(ui)?;

            Button::builder(button_id)
                .parent(surface_id)
                .label("Increment")
                .role(ActionRole::Primary)
                .create(ui)?;

            Ok(())
        })?;

        // Register ACTIVATE event handler on the button (§7.6, §29)
        let text = text_id;
        let prog = progress_id;

        session.on(button_id, ACTIVATE, move |ctx, _event| {
            ctx.transaction(|ui| {
                let current: u64 = ui
                    .get_node(text)
                    .and_then(|n| n.get_property(TEXT))
                    .and_then(|v| v.as_string())
                    .and_then(|s| s.strip_prefix("Count: "))
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
                let next_val = current + 1;
                ui.set(text, TEXT, format!("Count: {}", next_val))?;
                ui.set(prog, VALUE, (next_val as f64) / 100.0)?;
                ui.set(prog, VALUE_DESCRIPTION, format!("{} / 100", next_val))?;
                Ok(())
            })
            .expect("counter increment transaction failed");
        });

        Ok(Self {
            session,
            surface_id,
            text_id,
            progress_id,
            button_id,
        })
    }

    /// Returns a reference to the underlying [`Session`].
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Returns the root `Surface` node ID.
    pub fn surface_id(&self) -> NodeId {
        self.surface_id
    }

    /// Returns the `Text` node ID displaying the counter.
    pub fn text_id(&self) -> NodeId {
        self.text_id
    }

    /// Returns the `Progress` node ID.
    pub fn progress_id(&self) -> NodeId {
        self.progress_id
    }

    /// Returns the increment `Button` node ID.
    pub fn button_id(&self) -> NodeId {
        self.button_id
    }

    /// Returns the current numeric count value derived directly from authoritative store state (§6.3).
    pub fn get_count(&self) -> u64 {
        self.session.with_store(|store| {
            Text::from_store(store, self.text_id)
                .and_then(|t| t.text(store))
                .and_then(|s| s.strip_prefix("Count: "))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0)
        })
    }

    /// Returns the current committed text string from the store.
    pub fn get_text(&self) -> Option<String> {
        self.session.with_store(|store| {
            Text::from_store(store, self.text_id)
                .and_then(|t| t.text(store))
                .map(|s| s.to_string())
        })
    }

    /// Returns the current committed progress float from the store.
    pub fn get_progress(&self) -> Option<f64> {
        self.session.with_store(|store| {
            Progress::from_store(store, self.progress_id).and_then(|p| p.value(store))
        })
    }

    /// Returns the current committed progress description from the store.
    pub fn get_progress_description(&self) -> Option<String> {
        self.session.with_store(|store| {
            Progress::from_store(store, self.progress_id)
                .and_then(|p| p.value_description(store))
                .map(|s| s.to_string())
        })
    }

    /// Returns the current committed graph revision.
    pub fn current_revision(&self) -> u64 {
        self.session.current_revision().get()
    }

    /// Simulates clicking the increment button by dispatching an `ACTIVATE` event (§7.6, §7.7, §29).
    pub fn click(&self, event_seq: u64) -> Result<usize, SdkError> {
        let rev = self.session.current_revision();
        let event = Event::activate(event_seq, format!("click-{}", event_seq), rev, self.button_id);
        self.session.dispatch(event)
    }
}
