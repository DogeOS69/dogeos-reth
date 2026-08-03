use alloy_consensus::{Receipt, ReceiptWithBloom};
use alloy_serde::OtherFields;
use dogeos_protocol_types::ScrollReceiptEnvelope;
use serde::{Deserialize, Serialize};

/// RPC receipt with the inherited `l1Fee` quantity field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollTransactionReceipt {
    #[serde(flatten)]
    pub inner:
        alloy_rpc_types_eth::TransactionReceipt<ScrollReceiptEnvelope<alloy_rpc_types_eth::Log>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "alloy_serde::quantity::opt"
    )]
    pub l1_fee: Option<u128>,
}

impl alloy_network_primitives::ReceiptResponse for ScrollTransactionReceipt {
    fn contract_address(&self) -> Option<alloy_primitives::Address> {
        self.inner.contract_address
    }
    fn status(&self) -> bool {
        self.inner.inner.status()
    }
    fn block_hash(&self) -> Option<alloy_primitives::BlockHash> {
        self.inner.block_hash
    }
    fn block_number(&self) -> Option<u64> {
        self.inner.block_number
    }
    fn transaction_hash(&self) -> alloy_primitives::TxHash {
        self.inner.transaction_hash
    }
    fn transaction_index(&self) -> Option<u64> {
        self.inner.transaction_index()
    }
    fn gas_used(&self) -> u64 {
        self.inner.gas_used()
    }
    fn effective_gas_price(&self) -> u128 {
        self.inner.effective_gas_price()
    }
    fn blob_gas_used(&self) -> Option<u64> {
        self.inner.blob_gas_used()
    }
    fn blob_gas_price(&self) -> Option<u128> {
        self.inner.blob_gas_price()
    }
    fn from(&self) -> alloy_primitives::Address {
        self.inner.from()
    }
    fn to(&self) -> Option<alloy_primitives::Address> {
        self.inner.to()
    }
    fn cumulative_gas_used(&self) -> u64 {
        self.inner.cumulative_gas_used()
    }
    fn state_root(&self) -> Option<alloy_primitives::B256> {
        self.inner.state_root()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollTransactionReceiptFields {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "alloy_serde::quantity::opt"
    )]
    pub l1_fee: Option<u128>,
}

impl From<ScrollTransactionReceiptFields> for OtherFields {
    fn from(value: ScrollTransactionReceiptFields) -> Self {
        serde_json::to_value(value)
            .expect("receipt fields serialize")
            .try_into()
            .expect("receipt fields are a JSON object")
    }
}

impl From<ScrollTransactionReceipt> for ScrollReceiptEnvelope<alloy_primitives::Log> {
    fn from(value: ScrollTransactionReceipt) -> Self {
        fn consensus(
            value: ReceiptWithBloom<Receipt<alloy_rpc_types_eth::Log>>,
        ) -> ReceiptWithBloom<Receipt<alloy_primitives::Log>> {
            let ReceiptWithBloom {
                logs_bloom,
                receipt,
            } = value;
            ReceiptWithBloom {
                receipt: Receipt {
                    status: receipt.status,
                    cumulative_gas_used: receipt.cumulative_gas_used,
                    logs: receipt.logs.into_iter().map(|log| log.inner).collect(),
                },
                logs_bloom,
            }
        }
        match value.inner.inner {
            ScrollReceiptEnvelope::Legacy(receipt) => Self::Legacy(consensus(receipt)),
            ScrollReceiptEnvelope::Eip2930(receipt) => Self::Eip2930(consensus(receipt)),
            ScrollReceiptEnvelope::Eip1559(receipt) => Self::Eip1559(consensus(receipt)),
            ScrollReceiptEnvelope::Eip7702(receipt) => Self::Eip7702(consensus(receipt)),
            ScrollReceiptEnvelope::L1Message(receipt) => Self::L1Message(consensus(receipt)),
            _ => unreachable!("unsupported Scroll receipt variant"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l1_fee_is_an_optional_quantity() {
        let fields = ScrollTransactionReceiptFields {
            l1_fee: Some(0x123),
        };
        assert_eq!(
            serde_json::to_value(fields).unwrap(),
            serde_json::json!({"l1Fee":"0x123"})
        );
        assert_eq!(
            serde_json::to_value(ScrollTransactionReceiptFields::default()).unwrap(),
            serde_json::json!({})
        );
    }
}
