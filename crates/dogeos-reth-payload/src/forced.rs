use alloy_eips::eip2718::{Decodable2718, WithEncoded};
use dogeos_reth_engine::ScrollPayloadAttributes;
use dogeos_reth_primitives::ScrollTransactionSigned;

/// Decodes forced transactions once at the Engine/payload boundary, preserving their exact order
/// and original EIP-2718 bytes for execution and transaction-root calculation.
pub fn decode_forced_transactions(
    attributes: &ScrollPayloadAttributes,
) -> Result<Vec<WithEncoded<ScrollTransactionSigned>>, alloy_rlp::Error> {
    attributes
        .transactions
        .as_deref()
        .unwrap_or_default()
        .iter()
        .cloned()
        .map(|encoded| {
            let mut input = encoded.as_ref();
            let transaction =
                ScrollTransactionSigned::decode_2718(&mut input).map_err(alloy_rlp::Error::from)?;
            if !input.is_empty() {
                return Err(alloy_rlp::Error::UnexpectedLength);
            }
            Ok(WithEncoded::new(encoded, transaction))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Sealable;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::Bytes;
    use dogeos_protocol_types::{ScrollTxEnvelope, TxL1Message};

    fn encoded_message(queue_index: u64) -> Bytes {
        let tx = ScrollTxEnvelope::L1Message(
            TxL1Message {
                queue_index,
                gas_limit: 100_000,
                ..Default::default()
            }
            .seal_slow(),
        );
        tx.encoded_2718().into()
    }

    #[test]
    fn forced_transactions_preserve_attribute_order_and_bytes() {
        let encoded = vec![encoded_message(9), encoded_message(3)];
        let attributes = ScrollPayloadAttributes {
            transactions: Some(encoded.clone()),
            ..Default::default()
        };

        let decoded = decode_forced_transactions(&attributes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].encoded_bytes(), &encoded[0]);
        assert_eq!(decoded[1].encoded_bytes(), &encoded[1]);
        match (decoded[0].value(), decoded[1].value()) {
            (ScrollTxEnvelope::L1Message(first), ScrollTxEnvelope::L1Message(second)) => {
                assert_eq!(first.queue_index, 9);
                assert_eq!(second.queue_index, 3);
            }
            _ => panic!("expected L1 messages"),
        }
    }

    #[test]
    fn trailing_bytes_fail_closed() {
        let mut malformed = encoded_message(1).to_vec();
        malformed.push(0);
        let attributes = ScrollPayloadAttributes {
            transactions: Some(vec![malformed.into()]),
            ..Default::default()
        };
        assert!(decode_forced_transactions(&attributes).is_err());
    }

    #[test]
    fn canonical_zero_index_l1_message_preserves_oracle_bytes() {
        let encoded = alloy_primitives::bytes!(
            "7ef180830186a09400000000000000000000000000000000000000008080940000000000000000000000000000000000000000"
        );
        let attributes = ScrollPayloadAttributes {
            transactions: Some(vec![encoded.clone()]),
            ..Default::default()
        };

        let decoded = decode_forced_transactions(&attributes).unwrap();
        assert_eq!(decoded[0].encoded_bytes(), &encoded);
        match decoded[0].value() {
            ScrollTxEnvelope::L1Message(message) => {
                assert_eq!(message.queue_index, 0);
                assert_eq!(message.gas_limit, 100_000);
            }
            _ => panic!("expected L1 message"),
        }
    }
}
