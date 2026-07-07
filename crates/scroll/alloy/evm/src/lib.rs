//! Alloy Evm API for Scroll.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

mod block;
pub use block::{
    curie, feynman, galileo_v2, EvmExt, ReceiptBuilderCtx, ScrollBlockExecutionCtx,
    ScrollBlockExecutor, ScrollBlockExecutorFactory, ScrollReceiptBuilder, ScrollTxCompressionInfo,
    ScrollTxCompressionInfos,
};

pub mod gas_price_oracle;

mod tx;
pub use tx::{
    compute_compressed_size, compute_compression_ratio, FromTxWithCompressionInfo,
    ScrollTransactionIntoTxEnv, ToTxWithCompressionInfo, WithCompressionInfo,
};

mod system_caller;

extern crate alloc;

use alloc::sync::Arc;
use alloy_evm::{precompiles::PrecompilesMap, Database, Evm, EvmEnv, EvmFactory};
use alloy_primitives::{Address, Bytes};
use core::{
    fmt,
    ops::{Deref, DerefMut},
};
use revm::{
    context::{result::HaltReason, BlockEnv, TxEnv},
    context_interface::result::{EVMError, ResultAndState},
    handler::PrecompileProvider,
    inspector::NoOpInspector,
    interpreter::{interpreter::EthInterpreter, InterpreterResult},
    Context, ExecuteEvm, InspectEvm, Inspector, SystemCallEvm,
};
use revm_scroll::{
    builder::{
        DefaultScrollContext, EuclidEipActivations, FeynmanEipActivations, ScrollBuilder,
        ScrollContext,
    },
    instructions::ScrollInstructions,
    precompile::ScrollPrecompileProvider,
    ScrollSpecId,
};

use reth_scroll_chainspec::{ChainConfig, ScrollChainConfig};
/// Re-export `TX_L1_FEE_PRECISION_U256` from `revm-scroll` for convenience.
pub use revm_scroll::l1block::TX_L1_FEE_PRECISION_U256;
use scroll_alloy_hardforks::ScrollHardforks;

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
        let Context { block: block_env, cfg: cfg_env, journaled_state, .. } = self.inner.0.ctx;

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
#[derive(Debug)]
#[non_exhaustive]
pub struct ScrollEvmFactory<ChainSpec, P = ScrollDefaultPrecompilesFactory> {
    chain_spec: Arc<ChainSpec>,
    _precompiles_factory: core::marker::PhantomData<P>,
}

impl<ChainSpec, P> Clone for ScrollEvmFactory<ChainSpec, P> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: self.chain_spec.clone(),
            _precompiles_factory: core::marker::PhantomData,
        }
    }
}

impl<ChainSpec, P> ScrollEvmFactory<ChainSpec, P> {
    /// Creates a new instance of [`ScrollEvmFactory`] with the given chain spec.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, _precompiles_factory: core::marker::PhantomData }
    }
}

impl<
        ChainSpec: ScrollHardforks + ChainConfig<Config = ScrollChainConfig>,
        P: ScrollPrecompilesFactory,
    > EvmFactory for ScrollEvmFactory<ChainSpec, P>
{
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
        let spec_id = input.cfg_env.spec;
        let config = self.chain_spec.chain_config();
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .maybe_with_eip_7702()
                .maybe_with_eip_7623()
                .build_scroll_with_inspector(
                    NoOpInspector {},
                    config.allowed_transfer_precompile_caller,
                )
                .with_precompiles(P::with_spec(spec_id, config)),
            inspect: false,
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<ScrollSpecId>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        let spec_id = input.cfg_env.spec;
        let config = self.chain_spec.chain_config();
        ScrollEvm {
            inner: Context::scroll()
                .with_db(db)
                .with_block(input.block_env)
                .with_cfg(input.cfg_env)
                .maybe_with_eip_7702()
                .maybe_with_eip_7623()
                .build_scroll_with_inspector(inspector, config.allowed_transfer_precompile_caller)
                .with_precompiles(P::with_spec(spec_id, config)),
            inspect: true,
        }
    }
}

/// A factory trait for creating precompiles for Scroll EVM.
pub trait ScrollPrecompilesFactory: Default + fmt::Debug {
    /// Creates a new instance of precompiles for the given Scroll specification ID.
    fn with_spec(spec: ScrollSpecId, chain_config: &ScrollChainConfig) -> PrecompilesMap;
}

/// Default implementation of the Scroll precompiles factory.
#[derive(Default, Debug, Copy, Clone)]
pub struct ScrollDefaultPrecompilesFactory;

impl ScrollPrecompilesFactory for ScrollDefaultPrecompilesFactory {
    fn with_spec(spec_id: ScrollSpecId, chain_config: &ScrollChainConfig) -> PrecompilesMap {
        ScrollPrecompileProvider::new_with_spec(
            spec_id,
            chain_config.allowed_transfer_precompile_caller,
        )
        .into_precompiles_map()
    }
}
