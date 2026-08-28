//! Typed value system for the semantic tree model.
//!
//! Conforms to SRUI Specification v0.4:
//! - §6.5: Values (deliberately small typed value set)
//! - §14: Resource model (large blobs are resources, not properties)

use crate::ids::{ItemId, NodeId, PropertyRef, ResourceHash, TypeRef};
use std::fmt;

/// Discrete enumeration token identifying an enum type and variant (§6.5).
///
/// `enum_id` references an enum in the registry (e.g. ActionRole = 2),
/// and `value_id` references a specific variant (e.g. destructive = 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EnumToken {
    /// Registry enum identifier.
    pub enum_id: u32,
    /// Variant identifier within the enum.
    pub value_id: u32,
}

pub type EnumValue = EnumToken;

impl EnumToken {
    /// Creates a new `EnumToken`.
    pub const fn new(enum_id: u32, value_id: u32) -> Self {
        Self { enum_id, value_id }
    }
}

impl fmt::Display for EnumToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "enum({}:{})", self.enum_id, self.value_id)
    }
}

/// Semantic 2D size tuple (width, height) (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

pub type SizeVal = Size;

impl Size {
    /// Creates a new `Size`.
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// Semantic 2D point tuple (x, y) (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

pub type PointVal = Point;

impl Point {
    /// Creates a new `Point`.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Semantic 1D range tuple (location, length) (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Range {
    pub location: u64,
    pub length: u64,
}

pub type RangeVal = Range;

impl Range {
    /// Creates a new `Range`.
    pub const fn new(location: u64, length: u64) -> Self {
        Self { location, length }
    }
}

/// Semantic 2D rectangle tuple (x, y, width, height) (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub type RectVal = Rect;

impl Rect {
    /// Creates a new `Rect`.
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Semantic 2D edge insets tuple (top, leading, bottom, trailing) (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeInsets {
    pub top: f64,
    pub leading: f64,
    pub bottom: f64,
    pub trailing: f64,
}

pub type EdgeInsetsVal = EdgeInsets;

impl EdgeInsets {
    /// Creates a new `EdgeInsets`.
    pub const fn new(top: f64, leading: f64, bottom: f64, trailing: f64) -> Self {
        Self {
            top,
            leading,
            bottom,
            trailing,
        }
    }
}

/// A property binding associating a [`PropertyRef`] with a [`Value`] (§6.2, §6.5).
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub property: PropertyRef,
    pub value: Value,
}

impl Property {
    /// Creates a new `Property` binding.
    pub fn new(property: PropertyRef, value: impl Into<Value>) -> Self {
        Self {
            property,
            value: value.into(),
        }
    }
}

/// Small typed record containing a [`TypeRef`] and a list of properties (§6.5).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SmallRecord {
    pub type_ref: TypeRef,
    pub properties: Vec<Property>,
}

impl SmallRecord {
    /// Creates a new `SmallRecord`.
    pub fn new(type_ref: TypeRef, properties: Vec<Property>) -> Self {
        Self {
            type_ref,
            properties,
        }
    }
}

/// The deliberately small typed value set supported by SRUI Core (§6.5).
///
/// Large binary blobs are **not** values — large binary content is a Resource,
/// not a Value (per §6.5's closing line: *"Large blobs are resources, not properties"*).
/// Resources are content-addressed immutable binary objects referenced by [`Value::ResourceHash`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Null / empty value.
    Null,
    /// Boolean value (`true` or `false`).
    Bool(bool),
    /// 64-bit signed integer.
    SignedInt(i64),
    /// 64-bit unsigned integer.
    UnsignedInt(u64),
    /// 64-bit IEEE 754 floating-point number.
    Float64(f64),
    /// UTF-8 text string.
    String(String),
    /// Stable session-scoped semantic node identifier.
    NodeId(NodeId),
    /// Stable collection item identifier.
    ItemId(ItemId),
    /// Content-addressed 32-byte SHA-256 binary resource identifier.
    ResourceHash(ResourceHash),
    /// Discrete enum token.
    EnumToken(EnumToken),
    /// Semantic 2D size tuple (width, height).
    Size(Size),
    /// Semantic 2D point tuple (x, y).
    Point(Point),
    /// Semantic 1D range tuple (location, length).
    Range(Range),
    /// Semantic 2D rectangle tuple (x, y, width, height).
    Rect(Rect),
    /// Semantic 2D edge insets tuple (top, leading, bottom, trailing).
    EdgeInsets(EdgeInsets),
    /// List of scalar values.
    List(Vec<Value>),
    /// Small typed record.
    Record(SmallRecord),
}

impl Default for Value {
    fn default() -> Self {
        Self::Null
    }
}

impl Value {
    /// Returns `true` if this value is `Value::Null`.
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Returns `true` if this value is a scalar type (not a `List` or `Record`).
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Self::List(_) | Self::Record(_))
    }

    /// Returns `Some(bool)` if the value is `Value::Bool`.
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns `Some(i64)` if the value is `Value::SignedInt`.
    pub const fn as_signed_int(&self) -> Option<i64> {
        match self {
            Self::SignedInt(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns `Some(u64)` if the value is `Value::UnsignedInt`.
    pub const fn as_unsigned_int(&self) -> Option<u64> {
        match self {
            Self::UnsignedInt(u) => Some(*u),
            _ => None,
        }
    }

    /// Returns `Some(f64)` if the value is `Value::Float64`.
    pub const fn as_float64(&self) -> Option<f64> {
        match self {
            Self::Float64(f) => Some(*f),
            _ => None,
        }
    }

    /// Returns `Some(&str)` if the value is `Value::String`.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns `Some(NodeId)` if the value is `Value::NodeId`.
    pub const fn as_node_id(&self) -> Option<NodeId> {
        match self {
            Self::NodeId(id) => Some(*id),
            _ => None,
        }
    }

    /// Returns `Some(ItemId)` if the value is `Value::ItemId`.
    pub const fn as_item_id(&self) -> Option<ItemId> {
        match self {
            Self::ItemId(id) => Some(*id),
            _ => None,
        }
    }

    /// Returns `Some(ResourceHash)` if the value is `Value::ResourceHash`.
    pub const fn as_resource_hash(&self) -> Option<ResourceHash> {
        match self {
            Self::ResourceHash(h) => Some(*h),
            _ => None,
        }
    }

    /// Returns `Some(EnumToken)` if the value is `Value::EnumToken`.
    pub const fn as_enum_token(&self) -> Option<EnumToken> {
        match self {
            Self::EnumToken(e) => Some(*e),
            _ => None,
        }
    }

    /// Returns `Some(Size)` if the value is `Value::Size`.
    pub const fn as_size(&self) -> Option<Size> {
        match self {
            Self::Size(s) => Some(*s),
            _ => None,
        }
    }

    /// Returns `Some(Point)` if the value is `Value::Point`.
    pub const fn as_point(&self) -> Option<Point> {
        match self {
            Self::Point(p) => Some(*p),
            _ => None,
        }
    }

    /// Returns `Some(Range)` if the value is `Value::Range`.
    pub const fn as_range(&self) -> Option<Range> {
        match self {
            Self::Range(r) => Some(*r),
            _ => None,
        }
    }

    /// Returns `Some(Rect)` if the value is `Value::Rect`.
    pub const fn as_rect(&self) -> Option<Rect> {
        match self {
            Self::Rect(r) => Some(*r),
            _ => None,
        }
    }

    /// Returns `Some(EdgeInsets)` if the value is `Value::EdgeInsets`.
    pub const fn as_edge_insets(&self) -> Option<EdgeInsets> {
        match self {
            Self::EdgeInsets(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns `Some(&[Value])` if the value is `Value::List`.
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(l) => Some(l.as_slice()),
            _ => None,
        }
    }

    /// Returns `Some(&SmallRecord)` if the value is `Value::Record`.
    pub fn as_record(&self) -> Option<&SmallRecord> {
        match self {
            Self::Record(r) => Some(r),
            _ => None,
        }
    }
}

// -----------------------------------------------------------------------------
// Ergonomic `From` Conversions
// -----------------------------------------------------------------------------

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Self::Null
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Self::SignedInt(i)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Self::SignedInt(i as i64)
    }
}

impl From<u64> for Value {
    fn from(u: u64) -> Self {
        Self::UnsignedInt(u)
    }
}

impl From<u32> for Value {
    fn from(u: u32) -> Self {
        Self::UnsignedInt(u as u64)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Self::Float64(f)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.to_string())
    }
}

impl From<NodeId> for Value {
    fn from(id: NodeId) -> Self {
        Self::NodeId(id)
    }
}

impl From<ItemId> for Value {
    fn from(id: ItemId) -> Self {
        Self::ItemId(id)
    }
}

impl From<ResourceHash> for Value {
    fn from(hash: ResourceHash) -> Self {
        Self::ResourceHash(hash)
    }
}

impl From<EnumToken> for Value {
    fn from(token: EnumToken) -> Self {
        Self::EnumToken(token)
    }
}

impl From<Size> for Value {
    fn from(size: Size) -> Self {
        Self::Size(size)
    }
}

impl From<Point> for Value {
    fn from(point: Point) -> Self {
        Self::Point(point)
    }
}

impl From<Range> for Value {
    fn from(range: Range) -> Self {
        Self::Range(range)
    }
}

impl From<Rect> for Value {
    fn from(rect: Rect) -> Self {
        Self::Rect(rect)
    }
}

impl From<EdgeInsets> for Value {
    fn from(insets: EdgeInsets) -> Self {
        Self::EdgeInsets(insets)
    }
}

impl From<Vec<Value>> for Value {
    fn from(list: Vec<Value>) -> Self {
        Self::List(list)
    }
}

impl From<SmallRecord> for Value {
    fn from(record: SmallRecord) -> Self {
        Self::Record(record)
    }
}

// -----------------------------------------------------------------------------
// Protobuf Wire Conversions
// -----------------------------------------------------------------------------

/// Error converting protobuf wire value to in-memory [`Value`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueConversionError {
    /// Invalid resource hash byte length.
    InvalidResourceHashLength { expected: usize, actual: usize },
    /// Missing required sub-field in wire structure.
    MissingRequiredField(&'static str),
}

impl fmt::Display for ValueConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidResourceHashLength { expected, actual } => write!(
                f,
                "invalid resource hash byte length: expected {}, got {}",
                expected, actual
            ),
            Self::MissingRequiredField(field) => {
                write!(f, "missing required protobuf field: {}", field)
            }
        }
    }
}

impl std::error::Error for ValueConversionError {}

impl From<Value> for srui_protocol::Value {
    fn from(val: Value) -> Self {
        use srui_protocol::value::Value as WireVal;

        let wire = match val {
            Value::Null => WireVal::NullValue(0),
            Value::Bool(b) => WireVal::BoolValue(b),
            Value::SignedInt(i) => WireVal::IntValue(i),
            Value::UnsignedInt(u) => WireVal::UintValue(u),
            Value::Float64(f) => WireVal::FloatValue(f),
            Value::String(s) => WireVal::StringValue(s),
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
                location: r.location,
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
            Value::List(l) => WireVal::ListValue(srui_protocol::ValueList {
                values: l.into_iter().map(Into::into).collect(),
            }),
            Value::Record(rec) => WireVal::RecordValue(rec.into()),
        };

        srui_protocol::Value { value: Some(wire) }
    }
}

impl TryFrom<srui_protocol::Value> for Value {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::Value) -> Result<Self, Self::Error> {
        use srui_protocol::value::Value as WireVal;

        let val = match wire.value {
            None => Value::Null,
            Some(WireVal::NullValue(_)) => Value::Null,
            Some(WireVal::BoolValue(b)) => Value::Bool(b),
            Some(WireVal::IntValue(i)) => Value::SignedInt(i),
            Some(WireVal::UintValue(u)) => Value::UnsignedInt(u),
            Some(WireVal::FloatValue(f)) => Value::Float64(f),
            Some(WireVal::StringValue(s)) => Value::String(s),
            Some(WireVal::NodeIdValue(id)) => Value::NodeId(NodeId(id)),
            Some(WireVal::ItemIdValue(id)) => Value::ItemId(ItemId(id)),
            Some(WireVal::ResourceHash(bytes)) => {
                if bytes.len() != 32 {
                    return Err(ValueConversionError::InvalidResourceHashLength {
                        expected: 32,
                        actual: bytes.len(),
                    });
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Value::ResourceHash(ResourceHash(arr))
            }
            Some(WireVal::EnumValue(e)) => Value::EnumToken(EnumToken::new(e.enum_id, e.value_id)),
            Some(WireVal::SizeValue(s)) => Value::Size(Size::new(s.width, s.height)),
            Some(WireVal::PointValue(p)) => Value::Point(Point::new(p.x, p.y)),
            Some(WireVal::RangeValue(r)) => Value::Range(Range::new(r.location, r.length)),
            Some(WireVal::RectValue(r)) => Value::Rect(Rect::new(r.x, r.y, r.width, r.height)),
            Some(WireVal::InsetsValue(i)) => {
                Value::EdgeInsets(EdgeInsets::new(i.top, i.leading, i.bottom, i.trailing))
            }
            Some(WireVal::ListValue(l)) => {
                let mut list = Vec::with_capacity(l.values.len());
                for v in l.values {
                    list.push(Value::try_from(v)?);
                }
                Value::List(list)
            }
            Some(WireVal::RecordValue(r)) => Value::Record(SmallRecord::try_from(r)?),
        };

        Ok(val)
    }
}

impl From<SmallRecord> for srui_protocol::SmallRecord {
    fn from(record: SmallRecord) -> Self {
        srui_protocol::SmallRecord {
            r#type: Some(record.type_ref.into()),
            properties: record.properties.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<srui_protocol::SmallRecord> for SmallRecord {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::SmallRecord) -> Result<Self, Self::Error> {
        let type_ref = wire
            .r#type
            .map(Into::into)
            .ok_or(ValueConversionError::MissingRequiredField("SmallRecord.type"))?;

        let mut properties = Vec::with_capacity(wire.properties.len());
        for p in wire.properties {
            properties.push(Property::try_from(p)?);
        }

        Ok(SmallRecord {
            type_ref,
            properties,
        })
    }
}

impl From<Property> for srui_protocol::Property {
    fn from(prop: Property) -> Self {
        srui_protocol::Property {
            property: Some(prop.property.into()),
            value: Some(prop.value.into()),
        }
    }
}

impl TryFrom<srui_protocol::Property> for Property {
    type Error = ValueConversionError;

    fn try_from(wire: srui_protocol::Property) -> Result<Self, Self::Error> {
        let property = wire
            .property
            .map(Into::into)
            .ok_or(ValueConversionError::MissingRequiredField("Property.property"))?;

        let value = wire
            .value
            .map(Value::try_from)
            .transpose()?
            .unwrap_or(Value::Null);

        Ok(Property { property, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_construct_and_compare_all_17_value_variants() {
        // 1. Null
        let v_null = Value::Null;
        assert_eq!(v_null, Value::Null);
        assert!(v_null.is_null());
        assert!(v_null.is_scalar());

        // 2. Bool
        let v_bool = Value::Bool(true);
        assert_eq!(v_bool, Value::from(true));
        assert_ne!(v_bool, Value::Bool(false));
        assert_eq!(v_bool.as_bool(), Some(true));
        assert!(v_bool.is_scalar());

        // 3. SignedInt
        let v_sint = Value::SignedInt(-42);
        assert_eq!(v_sint, Value::from(-42i64));
        assert_eq!(v_sint.as_signed_int(), Some(-42));
        assert!(v_sint.is_scalar());

        // 4. UnsignedInt
        let v_uint = Value::UnsignedInt(12345);
        assert_eq!(v_uint, Value::from(12345u64));
        assert_eq!(v_uint.as_unsigned_int(), Some(12345));
        assert!(v_uint.is_scalar());

        // 5. Float64
        let v_float = Value::Float64(3.14159);
        assert_eq!(v_float, Value::from(3.14159f64));
        assert_eq!(v_float.as_float64(), Some(3.14159));
        assert!(v_float.is_scalar());

        // 6. String
        let v_str = Value::String("SRUI protocol".to_string());
        assert_eq!(v_str, Value::from("SRUI protocol"));
        assert_eq!(v_str.as_string(), Some("SRUI protocol"));
        assert!(v_str.is_scalar());

        // 7. NodeId
        let node_id = NodeId::new(42);
        let v_node = Value::NodeId(node_id);
        assert_eq!(v_node, Value::from(node_id));
        assert_eq!(v_node.as_node_id(), Some(node_id));
        assert!(v_node.is_scalar());

        // 8. ItemId
        let item_id = ItemId::new(99);
        let v_item = Value::ItemId(item_id);
        assert_eq!(v_item, Value::from(item_id));
        assert_eq!(v_item.as_item_id(), Some(item_id));
        assert!(v_item.is_scalar());

        // 9. ResourceHash
        let hash = ResourceHash::new([0xfe; 32]);
        let v_hash = Value::ResourceHash(hash);
        assert_eq!(v_hash, Value::from(hash));
        assert_eq!(v_hash.as_resource_hash(), Some(hash));
        assert!(v_hash.is_scalar());

        // 10. EnumToken
        let token = EnumToken::new(2, 3); // ActionRole::destructive
        let v_enum = Value::EnumToken(token);
        assert_eq!(v_enum, Value::from(token));
        assert_eq!(v_enum.as_enum_token(), Some(token));
        assert!(v_enum.is_scalar());

        // 11. Size
        let size = Size::new(120.0, 48.0);
        let v_size = Value::Size(size);
        assert_eq!(v_size, Value::from(size));
        assert_eq!(v_size.as_size(), Some(size));
        assert!(v_size.is_scalar());

        // 12. Point
        let point = Point::new(10.5, 20.25);
        let v_point = Value::Point(point);
        assert_eq!(v_point, Value::from(point));
        assert_eq!(v_point.as_point(), Some(point));
        assert!(v_point.is_scalar());

        // 13. Range
        let range = Range::new(0, 100);
        let v_range = Value::Range(range);
        assert_eq!(v_range, Value::from(range));
        assert_eq!(v_range.as_range(), Some(range));
        assert!(v_range.is_scalar());

        // 14. Rect
        let rect = Rect::new(0.0, 0.0, 640.0, 480.0);
        let v_rect = Value::Rect(rect);
        assert_eq!(v_rect, Value::from(rect));
        assert_eq!(v_rect.as_rect(), Some(rect));
        assert!(v_rect.is_scalar());

        // 15. EdgeInsets
        let insets = EdgeInsets::new(8.0, 12.0, 8.0, 12.0);
        let v_insets = Value::EdgeInsets(insets);
        assert_eq!(v_insets, Value::from(insets));
        assert_eq!(v_insets.as_edge_insets(), Some(insets));
        assert!(v_insets.is_scalar());

        // 16. List
        let list_elements = vec![Value::SignedInt(1), Value::SignedInt(2), Value::SignedInt(3)];
        let v_list = Value::List(list_elements.clone());
        assert_eq!(v_list, Value::from(list_elements));
        assert_eq!(v_list.as_list().unwrap().len(), 3);
        assert!(!v_list.is_scalar()); // Lists are collection types, not scalars

        // 17. Record
        let record = SmallRecord::new(
            TypeRef::BUTTON,
            vec![
                Property::new(PropertyRef::LABEL, Value::String("OK".to_string())),
                Property::new(PropertyRef::ENABLED, Value::Bool(true)),
            ],
        );
        let v_record = Value::Record(record.clone());
        assert_eq!(v_record, Value::from(record));
        assert_eq!(v_record.as_record().unwrap().properties.len(), 2);
        assert!(!v_record.is_scalar()); // Records are composite types, not scalars

        // Verify distinct variants are not equal
        assert_ne!(v_null, v_bool);
        assert_ne!(v_sint, v_uint);
        assert_ne!(v_float, v_str);
        assert_ne!(v_node, v_item);
        assert_ne!(v_size, v_point);
        assert_ne!(v_range, v_rect);
        assert_ne!(v_insets, v_list);
        assert_ne!(v_list, v_record);
    }

    #[test]
    fn test_protobuf_wire_roundtrip_all_variants() {
        let variants = vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::SignedInt(-999),
            Value::UnsignedInt(8888),
            Value::Float64(2.71828),
            Value::String("Test string".to_string()),
            Value::NodeId(NodeId::new(42)),
            Value::ItemId(ItemId::new(100)),
            Value::ResourceHash(ResourceHash::new([0xcd; 32])),
            Value::EnumToken(EnumToken::new(1, 4)),
            Value::Size(Size::new(200.0, 100.0)),
            Value::Point(Point::new(15.0, 30.0)),
            Value::Range(Range::new(5, 25)),
            Value::Rect(Rect::new(10.0, 20.0, 300.0, 150.0)),
            Value::EdgeInsets(EdgeInsets::new(4.0, 8.0, 4.0, 8.0)),
            Value::List(vec![
                Value::from("alpha"),
                Value::from("beta"),
                Value::from("gamma"),
            ]),
            Value::Record(SmallRecord::new(
                TypeRef::TEXT,
                vec![Property::new(
                    PropertyRef::TEXT,
                    Value::from("Nested label"),
                )],
            )),
        ];

        for original in variants {
            let wire: srui_protocol::Value = original.clone().into();
            let roundtripped = Value::try_from(wire).expect("convert back from wire");
            assert_eq!(original, roundtripped);
        }
    }
}
