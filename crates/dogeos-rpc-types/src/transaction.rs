use alloy_consensus::{Transaction, Typed2718, transaction::Recovered};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Address, B256, BlockHash, Bytes, ChainId, TxKind, U256};
use alloy_serde::OtherFields;
use dogeos_protocol_types::{ScrollTransactionInfo, ScrollTxEnvelope};
use serde::{Deserialize, Serialize};

mod request;
pub use request::ScrollTransactionRequest;

/// RPC transaction preserving Scroll L1-message JSON fields.
#[derive(Clone, Debug, PartialEq, Eq, derive_more::Deref, derive_more::DerefMut)]
pub struct ScrollRpcTransaction {
    #[deref]
    #[deref_mut]
    pub inner: alloy_rpc_types_eth::Transaction<ScrollTxEnvelope>,
}

impl ScrollRpcTransaction {
    pub fn from_transaction(
        transaction: Recovered<ScrollTxEnvelope>,
        info: ScrollTransactionInfo,
    ) -> Self {
        let effective_gas_price = if transaction.is_l1_message() {
            0
        } else {
            info.inner.base_fee.map_or_else(
                || transaction.max_fee_per_gas(),
                |base_fee| {
                    transaction
                        .effective_tip_per_gas(base_fee)
                        .unwrap_or_default()
                        + u128::from(base_fee)
                },
            )
        };
        Self {
            inner: alloy_rpc_types_eth::Transaction {
                inner: transaction,
                block_hash: info.inner.block_hash,
                block_number: info.inner.block_number,
                transaction_index: info.inner.index,
                effective_gas_price: Some(effective_gas_price),
            },
        }
    }
}

impl Typed2718 for ScrollRpcTransaction {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl Transaction for ScrollRpcTransaction {
    fn chain_id(&self) -> Option<ChainId> {
        self.inner.chain_id()
    }
    fn nonce(&self) -> u64 {
        self.inner.nonce()
    }
    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }
    fn gas_price(&self) -> Option<u128> {
        self.inner.gas_price()
    }
    fn max_fee_per_gas(&self) -> u128 {
        self.inner.max_fee_per_gas()
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.max_priority_fee_per_gas()
    }
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.max_fee_per_blob_gas()
    }
    fn priority_fee_or_price(&self) -> u128 {
        self.inner.priority_fee_or_price()
    }
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.effective_gas_price(base_fee)
    }
    fn is_dynamic_fee(&self) -> bool {
        self.inner.is_dynamic_fee()
    }
    fn kind(&self) -> TxKind {
        self.inner.kind()
    }
    fn is_create(&self) -> bool {
        self.inner.is_create()
    }
    fn value(&self) -> U256 {
        self.inner.value()
    }
    fn input(&self) -> &Bytes {
        self.inner.input()
    }
    fn access_list(&self) -> Option<&AccessList> {
        self.inner.access_list()
    }
    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.inner.blob_versioned_hashes()
    }
    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.inner.authorization_list()
    }
}

impl alloy_network_primitives::TransactionResponse for ScrollRpcTransaction {
    fn tx_hash(&self) -> alloy_primitives::TxHash {
        self.inner.tx_hash()
    }
    fn block_hash(&self) -> Option<BlockHash> {
        self.inner.block_hash()
    }
    fn block_number(&self) -> Option<u64> {
        self.inner.block_number()
    }
    fn transaction_index(&self) -> Option<u64> {
        self.inner.transaction_index()
    }
    fn from(&self) -> Address {
        self.inner.from()
    }
}

impl AsRef<ScrollTxEnvelope> for ScrollRpcTransaction {
    fn as_ref(&self) -> &ScrollTxEnvelope {
        self.inner.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollL1MessageTransactionFields {
    pub queue_index: u64,
    pub sender: Address,
}

impl From<ScrollL1MessageTransactionFields> for OtherFields {
    fn from(value: ScrollL1MessageTransactionFields) -> Self {
        serde_json::to_value(value)
            .expect("transaction fields serialize")
            .try_into()
            .expect("transaction fields are a JSON object")
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionSerde {
    #[serde(flatten)]
    inner: ScrollTxEnvelope,
    #[serde(default)]
    block_hash: Option<BlockHash>,
    #[serde(default, with = "alloy_serde::quantity::opt")]
    block_number: Option<u64>,
    #[serde(default, with = "alloy_serde::quantity::opt")]
    transaction_index: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from: Option<Address>,
    #[serde(
        default,
        rename = "gasPrice",
        skip_serializing_if = "Option::is_none",
        with = "alloy_serde::quantity::opt"
    )]
    effective_gas_price: Option<u128>,
}

impl Serialize for ScrollRpcTransaction {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let recovered = &self.inner.inner;
        TransactionSerde {
            inner: recovered.inner().clone(),
            block_hash: self.inner.block_hash,
            block_number: self.inner.block_number,
            transaction_index: self.inner.transaction_index,
            from: Some(recovered.signer()),
            effective_gas_price: self
                .inner
                .effective_gas_price
                .filter(|_| recovered.gas_price().is_none()),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ScrollRpcTransaction {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = TransactionSerde::deserialize(deserializer)?;
        let from = value
            .from
            .or_else(|| match &value.inner {
                ScrollTxEnvelope::L1Message(transaction) => Some(transaction.sender),
                _ => None,
            })
            .ok_or_else(|| serde::de::Error::custom("missing `from` field"))?;
        let effective_gas_price = value
            .effective_gas_price
            .or_else(|| value.inner.gas_price());
        Ok(Self {
            inner: alloy_rpc_types_eth::Transaction {
                inner: Recovered::new_unchecked(value.inner, from),
                block_hash: value.block_hash,
                block_number: value.block_number,
                transaction_index: value.transaction_index,
                effective_gas_price,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_network_primitives::TransactionResponse;
    use dogeos_protocol_types::TxL1Message;

    #[test]
    fn l1_message_uses_zero_rpc_gas_price_and_sender_fallback() {
        let sender = Address::repeat_byte(7);
        let envelope: ScrollTxEnvelope = TxL1Message {
            sender,
            queue_index: 9,
            ..Default::default()
        }
        .into();
        let json = serde_json::to_value(ScrollRpcTransaction {
            inner: alloy_rpc_types_eth::Transaction {
                inner: Recovered::new_unchecked(envelope, sender),
                block_hash: None,
                block_number: None,
                transaction_index: None,
                effective_gas_price: Some(0),
            },
        })
        .unwrap();
        let roundtrip: ScrollRpcTransaction = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.from(), sender);
        assert_eq!(roundtrip.inner.effective_gas_price, Some(0));
    }
}
