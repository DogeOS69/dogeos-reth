//! State-aware base fees at the boundary between Reth maintenance and the actual pool.

use alloy_consensus::BlockHeader;
use alloy_eips::{
    eip4844::{BlobAndProofV1, BlobAndProofV2},
    eip7594::BlobTransactionSidecarVariant,
};
use alloy_primitives::{Address, B256, TxHash, map::AddressSet};
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use dogeos_reth_evm::{MAX_L2_BASE_FEE, ScrollBaseFeeProvider};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_eth_wire::HandleMempoolData;
use reth_execution_types::ChangedAccount;
use reth_primitives_traits::Recovered;
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_storage_errors::provider::ProviderError;
use reth_transaction_pool::{
    AddedTransactionOutcome, AllPoolTransactions, AllTransactionsEvents, BestTransactions,
    BestTransactionsAttributes, BlobStoreError, BlockInfo, CanonicalStateUpdate,
    GetPooledTransactionLimit, NewBlobSidecar, NewTransactionEvent, PoolResult, PoolSize,
    PoolTransaction, PropagatedTransactions, TransactionEvents, TransactionListenerKind,
    TransactionOrigin, TransactionPool, TransactionPoolExt, ValidPoolTransaction,
};
use std::{fmt, future::Future, sync::Arc};
use tokio::sync::mpsc::Receiver;

/// A maintenance-only view of a pool which replaces Reth's stateless fee prediction before
/// applying an update. RPC, networking and the payload builder keep using the inner pool.
///
/// This preserves upstream maintenance (including reorg reinsertion and account reconciliation)
/// without a Reth patch or a second task racing to correct an already-applied fee.
#[derive(Clone)]
pub struct DogeosPoolMaintenance<P, Client> {
    inner: P,
    client: Client,
}

impl<P, Client> DogeosPoolMaintenance<P, Client> {
    pub const fn new(inner: P, client: Client) -> Self {
        Self { inner, client }
    }
}

impl<P: fmt::Debug, Client> fmt::Debug for DogeosPoolMaintenance<P, Client> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DogeosPoolMaintenance")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<P, Client> DogeosPoolMaintenance<P, Client>
where
    P: TransactionPool,
    Client: BlockReaderIdExt + StateProviderFactory + ChainSpecProvider,
    Client::ChainSpec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    fn next_base_fee(&self, hash: B256) -> Result<u64, ProviderError> {
        let header = self
            .client
            .header(hash)?
            .ok_or(ProviderError::HeaderNotFound(hash.into()))?;
        // No base fee is required before London. In particular, do not call the Feynman-only
        // calculator for a header without a base fee.
        if header.base_fee_per_gas().is_none() {
            return Ok(0);
        }
        let state = self.client.state_by_block_hash(hash)?;
        ScrollBaseFeeProvider::new(self.client.chain_spec()).next_block_base_fee(
            &mut StateProviderDatabase::new(state),
            &header,
            header.timestamp(),
        )
    }

    fn pending_base_fee(&self, hash: B256) -> u64 {
        self.next_base_fee(hash).unwrap_or_else(|error| {
            // A queued notification can refer to state no longer available after a reorg.
            // Keep applying the head/account/mined-transaction update (dropping it would corrupt
            // pool state), but fail closed on the fee: keep the one the pool already enforces.
            // Lowering it would promote and propagate every parked underpriced transaction. A
            // stale fee at most delays propagation until a subsequent head can be read; payload
            // building computes the real fee and still selects parked transactions it unlocks.
            //
            // Before any head has been applied the pool has no fee of its own. Use the protocol
            // cap: no block's base fee can exceed it, so nothing underpriced is promoted, and
            // unlike Reth's stateless prediction it never parks transactions paying the cap.
            let previous = self.inner.block_info();
            let kept = if previous.last_seen_block_hash.is_zero() {
                MAX_L2_BASE_FEE
            } else {
                previous.pending_basefee
            };
            tracing::warn!(target: "reth::txpool", %hash, %error, kept_base_fee = kept,
                "Cannot read pending base fee; keeping the pool's previous base fee");
            kept
        })
    }
}

impl<P, Client> TransactionPoolExt for DogeosPoolMaintenance<P, Client>
where
    P: TransactionPoolExt,
    Client: BlockReaderIdExt + StateProviderFactory + ChainSpecProvider + Clone + Send + Sync,
    Client::ChainSpec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    type Block = P::Block;

    fn set_block_info(&self, mut info: BlockInfo) {
        info.pending_basefee = self.pending_base_fee(info.last_seen_block_hash);
        self.inner.set_block_info(info);
    }

    fn on_canonical_state_change(&self, mut update: CanonicalStateUpdate<'_, Self::Block>) {
        update.pending_block_base_fee = self.pending_base_fee(update.new_tip.hash());
        self.inner.on_canonical_state_change(update);
    }

    fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        self.inner.update_accounts(accounts);
    }
    fn delete_blob(&self, tx: B256) {
        self.inner.delete_blob(tx);
    }
    fn delete_blobs(&self, txs: Vec<B256>) {
        self.inner.delete_blobs(txs);
    }
    fn cleanup_blobs(&self) {
        self.inner.cleanup_blobs();
    }
}

// Forward the public pool API without copying any upstream pool/maintenance algorithms.
macro_rules! delegate_pool {
    ($(fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) $(-> $result:ty)?;)*) => {
        $(fn $name(&self $(, $arg: $ty)*) $(-> $result)? {
            self.inner.$name($($arg),*)
        })*
    };
}

impl<P, Client> TransactionPool for DogeosPoolMaintenance<P, Client>
where
    P: TransactionPool,
    Client: Clone + Send + Sync,
{
    type Transaction = P::Transaction;

    delegate_pool! {
        fn pool_size(&self) -> PoolSize;
        fn block_info(&self) -> BlockInfo;
        fn add_transaction_and_subscribe(
            &self,
            origin: TransactionOrigin,
            transaction: Self::Transaction,
        ) -> impl Future<Output = PoolResult<TransactionEvents>> + Send;
        fn add_transaction(
            &self,
            origin: TransactionOrigin,
            transaction: Self::Transaction,
        ) -> impl Future<Output = PoolResult<AddedTransactionOutcome>> + Send;
        fn add_transactions(
            &self,
            origin: TransactionOrigin,
            transactions: Vec<Self::Transaction>,
        ) -> impl Future<Output = Vec<PoolResult<AddedTransactionOutcome>>> + Send;
        fn add_transactions_with_origins(
            &self,
            transactions: Vec<(TransactionOrigin, Self::Transaction)>,
        ) -> impl Future<Output = Vec<PoolResult<AddedTransactionOutcome>>> + Send;
        fn transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents>;
        fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction>;
        fn pending_transactions_listener_for(&self, kind: TransactionListenerKind) -> Receiver<TxHash>;
        fn blob_transaction_sidecars_listener(&self) -> Receiver<NewBlobSidecar>;
        fn new_transactions_listener_for(
            &self,
            kind: TransactionListenerKind,
        ) -> Receiver<NewTransactionEvent<Self::Transaction>>;
        fn pooled_transaction_hashes(&self) -> Vec<TxHash>;
        fn pooled_transaction_hashes_max(&self, max: usize) -> Vec<TxHash>;
        fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pooled_transactions_max(
            &self,
            max: usize,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pooled_transaction_elements(
            &self,
            tx_hashes: Vec<TxHash>,
            limit: GetPooledTransactionLimit,
        ) -> Vec<<Self::Transaction as PoolTransaction>::Pooled>;
        fn get_pooled_transaction_element(
            &self,
            tx_hash: TxHash,
        ) -> Option<Recovered<<Self::Transaction as PoolTransaction>::Pooled>>;
        fn best_transactions(
            &self,
        ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;
        fn best_transactions_with_attributes(
            &self,
            best_transactions_attributes: BestTransactionsAttributes,
        ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>>;
        fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pending_transactions_max(
            &self,
            max: usize,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn pending_and_queued_txn_count(&self) -> (usize, usize);
        fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction>;
        fn all_transaction_hashes(&self) -> Vec<TxHash>;
        fn remove_transactions(
            &self,
            hashes: Vec<TxHash>,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn remove_transactions_and_descendants(
            &self,
            hashes: Vec<TxHash>,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn remove_transactions_by_sender(
            &self,
            sender: Address,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn prune_transactions(
            &self,
            hashes: Vec<TxHash>,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn on_propagated(&self, txs: PropagatedTransactions);
        fn get_transactions_by_sender(
            &self,
            sender: Address,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pending_transactions_by_sender(
            &self,
            sender: Address,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_queued_transactions_by_sender(
            &self,
            sender: Address,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_highest_transaction_by_sender(
            &self,
            sender: Address,
        ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_highest_consecutive_transaction_by_sender(
            &self,
            sender: Address,
            on_chain_nonce: u64,
        ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_transaction_by_sender_and_nonce(
            &self,
            sender: Address,
            nonce: u64,
        ) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_transactions_by_origin(
            &self,
            origin: TransactionOrigin,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn get_pending_transactions_by_origin(
            &self,
            origin: TransactionOrigin,
        ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>>;
        fn unique_senders(&self) -> AddressSet;
        fn get_blob(
            &self,
            tx_hash: TxHash,
        ) -> Result<Option<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;
        fn get_all_blobs(
            &self,
            tx_hashes: Vec<TxHash>,
        ) -> Result<Vec<(TxHash, Arc<BlobTransactionSidecarVariant>)>, BlobStoreError>;
        fn get_all_blobs_exact(
            &self,
            tx_hashes: Vec<TxHash>,
        ) -> Result<Vec<Arc<BlobTransactionSidecarVariant>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v1(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Vec<Option<BlobAndProofV1>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v2(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Option<Vec<BlobAndProofV2>>, BlobStoreError>;
        fn get_blobs_for_versioned_hashes_v3(
            &self,
            versioned_hashes: &[B256],
        ) -> Result<Vec<Option<BlobAndProofV2>>, BlobStoreError>;
    }

    fn retain_unknown<A: HandleMempoolData>(&self, announcement: &mut A) {
        self.inner.retain_unknown(announcement);
    }

    fn get_pending_transactions_with_predicate(
        &self,
        predicate: impl FnMut(&ValidPoolTransaction<Self::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        self.inner
            .get_pending_transactions_with_predicate(predicate)
    }
}
