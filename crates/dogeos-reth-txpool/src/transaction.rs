use alloy_consensus::{
    Transaction, Typed2718,
    transaction::{Recovered, TxHashRef},
};
use alloy_eips::{
    eip2718::Encodable2718,
    eip2930::AccessList,
    eip4844::{BlobTransactionValidationError, env_settings::KzgSettings},
    eip7594::BlobTransactionSidecarVariant,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, B256, Bytes, TxHash, TxKind, U256};
use dogeos_protocol_types::{ScrollPooledTransaction, ScrollTransaction};
use dogeos_reth_primitives::ScrollTransactionSigned;
use reth_primitives_traits::{InMemorySize, SignedTransaction};
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, EthPooledTransaction, PoolTransaction,
};
use std::sync::{Arc, OnceLock};

/// Pool wrapper that caches the exact EIP-2718 bytes required by DogeOS fee accounting.
#[derive(Debug, Clone, derive_more::Deref)]
pub struct DogeosPooledTransaction<Cons = ScrollTransactionSigned, Pooled = ScrollPooledTransaction>
{
    #[deref]
    inner: EthPooledTransaction<Cons>,
    pooled: core::marker::PhantomData<Pooled>,
    encoded_2718: OnceLock<Bytes>,
}

impl<Cons: SignedTransaction, Pooled> DogeosPooledTransaction<Cons, Pooled> {
    pub fn new(transaction: Recovered<Cons>, encoded_length: usize) -> Self {
        Self {
            inner: EthPooledTransaction::new(transaction, encoded_length),
            pooled: core::marker::PhantomData,
            encoded_2718: OnceLock::new(),
        }
    }

    pub fn encoded_2718(&self) -> &Bytes {
        self.encoded_2718
            .get_or_init(|| self.inner.transaction().encoded_2718().into())
    }
}

impl<Cons, Pooled> PoolTransaction for DogeosPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled>,
    Pooled: SignedTransaction + TryFrom<Cons, Error: core::error::Error>,
{
    type TryFromConsensusError = <Pooled as TryFrom<Cons>>::Error;
    type Consensus = Cons;
    type Pooled = Pooled;

    fn clone_into_consensus(&self) -> Recovered<Self::Consensus> {
        self.inner.transaction().clone()
    }

    fn consensus_ref(&self) -> Recovered<&Self::Consensus> {
        self.inner.transaction().as_recovered_ref()
    }

    fn into_consensus(self) -> Recovered<Self::Consensus> {
        self.inner.transaction
    }

    fn from_pooled(transaction: Recovered<Self::Pooled>) -> Self {
        let encoded_length = transaction.encode_2718_len();
        Self::new(transaction.convert(), encoded_length)
    }

    fn hash(&self) -> &TxHash {
        self.inner.transaction.tx_hash()
    }

    fn sender(&self) -> Address {
        self.inner.transaction.signer()
    }

    fn sender_ref(&self) -> &Address {
        self.inner.transaction.signer_ref()
    }

    fn cost(&self) -> &U256 {
        &self.inner.cost
    }

    fn encoded_length(&self) -> usize {
        self.inner.encoded_length
    }
}

impl<Cons: Typed2718, Pooled> Typed2718 for DogeosPooledTransaction<Cons, Pooled> {
    fn ty(&self) -> u8 {
        self.inner.ty()
    }
}

impl<Cons: InMemorySize, Pooled> InMemorySize for DogeosPooledTransaction<Cons, Pooled> {
    fn size(&self) -> usize {
        self.inner.size()
    }
}

impl<Cons, Pooled> Transaction for DogeosPooledTransaction<Cons, Pooled>
where
    Cons: Transaction,
    Pooled: core::fmt::Debug + Send + Sync + 'static,
{
    fn chain_id(&self) -> Option<u64> {
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

impl<Cons, Pooled> EthPoolTransaction for DogeosPooledTransaction<Cons, Pooled>
where
    Cons: SignedTransaction + From<Pooled>,
    Pooled: SignedTransaction + TryFrom<Cons, Error: core::error::Error>,
{
    fn take_blob(&mut self) -> EthBlobTransactionSidecar {
        EthBlobTransactionSidecar::None
    }

    fn try_into_pooled_eip4844(
        self,
        _sidecar: Arc<BlobTransactionSidecarVariant>,
    ) -> Option<Recovered<Self::Pooled>> {
        None
    }

    fn try_from_eip4844(
        _transaction: Recovered<Self::Consensus>,
        _sidecar: BlobTransactionSidecarVariant,
    ) -> Option<Self> {
        None
    }

    fn validate_blob(
        &self,
        _sidecar: &BlobTransactionSidecarVariant,
        _settings: &KzgSettings,
    ) -> Result<(), BlobTransactionValidationError> {
        Err(BlobTransactionValidationError::NotBlobTransaction(
            self.ty(),
        ))
    }
}

impl<Cons: ScrollTransaction, Pooled> ScrollTransaction for DogeosPooledTransaction<Cons, Pooled> {
    fn is_l1_message(&self) -> bool {
        self.inner.transaction.is_l1_message()
    }

    fn queue_index(&self) -> Option<u64> {
        self.inner.transaction.queue_index()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxLegacy};
    use alloy_primitives::Signature;

    #[test]
    fn caches_canonical_2718_bytes() {
        let transaction: ScrollTransactionSigned = Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            B256::repeat_byte(1),
        )
        .into();
        let expected: Bytes = transaction.encoded_2718().into();
        let pooled =
            DogeosPooledTransaction::<ScrollTransactionSigned, ScrollPooledTransaction>::new(
                Recovered::new_unchecked(transaction, Address::repeat_byte(2)),
                expected.len(),
            );

        assert_eq!(pooled.encoded_2718(), &expected);
        assert_eq!(
            pooled.encoded_2718().as_ptr(),
            pooled.encoded_2718().as_ptr()
        );
    }
}
