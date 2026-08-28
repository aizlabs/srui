use prost::Message;

/// Length-delimited wire framing helpers implementing §16 reference Protobuf envelope encoding.
pub fn encode_framed<M: Message>(msg: &M) -> Result<Vec<u8>, prost::EncodeError> {
    let mut buf = Vec::with_capacity(msg.encoded_len() + 10);
    msg.encode_length_delimited(&mut buf)?;
    Ok(buf)
}

pub fn decode_framed<M: Message + Default>(mut buf: &[u8]) -> Result<M, prost::DecodeError> {
    M::decode_length_delimited(&mut buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn test_length_delimited_framing_roundtrip() {
        let msg = SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: 10,
                new_revision: 11,
                priority: 1,
                operations: vec![],
            })),
        };

        let framed_bytes = encode_framed(&msg).expect("encode framed");
        assert!(!framed_bytes.is_empty());
        let decoded: SruiMessage = decode_framed(&framed_bytes).expect("decode framed");
        assert_eq!(msg, decoded);
    }
}
