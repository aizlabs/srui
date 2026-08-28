use crate::ids::{ItemId, NodeId, PropertyRef, ResourceHash, TypeRef};
use crate::value::types::{EdgeInsets, EnumToken, Point, Property, Range, Rect, Size, SmallRecord};
use crate::value::Value;
use std::fmt;

/// Error converting between protobuf wire representations and in-memory Value types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueConversionError {
    MissingField(&'static str),
    InvalidResourceHashLength(usize),
    InvalidNestedStructure(String),
}

impl fmt::Display for ValueConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing expected protobuf wire field: {}", field),
            Self::InvalidResourceHashLength(len) => {
                write!(f, "expected 32-byte resource hash, got {} bytes", len)
            }
            Self::InvalidNestedStructure(msg) => write!(f, "invalid nested structure: {}", msg),
        }
    }
}

impl std::error::Error for ValueConversionError {}

// In-Memory -> Wire Proto (borrowed: avoids cloning the full domain Value tree)
impl From<&Value> for srui_protocol::Value {
    fn from(val: &Value) -> Self {
        use srui_protocol::value::Value as WireVal;
        let inner = match val {
            Value::Null => WireVal::NullValue(srui_protocol::NullValue::NullValue as i32),
            Value::Bool(b) => WireVal::BoolValue(*b),
            Value::SignedInt(i) => WireVal::IntValue(*i),
            Value::UnsignedInt(u) => WireVal::UintValue(*u),
            Value::Float64(f) => WireVal::FloatValue(*f),
            Value::String(s) => WireVal::StringValue(s.clone()),
            Value::NodeId(id) => WireVal::NodeIdValue(id.0),
            Value::ItemId(id) => WireVal::ItemIdValue(id.0),
            Value::ResourceHash(h) => WireVal::ResourceHash(h.0.to_vec()),
            Value::EnumToken(e) => WireVal::EnumValue(srui_protocol::EnumValue {
                enum_id: e.enum_id,
                value_id: e.value_id,
            }),
            Value::Size(s) => WireVal::SizeValue(srui_protocol::SizeVal {
                width: s.width,
                height: s.height,
            }),
            Value::Point(p) => WireVal::PointValue(srui_protocol::PointVal {
                x: p.x,
                y: p.y,
            }),
            Value::Range(r) => WireVal::RangeValue(srui_protocol::RangeVal {
                location: r.start,
                length: r.length,
            }),
            Value::Rect(r) => WireVal::RectValue(srui_protocol::RectVal {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            }),
            Value::EdgeInsets(i) => WireVal::InsetsValue(srui_protocol::EdgeInsetsVal {
                top: i.top,
                leading: i.leading,
                bottom: i.bottom,
                trailing: i.trailing,
            }),
            Value::List(list) => {
                let wire_list: Vec<srui_protocol::Value> =
                    list.iter().map(srui_protocol::Value::from).collect();
                WireVal::ListValue(srui_protocol::ValueList { values: wire_list })
            }
            Value::Record(rec) => {
                let wire_rec = srui_protocol::SmallRecord {
                    r#type: Some(rec.type_ref.into()),
                    properties: rec
                        .properties
                        .iter()
                        .map(srui_protocol::Property::from)
                        .collect(),
                };
                WireVal::RecordValue(wire_rec)
            }
        };

        Self { value: Some(inner) }
    }
}

impl From<Value> for srui_protocol::Value {
    fn from(val: Value) -> Self {
        (&val).into()
    }
}

// Wire Proto -> In-Memory
impl TryFrom<srui_protocol::Value> for Value {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::Value) -> Result<Self, Self::Error> {
        use srui_protocol::value::Value as WireVal;
        let inner = wire
            .value
            .ok_or(ValueConversionError::MissingField("value"))?;

        match inner {
            WireVal::NullValue(_) => Ok(Self::Null),
            WireVal::BoolValue(b) => Ok(Self::Bool(b)),
            WireVal::IntValue(i) => Ok(Self::SignedInt(i)),
            WireVal::UintValue(u) => Ok(Self::UnsignedInt(u)),
            WireVal::FloatValue(f) => Ok(Self::Float64(f)),
            WireVal::StringValue(s) => Ok(Self::String(s)),
            WireVal::NodeIdValue(id) => Ok(Self::NodeId(NodeId::new(id))),
            WireVal::ItemIdValue(id) => Ok(Self::ItemId(ItemId::new(id))),
            WireVal::ResourceHash(bytes) => {
                if bytes.len() != 32 {
                    return Err(ValueConversionError::InvalidResourceHashLength(bytes.len()));
                }
                let mut hash_arr = [0u8; 32];
                hash_arr.copy_from_slice(&bytes);
                Ok(Self::ResourceHash(ResourceHash::new(hash_arr)))
            }
            WireVal::EnumValue(e) => Ok(Self::EnumToken(EnumToken::new(e.enum_id, e.value_id))),
            WireVal::SizeValue(s) => Ok(Self::Size(Size::new(s.width, s.height))),
            WireVal::PointValue(p) => Ok(Self::Point(Point::new(p.x, p.y))),
            WireVal::RangeValue(r) => Ok(Self::Range(Range::new(r.location, r.length))),
            WireVal::RectValue(r) => Ok(Self::Rect(Rect::new(r.x, r.y, r.width, r.height))),
            WireVal::InsetsValue(i) => Ok(Self::EdgeInsets(EdgeInsets::new(
                i.top,
                i.leading,
                i.bottom,
                i.trailing,
            ))),
            WireVal::ListValue(l) => {
                let mut list = Vec::with_capacity(l.values.len());
                for v in l.values {
                    list.push(Self::try_from(v)?);
                }
                Ok(Self::List(list))
            }
            WireVal::RecordValue(r) => {
                let type_ref = r
                    .r#type
                    .map(TypeRef::from)
                    .ok_or(ValueConversionError::MissingField("record.type"))?;

                let mut properties = Vec::with_capacity(r.properties.len());
                for p in r.properties {
                    properties.push(Property::try_from(p)?);
                }
                Ok(Self::Record(SmallRecord::new(type_ref, properties)))
            }
        }
    }
}

impl From<&Property> for srui_protocol::Property {
    fn from(prop: &Property) -> Self {
        Self {
            property: Some(prop.property.into()),
            value: Some((&prop.value).into()),
        }
    }
}

impl From<Property> for srui_protocol::Property {
    fn from(prop: Property) -> Self {
        (&prop).into()
    }
}

impl TryFrom<srui_protocol::Property> for Property {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::Property) -> Result<Self, Self::Error> {
        let property = wire
            .property
            .map(PropertyRef::from)
            .ok_or(ValueConversionError::MissingField("property"))?;

        let value = wire
            .value
            .map(Value::try_from)
            .transpose()?
            .unwrap_or(Value::Null);

        Ok(Self { property, value })
    }
}
