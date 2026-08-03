//! DogeOS-owned type boundary for the standalone Reth node.
//!
//! The protocol-facing primitives, chainspec, and Engine payload family are DogeOS-owned. Storage
//! remains the final temporary Ethereum alias until the Storage V2 codec is migrated.

use dogeos_chainspec::DogeosChainSpec;
use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_reth_primitives::DogeosPrimitives;
use reth_ethereum::node::EthereumNode;
use reth_node_types::NodeTypes;

/// Stateless node type configuration owned by DogeOS.
#[derive(Clone, Copy, Debug, Default)]
pub struct DogeosNodeTypes;

impl NodeTypes for DogeosNodeTypes {
    type Primitives = DogeosPrimitives;
    type ChainSpec = DogeosChainSpec;
    type Storage = <EthereumNode as NodeTypes>::Storage;
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
                Payload = DogeosEngineTypes,
            >,
    >() {
    }

    #[test]
    fn node_boundary_uses_dogeos_protocol_types() {
        requires_protocol_types::<DogeosNodeTypes>();
    }
}
