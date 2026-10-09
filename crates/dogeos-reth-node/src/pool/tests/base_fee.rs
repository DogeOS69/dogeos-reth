//! Audit 10008: run upstream maintenance over the DogeOS adapter and a real state provider.
use super::*;
use alloy_consensus::BlockHeader;
use dogeos_reth_evm::{MAX_L2_BASE_FEE, ScrollBaseFeeProvider};
use futures::channel::mpsc;
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_primitives_traits::SealedBlock;
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::HeaderProvider;
use reth_tasks::Runtime;
use reth_transaction_pool::{
    BlockInfo, CanonicalStateUpdate, Pool, PoolTransaction, PoolUpdateKind, TransactionPoolExt,
};
use revm::{database::BundleState, state::AccountInfo};

const SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);
const GAS_LIMIT: u64 = 20_000_000;
const SENDER: Address = Address::repeat_byte(0x71);

type TestPool = Pool<
    TransactionValidationTaskExecutor<
        DogeosTransactionValidator<NativeProvider, DogeosPooledTransaction, ScrollEvmConfig>,
    >,
    CoinbaseTipOrdering<DogeosPooledTransaction>,
    InMemoryBlobStore,
>;

fn fixture() -> (NativeProvider, TestPool, tokio::task::JoinHandle<()>) {
    let mut genesis = DOGEOS_DEV.genesis().clone();
    genesis.alloc.insert(
        SENDER,
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
    let provider = initialize_native_v2(chain_spec.clone());
    let validator =
        EthTransactionValidatorBuilder::new(provider.clone(), ScrollEvmConfig::dogeos(chain_spec))
            .no_eip4844()
            .build(InMemoryBlobStore::default());
    // L1 fee validation has its own tests; this fixture isolates the L2 pending base fee.
    let (validator, task) = TransactionValidationTaskExecutor::new(
        DogeosTransactionValidator::disabled(validator, false),
    );
    let task = tokio::spawn(task.run());
    let pool = Pool::new(
        validator,
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );
    (provider, pool, task)
}

fn block(
    provider: &NativeProvider,
    parent: B256,
    number: u64,
    base_fee: u64,
    gas_used: u64,
    overhead: u64,
) -> ExecutedBlock<DogeosPrimitives> {
    let chain_spec = provider.chain_spec();
    let address = chain_spec.config.l1_config.l2_system_config_address;
    let header = alloy_consensus::Header {
        parent_hash: parent,
        number,
        timestamp: chain_spec.genesis().timestamp + number,
        gas_limit: GAS_LIMIT,
        gas_used,
        base_fee_per_gas: Some(base_fee),
        // Distinguish same-height siblings even when their fee/header fields otherwise match.
        extra_data: Bytes::from(overhead.to_be_bytes().to_vec()),
        ..Default::default()
    };
    let recovered = RecoveredBlock::new_unhashed(
        Block {
            header,
            body: Default::default(),
        },
        vec![],
    );
    let mut executed = ExecutedBlock {
        recovered_block: Arc::new(recovered),
        ..Default::default()
    };
    Arc::make_mut(&mut executed.execution_output).state = BundleState::new(
        [(
            address,
            Some(AccountInfo::default()),
            Some(AccountInfo::default()),
            [(SLOT, (U256::ZERO, U256::from(overhead)))]
                .into_iter()
                .collect(),
        )],
        [[(
            address,
            Some(Some(AccountInfo::default())),
            Vec::<(U256, U256)>::new(),
        )]],
        [],
    );
    executed
}

fn commit(provider: &NativeProvider, blocks: Vec<ExecutedBlock<DogeosPrimitives>>) {
    let state = provider.canonical_in_memory_state();
    let tip = blocks.last().unwrap().recovered_block.clone_sealed_header();
    state.update_chain(NewCanonicalChain::Commit { new: blocks });
    state.set_canonical_head(tip);
}

fn chain(block: &ExecutedBlock<DogeosPrimitives>) -> Arc<Chain<DogeosPrimitives>> {
    Arc::new(Chain::new(
        [block.recovered_block.as_ref().clone()],
        ExecutionOutcome {
            bundle: block.execution_output.state.clone(),
            first_block: block.recovered_block.number(),
            receipts: vec![vec![]],
            ..Default::default()
        },
        BTreeMap::new(),
    ))
}

fn builder_fee(provider: &NativeProvider, hash: B256) -> u64 {
    let header = provider.header(hash).unwrap().unwrap();
    let mut db = StateProviderDatabase::new(provider.state_by_block_hash(hash).unwrap());
    ScrollBaseFeeProvider::new(provider.chain_spec())
        .next_block_base_fee(&mut db, &header, header.timestamp)
        .unwrap()
}

async fn wait_for_head(pool: &TestPool, hash: B256) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while pool.block_info().last_seen_block_hash != hash {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn start(
    provider: &NativeProvider,
    pool: &TestPool,
    max_update_depth: u64,
) -> (
    mpsc::UnboundedSender<CanonStateNotification<DogeosPrimitives>>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::unbounded();
    // Spawn through the helper that `DogeosPoolBuilder::build_pool` calls, so these tests fail if
    // the helper stops installing the DogeOS maintenance wrapper. They do not cover the builder's
    // call site: a builder that bypassed the helper would still pass.
    let task = tokio::spawn(dogeos_pool_maintenance_future(
        provider.clone(),
        pool.clone(),
        rx,
        Runtime::test(),
        MaintainPoolConfig {
            max_update_depth,
            ..Default::default()
        },
    ));
    (tx, task)
}

fn priced_transaction(provider: &NativeProvider, price: u64) -> DogeosPooledTransaction {
    let signed: ScrollTransactionSigned = Signed::new_unchecked(
        TxLegacy {
            chain_id: Some(provider.chain_spec().chain().id()),
            gas_price: price as u128,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            ..Default::default()
        },
        Signature::test_signature(),
        B256::repeat_byte(0x72),
    )
    .into();
    let len = signed.encode_2718_len();
    DogeosPooledTransaction::new(Recovered::new_unchecked(signed, SENDER), len)
}

#[tokio::test]
async fn cap_priced_transaction_is_pending_and_announced_on_startup() {
    let (provider, pool, validator) = fixture();
    let parent = block(
        &provider,
        provider.chain_spec().genesis_hash(),
        1,
        MAX_L2_BASE_FEE,
        GAS_LIMIT,
        0,
    );
    let hash = parent.recovered_block.hash();
    commit(&provider, vec![parent.clone()]);
    let header = parent.recovered_block.header();
    let stateless = provider
        .chain_spec()
        .next_block_base_fee(header, header.timestamp())
        .unwrap();
    assert!(stateless > MAX_L2_BASE_FEE);

    // Reproduce the original bug with the exact fee emitted by upstream maintenance.
    pool.set_block_info(BlockInfo {
        last_seen_block_hash: hash,
        block_gas_limit: GAS_LIMIT,
        pending_basefee: stateless,
        ..Default::default()
    });
    let tx = priced_transaction(&provider, MAX_L2_BASE_FEE);
    let tx_hash = *tx.hash();
    let mut gossip = pool.pending_transactions_listener();
    pool.add_transaction(TransactionOrigin::External, tx)
        .await
        .unwrap();
    assert_eq!(pool.pool_size().pending, 0);
    assert_eq!(pool.pool_size().basefee, 1);
    assert!(gossip.try_recv().is_err());

    // Force the wait to observe maintenance's startup update, rather than the setup above.
    pool.set_block_info(BlockInfo {
        block_gas_limit: GAS_LIMIT,
        pending_basefee: stateless,
        ..Default::default()
    });
    let (_events, maintenance) = start(&provider, &pool, 64);
    wait_for_head(&pool, hash).await;
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, hash)
    );
    assert_eq!(pool.block_info().pending_basefee, MAX_L2_BASE_FEE);
    assert_eq!(pool.pool_size().pending, 1);
    assert_eq!(pool.pool_size().basefee, 0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), gossip.recv())
            .await
            .unwrap(),
        Some(tx_hash)
    );
    maintenance.abort();
    validator.abort();
}

#[tokio::test]
async fn commit_uses_event_state_even_when_provider_is_ahead_and_reorg_changes_overhead() {
    let (provider, pool, validator) = fixture();
    let genesis = provider.chain_spec().genesis_hash();
    let (events, maintenance) = start(&provider, &pool, 64);
    wait_for_head(&pool, genesis).await;

    let first = block(&provider, genesis, 1, 1_000_000_000, GAS_LIMIT, 100_000_000);
    let first_hash = first.recovered_block.hash();
    let second = block(
        &provider,
        first_hash,
        2,
        1_000_000_000,
        GAS_LIMIT,
        800_000_000,
    );
    let second_hash = second.recovered_block.hash();
    commit(&provider, vec![first.clone(), second.clone()]);
    assert_ne!(
        builder_fee(&provider, first_hash),
        builder_fee(&provider, second_hash)
    );
    events
        .unbounded_send(CanonStateNotification::Commit { new: chain(&first) })
        .unwrap();
    wait_for_head(&pool, first_hash).await;
    assert_eq!(pool.block_info().pending_basefee, 1_112_500_000);
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, first_hash)
    );
    events
        .unbounded_send(CanonStateNotification::Commit {
            new: chain(&second),
        })
        .unwrap();
    wait_for_head(&pool, second_hash).await;
    assert_eq!(pool.block_info().pending_basefee, 1_025_000_000);
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, second_hash)
    );

    let sibling = block(
        &provider,
        first_hash,
        2,
        1_000_000_000,
        GAS_LIMIT,
        400_000_000,
    );
    let sibling_hash = sibling.recovered_block.hash();
    let state = provider.canonical_in_memory_state();
    state.update_chain(NewCanonicalChain::Reorg {
        new: vec![sibling.clone()],
        old: vec![second.clone()],
    });
    state.set_canonical_head(sibling.recovered_block.clone_sealed_header());
    events
        .unbounded_send(CanonStateNotification::Reorg {
            old: chain(&second),
            new: chain(&sibling),
        })
        .unwrap();
    wait_for_head(&pool, sibling_hash).await;
    assert_eq!(pool.block_info().pending_basefee, 1_075_000_000);
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, sibling_hash)
    );
    maintenance.abort();
    validator.abort();
}

#[tokio::test]
async fn deep_commit_replaces_stateless_fee_and_preserves_head_fields() {
    let (provider, pool, validator) = fixture();
    let genesis = provider.chain_spec().genesis_hash();
    let (events, maintenance) = start(&provider, &pool, 1);
    wait_for_head(&pool, genesis).await;
    let first = block(&provider, genesis, 1, MAX_L2_BASE_FEE, GAS_LIMIT, 0);
    let second = block(
        &provider,
        first.recovered_block.hash(),
        2,
        MAX_L2_BASE_FEE,
        GAS_LIMIT,
        0,
    );
    let hash = second.recovered_block.hash();
    commit(&provider, vec![first, second.clone()]);
    events
        .unbounded_send(CanonStateNotification::Commit {
            new: chain(&second),
        })
        .unwrap();
    wait_for_head(&pool, hash).await;
    let info = pool.block_info();
    assert_eq!(info.pending_basefee, builder_fee(&provider, hash));
    assert_eq!(info.last_seen_block_number, 2);
    assert_eq!(info.block_gas_limit, GAS_LIMIT);
    maintenance.abort();
    validator.abort();
}

#[tokio::test]
async fn default_and_non_default_overhead_match_builder_at_target_and_full_gas() {
    let (provider, pool, validator) = fixture();
    let adapter = DogeosPoolMaintenance::new(pool.clone(), provider.clone());
    let mut parent = provider.chain_spec().genesis_hash();
    let mut number = 0;
    for overhead in [0, 400_000_000, 420_000_000_000] {
        for gas_used in [GAS_LIMIT / 2, GAS_LIMIT] {
            number += 1;
            let head = block(&provider, parent, number, 1_000_000_000, gas_used, overhead);
            parent = head.recovered_block.hash();
            commit(&provider, vec![head]);
            adapter.set_block_info(BlockInfo {
                last_seen_block_hash: parent,
                pending_basefee: u64::MAX,
                ..Default::default()
            });
            assert_eq!(
                pool.block_info().pending_basefee,
                builder_fee(&provider, parent)
            );
        }
    }
    validator.abort();
}

#[tokio::test]
async fn unavailable_head_keeps_previous_fee_then_recovers() {
    let (provider, pool, validator) = fixture();
    let adapter = DogeosPoolMaintenance::new(pool.clone(), provider.clone());
    let unknown = BlockInfo {
        last_seen_block_hash: B256::repeat_byte(0xff),
        last_seen_block_number: 2,
        block_gas_limit: GAS_LIMIT,
        pending_basefee: 0,
        pending_blob_fee: Some(7),
    };

    // Before any head has been applied there is no previous fee: fail closed at the cap.
    adapter.set_block_info(unknown);
    let actual = pool.block_info();
    assert_eq!(actual.pending_basefee, MAX_L2_BASE_FEE);
    assert_eq!(actual.last_seen_block_hash, unknown.last_seen_block_hash);
    assert_eq!(actual.pending_blob_fee, Some(7));

    // Readable head: 1 gwei full parent with 400M overhead, (1e9 - 4e8) * 1.125 + 4e8.
    let head = block(
        &provider,
        provider.chain_spec().genesis_hash(),
        1,
        1_000_000_000,
        GAS_LIMIT,
        400_000_000,
    );
    let hash = head.recovered_block.hash();
    commit(&provider, vec![head]);
    adapter.set_block_info(BlockInfo {
        last_seen_block_hash: hash,
        pending_basefee: 0,
        ..unknown
    });
    assert_eq!(pool.block_info().pending_basefee, 1_075_000_000);

    // A priced-out transaction stays parked while the next head is unreadable.
    let underpriced = priced_transaction(&provider, 1_000_000_000);
    pool.add_transaction(TransactionOrigin::External, underpriced)
        .await
        .unwrap();
    assert_eq!(pool.pool_size().basefee, 1);
    adapter.on_canonical_state_change(CanonicalStateUpdate {
        new_tip: &SealedBlock::seal_slow(Block {
            header: alloy_consensus::Header {
                number: 2,
                gas_limit: GAS_LIMIT,
                ..Default::default()
            },
            body: Default::default(),
        }),
        pending_block_base_fee: 0,
        pending_block_blob_fee: None,
        changed_accounts: vec![],
        mined_transactions: vec![],
        update_kind: PoolUpdateKind::Commit,
    });
    assert_eq!(pool.block_info().pending_basefee, 1_075_000_000);
    assert_eq!(pool.pool_size().pending, 0);
    assert_eq!(pool.pool_size().basefee, 1);

    // Recovery: a readable head restores the exact fee.
    adapter.set_block_info(BlockInfo {
        last_seen_block_hash: provider.chain_spec().genesis_hash(),
        ..unknown
    });
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, provider.chain_spec().genesis_hash())
    );
    validator.abort();
}

#[tokio::test]
async fn reorg_reinserts_mined_transaction_with_the_new_head_fee() {
    use revm::database::{AccountStatus, BundleAccount};

    let (provider, pool, validator) = fixture();
    let genesis = provider.chain_spec().genesis_hash();
    let (events, maintenance) = start(&provider, &pool, 64);
    wait_for_head(&pool, genesis).await;
    let tx = priced_transaction(&provider, MAX_L2_BASE_FEE);
    let hash = *tx.hash();
    pool.add_transaction(TransactionOrigin::External, tx.clone())
        .await
        .unwrap();
    assert_eq!(pool.pool_size().pending, 1);

    let mut old = block(&provider, genesis, 1, MAX_L2_BASE_FEE, GAS_LIMIT, 0);
    old.recovered_block = Arc::new(RecoveredBlock::new_unhashed(
        Block {
            header: old.recovered_block.header().clone(),
            body: alloy_consensus::BlockBody {
                transactions: vec![tx.into_consensus().into_inner()],
                ..Default::default()
            },
        },
        vec![SENDER],
    ));
    let original = AccountInfo {
        balance: U256::MAX,
        ..Default::default()
    };
    let present = AccountInfo {
        nonce: 1,
        balance: U256::MAX - U256::from(21_000_u64 * MAX_L2_BASE_FEE),
        ..Default::default()
    };
    Arc::make_mut(&mut old.execution_output).state.state.insert(
        SENDER,
        BundleAccount::new(
            Some(original),
            Some(present),
            Default::default(),
            AccountStatus::Changed,
        ),
    );
    commit(&provider, vec![old.clone()]);
    events
        .unbounded_send(CanonStateNotification::Commit { new: chain(&old) })
        .unwrap();
    wait_for_head(&pool, old.recovered_block.hash()).await;
    assert!(!pool.contains(&hash));

    let new = block(
        &provider,
        genesis,
        1,
        MAX_L2_BASE_FEE,
        GAS_LIMIT,
        400_000_000,
    );
    let state = provider.canonical_in_memory_state();
    state.update_chain(NewCanonicalChain::Reorg {
        old: vec![old.clone()],
        new: vec![new.clone()],
    });
    state.set_canonical_head(new.recovered_block.clone_sealed_header());
    let mut gossip = pool.pending_transactions_listener();
    events
        .unbounded_send(CanonStateNotification::Reorg {
            old: chain(&old),
            new: chain(&new),
        })
        .unwrap();
    wait_for_head(&pool, new.recovered_block.hash()).await;
    assert_eq!(
        pool.block_info().pending_basefee,
        builder_fee(&provider, new.recovered_block.hash())
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), gossip.recv())
            .await
            .unwrap(),
        Some(hash)
    );
    assert_eq!(pool.pool_size().pending, 1);
    maintenance.abort();
    validator.abort();
}
