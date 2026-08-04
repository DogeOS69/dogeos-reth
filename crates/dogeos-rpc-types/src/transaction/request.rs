use alloy_consensus::{
    Sealed, SignableTransaction, Signed, TxEip1559, TxEip4844, TypedTransaction,
};
use alloy_primitives::{Address, Signature, TxKind, U256};
use alloy_rpc_types_eth::{AccessList, TransactionInput, TransactionRequest};
use dogeos_protocol_types::{ScrollTxEnvelope, ScrollTypedTransaction, TxL1Message};
use serde::{Deserialize, Serialize};

/// Transaction request that builds the inherited Scroll transaction family.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    derive_more::From,
    derive_more::AsRef,
    derive_more::AsMut,
    Serialize,
    Deserialize,
)]
#[serde(transparent)]
pub struct ScrollTransactionRequest(TransactionRequest);

impl ScrollTransactionRequest {
    pub fn into_inner(self) -> TransactionRequest {
        self.0
    }

    pub const fn from(mut self, value: Address) -> Self {
        self.0.from = Some(value);
        self
    }
    pub const fn transaction_type(mut self, value: u8) -> Self {
        self.0.transaction_type = Some(value);
        self
    }
    pub const fn gas_limit(mut self, value: u64) -> Self {
        self.0.gas = Some(value);
        self
    }
    pub const fn nonce(mut self, value: u64) -> Self {
        self.0.nonce = Some(value);
        self
    }
    pub const fn max_fee_per_gas(mut self, value: u128) -> Self {
        self.0.max_fee_per_gas = Some(value);
        self
    }
    pub const fn max_priority_fee_per_gas(mut self, value: u128) -> Self {
        self.0.max_priority_fee_per_gas = Some(value);
        self
    }
    pub const fn to(mut self, value: Address) -> Self {
        self.0.to = Some(TxKind::Call(value));
        self
    }
    pub const fn value(mut self, value: U256) -> Self {
        self.0.value = Some(value);
        self
    }
    pub fn access_list(mut self, value: AccessList) -> Self {
        self.0.access_list = Some(value);
        self
    }
    pub fn input(mut self, value: TransactionInput) -> Self {
        self.0.input = value;
        self
    }

    /// Builds a supported transaction. Blob requests retain their ordinary transaction fields but
    /// are lowered to EIP-1559 because DogeOS has no blob transaction type.
    #[allow(clippy::result_large_err)] // Preserve the request for callers that fill missing fields.
    pub fn build_typed_tx(self) -> Result<ScrollTypedTransaction, Self> {
        let transaction = self
            .0
            .build_consensus_tx()
            .map_err(|error| Self(error.tx))?;
        Ok(match transaction {
            TypedTransaction::Legacy(transaction) => ScrollTypedTransaction::Legacy(transaction),
            TypedTransaction::Eip2930(transaction) => ScrollTypedTransaction::Eip2930(transaction),
            TypedTransaction::Eip1559(transaction) => ScrollTypedTransaction::Eip1559(transaction),
            TypedTransaction::Eip4844(transaction) => {
                let transaction: TxEip4844 = transaction.into();
                ScrollTypedTransaction::Eip1559(TxEip1559 {
                    chain_id: transaction.chain_id,
                    nonce: transaction.nonce,
                    gas_limit: transaction.gas_limit,
                    max_priority_fee_per_gas: transaction.max_priority_fee_per_gas,
                    max_fee_per_gas: transaction.max_fee_per_gas,
                    to: TxKind::Call(transaction.to),
                    value: transaction.value,
                    access_list: transaction.access_list,
                    input: transaction.input,
                })
            }
            TypedTransaction::Eip7702(transaction) => ScrollTypedTransaction::Eip7702(transaction),
        })
    }
}

#[cfg(feature = "reth")]
impl reth_rpc_traits::SignableTxRequest<ScrollTxEnvelope> for ScrollTransactionRequest {
    async fn try_build_and_sign(
        self,
        signer: impl alloy_network::TxSigner<Signature> + Send,
    ) -> Result<ScrollTxEnvelope, reth_rpc_traits::SignTxRequestError> {
        let mut transaction = self
            .build_typed_tx()
            .map_err(|_| reth_rpc_traits::SignTxRequestError::InvalidTransactionRequest)?;
        let signature = signer.sign_transaction(&mut transaction).await?;
        Ok(transaction.into_signed(signature).into())
    }
}

impl From<TxL1Message> for ScrollTransactionRequest {
    fn from(transaction: TxL1Message) -> Self {
        Self(TransactionRequest {
            from: Some(transaction.sender),
            to: Some(transaction.to.into()),
            value: Some(transaction.value),
            gas: Some(transaction.gas_limit),
            input: transaction.input.into(),
            ..Default::default()
        })
    }
}

impl From<Sealed<TxL1Message>> for ScrollTransactionRequest {
    fn from(transaction: Sealed<TxL1Message>) -> Self {
        transaction.into_inner().into()
    }
}

impl<T> From<Signed<T, Signature>> for ScrollTransactionRequest
where
    T: SignableTransaction<Signature> + Into<TransactionRequest>,
{
    fn from(transaction: Signed<T, Signature>) -> Self {
        Self(transaction.strip_signature().into())
    }
}

impl From<ScrollTypedTransaction> for ScrollTransactionRequest {
    fn from(transaction: ScrollTypedTransaction) -> Self {
        match transaction {
            ScrollTypedTransaction::Legacy(value) => Self(value.into()),
            ScrollTypedTransaction::Eip2930(value) => Self(value.into()),
            ScrollTypedTransaction::Eip1559(value) => Self(value.into()),
            ScrollTypedTransaction::Eip7702(value) => Self(value.into()),
            ScrollTypedTransaction::L1Message(value) => value.into(),
        }
    }
}

impl From<ScrollTxEnvelope> for ScrollTransactionRequest {
    fn from(transaction: ScrollTxEnvelope) -> Self {
        #[allow(unreachable_patterns)]
        match transaction {
            ScrollTxEnvelope::Legacy(value) => value.into(),
            ScrollTxEnvelope::Eip2930(value) => value.into(),
            ScrollTxEnvelope::Eip1559(value) => value.into(),
            ScrollTxEnvelope::Eip7702(value) => value.into(),
            ScrollTxEnvelope::L1Message(value) => value.into(),
            _ => Self::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn blob_request_is_lowered_to_eip1559() {
        let mut request = ScrollTransactionRequest::default()
            .transaction_type(3)
            .to(Address::repeat_byte(1))
            .gas_limit(21_000)
            .nonce(0)
            .max_fee_per_gas(10)
            .max_priority_fee_per_gas(1);
        request.as_mut().chain_id = Some(1);
        request.as_mut().max_fee_per_blob_gas = Some(1);
        request.as_mut().blob_versioned_hashes = Some(vec![B256::repeat_byte(1)]);
        assert!(matches!(
            request.build_typed_tx().unwrap(),
            ScrollTypedTransaction::Eip1559(_)
        ));
    }

    #[test]
    fn l1_message_request_preserves_execution_fields() {
        let message = TxL1Message {
            sender: Address::repeat_byte(2),
            to: Address::repeat_byte(3),
            gas_limit: 123,
            value: U256::from(4),
            ..Default::default()
        };
        let request: ScrollTransactionRequest = message.into();
        assert_eq!(request.as_ref().from, Some(Address::repeat_byte(2)));
        assert_eq!(request.as_ref().gas, Some(123));
    }
}
