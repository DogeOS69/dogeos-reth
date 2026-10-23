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
use dogeos_hardforks::{DogeosHardfork, DogeosHardforks};
use dogeos_reth_primitives::{DogeosPrimitives, ScrollReceipt};
use reth_chainspec::EthChainSpec;
#[cfg(feature = "std")]
use reth_evm::{ConfigureEngineEvm, EvmEnvFor, ExecutableTxIterator};
use reth_evm::{
    ConfigureEvm, EvmEnv,
    execute::{BlockExecutionError, BlockValidationError},
};
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

    fn validate_block_pre_execution<DB: revm::Database>(
        &self,
        db: &mut DB,
        env: &EvmEnv<ScrollSpecId>,
        parent_hash: B256,
        parent: &SealedHeader<Header>,
    ) -> Result<(), BlockExecutionError> {
        let timestamp = env.block_env.timestamp.to();
        if !self
            .chain_spec()
            .dogeos_fork_activation(DogeosHardfork::Feynman)
            .active_at_timestamp_or_number(timestamp, env.block_env.number.to())
        {
            return Ok(());
        }
        let parent_inputs = crate::ScrollBaseFeeParent::from_header(parent.header())
            .filter(|_| parent.hash() == parent_hash)
            .ok_or_else(|| {
                BlockExecutionError::msg("cannot check the base fee: invalid parent header")
            })?;
        let expected = crate::ScrollBaseFeeProvider::new(self.chain_spec().clone())
            .next_block_base_fee_for_parent(db, &parent_inputs, timestamp)
            .map_err(|err| {
                BlockExecutionError::msg(format!("failed to read the base-fee overhead: {err}"))
            })?;
        let got = env.block_env.basefee;
        if got != expected {
            return Err(BlockValidationError::msg(format!(
                "invalid base fee: got {got}, expected {expected}"
            ))
            .into());
        }
        Ok(())
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
    fn chikyu_selects_the_network_fork_at_each_boundary() {
        let config = ScrollEvmConfig::dogeos(DOGEOS_CHIKYU.clone());
        for (timestamp, expected) in [
            (1_785_892_970, ScrollSpecId::FEYNMAN),
            (1_790_030_799, ScrollSpecId::FEYNMAN),
            (1_790_030_800, ScrollSpecId::GALILEO),
            (1_790_030_899, ScrollSpecId::GALILEO),
            (1_790_030_900, ScrollSpecId::TSUKI),
            (1_790_030_901, ScrollSpecId::TSUKI),
        ] {
            assert_eq!(
                config.spec_id_at_timestamp_and_number(timestamp, 6_777_906),
                expected
            );
        }
    }

    #[test]
    fn import_hook_checks_the_fee_and_parent() {
        use revm::database::EmptyDB;

        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent = SealedHeader::seal_slow(Header {
            gas_limit: 30_000_000,
            gas_used: 3_000_000,
            base_fee_per_gas: Some(1_015_680_000),
            ..Default::default()
        });
        let child = Header {
            number: 1,
            timestamp: 1,
            parent_hash: parent.hash(),
            base_fee_per_gas: parent.base_fee_per_gas,
            ..Default::default()
        };
        let mut env = config.evm_env(&child).unwrap();
        let mut db = EmptyDB::default();
        config
            .validate_block_pre_execution(&mut db, &env, parent.hash(), &parent)
            .unwrap();
        for fee in [1_015_679_999, 1_015_680_001] {
            env.block_env.basefee = fee;
            assert!(matches!(
                config.validate_block_pre_execution(&mut db, &env, parent.hash(), &parent),
                Err(BlockExecutionError::Validation(_))
            ));
        }
        assert!(matches!(
            config.validate_block_pre_execution(&mut db, &env, B256::ZERO, &parent),
            Err(BlockExecutionError::Internal(_))
        ));
        let no_fee = SealedHeader::seal_slow(Header::default());
        assert!(matches!(
            config.validate_block_pre_execution(&mut db, &env, no_fee.hash(), &no_fee),
            Err(BlockExecutionError::Internal(_))
        ));
    }

    #[test]
    fn import_validation_uses_the_batch_pre_state() {
        use alloy_consensus::{Signed, TxLegacy};
        use alloy_primitives::{Signature, TxKind};
        use dogeos_reth_primitives::ScrollTransactionSigned;
        use reth_evm::execute::Executor;
        use reth_primitives_traits::RecoveredBlock;
        use revm::{
            database::{EmptyDB, State, states::plain_account::PlainStorage},
            state::AccountInfo,
        };

        const GAS_LIMIT: u64 = 30_000_000;
        const OLD: u64 = 100_000_000;
        const NEW: u64 = 200_000_000;
        let sender = Address::repeat_byte(0x55);
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let mut db = State::builder().with_database(EmptyDB::default()).build();
        db.insert_account(
            sender,
            AccountInfo {
                balance: U256::from(10u64).pow(U256::from(18u64)),
                ..Default::default()
            },
        );
        // SSTORE(101, calldata[0:32]); STOP.
        db.insert_account_with_storage(
            system_config,
            AccountInfo::from_bytecode(revm::bytecode::Bytecode::new_raw(
                [0x60, 0x00, 0x35, 0x60, 0x65, 0x55, 0x00].into(),
            )),
            PlainStorage::from_iter([(U256::from(101), U256::from(OLD))]),
        );
        let genesis = SealedHeader::seal_slow(Header {
            gas_limit: GAS_LIMIT,
            gas_used: GAS_LIMIT,
            base_fee_per_gas: Some(OLD + 1_000_000_000),
            ..Default::default()
        });
        let fee_after = |parent: &SealedHeader<Header>, overhead| {
            crate::next_block_base_fee_with_overhead(
                &**DOGEOS_MAINNET,
                &crate::ScrollBaseFeeParent::from_header(parent.header()).unwrap(),
                overhead,
                parent.timestamp + 1,
            )
        };
        let first_fee = fee_after(&genesis, OLD);
        let tx = ScrollTransactionSigned::from(Signed::new_unchecked(
            TxLegacy {
                gas_price: first_fee.into(),
                gas_limit: 100_000,
                to: TxKind::Call(system_config),
                input: U256::from(NEW).to_be_bytes::<32>().into(),
                ..Default::default()
            },
            Signature::test_signature(),
            B256::ZERO,
        ));
        let first = RecoveredBlock::new_sealed(
            SealedBlock::seal_slow(Block::new(
                Header {
                    number: 1,
                    timestamp: 1,
                    parent_hash: genesis.hash(),
                    gas_limit: GAS_LIMIT,
                    base_fee_per_gas: Some(first_fee),
                    ..Default::default()
                },
                BlockBody {
                    transactions: vec![tx],
                    ..Default::default()
                },
            )),
            vec![sender],
        );
        let mut executor = config.batch_executor(db);
        executor
            .validate_block_pre_execution(&first, &genesis)
            .unwrap();
        let result = executor.execute_one(&first).unwrap();
        let parent = SealedHeader::seal_slow(Header {
            gas_used: result.gas_used,
            ..first.header().clone()
        });
        let fresh = fee_after(&parent, NEW);
        let stale = fee_after(&parent, OLD);
        assert_ne!(fresh, stale);
        let child = |fee| {
            RecoveredBlock::new_sealed(
                SealedBlock::seal_slow(Block::new(
                    Header {
                        number: 2,
                        timestamp: 2,
                        parent_hash: parent.hash(),
                        gas_limit: GAS_LIMIT,
                        base_fee_per_gas: Some(fee),
                        ..Default::default()
                    },
                    BlockBody::default(),
                )),
                Vec::new(),
            )
        };
        let wrong = child(stale);
        assert!(matches!(
            executor.validate_block_pre_execution(&wrong, &parent),
            Err(BlockExecutionError::Validation(_))
        ));
        let correct = child(fresh);
        executor
            .validate_block_pre_execution(&correct, &parent)
            .unwrap();
        executor.execute_one(&correct).unwrap();
        // Ordinary replay remains available without import validation or parent inputs.
        executor.execute_one(&wrong).unwrap();
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
