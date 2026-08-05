use crate::DogeosCompatibleNodeTypes;
use dogeos_reth_engine::DogeosEngineValidator;
use reth_node_builder::{AddOnsContext, FullNodeComponents, rpc::PayloadValidatorBuilder};

/// Installs the DogeOS Engine API validator into Reth's RPC add-ons.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct DogeosEngineValidatorBuilder;

impl<Node> PayloadValidatorBuilder<Node> for DogeosEngineValidatorBuilder
where
    Node: FullNodeComponents<Types: DogeosCompatibleNodeTypes>,
{
    type Validator = DogeosEngineValidator<dogeos_chainspec::DogeosChainSpec>;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(DogeosEngineValidator::new(ctx.config.chain.clone()))
    }
}
