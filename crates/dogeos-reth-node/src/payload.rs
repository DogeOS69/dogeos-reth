use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_reth_evm::ScrollNextBlockEnvAttributes;
use dogeos_reth_payload::{ScrollBuilderConfig, ScrollPayloadBuilder};
use dogeos_reth_primitives::{DogeosPrimitives, ScrollTransactionSigned};
use reth_evm::ConfigureEvm;
use reth_node_builder::{
    BuilderContext, FullNodeTypes, PayloadBuilderConfig, components::PayloadBuilderBuilder,
};
use reth_node_types::NodeTypes;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::time::Duration;

/// Default gas limit inherited by the standalone DogeOS sequencer.
pub const DOGEOS_DEFAULT_GAS_LIMIT: u64 = 20_000_000;
/// Maximum encoded transaction bytes considered during one payload build.
pub const DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT: u64 = 122_880;
/// Payload jobs stop selecting pool transactions after this duration.
pub const DOGEOS_DEFAULT_PAYLOAD_BUILDING_DURATION: Duration = Duration::from_secs(1);

/// Adapts the DogeOS payload policy to Reth 2's basic payload service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DogeosPayloadBuilderBuilder {
    pub payload_building_time_limit: Duration,
    pub block_da_size_limit: Option<u64>,
}

impl Default for DogeosPayloadBuilderBuilder {
    fn default() -> Self {
        Self {
            payload_building_time_limit: DOGEOS_DEFAULT_PAYLOAD_BUILDING_DURATION,
            block_da_size_limit: Some(DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT),
        }
    }
}

impl<Node, Pool, Evm> PayloadBuilderBuilder<Node, Pool, Evm> for DogeosPayloadBuilderBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<
            ChainSpec = dogeos_chainspec::DogeosChainSpec,
            Primitives = DogeosPrimitives,
            Payload = DogeosEngineTypes,
        >,
    >,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ScrollTransactionSigned>>
        + Unpin
        + 'static,
    Evm: ConfigureEvm<Primitives = DogeosPrimitives, NextBlockEnvCtx = ScrollNextBlockEnvAttributes>
        + 'static,
{
    type PayloadBuilder = ScrollPayloadBuilder<Pool, Node::Provider, Evm>;

    async fn build_payload_builder(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
        evm_config: Evm,
    ) -> eyre::Result<Self::PayloadBuilder> {
        let configured_gas_limit = ctx.payload_builder_config().gas_limit();
        let gas_limit = configured_gas_limit.unwrap_or_else(|| {
            tracing::warn!(
                target: "reth::cli",
                gas_limit = DOGEOS_DEFAULT_GAS_LIMIT,
                "payload gas limit not configured; using DogeOS default"
            );
            DOGEOS_DEFAULT_GAS_LIMIT
        });

        Ok(ScrollPayloadBuilder::new(
            ctx.provider().clone(),
            pool,
            evm_config,
            ScrollBuilderConfig::new(
                Some(gas_limit),
                self.payload_building_time_limit,
                self.block_da_size_limit,
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_current_sequencer_policy() {
        let config = DogeosPayloadBuilderBuilder::default();
        assert_eq!(config.payload_building_time_limit, Duration::from_secs(1));
        assert_eq!(
            config.block_da_size_limit,
            Some(DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT)
        );
    }
}
