use crate::{
    ExecutionInfo, ScrollBuilderConfig, decode_forced_transactions, forced_transactions_da_bytes,
};
use alloy_consensus::Transaction;
use alloy_eips::Typed2718;
use alloy_primitives::U256;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use dogeos_reth_engine::{ScrollBuiltPayload, ScrollPayloadAttributes};
use dogeos_reth_evm::{CodeWitnessHandle, ScrollBaseFeeProvider, ScrollNextBlockEnvAttributes};
use dogeos_reth_primitives::{DogeosPrimitives, ScrollTransactionSigned};
use either::Either;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, MissingPayloadBehaviour, PayloadBuilder, PayloadConfig,
    is_better_payload,
};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_errors::{BlockExecutionError, BlockValidationError};
use reth_evm::{
    ConfigureEvm, Evm,
    block::CommitChanges,
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutor, ExecutorTx},
};
use reth_execution_cache::CachedStateProvider;
use reth_execution_types::BlockExecutionOutput;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::BuiltPayloadExecutedBlock;
use reth_primitives_traits::{
    SignedTransaction, constants::GAS_LIMIT_BOUND_DIVISOR, transaction::TxHashRef,
};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
    ValidPoolTransaction,
    error::{InvalidPoolTransactionError, PoolTransactionError},
};
use revm::context_interface::Block as _;
use std::sync::Arc;
use tracing::{debug, trace, warn};

type BestTransactionsIter<Pool> = Box<
    dyn BestTransactions<Item = Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>,
>;

/// Errors imposed by the sequencer payload contract rather than by the EVM itself.
#[derive(Debug, thiserror::Error)]
pub enum ScrollPayloadBuilderError {
    #[error("failed to recover forced transaction signer")]
    TransactionEcRecoverFailed,
    #[error("blob transaction included in forced transaction list")]
    BlobTransactionRejected,
    #[error("forced transactions exceed block gas limit {gas}: {gas_spent_by_tx:?}")]
    BlockGasLimitExceededByForcedTransactions { gas_spent_by_tx: Vec<u64>, gas: u64 },
    #[error("forced transactions use {bytes} encoded bytes, exceeding block DA limit {limit}")]
    BlockDaLimitExceededByForcedTransactions { bytes: u64, limit: u64 },
    #[error("forced transactions exceed block code witness limit {limit} bytes")]
    BlockCodeLimitExceededByForcedTransactions { limit: u64 },
    #[error("system execution exceeds block code witness limit {limit} bytes")]
    BlockCodeLimitExceededBySystemExecution { limit: u64 },
}

/// A candidate-local resource rejection, not evidence of a consensus-invalid transaction.
#[derive(Debug, thiserror::Error)]
#[error("transaction would exceed block code witness limit {limit} bytes")]
struct CodeWitnessBudgetExceeded {
    limit: u64,
}

impl PoolTransactionError for CodeWitnessBudgetExceeded {
    fn is_bad_transaction(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Reth 2 payload builder for DogeOS/Scroll execution payloads.
#[derive(Debug, Clone)]
pub struct ScrollPayloadBuilder<Pool, Client, EvmConfig> {
    client: Client,
    pool: Pool,
    evm_config: EvmConfig,
    builder_config: ScrollBuilderConfig,
}

impl<Pool, Client, EvmConfig> ScrollPayloadBuilder<Pool, Client, EvmConfig> {
    pub const fn new(
        client: Client,
        pool: Pool,
        evm_config: EvmConfig,
        builder_config: ScrollBuilderConfig,
    ) -> Self {
        Self {
            client,
            pool,
            evm_config,
            builder_config,
        }
    }
}

impl<Pool, Client, EvmConfig> PayloadBuilder for ScrollPayloadBuilder<Pool, Client, EvmConfig>
where
    EvmConfig:
        ConfigureEvm<Primitives = DogeosPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    Client: StateProviderFactory
        + ChainSpecProvider<
            ChainSpec: EthChainSpec
                           + DogeosHardforks
                           + ChainConfig<Config = ScrollChainConfig>
                           + Clone,
        > + Clone,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
{
    type Attributes = ScrollPayloadAttributes;
    type BuiltPayload = ScrollBuiltPayload;

    fn try_build(
        &self,
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        build_payload::<EvmConfig, Client, Pool, _>(
            self.evm_config.clone(),
            self.client.clone(),
            self.builder_config.clone(),
            args,
            |attrs| self.pool.best_transactions_with_attributes(attrs),
        )
    }

    fn on_missing_payload(
        &self,
        _args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> MissingPayloadBehaviour<Self::BuiltPayload> {
        MissingPayloadBehaviour::AwaitInProgress
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        let args = BuildArguments::new(
            Default::default(),
            None,
            None,
            config,
            Default::default(),
            None,
        );
        build_payload::<EvmConfig, Client, Pool, _>(
            self.evm_config.clone(),
            self.client.clone(),
            self.builder_config.clone(),
            args,
            |_| Box::new(std::iter::empty()) as BestTransactionsIter<Pool>,
        )?
        .into_payload()
        .ok_or(PayloadBuilderError::MissingPayload)
    }
}

/// Selects the next block's gas limit.
///
/// An explicit attributes gas limit is used verbatim. Otherwise the block steps from the parent
/// toward the configured target by at most `parent / GAS_LIMIT_BOUND_DIVISOR`, the largest change
/// DogeOS consensus accepts.
fn next_block_gas_limit(attributes: Option<u64>, target: Option<u64>, parent: u64) -> u64 {
    if let Some(gas_limit) = attributes {
        return gas_limit;
    }
    let Some(target) = target else {
        return parent;
    };
    let step = parent / GAS_LIMIT_BOUND_DIVISOR;
    target.clamp(parent.saturating_sub(step), parent.saturating_add(step))
}

fn build_payload<EvmConfig, Client, Pool, F>(
    evm_config: EvmConfig,
    client: Client,
    builder_config: ScrollBuilderConfig,
    args: BuildArguments<ScrollPayloadAttributes, ScrollBuiltPayload>,
    best_txs: F,
) -> Result<BuildOutcome<ScrollBuiltPayload>, PayloadBuilderError>
where
    EvmConfig:
        ConfigureEvm<Primitives = DogeosPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>,
    Client: StateProviderFactory
        + ChainSpecProvider<
            ChainSpec: EthChainSpec
                           + DogeosHardforks
                           + ChainConfig<Config = ScrollChainConfig>
                           + Clone,
        >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>,
    F: FnOnce(BestTransactionsAttributes) -> BestTransactionsIter<Pool>,
{
    let BuildArguments {
        mut cached_reads,
        execution_cache,
        mut trie_handle,
        config,
        cancel,
        best_payload,
    } = args;
    let PayloadConfig {
        parent_header,
        attributes,
        payload_id,
    } = config;

    let mut state_provider = client.state_by_block_hash(parent_header.hash())?;
    if let Some(execution_cache) = execution_cache {
        state_provider = Box::new(CachedStateProvider::new(
            state_provider,
            execution_cache.cache().clone(),
            execution_cache.metrics().clone(),
        ));
    }
    let state = StateProviderDatabase::new(state_provider.as_ref());
    let mut db = State::builder()
        .with_database(cached_reads.as_db_mut(state))
        .with_bundle_update()
        .build();

    let chain_spec = client.chain_spec();
    let base_fee = ScrollBaseFeeProvider::new(chain_spec.clone())
        .next_block_base_fee(
            &mut db,
            parent_header.header(),
            attributes.payload_attributes.timestamp,
        )
        .map_err(PayloadBuilderError::other)?;
    let gas_limit = next_block_gas_limit(
        attributes.gas_limit,
        builder_config.gas_limit,
        parent_header.gas_limit,
    );
    let next_attributes = ScrollNextBlockEnvAttributes {
        timestamp: attributes.payload_attributes.timestamp,
        suggested_fee_recipient: attributes.payload_attributes.suggested_fee_recipient,
        gas_limit,
        base_fee,
    };
    let evm_env = evm_config
        .next_evm_env(&parent_header, &next_attributes)
        .map_err(PayloadBuilderError::other)?;
    let execution_ctx = evm_config
        .context_for_next_block(&parent_header, next_attributes)
        .map_err(PayloadBuilderError::other)?;
    // The handle tracks logical code accesses independently of the database caches. Admission
    // work limits do not apply here: a builder only enforces its block-level code budget.
    let code_witness = CodeWitnessHandle::new(builder_config.max_code_witness_bytes, u64::MAX);
    let evm = evm_config.evm_with_env_and_inspector(&mut db, evm_env, code_witness.inspector());
    let mut builder = evm_config.create_block_builder(evm, &parent_header, execution_ctx);

    debug!(target: "payload_builder", id=%payload_id, parent=?parent_header.hash(), "building DogeOS payload");
    if let Some(ref handle) = trie_handle {
        builder
            .executor_mut()
            .set_state_hook(Some(Box::new(handle.state_hook())));
    }
    code_witness.begin_transaction();
    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;
    accept_system_code_budget(&code_witness, builder_config.max_code_witness_bytes)?;

    let mut info = ExecutionInfo::new();
    let block_gas_limit = builder.evm().block().gas_limit();
    let forced_transactions =
        decode_forced_transactions(&attributes).map_err(PayloadBuilderError::other)?;
    let forced_da_bytes = forced_transactions_da_bytes(&forced_transactions);
    if let Some(limit) = builder_config.max_da_block_size
        && forced_da_bytes > limit
    {
        return Err(PayloadBuilderError::other(
            ScrollPayloadBuilderError::BlockDaLimitExceededByForcedTransactions {
                bytes: forced_da_bytes,
                limit,
            },
        ));
    }

    let mut forced_gas = Vec::new();
    for forced in forced_transactions {
        let encoded_len = forced.encoded_bytes().len() as u64;
        if forced.value().is_eip4844() {
            return Err(PayloadBuilderError::other(
                ScrollPayloadBuilderError::BlobTransactionRejected,
            ));
        }
        let tx = forced.value().try_clone_into_recovered().map_err(|_| {
            PayloadBuilderError::other(ScrollPayloadBuilderError::TransactionEcRecoverFailed)
        })?;
        let tx_gas = tx.gas_limit();
        if info.cumulative_gas_used.saturating_add(tx_gas) > block_gas_limit {
            forced_gas.push(tx_gas);
            return Err(PayloadBuilderError::other(
                ScrollPayloadBuilderError::BlockGasLimitExceededByForcedTransactions {
                    gas_spent_by_tx: forced_gas,
                    gas: block_gas_limit,
                },
            ));
        }
        let gas_used = match execute_with_code_budget(&mut builder, tx.clone(), &code_witness) {
            Ok(gas_used) => {
                forced_code_budget_result(gas_used, builder_config.max_code_witness_bytes)
                    .map_err(PayloadBuilderError::other)?
            }
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                error, ..
            })) => {
                trace!(target: "payload_builder", %error, hash=?tx.tx_hash(), "skipping invalid forced transaction");
                continue;
            }
            Err(err) => return Err(PayloadBuilderError::evm(err)),
        };
        // Count executed gas for every forced transaction, L1 messages included, so this budget
        // matches the executor's block-gas rule (and the header `gas_used`).
        info.cumulative_gas_used += gas_used;
        info.cumulative_da_bytes_used += encoded_len;
        forced_gas.push(gas_used);
    }

    if !attributes.no_tx_pool {
        let breaker = builder_config.breaker();
        let base_fee = builder.evm_mut().block().basefee();
        let mut best = best_txs(BestTransactionsAttributes::new(base_fee, None));
        while let Some(pool_tx) = best.next() {
            let tx = pool_tx.to_consensus();
            if info.is_tx_over_limits(
                tx.inner(),
                block_gas_limit,
                builder_config.max_da_block_size,
            ) || tx.is_eip4844()
                || tx.is_l1_message()
            {
                best.mark_invalid(
                    &pool_tx,
                    &InvalidPoolTransactionError::ExceedsGasLimit(tx.gas_limit(), block_gas_limit),
                );
                continue;
            }
            if cancel.is_cancelled() {
                return Ok(BuildOutcome::Cancelled);
            }
            if breaker.should_break(info.cumulative_gas_used, info.cumulative_da_bytes_used) {
                break;
            }
            let miner_fee = tx.effective_tip_per_gas(base_fee);
            let gas_used = match execute_with_code_budget(&mut builder, tx.clone(), &code_witness) {
                Ok(Some(gas_used)) => gas_used,
                Ok(None) => {
                    best.mark_invalid(
                        &pool_tx,
                        &InvalidPoolTransactionError::other(CodeWitnessBudgetExceeded {
                            limit: builder_config.max_code_witness_bytes,
                        }),
                    );
                    continue;
                }
                Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                    error,
                    ..
                })) => {
                    if !error.is_nonce_too_low() {
                        best.mark_invalid(
                            &pool_tx,
                            &InvalidPoolTransactionError::Consensus(
                                reth_primitives_traits::transaction::error::InvalidTransactionError::TxTypeNotSupported,
                            ),
                        );
                    }
                    continue;
                }
                Err(err) => return Err(PayloadBuilderError::evm(err)),
            };
            info.record_tx(tx.inner(), gas_used);
            info.total_fees +=
                U256::from(miner_fee.expect("valid fee after execution")) * U256::from(gas_used);
        }
        if !is_better_payload(best_payload.as_ref(), info.total_fees) {
            drop(builder);
            return Ok(BuildOutcome::Aborted {
                fees: info.total_fees,
                cached_reads,
            });
        }
    }

    code_witness.begin_transaction();
    let BlockBuilderOutcome {
        execution_result,
        hashed_state,
        trie_updates,
        block,
    } = if let Some(mut handle) = trie_handle.take() {
        // Drop the state hook, which sends FinishedStateUpdates and signals the sparse-trie task
        // to finalize.
        builder.executor_mut().set_state_hook(None);
        match handle.state_root() {
            Ok(outcome) => builder.finish(
                state_provider.as_ref(),
                Some((
                    outcome.state_root,
                    Arc::unwrap_or_clone(outcome.trie_updates),
                )),
            )?,
            Err(err) => {
                warn!(target: "payload_builder", %err, "sparse trie failed; computing state root synchronously");
                builder.finish(state_provider.as_ref(), None)?
            }
        }
    } else {
        builder.finish(state_provider.as_ref(), None)?
    };

    // Finalization may execute system calls. Abort the whole candidate if these do not fit.
    accept_system_code_budget(&code_witness, builder_config.max_code_witness_bytes)?;

    if !attributes.block_data_hint.is_empty() {
        trace!(
            target: "payload_builder",
            "ignoring legacy pre-Euclid block data hint"
        );
    }
    let sealed_block = Arc::new(block.sealed_block().clone());
    let executed = BuiltPayloadExecutedBlock {
        recovered_block: Arc::new(block),
        execution_output: Arc::new(BlockExecutionOutput {
            result: execution_result,
            state: db.take_bundle(),
        }),
        hashed_state: Either::Left(Arc::new(hashed_state)),
        trie_updates: Either::Left(Arc::new(trie_updates)),
    };
    let payload = ScrollBuiltPayload::new(sealed_block, Some(executed), info.total_fees);
    if attributes.no_tx_pool {
        Ok(BuildOutcome::Freeze(payload))
    } else {
        Ok(BuildOutcome::Better {
            payload,
            cached_reads,
        })
    }
}

/// System calls belong to the block even when it contains no user transactions.
fn accept_system_code_budget(
    code_witness: &CodeWitnessHandle,
    limit: u64,
) -> Result<(), PayloadBuilderError> {
    if !code_witness.accept_transaction() {
        return Err(PayloadBuilderError::other(
            ScrollPayloadBuilderError::BlockCodeLimitExceededBySystemExecution { limit },
        ));
    }
    Ok(())
}

/// Forced inputs may not be silently dropped when their code does not fit.
fn forced_code_budget_result(
    gas_used: Option<u64>,
    limit: u64,
) -> Result<u64, ScrollPayloadBuilderError> {
    gas_used.ok_or(ScrollPayloadBuilderError::BlockCodeLimitExceededByForcedTransactions { limit })
}

/// This gate runs before state changes, receipts, and the transaction list are committed.
fn code_witness_commit(code_witness: &CodeWitnessHandle) -> CommitChanges {
    if code_witness.exceeded().is_some() {
        CommitChanges::No
    } else {
        CommitChanges::Yes
    }
}

/// Commits code identities only when the corresponding EVM result was committed successfully.
fn execute_with_code_budget<B: BlockBuilder>(
    builder: &mut B,
    tx: impl ExecutorTx<B::Executor>,
    code_witness: &CodeWitnessHandle,
) -> Result<Option<u64>, BlockExecutionError> {
    code_witness.begin_transaction();
    let outcome = builder
        .execute_transaction_with_commit_condition(tx, |_| code_witness_commit(code_witness));
    if matches!(outcome, Ok(Some(_))) {
        code_witness.accept_transaction();
    } else {
        code_witness.reject_transaction();
    }
    outcome
}

#[cfg(test)]
mod code_witness_tests {
    use super::*;
    use alloy_consensus::{Header, Sealable, Signed, TxLegacy, transaction::Recovered};
    use alloy_primitives::{Address, B256, Bytes, Signature, TxKind};
    use dogeos_chainspec::DOGEOS_MAINNET;
    use dogeos_reth_evm::ScrollEvmConfig;
    use reth_primitives_traits::SealedHeader;
    use revm::{Database, bytecode::Bytecode, database::EmptyDB, state::AccountInfo};

    const SENDER: Address = Address::repeat_byte(0x11);
    /// The default L2 base-fee overhead: a parent at exactly that fee, having burned exactly
    /// the gas target, hands its child the same base fee.
    const TEST_BASE_FEE: u64 = 15_680_000;
    const FIRST: Address = Address::repeat_byte(0x22);
    const SECOND: Address = Address::repeat_byte(0x33);

    fn transaction(to: Address, nonce: u64) -> Recovered<ScrollTransactionSigned> {
        let tx = Signed::new_unchecked(
            TxLegacy {
                chain_id: Some(DOGEOS_MAINNET.chain().id()),
                nonce,
                gas_price: TEST_BASE_FEE.into(),
                gas_limit: 100_000,
                to: TxKind::Call(to),
                ..Default::default()
            },
            Signature::test_signature(),
            B256::repeat_byte(nonce as u8),
        );
        Recovered::new_unchecked(tx.into(), SENDER)
    }

    fn state_with_code() -> State<EmptyDB> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account(
            SENDER,
            AccountInfo {
                balance: U256::from(10u64).pow(U256::from(18u64)),
                ..Default::default()
            },
        );
        for (address, value) in [(FIRST, 1), (SECOND, 2)] {
            // Store the unique value in slot zero, then stop. Pad to 64 bytes.
            let mut bytes = vec![0x60, value, 0x60, 0x00, 0x55, 0x00];
            bytes.resize(64, 0);
            let code = Bytecode::new_raw(Bytes::from(bytes));
            state.insert_account(
                address,
                AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
        }
        state
    }

    fn block_environment() -> (
        ScrollEvmConfig,
        SealedHeader<Header>,
        ScrollNextBlockEnvAttributes,
    ) {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent = SealedHeader::seal_slow(Header {
            number: 1,
            gas_limit: 30_000_000,
            gas_used: 3_000_000,
            base_fee_per_gas: Some(TEST_BASE_FEE),
            ..Default::default()
        });
        let attributes = ScrollNextBlockEnvAttributes {
            timestamp: 1,
            suggested_fee_recipient: Address::ZERO,
            gas_limit: 30_000_000,
            base_fee: TEST_BASE_FEE,
        };
        (config, parent, attributes)
    }

    #[test]
    fn block_budget_rejects_split_access_and_preserves_nonce_state_and_receipts() {
        let (config, parent, attributes) = block_environment();
        let mut state = state_with_code();
        let meter = CodeWitnessHandle::new(80, u64::MAX);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let ctx = config.context_for_next_block(&parent, attributes).unwrap();
        let evm = config.evm_with_env_and_inspector(&mut state, env, meter.inspector());
        let mut builder = config.create_block_builder(evm, &parent, ctx);
        builder.apply_pre_execution_changes().unwrap();

        assert!(
            execute_with_code_budget(&mut builder, transaction(FIRST, 0), &meter)
                .unwrap()
                .is_some()
        );
        assert_eq!(meter.block_code_bytes(), 64);
        // Individually this transaction fits; its distinct code exceeds the block's remainder.
        assert!(
            execute_with_code_budget(&mut builder, transaction(SECOND, 1), &meter)
                .unwrap()
                .is_none()
        );
        assert_eq!(meter.block_code_bytes(), 64);
        assert_eq!(
            builder
                .evm_mut()
                .db_mut()
                .basic(SENDER)
                .unwrap()
                .unwrap()
                .nonce,
            1
        );
        assert_eq!(
            builder
                .evm_mut()
                .db_mut()
                .storage(SECOND, U256::ZERO)
                .unwrap(),
            U256::ZERO
        );
        // Retrying cached code must still fail and must not spend the rejected nonce.
        assert!(
            execute_with_code_budget(&mut builder, transaction(SECOND, 1), &meter)
                .unwrap()
                .is_none()
        );
        // Overlapping accepted code costs zero, even when the block has insufficient room for
        // another copy. The same nonce is still valid after both rejected attempts.
        assert!(
            execute_with_code_budget(&mut builder, transaction(FIRST, 1), &meter)
                .unwrap()
                .is_some()
        );
        assert_eq!(meter.block_code_bytes(), 64);
        let (_, output) = builder.into_executor().finish().unwrap();
        assert_eq!(output.receipts.len(), 2);
        assert_eq!(state.basic(SENDER).unwrap().unwrap().nonce, 2);
        assert_eq!(state.storage(FIRST, U256::ZERO).unwrap(), U256::ONE);
    }

    #[test]
    fn reverted_transaction_keeps_its_code_budget_and_receipt() {
        use alloy_consensus::TxReceipt;

        let (config, parent, attributes) = block_environment();
        let mut state = state_with_code();
        let mut bytes = vec![0x60, 0, 0x60, 0, 0xfd];
        bytes.resize(64, 0);
        let code = Bytecode::new_raw(Bytes::from(bytes));
        state.insert_account(
            FIRST,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        let meter = CodeWitnessHandle::new(64, u64::MAX);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let ctx = config.context_for_next_block(&parent, attributes).unwrap();
        let evm = config.evm_with_env_and_inspector(&mut state, env, meter.inspector());
        let mut builder = config.create_block_builder(evm, &parent, ctx);
        builder.apply_pre_execution_changes().unwrap();

        assert!(
            execute_with_code_budget(&mut builder, transaction(FIRST, 0), &meter)
                .unwrap()
                .is_some()
        );
        assert_eq!(meter.block_code_bytes(), 64);
        let (_, output) = builder.into_executor().finish().unwrap();
        assert_eq!(output.receipts.len(), 1);
        assert!(!output.receipts[0].status());
        assert_eq!(state.basic(SENDER).unwrap().unwrap().nonce, 1);
    }

    #[test]
    fn forced_l1_message_overflow_fails_without_committing() {
        use dogeos_protocol_types::{ScrollTxEnvelope, TxL1Message};

        let (config, parent, attributes) = block_environment();
        let mut state = state_with_code();
        let meter = CodeWitnessHandle::new(63, u64::MAX);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let ctx = config.context_for_next_block(&parent, attributes).unwrap();
        let evm = config.evm_with_env_and_inspector(&mut state, env, meter.inspector());
        let mut builder = config.create_block_builder(evm, &parent, ctx);
        builder.apply_pre_execution_changes().unwrap();
        let forced = ScrollTxEnvelope::L1Message(
            TxL1Message {
                gas_limit: 100_000,
                sender: SENDER,
                to: FIRST,
                ..Default::default()
            }
            .seal_slow(),
        )
        .try_clone_into_recovered()
        .unwrap();

        let outcome = execute_with_code_budget(&mut builder, forced, &meter).unwrap();
        assert!(matches!(
            forced_code_budget_result(outcome, 63),
            Err(
                ScrollPayloadBuilderError::BlockCodeLimitExceededByForcedTransactions { limit: 63 }
            )
        ));
        assert_eq!(meter.block_code_bytes(), 0);
        let (_, output) = builder.into_executor().finish().unwrap();
        assert!(output.receipts.is_empty());
        assert_eq!(state.basic(SENDER).unwrap().unwrap().nonce, 0);
        assert_eq!(state.storage(FIRST, U256::ZERO).unwrap(), U256::ZERO);
    }

    #[test]
    fn system_code_is_reserved_before_user_transactions() {
        use alloy_eips::eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE};

        let (config, parent, attributes) = block_environment();
        let mut state = state_with_code();
        let code = Bytecode::new_raw(HISTORY_STORAGE_CODE.clone());
        state.insert_account(
            HISTORY_STORAGE_ADDRESS,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        let meter = CodeWitnessHandle::new(1, u64::MAX);
        let env = config.next_evm_env(&parent, &attributes).unwrap();
        let ctx = config.context_for_next_block(&parent, attributes).unwrap();
        let evm = config.evm_with_env_and_inspector(&mut state, env, meter.inspector());
        let mut builder = config.create_block_builder(evm, &parent, ctx);

        meter.begin_transaction();
        builder.apply_pre_execution_changes().unwrap();
        assert!(accept_system_code_budget(&meter, 1).is_err());
    }

    #[test]
    fn pool_resource_rejection_does_not_penalize_peers() {
        let error = reth_transaction_pool::error::PoolError::new(
            B256::ZERO,
            InvalidPoolTransactionError::other(CodeWitnessBudgetExceeded { limit: 80 }),
        );
        assert!(!error.is_bad_transaction());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Sealable, SignableTransaction, TxEip1559};
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{Address, B256, Bytes, Signature, TxKind};
    use alloy_rpc_types_engine::{PayloadAttributes, PayloadId};
    use dogeos_chainspec::{DOGEOS_DEV, DogeosChainSpec};
    use dogeos_protocol_types::{ScrollTxEnvelope, TxL1Message};
    use dogeos_reth_consensus::DogeosConsensus;
    use dogeos_reth_engine::ScrollPayloadAttributes;
    use dogeos_reth_evm::ScrollEvmConfig;
    use dogeos_reth_primitives::DogeosBlock;
    use dogeos_reth_txpool::DogeosPooledTransaction;
    use reth_consensus::HeaderValidator;
    use reth_payload_primitives::BuiltPayload;
    use reth_primitives_traits::{Recovered, SealedBlock, SealedHeader};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_transaction_pool::{
        TransactionOrigin,
        identifier::{SenderId, TransactionId},
        noop::NoopTransactionPool,
    };
    use std::{collections::BTreeMap, time::Duration};

    type Provider = MockEthProvider<DogeosPrimitives, DogeosChainSpec>;
    type Pool = NoopTransactionPool<DogeosPooledTransaction>;
    type PooledTx = Arc<ValidPoolTransaction<DogeosPooledTransaction>>;

    const BLOCK_GAS_LIMIT: u64 = 300_000;
    const L1_MESSAGE_GAS_LIMIT: u64 = 200_000;
    const L2_GAS_LIMIT: u64 = 200_000;
    const _: () = assert!(L1_MESSAGE_GAS_LIMIT + L2_GAS_LIMIT > BLOCK_GAS_LIMIT);

    struct VecBestTransactions(std::vec::IntoIter<PooledTx>);

    impl Iterator for VecBestTransactions {
        type Item = PooledTx;

        fn next(&mut self) -> Option<Self::Item> {
            self.0.next()
        }
    }

    impl BestTransactions for VecBestTransactions {
        fn mark_invalid(&mut self, _: &Self::Item, _: &InvalidPoolTransactionError) {}
        fn no_updates(&mut self) {}
        fn set_skip_blobs(&mut self, _: bool) {}
    }

    /// Mock state seeded with the dev genesis allocation, so system contracts and the L1
    /// gas-price oracle are present.
    fn provider() -> Provider {
        let chain_spec = DOGEOS_DEV.clone();
        let provider = MockEthProvider::<DogeosPrimitives>::new()
            .with_chain_spec(chain_spec.as_ref().clone())
            .with_genesis_block();
        for (address, account) in &chain_spec.genesis().alloc {
            let storage = account
                .storage
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect::<BTreeMap<_, _>>();
            let mut extended =
                ExtendedAccount::new(account.nonce.unwrap_or_default(), account.balance)
                    .extend_storage(storage);
            if let Some(code) = account.code.clone() {
                extended = extended.with_bytecode(code);
            }
            provider.add_account(*address, extended);
        }
        provider
    }

    /// An L1 message whose 1800 bytes of non-zero calldata make it use about 50k gas, well below
    /// its 200k gas limit.
    fn l1_message() -> Bytes {
        ScrollTxEnvelope::L1Message(
            TxL1Message {
                queue_index: 0,
                gas_limit: L1_MESSAGE_GAS_LIMIT,
                to: Address::repeat_byte(0xde),
                sender: Address::repeat_byte(0x11),
                input: Bytes::from(vec![1; 1800]),
                ..Default::default()
            }
            .seal_slow(),
        )
        .encoded_2718()
        .into()
    }

    /// A funded L2 transfer with a gas limit that only fits beside the L1 message if the message
    /// is counted at its actual gas.
    fn l2_transaction(provider: &Provider, gas_limit: u64) -> Recovered<ScrollTransactionSigned> {
        let tx: ScrollTransactionSigned = TxEip1559 {
            chain_id: DOGEOS_DEV.chain().id(),
            gas_limit,
            max_fee_per_gas: 10_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            to: TxKind::Call(Address::repeat_byte(0xbe)),
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();
        let tx = tx.try_into_recovered().unwrap();
        provider.add_account(
            tx.signer(),
            ExtendedAccount::new(0, U256::from(10).pow(U256::from(21))),
        );
        tx
    }

    fn build(
        provider: Provider,
        forced: Vec<Bytes>,
        pool: Vec<Recovered<ScrollTransactionSigned>>,
    ) -> Result<SealedBlock<DogeosBlock>, PayloadBuilderError> {
        let chain_spec = DOGEOS_DEV.clone();
        let parent = Arc::new(SealedHeader::seal_slow(chain_spec.genesis_header().clone()));
        let attributes = ScrollPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: parent.timestamp + 1,
                ..Default::default()
            },
            transactions: Some(forced),
            no_tx_pool: pool.is_empty(),
            gas_limit: Some(BLOCK_GAS_LIMIT),
            ..Default::default()
        };
        let pool = pool
            .into_iter()
            .enumerate()
            .map(|(nonce, tx)| {
                let encoded_length = tx.encode_2718_len();
                Arc::new(ValidPoolTransaction {
                    transaction: DogeosPooledTransaction::new(tx, encoded_length),
                    transaction_id: TransactionId::new(SenderId::from(0), nonce as u64),
                    propagate: false,
                    timestamp: std::time::Instant::now(),
                    origin: TransactionOrigin::External,
                    authority_ids: None,
                })
            })
            .collect::<Vec<_>>();
        let args = BuildArguments::new(
            Default::default(),
            None,
            None,
            PayloadConfig::new(parent, attributes, PayloadId::default()),
            Default::default(),
            None,
        );
        let outcome = build_payload::<_, _, Pool, _>(
            ScrollEvmConfig::dogeos(chain_spec),
            provider,
            ScrollBuilderConfig::new(Some(BLOCK_GAS_LIMIT), Duration::from_secs(60), None),
            args,
            |_| Box::new(VecBestTransactions(pool.into_iter())) as BestTransactionsIter<Pool>,
        )?;
        Ok(outcome
            .into_payload()
            .expect("payload was built")
            .block()
            .clone())
    }

    fn l1_message_gas_used() -> u64 {
        let block = build(provider(), vec![l1_message()], vec![]).unwrap();
        let gas_used = block.gas_used;
        assert!(
            (40_000..60_000).contains(&gas_used),
            "L1 message should use about 50k gas, used {gas_used}"
        );
        // The L2 transaction fits only when the L1 message is counted at its actual gas.
        assert!(gas_used + L2_GAS_LIMIT <= BLOCK_GAS_LIMIT);
        gas_used
    }

    #[test]
    fn forced_l2_transaction_fits_after_l1_message_at_actual_gas() {
        let l1_gas = l1_message_gas_used();
        let provider = provider();
        let l2 = l2_transaction(&provider, L2_GAS_LIMIT);
        let block = build(
            provider,
            vec![l1_message(), l2.encoded_2718().into()],
            vec![],
        )
        .expect("follower rebuild accepts a block the executor accepts");

        assert_eq!(block.body().transactions.len(), 2);
        assert_eq!(block.gas_used, l1_gas + 21_000);
    }

    #[test]
    fn pool_transaction_fits_after_l1_message_at_actual_gas() {
        let l1_gas = l1_message_gas_used();
        let provider = provider();
        let l2 = l2_transaction(&provider, L2_GAS_LIMIT);
        let l2_hash = *l2.tx_hash();
        let block = build(provider, vec![l1_message()], vec![l2]).unwrap();

        let hashes = block
            .body()
            .transactions
            .iter()
            .map(|tx| *tx.tx_hash())
            .collect::<Vec<B256>>();
        assert_eq!(hashes.len(), 2, "pool transaction was left out");
        assert_eq!(hashes[1], l2_hash);
        assert_eq!(block.gas_used, l1_gas + 21_000);
    }

    #[test]
    fn forced_transaction_over_remaining_gas_is_still_rejected() {
        let l1_gas = l1_message_gas_used();
        let provider = provider();
        // One gas more than the block has left after the L1 message's actual gas.
        let l2 = l2_transaction(&provider, BLOCK_GAS_LIMIT - l1_gas + 1);
        let err = build(
            provider,
            vec![l1_message(), l2.encoded_2718().into()],
            vec![],
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("forced transactions exceed block gas limit"),
            "unexpected error: {err}"
        );
    }

    fn header(parent_hash: alloy_primitives::B256, number: u64, gas_limit: u64) -> SealedHeader {
        SealedHeader::seal_slow(alloy_consensus::Header {
            parent_hash,
            number,
            gas_limit,
            ..Default::default()
        })
    }

    /// Ramps from `start` to `target` and asserts that DogeOS consensus accepts every block.
    fn ramp(start: u64, target: u64) -> Vec<u64> {
        let mut parent = header(Default::default(), 0, start);
        let mut limits = vec![start];
        while parent.gas_limit != target {
            let gas_limit = next_block_gas_limit(None, Some(target), parent.gas_limit);
            assert_eq!(
                gas_limit.abs_diff(parent.gas_limit),
                (parent.gas_limit / GAS_LIMIT_BOUND_DIVISOR).min(parent.gas_limit.abs_diff(target)),
            );
            let child = header(parent.hash(), parent.number + 1, gas_limit);
            DogeosConsensus
                .validate_header_against_parent(&child, &parent)
                .expect("each ramp step is within the consensus bound");
            limits.push(gas_limit);
            parent = child;
        }
        limits
    }

    #[test]
    fn ramps_up_by_the_maximum_consensus_step() {
        let limits = ramp(10_000_000, 30_000_000);
        assert_eq!(limits[1], 10_000_000 + 10_000_000 / GAS_LIMIT_BOUND_DIVISOR);
        assert!(limits.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(limits.len() - 1, 1_126);

        // One more than the ramp step is over the consensus bound.
        let parent = header(Default::default(), 0, 10_000_000);
        let over = header(
            parent.hash(),
            1,
            10_000_000 + 10_000_000 / GAS_LIMIT_BOUND_DIVISOR + 1,
        );
        assert!(
            DogeosConsensus
                .validate_header_against_parent(&over, &parent)
                .is_err()
        );
    }

    #[test]
    fn ramps_down_by_the_maximum_consensus_step() {
        let limits = ramp(30_000_000, 10_000_000);
        assert_eq!(limits[1], 30_000_000 - 30_000_000 / GAS_LIMIT_BOUND_DIVISOR);
        assert!(limits.windows(2).all(|w| w[1] < w[0]));
        assert_eq!(limits.len() - 1, 1_125);
    }

    #[test]
    fn stops_at_the_target() {
        assert_eq!(
            next_block_gas_limit(None, Some(30_000_000), 30_000_000),
            30_000_000
        );
        assert_eq!(
            next_block_gas_limit(None, Some(30_000_000), 29_999_000),
            30_000_000
        );
        assert_eq!(
            next_block_gas_limit(None, Some(10_000_000), 10_001_000),
            10_000_000
        );
    }

    #[test]
    fn keeps_explicit_attributes_and_defaults_to_parent() {
        assert_eq!(
            next_block_gas_limit(Some(30_000_000), Some(20_000_000), 10_000_000),
            30_000_000
        );
        assert_eq!(next_block_gas_limit(None, None, 10_000_000), 10_000_000);
    }
}
