//! Semantic value types representing the 17 dynamic value variants in SRUI.

pub mod types;
pub mod wire;

pub use types::{EdgeInsets, EnumToken, Point, Property, Range, Rect, Size, SmallRecord};
pub use wire::ValueConversionError;

use crate::ids::{ItemId, NodeId, ResourceHash};
use std::fmt;

/// Dynamic value holding any of the 17 SRUI value types (§7.6).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 1. Null / absence of value
    Null,
    /// 2. Boolean (true / false)
    Bool(bool),
    /// 3. 64-bit signed integer
    SignedInt(i64),
    /// 4. 64-bit unsigned integer
    UnsignedInt(u64),
    /// 5. 64-bit floating point number
    Float64(f64),
    /// 6. UTF-8 string
    String(String),
    /// 7. Strongly-typed node graph reference
    NodeId(NodeId),
    /// 8. Strongly-typed collection item reference
    ItemId(ItemId),
    /// 9. 256-bit SHA-256 binary resource hash
    ResourceHash(ResourceHash),
    /// 10. Namespace-qualified enum token
    EnumToken(EnumToken),
    /// 11. 2D size tuple (width, height)
    Size(Size),
    /// 12. 2D point tuple (x, y)
    Point(Point),
    /// 13. 1D range tuple (start, length)
    Range(Range),
    /// 14. 2D rectangle tuple (x, y, width, height)
    Rect(Rect),
    /// 15. 4-edge insets tuple (top, leading, bottom, trailing)
    EdgeInsets(EdgeInsets),
    /// 16. Homogeneous/heterogeneous list of scalar values
    List(Vec<Value>),
    /// 17. Small typed composite record
    Record(SmallRecord),
}

impl Value {
    /// Returns `true` if this value is scalar (§7.6).
    pub fn is_scalar(&self) -> bool {
        !matches!(self, Self::List(_) | Self::Record(_))
    }

    /// Returns `true` if this value is `Value::Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_signed_int(&self) -> Option<i64> {
        match self {
            Self::SignedInt(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_unsigned_int(&self) -> Option<u64> {
        match self {
            Self::UnsignedInt(u) => Some(*u),
            _ => None,
        }
    }

    pub fn as_float64(&self) -> Option<f64> {
        match self {
            Self::Float64(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_node_id(&self) -> Option<NodeId> {
        match self {
            Self::NodeId(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_item_id(&self) -> Option<ItemId> {
        match self {
            Self::ItemId(id) => Some(*id),
            _ => None,
        }
    }

    pub fn as_resource_hash(&self) -> Option<ResourceHash> {
        match self {
            Self::ResourceHash(h) => Some(*h),
            _ => None,
        }
    }

    pub fn as_enum_token(&self) -> Option<EnumToken> {
        match self {
            Self::EnumToken(e) => Some(*e),
            _ => None,
        }
    }

    pub fn as_size(&self) -> Option<Size> {
        match self {
            Self::Size(s) => Some(*s),
            _ => None,
        }
    }

    pub fn as_point(&self) -> Option<Point> {
        match self {
            Self::Point(p) => Some(*p),
            _ => None,
        }
    }

    pub fn as_range(&self) -> Option<Range> {
        match self {
            Self::Range(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_rect(&self) -> Option<Rect> {
        match self {
            Self::Rect(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_edge_insets(&self) -> Option<EdgeInsets> {
        match self {
            Self::EdgeInsets(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(l) => Some(l.as_slice()),
            _ => None,
        }
    }

    pub fn as_record(&self) -> Option<&SmallRecord> {
        match self {
            Self::Record(r) => Some(r),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{}", b),
            Self::SignedInt(i) => write!(f, "{}", i),
            Self::UnsignedInt(u) => write!(f, "{}", u),
            Self::Float64(fl) => write!(f, "{}", fl),
            Self::String(s) => write!(f, "{:?}", s),
            Self::NodeId(id) => write!(f, "{}", id),
            Self::ItemId(id) => write!(f, "{}", id),
            Self::ResourceHash(h) => write!(f, "{}", h),
            Self::EnumToken(e) => write!(f, "{}", e),
            Self::Size(s) => write!(f, "{}", s),
            Self::Point(p) => write!(f, "{}", p),
            Self::Range(r) => write!(f, "{}", r),
            Self::Rect(r) => write!(f, "{}", r),
            Self::EdgeInsets(i) => write!(f, "{}", i),
            Self::List(l) => write!(f, "List({} items)", l.len()),
            Self::Record(r) => write!(f, "{}", r),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Self::SignedInt(v)
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Self::SignedInt(v as i64)
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Self::UnsignedInt(v)
    }
}

impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self::UnsignedInt(v as u64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Float64(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

impl From<NodeId> for Value {
    fn from(v: NodeId) -> Self {
        Self::NodeId(v)
    }
}

impl From<ItemId> for Value {
    fn from(v: ItemId) -> Self {
        Self::ItemId(v)
    }
}

impl From<ResourceHash> for Value {
    fn from(v: ResourceHash) -> Self {
        Self::ResourceHash(v)
    }
}

impl From<[u8; 32]> for Value {
    fn from(v: [u8; 32]) -> Self {
        Self::ResourceHash(ResourceHash::new(v))
    }
}

impl From<EnumToken> for Value {
    fn from(v: EnumToken) -> Self {
        Self::EnumToken(v)
    }
}

impl From<Size> for Value {
    fn from(v: Size) -> Self {
        Self::Size(v)
    }
}

impl From<Point> for Value {
    fn from(v: Point) -> Self {
        Self::Point(v)
    }
}

impl From<Range> for Value {
    fn from(v: Range) -> Self {
        Self::Range(v)
    }
}

impl From<Rect> for Value {
    fn from(v: Rect) -> Self {
        Self::Rect(v)
    }
}

impl From<EdgeInsets> for Value {
    fn from(v: EdgeInsets) -> Self {
        Self::EdgeInsets(v)
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Self::List(v)
    }
}

impl From<SmallRecord> for Value {
    fn from(v: SmallRecord) -> Self {
        Self::Record(v)
    }
}
