//! Wire protocol conversion and serialization bridge (§16).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§16 Reference Wire Encoding**: Converts platform-neutral in-memory types
//!   ([`Transaction`], [`Operation`], [`Value`], [`Event`], [`NodeRecord`]) to and from
//!   Protocol Buffers messages defined in `protocol/srui.proto`.
//! - **Lossless Bidirectional Roundtrip**: Every valid in-memory structure can be serialized to
//!   wire format and deserialized back to an identical in-memory representation.
//! - **In-Memory Serialization**: High-performance encode/decode functions operate on byte slices
//!   and `Vec<u8>` buffers without socket I/O dependencies.

use bytes::BufMut;
use prost::Message;
use std::collections::HashMap;
use std::fmt;

use crate::event::{ClientInstanceId, Event, EventId};
use crate::ids::{NodeId, PropertyRef, TypeRef};
use crate::store::node::Node;
use crate::transaction::error::TxnError;
use crate::transaction::operation::{Operation, Revision};
use crate::transaction::record::Transaction;
use crate::value::types::Property;
use crate::value::wire::ValueConversionError;
use crate::value::Value;

/// Error encountered during wire conversion, encoding, or decoding (§16).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Protobuf decoding error (malformed varint, unexpected EOF, invalid wire type).
    ProtobufDecode(String),
    /// Missing a required protobuf field in wire message.
    MissingField(&'static str),
    /// Value conversion failure.
    ValueConversion(ValueConversionError),
    /// Invalid resource hash byte length (expected 32 bytes).
    InvalidResourceHashLength(usize),
    /// Transaction validation or revision mismatch error.
    Transaction(String),
    /// Event conversion error.
    Event(String),
    /// NodeRecord conversion error.
    NodeRecord(String),
    /// Operation conversion error.
    InvalidOperation(String),
    /// General wire error.
    Other(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtobufDecode(msg) => write!(f, "protobuf decode error: {}", msg),
            Self::MissingField(field) => {
                write!(f, "missing expected protobuf wire field: {}", field)
            }
            Self::ValueConversion(err) => write!(f, "value conversion error: {}", err),
            Self::InvalidResourceHashLength(len) => {
                write!(f, "expected 32-byte resource hash, got {} bytes", len)
            }
            Self::Transaction(msg) => write!(f, "transaction error: {}", msg),
            Self::Event(msg) => write!(f, "event error: {}", msg),
            Self::NodeRecord(msg) => write!(f, "node record error: {}", msg),
            Self::InvalidOperation(msg) => write!(f, "invalid operation: {}", msg),
            Self::Other(msg) => write!(f, "wire error: {}", msg),
        }
    }
}

impl std::error::Error for WireError {}

impl From<prost::DecodeError> for WireError {
    fn from(err: prost::DecodeError) -> Self {
        Self::ProtobufDecode(err.to_string())
    }
}

impl From<ValueConversionError> for WireError {
    fn from(err: ValueConversionError) -> Self {
        Self::ValueConversion(err)
    }
}

impl From<TxnError> for WireError {
    fn from(err: TxnError) -> Self {
        Self::Transaction(err.to_string())
    }
}

/// Generic in-memory node record representing node identity, type, hierarchy, and properties (§6.2, §16).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRecord {
    /// Unique identifier for this node within the session (§6.2).
    pub node_id: NodeId,
    /// Type reference in a namespace registry (§6.4, §7.2).
    pub node_type: TypeRef,
    /// Parent node ID in the hierarchy (`None` for root nodes).
    pub parent_id: Option<NodeId>,
    /// Child insertion index under parent (`None` represents append / default placement).
    pub child_index: Option<usize>,
    /// Defined property values on this node (§7.4).
    pub properties: Vec<Property>,
}

impl NodeRecord {
    /// Constructs a new `NodeRecord`.
    pub fn new(
        node_id: impl Into<NodeId>,
        node_type: TypeRef,
        parent_id: Option<NodeId>,
        child_index: Option<usize>,
        properties: impl IntoIterator<Item = Property>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            node_type,
            parent_id,
            child_index,
            properties: properties.into_iter().collect(),
        }
    }

    /// Constructs a `NodeRecord` from an existing store [`Node`] snapshot.
    pub fn from_node(node: &Node, child_index: Option<usize>) -> Self {
        let properties = node
            .properties
            .iter()
            .map(|(&p, v)| Property::new(p, v.clone()))
            .collect();
        Self {
            node_id: node.id,
            node_type: node.node_type,
            parent_id: node.parent_id,
            child_index,
            properties,
        }
    }

    /// Returns a reference to the property value if present in this record.
    pub fn get_property(&self, prop: PropertyRef) -> Option<&Value> {
        self.properties
            .iter()
            .find(|p| p.property == prop)
            .map(|p| &p.value)
    }

    /// Returns `true` if the property is defined in this record.
    pub fn has_property(&self, prop: PropertyRef) -> bool {
        self.properties.iter().any(|p| p.property == prop)
    }

    /// Converts this record to a protobuf wire [`srui_protocol::NodeRecord`].
    pub fn to_wire(&self) -> srui_protocol::NodeRecord {
        self.into()
    }

    /// Encodes this record directly to protobuf wire bytes.
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        encode_node_record(self)
    }

    /// Decodes a `NodeRecord` from protobuf wire representation.
    pub fn from_wire(wire: srui_protocol::NodeRecord) -> Result<Self, WireError> {
        Self::try_from(wire)
    }

    /// Decodes a `NodeRecord` from raw protobuf wire bytes.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        decode_node_record(bytes)
    }
}

// -----------------------------------------------------------------------------
// NodeRecord <-> srui_protocol::NodeRecord
// -----------------------------------------------------------------------------

impl From<&NodeRecord> for srui_protocol::NodeRecord {
    fn from(rec: &NodeRecord) -> Self {
        let properties = rec
            .properties
            .iter()
            .map(srui_protocol::Property::from)
            .collect();

        Self {
            node_id: rec.node_id.get(),
            r#type: Some(rec.node_type.into()),
            parent_id: rec.parent_id.map(|p| p.get()).unwrap_or(0),
            child_index: rec.child_index.map(|idx| idx as u32).unwrap_or(u32::MAX),
            properties,
        }
    }
}

impl From<NodeRecord> for srui_protocol::NodeRecord {
    fn from(rec: NodeRecord) -> Self {
        (&rec).into()
    }
}

impl TryFrom<srui_protocol::NodeRecord> for NodeRecord {
    type Error = WireError;

    fn try_from(wire: srui_protocol::NodeRecord) -> Result<Self, Self::Error> {
        let node_id = NodeId::new(wire.node_id);
        let node_type = wire
            .r#type
            .map(TypeRef::from)
            .ok_or(WireError::MissingField("NodeRecord.type"))?;
        let parent_id = if wire.parent_id == 0 {
            None
        } else {
            Some(NodeId::new(wire.parent_id))
        };
        let child_index = if wire.child_index == u32::MAX {
            None
        } else {
            Some(wire.child_index as usize)
        };

        let mut properties = Vec::with_capacity(wire.properties.len());
        for p in wire.properties {
            properties.push(Property::try_from(p).map_err(WireError::ValueConversion)?);
        }

        Ok(Self {
            node_id,
            node_type,
            parent_id,
            child_index,
            properties,
        })
    }
}

// -----------------------------------------------------------------------------
// NodeRecord <-> Operation::CreateNode
// -----------------------------------------------------------------------------

impl From<NodeRecord> for Operation {
    fn from(rec: NodeRecord) -> Self {
        let properties = rec
            .properties
            .into_iter()
            .map(|p| (p.property, p.value))
            .collect();
        Self::CreateNode {
            id: rec.node_id,
            node_type: rec.node_type,
            parent_id: rec.parent_id,
            child_index: rec.child_index,
            properties,
        }
    }
}

impl TryFrom<Operation> for NodeRecord {
    type Error = WireError;

    fn try_from(op: Operation) -> Result<Self, Self::Error> {
        match op {
            Operation::CreateNode {
                id,
                node_type,
                parent_id,
                child_index,
                properties,
            } => {
                let props = properties
                    .into_iter()
                    .map(|(p, v)| Property::new(p, v))
                    .collect();
                Ok(Self {
                    node_id: id,
                    node_type,
                    parent_id,
                    child_index,
                    properties: props,
                })
            }
            _ => Err(WireError::InvalidOperation(
                "expected Operation::CreateNode to convert to NodeRecord".to_string(),
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Event <-> srui_protocol::Event
// -----------------------------------------------------------------------------

impl From<&Event> for srui_protocol::Event {
    fn from(event: &Event) -> Self {
        let client_instance_id = event
            .client_instance_id
            .as_ref()
            .map(|id| id.0.clone())
            .unwrap_or_default();

        let arguments = event
            .arguments
            .iter()
            .map(|(p, v)| srui_protocol::Property {
                property: Some((*p).into()),
                value: Some(v.into()),
            })
            .collect();

        Self {
            client_instance_id,
            event_seq: event.event_seq,
            event_id: event.event_id.0.clone(),
            observed_revision: event.observed_revision.get(),
            node_id: event.node_id.get(),
            event_type: Some(event.event_type.into()),
            arguments,
        }
    }
}

impl From<Event> for srui_protocol::Event {
    fn from(event: Event) -> Self {
        (&event).into()
    }
}

impl TryFrom<srui_protocol::Event> for Event {
    type Error = WireError;

    fn try_from(wire: srui_protocol::Event) -> Result<Self, Self::Error> {
        let client_instance_id = if wire.client_instance_id.is_empty() {
            None
        } else {
            Some(ClientInstanceId::new(wire.client_instance_id))
        };

        let event_type = wire
            .event_type
            .map(TypeRef::from)
            .ok_or(WireError::MissingField("Event.event_type"))?;

        let mut arguments = HashMap::with_capacity(wire.arguments.len());
        for p in wire.arguments {
            let prop = Property::try_from(p).map_err(WireError::ValueConversion)?;
            arguments.insert(prop.property, prop.value);
        }

        Ok(Self {
            client_instance_id,
            event_seq: wire.event_seq,
            event_id: EventId::new(wire.event_id),
            observed_revision: Revision::new(wire.observed_revision),
            node_id: NodeId::new(wire.node_id),
            event_type,
            arguments,
        })
    }
}

// -----------------------------------------------------------------------------
// Inherent Methods on Domain Types
// -----------------------------------------------------------------------------

impl Transaction {
    /// Converts this transaction envelope to a protobuf wire [`srui_protocol::Transaction`].
    pub fn to_wire(&self) -> srui_protocol::Transaction {
        self.into()
    }

    /// Encodes this transaction directly to protobuf wire bytes.
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        encode_transaction(self)
    }

    /// Decodes a `Transaction` from a protobuf wire message.
    pub fn from_wire(wire: srui_protocol::Transaction) -> Result<Self, TxnError> {
        Self::try_from(wire)
    }

    /// Decodes a `Transaction` from raw protobuf wire bytes.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        decode_transaction(bytes)
    }
}

impl Operation {
    /// Converts this mutation operation to a protobuf wire [`srui_protocol::Operation`].
    pub fn to_wire(&self) -> srui_protocol::Operation {
        self.into()
    }

    /// Encodes this operation directly to protobuf wire bytes.
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        encode_operation(self)
    }

    /// Decodes an `Operation` from a protobuf wire message.
    pub fn from_wire(wire: srui_protocol::Operation) -> Result<Self, TxnError> {
        Self::try_from(wire)
    }

    /// Decodes an `Operation` from raw protobuf wire bytes.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        decode_operation(bytes)
    }
}

impl Event {
    /// Converts this semantic event to a protobuf wire [`srui_protocol::Event`].
    pub fn to_wire(&self) -> srui_protocol::Event {
        self.into()
    }

    /// Encodes this event directly to protobuf wire bytes.
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        encode_event(self)
    }

    /// Decodes an `Event` from a protobuf wire message.
    pub fn from_wire(wire: srui_protocol::Event) -> Result<Self, WireError> {
        Self::try_from(wire)
    }

    /// Decodes an `Event` from raw protobuf wire bytes.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        decode_event(bytes)
    }
}

impl Value {
    /// Converts this dynamic value to a protobuf wire [`srui_protocol::Value`].
    pub fn to_wire(&self) -> srui_protocol::Value {
        self.into()
    }

    /// Encodes this value directly to protobuf wire bytes.
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        encode_value(self)
    }

    /// Decodes a `Value` from a protobuf wire message.
    pub fn from_wire(wire: srui_protocol::Value) -> Result<Self, ValueConversionError> {
        Self::try_from(wire)
    }

    /// Decodes a `Value` from raw protobuf wire bytes.
    pub fn from_wire_bytes(bytes: &[u8]) -> Result<Self, WireError> {
        decode_value(bytes)
    }
}

// -----------------------------------------------------------------------------
// Pure In-Memory Encode / Decode Functions (§16)
// -----------------------------------------------------------------------------

/// Serializes a [`Transaction`] to Protocol Buffers wire bytes.
pub fn encode_transaction(txn: &Transaction) -> Vec<u8> {
    let wire_txn: srui_protocol::Transaction = txn.into();
    let mut buf = Vec::with_capacity(wire_txn.encoded_len());
    wire_txn
        .encode(&mut buf)
        .expect("Transaction protobuf encoding should never fail in-memory");
    buf
}

/// Encodes a [`Transaction`] into an existing buffer without cloning the domain transaction.
pub fn encode_transaction_ref(
    txn: &Transaction,
    buf: &mut impl BufMut,
) -> Result<(), prost::EncodeError> {
    let wire_txn: srui_protocol::Transaction = txn.into();
    wire_txn.encode(buf)
}

/// Deserializes a [`Transaction`] from Protocol Buffers wire bytes.
pub fn decode_transaction(bytes: &[u8]) -> Result<Transaction, WireError> {
    let wire_txn = srui_protocol::Transaction::decode(bytes)?;
    Transaction::try_from(wire_txn).map_err(WireError::from)
}

/// Serializes an [`Operation`] to Protocol Buffers wire bytes.
pub fn encode_operation(op: &Operation) -> Vec<u8> {
    let wire_op: srui_protocol::Operation = op.into();
    let mut buf = Vec::with_capacity(wire_op.encoded_len());
    wire_op
        .encode(&mut buf)
        .expect("Operation protobuf encoding should never fail in-memory");
    buf
}

/// Encodes an [`Operation`] into an existing buffer without cloning the domain operation.
pub fn encode_operation_ref(
    op: &Operation,
    buf: &mut impl BufMut,
) -> Result<(), prost::EncodeError> {
    let wire_op: srui_protocol::Operation = op.into();
    wire_op.encode(buf)
}

/// Deserializes an [`Operation`] from Protocol Buffers wire bytes.
pub fn decode_operation(bytes: &[u8]) -> Result<Operation, WireError> {
    let wire_op = srui_protocol::Operation::decode(bytes)?;
    Operation::try_from(wire_op).map_err(WireError::from)
}

/// Serializes an [`Event`] to Protocol Buffers wire bytes.
pub fn encode_event(event: &Event) -> Vec<u8> {
    let wire_event: srui_protocol::Event = event.into();
    let mut buf = Vec::with_capacity(wire_event.encoded_len());
    wire_event
        .encode(&mut buf)
        .expect("Event protobuf encoding should never fail in-memory");
    buf
}

/// Encodes an [`Event`] into an existing buffer without cloning the domain event.
pub fn encode_event_ref(event: &Event, buf: &mut impl BufMut) -> Result<(), prost::EncodeError> {
    let wire_event: srui_protocol::Event = event.into();
    wire_event.encode(buf)
}

/// Deserializes an [`Event`] from Protocol Buffers wire bytes.
pub fn decode_event(bytes: &[u8]) -> Result<Event, WireError> {
    let wire_event = srui_protocol::Event::decode(bytes)?;
    Event::try_from(wire_event)
}

/// Serializes a [`Value`] to Protocol Buffers wire bytes.
pub fn encode_value(value: &Value) -> Vec<u8> {
    let wire_value: srui_protocol::Value = value.into();
    let mut buf = Vec::with_capacity(wire_value.encoded_len());
    wire_value
        .encode(&mut buf)
        .expect("Value protobuf encoding should never fail in-memory");
    buf
}

/// Encodes a [`Value`] into an existing buffer without cloning the domain value.
pub fn encode_value_ref(value: &Value, buf: &mut impl BufMut) -> Result<(), prost::EncodeError> {
    let wire_value: srui_protocol::Value = value.into();
    wire_value.encode(buf)
}

/// Deserializes a [`Value`] from Protocol Buffers wire bytes.
pub fn decode_value(bytes: &[u8]) -> Result<Value, WireError> {
    let wire_value = srui_protocol::Value::decode(bytes)?;
    Value::try_from(wire_value).map_err(WireError::ValueConversion)
}

/// Serializes a [`NodeRecord`] to Protocol Buffers wire bytes.
pub fn encode_node_record(node: &NodeRecord) -> Vec<u8> {
    let wire_node: srui_protocol::NodeRecord = node.into();
    let mut buf = Vec::with_capacity(wire_node.encoded_len());
    wire_node
        .encode(&mut buf)
        .expect("NodeRecord protobuf encoding should never fail in-memory");
    buf
}

/// Encodes a [`NodeRecord`] into an existing buffer without cloning the domain record.
pub fn encode_node_record_ref(
    node: &NodeRecord,
    buf: &mut impl BufMut,
) -> Result<(), prost::EncodeError> {
    let wire_node: srui_protocol::NodeRecord = node.into();
    wire_node.encode(buf)
}

/// Deserializes a [`NodeRecord`] from Protocol Buffers wire bytes.
pub fn decode_node_record(bytes: &[u8]) -> Result<NodeRecord, WireError> {
    let wire_node = srui_protocol::NodeRecord::decode(bytes)?;
    NodeRecord::try_from(wire_node)
}

/// Serializes a top-level [`srui_protocol::SruiMessage`] envelope to Protocol Buffers wire bytes.
pub fn encode_message(msg: &srui_protocol::SruiMessage) -> Vec<u8> {
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf)
        .expect("SruiMessage protobuf encoding should never fail in-memory");
    buf
}

/// Deserializes a top-level [`srui_protocol::SruiMessage`] envelope from Protocol Buffers wire bytes.
pub fn decode_message(bytes: &[u8]) -> Result<srui_protocol::SruiMessage, WireError> {
    srui_protocol::SruiMessage::decode(bytes).map_err(WireError::from)
}
