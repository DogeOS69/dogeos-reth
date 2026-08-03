//! DogeOS-owned type boundary for the standalone Reth node.
//!
//! The protocol-facing primitives, chainspec, Engine payload family, and generic transaction
//! storage binding are all selected at this boundary.

use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_reth_primitives::DogeosPrimitives;
use reth_node_types::NodeTypes;

mod payload;
pub use payload::DogeosPayloadBuilderBuilder;
mod engine;
pub use engine::DogeosEngineValidatorBuilder;
mod consensus;
pub use consensus::DogeosConsensusBuilder;
mod execution;
pub use execution::DogeosExecutorBuilder;

/// Reth 2's generic body storage bound to the inherited Scroll transaction envelope.
pub type DogeosStorage = reth_storage_api::EthStorage<
    <DogeosPrimitives as reth_primitives_traits::NodePrimitives>::SignedTx,
>;

/// Stateless node type configuration owned by DogeOS.
#[derive(Clone, Copy, Debug, Default)]
pub struct DogeosNodeTypes;

impl NodeTypes for DogeosNodeTypes {
    type Primitives = DogeosPrimitives;
    type ChainSpec = DogeosChainSpec;
    type Storage = DogeosStorage;
    type Payload = DogeosEngineTypes;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requires_node_types<T: NodeTypes>() {}

    #[test]
    fn dogeos_node_types_satisfy_reths_public_contract() {
        requires_node_types::<DogeosNodeTypes>();
    }

    fn requires_protocol_types<
        T: NodeTypes<
                Primitives = DogeosPrimitives,
                ChainSpec = DogeosChainSpec,
                Storage = DogeosStorage,
                Payload = DogeosEngineTypes,
            >,
    >() {
    }

    #[test]
    fn node_boundary_uses_dogeos_protocol_types() {
        requires_protocol_types::<DogeosNodeTypes>();
    }
}
