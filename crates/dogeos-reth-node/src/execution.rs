use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_evm::ScrollEvmConfig;
use dogeos_reth_primitives::DogeosPrimitives;
use reth_node_builder::{BuilderContext, FullNodeTypes, components::ExecutorBuilder};
use reth_node_types::NodeTypes;

/// Constructs the DogeOS REVM 36 executor configuration for node components.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct DogeosExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for DogeosExecutorBuilder
where
    Node:
        FullNodeTypes<Types: NodeTypes<ChainSpec = DogeosChainSpec, Primitives = DogeosPrimitives>>,
{
    type EVM = ScrollEvmConfig<dogeos_chainspec::DogeosChainSpec>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        Ok(ScrollEvmConfig::dogeos(ctx.chain_spec()))
    }
}
