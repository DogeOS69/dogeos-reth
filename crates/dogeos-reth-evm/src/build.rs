use crate::ScrollBlockExecutionCtx;
use alloc::sync::Arc;
use alloy_consensus::{Block, BlockBody, EMPTY_OMMER_ROOT_HASH, Header, TxReceipt, proofs};
use alloy_eips::merge::BEACON_NONCE;
use alloy_evm::block::BlockExecutorFactory;
use alloy_primitives::{Address, logs_bloom};
use dogeos_reth_primitives::ScrollReceipt;
use reth_evm::execute::{BlockAssembler, BlockAssemblerInput, BlockExecutionError};
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
            mix_hash: evm_env.block_env.prevrandao().unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(evm_env.block_env.basefee()),
            number: evm_env.block_env.number().saturating_to(),
            gas_limit: evm_env.block_env.gas_limit(),
            difficulty: evm_env.block_env.difficulty(),
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
