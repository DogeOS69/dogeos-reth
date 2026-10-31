use alloy_consensus::{Sealed, SignableTransaction, Signed, TypedTransaction, error::ValueError};
use alloy_eips::eip2718::EIP4844_TX_TYPE_ID;
use alloy_primitives::{Address, Signature, TxKind, U256};
use alloy_rpc_types_eth::{AccessList, TransactionInput, TransactionRequest};
use dogeos_protocol_types::{ScrollTxEnvelope, ScrollTypedTransaction, TxL1Message};
use serde::{Deserialize, Serialize};

/// Error message for requests that ask for a blob (EIP-4844) transaction.
pub(crate) const BLOB_REQUEST_UNSUPPORTED: &str = "blob transactions are not supported";

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

    /// Returns true if the request asks for a blob transaction: type 3 or any EIP-4844 field.
    pub const fn is_blob_request(&self) -> bool {
        matches!(self.0.transaction_type, Some(EIP4844_TX_TYPE_ID)) || self.0.has_eip4844_fields()
    }

    /// Builds a supported transaction. Blob requests are rejected because DogeOS has no blob
    /// transaction type. The error returns the request for callers that fill missing fields.
    pub fn build_typed_tx(self) -> Result<ScrollTypedTransaction, ValueError<Self>> {
        if self.is_blob_request() {
            return Err(ValueError::new_static(self, BLOB_REQUEST_UNSUPPORTED));
        }
        let transaction = self
            .0
            .build_consensus_tx()
            .map_err(|error| ValueError::new_static(Self(error.tx), "required fields missing"))?;
        Ok(match transaction {
            TypedTransaction::Legacy(transaction) => ScrollTypedTransaction::Legacy(transaction),
            TypedTransaction::Eip2930(transaction) => ScrollTypedTransaction::Eip2930(transaction),
            TypedTransaction::Eip1559(transaction) => ScrollTypedTransaction::Eip1559(transaction),
            // Unreachable after the check above; rejected rather than lowered all the same.
            TypedTransaction::Eip4844(transaction) => {
                return Err(ValueError::new_static(
                    Self(transaction.into()),
                    BLOB_REQUEST_UNSUPPORTED,
                ));
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

    fn call_request() -> ScrollTransactionRequest {
        let mut request = ScrollTransactionRequest::default()
            .to(Address::repeat_byte(1))
            .gas_limit(21_000)
            .nonce(0);
        request.as_mut().chain_id = Some(1);
        request
    }

    fn eip1559_request() -> ScrollTransactionRequest {
        call_request()
            .max_fee_per_gas(10)
            .max_priority_fee_per_gas(1)
    }

    fn assert_blob_rejected(request: ScrollTransactionRequest) {
        let error = request.clone().build_typed_tx().unwrap_err();
        assert_eq!(error.to_string(), BLOB_REQUEST_UNSUPPORTED);
        assert_eq!(error.into_value(), request);
    }

    #[test]
    fn blob_requests_are_rejected() {
        let mut with_blob_fields = eip1559_request().transaction_type(3);
        with_blob_fields.as_mut().max_fee_per_blob_gas = Some(1);
        with_blob_fields.as_mut().blob_versioned_hashes = Some(vec![B256::repeat_byte(1)]);
        assert_blob_rejected(with_blob_fields);

        assert_blob_rejected(eip1559_request().transaction_type(3));

        let mut untyped_blob_hashes = eip1559_request();
        untyped_blob_hashes.as_mut().blob_versioned_hashes = Some(vec![B256::repeat_byte(1)]);
        assert_blob_rejected(untyped_blob_hashes);

        let mut untyped_blob_fee = eip1559_request();
        untyped_blob_fee.as_mut().max_fee_per_blob_gas = Some(1);
        assert_blob_rejected(untyped_blob_fee);
    }

    #[test]
    fn non_blob_requests_still_build() {
        let mut legacy = call_request();
        legacy.as_mut().gas_price = Some(10);
        assert!(matches!(
            legacy.clone().build_typed_tx().unwrap(),
            ScrollTypedTransaction::Legacy(_)
        ));

        let eip2930 = legacy.access_list(AccessList::default());
        assert!(matches!(
            eip2930.build_typed_tx().unwrap(),
            ScrollTypedTransaction::Eip2930(_)
        ));

        assert!(matches!(
            eip1559_request().build_typed_tx().unwrap(),
            ScrollTypedTransaction::Eip1559(_)
        ));

        let mut eip7702 = eip1559_request();
        eip7702.as_mut().authorization_list = Some(Vec::new());
        assert!(matches!(
            eip7702.build_typed_tx().unwrap(),
            ScrollTypedTransaction::Eip7702(_)
        ));
    }

    #[test]
    fn incomplete_request_returns_the_request() {
        let request = ScrollTransactionRequest::default().to(Address::repeat_byte(1));
        let error = request.clone().build_typed_tx().unwrap_err();
        assert_eq!(error.to_string(), "required fields missing");
        assert_eq!(error.into_value(), request);
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
