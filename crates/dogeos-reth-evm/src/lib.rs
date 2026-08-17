//! DogeOS EVM factory and transaction adapters.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

mod tx;
pub use tx::{
    FromTxWithCompressionInfo, ScrollTransactionIntoTxEnv, ToTxWithCompressionInfo,
    WithCompressionInfo, compute_compressed_size, compute_compression_ratio,
};

extern crate alloc;

mod base_fee;
pub mod gas_price_oracle;
pub use base_fee::{
    BASE_FEE_FLOOR, BaseFeeError, DEFAULT_BASE_FEE_OVERHEAD, DYNAMIC_BASE_FEE_GAS_TARGET,
    DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR, DynamicBaseFeeError, DynamicBaseFeeState,
    INITIAL_CONTROLLED_BASE_FEE, L1_BASE_FEE_PRECISION, LEGACY_MAX_L2_BASE_FEE, MAX_L2_BASE_FEE,
    NEXT_CONTROLLED_BASE_FEE_SLOT, NEXT_CONTROLLED_BASE_FEE_SLOT_NAMESPACE, ScrollBaseFeeProvider,
    calculate_next_controlled_base_fee, predict_next_payload_timestamp,
};
mod l1;
pub use l1::RethL1BlockInfo;
mod protocol_storage;
pub use protocol_storage::derive_protocol_storage_slot;
mod spec;
pub use spec::spec_id_at_timestamp_and_number;
mod system_caller;
pub mod transitions;
mod witness_state;
pub use witness_state::{
    L2_MESSAGE_QUEUE_ADDRESS, LoadMessageQueueWitnessState, NEXT_MESSAGE_INDEX_SLOT,
    WITHDRAW_TRIE_ROOT_SLOT,
};

mod block;
pub use block::{
    EvmExt, ReceiptBuilderCtx, ScrollBlockExecutionCtx, ScrollBlockExecutor,
    ScrollBlockExecutorFactory, ScrollReceiptBuilder, ScrollTxCompressionInfo,
    ScrollTxCompressionInfos,
};

#[cfg(feature = "std")]
mod build;
#[cfg(feature = "std")]
pub use build::ScrollBlockAssembler;
#[cfg(feature = "std")]
mod config;
#[cfg(feature = "std")]
pub use config::{ScrollEvmConfig, ScrollNextBlockEnvAttributes};
#[cfg(feature = "std")]
mod receipt;
#[cfg(feature = "std")]
pub use receipt::ScrollRethReceiptBuilder;

use alloy_evm::{Database, Evm, EvmEnv, EvmFactory, precompiles::PrecompilesMap};
use alloy_primitives::{Address, Bytes};
use core::{
    fmt,
    ops::{Deref, DerefMut},
};
use revm::{
    Context, ExecuteEvm, InspectEvm, Inspector, SystemCallEvm,
    context::{BlockEnv, TxEnv, result::HaltReason},
    context_interface::result::{EVMError, ResultAndState},
    handler::PrecompileProvider,
    inspector::NoOpInspector,
    interpreter::{InterpreterResult, interpreter::EthInterpreter},
};
use revm_scroll::{
    ScrollSpecId,
    builder::{DefaultScrollContext, ScrollBuilder, ScrollCfgExt, ScrollContext},
    instructions::ScrollInstructions,
    precompile::ScrollPrecompileProvider,
};

/// Re-export `TX_L1_FEE_PRECISION_U256` from `revm-scroll` for convenience.
pub use revm_scroll::l1block::TX_L1_FEE_PRECISION_U256;

/// Scroll EVM implementation.
#[allow(missing_debug_implementations)]
pub struct ScrollEvm<DB: Database, I, P = ScrollPrecompileProvider> {
    inner: revm_scroll::ScrollEvm<
        ScrollContext<DB>,
        I,
        ScrollInstructions<EthInterpreter, ScrollContext<DB>>,
        P,
    >,
    inspect: bool,
}

impl<DB: Database, I, P> ScrollEvm<DB, I, P> {
    /// Creates a new instance of [`ScrollEvm`].
    pub const fn new(
        inner: revm_scroll::ScrollEvm<
            ScrollContext<DB>,
            I,
            ScrollInstructions<EthInterpreter, ScrollContext<DB>>,
            P,
        >,
        inspect: bool,
    ) -> Self {
        Self { inner, inspect }
    }

    /// Provides a reference to the EVM context.
    pub const fn ctx(&self) -> &ScrollContext<DB> {
        &self.inner.0.ctx
    }

    /// Provides a mutable reference to the EVM context.
    pub const fn ctx_mut(&mut self) -> &mut ScrollContext<DB> {
        &mut self.inner.0.ctx
    }
}

impl<DB: Database, I, P> Deref for ScrollEvm<DB, I, P> {
    type Target = ScrollContext<DB>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.ctx()
    }
}

impl<DB: Database, I, P> DerefMut for ScrollEvm<DB, I, P> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx_mut()
    }
}

impl<DB, I, P> Evm for ScrollEvm<DB, I, P>
where
    DB: Database,
    I: Inspector<ScrollContext<DB>>,
    P: PrecompileProvider<ScrollContext<DB>, Output = InterpreterResult>,
{
    type DB = DB;
    type Tx = ScrollTransactionIntoTxEnv<TxEnv>;
    type Error = EVMError<DB::Error>;
    type HaltReason = HaltReason;
    type Spec = ScrollSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = P;
    type Inspector = I;

    fn block(&self) -> &Self::BlockEnv {
        &self.block
    }

    fn chain_id(&self) -> u64 {
        self.cfg.chain_id
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        if self.inspect {
            self.inner.inspect_tx(tx.into())
        } else {
            self.inner.transact(tx.into())
        }
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.inner.system_call_with_caller(caller, contract, data)
    }

    fn db_mut(&mut self) -> &mut Self::DB {
        &mut self.journaled_state.database
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec>)
    where
        Self: Sized,
    {
        let Context {
            block: block_env,
            cfg: cfg_env,
            journaled_state,
            ..
        } = self.inner.0.ctx;

        (journaled_state.database, EvmEnv { block_env, cfg_env })
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.inspect = enabled;
    }

    fn precompiles(&self) -> &Self::Precompiles {
        &self.inner.0.precompiles
    }

    fn precompiles_mut(&mut self) -> &mut Self::Precompiles {
        &mut self.inner.0.precompiles
    }

    fn inspector(&self) -> &Self::Inspector {
        &self.inner.0.inspector
    }

    fn inspector_mut(&mut self) -> &mut Self::Inspector {
        &mut self.inner.0.inspector
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        (
            &self.inner.0.ctx.journaled_state.database,
            &self.inner.0.inspector,
            &self.inner.0.precompiles,
        )
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        (
            &mut self.inner.0.ctx.journaled_state.database,
            &mut self.inner.0.inspector,
            &mut self.inner.0.precompiles,
        )
    }
}

/// Factory producing [`ScrollEvm`]s.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollEvmFactory<P = ScrollDefaultPrecompilesFactory> {
    _precompiles_factory: core::marker::PhantomData<P>,
}

impl<P: ScrollPrecompilesFactory> EvmFactory for ScrollEvmFactory<P> {
    type Evm<DB: Database, I: Inspector<ScrollContext<DB>>> = ScrollEvm<DB, I, Self::Precompiles>;
    type Context<DB: Database> = ScrollContext<DB>;
    type Tx = ScrollTransactionIntoTxEnv<TxEnv>;
    type Error<DBError: core::error::Error + Send + Sync + 'static> = EVMError<DBError>;
    type HaltReason = HaltReason;
    type Spec = ScrollSpecId;
    type BlockEnv = BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<ScrollSpecId>,
    ) -> Self::Evm<DB, NoOpInspector> {
        let mut cfg_env = input.cfg_env;
        let spec_id = cfg_env.spec;
        // `EvmEnv` can be constructed outside `ScrollEvmConfig`. Normalize the configuration at
        // the factory boundary so direct callers receive the gas parameters and transaction limits
        // associated with the requested Scroll fork as well.
        cfg_env.set_scroll_spec(spec_id);
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(cfg_env)
                .build_scroll_with_inspector(NoOpInspector {})
                .with_precompiles(P::with_spec(spec_id)),
            inspect: false,
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<ScrollSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let mut cfg_env = input.cfg_env;
        let spec_id = cfg_env.spec;
        // Keep the inspected construction path semantically identical to `create_evm`.
        cfg_env.set_scroll_spec(spec_id);
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(cfg_env)
                .build_scroll_with_inspector(inspector)
                .with_precompiles(P::with_spec(spec_id)),
            inspect: true,
        }
    }
}

/// A factory trait for creating precompiles for Scroll EVM.
pub trait ScrollPrecompilesFactory: Default + fmt::Debug {
    /// Creates a new instance of precompiles for the given Scroll specification ID.
    fn with_spec(spec: ScrollSpecId) -> PrecompilesMap;
}

/// Default implementation of the Scroll precompiles factory.
#[derive(Default, Debug, Copy, Clone)]
pub struct ScrollDefaultPrecompilesFactory;

impl ScrollPrecompilesFactory for ScrollDefaultPrecompilesFactory {
    fn with_spec(spec_id: ScrollSpecId) -> PrecompilesMap {
        ScrollPrecompileProvider::new_with_spec(spec_id).into_precompiles_map()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use revm::{context::CfgEnv, database::EmptyDB, primitives::eip7825};

    #[test]
    fn factory_normalizes_directly_constructed_scroll_cfg() {
        let evm = ScrollEvmFactory::<ScrollDefaultPrecompilesFactory>::default().create_evm(
            EmptyDB::default(),
            EvmEnv::new(
                CfgEnv::new_with_spec(ScrollSpecId::TSUKI),
                BlockEnv::default(),
            ),
        );

        assert_eq!(
            evm.ctx().cfg.tx_gas_limit_cap,
            Some(eip7825::TX_GAS_LIMIT_CAP)
        );
        assert_eq!(evm.ctx().cfg.gas_params.tx_floor_cost_per_token(), 10);
    }
}
