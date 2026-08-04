use alloc::vec::Vec;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::{PayloadAttributes, PayloadId};
use reth_payload_primitives::PayloadAttributes as PayloadAttributesTrait;

/// DogeOS rollup payloads use the Scroll Engine V1 payload-ID domain.
const DOGEOS_PAYLOAD_VERSION: u8 = 1;

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

/// Legacy pre-Euclid block fields retained in the Engine JSON and payload-ID contract.
///
/// The Feynman+/post-Euclid payload builder intentionally ignores these values: execution owns the
/// state root and consensus fixes coinbase, nonce, difficulty, and extra data.
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

    let mut output = hasher.finalize();
    // Reth 2 computes the ID directly from RPC attributes and no longer passes the Engine method
    // version into a builder-attributes constructor. DogeOS only supports Scroll Engine V1, so
    // retain the oracle's version-domain byte explicitly.
    output[0] = DOGEOS_PAYLOAD_VERSION;
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
        assert_eq!(
            base.payload_id(&parent),
            PayloadId::new([0x01, 0xd8, 0x90, 0x28, 0xac, 0x91, 0x1a, 0x64])
        );
        let mut changed = base.clone();
        changed.no_tx_pool = true;
        assert_ne!(base.payload_id(&parent), changed.payload_id(&parent));

        changed = base.clone();
        changed.block_data_hint = BlockDataHint {
            extra_data: Some(Bytes::from_static(b"legacy")),
            state_root: Some(B256::repeat_byte(2)),
            coinbase: Some(Address::repeat_byte(3)),
            nonce: Some(4),
            difficulty: Some(U256::from(5)),
        };
        assert_ne!(base.payload_id(&parent), changed.payload_id(&parent));
    }

    #[test]
    fn payload_id_matches_the_scroll_v1_oracle_vector() {
        let attributes = ScrollPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1_728_933_301,
                prev_randao: alloy_primitives::b256!(
                    "9158595abbdab2c90635087619aa7042bbebe47642dfab3c9bfb934f6b082765"
                ),
                suggested_fee_recipient: alloy_primitives::address!(
                    "4200000000000000000000000000000000000011"
                ),
                withdrawals: Some(Vec::new()),
                parent_beacon_block_root: Some(alloy_primitives::b256!(
                    "8fe0193b9bf83cb7e5a08538e494fecc23046aab9a497af3704f4afdae3250ff"
                )),
            },
            transactions: Some(vec![alloy_primitives::bytes!(
                "7ef8f8a0dc19cfa777d90980e4875d0a548a881baaa3f83f14d1bc0d3038bc329350e54194deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e20000f424000000000000000000000000300000000670d6d890000000000000125000000000000000000000000000000000000000000000000000000000000000700000000000000000000000000000000000000000000000000000000000000014bf9181db6e381d4384bbf69c48b0ee0eed23c6ca26143c6d2544f9d39997a590000000000000000000000007f83d659683caf2767fd3c720981d51f5bc365bc"
            )]),
            no_tx_pool: false,
            block_data_hint: BlockDataHint {
                extra_data: Some(alloy_primitives::bytes!(
                    "476574682f76312e302e302f6c696e75782f676f312e342e32"
                )),
                state_root: Some(alloy_primitives::b256!(
                    "000000000000000000000000000000000000000000000000000000000000dead"
                )),
                coinbase: Some(alloy_primitives::address!(
                    "000000000000000000000000000000000000dead"
                )),
                nonce: Some(u64::MAX),
                difficulty: Some(U256::from(10)),
            },
            gas_limit: Some(10_000_000),
        };

        assert_eq!(
            payload_id_scroll(
                &alloy_primitives::b256!(
                    "3533bf30edaf9505d0810bf475cbe4e5f4b9889904b9845e83efdeab4e92eb1e"
                ),
                &attributes
            ),
            PayloadId::new([0x01, 0x63, 0x69, 0x37, 0x0c, 0x15, 0x5d, 0x4c])
        );
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
