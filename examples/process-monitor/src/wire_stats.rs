//! Wire-traffic measurement (§19, §23).

/// Framed size and operation mix of one committed transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransactionWireStats {
    /// Revision this transaction advanced the session to.
    pub revision: u64,
    /// Total operation count.
    pub operations: usize,
    /// `SET_PROPERTY` operations.
    pub set_property: usize,
    /// `MODEL_INSERT` operations.
    pub model_insert: usize,
    /// `MODEL_UPDATE` operations.
    pub model_update: usize,
    /// `MODEL_DELETE` operations.
    pub model_delete: usize,
    /// Every other operation kind.
    pub other: usize,
    /// Length of the length-delimited SRUI frame, before SSH encryption (§26).
    pub framed_bytes: usize,
}

impl std::fmt::Display for TransactionWireStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "revision={} ops={} set_property={} model_insert={} model_update={} model_delete={} other={} framed_bytes={}",
            self.revision,
            self.operations,
            self.set_property,
            self.model_insert,
            self.model_update,
            self.model_delete,
            self.other,
            self.framed_bytes
        )
    }
}

/// Measures a committed transaction exactly as `handle_connection` would put it on the wire.
///
/// The transaction is wrapped in the same [`srui_protocol::SruiMessage`] envelope and encoded with
/// the canonical varint length-delimited framing helper, so the reported size is the semantic SRUI
/// frame the client would receive (§26). SSH transport encryption and compression are not included.
///
/// A framing failure is returned rather than reported as a size: a transaction that cannot be
/// encoded also cannot be sent, and silently logging `framed_bytes=0` would hide exactly the
/// oversize-frame case these statistics exist to diagnose.
pub fn measure_transaction(
    transaction: &srui_protocol::Transaction,
) -> Result<TransactionWireStats, String> {
    use srui_protocol::operation::Op;

    let mut stats = TransactionWireStats {
        revision: transaction.new_revision,
        operations: transaction.operations.len(),
        ..TransactionWireStats::default()
    };
    for operation in &transaction.operations {
        match operation.op {
            Some(Op::SetProperty(_)) => stats.set_property += 1,
            Some(Op::ModelInsert(_)) => stats.model_insert += 1,
            Some(Op::ModelUpdate(_)) => stats.model_update += 1,
            Some(Op::ModelDelete(_)) => stats.model_delete += 1,
            _ => stats.other += 1,
        }
    }

    let envelope = srui_protocol::SruiMessage {
        msg: Some(srui_protocol::srui_message::Msg::Transaction(
            transaction.clone(),
        )),
    };
    stats.framed_bytes = srui_protocol::encode_framed(&envelope)
        .map(|bytes| bytes.len())
        .map_err(|error| {
            format!(
                "revision {} could not be framed: {error}",
                transaction.new_revision
            )
        })?;
    Ok(stats)
}
