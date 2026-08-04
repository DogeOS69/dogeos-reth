use crate::{
    ScrollBlockAssembler, ScrollBlockExecutionCtx, ScrollBlockExecutorFactory,
    ScrollDefaultPrecompilesFactory, ScrollPrecompilesFactory, ScrollReceiptBuilder,
    ScrollRethReceiptBuilder, ScrollTransactionIntoTxEnv, spec_id_at_timestamp_and_number,
};
use alloc::sync::Arc;
use alloy_consensus::{Block, BlockBody, BlockHeader, Header};
#[cfg(feature = "std")]
use alloy_eips::Decodable2718;
use alloy_evm::{FromRecoveredTx, FromTxWithEncoded};
#[cfg(feature = "std")]
use alloy_primitives::Bytes;
use alloy_primitives::{Address, B256, BlockNumber, BlockTimestamp, U256};
#[cfg(feature = "std")]
use alloy_rpc_types_engine::ExecutionData;
use core::convert::Infallible;
use dogeos_chainspec::{ChainConfig, DogeosChainSpec, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use dogeos_reth_primitives::{DogeosPrimitives, ScrollReceipt};
use reth_chainspec::EthChainSpec;
#[cfg(feature = "std")]
use reth_evm::{ConfigureEngineEvm, EvmEnvFor, ExecutableTxIterator};
use reth_evm::{ConfigureEvm, EvmEnv};
#[cfg(feature = "std")]
use reth_primitives_traits::TxTy;
use reth_primitives_traits::{NodePrimitives, SealedBlock, SealedHeader, SignedTransaction};
#[cfg(feature = "std")]
use reth_storage_errors::any::AnyError;
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

#[cfg(feature = "std")]
impl<ChainSpec, N, R, P> ConfigureEngineEvm<ExecutionData> for ScrollEvmConfig<ChainSpec, N, R, P>
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
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error> {
        let timestamp = payload.payload.timestamp();
        let block_number = payload.payload.block_number();
        let spec_id = self.spec_id_at_timestamp_and_number(timestamp, block_number);
        let cfg_env = CfgEnv::new_scroll(spec_id).with_chain_id(self.chain_spec().chain().id());
        let beneficiary = self
            .chain_spec()
            .chain_config()
            .fee_vault_address
            .unwrap_or_else(|| payload.payload.fee_recipient());

        Ok(EvmEnv::new(
            cfg_env,
            BlockEnv {
                number: U256::from(block_number),
                beneficiary,
                timestamp: U256::from(timestamp),
                difficulty: U256::ONE,
                prevrandao: Some(B256::ZERO),
                gas_limit: payload.payload.gas_limit(),
                basefee: payload.payload.saturated_base_fee_per_gas(),
                blob_excess_gas_and_price: None,
                slot_num: 0,
            },
        ))
    }

    fn context_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        Ok(ScrollBlockExecutionCtx {
            parent_hash: payload.parent_hash(),
        })
    }

    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error> {
        let transactions = payload.payload.transactions().clone();
        let convert = |encoded: Bytes| {
            let transaction = TxTy::<Self::Primitives>::decode_2718_exact(encoded.as_ref())
                .map_err(AnyError::new)?;
            let signer = transaction.try_recover().map_err(AnyError::new)?;
            Ok::<_, AnyError>(transaction.with_signer(signer))
        };

        Ok((transactions, convert))
    }
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

    fn context_for_block(
        &self,
        block: &SealedBlock<Block<N::SignedTx>>,
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
    use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1};
    use dogeos_chainspec::DOGEOS_MAINNET;

    #[test]
    fn supported_mainnet_selects_tsuki_at_genesis() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        assert_eq!(
            config.spec_id_at_timestamp_and_number(0, 0),
            ScrollSpecId::TSUKI
        );
    }

    #[test]
    fn engine_payload_uses_scroll_block_environment() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent_hash = B256::repeat_byte(0x11);
        let payload_fee_recipient = Address::repeat_byte(0x22);
        let payload = ExecutionData::new(
            ExecutionPayload::V1(ExecutionPayloadV1 {
                parent_hash,
                fee_recipient: payload_fee_recipient,
                state_root: B256::ZERO,
                receipts_root: B256::ZERO,
                logs_bloom: alloy_primitives::Bloom::ZERO,
                prev_randao: B256::ZERO,
                block_number: 7,
                gas_limit: 30_000_000,
                gas_used: 0,
                timestamp: 9,
                extra_data: Bytes::new(),
                base_fee_per_gas: U256::from(10),
                block_hash: B256::ZERO,
                transactions: Vec::new(),
            }),
            ExecutionPayloadSidecar::default(),
        );

        let environment = config.evm_env_for_payload(&payload).unwrap();
        assert_eq!(environment.block_env.number, U256::from(7));
        assert_eq!(environment.block_env.timestamp, U256::from(9));
        assert_eq!(environment.block_env.gas_limit, 30_000_000);
        assert_eq!(environment.block_env.basefee, 10);
        assert_eq!(environment.block_env.difficulty, U256::ONE);
        assert_eq!(environment.block_env.prevrandao, Some(B256::ZERO));
        assert_eq!(
            environment.block_env.beneficiary,
            DOGEOS_MAINNET
                .chain_config()
                .fee_vault_address
                .unwrap_or(payload_fee_recipient)
        );
        assert_eq!(
            config.context_for_payload(&payload).unwrap().parent_hash,
            parent_hash
        );
    }
}
