use crate::{
    ExecutionInfo, ScrollBuilderConfig, decode_forced_transactions, forced_transactions_da_bytes,
};
use alloy_consensus::Transaction;
use alloy_eips::Typed2718;
use alloy_primitives::U256;
use alloy_rlp::Encodable;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use dogeos_reth_engine::{ScrollBuiltPayload, ScrollPayloadAttributes};
use dogeos_reth_evm::{ScrollBaseFeeProvider, ScrollNextBlockEnvAttributes};
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
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutor},
};
use reth_execution_cache::CachedStateProvider;
use reth_execution_types::BlockExecutionOutput;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::BuiltPayloadExecutedBlock;
use reth_primitives_traits::{SignedTransaction, transaction::TxHashRef};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::StateProviderFactory;
use reth_transaction_pool::{
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
    ValidPoolTransaction, error::InvalidPoolTransactionError,
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
    let gas_limit = attributes
        .gas_limit
        .or(builder_config.gas_limit)
        .unwrap_or(parent_header.gas_limit);
    let mut builder = evm_config
        .builder_for_next_block(
            &mut db,
            &parent_header,
            ScrollNextBlockEnvAttributes {
                timestamp: attributes.payload_attributes.timestamp,
                suggested_fee_recipient: attributes.payload_attributes.suggested_fee_recipient,
                gas_limit,
                base_fee,
            },
        )
        .map_err(PayloadBuilderError::other)?;

    debug!(target: "payload_builder", id=%payload_id, parent=?parent_header.hash(), "building DogeOS payload");
    if let Some(ref handle) = trie_handle {
        builder
            .executor_mut()
            .set_state_hook(Some(Box::new(handle.state_hook())));
    }
    builder.apply_pre_execution_changes().map_err(|err| {
        warn!(target: "payload_builder", %err, "failed to apply pre-execution changes");
        PayloadBuilderError::Internal(err.into())
    })?;

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
        let gas_used = match builder.execute_transaction(tx.clone()) {
            Ok(gas_used) => gas_used,
            Err(BlockExecutionError::Validation(BlockValidationError::InvalidTx {
                error, ..
            })) => {
                trace!(target: "payload_builder", %error, hash=?tx.tx_hash(), "skipping invalid forced transaction");
                continue;
            }
            Err(err) => return Err(PayloadBuilderError::evm(err)),
        };
        let gas_used = if tx.is_l1_message() {
            tx.gas_limit()
        } else {
            gas_used
        };
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
            let gas_used = match builder.execute_transaction(tx.clone()) {
                Ok(gas_used) => gas_used,
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
            info.cumulative_gas_used += gas_used;
            info.cumulative_da_bytes_used += tx.inner().length() as u64;
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

    let BlockBuilderOutcome {
        execution_result,
        hashed_state,
        trie_updates,
        block,
    } = if let Some(mut handle) = trie_handle.take() {
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
