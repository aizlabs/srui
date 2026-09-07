//! Optional extension registration and namespace isolation (§6.4, §11.1, §15).

use srui_protocol::{ClientHello, TERMINAL_PROFILE_URI};
use srui_sdk::{Profile, Surface};
use srui_sessiond::{Session, SessionError, TerminalSpec};

const DIFF_PROFILE: &str = "org.example.diff/1";

fn diff_profile() -> Profile {
    Profile::parse(DIFF_PROFILE).expect("valid test profile")
}

fn hello(profiles: &[&str]) -> ClientHello {
    ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: profiles
            .iter()
            .map(|profile| (*profile).to_string())
            .collect(),
        limits: None,
        client_instance_id: b"extension-test".to_vec(),
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    }
}

#[test]
fn optional_registration_is_stable_and_advertised_without_negotiation() {
    let session = Session::new("optional-extension");
    let first = session
        .register_optional_extension_profile(diff_profile())
        .expect("register extension");
    let second = session
        .register_optional_extension_profile(diff_profile())
        .expect("repeat registration");
    assert_ne!(first, 0);
    assert_eq!(first, second);

    let bootstrap = session
        .bootstrap_fresh_client(&hello(&["org.srui.standard-widgets/1"]))
        .expect("base client negotiates");
    assert!(bootstrap
        .welcome
        .optional_profiles
        .iter()
        .any(|profile| profile == DIFF_PROFILE));
    assert!(bootstrap
        .welcome
        .extension_namespaces
        .iter()
        .any(|mapping| { mapping.extension_uri == DIFF_PROFILE && mapping.namespace_id == first }));
}

#[test]
fn terminal_and_placeholder_namespaces_are_unique_in_either_order() {
    for placeholder_first in [true, false] {
        let session = Session::new(format!("namespace-order-{placeholder_first}"));
        session
            .transaction(|ui| {
                Surface::builder(1).create(ui)?;
                Ok(())
            })
            .expect("surface");

        let placeholder = if placeholder_first {
            session
                .register_optional_extension_profile(diff_profile())
                .expect("placeholder first")
        } else {
            session
                .create_terminal_node(2.into(), 1.into(), TerminalSpec::interactive_shell())
                .expect("terminal first");
            session
                .register_optional_extension_profile(diff_profile())
                .expect("placeholder second")
        };
        if placeholder_first {
            session
                .create_terminal_node(2.into(), 1.into(), TerminalSpec::interactive_shell())
                .expect("terminal second");
        }
        let terminal = session.terminal_namespace_id().expect("terminal namespace");
        assert_ne!(placeholder, terminal);
        session.pty().shutdown();
    }
}

#[test]
fn registration_rejects_reserved_profiles_and_late_mutation() {
    let session = Session::new("reserved-extension");
    assert!(matches!(
        session.register_optional_extension_profile(Profile::standard_widgets_v1()),
        Err(SessionError::InvalidInput(_))
    ));
    assert!(matches!(
        session.register_optional_extension_profile(Profile::terminal_v1()),
        Err(SessionError::InvalidInput(_))
    ));

    let attached = Session::new("attached-extension");
    let _attachment = attached.attach().expect("attachment");
    assert!(matches!(
        attached.register_optional_extension_profile(diff_profile()),
        Err(SessionError::InvalidInput(_))
    ));

    let negotiated = Session::new("negotiated-extension");
    let bootstrap = negotiated
        .bootstrap_fresh_client(&hello(&["org.srui.standard-widgets/1"]))
        .expect("handshake");
    drop(bootstrap);
    assert!(matches!(
        negotiated.register_optional_extension_profile(diff_profile()),
        Err(SessionError::InvalidInput(_))
    ));
    assert!(negotiated
        .extension_namespaces()
        .iter()
        .all(|mapping| mapping.extension_uri != TERMINAL_PROFILE_URI || mapping.namespace_id != 0));
}
