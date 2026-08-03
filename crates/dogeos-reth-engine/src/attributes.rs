use alloc::vec::Vec;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::{PayloadAttributes, PayloadId};
use reth_payload_primitives::PayloadAttributes as PayloadAttributesTrait;

/// Engine payload attributes with forced transactions and `noTxPool` support.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollPayloadAttributes {
    pub payload_attributes: PayloadAttributes,
    pub transactions: Option<Vec<Bytes>>,
    pub no_tx_pool: bool,
    pub block_data_hint: BlockDataHint,
    pub gas_limit: Option<u64>,
}

/// Optional block fields supplied by the sequencer.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockDataHint {
    pub extra_data: Option<Bytes>,
    pub state_root: Option<B256>,
    pub coinbase: Option<Address>,
    pub nonce: Option<u64>,
    pub difficulty: Option<U256>,
}

impl BlockDataHint {
    pub const fn is_empty(&self) -> bool {
        self.extra_data.is_none()
            && self.state_root.is_none()
            && self.coinbase.is_none()
            && self.nonce.is_none()
            && self.difficulty.is_none()
    }
}

impl PayloadAttributesTrait for ScrollPayloadAttributes {
    fn payload_id(&self, parent_hash: &B256) -> PayloadId {
        payload_id_scroll(parent_hash, self)
    }

    fn timestamp(&self) -> u64 {
        self.payload_attributes.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<alloy_eips::eip4895::Withdrawal>> {
        self.payload_attributes.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.payload_attributes.parent_beacon_block_root
    }
}

/// Generates a stable payload identifier over every DogeOS-specific input.
pub fn payload_id_scroll(parent: &B256, attributes: &ScrollPayloadAttributes) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_slice());
    hasher.update(attributes.payload_attributes.timestamp.to_be_bytes());
    hasher.update(attributes.payload_attributes.prev_randao.as_slice());
    hasher.update(
        attributes
            .payload_attributes
            .suggested_fee_recipient
            .as_slice(),
    );
    if let Some(withdrawals) = &attributes.payload_attributes.withdrawals {
        let mut encoded = Vec::new();
        withdrawals.encode(&mut encoded);
        hasher.update(encoded);
    }
    if let Some(root) = attributes.payload_attributes.parent_beacon_block_root {
        hasher.update(root);
    }
    if attributes.no_tx_pool
        || attributes
            .transactions
            .as_ref()
            .is_some_and(|txs| !txs.is_empty())
    {
        hasher.update([attributes.no_tx_pool as u8]);
        hasher.update(
            attributes
                .transactions
                .as_ref()
                .map_or(0, Vec::len)
                .to_be_bytes(),
        );
        if let Some(transactions) = &attributes.transactions {
            for transaction in transactions {
                hasher.update(keccak256(transaction));
            }
        }
    }
    if let Some(value) = &attributes.block_data_hint.extra_data {
        hasher.update(value);
    }
    if let Some(value) = attributes.block_data_hint.state_root {
        hasher.update(value);
    }
    if let Some(value) = attributes.block_data_hint.coinbase {
        hasher.update(value);
    }
    if let Some(value) = attributes.block_data_hint.nonce {
        hasher.update(value.to_be_bytes());
    }
    if let Some(value) = attributes.block_data_hint.difficulty {
        hasher.update(value.to_be_bytes::<32>());
    }
    if let Some(value) = attributes.gas_limit {
        hasher.update(value.to_be_bytes());
    }

    let output = hasher.finalize();
    PayloadId::new(
        output[..8]
            .try_into()
            .expect("SHA-256 output has at least eight bytes"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_fields_change_payload_id() {
        let parent = B256::repeat_byte(1);
        let base = ScrollPayloadAttributes::default();
        let mut changed = base.clone();
        changed.no_tx_pool = true;
        assert_ne!(base.payload_id(&parent), changed.payload_id(&parent));
    }

    #[test]
    fn attributes_roundtrip_json() {
        let attributes = ScrollPayloadAttributes {
            no_tx_pool: true,
            gas_limit: Some(10_000_000),
            ..Default::default()
        };
        let json = serde_json::to_string(&attributes).unwrap();
        assert!(json.contains("\"payloadAttributes\""));
        assert!(json.contains("\"noTxPool\":true"));
        assert_eq!(
            serde_json::from_str::<ScrollPayloadAttributes>(&json).unwrap(),
            attributes
        );
    }
}
