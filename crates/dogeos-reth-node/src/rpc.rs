use crate::DogeosNodeTypes;
use alloy_consensus::BlockHeader;
use alloy_primitives::Address;
use dogeos_reth_evm::{ScrollEvmConfig, ScrollNextBlockEnvAttributes};
use dogeos_reth_primitives::ScrollReceipt;
use dogeos_reth_rpc::{DogeosRpcConverter, dogeos_rpc_converter};
use reth_node_builder::{
    FullNodeComponents,
    rpc::{EthApiBuilder, EthApiCtx},
};
use reth_primitives_traits::SealedHeader;
use reth_rpc::EthApi;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{FullEthApiServer, helpers::pending_block::PendingEnvBuilder};
use reth_rpc_eth_types::{EthApiError, error::FromEvmError};
use reth_storage_api::ReceiptProvider;
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

/// Derives a best-effort pending environment without introducing an RPC dependency into the EVM
/// owner crate. Canonical payload construction continues to use the state-aware base-fee provider.
#[derive(Debug, Default, Clone, Copy)]
pub struct DogeosPendingEnvBuilder;

impl PendingEnvBuilder<ScrollEvmConfig> for DogeosPendingEnvBuilder {
    fn pending_env_attributes(
        &self,
        parent: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<ScrollNextBlockEnvAttributes, EthApiError> {
        Ok(ScrollNextBlockEnvAttributes {
            timestamp: parent.timestamp(),
            suggested_fee_recipient: parent.beneficiary(),
            gas_limit: parent.gas_limit(),
            base_fee: parent.base_fee_per_gas().unwrap_or_default(),
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
}

impl<N> EthApiBuilder<N> for DogeosEthApiBuilder
where
    N: FullNodeComponents<Types = DogeosNodeTypes, Evm = ScrollEvmConfig>,
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
                .with_pending_env_builder(DogeosPendingEnvBuilder)
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::{Address, B256};

    #[test]
    fn pending_environment_preserves_equal_timestamp_policy() {
        let header = Header {
            timestamp: 42,
            beneficiary: Address::repeat_byte(1),
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(7),
            ..Default::default()
        };
        let parent = SealedHeader::new(header, B256::ZERO);
        let attributes = DogeosPendingEnvBuilder
            .pending_env_attributes(&parent)
            .unwrap();
        assert_eq!(attributes.timestamp, 42);
        assert_eq!(attributes.base_fee, 7);
        assert_eq!(attributes.gas_limit, 30_000_000);
    }
}
