use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_reth_engine::DogeosEngineValidator;
use dogeos_reth_primitives::DogeosPrimitives;
use reth_node_builder::{AddOnsContext, FullNodeComponents, rpc::PayloadValidatorBuilder};
use reth_node_types::NodeTypes;

/// Installs the DogeOS Engine API validator into Reth's RPC add-ons.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct DogeosEngineValidatorBuilder;

impl<Node> PayloadValidatorBuilder<Node> for DogeosEngineValidatorBuilder
where
    Node: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec = DogeosChainSpec,
            Primitives = DogeosPrimitives,
            Payload = DogeosEngineTypes,
        >,
    >,
{
    type Validator = DogeosEngineValidator<dogeos_chainspec::DogeosChainSpec>;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(DogeosEngineValidator::new(ctx.config.chain.clone()))
    }
}
