use crate::ids::{PropertyRef, TypeRef};
use crate::value::Value;
use std::fmt;

/// Strongly-typed 2D size tuple (width, height) in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Size({}x{})", self.width, self.height)
    }
}

/// Strongly-typed 2D point tuple (x, y) in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Point({}, {})", self.x, self.y)
    }
}

/// Strongly-typed 2D rectangle tuple (x, y, width, height) in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Rect({}, {}, {}x{})",
            self.x, self.y, self.width, self.height
        )
    }
}

/// Strongly-typed 4-edge insets tuple (top, leading, bottom, trailing).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeInsets {
    pub top: f64,
    pub leading: f64,
    pub bottom: f64,
    pub trailing: f64,
}

impl EdgeInsets {
    pub const fn new(top: f64, leading: f64, bottom: f64, trailing: f64) -> Self {
        Self {
            top,
            leading,
            bottom,
            trailing,
        }
    }
}

impl fmt::Display for EdgeInsets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EdgeInsets(top: {}, leading: {}, bottom: {}, trailing: {})",
            self.top, self.leading, self.bottom, self.trailing
        )
    }
}

/// Strongly-typed 1D continuous range tuple (start, length).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: u64,
    pub length: u64,
}

impl Range {
    pub const fn new(start: u64, length: u64) -> Self {
        Self { start, length }
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Range(start: {}, length: {})", self.start, self.length)
    }
}

/// Strongly-typed standard or custom enum token pair (enum_id, value_id) (§7.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnumToken {
    pub enum_id: u32,
    pub value_id: u32,
}

impl EnumToken {
    pub const fn new(enum_id: u32, value_id: u32) -> Self {
        Self { enum_id, value_id }
    }
}

impl fmt::Display for EnumToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnumToken({}:{})", self.enum_id, self.value_id)
    }
}

/// A property key-value pair.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub property: PropertyRef,
    pub value: Value,
}

impl Property {
    pub const fn new(property: PropertyRef, value: Value) -> Self {
        Self { property, value }
    }
}

impl fmt::Display for Property {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Property({}, {})", self.property, self.value)
    }
}

/// Small typed record representing a structured non-node composite value (§7.6).
#[derive(Debug, Clone, PartialEq)]
pub struct SmallRecord {
    pub type_ref: TypeRef,
    pub properties: Vec<Property>,
}

impl SmallRecord {
    pub const fn new(type_ref: TypeRef, properties: Vec<Property>) -> Self {
        Self {
            type_ref,
            properties,
        }
    }
}

impl fmt::Display for SmallRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SmallRecord({}, {} properties)",
            self.type_ref,
            self.properties.len()
        )
    }
}
