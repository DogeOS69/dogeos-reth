use crate::DogeosCompatibleNodeTypes;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, BlockHash, map::HashSet};
use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_evm::{ScrollBaseFeeProvider, predict_next_payload_timestamp};
use dogeos_reth_txpool::{
    DogeosL1FeeSnapshot, DogeosPooledTransaction, DogeosTransactionPool, DogeosTransactionValidator,
};
use futures::{
    FutureExt, Stream, StreamExt,
    future::{Fuse, FusedFuture},
};
use reth_chainspec::ChainSpecProvider;
use reth_evm::ConfigureEvm;
use reth_execution_types::ChangedAccount;
use reth_node_builder::{
    BuilderContext, FullNodeTypes,
    components::{PoolBuilder, PoolBuilderConfigOverrides},
};
use reth_primitives_traits::{NodePrimitives, SealedHeader, transaction::TxHashRef};
use reth_provider::{CanonStateNotification, CanonStateNotifications, CanonStateSubscriptions};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory, errors::provider::ProviderError};
use reth_transaction_pool::{
    BlockInfo, CanonicalStateUpdate, CoinbaseTipOrdering, PoolTransaction, PoolUpdateKind,
    TransactionPool, TransactionPoolExt, TransactionValidationTaskExecutor,
    blobstore::DiskFileBlobStore, maintain::MaintainPoolConfig,
};
use std::{
    borrow::Borrow,
    hash::{Hash, Hasher},
};
use tokio::sync::{broadcast::error, oneshot};

#[derive(Clone, Copy)]
struct L1FeeRetryBackoff {
    initial: std::time::Duration,
    max: std::time::Duration,
}

const L1_FEE_RETRY_BACKOFF: L1FeeRetryBackoff = L1FeeRetryBackoff {
    initial: std::time::Duration::from_secs(1),
    max: std::time::Duration::from_secs(30),
};

#[cfg(test)]
const TEST_L1_FEE_RETRY_BACKOFF: L1FeeRetryBackoff = L1FeeRetryBackoff {
    initial: std::time::Duration::from_millis(10),
    max: std::time::Duration::from_millis(40),
};

async fn wait_for_l1_fee_retry(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Maintains L1-fee state using subscribe → drain → reconcile → ready startup ordering.
///
/// Canonical notifications trigger immediate reconciliation. An unavailable cache is retried with
/// bounded exponential backoff so admission can recover even when no new block is published.
async fn run_dogeos_l1_fee_cache_maintenance<N, Refresh>(
    mut notifications: CanonStateNotifications<N>,
    mut refresh: Refresh,
    ready: oneshot::Sender<()>,
    retry_backoff: L1FeeRetryBackoff,
) where
    N: NodePrimitives,
    Refresh: FnMut() -> bool + Send + 'static,
{
    loop {
        match notifications.try_recv() {
            Ok(_) => {
                refresh();
            }
            Err(error::TryRecvError::Lagged(skipped)) => {
                tracing::warn!(
                    target: "reth::txpool",
                    skipped,
                    "DogeOS L1 fee canonical notifications lagged during startup; resynchronizing from latest"
                );
                notifications = notifications.resubscribe();
                refresh();
            }
            Err(error::TryRecvError::Empty) => break,
            Err(error::TryRecvError::Closed) => {
                tracing::warn!(
                    target: "reth::txpool",
                    "DogeOS L1 fee canonical notifications closed before startup reconciliation"
                );
                return;
            }
        }
    }

    // Close the subscribe-then-hydrate construction window before the pool can be returned.
    let mut available = refresh();
    if ready.send(()).is_err() {
        return;
    }

    let mut retry_delay = retry_backoff.initial;
    let mut retry_deadline = (!available).then(|| tokio::time::Instant::now() + retry_delay);

    loop {
        tokio::select! {
            result = notifications.recv() => {
                match result {
                    Ok(_) => {}
                    Err(error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            target: "reth::txpool",
                            skipped,
                            "DogeOS L1 fee canonical notifications lagged; resynchronizing from latest"
                        );
                        // Subscribe first so a head published during the latest-state refresh remains
                        // queued for the next iteration instead of falling into another gap.
                        notifications = notifications.resubscribe();
                    }
                    Err(error::RecvError::Closed) => {
                        tracing::warn!(
                            target: "reth::txpool",
                            "DogeOS L1 fee canonical notifications closed; fee maintenance stopped"
                        );
                        return;
                    }
                }
                available = refresh();
                retry_delay = retry_backoff.initial;
                retry_deadline =
                    (!available).then(|| tokio::time::Instant::now() + retry_delay);
            }
            () = wait_for_l1_fee_retry(retry_deadline) => {
                available = refresh();
                if available {
                    retry_delay = retry_backoff.initial;
                    retry_deadline = None;
                } else {
                    retry_delay = retry_delay.saturating_mul(2).min(retry_backoff.max);
                    retry_deadline = Some(tokio::time::Instant::now() + retry_delay);
                }
            }
        }
    }
}

fn state_backed_pending_base_fee<Client, H>(
    client: &Client,
    parent: &H,
    parent_hash: BlockHash,
) -> Result<u64, String>
where
    Client: StateProviderFactory + ChainSpecProvider<ChainSpec = DogeosChainSpec>,
    H: BlockHeader,
{
    let state = client
        .state_by_block_hash(parent_hash)
        .map_err(|error| error.to_string())?;
    let mut state = StateProviderDatabase::new(state.as_ref());
    ScrollBaseFeeProvider::new(client.chain_spec())
        .next_block_base_fee(
            &mut state,
            parent,
            predict_next_payload_timestamp(parent.timestamp()),
        )
        .map_err(|error| error.to_string())
}

#[derive(Debug, PartialEq, Eq)]
enum MaintainedPoolState {
    InSync,
    Drifted,
}

impl MaintainedPoolState {
    const fn is_drifted(&self) -> bool {
        matches!(self, Self::Drifted)
    }
}

#[derive(Eq)]
struct ChangedAccountEntry(ChangedAccount);

impl PartialEq for ChangedAccountEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.address == other.0.address
    }
}

impl Hash for ChangedAccountEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.address.hash(state);
    }
}

impl Borrow<Address> for ChangedAccountEntry {
    fn borrow(&self) -> &Address {
        &self.0.address
    }
}

#[derive(Default)]
struct LoadedAccounts {
    accounts: Vec<ChangedAccount>,
    failed_to_load: Vec<Address>,
}

fn load_accounts<Client, I>(
    client: Client,
    at: BlockHash,
    addresses: I,
) -> Result<LoadedAccounts, (Vec<Address>, ProviderError)>
where
    I: IntoIterator<Item = Address>,
    Client: StateProviderFactory,
{
    let addresses = addresses.into_iter().collect::<Vec<_>>();
    let state = client
        .history_by_block_hash(at)
        .map_err(|error| (addresses.clone(), error))?;
    let mut loaded = LoadedAccounts::default();
    for address in addresses {
        match state.basic_account(&address) {
            Ok(account) => loaded.accounts.push(
                account
                    .map(|account| ChangedAccount {
                        address,
                        nonce: account.nonce,
                        balance: account.balance,
                    })
                    .unwrap_or_else(|| ChangedAccount::empty(address)),
            ),
            Err(_) => loaded.failed_to_load.push(address),
        }
    }
    Ok(loaded)
}

/// DogeOS variant of Reth's canonical txpool maintenance.
///
/// DogeOS does not support blob transactions, so this keeps the canonical account, mining, reorg,
/// and stale-transaction behavior while deriving every pending base fee from the canonical tip's
/// post-state. The generic Reth task cannot do that because its fee callback only receives a
/// header.
async fn maintain_dogeos_transaction_pool<N, Client, P, St>(
    client: Client,
    pool: P,
    mut events: St,
    config: MaintainPoolConfig,
) where
    N: NodePrimitives,
    Client: StateProviderFactory
        + BlockReaderIdExt<Header = N::BlockHeader>
        + ChainSpecProvider<ChainSpec = DogeosChainSpec>
        + Clone
        + Send
        + Sync
        + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>, Block = N::Block>
        + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let MaintainPoolConfig {
        max_update_depth,
        max_reload_accounts,
        ..
    } = config;

    if let Ok(Some(latest)) = client.header_by_number_or_tag(BlockNumberOrTag::Latest) {
        let latest = SealedHeader::seal_slow(latest);
        let pending_basefee =
            match state_backed_pending_base_fee(&client, latest.header(), latest.hash()) {
                Ok(base_fee) => base_fee,
                Err(error) => {
                    tracing::error!(
                        target: "reth::txpool",
                        %error,
                        "failed to initialize DogeOS state-backed pending base fee"
                    );
                    return;
                }
            };
        pool.set_block_info(BlockInfo {
            block_gas_limit: latest.gas_limit(),
            last_seen_block_hash: latest.hash(),
            last_seen_block_number: latest.number(),
            pending_basefee,
            pending_blob_fee: None,
        });
    }

    let mut dirty_addresses = HashSet::default();
    let mut maintained_state = MaintainedPoolState::InSync;
    let mut reload_accounts_fut = Fuse::terminated();
    let mut stale_eviction_interval = tokio::time::interval(config.max_tx_lifetime);
    let mut first_event = true;

    loop {
        let pool_info = pool.block_info();
        if maintained_state.is_drifted() {
            dirty_addresses = pool.unique_senders();
            maintained_state = MaintainedPoolState::InSync;
        }

        if !dirty_addresses.is_empty() && reload_accounts_fut.is_terminated() {
            let (tx, rx) = oneshot::channel();
            let client = client.clone();
            let at = pool_info.last_seen_block_hash;
            let accounts = dirty_addresses
                .iter()
                .copied()
                .take(max_reload_accounts)
                .collect::<Vec<_>>();
            for address in &accounts {
                dirty_addresses.remove(address);
            }
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(load_accounts(client, at, accounts));
            });
            reload_accounts_fut = rx.fuse();
        }

        let mut event = None;
        let mut reloaded = None;
        tokio::select! {
            result = &mut reload_accounts_fut => reloaded = Some(result),
            next = events.next() => {
                let Some(next) = next else { break };
                event = Some(next);
                if first_event {
                    maintained_state = MaintainedPoolState::Drifted;
                    first_event = false;
                }
            }
            _ = stale_eviction_interval.tick() => {
                let now = std::time::Instant::now();
                let stale = pool
                    .queued_transactions()
                    .into_iter()
                    .filter(|tx| {
                        (tx.origin.is_external() || config.no_local_exemptions) &&
                            now.duration_since(tx.timestamp) > config.max_tx_lifetime
                    })
                    .map(|tx| *tx.hash())
                    .collect();
                pool.remove_transactions(stale);
            }
        }

        match reloaded {
            Some(Ok(Ok(LoadedAccounts {
                accounts,
                failed_to_load,
            }))) => {
                dirty_addresses.extend(failed_to_load);
                pool.update_accounts(accounts);
            }
            Some(Ok(Err((accounts, error)))) => {
                tracing::debug!(target: "reth::txpool", %error, "failed to reload accounts");
                dirty_addresses.extend(accounts);
            }
            Some(Err(_)) => maintained_state = MaintainedPoolState::Drifted,
            None => {}
        }

        let Some(event) = event else { continue };
        match event {
            CanonStateNotification::Reorg { old, new } => {
                let (old_blocks, old_state) = old.inner();
                let (new_blocks, new_state) = new.inner();
                let new_tip = new_blocks.tip();
                let new_first = new_blocks.first();
                let old_first = old_blocks.first();

                if !(old_first.parent_hash() == pool_info.last_seen_block_hash
                    || new_first.parent_hash() == pool_info.last_seen_block_hash)
                {
                    maintained_state = MaintainedPoolState::Drifted;
                }

                let pending_block_base_fee = match state_backed_pending_base_fee(
                    &client,
                    new_tip.header(),
                    new_tip.hash(),
                ) {
                    Ok(base_fee) => base_fee,
                    Err(error) => {
                        tracing::error!(target: "reth::txpool", %error, "failed to derive pending base fee after reorg");
                        return;
                    }
                };

                let new_changed_accounts: HashSet<_> = new_state
                    .changed_accounts()
                    .map(ChangedAccountEntry)
                    .collect();
                let missing_changed_accounts = old_state
                    .accounts_iter()
                    .map(|(address, _)| address)
                    .filter(|address| !new_changed_accounts.contains(address));
                let mut changed_accounts = match load_accounts(
                    client.clone(),
                    new_tip.hash(),
                    missing_changed_accounts,
                ) {
                    Ok(LoadedAccounts {
                        accounts,
                        failed_to_load,
                    }) => {
                        dirty_addresses.extend(failed_to_load);
                        accounts
                    }
                    Err((addresses, error)) => {
                        tracing::debug!(target: "reth::txpool", %error, "failed to load reorged accounts");
                        dirty_addresses.extend(addresses);
                        Vec::new()
                    }
                };
                changed_accounts.extend(new_changed_accounts.into_iter().map(|entry| entry.0));

                let new_mined_transactions: HashSet<_> = new_blocks.transaction_hashes().collect();
                let pruned_old_transactions = old_blocks
                    .transactions_ecrecovered()
                    .filter(|tx| !new_mined_transactions.contains(tx.tx_hash()))
                    .filter_map(|tx| {
                        <P as TransactionPool>::Transaction::try_from_consensus(tx).ok()
                    })
                    .collect::<Vec<_>>();

                pool.on_canonical_state_change(CanonicalStateUpdate {
                    new_tip: new_tip.sealed_block(),
                    pending_block_base_fee,
                    pending_block_blob_fee: None,
                    changed_accounts,
                    mined_transactions: new_blocks.transaction_hashes().collect(),
                    update_kind: PoolUpdateKind::Reorg,
                });
                let _ = pool
                    .add_external_transactions(pruned_old_transactions)
                    .await;
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();
                let pending_block_base_fee = match state_backed_pending_base_fee(
                    &client,
                    tip.header(),
                    tip.hash(),
                ) {
                    Ok(base_fee) => base_fee,
                    Err(error) => {
                        tracing::error!(target: "reth::txpool", %error, "failed to derive pending base fee after commit");
                        return;
                    }
                };
                let first_block = blocks.first();
                let depth = tip.number().abs_diff(pool_info.last_seen_block_number);
                if depth > max_update_depth {
                    maintained_state = MaintainedPoolState::Drifted;
                    pool.set_block_info(BlockInfo {
                        block_gas_limit: tip.gas_limit(),
                        last_seen_block_hash: tip.hash(),
                        last_seen_block_number: tip.number(),
                        pending_basefee: pending_block_base_fee,
                        pending_blob_fee: None,
                    });
                    continue;
                }

                let mut changed_accounts = Vec::with_capacity(state.state().len());
                for account in state.changed_accounts() {
                    dirty_addresses.remove(&account.address);
                    changed_accounts.push(account);
                }
                if first_block.parent_hash() != pool_info.last_seen_block_hash {
                    maintained_state = MaintainedPoolState::Drifted;
                }
                pool.on_canonical_state_change(CanonicalStateUpdate {
                    new_tip: tip.sealed_block(),
                    pending_block_base_fee,
                    pending_block_blob_fee: None,
                    changed_accounts,
                    mined_transactions: blocks.transaction_hashes().collect(),
                    update_kind: PoolUpdateKind::Commit,
                });
            }
        }
    }
}

/// Builds the DogeOS transaction pool and its canonical-chain maintenance tasks.
#[derive(Debug, Clone, Default)]
pub struct DogeosPoolBuilder {
    pub pool_config_overrides: PoolBuilderConfigOverrides,
}

impl DogeosPoolBuilder {
    pub fn with_pool_config_overrides(mut self, overrides: PoolBuilderConfigOverrides) -> Self {
        self.pool_config_overrides = overrides;
        self
    }
}

impl<Node, Evm> PoolBuilder<Node, Evm> for DogeosPoolBuilder
where
    Node: FullNodeTypes<Types: DogeosCompatibleNodeTypes>,
    Evm: ConfigureEvm<Primitives = dogeos_reth_primitives::DogeosPrimitives> + Clone + 'static,
{
    type Pool =
        DogeosTransactionPool<Node::Provider, DiskFileBlobStore, DogeosPooledTransaction, Evm>;

    async fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        evm_config: Evm,
    ) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.config().datadir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;
        let pool_config = self.pool_config_overrides.clone().apply(ctx.pool_config());
        let require_l1_data_gas_fee = !ctx.config().dev.dev;
        let require_l1_data_fee_buffer = ctx.chain_spec().config.l1_data_fee_buffer_check;
        let additional_validation_tasks = self
            .pool_config_overrides
            .additional_validation_tasks
            .unwrap_or(ctx.config().txpool.additional_validation_tasks);
        if !require_l1_data_gas_fee {
            tracing::info!(
                target: "reth::txpool",
                "DogeOS L1 fee enforcement disabled in development mode"
            );
        }
        let l1_fee_notifications =
            require_l1_data_gas_fee.then(|| ctx.provider().subscribe_to_canonical_state());
        let canonical_state_stream = ctx.provider().canonical_state_stream();

        let l1_fee_snapshot = require_l1_data_gas_fee
            .then(|| DogeosL1FeeSnapshot::load_latest(ctx.provider()))
            .transpose()?;

        let inner =
            TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone(), evm_config)
                .no_eip4844()
                .kzg_settings(ctx.kzg_settings()?)
                .with_local_transactions_config(pool_config.local_transactions_config.clone())
                .with_max_tx_input_bytes(ctx.chain_spec().config.max_tx_payload_bytes_per_block)
                .build(blob_store.clone());
        let validator = match l1_fee_snapshot {
            Some(snapshot) => {
                DogeosTransactionValidator::new(inner, snapshot, require_l1_data_fee_buffer)
            }
            None => DogeosTransactionValidator::disabled(inner, require_l1_data_fee_buffer),
        };
        let validator = TransactionValidationTaskExecutor::spawn(
            validator,
            ctx.task_executor(),
            additional_validation_tasks,
        );

        if let Some(notifications) = l1_fee_notifications {
            let synchronizer = validator.clone();
            let (ready_tx, ready_rx) = oneshot::channel();
            ctx.task_executor().spawn_critical_blocking_task(
                "DogeOS L1 fee canonical state task",
                run_dogeos_l1_fee_cache_maintenance(
                    notifications,
                    move || synchronizer.validator().refresh_l1_fee_cache_from_latest(),
                    ready_tx,
                    L1_FEE_RETRY_BACKOFF,
                ),
            );
            ready_rx.await.map_err(|_| {
                eyre::eyre!(
                    "DogeOS L1 fee canonical state task stopped before startup reconciliation"
                )
            })?;
        }

        let pool = reth_transaction_pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            pool_config,
        );
        let transactions_path = data_dir.txpool_transactions();
        let backup_config =
            reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(
                transactions_path,
            );
        ctx.task_executor()
            .spawn_critical_with_graceful_shutdown_signal("local transactions backup task", {
                let pool = pool.clone();
                move |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool,
                        backup_config,
                    )
                }
            });

        ctx.task_executor().spawn_critical_task(
            "txpool maintenance task",
            maintain_dogeos_transaction_pool(
                ctx.provider().clone(),
                pool.clone(),
                canonical_state_stream,
                reth_transaction_pool::maintain::MaintainPoolConfig {
                    max_tx_lifetime: pool.config().max_queued_lifetime,
                    ..Default::default()
                },
            ),
        );
        tracing::info!(target: "reth::cli", "DogeOS transaction pool initialized");
        Ok(pool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DogeosNodeTypes;
    use alloy_consensus::{Block, Signed, TxLegacy, transaction::Recovered};
    use alloy_eips::Encodable2718;
    use alloy_genesis::GenesisAccount;
    use alloy_primitives::{
        Address, B256, Bytes, Signature, TxKind, U256, keccak256, map::HashMap,
    };
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_DEV, DogeosChainSpec, DogeosChainSpecBuilder};
    use dogeos_hardforks::{DogeosHardfork, ForkCondition};
    use dogeos_reth_evm::{NEXT_CONTROLLED_BASE_FEE_SLOT, ScrollEvmConfig};
    use dogeos_reth_primitives::{DogeosBlock, DogeosPrimitives, ScrollTransactionSigned};
    use dogeos_reth_txpool::{DogeosL1FeeError, DogeosPooledTransaction};
    use reth_chainspec::{ChainSpecProvider, EthChainSpec};
    use reth_db_common::init::init_genesis_with_settings;
    use reth_primitives_traits::{
        Block as _, RecoveredBlock, transaction::error::InvalidTransactionError,
    };
    use reth_provider::{
        BlockWriter, CanonStateNotification, Chain, ExecutionOutcome,
        providers::BlockchainProvider,
        test_utils::{
            ExtendedAccount, MockEthProvider, create_test_provider_factory_with_node_types,
        },
    };
    use reth_storage_api::{
        AccountReader, BlockReaderIdExt, StateProvider, StateProviderFactory, StorageSettings,
    };
    use reth_transaction_pool::{
        TransactionOrigin, TransactionPool, blobstore::InMemoryBlobStore,
        error::InvalidPoolTransactionError, maintain::MaintainPoolConfig,
        validate::EthTransactionValidatorBuilder,
    };
    use reth_trie_common::KeccakKeyHasher;
    use std::{collections::BTreeMap, sync::Arc, time::Duration};

    type NativeProvider = BlockchainProvider<
        reth_node_types::NodeTypesWithDBAdapter<
            DogeosNodeTypes,
            Arc<reth_db::test_utils::TempDatabase<reth_db::DatabaseEnv>>,
        >,
    >;
    type MockProvider = MockEthProvider<DogeosPrimitives, DogeosChainSpec>;
    type MockValidator =
        DogeosTransactionValidator<MockProvider, DogeosPooledTransaction, ScrollEvmConfig>;

    #[test]
    fn pending_pool_fee_reads_tsuki_controller_from_tip_state() {
        let provider =
            MockEthProvider::<DogeosPrimitives>::new().with_chain_spec(DOGEOS_DEV.as_ref().clone());
        let system_config = DOGEOS_DEV.config.l1_config.l2_system_config_address;
        provider.add_account(
            system_config,
            ExtendedAccount::new(1, U256::ZERO).extend_storage([
                (B256::from(U256::from(101)), U256::from(100_000_000u64)),
                (
                    B256::from(NEXT_CONTROLLED_BASE_FEE_SLOT),
                    U256::from(600_000_000_000u64),
                ),
            ]),
        );
        let header = alloy_consensus::Header {
            timestamp: 1,
            base_fee_per_gas: Some(7),
            ..Default::default()
        };

        assert_eq!(
            state_backed_pending_base_fee(&provider, &header, B256::ZERO).unwrap(),
            600_100_000_000
        );
    }

    fn oracle_account(chain_spec: &DogeosChainSpec, l1_base_fee: U256) -> ExtendedAccount {
        use revm_scroll::l1block::{
            L1_BASE_FEE_SLOT, L1_COMMIT_SCALAR_SLOT, L1_GAS_PRICE_ORACLE_ADDRESS,
        };

        let genesis_account = chain_spec
            .genesis()
            .alloc
            .get(&L1_GAS_PRICE_ORACLE_ADDRESS)
            .unwrap();
        let mut storage = genesis_account
            .storage
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect::<BTreeMap<_, _>>();
        storage.insert(B256::from(L1_BASE_FEE_SLOT), l1_base_fee);
        storage.insert(
            B256::from(L1_COMMIT_SCALAR_SLOT),
            U256::from(1_000_000_000_u64),
        );

        let mut account = ExtendedAccount::new(
            genesis_account.nonce.unwrap_or_default(),
            genesis_account.balance,
        )
        .extend_storage(storage);
        if let Some(code) = genesis_account.code.clone() {
            account = account.with_bytecode(code);
        }
        account
    }

    fn mock_provider(chain_spec: Arc<DogeosChainSpec>, sender: Address) -> MockProvider {
        use revm_scroll::l1block::L1_GAS_PRICE_ORACLE_ADDRESS;

        let provider = MockEthProvider::<DogeosPrimitives>::new()
            .with_chain_spec(chain_spec.as_ref().clone())
            .with_genesis_block();
        provider.add_account(sender, ExtendedAccount::new(0, U256::MAX));
        provider.add_account(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            oracle_account(&chain_spec, U256::ZERO),
        );
        provider
    }

    fn mock_validator(provider: MockProvider) -> MockValidator {
        let snapshot = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let chain_spec = provider.chain_spec();
        let inner =
            EthTransactionValidatorBuilder::new(provider, ScrollEvmConfig::dogeos(chain_spec))
                .no_eip4844()
                .build(InMemoryBlobStore::default());
        DogeosTransactionValidator::new(inner, snapshot, false)
    }

    fn add_mock_head(
        provider: &MockProvider,
        chain_spec: &DogeosChainSpec,
        parent_hash: B256,
        number: u64,
        timestamp: u64,
        l1_base_fee: U256,
    ) -> CanonStateNotification<DogeosPrimitives> {
        use revm_scroll::l1block::L1_GAS_PRICE_ORACLE_ADDRESS;

        provider.add_account(
            L1_GAS_PRICE_ORACLE_ADDRESS,
            oracle_account(chain_spec, l1_base_fee),
        );
        let block: DogeosBlock = Block {
            header: alloy_consensus::Header {
                parent_hash,
                number,
                timestamp,
                gas_limit: 30_000_000,
                ..Default::default()
            },
            body: Default::default(),
        };
        let block = block.seal_slow();
        provider.add_block(block.hash(), block.clone().unseal());
        let recovered = RecoveredBlock::new_unhashed(block.unseal(), Vec::new());
        CanonStateNotification::Commit {
            new: Arc::new(Chain::new(
                [recovered],
                ExecutionOutcome::default(),
                BTreeMap::new(),
            )),
        }
    }

    fn rollup_fee_overflows(validator: &MockValidator, sender: Address, chain_id: u64) -> bool {
        matches!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_id, 0x55),
                )
                .as_invalid(),
            Some(InvalidPoolTransactionError::Consensus(
                InvalidTransactionError::GasUintOverflow
            ))
        )
    }

    fn transaction(sender: Address, chain_id: u64, hash_byte: u8) -> DogeosPooledTransaction {
        let signed: ScrollTransactionSigned = Signed::new_unchecked(
            TxLegacy {
                chain_id: Some(chain_id),
                gas_price: 1,
                gas_limit: 21_000,
                to: TxKind::Call(Address::ZERO),
                ..Default::default()
            },
            Signature::test_signature(),
            B256::repeat_byte(hash_byte),
        )
        .into();
        let encoded_length = signed.encode_2718_len();
        DogeosPooledTransaction::new(Recovered::new_unchecked(signed, sender), encoded_length)
    }

    fn initialize_native_v2(chain_spec: Arc<DogeosChainSpec>) -> NativeProvider {
        let factory = create_test_provider_factory_with_node_types::<DogeosNodeTypes>(Arc::clone(
            &chain_spec,
        ));
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();
        BlockchainProvider::new(factory).unwrap()
    }

    fn assert_fresh_native_v2_admission(chain_spec: Arc<DogeosChainSpec>, sender: Address) {
        let provider = initialize_native_v2(Arc::clone(&chain_spec));
        let snapshot = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        assert_eq!(snapshot.head_hash(), chain_spec.genesis_hash());
        assert_eq!(snapshot.number(), 0);
        assert_eq!(snapshot.timestamp(), chain_spec.genesis().timestamp);

        let inner = EthTransactionValidatorBuilder::new(
            provider,
            ScrollEvmConfig::dogeos(Arc::clone(&chain_spec)),
        )
        .no_eip4844()
        .build(InMemoryBlobStore::default());
        let validator = DogeosTransactionValidator::new(inner, snapshot, false);

        assert!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x11),
                )
                .is_valid()
        );
    }

    #[tokio::test]
    async fn buffered_startup_head_is_reconciled_before_fee_cache_readiness() {
        let chain_spec = DOGEOS_CHIKYU.clone();
        let sender = Address::repeat_byte(0x41);
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        assert!(!rollup_fee_overflows(
            &validator,
            sender,
            chain_spec.chain().id()
        ));
        let (executor, _validation_task) = TransactionValidationTaskExecutor::new(validator);
        let (notifications_tx, notifications_rx) = tokio::sync::broadcast::channel(16);

        let genesis = provider.latest_header().unwrap().unwrap();
        let notification = add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            1,
            genesis.timestamp + 1,
            U256::from(u64::MAX),
        );
        notifications_tx.send(notification).unwrap();

        let synchronizer = executor.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            move || synchronizer.validator().refresh_l1_fee_cache_from_latest(),
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));
        tokio::time::timeout(Duration::from_secs(1), ready_rx)
            .await
            .unwrap()
            .unwrap();

        assert!(rollup_fee_overflows(
            executor.validator(),
            sender,
            chain_spec.chain().id()
        ));
        cache_task.abort();
    }

    #[tokio::test]
    async fn unavailable_fee_cache_recovers_on_retry_without_a_new_head() {
        let chain_spec = DOGEOS_CHIKYU.clone();
        let sender = Address::repeat_byte(0x44);
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        let blocks = std::mem::take(&mut *provider.blocks.lock());
        let headers = std::mem::take(&mut *provider.headers.lock());
        let (executor, _validation_task) = TransactionValidationTaskExecutor::new(validator);
        let (_notifications_tx, notifications_rx) =
            tokio::sync::broadcast::channel::<CanonStateNotification<DogeosPrimitives>>(16);
        let synchronizer = executor.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            move || synchronizer.validator().refresh_l1_fee_cache_from_latest(),
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));
        ready_rx.await.unwrap();

        let unavailable = executor.validator().validate_one(
            TransactionOrigin::External,
            transaction(sender, chain_spec.chain().id(), 0x56),
        );
        let reth_transaction_pool::TransactionValidationOutcome::Error(_, error) = unavailable
        else {
            panic!("expected transient latest-header failure to block admission")
        };
        assert!(matches!(
            error.downcast_ref::<DogeosL1FeeError>(),
            Some(DogeosL1FeeError::CacheUnavailable { source })
                if matches!(source.as_ref(), DogeosL1FeeError::LatestHeaderRead { .. })
        ));

        *provider.blocks.lock() = blocks;
        *provider.headers.lock() = headers;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !executor
                .validator()
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x57),
                )
                .is_valid()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cache_task.abort();
    }

    #[tokio::test]
    async fn closed_startup_channel_fails_readiness() {
        let (notifications_tx, notifications_rx) =
            tokio::sync::broadcast::channel::<CanonStateNotification<DogeosPrimitives>>(1);
        drop(notifications_tx);
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            || true,
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));

        assert!(
            tokio::time::timeout(Duration::from_secs(1), ready_rx)
                .await
                .unwrap()
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(1), cache_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn lagged_fee_notifications_resynchronize_from_latest_and_discard_backlog() {
        let chain_spec = DOGEOS_CHIKYU.clone();
        let sender = Address::repeat_byte(0x42);
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        let (executor, _validation_task) = TransactionValidationTaskExecutor::new(validator);
        let (notifications_tx, notifications_rx) = tokio::sync::broadcast::channel(1);
        let synchronizer = executor.clone();
        let provider_for_refresh = provider.clone();
        let chain_spec_for_refresh = Arc::clone(&chain_spec);
        let notifications_during_refresh = notifications_tx.clone();
        let mut refresh_count = 0;
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            move || {
                let available = synchronizer.validator().refresh_l1_fee_cache_from_latest();
                refresh_count += 1;
                if refresh_count == 2 {
                    let head = provider_for_refresh.latest_header().unwrap().unwrap();
                    let notification = add_mock_head(
                        &provider_for_refresh,
                        &chain_spec_for_refresh,
                        head.hash(),
                        2,
                        head.timestamp + 1,
                        U256::from(u64::MAX),
                    );
                    notifications_during_refresh.send(notification).unwrap();
                }
                available
            },
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));
        ready_rx.await.unwrap();

        let genesis = provider.latest_header().unwrap().unwrap();
        let notification = add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            1,
            genesis.timestamp + 1,
            U256::from(1),
        );
        for _ in 0..3 {
            notifications_tx.send(notification.clone()).unwrap();
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            while !rollup_fee_overflows(executor.validator(), sender, chain_spec.chain().id()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cache_task.abort();
    }

    #[tokio::test]
    async fn same_height_sibling_reorg_refreshes_fee_state() {
        let chain_spec = DOGEOS_CHIKYU.clone();
        let sender = Address::repeat_byte(0x45);
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        let (executor, _validation_task) = TransactionValidationTaskExecutor::new(validator);
        let (notifications_tx, notifications_rx) = tokio::sync::broadcast::channel(16);
        let synchronizer = executor.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            move || synchronizer.validator().refresh_l1_fee_cache_from_latest(),
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));
        ready_rx.await.unwrap();

        let genesis = provider.latest_header().unwrap().unwrap();
        let old_notification = add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            1,
            genesis.timestamp + 1,
            U256::from(u64::MAX),
        );
        let CanonStateNotification::Commit { new: old } = old_notification else {
            unreachable!("helper always creates a commit")
        };
        let old_hash = old.tip().hash();
        notifications_tx
            .send(CanonStateNotification::Commit {
                new: Arc::clone(&old),
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !rollup_fee_overflows(executor.validator(), sender, chain_spec.chain().id()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        provider.headers.lock().remove(&old_hash);
        provider.blocks.lock().remove(&old_hash);
        let new_notification = add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            1,
            genesis.timestamp + 2,
            U256::ONE,
        );
        let CanonStateNotification::Commit { new } = new_notification else {
            unreachable!("helper always creates a commit")
        };
        assert_ne!(old.tip().hash(), new.tip().hash());
        notifications_tx
            .send(CanonStateNotification::Reorg { old, new })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while !executor
                .validator()
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x58),
                )
                .is_valid()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cache_task.abort();
    }

    #[tokio::test]
    async fn deep_pool_commit_still_refreshes_fee_cache() {
        use futures::channel::mpsc;
        use reth_tasks::Runtime;

        let chain_spec = DOGEOS_CHIKYU.clone();
        let sender = Address::repeat_byte(0x43);
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        let (executor, _validation_task) = TransactionValidationTaskExecutor::new(validator);
        let (notifications_tx, notifications_rx) = tokio::sync::broadcast::channel(16);
        let synchronizer = executor.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let cache_task = tokio::spawn(run_dogeos_l1_fee_cache_maintenance(
            notifications_rx,
            move || synchronizer.validator().refresh_l1_fee_cache_from_latest(),
            ready_tx,
            TEST_L1_FEE_RETRY_BACKOFF,
        ));
        ready_rx.await.unwrap();

        let pool = reth_transaction_pool::Pool::new(
            executor.clone(),
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            Default::default(),
        );
        let (maintenance_tx, maintenance_rx) = mpsc::unbounded();
        let maintenance_task = tokio::spawn(
            reth_transaction_pool::maintain::maintain_transaction_pool_future(
                provider.clone(),
                pool.clone(),
                maintenance_rx,
                Runtime::test(),
                MaintainPoolConfig {
                    max_update_depth: 1,
                    ..Default::default()
                },
            ),
        );
        let genesis = provider.latest_header().unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while pool.block_info().last_seen_block_hash != genesis.hash() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let notification = add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            65,
            genesis.timestamp + 65,
            U256::from(u64::MAX),
        );
        notifications_tx.send(notification.clone()).unwrap();
        maintenance_tx.unbounded_send(notification).unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while pool.block_info().last_seen_block_number != 65
                || !rollup_fee_overflows(executor.validator(), sender, chain_spec.chain().id())
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        maintenance_task.abort();
        cache_task.abort();
    }

    #[test]
    fn fee_cap_uses_the_hydrated_snapshot_timestamp_across_tsuki() {
        let sender = Address::repeat_byte(0x46);
        let activation = DOGEOS_CHIKYU.genesis().timestamp + 2;
        let chain_spec = Arc::new(
            DogeosChainSpecBuilder::dogeos_chikyu()
                .with_fork(DogeosHardfork::Tsuki, ForkCondition::Timestamp(activation))
                .build(DOGEOS_CHIKYU.config),
        );
        let provider = mock_provider(Arc::clone(&chain_spec), sender);
        let validator = mock_validator(provider.clone());
        let genesis = provider.latest_header().unwrap().unwrap();

        add_mock_head(
            &provider,
            &chain_spec,
            genesis.hash(),
            1,
            activation - 1,
            U256::from(u64::MAX),
        );
        assert!(validator.refresh_l1_fee_cache_from_latest());
        assert!(matches!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x31),
                )
                .as_invalid(),
            Some(InvalidPoolTransactionError::Consensus(
                InvalidTransactionError::GasUintOverflow
            ))
        ));
        let pre_tsuki_head = provider.latest_header().unwrap().unwrap();

        add_mock_head(
            &provider,
            &chain_spec,
            pre_tsuki_head.hash(),
            2,
            activation,
            U256::from(u64::MAX),
        );
        assert!(validator.refresh_l1_fee_cache_from_latest());
        assert!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x59),
                )
                .is_valid()
        );
    }

    #[test]
    fn fresh_feynman_native_v2_hydrates_before_first_validation() {
        let sender = Address::repeat_byte(0x11);
        let mut genesis = DOGEOS_CHIKYU.genesis().clone();
        genesis.alloc.insert(
            sender,
            GenesisAccount {
                balance: U256::MAX,
                ..Default::default()
            },
        );
        let chain_spec = Arc::new(
            DogeosChainSpecBuilder::dogeos_chikyu()
                .genesis(genesis)
                .build(DOGEOS_CHIKYU.config),
        );

        assert_fresh_native_v2_admission(chain_spec, sender);
    }

    #[test]
    fn fresh_tsuki_native_v2_hydrates_before_first_validation() {
        let sender = Address::repeat_byte(0x12);
        let mut genesis = DOGEOS_DEV.genesis().clone();
        genesis.alloc.insert(
            sender,
            GenesisAccount {
                balance: U256::MAX,
                ..Default::default()
            },
        );
        let chain_spec = Arc::new(
            DogeosChainSpecBuilder::dev()
                .genesis(genesis)
                .build(DOGEOS_DEV.config),
        );

        assert_fresh_native_v2_admission(chain_spec, sender);
    }

    #[test]
    fn native_v2_empty_code_sender_is_admitted_and_nonempty_code_is_rejected() {
        let empty_code_sender = Address::repeat_byte(0x21);
        let nonempty_code_sender = Address::repeat_byte(0x22);
        let nonempty_code = Bytes::from_static(&[0x00]);
        let mut genesis = DOGEOS_CHIKYU.genesis().clone();
        genesis.alloc.extend([
            (
                empty_code_sender,
                GenesisAccount {
                    balance: U256::MAX,
                    code: Some(Bytes::new()),
                    ..Default::default()
                },
            ),
            (
                nonempty_code_sender,
                GenesisAccount {
                    balance: U256::MAX,
                    code: Some(nonempty_code.clone()),
                    ..Default::default()
                },
            ),
        ]);
        let chain_spec = Arc::new(
            DogeosChainSpecBuilder::dogeos_chikyu()
                .genesis(genesis)
                .build(DOGEOS_CHIKYU.config),
        );
        let provider = initialize_native_v2(Arc::clone(&chain_spec));
        let state = provider.latest().unwrap();
        assert_eq!(
            state
                .basic_account(&empty_code_sender)
                .unwrap()
                .unwrap()
                .bytecode_hash,
            Some(alloy_consensus::constants::KECCAK_EMPTY)
        );
        assert_eq!(
            state
                .basic_account(&nonempty_code_sender)
                .unwrap()
                .unwrap()
                .bytecode_hash,
            Some(keccak256(&nonempty_code))
        );
        drop(state);

        let snapshot = DogeosL1FeeSnapshot::load_latest(&provider).unwrap();
        let inner = EthTransactionValidatorBuilder::new(
            provider,
            ScrollEvmConfig::dogeos(Arc::clone(&chain_spec)),
        )
        .no_eip4844()
        .build(InMemoryBlobStore::default());
        let validator = DogeosTransactionValidator::new(inner, snapshot, false);

        assert!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(empty_code_sender, chain_spec.chain().id(), 0x21),
                )
                .is_valid()
        );
        assert!(matches!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(nonempty_code_sender, chain_spec.chain().id(), 0x22),
                )
                .as_invalid(),
            Some(InvalidPoolTransactionError::Consensus(
                InvalidTransactionError::SignerAccountHasBytecode
            ))
        ));
    }

    #[test]
    fn restart_hydrates_persisted_native_v2_head_before_next_callback() {
        use revm::{database::BundleState, state::AccountInfo};
        use revm_scroll::l1block::{
            L1_BASE_FEE_SLOT, L1_COMMIT_SCALAR_SLOT, L1_GAS_PRICE_ORACLE_ADDRESS,
        };

        let sender = Address::repeat_byte(0x31);
        let mut genesis = DOGEOS_CHIKYU.genesis().clone();
        genesis.alloc.insert(
            sender,
            GenesisAccount {
                balance: U256::MAX,
                ..Default::default()
            },
        );
        let chain_spec = Arc::new(
            DogeosChainSpecBuilder::dogeos_chikyu()
                .genesis(genesis)
                .build(DOGEOS_CHIKYU.config),
        );
        let factory = create_test_provider_factory_with_node_types::<DogeosNodeTypes>(Arc::clone(
            &chain_spec,
        ));
        init_genesis_with_settings(&factory, StorageSettings::v2()).unwrap();
        let first_provider = BlockchainProvider::new(factory.clone()).unwrap();
        let genesis_head = first_provider.latest_header().unwrap().unwrap();
        let genesis_snapshot = DogeosL1FeeSnapshot::load_latest(&first_provider).unwrap();
        let genesis_inner = EthTransactionValidatorBuilder::new(
            first_provider.clone(),
            ScrollEvmConfig::dogeos(Arc::clone(&chain_spec)),
        )
        .no_eip4844()
        .build(InMemoryBlobStore::default());
        let genesis_validator =
            DogeosTransactionValidator::new(genesis_inner, genesis_snapshot, false);
        assert!(
            genesis_validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x30),
                )
                .is_valid()
        );
        let state = first_provider.latest().unwrap();
        let oracle = state
            .basic_account(&L1_GAS_PRICE_ORACLE_ADDRESS)
            .unwrap()
            .unwrap();
        let slot_key = B256::from(L1_BASE_FEE_SLOT);
        let old_fee = state
            .storage(L1_GAS_PRICE_ORACLE_ADDRESS, slot_key)
            .unwrap()
            .unwrap_or_default();
        let old_commit_scalar = state
            .storage(
                L1_GAS_PRICE_ORACLE_ADDRESS,
                B256::from(L1_COMMIT_SCALAR_SLOT),
            )
            .unwrap()
            .unwrap_or_default();
        drop(state);

        let oracle_info = AccountInfo {
            balance: oracle.balance,
            nonce: oracle.nonce,
            code_hash: oracle.bytecode_hash.unwrap_or_default(),
            code: None,
            ..Default::default()
        };
        let new_fee = U256::MAX;
        let new_commit_scalar = U256::from(1_000_000_000_u64);
        let bundle = BundleState::builder(1..=1)
            .state_present_account_info(L1_GAS_PRICE_ORACLE_ADDRESS, oracle_info.clone())
            .state_storage(
                L1_GAS_PRICE_ORACLE_ADDRESS,
                HashMap::from_iter([
                    (L1_BASE_FEE_SLOT, (old_fee, new_fee)),
                    (
                        L1_COMMIT_SCALAR_SLOT,
                        (old_commit_scalar, new_commit_scalar),
                    ),
                ]),
            )
            .revert_account_info(1, L1_GAS_PRICE_ORACLE_ADDRESS, Some(Some(oracle_info)))
            .revert_storage(
                1,
                L1_GAS_PRICE_ORACLE_ADDRESS,
                vec![
                    (L1_BASE_FEE_SLOT, old_fee),
                    (L1_COMMIT_SCALAR_SLOT, old_commit_scalar),
                ],
            )
            .build();
        let outcome = ExecutionOutcome::new(bundle, vec![vec![]], 1, Vec::new());
        let hashed_state = outcome.hash_state_slow::<KeccakKeyHasher>().into_sorted();
        let block = RecoveredBlock::new_unhashed(
            Block {
                header: alloy_consensus::Header {
                    parent_hash: genesis_head.hash(),
                    number: 1,
                    timestamp: genesis_head.timestamp + 1,
                    gas_limit: 30_000_000,
                    ..Default::default()
                },
                body: Default::default(),
            },
            Vec::new(),
        );

        let writer = factory.provider_rw().unwrap();
        writer
            .append_blocks_with_state(vec![block], &outcome, hashed_state)
            .unwrap();
        writer.commit().unwrap();
        drop(first_provider);

        let restarted = BlockchainProvider::new(factory).unwrap();
        let snapshot = DogeosL1FeeSnapshot::load_latest(&restarted).unwrap();
        assert_eq!(snapshot.number(), 1);
        assert_eq!(snapshot.timestamp(), genesis_head.timestamp + 1);
        assert_ne!(snapshot.head_hash(), genesis_head.hash());
        let restarted_state = restarted.latest().unwrap();
        assert_eq!(
            restarted_state
                .storage(L1_GAS_PRICE_ORACLE_ADDRESS, slot_key)
                .unwrap(),
            Some(new_fee)
        );
        drop(restarted_state);

        let inner = EthTransactionValidatorBuilder::new(
            restarted,
            ScrollEvmConfig::dogeos(Arc::clone(&chain_spec)),
        )
        .no_eip4844()
        .build(InMemoryBlobStore::default());
        let validator = DogeosTransactionValidator::new(inner, snapshot, false);
        assert!(matches!(
            validator
                .validate_one(
                    TransactionOrigin::External,
                    transaction(sender, chain_spec.chain().id(), 0x31),
                )
                .as_invalid(),
            Some(InvalidPoolTransactionError::Consensus(
                InvalidTransactionError::GasUintOverflow
            ))
        ));
    }
}
