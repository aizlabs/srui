use srui_semantic_tree::*;

#[test]
fn test_construct_and_compare_all_17_value_variants() {
    let vals: Vec<Value> = vec![
        Value::Null,
        Value::Bool(true),
        Value::SignedInt(-42),
        Value::UnsignedInt(42),
        Value::Float64(std::f64::consts::PI),
        Value::String("hello world".to_string()),
        Value::NodeId(NodeId::new(10)),
        Value::ItemId(ItemId::new(20)),
        Value::ResourceHash(ResourceHash::new([0xab; 32])),
        Value::EnumToken(EnumToken::new(1, 2)),
        Value::Size(Size::new(100.0, 50.0)),
        Value::Point(Point::new(10.0, 20.0)),
        Value::Range(Range::new(0, 100)),
        Value::Rect(Rect::new(0.0, 0.0, 100.0, 50.0)),
        Value::EdgeInsets(EdgeInsets::new(8.0, 12.0, 8.0, 12.0)),
        Value::List(vec![Value::Bool(true), Value::Bool(false)]),
        Value::Record(SmallRecord::new(
            TypeRef::new(0, 1),
            vec![Property::new(PropertyRef::new(0, 1), Value::String("foo".to_string()))],
        )),
    ];

    assert_eq!(vals.len(), 17);

    for (i, v1) in vals.iter().enumerate() {
        for (j, v2) in vals.iter().enumerate() {
            if i == j {
                assert_eq!(v1, v2);
            } else {
                assert_ne!(v1, v2);
            }
        }
    }
}

#[test]
fn test_protobuf_wire_roundtrip_all_variants() {
    let vals: Vec<Value> = vec![
        Value::Null,
        Value::Bool(true),
        Value::SignedInt(-42),
        Value::UnsignedInt(42),
        Value::Float64(std::f64::consts::PI),
        Value::String("hello world".to_string()),
        Value::NodeId(NodeId::new(10)),
        Value::ItemId(ItemId::new(20)),
        Value::ResourceHash(ResourceHash::new([0xab; 32])),
        Value::EnumToken(EnumToken::new(1, 2)),
        Value::Size(Size::new(100.0, 50.0)),
        Value::Point(Point::new(10.0, 20.0)),
        Value::Range(Range::new(0, 100)),
        Value::Rect(Rect::new(0.0, 0.0, 100.0, 50.0)),
        Value::EdgeInsets(EdgeInsets::new(8.0, 12.0, 8.0, 12.0)),
        Value::List(vec![Value::Bool(true), Value::SignedInt(123)]),
        Value::Record(SmallRecord::new(
            TypeRef::new(0, 1),
            vec![Property::new(PropertyRef::new(0, 1), Value::String("foo".to_string()))],
        )),
    ];

    for val in vals {
        let wire: srui_protocol::Value = val.clone().into();
        let back = Value::try_from(wire).expect("protobuf roundtrip");
        assert_eq!(val, back);
    }
}
