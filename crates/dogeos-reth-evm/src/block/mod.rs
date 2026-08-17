pub use receipt_builder::{ReceiptBuilderCtx, ScrollReceiptBuilder};
mod receipt_builder;

use crate::{
    FromTxWithCompressionInfo, ScrollBaseFeeProvider, ScrollDefaultPrecompilesFactory, ScrollEvm,
    ScrollEvmFactory, ScrollPrecompilesFactory, ScrollTransactionIntoTxEnv,
    ToTxWithCompressionInfo, calculate_next_controlled_base_fee,
    system_caller::ScrollSystemCaller,
    transitions::{
        apply_feynman_hard_fork, apply_galileo_v2_hard_fork, apply_tsuki_hard_fork,
        store_next_controlled_base_fee_with_state,
    },
};

use alloc::{boxed::Box, format, vec::Vec};
use alloy_consensus::{Transaction, TxReceipt, Typed2718};
use alloy_eips::Encodable2718;
use alloy_evm::{
    Database, Evm, EvmFactory, FromRecoveredTx, FromTxWithEncoded, RecoveredTx,
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        BlockExecutorFor, BlockValidationError, ExecutableTx, OnStateHook, StateChangeSource,
        StateDB, TxResult,
    },
};
use alloy_primitives::{B256, U256};
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::{DogeosHardfork, DogeosHardforks};
use dogeos_protocol_types::L1_MESSAGE_TRANSACTION_TYPE;
use revm::{
    Inspector,
    context::{Block, ContextTr, TxEnv, result::InvalidTransaction},
    handler::PrecompileProvider,
    interpreter::InterpreterResult,
};
use revm_scroll::builder::ScrollContext;

/// Compression info is a pair of (compression ratio, compressed size).
pub type ScrollTxCompressionInfo = (U256, usize);

/// The result of executing a Scroll transaction.
#[derive(Debug)]
pub struct ScrollTxResult<H> {
    /// Result of the transaction execution.
    pub result: revm::context::result::ResultAndState<H>,
    /// L1 fee for the transaction (zero for L1 messages).
    pub l1_fee: U256,
    /// Transaction type byte.
    pub tx_type: u8,
}

impl<H> TxResult for ScrollTxResult<H> {
    type HaltReason = H;

    fn result(&self) -> &revm::context::result::ResultAndState<H> {
        &self.result
    }

    fn into_result(self) -> revm::context::result::ResultAndState<H> {
        self.result
    }
}

/// A cache for transaction compression infos, i.e. (compression ratio, compressed size) pairs.
pub type ScrollTxCompressionInfos = Vec<ScrollTxCompressionInfo>;

/// Context for Scroll Block Execution.
#[derive(Debug, Default, Clone)]
pub struct ScrollBlockExecutionCtx {
    /// Parent block hash.
    pub parent_hash: B256,
}

/// Block executor for Scroll.
#[derive(Debug)]
pub struct ScrollBlockExecutor<Evm, R: ScrollReceiptBuilder, Spec> {
    /// Spec.
    spec: Spec,
    /// Receipt builder.
    receipt_builder: R,
    /// The EVM used by executor.
    evm: Evm,
    /// Context for block execution.
    ctx: ScrollBlockExecutionCtx,
    /// Receipts of executed transactions.
    receipts: Vec<R::Receipt>,
    /// Total gas used by executed transactions.
    gas_used: u64,
    /// Tsuki controlled component accepted for the block currently being executed.
    current_controlled_base_fee: Option<u64>,
    /// Utility to call system smart contracts.
    system_caller: ScrollSystemCaller<Spec>,
}

impl<E, R: ScrollReceiptBuilder, Spec> ScrollBlockExecutor<E, R, Spec> {
    /// Returns the spec for [`ScrollBlockExecutor`].
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }
}

impl<E, R, Spec> ScrollBlockExecutor<E, R, Spec>
where
    E: EvmExt,
    R: ScrollReceiptBuilder,
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig> + Clone,
{
    /// Creates a new [`ScrollBlockExecutor`].
    pub fn new(evm: E, ctx: ScrollBlockExecutionCtx, spec: Spec, receipt_builder: R) -> Self {
        Self {
            evm,
            ctx,
            system_caller: ScrollSystemCaller::new(spec.clone()),
            spec,
            receipt_builder,
            receipts: Vec::new(),
            gas_used: 0,
            current_controlled_base_fee: None,
        }
    }
}

impl<DB, E, R, Spec> ScrollBlockExecutor<E, R, Spec>
where
    DB: StateDB,
    E: EvmExt<
            DB = DB,
            Tx: FromRecoveredTx<R::Transaction>
                    + FromTxWithEncoded<R::Transaction>
                    + FromTxWithCompressionInfo<R::Transaction>,
        >,
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig> + Clone,
{
    /// Executes all transactions in a block, applying pre and post execution changes. The provided
    /// transaction compression infos are expected to be in the same order as the
    /// transactions.
    pub fn execute_block_with_compression_cache(
        mut self,
        transactions: impl IntoIterator<
            Item = impl ExecutableTx<Self>
                   + ToTxWithCompressionInfo<<Self as BlockExecutor>::Transaction>,
        >,
        compression_infos: impl IntoIterator<Item = ScrollTxCompressionInfo>,
    ) -> Result<BlockExecutionResult<R::Receipt>, BlockExecutionError>
    where
        Self: Sized,
    {
        self.apply_pre_execution_changes()?;

        for (tx, (compression_ratio, compressed_size)) in
            transactions.into_iter().zip(compression_infos)
        {
            let tx = tx.with_compression_info(compression_ratio, compressed_size);
            self.execute_transaction(&tx)?;
        }

        self.apply_post_execution_changes()
    }
}

impl<DB, E, R, Spec> BlockExecutor for ScrollBlockExecutor<E, R, Spec>
where
    DB: StateDB,
    E: EvmExt<DB = DB, Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>>,
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig> + Clone,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = E;
    type Result = ScrollTxResult<<E as Evm>::HaltReason>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        let timestamp = self.evm.block().timestamp().to();
        if self.spec.is_tsuki_active_at_timestamp(timestamp) {
            let actual_base_fee = self.evm.block().basefee();
            let state = ScrollBaseFeeProvider::new(self.spec.clone())
                .dynamic_base_fee_state(self.evm.db_mut())
                .map_err(|error| {
                    BlockExecutionError::msg(format!(
                        "failed to derive Tsuki base fee from parent state: {error}"
                    ))
                })?;
            let expected_base_fee = state.header_base_fee();
            if actual_base_fee != expected_base_fee {
                return Err(BlockExecutionError::msg(format!(
                    "invalid Tsuki base fee: expected {expected_base_fee}, got {actual_base_fee}"
                )));
            }
            self.current_controlled_base_fee = Some(state.controlled_fee);
        }

        // apply gas oracle predeploy upgrade at Feynman transition block.
        #[allow(clippy::collapsible_if)]
        if self
            .spec
            .dogeos_fork_activation(DogeosHardfork::Feynman)
            .active_at_timestamp(self.evm.block().timestamp().to())
        {
            let state = apply_feynman_hard_fork(self.evm.db_mut()).map_err(|err| {
                BlockExecutionError::msg(format!("error occurred at Feynman fork: {err:?}"))
            })?;
            self.system_caller.on_pre_block_state(&state);
        }

        // apply gas oracle predeploy upgrade at GalileoV2 transition block.
        #[allow(clippy::collapsible_if)]
        if self
            .spec
            .dogeos_fork_activation(DogeosHardfork::GalileoV2)
            .active_at_timestamp(self.evm.block().timestamp().to())
        {
            let state = apply_galileo_v2_hard_fork(self.evm.db_mut()).map_err(|err| {
                BlockExecutionError::msg(format!("error occurred at GalileoV2 fork: {err:?}"))
            })?;
            self.system_caller.on_pre_block_state(&state);
        }

        // inject NativeDogeToken predeploy at Tsuki transition block.
        #[allow(clippy::collapsible_if)]
        if self
            .spec
            .dogeos_fork_activation(DogeosHardfork::Tsuki)
            .active_at_timestamp(self.evm.block().timestamp().to())
        {
            let state = apply_tsuki_hard_fork(self.evm.db_mut()).map_err(|err| {
                BlockExecutionError::msg(format!("error occurred at Tsuki fork: {err:?}"))
            })?;
            self.system_caller.on_pre_block_state(&state);
        }

        // apply eip-2935.
        self.system_caller
            .apply_blockhashes_contract_call(self.ctx.parent_hash, &mut self.evm)?;

        Ok(())
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (tx_env, recovered) = tx.into_parts();
        let tx = recovered.tx();
        let chain_spec = &self.spec;
        let is_l1_message = tx.ty() == L1_MESSAGE_TRANSACTION_TYPE;

        // The sum of the transaction’s gas limit and the gas utilized in this block prior,
        // must be no greater than the block’s gasLimit.
        let block_available_gas = self.evm.block().gas_limit() - self.gas_used;
        if tx.gas_limit() > block_available_gas {
            return Err(
                BlockValidationError::TransactionGasLimitMoreThanAvailableBlockGas {
                    transaction_gas_limit: tx.gas_limit(),
                    block_available_gas,
                }
                .into(),
            );
        }

        let hash = tx.trie_hash();
        let tx_type = tx.ty();

        // Cache block values before mutable evm access.
        // Feynman is the baseline, so EIP-2930, EIP-1559 and EIP-7702 no longer
        // need legacy fork gates. Blob transactions remain unsupported.
        if tx.is_eip4844() {
            return Err(BlockValidationError::InvalidTx {
                hash,
                error: alloc::boxed::Box::new(InvalidTransaction::Eip4844NotSupported),
            }
            .into());
        }

        // disable the base fee and nonce checks for l1 messages.
        self.evm.with_base_fee_check(!is_l1_message);
        self.evm.with_nonce_check(!is_l1_message);
        self.evm
            .with_l1_data_fee_buffer_check(chain_spec.chain_config().l1_data_fee_buffer_check);

        // execute the transaction.
        let result = self
            .evm
            .transact(tx_env)
            .map_err(move |err| BlockExecutionError::evm(err, hash))?;

        let l1_fee = if is_l1_message {
            U256::ZERO
        } else {
            // compute l1 fee for all non-l1 transactions
            self.evm.l1_fee().expect("l1 fee loaded")
        };

        Ok(ScrollTxResult {
            result,
            l1_fee,
            tx_type,
        })
    }

    fn commit_transaction(&mut self, output: Self::Result) -> Result<u64, BlockExecutionError> {
        let ScrollTxResult {
            result: revm::context::result::ResultAndState { result, state },
            l1_fee,
            tx_type,
        } = output;

        self.system_caller
            .on_state(StateChangeSource::Transaction(self.receipts.len()), &state);

        let gas_used = result.gas_used();
        self.gas_used += gas_used;

        let ctx = ReceiptBuilderCtx::<E> {
            tx_type,
            result,
            cumulative_gas_used: self.gas_used,
            l1_fee,
        };
        self.receipts.push(self.receipt_builder.build_receipt(ctx));

        self.evm.db_mut().commit(state);

        Ok(gas_used)
    }

    fn finish(
        mut self,
    ) -> Result<(Self::Evm, BlockExecutionResult<R::Receipt>), BlockExecutionError> {
        if let Some(controlled_fee) = self.current_controlled_base_fee {
            // Re-read parameters after transaction execution so a SystemConfig update in this
            // block takes effect in the next block. Rebase before applying the formula so a valid
            // floor/ceiling update cannot strand the controller outside its new range.
            let params = ScrollBaseFeeProvider::new(self.spec.clone())
                .dynamic_base_fee_params(self.evm.db_mut())
                .map_err(|error| {
                    BlockExecutionError::msg(format!(
                        "failed to read next dynamic base-fee parameters: {error}"
                    ))
                })?;
            let controlled_fee = params.rebase_controlled_fee(controlled_fee);
            let next_controlled_fee =
                calculate_next_controlled_base_fee(controlled_fee, self.gas_used, params).map_err(
                    |error| {
                        BlockExecutionError::msg(format!(
                            "failed to calculate next controlled base fee: {error}"
                        ))
                    },
                )?;
            let system_config = self.spec.chain_config().l1_config.l2_system_config_address;
            let state = store_next_controlled_base_fee_with_state(
                self.evm.db_mut(),
                system_config,
                next_controlled_fee,
            )
            .map_err(|error| {
                BlockExecutionError::msg(format!(
                    "failed to persist next controlled base fee: {error:?}"
                ))
            })?;
            self.system_caller.on_post_block_state(&state);
        }

        Ok((
            self.evm,
            BlockExecutionResult {
                receipts: self.receipts,
                requests: Default::default(),
                gas_used: self.gas_used,
                blob_gas_used: 0,
            },
        ))
    }

    fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.system_caller.set_state_hook(hook);
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        &mut self.evm
    }

    fn evm(&self) -> &Self::Evm {
        &self.evm
    }

    fn receipts(&self) -> &[Self::Receipt] {
        &self.receipts
    }
}

/// An extension of the [`Evm`] trait for Scroll.
pub trait EvmExt: Evm {
    /// Sets whether the evm should enable or disable the base fee checks.
    fn with_base_fee_check(&mut self, enabled: bool);
    /// Sets whether the evm should enable or disable the nonce checks.
    fn with_nonce_check(&mut self, enabled: bool);
    /// Sets whether the evm should enable or disable the l1 data fee buffer checks.
    fn with_l1_data_fee_buffer_check(&mut self, enabled: bool);
    /// Returns the l1 fee for the transaction.
    fn l1_fee(&self) -> Option<U256>;
}

impl<DB, I, P> EvmExt for ScrollEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<ScrollContext<DB>>,
    P: PrecompileProvider<ScrollContext<DB>, Output = InterpreterResult>,
{
    fn with_base_fee_check(&mut self, enabled: bool) {
        self.ctx_mut().cfg.disable_base_fee = !enabled;
    }

    fn with_nonce_check(&mut self, enabled: bool) {
        self.ctx_mut().cfg.disable_nonce_check = !enabled;
    }

    fn with_l1_data_fee_buffer_check(&mut self, enabled: bool) {
        self.chain_mut().policy.require_l1_data_fee_buffer = enabled;
    }

    fn l1_fee(&self) -> Option<U256> {
        let l1_block_info = &self.ctx().chain.l1_block_info;
        let transaction_rlp_bytes = self.ctx().tx.rlp_bytes.as_ref()?;
        let compression_ratio = self.ctx().tx.compression_ratio;
        let compressed_size = self.ctx().tx.compressed_size;
        Some(l1_block_info.calculate_tx_l1_cost(
            transaction_rlp_bytes,
            self.ctx().cfg.spec,
            compression_ratio,
            compressed_size,
        ))
    }
}

/// Scroll block executor factory.
#[derive(Debug, Clone, Default, Copy)]
pub struct ScrollBlockExecutorFactory<R, Spec, P = ScrollDefaultPrecompilesFactory> {
    /// Receipt builder.
    receipt_builder: R,
    /// Chain specification.
    spec: Spec,
    /// EVM factory.
    evm_factory: ScrollEvmFactory<P>,
}

impl<R, Spec, P> ScrollBlockExecutorFactory<R, Spec, P> {
    /// Creates a new [`ScrollBlockExecutorFactory`] with the given receipt builder, spec and
    /// factory.
    pub const fn new(receipt_builder: R, spec: Spec, evm_factory: ScrollEvmFactory<P>) -> Self {
        Self {
            receipt_builder,
            spec,
            evm_factory,
        }
    }

    /// Exposes the receipt builder.
    pub const fn receipt_builder(&self) -> &R {
        &self.receipt_builder
    }

    /// Exposes the chain specification.
    pub const fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Exposes the EVM factory.
    pub const fn evm_factory(&self) -> &ScrollEvmFactory<P> {
        &self.evm_factory
    }
}

impl<R, Spec, P> BlockExecutorFactory for ScrollBlockExecutorFactory<R, Spec, P>
where
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig> + Clone,
    P: ScrollPrecompilesFactory,
    ScrollTransactionIntoTxEnv<TxEnv>:
        FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>,
    Self: 'static,
{
    type EvmFactory = ScrollEvmFactory<P>;
    type ExecutionCtx<'a> = ScrollBlockExecutionCtx;
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;

    fn evm_factory(&self) -> &Self::EvmFactory {
        &self.evm_factory
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <Self::EvmFactory as EvmFactory>::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> impl BlockExecutorFor<'a, Self, DB, I>
    where
        DB: StateDB + 'a,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>> + 'a,
    {
        ScrollBlockExecutor::new(evm, ctx, self.spec.clone(), &self.receipt_builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DEFAULT_BASE_FEE_OVERHEAD, INITIAL_CONTROLLED_BASE_FEE, ScrollRethReceiptBuilder,
        dynamic_base_fee_slots as slots,
    };
    use alloy_evm::{EvmEnv, EvmFactory};
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_MAINNET, DogeosChainSpec};
    use revm::{
        Database,
        context::{BlockEnv, CfgEnv},
        database::{EmptyDB, State, states::plain_account::PlainStorage},
        state::AccountInfo,
    };
    use revm_scroll::ScrollSpecId;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn executor(
        chain_spec: alloc::sync::Arc<DogeosChainSpec>,
        spec_id: ScrollSpecId,
        base_fee: u64,
    ) -> ScrollBlockExecutor<
        ScrollEvm<
            State<EmptyDB>,
            revm::inspector::NoOpInspector,
            alloy_evm::precompiles::PrecompilesMap,
        >,
        ScrollRethReceiptBuilder,
        alloc::sync::Arc<DogeosChainSpec>,
    > {
        let state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        executor_with_state(state, chain_spec, spec_id, base_fee)
    }

    fn executor_with_state(
        state: State<EmptyDB>,
        chain_spec: alloc::sync::Arc<DogeosChainSpec>,
        spec_id: ScrollSpecId,
        base_fee: u64,
    ) -> ScrollBlockExecutor<
        ScrollEvm<
            State<EmptyDB>,
            revm::inspector::NoOpInspector,
            alloy_evm::precompiles::PrecompilesMap,
        >,
        ScrollRethReceiptBuilder,
        alloc::sync::Arc<DogeosChainSpec>,
    > {
        let env = EvmEnv::new(
            CfgEnv::new_with_spec(spec_id),
            BlockEnv {
                number: U256::ONE,
                timestamp: U256::ONE,
                gas_limit: 20_000_000,
                basefee: base_fee,
                ..Default::default()
            },
        );
        let evm =
            ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(state, env);
        ScrollBlockExecutor::new(
            evm,
            ScrollBlockExecutionCtx::default(),
            chain_spec,
            ScrollRethReceiptBuilder,
        )
    }

    fn tsuki_executor(
        base_fee: u64,
    ) -> ScrollBlockExecutor<
        ScrollEvm<
            State<EmptyDB>,
            revm::inspector::NoOpInspector,
            alloy_evm::precompiles::PrecompilesMap,
        >,
        ScrollRethReceiptBuilder,
        alloc::sync::Arc<DogeosChainSpec>,
    > {
        executor(DOGEOS_MAINNET.clone(), ScrollSpecId::TSUKI, base_fee)
    }

    #[test]
    fn tsuki_executor_validates_seed_and_persists_next_controlled_fee() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let expected = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone()).next_block_base_fee(
            &mut state,
            &alloy_consensus::Header::default(),
            1,
        )?;
        assert_eq!(
            expected,
            INITIAL_CONTROLLED_BASE_FEE + DEFAULT_BASE_FEE_OVERHEAD.to::<u64>()
        );
        let mut executor =
            executor_with_state(state, DOGEOS_MAINNET.clone(), ScrollSpecId::TSUKI, expected);
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let saw_controller_update = alloc::sync::Arc::new(AtomicBool::new(false));
        let hook_flag = saw_controller_update.clone();
        executor.set_state_hook(Some(Box::new(move |_, state: &revm::state::EvmState| {
            if state
                .get(&system_config)
                .and_then(|account| account.storage.get(&slots::NEXT_CONTROLLED_FEE.value()))
                .is_some_and(|slot| slot.present_value == U256::from(437_500_000_000u64))
            {
                hook_flag.store(true, Ordering::Relaxed);
            }
        })));

        executor.apply_pre_execution_changes()?;
        let (mut evm, result) = executor.finish()?;

        assert_eq!(result.gas_used, 0);
        assert_eq!(
            evm.db_mut()
                .storage(system_config, slots::NEXT_CONTROLLED_FEE.value())?,
            U256::from(437_500_000_000u64)
        );
        assert_eq!(evm.db_mut().basic(system_config)?.unwrap().nonce, 1);
        assert!(saw_controller_update.load(Ordering::Relaxed));
        Ok(())
    }

    #[test]
    fn tsuki_executor_rejects_a_header_fee_mismatch() {
        let expected = INITIAL_CONTROLLED_BASE_FEE + DEFAULT_BASE_FEE_OVERHEAD.to::<u64>();
        let mut executor = tsuki_executor(expected + 1);

        assert!(
            executor
                .apply_pre_execution_changes()
                .unwrap_err()
                .to_string()
                .contains("invalid Tsuki base fee")
        );
    }

    #[test]
    fn later_tsuki_block_uses_persisted_components() -> eyre::Result<()> {
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            system_config,
            AccountInfo {
                nonce: 1,
                ..Default::default()
            },
            PlainStorage::from_iter([
                (U256::from(101), U256::from(100_000_000)),
                (
                    slots::NEXT_CONTROLLED_FEE.value(),
                    U256::from(600_000_000_000u64),
                ),
            ]),
        );
        let expected = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone()).next_block_base_fee(
            &mut state,
            &alloy_consensus::Header::default(),
            1,
        )?;
        assert_eq!(expected, 600_100_000_000);
        let mut executor =
            executor_with_state(state, DOGEOS_MAINNET.clone(), ScrollSpecId::TSUKI, expected);

        executor.apply_pre_execution_changes()?;
        let (mut evm, _) = executor.finish()?;

        assert_eq!(
            evm.db_mut()
                .storage(system_config, slots::NEXT_CONTROLLED_FEE.value())?,
            U256::from(525_000_000_000u64)
        );
        Ok(())
    }

    #[test]
    fn parameter_update_rebases_controller_for_the_next_block() -> eyre::Result<()> {
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            system_config,
            AccountInfo {
                nonce: 1,
                ..Default::default()
            },
            PlainStorage::from_iter([(
                slots::NEXT_CONTROLLED_FEE.value(),
                U256::from(800_000_000_000u64),
            )]),
        );
        let expected = 800_000_000_000 + DEFAULT_BASE_FEE_OVERHEAD.to::<u64>();
        let mut executor =
            executor_with_state(state, DOGEOS_MAINNET.clone(), ScrollSpecId::TSUKI, expected);

        executor.apply_pre_execution_changes()?;
        executor.evm.db_mut().insert_account_with_storage(
            system_config,
            AccountInfo {
                nonce: 1,
                ..Default::default()
            },
            PlainStorage::from_iter([
                (
                    slots::INITIAL_CONTROLLED_FEE.value(),
                    U256::from(200_000_000_000u64),
                ),
                (slots::MAXIMUM.value(), U256::from(300_000_000_000u64)),
            ]),
        );

        let (mut evm, _) = executor.finish()?;
        assert_eq!(
            evm.db_mut()
                .storage(system_config, slots::NEXT_CONTROLLED_FEE.value())?,
            U256::from(262_500_000_000u64)
        );
        Ok(())
    }

    #[test]
    fn pre_tsuki_executor_does_not_write_controller_state() -> eyre::Result<()> {
        let mut executor = executor(DOGEOS_CHIKYU.clone(), ScrollSpecId::FEYNMAN, 1_000_000_000);

        executor.apply_pre_execution_changes()?;
        let (mut evm, _) = executor.finish()?;

        let system_config = DOGEOS_CHIKYU.config.l1_config.l2_system_config_address;
        assert_eq!(
            evm.db_mut()
                .storage(system_config, slots::NEXT_CONTROLLED_FEE.value())?,
            U256::ZERO
        );
        Ok(())
    }
}
