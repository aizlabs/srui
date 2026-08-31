use srui_semantic_tree::{
    CapabilitySet, NegotiationError, ParseProfileError, Profile, ServerCapabilities,
    PROFILE_CODING, PROFILE_MEDIA_SURFACE, PROFILE_RICHTEXT, PROFILE_STANDARD_WIDGETS,
    PROFILE_TERMINAL, PROFILE_VECTOR_SCENE,
};

#[test]
fn test_profile_parsing_valid() {
    let p1 = Profile::parse("org.srui.standard-widgets/1").unwrap();
    assert_eq!(p1.name(), "org.srui.standard-widgets");
    assert_eq!(p1.version(), 1);
    assert_eq!(p1.to_string(), "org.srui.standard-widgets/1");

    let p2 = Profile::parse("org.srui.terminal/2").unwrap();
    assert_eq!(p2.name(), "org.srui.terminal");
    assert_eq!(p2.version(), 2);

    let p3 = Profile::new("org.example.custom", 5).unwrap();
    assert_eq!(p3.to_string(), "org.example.custom/5");
}

#[test]
fn test_profile_parsing_invalid() {
    // 1. Empty string
    assert_eq!(Profile::parse(""), Err(ParseProfileError::EmptyString));
    assert_eq!(Profile::parse("   "), Err(ParseProfileError::EmptyString));

    // 2. Missing delimiter
    assert!(matches!(
        Profile::parse("org.srui.standard-widgets"),
        Err(ParseProfileError::MissingVersionDelimiter(_))
    ));

    // 3. Empty name
    assert_eq!(Profile::parse("/1"), Err(ParseProfileError::EmptyName));
    assert_eq!(Profile::parse("  /1"), Err(ParseProfileError::EmptyName));

    // 4. Non-numeric or invalid version
    assert!(matches!(
        Profile::parse("org.srui.standard-widgets/abc"),
        Err(ParseProfileError::InvalidVersion(_))
    ));

    // 5. Version zero (versions must be >= 1)
    assert!(matches!(
        Profile::parse("org.srui.standard-widgets/0"),
        Err(ParseProfileError::InvalidVersion(_))
    ));
}

#[test]
fn test_capability_set_basic_operations() {
    let mut set = CapabilitySet::new();
    assert!(set.is_empty());
    assert_eq!(set.len(), 0);

    assert!(set.insert_str("org.srui.standard-widgets/1").unwrap());
    assert!(!set.insert_str("org.srui.standard-widgets/1").unwrap()); // duplicate insert returns false

    set.insert(Profile::terminal_v1());
    assert_eq!(set.len(), 2);
    assert!(!set.is_empty());

    assert!(set.contains_str("org.srui.standard-widgets/1"));
    assert!(set.contains_str("org.srui.terminal/1"));
    assert!(!set.contains_str("org.srui.richtext/1"));

    assert!(set.contains_name("org.srui.standard-widgets"));
    assert_eq!(set.get_version("org.srui.standard-widgets"), Some(1));
    assert_eq!(set.get_version("org.srui.unknown"), None);

    let profile = Profile::parse("org.srui.terminal/1").unwrap();
    assert!(set.remove(&profile));
    assert_eq!(set.len(), 1);
    assert!(!set.contains(&profile));
}

#[test]
fn test_capability_set_algebra() {
    let set_a =
        CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1", "org.srui.terminal/1"])
            .unwrap();

    let set_b =
        CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1", "org.srui.richtext/1"])
            .unwrap();

    // Union: A + B
    let union_set = set_a.union(&set_b);
    assert_eq!(union_set.len(), 3);
    assert!(union_set.contains_str("org.srui.standard-widgets/1"));
    assert!(union_set.contains_str("org.srui.terminal/1"));
    assert!(union_set.contains_str("org.srui.richtext/1"));

    // Intersection: A & B
    let inter_set = set_a.intersection(&set_b);
    assert_eq!(inter_set.len(), 1);
    assert!(inter_set.contains_str("org.srui.standard-widgets/1"));

    // Difference: A - B
    let diff_set = set_a.difference(&set_b);
    assert_eq!(diff_set.len(), 1);
    assert!(diff_set.contains_str("org.srui.terminal/1"));

    // Subsets & Supersets
    assert!(union_set.is_superset(&set_a));
    assert!(set_a.is_subset(&union_set));
    assert!(!set_a.is_superset(&set_b));
}

#[test]
fn test_capability_negotiation_matching_sets_succeed() {
    let client_offered = CapabilitySet::from_str_slice(&[
        "org.srui.standard-widgets/1",
        "org.srui.terminal/1",
        "org.srui.richtext/1",
    ])
    .unwrap();

    let server_required = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let server_optional = CapabilitySet::from_str_slice(&["org.srui.terminal/1"]).unwrap();

    let negotiated =
        CapabilitySet::negotiate(&client_offered, &server_required, &server_optional).unwrap();

    assert_eq!(negotiated.len(), 2);
    assert!(negotiated.contains_str("org.srui.standard-widgets/1"));
    assert!(negotiated.contains_str("org.srui.terminal/1"));
    assert!(!negotiated.contains_str("org.srui.richtext/1")); // not in server config
}

#[test]
fn test_capability_negotiation_missing_required_profile_fails_hard() {
    // Client offers terminal and richtext, but NOT standard-widgets
    let client_offered =
        CapabilitySet::from_str_slice(&["org.srui.terminal/1", "org.srui.richtext/1"]).unwrap();

    let server_required = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let server_optional = CapabilitySet::from_str_slice(&["org.srui.terminal/1"]).unwrap();

    let result = CapabilitySet::negotiate(&client_offered, &server_required, &server_optional);

    // §4 Invariant 13: Must fail explicitly with a hard error
    assert_eq!(
        result,
        Err(NegotiationError::UnsatisfiedRequiredProfiles {
            missing: vec![Profile::standard_widgets_v1()],
        })
    );

    let err_display = format!("{}", result.unwrap_err());
    assert!(err_display.contains("org.srui.standard-widgets/1"));
}

#[test]
fn test_capability_negotiation_missing_optional_profile_is_omitted_without_error() {
    // Client offers only required profile, not optional ones
    let client_offered = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();

    let server_required = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let server_optional =
        CapabilitySet::from_str_slice(&["org.srui.terminal/1", "org.srui.richtext/1"]).unwrap();

    let negotiated =
        CapabilitySet::negotiate(&client_offered, &server_required, &server_optional).unwrap();

    // Succeeded, and negotiated set contains ONLY the required profile
    assert_eq!(negotiated.len(), 1);
    assert!(negotiated.contains_str("org.srui.standard-widgets/1"));
    assert!(!negotiated.contains_str("org.srui.terminal/1"));
    assert!(!negotiated.contains_str("org.srui.richtext/1"));
}

#[test]
fn test_capability_negotiation_unknown_client_profiles_ignored() {
    let client_offered = CapabilitySet::from_str_slice(&[
        "org.srui.standard-widgets/1",
        "com.unsupported.plugin/1",
        "org.experimental.unknown/9",
    ])
    .unwrap();

    let server_required = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let server_optional = CapabilitySet::new();

    let negotiated =
        CapabilitySet::negotiate(&client_offered, &server_required, &server_optional).unwrap();

    assert_eq!(negotiated.len(), 1);
    assert!(negotiated.contains_str("org.srui.standard-widgets/1"));
    assert!(!negotiated.contains_str("com.unsupported.plugin/1"));
    assert!(!negotiated.contains_str("org.experimental.unknown/9"));
}

#[test]
fn test_capability_negotiation_version_mismatch_fails_required() {
    // Client offers standard-widgets version 2, but server requires version 1
    let client_offered = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/2"]).unwrap();

    let server_required = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let server_optional = CapabilitySet::new();

    let result = CapabilitySet::negotiate(&client_offered, &server_required, &server_optional);

    assert_eq!(
        result,
        Err(NegotiationError::UnsatisfiedRequiredProfiles {
            missing: vec![Profile::standard_widgets_v1()],
        })
    );
}

#[test]
fn test_server_capabilities_wrapper() {
    let server_caps = ServerCapabilities::new(
        CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap(),
        CapabilitySet::from_str_slice(&["org.srui.terminal/1"]).unwrap(),
    );

    let client_matching =
        CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1", "org.srui.terminal/1"])
            .unwrap();
    let negotiated = server_caps.negotiate(&client_matching).unwrap();
    assert_eq!(negotiated.len(), 2);

    let client_minimal = CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap();
    let negotiated_min = server_caps.negotiate(&client_minimal).unwrap();
    assert_eq!(negotiated_min.len(), 1);

    let client_incompatible = CapabilitySet::from_str_slice(&["org.srui.terminal/1"]).unwrap();
    assert!(server_caps.negotiate(&client_incompatible).is_err());
}

#[test]
fn test_well_known_profile_constants_and_factories() {
    assert_eq!(PROFILE_STANDARD_WIDGETS, "org.srui.standard-widgets");
    assert_eq!(PROFILE_TERMINAL, "org.srui.terminal");
    assert_eq!(PROFILE_RICHTEXT, "org.srui.richtext");
    assert_eq!(PROFILE_VECTOR_SCENE, "org.srui.vector-scene");
    assert_eq!(PROFILE_MEDIA_SURFACE, "org.srui.media-surface");
    assert_eq!(PROFILE_CODING, "org.srui.coding");

    assert_eq!(
        Profile::standard_widgets_v1().to_string(),
        "org.srui.standard-widgets/1"
    );
    assert_eq!(Profile::terminal_v1().to_string(), "org.srui.terminal/1");
    assert_eq!(Profile::richtext_v1().to_string(), "org.srui.richtext/1");
    assert_eq!(
        Profile::vector_scene_v1().to_string(),
        "org.srui.vector-scene/1"
    );
    assert_eq!(
        Profile::media_surface_v1().to_string(),
        "org.srui.media-surface/1"
    );
    assert_eq!(Profile::coding_v1().to_string(), "org.srui.coding/1");
}
