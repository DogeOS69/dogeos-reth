use crate::{
    ScrollBaseFeeRule, ScrollBlockAssembler, ScrollBlockExecutionCtx, ScrollBlockExecutorFactory,
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
        Ok(ScrollBlockExecutionCtx::new(
            payload.parent_hash(),
            ScrollBaseFeeRule::missing_parent(),
        ))
    }

    fn context_for_payload_with_parent(
        &self,
        payload: &ExecutionData,
        parent: &SealedHeader<Header>,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        Ok(ScrollBlockExecutionCtx::new(
            payload.parent_hash(),
            base_fee_rule_for(payload.parent_hash(), parent),
        ))
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
        Ok(ScrollBlockExecutionCtx::new(
            block.header().parent_hash(),
            ScrollBaseFeeRule::missing_parent(),
        ))
    }

    fn context_for_block_with_parent(
        &self,
        block: &SealedBlock<Block<N::SignedTx>>,
        parent: &SealedHeader<Header>,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        Ok(ScrollBlockExecutionCtx::new(
            block.header().parent_hash(),
            base_fee_rule_for(block.header().parent_hash(), parent),
        ))
    }

    fn context_for_next_block(
        &self,
        parent: &SealedHeader<Header>,
        _attributes: Self::NextBlockEnvCtx,
    ) -> Result<ScrollBlockExecutionCtx, Self::Error> {
        // Building, not validating: there is no header whose base fee could be wrong. The
        // caller's attributes carry the fee, and the builder derived it from this very parent.
        Ok(ScrollBlockExecutionCtx::new(
            parent.hash(),
            ScrollBaseFeeRule::no_header_to_check(),
        ))
    }
}

/// Builds the base-fee rule from a caller-supplied parent, but only from the real parent.
///
/// The hash check keeps a caller that offers the wrong header from silently validating the base
/// fee against it; the executor then fails closed on the missing parent.
fn base_fee_rule_for(parent_hash: B256, parent: &SealedHeader<Header>) -> ScrollBaseFeeRule {
    if parent.hash() != parent_hash {
        return ScrollBaseFeeRule::missing_parent();
    }
    ScrollBaseFeeRule::for_parent_header(parent.header())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScrollBaseFeeParent;
    use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1};
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_MAINNET};

    #[test]
    fn supported_mainnet_selects_tsuki_at_genesis() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        assert_eq!(
            config.spec_id_at_timestamp_and_number(0, 0),
            ScrollSpecId::TSUKI
        );
    }

    #[test]
    fn chikyu_checkpoint_selects_feynman_without_later_forks() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_CHIKYU.clone());
        assert_eq!(
            config.spec_id_at_timestamp_and_number(1_785_892_970, 6_777_906),
            ScrollSpecId::FEYNMAN
        );
    }

    fn feynman_parent() -> SealedHeader<Header> {
        SealedHeader::seal_slow(Header {
            number: 6,
            gas_limit: 30_000_000,
            gas_used: 3_000_000,
            base_fee_per_gas: Some(15_680_000),
            ..Default::default()
        })
    }

    #[test]
    fn a_block_context_carries_the_parent_fee_inputs() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent = feynman_parent();
        let block = SealedBlock::seal_slow(Block::new(
            Header {
                number: 7,
                parent_hash: parent.hash(),
                ..Default::default()
            },
            Default::default(),
        ));

        // Without the parent there are no inputs, and block execution fails closed.
        assert_eq!(
            config.context_for_block(&block).unwrap().base_fee,
            ScrollBaseFeeRule::missing_parent()
        );

        let ctx = config
            .context_for_block_with_parent(&block, &parent)
            .unwrap();
        assert_eq!(
            ctx.base_fee,
            ScrollBaseFeeRule::check(ScrollBaseFeeParent {
                base_fee_per_gas: 15_680_000,
                gas_used: 3_000_000,
                gas_limit: 30_000_000,
            })
        );

        // A header that is not this block's parent is refused rather than believed.
        let impostor = SealedHeader::seal_slow(Header {
            number: 6,
            gas_limit: 30_000_000,
            gas_used: 30_000_000,
            base_fee_per_gas: Some(99_000_000),
            ..Default::default()
        });
        assert_eq!(
            config
                .context_for_block_with_parent(&block, &impostor)
                .unwrap()
                .base_fee,
            ScrollBaseFeeRule::missing_parent()
        );
    }

    /// Building a block is not validating one, so the rule does not run: the payload builder,
    /// the RPC pending block and `eth_simulateV1` all set the base fee themselves, and
    /// simulation is free to zero it or override it. No import path reaches this constructor —
    /// importers start from a block or a payload.
    #[test]
    fn a_next_block_context_has_no_header_to_check() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let attributes = ScrollNextBlockEnvAttributes {
            timestamp: 1,
            suggested_fee_recipient: Address::ZERO,
            gas_limit: 30_000_000,
            base_fee: 0,
        };
        assert_eq!(
            config
                .context_for_next_block(&feynman_parent(), attributes)
                .unwrap()
                .base_fee,
            ScrollBaseFeeRule::no_header_to_check()
        );
    }

    /// Seeding the executor with the parent of the first block and then letting it track the
    /// parent itself is what covers staged sync (a contiguous range) and the single-block
    /// re-execution behind `debug_executionWitness`.
    #[test]
    fn a_seeded_executor_checks_a_whole_range() {
        use reth_evm::execute::Executor;
        use reth_primitives_traits::RecoveredBlock;
        use revm::database::EmptyDB;

        const GAS_LIMIT: u64 = 30_000_000;
        const OVERHEAD: u64 = 15_680_000;

        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let fee_after = |parent: &SealedHeader<Header>| {
            crate::next_block_base_fee_with_overhead(
                &**DOGEOS_MAINNET,
                &ScrollBaseFeeParent::from_header(parent.header()).unwrap(),
                OVERHEAD,
                parent.timestamp() + 1,
            )
        };
        let empty_block = |number: u64, parent: &SealedHeader<Header>, base_fee: u64| {
            RecoveredBlock::new_sealed(
                SealedBlock::seal_slow(Block::new(
                    Header {
                        number,
                        parent_hash: parent.hash(),
                        gas_limit: GAS_LIMIT,
                        gas_used: 0,
                        timestamp: parent.timestamp() + 1,
                        base_fee_per_gas: Some(base_fee),
                        ..Default::default()
                    },
                    Default::default(),
                )),
                Vec::new(),
            )
        };

        // A full genesis block, so each later base fee differs from the last.
        let genesis = SealedHeader::seal_slow(Header {
            number: 0,
            gas_limit: GAS_LIMIT,
            gas_used: GAS_LIMIT,
            base_fee_per_gas: Some(OVERHEAD + 1_000_000_000),
            ..Default::default()
        });
        let first = empty_block(1, &genesis, fee_after(&genesis));
        let second = empty_block(2, first.sealed_header(), fee_after(first.sealed_header()));
        assert_ne!(
            first.header().base_fee_per_gas,
            second.header().base_fee_per_gas
        );

        // Seeded with the genesis header, the executor checks both blocks: the second one
        // against the first, which it tracked itself.
        let mut executor = config.executor_with_parent(EmptyDB::default(), Some(genesis.clone()));
        executor.execute_one(&first).expect("first block");
        executor.execute_one(&second).expect("second block");

        // The second block's fee is judged against the first, not against the seed.
        let wrong = empty_block(2, first.sealed_header(), fee_after(&genesis));
        let mut executor = config.executor_with_parent(EmptyDB::default(), Some(genesis.clone()));
        executor.execute_one(&first).expect("first block");
        let err = executor.execute_one(&wrong).unwrap_err().to_string();
        assert!(
            err.starts_with("invalid base fee: got "),
            "unexpected error: {err}"
        );

        // A gap in the range leaves no parent, and the block fails closed.
        let mut executor = config.executor_with_parent(EmptyDB::default(), Some(genesis));
        assert_eq!(
            executor.execute_one(&second).unwrap_err().to_string(),
            "cannot check the base fee: the caller supplied no parent header"
        );

        // So does an executor nobody seeded.
        let mut executor = config.batch_executor(EmptyDB::default());
        assert_eq!(
            executor.execute_one(&first).unwrap_err().to_string(),
            "cannot check the base fee: the caller supplied no parent header"
        );
    }

    /// The state-hook variant tracks the parent the same way; the engine tree's prewarming and
    /// state-root work go through it, so a divergence here would check a different block.
    #[test]
    fn the_state_hook_variant_tracks_the_parent_too() {
        use reth_evm::execute::Executor;
        use reth_primitives_traits::RecoveredBlock;
        use revm::database::EmptyDB;

        const GAS_LIMIT: u64 = 30_000_000;
        const OVERHEAD: u64 = 15_680_000;

        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let genesis = SealedHeader::seal_slow(Header {
            number: 0,
            gas_limit: GAS_LIMIT,
            gas_used: GAS_LIMIT,
            base_fee_per_gas: Some(OVERHEAD + 1_000_000_000),
            ..Default::default()
        });
        let base_fee = crate::next_block_base_fee_with_overhead(
            &**DOGEOS_MAINNET,
            &ScrollBaseFeeParent::from_header(genesis.header()).unwrap(),
            OVERHEAD,
            genesis.timestamp() + 1,
        );
        let block = |base_fee| {
            RecoveredBlock::new_sealed(
                SealedBlock::seal_slow(Block::new(
                    Header {
                        number: 1,
                        parent_hash: genesis.hash(),
                        gas_limit: GAS_LIMIT,
                        timestamp: genesis.timestamp() + 1,
                        base_fee_per_gas: Some(base_fee),
                        ..Default::default()
                    },
                    Default::default(),
                )),
                Vec::new(),
            )
        };
        let no_hook = |_: alloy_evm::block::StateChangeSource, _: &revm::state::EvmState| {};

        let mut executor = config.executor_with_parent(EmptyDB::default(), Some(genesis.clone()));
        executor
            .execute_one_with_state_hook(&block(base_fee), no_hook)
            .expect("seeded with the parent");

        let mut executor = config.executor_with_parent(EmptyDB::default(), Some(genesis.clone()));
        let err = executor
            .execute_one_with_state_hook(&block(base_fee + 1), no_hook)
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("invalid base fee: got "),
            "unexpected error: {err}"
        );

        let mut executor = config.batch_executor(EmptyDB::default());
        assert_eq!(
            executor
                .execute_one_with_state_hook(&block(base_fee), no_hook)
                .unwrap_err()
                .to_string(),
            "cannot check the base fee: the caller supplied no parent header"
        );
    }

    /// `newPayload` is the import path for both the Engine API and `scroll/1` gossip, and the
    /// parent it is given is whatever the engine tree resolved — commonly a block that has been
    /// `newPayload`-ed but not yet made canonical, so it exists only in the tree's own state and
    /// never on disk. The context does not care where the header came from, only that it is the
    /// payload's parent.
    #[test]
    fn a_payload_context_carries_the_parent_fee_inputs() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent = feynman_parent();
        let payload = |parent_hash| {
            ExecutionData::new(
                ExecutionPayload::V1(ExecutionPayloadV1 {
                    parent_hash,
                    fee_recipient: Address::ZERO,
                    state_root: B256::ZERO,
                    receipts_root: B256::ZERO,
                    logs_bloom: alloy_primitives::Bloom::ZERO,
                    prev_randao: B256::ZERO,
                    block_number: 7,
                    gas_limit: 30_000_000,
                    gas_used: 0,
                    timestamp: 1,
                    extra_data: Default::default(),
                    base_fee_per_gas: U256::from(15_680_000),
                    block_hash: B256::ZERO,
                    transactions: Vec::new(),
                }),
                ExecutionPayloadSidecar::none(),
            )
        };

        assert_eq!(
            config
                .context_for_payload_with_parent(&payload(parent.hash()), &parent)
                .unwrap()
                .base_fee,
            ScrollBaseFeeRule::check(ScrollBaseFeeParent {
                base_fee_per_gas: 15_680_000,
                gas_used: 3_000_000,
                gas_limit: 30_000_000,
            })
        );

        // Without a parent, and with a header that is not this payload's parent, it fails closed.
        assert_eq!(
            config
                .context_for_payload(&payload(parent.hash()))
                .unwrap()
                .base_fee,
            ScrollBaseFeeRule::missing_parent()
        );
        assert_eq!(
            config
                .context_for_payload_with_parent(&payload(B256::repeat_byte(0x99)), &parent)
                .unwrap()
                .base_fee,
            ScrollBaseFeeRule::missing_parent()
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
