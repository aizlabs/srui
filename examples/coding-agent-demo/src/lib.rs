//! Coding-agent example composition with a Standard Widget extension fallback (§11.1, §30).

use std::sync::Arc;

use srui_sdk::*;
use srui_semantic_tree::{EditSeq, Event as DomainEvent, ModelItem, StandardValidationState};
use srui_sessiond::{EventOutcome, Session, SessionError, TerminalSpec};

pub const DIFF_PROFILE_URI: &str = "org.example.diff/1";
pub const DIFF_LOCAL_TYPE_ID: u32 = 1;

pub const SURFACE_ID: NodeId = NodeId::new(1);
pub const MAIN_COLUMN_ID: NodeId = NodeId::new(2);
pub const HEADER_ROW_ID: NodeId = NodeId::new(3);
pub const HEADING_ID: NodeId = NodeId::new(4);
pub const PROGRESS_ID: NodeId = NodeId::new(5);
pub const CONTENT_ROW_ID: NodeId = NodeId::new(6);
pub const FILE_TREE_ID: NodeId = NodeId::new(7);
pub const RIGHT_COLUMN_ID: NodeId = NodeId::new(8);
pub const CONVERSATION_ID: NodeId = NodeId::new(9);
pub const DIFF_EXTENSION_ID: NodeId = NodeId::new(10);
pub const FALLBACK_COLUMN_ID: NodeId = NodeId::new(11);
pub const FALLBACK_HEADING_ID: NodeId = NodeId::new(12);
pub const FALLBACK_DIFF_ID: NodeId = NodeId::new(13);
pub const TERMINAL_ID: NodeId = NodeId::new(14);
pub const PROMPT_ID: NodeId = NodeId::new(15);
pub const ACTION_ROW_ID: NodeId = NodeId::new(16);
pub const APPROVE_ID: NodeId = NodeId::new(17);
pub const REJECT_ID: NodeId = NodeId::new(18);
pub const FILE_MODEL_ID: ModelId = ModelId::new(1);

const INITIAL_CONVERSATION: &str = "Agent: I implemented token validation in src/auth.rs.";
const FALLBACK_DIFF: &str =
    "@@ -10,6 +10,15 @@\n+// Validate bearer tokens before accepting a request.\n+pub fn validate_token(token: &str) -> bool { ... }";

#[derive(Debug, Clone, Copy)]
pub struct CodingAgentNodes {
    pub diff_type: TypeRef,
}

#[derive(Debug, Clone)]
pub struct CodingAgentApp {
    session: Arc<Session>,
    pub nodes: CodingAgentNodes,
}

impl CodingAgentApp {
    pub fn new() -> Result<Self, SessionError> {
        Self::with_terminal_spec(TerminalSpec::interactive_shell())
    }

    pub fn with_terminal_spec(terminal_spec: TerminalSpec) -> Result<Self, SessionError> {
        let session = Arc::new(Session::new("coding-agent-demo-session"));
        let profile = Profile::parse(DIFF_PROFILE_URI)
            .map_err(|error| SessionError::InvalidInput(error.to_string()))?;
        let diff_namespace = session.register_optional_extension_profile(profile)?;
        let diff_type = TypeRef::new(diff_namespace, DIFF_LOCAL_TYPE_ID);
        let files = [
            "src/",
            "src/auth.rs",
            "src/main.rs",
            "Cargo.toml",
            "README.md",
        ];
        let model_items: Vec<ModelItem> = files
            .iter()
            .enumerate()
            .map(|(index, path)| ModelItem::with_value(ItemId::new(index as u64 + 1), *path))
            .collect();

        // Revision 1: everything whose parent can be created atomically before the PTY node.
        session.transaction(|ui| {
            ui.apply_op(&Operation::create_model(FILE_MODEL_ID, TypeRef::TREE, 0))?;
            ui.apply_op(&Operation::model_insert(FILE_MODEL_ID, 0, model_items))?;

            Surface::builder(SURFACE_ID)
                .label("Agent session — myproject")
                .create(ui)?;
            Column::builder(MAIN_COLUMN_ID)
                .parent(SURFACE_ID)
                .spacing_role(SpacingRole::Normal)
                .padding_role(PaddingRole::Normal)
                .grow(1.0)
                .create(ui)?;
            Row::builder(HEADER_ROW_ID)
                .parent(MAIN_COLUMN_ID)
                .spacing_role(SpacingRole::Normal)
                .create(ui)?;
            Text::builder(HEADING_ID)
                .parent(HEADER_ROW_ID)
                .text("Implementing auth module…")
                .role(TextRole::Heading)
                .create(ui)?;
            Progress::builder(PROGRESS_ID)
                .parent(HEADER_ROW_ID)
                .value(0.62)
                .value_description("62% complete")
                .grow(1.0)
                .create(ui)?;
            Row::builder(CONTENT_ROW_ID)
                .parent(MAIN_COLUMN_ID)
                .spacing_role(SpacingRole::Normal)
                .grow(1.0)
                .create(ui)?;
            Tree::builder(FILE_TREE_ID)
                .parent(CONTENT_ROW_ID)
                .model_ref(FILE_MODEL_ID)
                .selection_mode(SelectionMode::Single)
                .label("Project files")
                .create(ui)?;
            Column::builder(RIGHT_COLUMN_ID)
                .parent(CONTENT_ROW_ID)
                .spacing_role(SpacingRole::Normal)
                .grow(1.0)
                .create(ui)?;
            RichText::builder(CONVERSATION_ID)
                .parent(RIGHT_COLUMN_ID)
                .text(INITIAL_CONVERSATION)
                .read_only(true)
                .create(ui)?;
            ui.create_node(
                DIFF_EXTENSION_ID,
                diff_type,
                Some(RIGHT_COLUMN_ID),
                None,
                [],
            )?;
            Column::builder(FALLBACK_COLUMN_ID)
                .parent(DIFF_EXTENSION_ID)
                .spacing_role(SpacingRole::Tight)
                .create(ui)?;
            Text::builder(FALLBACK_HEADING_ID)
                .parent(FALLBACK_COLUMN_ID)
                .text("Proposed Changes: src/auth.rs")
                .role(TextRole::Heading)
                .create(ui)?;
            RichText::builder(FALLBACK_DIFF_ID)
                .parent(FALLBACK_COLUMN_ID)
                .text(FALLBACK_DIFF)
                .read_only(true)
                .create(ui)?;
            TextArea::builder(PROMPT_ID)
                .parent(MAIN_COLUMN_ID)
                .value("")
                .placeholder("Ask the agent…")
                .role(InputRole::Command)
                .action_key("prompt")
                .create(ui)?;
            Ok(())
        })?;

        // Revision 2: create_terminal_node owns its PTY spawn and semantic transaction.
        session.create_terminal_node(TERMINAL_ID, RIGHT_COLUMN_ID, terminal_spec)?;

        // Revision 3: appending actions after Terminal preserves the §30 child order.
        session.transaction(|ui| {
            Row::builder(ACTION_ROW_ID)
                .parent(RIGHT_COLUMN_ID)
                .spacing_role(SpacingRole::Normal)
                .create(ui)?;
            Button::builder(APPROVE_ID)
                .parent(ACTION_ROW_ID)
                .label("Approve")
                .role(ActionRole::Primary)
                .action_key("approve")
                .create(ui)?;
            Button::builder(REJECT_ID)
                .parent(ACTION_ROW_ID)
                .label("Reject")
                .role(ActionRole::Destructive)
                .action_key("reject")
                .create(ui)?;
            Ok(())
        })?;

        session.on(APPROVE_ID, ACTIVATE, |ctx, _event| {
            ctx.transaction(|ui| {
                append_conversation(ui, "User approved the proposed changes.")?;
                ui.set(PROGRESS_ID, VALUE, 0.75)?;
                ui.set(PROGRESS_ID, VALUE_DESCRIPTION, "75% complete")?;
                Ok(())
            })
            .expect("approve transaction failed");
        });
        session.on(REJECT_ID, ACTIVATE, |ctx, _event| {
            ctx.transaction(|ui| {
                append_conversation(ui, "User rejected the proposed changes.")?;
                ui.set(PROGRESS_ID, VALUE, 0.50)?;
                ui.set(PROGRESS_ID, VALUE_DESCRIPTION, "50% complete")?;
                Ok(())
            })
            .expect("reject transaction failed");
        });
        session.on_text_edit(|_ctx, request| {
            if request.node_id == PROMPT_ID {
                srui_sessiond::TextEditDecision::Accept
            } else {
                srui_sessiond::TextEditDecision::Reject {
                    value: None,
                    reason: "unknown editor".to_string(),
                }
            }
        });

        Ok(Self {
            session,
            nodes: CodingAgentNodes { diff_type },
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_arc(&self) -> Arc<Session> {
        Arc::clone(&self.session)
    }

    pub fn current_revision(&self) -> u64 {
        self.session.current_revision()
    }

    pub fn conversation(&self) -> String {
        self.string_property(CONVERSATION_ID, TEXT)
    }

    pub fn prompt(&self) -> String {
        self.string_property(PROMPT_ID, VALUE)
    }

    pub fn progress(&self) -> f64 {
        self.session.with_store(|store| {
            store
                .get_node(PROGRESS_ID)
                .and_then(|node| node.get_property(VALUE))
                .and_then(Value::as_float64)
                .unwrap_or(0.0)
        })
    }

    pub fn activate(&self, node_id: NodeId, event_seq: u64) -> Result<EventOutcome, SessionError> {
        let event = DomainEvent::activate(
            event_seq,
            format!("activate-{event_seq}"),
            self.current_revision(),
            node_id,
        )
        .with_client_instance_id(b"coding-agent-demo".to_vec())
        .to_wire();
        self.session.process_event(&event)
    }

    pub fn edit_prompt(
        &self,
        event_seq: u64,
        edit_seq: u64,
        value: &str,
    ) -> Result<EventOutcome, SessionError> {
        let edit_seq = EditSeq::new(edit_seq)
            .ok_or_else(|| SessionError::InvalidInput("edit_seq must be positive".to_string()))?;
        let event = DomainEvent::text_edit(
            event_seq,
            format!("prompt-{event_seq}"),
            self.current_revision(),
            PROMPT_ID,
            value,
            edit_seq,
        )
        .with_client_instance_id(b"coding-agent-demo".to_vec())
        .to_wire();
        self.session.process_event(&event)
    }

    pub fn prompt_validation(&self) -> Option<StandardValidationState> {
        self.session.with_store(|store| {
            store
                .get_node(PROMPT_ID)
                .and_then(|node| node.get_property(VALIDATION_STATE))
                .and_then(Value::as_enum_token)
                .and_then(|token| StandardValidationState::try_from(token).ok())
        })
    }

    fn string_property(&self, node_id: NodeId, property: PropertyRef) -> String {
        self.session.with_store(|store| {
            store
                .get_node(node_id)
                .and_then(|node| node.get_property(property))
                .and_then(Value::as_string)
                .unwrap_or("")
                .to_string()
        })
    }
}

fn append_conversation(ui: &mut srui_sdk::UiTransaction, message: &str) -> Result<(), StoreError> {
    let previous = ui
        .get_node(CONVERSATION_ID)
        .and_then(|node| node.get_property(TEXT))
        .and_then(Value::as_string)
        .unwrap_or("");
    ui.set(CONVERSATION_ID, TEXT, format!("{previous}\n\n{message}"))?;
    Ok(())
}
