pub use receipt_builder::{ReceiptBuilderCtx, ScrollReceiptBuilder};
mod receipt_builder;

use crate::{
    FromTxWithCompressionInfo, ScrollBaseFeeParent, ScrollDefaultPrecompilesFactory, ScrollEvm,
    ScrollEvmFactory, ScrollPrecompilesFactory, ScrollTransactionIntoTxEnv,
    ToTxWithCompressionInfo,
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
use reth_chainspec::EthChainSpec;
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
    /// The parent header's inputs to the Feynman+ base-fee rule.
    ///
    /// `None` when the caller had no parent header to offer. Executing a Feynman+ block to
    /// completion then fails, rather than skipping the rule: see
    /// [`ScrollBlockExecutor::apply_post_execution_changes`].
    pub base_fee_parent: Option<ScrollBaseFeeParent>,
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
    /// L2 base-fee overhead read from the block's pre-state, set in pre-execution.
    base_fee_overhead: Option<u64>,
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
            base_fee_overhead: None,
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
    Spec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    /// Executes all transactions in a block, applying pre and post execution changes. The provided
    /// transaction compression infos are expected to be in the same order as the
    /// transactions, one per transaction.
    ///
    /// Returns an error if the transaction and compression info counts differ. Pre-execution
    /// changes and earlier transactions may already be applied, so the caller must discard
    /// the executor's state on error.
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

        let mut compression_infos = compression_infos.into_iter();
        for tx in transactions {
            let Some((compression_ratio, compressed_size)) = compression_infos.next() else {
                return Err(BlockExecutionError::msg(
                    "fewer compression infos than transactions",
                ));
            };
            let tx = tx.with_compression_info(compression_ratio, compressed_size);
            self.execute_transaction(&tx)?;
        }
        if compression_infos.next().is_some() {
            return Err(BlockExecutionError::msg(
                "more compression infos than transactions",
            ));
        }

        self.apply_post_execution_changes()
    }
}

impl<DB, E, R, Spec> ScrollBlockExecutor<E, R, Spec>
where
    DB: StateDB,
    E: EvmExt<DB = DB>,
    R: ScrollReceiptBuilder,
    Spec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    /// Checks the Feynman+ base-fee rule for the block that was just executed.
    ///
    /// The rule is `header.base_fee == next_block_base_fee(parent, overhead)`, where the
    /// overhead comes from slot 101 of the system config contract in this block's pre-state
    /// (read in [`BlockExecutor::apply_pre_execution_changes`]).
    ///
    /// It runs at the end of execution rather than at the start for two reasons. A block
    /// cannot be imported without reaching this point, so there is no import path that can
    /// skip the rule; and the paths that stop after pre-execution — tracing, simulation,
    /// access-list building — replay an already-validated block and have no parent to offer.
    ///
    /// A Feynman+ block executed to completion without parent inputs is an error, not a
    /// skipped check.
    fn validate_base_fee(&self) -> Result<(), BlockExecutionError> {
        let timestamp = self.evm.block().timestamp().to();
        if !self
            .spec
            .dogeos_fork_activation(DogeosHardfork::Feynman)
            .active_at_timestamp(timestamp)
        {
            return Ok(());
        }

        let Some(overhead) = self.base_fee_overhead else {
            return Err(BlockExecutionError::msg(
                "base-fee overhead was not read before execution",
            ));
        };
        let Some(parent) = self.ctx.base_fee_parent else {
            return Err(BlockExecutionError::msg(
                "cannot check the base fee: the caller supplied no parent header",
            ));
        };

        let expected =
            crate::next_block_base_fee_with_overhead(&self.spec, &parent, overhead, timestamp);
        let got = self.evm.block().basefee();
        if got != expected {
            return Err(BlockExecutionError::msg(format!(
                "invalid base fee: got {got}, expected {expected}"
            )));
        }
        Ok(())
    }
}

impl<DB, E, R, Spec> BlockExecutor for ScrollBlockExecutor<E, R, Spec>
where
    DB: StateDB,
    E: EvmExt<DB = DB, Tx: FromRecoveredTx<R::Transaction> + FromTxWithEncoded<R::Transaction>>,
    R: ScrollReceiptBuilder<Transaction: Transaction + Encodable2718, Receipt: TxReceipt>,
    Spec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    type Transaction = R::Transaction;
    type Receipt = R::Receipt;
    type Evm = E;
    type Result = ScrollTxResult<<E as Evm>::HaltReason>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        // Read the L2 base-fee overhead from the block's pre-state, before any transition can
        // touch it. The read is pure, and it happens here so that an execution witness taken
        // for this block carries the system config account and its slot 101 — the base-fee
        // rule is checked again, against the same value, in post-execution.
        let Self { spec, evm, .. } = self;
        let overhead = crate::base_fee_overhead(spec, evm.db_mut()).map_err(|err| {
            BlockExecutionError::msg(format!("failed to read the base-fee overhead: {err}"))
        })?;
        self.base_fee_overhead = Some(overhead);

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
        self.validate_base_fee()?;
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
    Spec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig> + Clone,
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
    use crate::next_block_base_fee_with_overhead;
    use crate::{
        ScrollRethReceiptBuilder, ScrollTransactionIntoTxEnv, TX_L1_FEE_PRECISION_U256,
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

    /// Mainnet at genesis runs the Tsuki fee params (denominator 48, elasticity 10), so a
    /// parent that burned exactly the gas target keeps the part of its fee above the overhead
    /// and the child's base fee equals the parent's.
    const TEST_GAS_LIMIT: u64 = 30_000_000;
    const TEST_BASE_FEE: u64 = 15_680_000 + 1_000_000_000;
    const FIXTURE_SENDER: Address = Address::repeat_byte(0x55);

    /// An execution context whose parent satisfies the base-fee rule for [`TEST_BASE_FEE`].
    fn test_ctx() -> ScrollBlockExecutionCtx {
        ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: Some(ScrollBaseFeeParent {
                base_fee_per_gas: TEST_BASE_FEE,
                gas_used: TEST_GAS_LIMIT / 10,
                gas_limit: TEST_GAS_LIMIT,
            }),
        }
    }

    fn empty_state_evm()
    -> impl EvmExt<DB = State<EmptyDB>, Tx = ScrollTransactionIntoTxEnv<TxEnv>, HaltReason = HaltReason>
    {
        empty_state_evm_with_base_fee(TEST_BASE_FEE)
    }

    fn empty_state_evm_with_base_fee(
        base_fee: u64,
    ) -> impl EvmExt<DB = State<EmptyDB>, Tx = ScrollTransactionIntoTxEnv<TxEnv>, HaltReason = HaltReason>
    {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        // The fixture transactions pay the block's base fee, so their sender needs a balance.
        state.insert_account(
            FIXTURE_SENDER,
            AccountInfo {
                balance: U256::from(u128::from(TEST_BASE_FEE) * 1_000_000),
                ..Default::default()
            },
        );
        ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state,
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                BlockEnv {
                    basefee: base_fee,
                    gas_limit: TEST_GAS_LIMIT,
                    ..Default::default()
                },
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

    fn execute_with_compression_infos(
        tx_count: u64,
        info_count: usize,
    ) -> Result<BlockExecutionResult<dogeos_reth_primitives::ScrollReceipt>, BlockExecutionError>
    {
        use alloy_consensus::{Signed, TxLegacy, transaction::Recovered};
        use alloy_primitives::{Signature, TxKind};
        use dogeos_reth_primitives::ScrollTransactionSigned;

        let sender = FIXTURE_SENDER;
        let transactions = (0..tx_count)
            .map(|nonce| {
                let tx = TxLegacy {
                    nonce,
                    gas_price: TEST_BASE_FEE.into(),
                    gas_limit: 21_000,
                    to: TxKind::Call(Address::repeat_byte(0x66)),
                    ..Default::default()
                };
                let signed = Signed::new_unchecked(tx, Signature::test_signature(), B256::ZERO);
                ScrollTransactionSigned::from(signed)
            })
            .collect::<Vec<_>>();
        let executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            test_ctx(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        executor.execute_block_with_compression_cache(
            transactions
                .iter()
                .map(|tx| Recovered::new_unchecked(tx, sender)),
            vec![(TX_L1_FEE_PRECISION_U256, 1); info_count],
        )
    }

    #[test]
    fn compression_cache_executes_every_transaction_when_counts_match() {
        let result = execute_with_compression_infos(3, 3).unwrap();
        assert_eq!(result.receipts.len(), 3);
    }

    fn no_transactions() -> core::iter::Empty<
        alloy_consensus::transaction::Recovered<
            &'static dogeos_reth_primitives::ScrollTransactionSigned,
        >,
    > {
        core::iter::empty()
    }

    /// Runs a transaction-free block with `ctx` and whatever slot-101 value `overhead` gives.
    fn execute_empty_block(
        ctx: ScrollBlockExecutionCtx,
        overhead: Option<u64>,
    ) -> Result<BlockExecutionResult<dogeos_reth_primitives::ScrollReceipt>, BlockExecutionError>
    {
        execute_with_overhead(ctx, TEST_BASE_FEE, overhead)
    }

    #[test]
    fn correct_base_fee_is_accepted() {
        assert!(execute_empty_block(test_ctx(), None).is_ok());
    }

    #[test]
    fn wrong_base_fee_is_rejected() {
        // One wei off the rule's output on either side.
        for delta in [-1i64, 1] {
            let mut ctx = test_ctx();
            let parent = ctx.base_fee_parent.as_mut().unwrap();
            parent.base_fee_per_gas = parent.base_fee_per_gas.wrapping_add_signed(delta);
            let err = execute_empty_block(ctx, None).unwrap_err().to_string();
            assert!(
                err.starts_with("invalid base fee: got "),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn a_block_without_parent_inputs_is_rejected() {
        let ctx = ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: None,
        };
        assert_eq!(
            execute_empty_block(ctx, None).unwrap_err().to_string(),
            "cannot check the base fee: the caller supplied no parent header"
        );
    }

    /// A parent that burned a full block, so the part of its fee above the overhead grows and
    /// the overhead actually matters to the result.
    fn full_block_parent() -> ScrollBaseFeeParent {
        ScrollBaseFeeParent {
            base_fee_per_gas: DEFAULT_OVERHEAD + 1_000_000_000,
            gas_used: TEST_GAS_LIMIT,
            gas_limit: TEST_GAS_LIMIT,
        }
    }

    const DEFAULT_OVERHEAD: u64 = 15_680_000;
    /// 1 gwei above the default overhead, grown by 9/48 over a full block.
    const FULL_BLOCK_CHILD_BASE_FEE: u64 = DEFAULT_OVERHEAD + 1_187_500_000;

    #[test]
    fn an_unset_slot_means_the_default_overhead() {
        let ctx = ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: Some(full_block_parent()),
        };
        let executor = ScrollBlockExecutor::new(
            empty_state_evm_with_base_fee(FULL_BLOCK_CHILD_BASE_FEE),
            ctx,
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        assert!(executor.execute_block(no_transactions()).is_ok());
    }

    #[test]
    fn a_configured_overhead_replaces_the_default() {
        // One wei more overhead leaves one wei less below it to grow, so the child's base fee
        // is one wei lower than under the default.
        let configured = DEFAULT_OVERHEAD + 1;
        let expected = FULL_BLOCK_CHILD_BASE_FEE - 1;

        let ctx = ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: Some(full_block_parent()),
        };
        assert!(
            execute_with_overhead(ctx.clone(), expected, Some(configured)).is_ok(),
            "the configured overhead was not used"
        );

        // The header that the default overhead would have produced is now wrong.
        let err = execute_with_overhead(ctx, FULL_BLOCK_CHILD_BASE_FEE, Some(configured))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            format!("invalid base fee: got {FULL_BLOCK_CHILD_BASE_FEE}, expected {expected}")
        );
    }

    fn execute_with_overhead(
        ctx: ScrollBlockExecutionCtx,
        base_fee: u64,
        overhead: Option<u64>,
    ) -> Result<BlockExecutionResult<dogeos_reth_primitives::ScrollReceipt>, BlockExecutionError>
    {
        let mut evm = empty_state_evm_with_base_fee(base_fee);
        if let Some(overhead) = overhead {
            set_overhead(evm.db_mut(), overhead);
        }
        ScrollBlockExecutor::new(evm, ctx, DOGEOS_MAINNET.clone(), ScrollRethReceiptBuilder)
            .execute_block(no_transactions())
    }

    fn set_overhead(db: &mut State<EmptyDB>, overhead: u64) {
        db.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            revm::database::states::plain_account::PlainStorage::from_iter([(
                U256::from(101),
                U256::from(overhead),
            )]),
        );
    }

    /// `SSTORE(101, calldata[0:32]); STOP` — stands in for the system config's overhead setter.
    const OVERHEAD_SETTER_CODE: [u8; 7] = [0x60, 0x00, 0x35, 0x60, 0x65, 0x55, 0x00];

    #[test]
    fn an_overhead_change_applies_from_the_next_block() {
        use alloy_consensus::{Signed, TxLegacy, transaction::Recovered};
        use alloy_primitives::{Signature, TxKind};
        use dogeos_reth_primitives::ScrollTransactionSigned;

        const OLD_OVERHEAD: u64 = 100_000_000;
        const NEW_OVERHEAD: u64 = 200_000_000;
        // A full parent block one gwei above the old overhead: the excess grows by 9/48.
        let parent = ScrollBaseFeeParent {
            base_fee_per_gas: OLD_OVERHEAD + 1_000_000_000,
            gas_used: TEST_GAS_LIMIT,
            gas_limit: TEST_GAS_LIMIT,
        };
        let under_old = OLD_OVERHEAD + 1_187_500_000;
        let under_new =
            next_block_base_fee_with_overhead(&**DOGEOS_MAINNET, &parent, NEW_OVERHEAD, 0);
        assert_ne!(under_old, under_new);

        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account(
            FIXTURE_SENDER,
            AccountInfo {
                balance: U256::from(10u64).pow(U256::from(18u64)),
                ..Default::default()
            },
        );
        state.insert_account_with_storage(
            system_config,
            AccountInfo::from_bytecode(revm::bytecode::Bytecode::new_raw(
                OVERHEAD_SETTER_CODE.into(),
            )),
            revm::database::states::plain_account::PlainStorage::from_iter([(
                U256::from(101),
                U256::from(OLD_OVERHEAD),
            )]),
        );

        // Block n sets the new overhead, and is itself judged by the old one.
        let set_new_overhead = {
            let tx = TxLegacy {
                nonce: 0,
                gas_price: under_old.into(),
                gas_limit: 100_000,
                to: TxKind::Call(system_config),
                input: U256::from(NEW_OVERHEAD).to_be_bytes::<32>().into(),
                ..Default::default()
            };
            ScrollTransactionSigned::from(Signed::new_unchecked(
                tx,
                Signature::test_signature(),
                B256::ZERO,
            ))
        };
        let ctx = ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: Some(parent),
        };
        let result = ScrollBlockExecutor::new(
            evm_over(&mut state, under_old),
            ctx,
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        )
        .execute_block(core::iter::once(Recovered::new_unchecked(
            &set_new_overhead,
            FIXTURE_SENDER,
        )))
        .expect("block n follows the overhead of its own pre-state");

        assert_eq!(
            crate::base_fee_overhead(&**DOGEOS_MAINNET, &mut state).expect("storage read"),
            NEW_OVERHEAD,
            "block n did not change the overhead"
        );

        // Block n + 1 is judged by the overhead block n left behind.
        let parent_n = ScrollBaseFeeParent {
            base_fee_per_gas: under_old,
            gas_used: result.gas_used,
            gas_limit: TEST_GAS_LIMIT,
        };
        let stale =
            next_block_base_fee_with_overhead(&**DOGEOS_MAINNET, &parent_n, OLD_OVERHEAD, 0);
        let fresh =
            next_block_base_fee_with_overhead(&**DOGEOS_MAINNET, &parent_n, NEW_OVERHEAD, 0);
        assert_ne!(stale, fresh);

        let ctx = ScrollBlockExecutionCtx {
            parent_hash: B256::ZERO,
            base_fee_parent: Some(parent_n),
        };
        let err = ScrollBlockExecutor::new(
            evm_over(&mut state, stale),
            ctx.clone(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        )
        .execute_block(no_transactions())
        .unwrap_err()
        .to_string();
        assert_eq!(
            err,
            format!("invalid base fee: got {stale}, expected {fresh}")
        );

        ScrollBlockExecutor::new(
            evm_over(&mut state, fresh),
            ctx,
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        )
        .execute_block(no_transactions())
        .expect("block n + 1 follows the overhead block n set");
    }

    fn evm_over(
        state: &mut State<EmptyDB>,
        base_fee: u64,
    ) -> impl EvmExt<
        DB = &mut State<EmptyDB>,
        Tx = ScrollTransactionIntoTxEnv<TxEnv>,
        HaltReason = HaltReason,
    > {
        ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            state,
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                BlockEnv {
                    basefee: base_fee,
                    gas_limit: TEST_GAS_LIMIT,
                    ..Default::default()
                },
            ),
        )
    }

    #[test]
    fn pre_execution_reads_the_system_config_slot() {
        let mut executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            test_ctx(),
            DOGEOS_MAINNET.clone(),
            ScrollRethReceiptBuilder,
        );
        executor.apply_pre_execution_changes().unwrap();

        // The read loaded the system config account and slot 101 into the state the execution
        // witness is recorded from, and left them unchanged.
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let account = executor
            .evm_mut()
            .db_mut()
            .cache
            .accounts
            .get(&system_config)
            .expect("system config account is in the state cache");
        assert!(
            account
                .account
                .as_ref()
                .is_some_and(|account| account.storage.contains_key(&U256::from(101)))
                || account.account.is_none(),
            "slot 101 was not read"
        );
    }

    #[test]
    fn compression_cache_rejects_mismatched_counts() {
        for (info_count, expected_error) in [
            (2, "fewer compression infos than transactions"),
            (4, "more compression infos than transactions"),
        ] {
            let err = execute_with_compression_infos(3, info_count).unwrap_err();
            assert_eq!(err.to_string(), expected_error);
        }
    }

    #[test]
    fn executor_forwards_transaction_state_hooks_with_receipt_indices() {
        let mut executor = ScrollBlockExecutor::new(
            empty_state_evm(),
            test_ctx(),
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
            test_ctx(),
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
            test_ctx(),
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
