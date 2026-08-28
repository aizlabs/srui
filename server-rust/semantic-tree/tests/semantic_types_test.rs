use std::collections::{HashMap, HashSet};
use srui_semantic_tree::*;

#[test]
fn test_all_value_variants_construction_and_equality() {
    // 1. Null
    let null_val = Value::Null;
    assert_eq!(null_val, Value::Null);
    assert!(null_val.is_null());
    assert!(null_val.is_scalar());

    // 2. Bool
    let true_val = Value::Bool(true);
    let false_val = Value::Bool(false);
    assert_eq!(true_val, Value::from(true));
    assert_eq!(false_val, Value::from(false));
    assert_ne!(true_val, false_val);
    assert_eq!(true_val.as_bool(), Some(true));
    assert_eq!(false_val.as_bool(), Some(false));

    // 3. SignedInt
    let neg_int = Value::SignedInt(-1234567890);
    let pos_int = Value::SignedInt(1234567890);
    assert_eq!(neg_int, Value::from(-1234567890i64));
    assert_eq!(pos_int, Value::from(1234567890i64));
    assert_ne!(neg_int, pos_int);
    assert_eq!(neg_int.as_signed_int(), Some(-1234567890));

    // 4. UnsignedInt
    let uint_val = Value::UnsignedInt(9876543210);
    assert_eq!(uint_val, Value::from(9876543210u64));
    assert_eq!(uint_val.as_unsigned_int(), Some(9876543210));

    // 5. Float64
    let float_val = Value::Float64(0.71);
    assert_eq!(float_val, Value::from(0.71f64));
    assert_eq!(float_val.as_float64(), Some(0.71));

    // 6. String
    let string_val = Value::String("Hello SRUI".to_string());
    assert_eq!(string_val, Value::from("Hello SRUI"));
    assert_eq!(string_val.as_string(), Some("Hello SRUI"));

    // 7. NodeId
    let node_val = Value::NodeId(NodeId::new(101));
    assert_eq!(node_val, Value::from(NodeId::new(101)));
    assert_eq!(node_val.as_node_id(), Some(NodeId::new(101)));

    // 8. ItemId
    let item_val = Value::ItemId(ItemId::new(202));
    assert_eq!(item_val, Value::from(ItemId::new(202)));
    assert_eq!(item_val.as_item_id(), Some(ItemId::new(202)));

    // 9. ResourceHash
    let hash_bytes = [0x42u8; 32];
    let hash_val = Value::ResourceHash(ResourceHash::new(hash_bytes));
    assert_eq!(hash_val, Value::from(ResourceHash::new(hash_bytes)));
    assert_eq!(hash_val.as_resource_hash(), Some(ResourceHash::new(hash_bytes)));

    // 10. EnumToken
    let enum_val = Value::EnumToken(EnumToken::new(2, 2)); // ActionRole::primary
    assert_eq!(enum_val, Value::from(EnumToken::new(2, 2)));
    assert_eq!(enum_val.as_enum_token(), Some(EnumToken::new(2, 2)));

    // 11. Size tuple
    let size_val = Value::Size(Size::new(800.0, 600.0));
    assert_eq!(size_val, Value::from(Size::new(800.0, 600.0)));
    assert_eq!(size_val.as_size(), Some(Size::new(800.0, 600.0)));

    // 12. Point tuple
    let point_val = Value::Point(Point::new(50.0, 75.0));
    assert_eq!(point_val, Value::from(Point::new(50.0, 75.0)));
    assert_eq!(point_val.as_point(), Some(Point::new(50.0, 75.0)));

    // 13. Range tuple
    let range_val = Value::Range(Range::new(10, 50));
    assert_eq!(range_val, Value::from(Range::new(10, 50)));
    assert_eq!(range_val.as_range(), Some(Range::new(10, 50)));

    // 14. Rect tuple
    let rect_val = Value::Rect(Rect::new(10.0, 20.0, 100.0, 200.0));
    assert_eq!(rect_val, Value::from(Rect::new(10.0, 20.0, 100.0, 200.0)));
    assert_eq!(rect_val.as_rect(), Some(Rect::new(10.0, 20.0, 100.0, 200.0)));

    // 15. EdgeInsets tuple
    let insets_val = Value::EdgeInsets(EdgeInsets::new(5.0, 10.0, 15.0, 20.0));
    assert_eq!(insets_val, Value::from(EdgeInsets::new(5.0, 10.0, 15.0, 20.0)));
    assert_eq!(insets_val.as_edge_insets(), Some(EdgeInsets::new(5.0, 10.0, 15.0, 20.0)));

    // 16. List of scalars
    let list_data = vec![Value::from(10i64), Value::from(20i64), Value::from(30i64)];
    let list_val = Value::List(list_data.clone());
    assert_eq!(list_val, Value::from(list_data));
    assert_eq!(list_val.as_list().unwrap().len(), 3);
    assert!(!list_val.is_scalar());

    // 17. Small typed record
    let record = SmallRecord::new(
        TypeRef::standard(11), // Button
        vec![
            Property::new(PropertyRef::standard(1), Value::from("Click Me")),
            Property::new(PropertyRef::standard(7), Value::from(true)),
        ],
    );
    let record_val = Value::Record(record.clone());
    assert_eq!(record_val, Value::from(record));
    assert_eq!(record_val.as_record().unwrap().properties.len(), 2);
    assert!(!record_val.is_scalar());
}

#[test]
fn test_typeref_and_propertyref_hashing_and_maps() {
    let mut type_map: HashMap<TypeRef, &'static str> = HashMap::new();
    let mut prop_map: HashMap<PropertyRef, &'static str> = HashMap::new();
    let mut type_set: HashSet<TypeRef> = HashSet::new();
    let mut prop_set: HashSet<PropertyRef> = HashSet::new();

    // Populate TypeRef map & set
    type_map.insert(TypeRef::BUTTON, "Button");
    type_map.insert(TypeRef::TEXT, "Text");
    type_map.insert(TypeRef::new(1, 100), "CustomType");

    type_set.insert(TypeRef::BUTTON);
    type_set.insert(TypeRef::TEXT);
    type_set.insert(TypeRef::new(1, 100));

    assert_eq!(type_map.get(&TypeRef::standard(11)), Some(&"Button"));
    assert_eq!(type_map.get(&TypeRef::standard(9)), Some(&"Text"));
    assert_eq!(type_map.get(&TypeRef::new(1, 100)), Some(&"CustomType"));
    assert_eq!(type_map.get(&TypeRef::standard(1)), None);

    assert!(type_set.contains(&TypeRef::standard(11)));
    assert!(type_set.contains(&TypeRef::BUTTON));
    assert!(!type_set.contains(&TypeRef::SURFACE));

    // Populate PropertyRef map & set
    prop_map.insert(PropertyRef::LABEL, "label");
    prop_map.insert(PropertyRef::VALUE, "value");
    prop_map.insert(PropertyRef::new(1, 200), "custom_prop");

    prop_set.insert(PropertyRef::LABEL);
    prop_set.insert(PropertyRef::VALUE);
    prop_set.insert(PropertyRef::new(1, 200));

    assert_eq!(prop_map.get(&PropertyRef::standard(1)), Some(&"label"));
    assert_eq!(prop_map.get(&PropertyRef::standard(13)), Some(&"value"));
    assert_eq!(prop_map.get(&PropertyRef::new(1, 200)), Some(&"custom_prop"));
    assert_eq!(prop_map.get(&PropertyRef::standard(7)), None);

    assert!(prop_set.contains(&PropertyRef::standard(1)));
    assert!(prop_set.contains(&PropertyRef::LABEL));
    assert!(!prop_set.contains(&PropertyRef::ENABLED));
}

#[test]
fn test_standard_registry_lookups() {
    // Check known node types match registry.yaml
    assert_eq!(resolve_standard_node_type("Surface").unwrap(), TypeRef::SURFACE);
    assert_eq!(resolve_standard_node_type("Dialog").unwrap(), TypeRef::DIALOG);
    assert_eq!(resolve_standard_node_type("Row").unwrap(), TypeRef::ROW);
    assert_eq!(resolve_standard_node_type("Column").unwrap(), TypeRef::COLUMN);
    assert_eq!(resolve_standard_node_type("Grid").unwrap(), TypeRef::GRID);
    assert_eq!(resolve_standard_node_type("Spacer").unwrap(), TypeRef::SPACER);
    assert_eq!(resolve_standard_node_type("Separator").unwrap(), TypeRef::SEPARATOR);
    assert_eq!(resolve_standard_node_type("Scroll").unwrap(), TypeRef::SCROLL);
    assert_eq!(resolve_standard_node_type("Text").unwrap(), TypeRef::TEXT);
    assert_eq!(resolve_standard_node_type("RichText").unwrap(), TypeRef::RICHTEXT);
    assert_eq!(resolve_standard_node_type("Button").unwrap(), TypeRef::BUTTON);
    assert_eq!(resolve_standard_node_type("Toggle").unwrap(), TypeRef::TOGGLE);
    assert_eq!(resolve_standard_node_type("TextInput").unwrap(), TypeRef::TEXT_INPUT);
    assert_eq!(resolve_standard_node_type("TextArea").unwrap(), TypeRef::TEXT_AREA);
    assert_eq!(resolve_standard_node_type("Progress").unwrap(), TypeRef::PROGRESS);
    assert_eq!(resolve_standard_node_type("Image").unwrap(), TypeRef::IMAGE);
    assert_eq!(resolve_standard_node_type("List").unwrap(), TypeRef::LIST);
    assert_eq!(resolve_standard_node_type("Table").unwrap(), TypeRef::TABLE);
    assert_eq!(resolve_standard_node_type("Tree").unwrap(), TypeRef::TREE);
    assert_eq!(resolve_standard_node_type("Select").unwrap(), TypeRef::SELECT);
    assert_eq!(resolve_standard_node_type("ChoiceGroup").unwrap(), TypeRef::CHOICE_GROUP);
    assert_eq!(resolve_standard_node_type("Slider").unwrap(), TypeRef::SLIDER);
    assert_eq!(resolve_standard_node_type("NumberInput").unwrap(), TypeRef::NUMBER_INPUT);
    assert_eq!(resolve_standard_node_type("Tabs").unwrap(), TypeRef::TABS);
    assert_eq!(resolve_standard_node_type("Split").unwrap(), TypeRef::SPLIT);
    assert_eq!(resolve_standard_node_type("Menu").unwrap(), TypeRef::MENU);
    assert_eq!(resolve_standard_node_type("Toolbar").unwrap(), TypeRef::TOOLBAR);

    // Check all 27 standard node types round-trip with their names
    for (id, name) in STANDARD_NODE_TYPES {
        let type_ref = TypeRef::standard(*id);
        assert_eq!(type_ref.standard_name(), Some(*name));
        assert_eq!(TypeRef::resolve_standard(name).unwrap(), type_ref);
    }

    // Check all 30 standard properties round-trip with their names
    for (id, name) in STANDARD_PROPERTIES {
        let prop_ref = PropertyRef::standard(*id);
        assert_eq!(prop_ref.standard_name(), Some(*name));
        assert_eq!(PropertyRef::resolve_standard(name).unwrap(), prop_ref);
    }
}

#[test]
fn test_unknown_registry_lookup_fails_cleanly() {
    let bad_nodes = ["Window", "CheckBox", "Canvas", "UnknownNode", ""];
    for bad in bad_nodes {
        let res = resolve_standard_node_type(bad);
        match res {
            Err(RegistryLookupError::UnknownNodeType(name)) => assert_eq!(name, bad),
            other => panic!("Expected UnknownNodeType for {:?}, got {:?}", bad, other),
        }
    }

    let bad_props = ["title", "width", "height", "color", "background", "unknown_prop", ""];
    for bad in bad_props {
        let res = resolve_standard_property(bad);
        match res {
            Err(RegistryLookupError::UnknownProperty(name)) => assert_eq!(name, bad),
            other => panic!("Expected UnknownProperty for {:?}, got {:?}", bad, other),
        }
    }
}

#[test]
fn test_resource_hash_hex_encoding_and_error_handling() {
    let raw = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10,
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
        0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
    ];
    let hash = ResourceHash::new(raw);
    let hex = hash.to_hex();
    assert_eq!(hex, "0123456789abcdeffedcba987654321000112233445566778899aabbccddeeff");

    // Roundtrip
    let parsed = ResourceHash::from_hex(&hex).expect("parsed hex");
    assert_eq!(parsed, hash);

    // sha256: prefix roundtrip
    let prefixed = format!("sha256:{}", hex);
    let parsed_prefixed = ResourceHash::from_hex(&prefixed).expect("parsed prefixed");
    assert_eq!(parsed_prefixed, hash);

    // Invalid length
    assert!(matches!(
        ResourceHash::from_hex("0123456789abcdef"),
        Err(ParseResourceHashError::InvalidLength(16))
    ));

    // Invalid character
    let mut invalid_char_hex = hex.clone();
    invalid_char_hex.replace_range(0..2, "zz");
    assert!(matches!(
        ResourceHash::from_hex(&invalid_char_hex),
        Err(ParseResourceHashError::InvalidHexCharacter)
    ));
}
