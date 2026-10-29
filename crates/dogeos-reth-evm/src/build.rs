use crate::ScrollBlockExecutionCtx;
use alloc::sync::Arc;
use alloy_consensus::{Block, BlockBody, EMPTY_OMMER_ROOT_HASH, Header, TxReceipt, proofs};
use alloy_eips::merge::BEACON_NONCE;
use alloy_evm::block::BlockExecutorFactory;
use alloy_primitives::{Address, B256, U256, logs_bloom};
use dogeos_reth_primitives::ScrollReceipt;
use reth_evm::execute::{
    BlockAssembler, BlockAssemblerInput, BlockExecutionError, BlockValidationError,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::SignedTransaction;
use revm::context::Block as _;

/// Assembles a Feynman+ DogeOS block from executor output.
#[derive(Debug)]
pub struct ScrollBlockAssembler<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> ScrollBlockAssembler<ChainSpec> {
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec> Clone for ScrollBlockAssembler<ChainSpec> {
    fn clone(&self) -> Self {
        Self {
            chain_spec: Arc::clone(&self.chain_spec),
        }
    }
}

impl<F, ChainSpec> BlockAssembler<F> for ScrollBlockAssembler<ChainSpec>
where
    F: for<'a> BlockExecutorFactory<
            ExecutionCtx<'a> = ScrollBlockExecutionCtx,
            Transaction: SignedTransaction,
            Receipt = ScrollReceipt,
        >,
{
    type Block = Block<F::Transaction>;

    fn assemble_block(
        &self,
        input: BlockAssemblerInput<'_, '_, F>,
    ) -> Result<Self::Block, BlockExecutionError> {
        let BlockAssemblerInput {
            evm_env,
            execution_ctx: ctx,
            transactions,
            output: BlockExecutionResult {
                receipts, gas_used, ..
            },
            state_root,
            ..
        } = input;

        // Refuse a block that header validation would reject.
        let prevrandao = evm_env.block_env.prevrandao();
        if prevrandao != Some(B256::ZERO) {
            return Err(BlockValidationError::msg(format!(
                "invalid prevrandao: got {prevrandao:?}, expected zero"
            ))
            .into());
        }
        let difficulty = evm_env.block_env.difficulty();
        if difficulty != U256::ONE {
            return Err(BlockValidationError::msg(format!(
                "invalid difficulty: got {difficulty}, expected one"
            ))
            .into());
        }

        let transactions_root = proofs::calculate_transaction_root(&transactions);
        let receipts_root = ScrollReceipt::calculate_receipt_root_no_memo(receipts);
        let logs_bloom = logs_bloom(receipts.iter().flat_map(TxReceipt::logs));

        let header = Header {
            parent_hash: ctx.parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::ZERO,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: None,
            logs_bloom,
            timestamp: evm_env.block_env.timestamp().saturating_to(),
            mix_hash: B256::ZERO,
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(evm_env.block_env.basefee()),
            number: evm_env.block_env.number().saturating_to(),
            gas_limit: evm_env.block_env.gas_limit(),
            difficulty,
            gas_used: *gas_used,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            requests_hash: None,
        };

        Ok(Block {
            header,
            body: BlockBody {
                transactions,
                ommers: Default::default(),
                withdrawals: None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScrollEvmConfig, ScrollNextBlockEnvAttributes};
    use dogeos_chainspec::DOGEOS_MAINNET;
    use reth_evm::{ConfigureEvm, EvmEnvFor};
    use reth_primitives_traits::SealedHeader;
    use reth_storage_api::noop::NoopProvider;
    use revm::database::BundleState;

    fn assemble(
        edit: impl FnOnce(&mut EvmEnvFor<ScrollEvmConfig>),
    ) -> Result<Header, BlockExecutionError> {
        let config = ScrollEvmConfig::dogeos(DOGEOS_MAINNET.clone());
        let parent = SealedHeader::seal_slow(Header {
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1_000_000),
            ..Default::default()
        });
        let mut evm_env = config
            .next_evm_env(
                &parent,
                &ScrollNextBlockEnvAttributes {
                    timestamp: 1,
                    suggested_fee_recipient: Address::ZERO,
                    gas_limit: 30_000_000,
                    base_fee: 1_000_000,
                },
            )
            .unwrap();
        edit(&mut evm_env);
        let output = BlockExecutionResult::<ScrollReceipt>::default();
        let bundle_state = BundleState::default();
        let state_provider = NoopProvider::default();
        let input =
            BlockAssemblerInput::<<ScrollEvmConfig as ConfigureEvm>::BlockExecutorFactory>::new(
                evm_env,
                ScrollBlockExecutionCtx {
                    parent_hash: parent.hash(),
                },
                &parent,
                Vec::new(),
                &output,
                &bundle_state,
                &state_provider,
                B256::ZERO,
            );
        config
            .block_assembler()
            .assemble_block(input)
            .map(|block| block.header)
    }

    #[test]
    fn assembles_with_dogeos_header_constants() {
        let header = assemble(|_| {}).unwrap();
        assert_eq!(header.mix_hash, B256::ZERO);
        assert_eq!(header.difficulty, U256::ONE);
    }

    #[test]
    fn rejects_non_zero_or_missing_prevrandao() {
        for prevrandao in [Some(B256::repeat_byte(0x01)), None] {
            assert!(matches!(
                assemble(|env| env.block_env.prevrandao = prevrandao),
                Err(BlockExecutionError::Validation(_))
            ));
        }
    }

    #[test]
    fn rejects_difficulty_other_than_one() {
        for difficulty in [U256::ZERO, U256::from(2)] {
            assert!(matches!(
                assemble(|env| env.block_env.difficulty = difficulty),
                Err(BlockExecutionError::Validation(_))
            ));
        }
    }
}
