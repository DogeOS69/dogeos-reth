pub use receipt_builder::{ReceiptBuilderCtx, ScrollReceiptBuilder};
mod receipt_builder;

use crate::{
    FromTxWithCompressionInfo, ScrollDefaultPrecompilesFactory, ScrollEvm, ScrollEvmFactory,
    ScrollPrecompilesFactory, ScrollTransactionIntoTxEnv, ToTxWithCompressionInfo,
    system_caller::ScrollSystemCaller,
    transitions::{apply_feynman_hard_fork, apply_galileo_v2_hard_fork, apply_tsuki_hard_fork},
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
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
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
    Spec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = E;
    type Result = ScrollTxResult<<E as Evm>::HaltReason>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
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

    fn finish(self) -> Result<(Self::Evm, BlockExecutionResult<R::Receipt>), BlockExecutionError> {
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
        ScrollRethReceiptBuilder, ScrollTransactionIntoTxEnv,
        gas_price_oracle::L1_GAS_PRICE_ORACLE_ADDRESS,
    };
    use alloy_evm::{EvmEnv, block::StateChangePreBlockSource};
    use alloy_primitives::{Address, Bytes};
    use dogeos_chainspec::DOGEOS_MAINNET;
    use revm::{
        context::{
            BlockEnv, CfgEnv, TxEnv,
            result::{ExecutionResult, HaltReason, ResultAndState, ResultGas},
        },
        database::{EmptyDB, State},
        state::{Account, AccountInfo, EvmState},
    };
    use revm_scroll::{ScrollSpecId, precompile::transfer::NATIVE_DOGE_TOKEN_ADDRESS};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    type RecordedUpdates = Arc<Mutex<Vec<(StateChangeSource, Vec<Address>)>>>;

    fn recording_hook() -> (RecordedUpdates, Box<dyn OnStateHook>) {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let hook_updates = Arc::clone(&updates);
        let hook = Box::new(move |source, state: &EvmState| {
            let mut keys = state.keys().copied().collect::<Vec<_>>();
            keys.sort_unstable();
            hook_updates.lock().unwrap().push((source, keys));
        });
        (updates, hook)
    }

    fn empty_state_evm()
    -> impl EvmExt<DB = State<EmptyDB>, Tx = ScrollTransactionIntoTxEnv<TxEnv>, HaltReason = HaltReason>
    {
        let state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state,
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                BlockEnv::default(),
            ),
        )
    }

    fn transaction_result(address: Address) -> ScrollTxResult<HaltReason> {
        let mut account = Account::from(AccountInfo {
            balance: U256::ONE,
            ..Default::default()
        });
        account.mark_touch();

        ScrollTxResult {
            result: ResultAndState {
                result: ExecutionResult::Revert {
                    gas: ResultGas::default(),
                    logs: Vec::new(),
                    output: Bytes::new(),
                },
                state: [(address, account)].into_iter().collect(),
            },
            l1_fee: U256::ZERO,
            tx_type: 0,
        }
    }

    #[test]
    fn executor_forwards_transaction_state_hooks_with_receipt_indices() {
        let mut executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            ScrollBlockExecutionCtx::default(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        let (updates, hook) = recording_hook();
        let first = Address::repeat_byte(0x11);
        let second = Address::repeat_byte(0x22);
        executor
            .evm_mut()
            .db_mut()
            .load_cache_account(first)
            .unwrap();
        executor
            .evm_mut()
            .db_mut()
            .load_cache_account(second)
            .unwrap();
        executor.set_state_hook(Some(hook));

        executor
            .commit_transaction(transaction_result(first))
            .unwrap();
        executor
            .commit_transaction(transaction_result(second))
            .unwrap();

        let updates = updates.lock().unwrap();
        assert_eq!(updates.len(), 2);
        assert!(matches!(updates[0].0, StateChangeSource::Transaction(0)));
        assert_eq!(updates[0].1, [first]);
        assert!(matches!(updates[1].0, StateChangeSource::Transaction(1)));
        assert_eq!(updates[1].1, [second]);
    }

    #[test]
    fn executor_forwards_pre_block_transition_state_hooks() {
        let mut executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            ScrollBlockExecutionCtx::default(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        let (updates, hook) = recording_hook();
        executor.set_state_hook(Some(hook));

        executor.apply_pre_execution_changes().unwrap();

        let updates = updates.lock().unwrap();
        assert!(!updates.is_empty());
        assert!(updates.iter().all(|(source, _)| matches!(
            source,
            StateChangeSource::PreBlock(StateChangePreBlockSource::BlockHashesContract)
        )));
        let keys = updates
            .iter()
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect::<Vec<_>>();
        assert!(keys.contains(&L1_GAS_PRICE_ORACLE_ADDRESS));
        assert!(keys.contains(&NATIVE_DOGE_TOKEN_ADDRESS));
    }

    #[test]
    fn clearing_executor_state_hook_drops_previous_hook_once() {
        struct DropCounter(Arc<AtomicUsize>);

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let mut executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            ScrollBlockExecutionCtx::default(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        let drops = Arc::new(AtomicUsize::new(0));
        let guard = DropCounter(Arc::clone(&drops));
        executor.set_state_hook(Some(Box::new(move |_, _: &EvmState| {
            let _ = &guard;
        })));
        assert_eq!(drops.load(Ordering::Relaxed), 0);

        executor.set_state_hook(None);

        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
