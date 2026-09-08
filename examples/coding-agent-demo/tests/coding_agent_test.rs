//! Task 31 composition, negotiation, interaction, text editing, and PTY tests.

use std::time::Duration;

use srui_example_coding_agent::*;
use srui_protocol::ClientHello;
use srui_pty::TerminalEvent;
use srui_sdk::{Profile, PropertyRef, TypeRef};
use srui_semantic_tree::StandardValidationState;
use srui_sessiond::EventOutcome;

#[test]
fn tree_composition_and_extension_fallback_are_complete_before_attach() {
    let app = CodingAgentApp::new().expect("app");
    assert_eq!(app.current_revision(), 3);
    app.session().with_store(|store| {
        assert_eq!(store.root_ids(), &[SURFACE_ID]);
        assert_eq!(
            store
                .get_node(MAIN_COLUMN_ID)
                .expect("main")
                .ordered_children,
            vec![HEADER_ROW_ID, CONTENT_ROW_ID, PROMPT_ID]
        );
        assert_eq!(
            store
                .get_node(RIGHT_COLUMN_ID)
                .expect("right")
                .ordered_children,
            vec![
                CONVERSATION_ID,
                DIFF_EXTENSION_ID,
                TERMINAL_ID,
                ACTION_ROW_ID
            ]
        );
        assert_eq!(
            store
                .get_node(DIFF_EXTENSION_ID)
                .expect("diff")
                .ordered_children,
            vec![FALLBACK_COLUMN_ID]
        );
        assert!(store
            .get_node(FALLBACK_COLUMN_ID)
            .expect("fallback")
            .node_type
            .is_standard());
        assert_eq!(
            store.get_model(FILE_MODEL_ID).expect("files").item_count(),
            5
        );
        assert_eq!(
            store
                .get_model(FILE_MODEL_ID)
                .expect("files")
                .get_item_by_index(1)
                .expect("auth")
                .value
                .as_string(),
            Some("src/auth.rs")
        );
    });
    assert!(app.session().pty().contains(TERMINAL_ID));
    app.session().pty().shutdown();
}

#[test]
fn app_instances_mint_distinct_session_incarnations() {
    let first = CodingAgentApp::new().expect("first app");
    let second = CodingAgentApp::new().expect("second app");

    assert_ne!(first.session().session_id(), second.session().session_id());

    first.session().pty().shutdown();
    second.session().pty().shutdown();
}

#[test]
fn base_client_negotiates_without_placeholder_profile() {
    let app = CodingAgentApp::new().expect("app");
    let bootstrap = app
        .session()
        .bootstrap_fresh_client(&ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec![
                "org.srui.standard-widgets/1".to_string(),
                "org.srui.terminal/1".to_string(),
            ],
            limits: None,
            client_instance_id: b"base-client".to_vec(),
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })
        .expect("base handshake");
    assert!(!bootstrap
        .welcome
        .required_profiles
        .iter()
        .any(|profile| profile == DIFF_PROFILE_URI));
    assert!(bootstrap
        .welcome
        .optional_profiles
        .iter()
        .any(|profile| profile == DIFF_PROFILE_URI));
    let diff_namespace = bootstrap
        .welcome
        .extension_namespaces
        .iter()
        .find(|mapping| mapping.extension_uri == DIFF_PROFILE_URI)
        .expect("diff mapping")
        .namespace_id;
    assert_eq!(
        app.nodes.diff_type,
        TypeRef::new(diff_namespace, DIFF_LOCAL_TYPE_ID)
    );
    assert_ne!(
        diff_namespace,
        app.session()
            .terminal_namespace_id()
            .expect("terminal namespace")
    );
    assert_eq!(
        Profile::parse(DIFF_PROFILE_URI)
            .expect("valid diff profile")
            .version(),
        1
    );
    app.session().pty().shutdown();
}

#[test]
fn approve_and_real_text_edit_advance_authoritative_state() {
    let app = CodingAgentApp::new().expect("app");
    assert!(matches!(
        app.activate(APPROVE_ID, 1).expect("approve"),
        EventOutcome::Processed { .. }
    ));
    assert_eq!(app.current_revision(), 4);
    assert!(app.conversation().contains("approved"));
    assert_eq!(app.progress(), 0.75);

    assert!(matches!(
        app.edit_prompt(2, 1, "Add an expiry test")
            .expect("text edit"),
        EventOutcome::Processed { .. }
    ));
    assert_eq!(app.current_revision(), 5);
    assert_eq!(app.prompt(), "Add an expiry test");
    assert_eq!(
        app.prompt_validation(),
        Some(StandardValidationState::Valid)
    );
    assert!(app
        .session()
        .get_node(PROMPT_ID)
        .expect("prompt")
        .get_property(PropertyRef::VALUE)
        .is_some());
    app.session().pty().shutdown();
}

#[tokio::test]
async fn terminal_stream_runs_a_real_pty() {
    let app = CodingAgentApp::new().expect("app");
    let mut subscription = app
        .session()
        .pty()
        .subscribe(TERMINAL_ID, 0)
        .expect("subscribe")
        .subscription;
    app.session()
        .pty()
        .input(TERMINAL_ID, b"printf 'TASK31_PTY_OK\n'\n".to_vec())
        .expect("terminal input");

    let seen = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let events = subscription.recv().await;
            if events.is_empty() {
                return false;
            }
            for event in events {
                if let TerminalEvent::Data(data) = event {
                    if data
                        .data
                        .windows(b"TASK31_PTY_OK".len())
                        .any(|window| window == b"TASK31_PTY_OK")
                    {
                        return true;
                    }
                }
            }
        }
    })
    .await
    .expect("terminal output timeout");
    assert!(seen);
    app.session().pty().shutdown();
}
