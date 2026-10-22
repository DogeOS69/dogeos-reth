use crate::DogeosCompatibleNodeTypes;
use alloy_consensus::BlockHeader;
use alloy_primitives::Address;
use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_evm::{
    ScrollBaseFeeProvider, ScrollEvmConfig, ScrollNextBlockEnvAttributes, SequencerBaseFeePolicy,
};
use dogeos_reth_primitives::ScrollReceipt;
use dogeos_reth_rpc::{DogeosRpcConverter, dogeos_rpc_converter};
use reth_chainspec::ChainSpecProvider;
use reth_node_builder::{
    FullNodeComponents,
    rpc::{EthApiBuilder, EthApiCtx},
};
use reth_primitives_traits::SealedHeader;
use reth_revm::database::StateProviderDatabase;
use reth_rpc::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{FullEthApiServer, helpers::pending_block::PendingEnvBuilder};
use reth_rpc_eth_types::{EthApiError, error::FromEvmError};
use reth_storage_api::{ReceiptProvider, StateProviderFactory};
use std::sync::{Arc, Mutex};

/// One-shot handoff from the network component builder to the RPC launch phase, where Reth makes
/// the consensus Engine handle available.
#[derive(Debug, Clone, Default)]
pub(crate) struct ScrollWireRuntime {
    manager: Arc<Mutex<Option<dogeos_scroll_wire::ScrollWireManager>>>,
}

impl ScrollWireRuntime {
    pub(crate) fn install(&self, manager: dogeos_scroll_wire::ScrollWireManager) {
        *self.manager.lock().expect("scroll-wire runtime lock") = Some(manager);
    }

    fn take(&self) -> Option<dogeos_scroll_wire::ScrollWireManager> {
        self.manager
            .lock()
            .expect("scroll-wire runtime lock")
            .take()
    }
}

/// Derives a pending environment from the canonical parent's post-state and producer policy.
#[derive(Debug, Clone)]
pub struct DogeosPendingEnvBuilder<Provider> {
    provider: Provider,
    base_fee_policy: SequencerBaseFeePolicy,
    desired_gas_limit: u64,
}

impl<Provider> DogeosPendingEnvBuilder<Provider> {
    pub const fn new(
        provider: Provider,
        base_fee_policy: SequencerBaseFeePolicy,
        desired_gas_limit: u64,
    ) -> Self {
        Self {
            provider,
            base_fee_policy,
            desired_gas_limit,
        }
    }
}

impl<Provider> PendingEnvBuilder<ScrollEvmConfig> for DogeosPendingEnvBuilder<Provider>
where
    Provider: StateProviderFactory
        + ChainSpecProvider<ChainSpec = DogeosChainSpec>
        + Clone
        + Send
        + Sync
        + Unpin
        + 'static,
{
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<ScrollNextBlockEnvAttributes, EthApiError> {
        let state = self
            .provider
            .state_by_block_hash(parent.hash())
            .map_err(EthApiError::from)?;
        let mut state = StateProviderDatabase::new(state.as_ref());
        let base_fee = ScrollBaseFeeProvider::new(self.provider.chain_spec(), self.base_fee_policy)
            .next_block_base_fee(&mut state, parent.header())
            .map_err(|error| EthApiError::EvmCustom(error.to_string()))?;

        Ok(ScrollNextBlockEnvAttributes {
            timestamp: parent.timestamp(),
            suggested_fee_recipient: parent.beneficiary(),
            gas_limit: dogeos_reth_payload::next_block_gas_limit(
                parent.gas_limit(),
                self.desired_gas_limit,
            ),
            base_fee,
        })
    }
}

/// Builds the standard Reth `eth_` API with DogeOS RPC schemas and converters.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DogeosEthApiBuilder {
    scroll_wire: ScrollWireRuntime,
    scroll_wire_signer: Option<Address>,
    base_fee_policy: SequencerBaseFeePolicy,
    desired_gas_limit: u64,
}

impl Default for DogeosEthApiBuilder {
    fn default() -> Self {
        Self::without_scroll_wire()
    }
}

impl DogeosEthApiBuilder {
    pub(crate) const fn new(
        scroll_wire: ScrollWireRuntime,
        scroll_wire_signer: Option<Address>,
        base_fee_policy: SequencerBaseFeePolicy,
        desired_gas_limit: u64,
    ) -> Self {
        Self {
            scroll_wire,
            scroll_wire_signer,
            base_fee_policy,
            desired_gas_limit,
        }
    }

    /// Creates a DogeOS `eth_` API builder without installing the inherited `scroll/1` importer.
    ///
    /// This is intended for downstream nodes that own their own network import path but still want
    /// DogeOS RPC schemas, conversion, pending-block policy, and gas-oracle behavior.
    pub fn without_scroll_wire() -> Self {
        Self {
            scroll_wire: ScrollWireRuntime {
                manager: Arc::new(Mutex::new(None)),
            },
            scroll_wire_signer: None,
            base_fee_policy: crate::args::default_sequencer_base_fee_policy(),
            desired_gas_limit: crate::DOGEOS_DEFAULT_GAS_LIMIT,
        }
    }

    /// Overrides pending prediction with the policy and gas target used by payload production.
    pub const fn with_producer_policy(
        mut self,
        base_fee_policy: SequencerBaseFeePolicy,
        desired_gas_limit: u64,
    ) -> Self {
        self.base_fee_policy = base_fee_policy;
        self.desired_gas_limit = desired_gas_limit;
        self
    }
}

impl<N> EthApiBuilder<N> for DogeosEthApiBuilder
where
    N: FullNodeComponents<Types: DogeosCompatibleNodeTypes, Evm = ScrollEvmConfig>,
    N::Provider: ReceiptProvider<Receipt = ScrollReceipt>
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec = DogeosChainSpec>,
    DogeosRpcConverter<N::Provider>: RpcConvert<
            Primitives = dogeos_reth_primitives::DogeosPrimitives,
            Evm = ScrollEvmConfig,
            Error = EthApiError,
            Network = dogeos_rpc_types::Scroll,
        >,
    EthApi<N, DogeosRpcConverter<N::Provider>>:
        FullEthApiServer<Provider = N::Provider, Pool = N::Pool>,
    EthApiError: FromEvmError<ScrollEvmConfig>,
{
    type EthApi = EthApi<N, DogeosRpcConverter<N::Provider>>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        if let Some(manager) = self.scroll_wire.take() {
            let importer = crate::DogeosScrollWireEngineImporter::new(
                manager,
                ctx.engine_handle.clone(),
                self.scroll_wire_signer,
            );
            ctx.components
                .task_executor()
                .spawn_critical_task("scroll-wire engine importer", importer.run());
            tracing::info!(target: "reth::cli", "DogeOS scroll/1 block importer started");
        }

        let provider = ctx.components.provider().clone();
        let converter = dogeos_rpc_converter(provider.clone());
        let pending_env_builder =
            DogeosPendingEnvBuilder::new(provider, self.base_fee_policy, self.desired_gas_limit);
        let config = ctx.config;
        Ok(
            reth_rpc::EthApiBuilder::new_with_components(ctx.components.clone())
                .eth_cache(ctx.cache)
                .task_spawner(ctx.components.task_executor().clone())
                .gas_cap(config.rpc_gas_cap.into())
                .max_simulate_blocks(config.rpc_max_simulate_blocks)
                .eth_proof_window(config.eth_proof_window)
                .fee_history_cache_config(config.fee_history_cache)
                .proof_permits(config.proof_permits)
                .gas_oracle_config(config.gas_oracle)
                .max_batch_size(config.max_batch_size)
                .max_blocking_io_requests(config.max_blocking_io_requests)
                .pending_block_kind(config.pending_block_kind)
                .evm_memory_limit(config.rpc_evm_memory_limit)
                .force_blob_sidecar_upcasting(config.force_blob_sidecar_upcasting)
                .with_rpc_converter(converter)
                .with_pending_env_builder(pending_env_builder)
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{Address, B256};
    use dogeos_chainspec::DOGEOS_DEV;
    use dogeos_reth_primitives::DogeosPrimitives;
    use reth_provider::test_utils::MockEthProvider;

    #[test]
    fn without_scroll_wire_cannot_launch_importer() {
        let builder = DogeosEthApiBuilder::without_scroll_wire();

        assert!(builder.scroll_wire.take().is_none());
        assert!(builder.scroll_wire_signer.is_none());
    }

    #[test]
    fn pending_environment_uses_producer_policy_and_bounded_gas_limit() {
        let provider =
            MockEthProvider::<DogeosPrimitives>::new().with_chain_spec(DOGEOS_DEV.as_ref().clone());
        let header = Header {
            timestamp: 42,
            beneficiary: Address::repeat_byte(1),
            gas_limit: 20_000_000,
            gas_used: 20_000_000,
            base_fee_per_gas: Some(420_000_000_000),
            ..Default::default()
        };
        let parent = SealedHeader::new(header, B256::ZERO);
        let attributes = DogeosPendingEnvBuilder::new(
            provider,
            crate::args::default_sequencer_base_fee_policy(),
            30_000_000,
        )
        .pending_env_attributes(&parent)
        .unwrap();
        assert_eq!(attributes.timestamp, 42);
        assert_eq!(attributes.base_fee, 427_382_812_500);
        assert_eq!(attributes.gas_limit, 20_019_530);
    }
}
