use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_rlp::Encodable;
use alloy_rpc_types_debug::ExecutionWitness;
use dogeos_hardforks::DogeosHardforks;
use dogeos_reth_evm::LoadMessageQueueWitnessState;
use reth_chainspec::ChainSpecProvider;
use reth_evm::{ConfigureEvm, execute::Executor};
use reth_primitives_traits::RecoveredBlock;
use reth_revm::witness::ExecutionWitnessRecord;
use reth_rpc::DebugApi;
use reth_rpc_eth_api::{RpcNodeCore, helpers::TraceExt};
use reth_rpc_eth_types::EthApiError;
use reth_storage_api::{HeaderProvider, ProviderBlock, StateProofProvider};
use std::sync::Arc;

/// Scroll-aware execution witness implementation that replaces Reth's generic debug methods.
#[derive(Clone, Debug)]
pub struct DogeosDebugWitnessApi<Eth: RpcNodeCore> {
    inner: DebugApi<Eth>,
}

impl<Eth: RpcNodeCore> DogeosDebugWitnessApi<Eth> {
    pub const fn new(inner: DebugApi<Eth>) -> Self {
        Self { inner }
    }

    pub const fn inner(&self) -> &DebugApi<Eth> {
        &self.inner
    }
}

impl<Eth> DogeosDebugWitnessApi<Eth>
where
    Eth: TraceExt,
    Eth::Provider:
        ChainSpecProvider<ChainSpec: DogeosHardforks> + HeaderProvider + StateProofProvider,
{
    pub async fn execution_witness_by_block_hash(
        &self,
        hash: B256,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let block = self
            .inner
            .eth_api()
            .recovered_block(hash.into())
            .await?
            .ok_or(EthApiError::HeaderNotFound(hash.into()))?;
        self.execution_witness_for_block(block).await
    }

    pub async fn execution_witness(
        &self,
        block: BlockNumberOrTag,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let recovered = self
            .inner
            .eth_api()
            .recovered_block(block.into())
            .await?
            .ok_or(EthApiError::HeaderNotFound(block.into()))?;
        self.execution_witness_for_block(recovered).await
    }

    pub async fn execution_witness_for_block(
        &self,
        block: Arc<RecoveredBlock<ProviderBlock<Eth::Provider>>>,
    ) -> Result<ExecutionWitness, Eth::Error> {
        let block_number = block.header().number();
        let include_next_message_index = self
            .inner
            .provider()
            .chain_spec()
            .is_tsuki_active_at_timestamp(block.timestamp());

        let (mut witness, lowest_block_number) = self
            .inner
            .eth_api()
            .spawn_with_state_at_block(block.parent_hash(), move |eth_api, mut db| {
                let mut executor = eth_api.evm_config().executor(&mut db);
                executor
                    .execute_one(&block)
                    .map_err(|error| EthApiError::Internal(error.into()))?;
                let mut state = executor.into_state();
                state
                    .load_message_queue_witness_state(include_next_message_index)
                    .map_err(EthApiError::from)?;

                let mut record = ExecutionWitnessRecord::default();
                record.record_executed_state(&state);
                let ExecutionWitnessRecord {
                    hashed_state,
                    codes,
                    keys,
                    lowest_block_number,
                } = record;
                let state = db
                    .database
                    .0
                    .witness(Default::default(), hashed_state)
                    .map_err(EthApiError::from)?;
                Ok((
                    ExecutionWitness {
                        state,
                        codes,
                        keys,
                        ..Default::default()
                    },
                    lowest_block_number,
                ))
            })
            .await?;

        let smallest = lowest_block_number.unwrap_or_else(|| block_number.saturating_sub(1));
        witness.headers = self
            .inner
            .provider()
            .headers_range(smallest..block_number)
            .map_err(EthApiError::from)?
            .into_iter()
            .map(|header| {
                let mut encoded = Vec::new();
                header.encode(&mut encoded);
                encoded.into()
            })
            .collect();
        Ok(witness)
    }
}
