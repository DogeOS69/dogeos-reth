use crate::{
    ScrollBlockAssembler, ScrollBlockExecutionCtx, ScrollBlockExecutorFactory,
    ScrollDefaultPrecompilesFactory, ScrollPrecompilesFactory, ScrollReceiptBuilder,
    ScrollRethReceiptBuilder, ScrollTransactionIntoTxEnv,
};
use alloc::sync::Arc;
use alloy_consensus::{Block, BlockBody, BlockHeader, Header};
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded};
use alloy_primitives::{Address, B256, BlockNumber, BlockTimestamp, U256};
use core::convert::Infallible;
use dogeos_chainspec::{ChainConfig, DogeosChainSpec, ScrollChainConfig};
use dogeos_hardforks::{DogeosHardfork, DogeosHardforks};
use dogeos_reth_primitives::{DogeosPrimitives, ScrollReceipt};
use reth_chainspec::EthChainSpec;
use reth_evm::{ConfigureEvm, EvmEnv};
use reth_primitives_traits::{NodePrimitives, SealedBlock, SealedHeader, SignedTransaction};
use revm::context::{BlockEnv, CfgEnv, TxEnv};
use revm_scroll::{ScrollSpecId, builder::ScrollCfgExt};

/// Complete Reth EVM configuration for Feynman+ DogeOS chains.
#[derive(Debug)]
pub struct ScrollEvmConfig<
    ChainSpec = DogeosChainSpec,
    N: NodePrimitives = DogeosPrimitives,
    R = ScrollRethReceiptBuilder,
    P = ScrollDefaultPrecompilesFactory,
> {
    executor_factory: ScrollBlockExecutorFactory<R, Arc<ChainSpec>, P>,
    block_assembler: ScrollBlockAssembler<ChainSpec>,
    _primitives: core::marker::PhantomData<N>,
}

impl<ChainSpec, N: NodePrimitives, R: Clone, P: Clone> Clone
    for ScrollEvmConfig<ChainSpec, N, R, P>
{
    fn clone(&self) -> Self {
        Self {
            executor_factory: self.executor_factory.clone(),
            block_assembler: self.block_assembler.clone(),
            _primitives: self._primitives,
        }
    }
}

impl<ChainSpec> ScrollEvmConfig<ChainSpec>
where
    ChainSpec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    pub fn dogeos(chain_spec: Arc<ChainSpec>) -> Self {
        Self::new(chain_spec, ScrollRethReceiptBuilder)
    }
}

impl<ChainSpec, N: NodePrimitives, R, P: Default> ScrollEvmConfig<ChainSpec, N, R, P>
where
    ChainSpec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    pub fn new(chain_spec: Arc<ChainSpec>, receipt_builder: R) -> Self {
        Self {
            block_assembler: ScrollBlockAssembler::new(Arc::clone(&chain_spec)),
            executor_factory: ScrollBlockExecutorFactory::new(
                receipt_builder,
                chain_spec,
                Default::default(),
            ),
            _primitives: core::marker::PhantomData,
        }
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        self.executor_factory.spec()
    }

    pub fn spec_id_at_timestamp_and_number(
        &self,
        timestamp: BlockTimestamp,
        number: BlockNumber,
    ) -> ScrollSpecId {
        spec_id_at_timestamp_and_number(timestamp, number, self.chain_spec())
    }
}

/// Maps the retained Feynman+ hardfork schedule to `revm-scroll` specification IDs.
pub fn spec_id_at_timestamp_and_number(
    timestamp: u64,
    number: u64,
    chain_spec: impl DogeosHardforks,
) -> ScrollSpecId {
    let active = |fork| {
        chain_spec
            .dogeos_fork_activation(fork)
            .active_at_timestamp_or_number(timestamp, number)
    };
    if active(DogeosHardfork::Tsuki) {
        ScrollSpecId::TSUKI
    } else if active(DogeosHardfork::GalileoV2) || active(DogeosHardfork::Galileo) {
        ScrollSpecId::GALILEO
    } else {
        ScrollSpecId::FEYNMAN
    }
}

/// Inputs that cannot be derived from the parent header while building the next block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollNextBlockEnvAttributes {
    pub timestamp: u64,
    pub suggested_fee_recipient: Address,
    pub gas_limit: u64,
    pub base_fee: u64,
}

impl<ChainSpec, N, R, P> ConfigureEvm for ScrollEvmConfig<ChainSpec, N, R, P>
where
    ChainSpec: EthChainSpec<Header = Header>
        + ChainConfig<Config = ScrollChainConfig>
        + DogeosHardforks
        + 'static,
    N: NodePrimitives<
            Receipt = R::Receipt,
            SignedTx = R::Transaction,
            BlockHeader = Header,
            BlockBody = BlockBody<R::Transaction>,
            Block = Block<R::Transaction>,
        >,
    ScrollTransactionIntoTxEnv<TxEnv>:
        FromRecoveredTx<N::SignedTx> + FromTxWithEncoded<N::SignedTx>,
    R: ScrollReceiptBuilder<Receipt = ScrollReceipt, Transaction: SignedTransaction>,
    P: ScrollPrecompilesFactory,
    Self: Send + Sync + Unpin + Clone + 'static,
{
    type Primitives = N;
    type Error = Infallible;
    type NextBlockEnvCtx = ScrollNextBlockEnvAttributes;
    type BlockExecutorFactory = ScrollBlockExecutorFactory<R, Arc<ChainSpec>, P>;
    type BlockAssembler = ScrollBlockAssembler<ChainSpec>;

    fn block_executor_factory(&self) -> &Self::BlockExecutorFactory {
        &self.executor_factory
    }

    fn block_assembler(&self) -> &Self::BlockAssembler {
        &self.block_assembler
    }

    fn evm_env(&self, header: &Header) -> Result<EvmEnv<ScrollSpecId>, Self::Error> {
        let spec_id = self.spec_id_at_timestamp_and_number(header.timestamp(), header.number());
        let cfg_env = CfgEnv::new_scroll(spec_id).with_chain_id(self.chain_spec().chain().id());
        let beneficiary = self
            .chain_spec()
            .chain_config()
            .fee_vault_address
            .unwrap_or_else(|| header.beneficiary());
        Ok(EvmEnv::new(
            cfg_env,
            BlockEnv {
                number: U256::from(header.number()),
                beneficiary,
                timestamp: U256::from(header.timestamp()),
                gas_limit: header.gas_limit(),
                basefee: header.base_fee_per_gas().unwrap_or_default(),
                difficulty: header.difficulty(),
                prevrandao: header.mix_hash(),
                blob_excess_gas_and_price: None,
                slot_num: 0,
            },
        ))
    }

    fn next_evm_env(
        &self,
        parent: &Header,
        attributes: &Self::NextBlockEnvCtx,
    ) -> Result<EvmEnv<ScrollSpecId>, Self::Error> {
        let number = parent.number() + 1;
        let spec_id = self.spec_id_at_timestamp_and_number(attributes.timestamp, number);
        let cfg_env = CfgEnv::new_scroll(spec_id).with_chain_id(self.chain_spec().chain().id());
        let beneficiary = self
            .chain_spec()
            .chain_config()
            .fee_vault_address
            .unwrap_or(attributes.suggested_fee_recipient);
        Ok(EvmEnv::new(
            cfg_env,
            BlockEnv {
                number: U256::from(number),
                beneficiary,
                timestamp: U256::from(attributes.timestamp),
                gas_limit: attributes.gas_limit,
                basefee: attributes.base_fee,
                difficulty: U256::ONE,
                prevrandao: Some(B256::ZERO),
                blob_excess_gas_and_price: None,
                slot_num: 0,
            },
        ))
    }

    fn context_for_block<'a>(
        &self,
        block: &'a SealedBlock<Block<N::SignedTx>>,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        Ok(ScrollBlockExecutionCtx {
            parent_hash: block.header().parent_hash(),
        })
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<Header>,
        _attributes: Self::NextBlockEnvCtx,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        Ok(ScrollBlockExecutionCtx {
            parent_hash: parent.hash(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::DOGEOS_MAINNET;

    #[test]
    fn supported_mainnet_selects_tsuki_at_genesis() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        assert_eq!(
            config.spec_id_at_timestamp_and_number(0, 0),
            ScrollSpecId::TSUKI
        );
    }
}
