use srui_semantic_tree::*;

#[test]
fn test_node_id_newtype() {
    let id1 = NodeId::new(42);
    let id2 = NodeId::new(42);
    let id3 = NodeId::new(43);

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
    assert_eq!(id1.get(), 42);
    assert_eq!(u64::from(id1), 42);
    assert_eq!(NodeId::from(42u64), id1);
    assert_eq!(format!("{}", id1), "NodeId(42)");
}

#[test]
fn test_item_id_newtype() {
    let id1 = ItemId::new(100);
    let id2 = ItemId::new(100);
    let id3 = ItemId::new(101);

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
    assert_eq!(id1.get(), 100);
    assert_eq!(u64::from(id1), 100);
    assert_eq!(ItemId::from(100u64), id1);
    assert_eq!(format!("{}", id1), "ItemId(100)");
}

#[test]
fn test_type_ref_equality_and_hashing() {
    let t1 = TypeRef::new(0, 1);
    let t2 = TypeRef::standard(1);
    let t3 = TypeRef::new(1, 1);

    assert_eq!(t1, t2);
    assert_ne!(t1, t3);
    assert!(t1.is_standard());
    assert!(!t3.is_standard());
    assert_eq!(t1.standard_name(), Some("Surface"));
    assert_eq!(t3.standard_name(), None);
}

#[test]
fn test_property_ref_equality_and_hashing() {
    let p1 = PropertyRef::new(0, 1);
    let p2 = PropertyRef::standard(1);
    let p3 = PropertyRef::new(1, 1);

    assert_eq!(p1, p2);
    assert_ne!(p1, p3);
    assert!(p1.is_standard());
    assert!(!p3.is_standard());
    assert_eq!(p1.standard_name(), Some("label"));
    assert_eq!(p3.standard_name(), None);
}

#[test]
fn test_wire_proto_conversion() {
    let type_ref = TypeRef::new(0, 11);
    let wire_type: srui_protocol::TypeRef = type_ref.into();
    assert_eq!(wire_type.namespace_id, 0);
    assert_eq!(wire_type.local_id, 11);
    let back_type: TypeRef = wire_type.into();
    assert_eq!(type_ref, back_type);

    let prop_ref = PropertyRef::new(0, 1);
    let wire_prop: srui_protocol::PropertyRef = prop_ref.into();
    assert_eq!(wire_prop.namespace_id, 0);
    assert_eq!(wire_prop.local_id, 1);
    let back_prop: PropertyRef = wire_prop.into();
    assert_eq!(prop_ref, back_prop);
}

#[test]
fn test_resolve_known_registry_node_types() {
    assert_eq!(resolve_standard_node_type("Surface").unwrap(), TypeRef::SURFACE);
    assert_eq!(resolve_standard_node_type("Button").unwrap(), TypeRef::BUTTON);
    assert_eq!(resolve_standard_node_type("Text").unwrap(), TypeRef::TEXT);
}

#[test]
fn test_resolve_known_registry_properties() {
    assert_eq!(resolve_standard_property("label").unwrap(), PropertyRef::LABEL);
    assert_eq!(resolve_standard_property("enabled").unwrap(), PropertyRef::ENABLED);
    assert_eq!(resolve_standard_property("text").unwrap(), PropertyRef::TEXT);
}

#[test]
fn test_resolve_unknown_name_fails_clearly() {
    assert!(matches!(
        resolve_standard_node_type("NonExistentWidget"),
        Err(RegistryLookupError::UnknownNodeType(name)) if name == "NonExistentWidget"
    ));
    assert!(matches!(
        resolve_standard_property("non_existent_prop"),
        Err(RegistryLookupError::UnknownProperty(name)) if name == "non_existent_prop"
    ));
}

#[test]
fn test_resource_hash() {
    let bytes = [7u8; 32];
    let hash = ResourceHash::new(bytes);
    assert_eq!(hash.as_bytes(), &bytes);
    let hex = hash.to_hex();
    assert_eq!(hex, "0707070707070707070707070707070707070707070707070707070707070707");

    let parsed = ResourceHash::from_hex(&hex).unwrap();
    assert_eq!(hash, parsed);

    let prefixed = format!("sha256:{}", hex);
    let parsed_prefixed = ResourceHash::from_hex(&prefixed).unwrap();
    assert_eq!(hash, parsed_prefixed);
}

#[test]
fn test_resolve_standard_enum_values() {
    assert_eq!(
        resolve_standard_enum_value("ActionRole", "destructive"),
        Some(EnumToken::ACTION_ROLE_DESTRUCTIVE)
    );
    assert_eq!(
        resolve_standard_enum_value("EnumActionRole", "primary"),
        Some(EnumToken::ACTION_ROLE_PRIMARY)
    );
    assert_eq!(
        resolve_standard_enum_value("Visibility", "collapsed"),
        Some(EnumToken::VISIBILITY_COLLAPSED)
    );
    assert_eq!(
        lookup_standard_enum_value(2, "destructive"),
        Some(3)
    );
    assert_eq!(
        standard_enum_value_name(2, 3),
        Some("destructive")
    );
    assert_eq!(
        resolve_standard_enum_value("ActionRole", "non_existent"),
        None
    );
    assert_eq!(
        resolve_standard_enum_value("NonExistentEnum", "val"),
        None
    );
}
