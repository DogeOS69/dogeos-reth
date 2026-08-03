use crate::DogeosNodeTypes;
use dogeos_reth_evm::ScrollEvmConfig;
use reth_node_builder::{BuilderContext, FullNodeTypes, components::ExecutorBuilder};

/// Constructs the DogeOS REVM 36 executor configuration for node components.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct DogeosExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for DogeosExecutorBuilder
where
    Node: FullNodeTypes<Types = DogeosNodeTypes>,
{
    type EVM = ScrollEvmConfig<dogeos_chainspec::DogeosChainSpec>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        Ok(ScrollEvmConfig::dogeos(ctx.chain_spec()))
    }
}
