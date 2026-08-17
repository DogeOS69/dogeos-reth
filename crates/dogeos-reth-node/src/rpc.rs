use crate::DogeosCompatibleNodeTypes;
use alloy_consensus::BlockHeader;
use alloy_primitives::Address;
use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_evm::{
    ScrollBaseFeeProvider, ScrollEvmConfig, ScrollNextBlockEnvAttributes,
    predict_next_payload_timestamp,
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

/// Derives a pending environment from the canonical parent's post-state.
#[derive(Debug, Clone)]
pub struct DogeosPendingEnvBuilder<Provider> {
    provider: Provider,
}

impl<Provider> DogeosPendingEnvBuilder<Provider> {
    pub const fn new(provider: Provider) -> Self {
        Self { provider }
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
        let timestamp = predict_next_payload_timestamp(parent.timestamp());
        let state = self
            .provider
            .state_by_block_hash(parent.hash())
            .map_err(EthApiError::from)?;
        let mut state = StateProviderDatabase::new(state.as_ref());
        let base_fee = ScrollBaseFeeProvider::new(self.provider.chain_spec())
            .next_block_base_fee(&mut state, parent.header(), timestamp)
            .map_err(|error| EthApiError::EvmCustom(error.to_string()))?;

        Ok(ScrollNextBlockEnvAttributes {
            timestamp,
            suggested_fee_recipient: parent.beneficiary(),
            gas_limit: parent.gas_limit(),
            base_fee,
        })
    }
}

/// Builds the standard Reth `eth_` API with DogeOS RPC schemas and converters.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct DogeosEthApiBuilder {
    scroll_wire: ScrollWireRuntime,
    scroll_wire_signer: Option<Address>,
}

impl DogeosEthApiBuilder {
    pub(crate) const fn new(
        scroll_wire: ScrollWireRuntime,
        scroll_wire_signer: Option<Address>,
    ) -> Self {
        Self {
            scroll_wire,
            scroll_wire_signer,
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
        }
    }
}

impl<N> EthApiBuilder<N> for DogeosEthApiBuilder
where
    N: FullNodeComponents<Types: DogeosCompatibleNodeTypes, Evm = ScrollEvmConfig>,
    N::Provider: ReceiptProvider<Receipt = ScrollReceipt>,
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

        let converter = dogeos_rpc_converter(ctx.components.provider().clone());
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
                .with_pending_env_builder(DogeosPendingEnvBuilder::new(
                    ctx.components.provider().clone(),
                ))
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{Address, B256, U256};
    use dogeos_chainspec::DOGEOS_MAINNET;
    use dogeos_reth_evm::NEXT_CONTROLLED_BASE_FEE_SLOT;
    use dogeos_reth_primitives::DogeosPrimitives;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    #[test]
    fn without_scroll_wire_cannot_launch_importer() {
        let builder = DogeosEthApiBuilder::without_scroll_wire();

        assert!(builder.scroll_wire.take().is_none());
        assert!(builder.scroll_wire_signer.is_none());
    }

    #[test]
    fn pending_environment_reads_the_next_fee_from_parent_state() {
        let provider = MockEthProvider::<DogeosPrimitives>::new()
            .with_chain_spec(DOGEOS_MAINNET.as_ref().clone());
        let system_config = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        provider.add_account(
            system_config,
            ExtendedAccount::new(1, U256::ZERO).extend_storage([
                (B256::from(U256::from(101)), U256::from(100_000_000u64)),
                (
                    B256::from(NEXT_CONTROLLED_BASE_FEE_SLOT),
                    U256::from(600_000_000_000u64),
                ),
            ]),
        );
        let header = Header {
            timestamp: 42,
            beneficiary: Address::repeat_byte(1),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(7),
            ..Default::default()
        };
        let parent = SealedHeader::new(header, B256::ZERO);
        let attributes = DogeosPendingEnvBuilder::new(provider)
            .pending_env_attributes(&parent)
            .unwrap();
        assert_eq!(attributes.timestamp, 43);
        assert_eq!(attributes.base_fee, 600_100_000_000);
        assert_eq!(attributes.gas_limit, 30_000_000);
    }
}
