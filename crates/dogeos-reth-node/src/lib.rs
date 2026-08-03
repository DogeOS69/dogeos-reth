//! DogeOS-owned type boundary for the standalone Reth node.
//!
//! The initial spike deliberately delegates its associated types to Reth's
//! Ethereum defaults. Each associated type is an explicit replacement seam for
//! a DogeOS primitive, chainspec, storage codec, or Engine payload type.

use reth_ethereum::node::EthereumNode;
use reth_node_builder::{components::NodeComponentsBuilder, node::FullNodeTypes};
use reth_node_types::NodeTypes;

/// Stateless node type configuration owned by DogeOS.
#[derive(Clone, Copy, Debug, Default)]
pub struct DogeosNodeTypes;

impl NodeTypes for DogeosNodeTypes {
    type Primitives = <EthereumNode as NodeTypes>::Primitives;
    type ChainSpec = <EthereumNode as NodeTypes>::ChainSpec;
    type Storage = <EthereumNode as NodeTypes>::Storage;
    type Payload = <EthereumNode as NodeTypes>::Payload;
}

/// Builds the temporary Reth 2 component set for DogeOS node types.
///
/// The components are intentionally inherited only while their associated
/// types remain aliases. DogeOS policy replaces them one boundary at a time;
/// this named function prevents callers from treating `EthereumNode` as the
/// durable owner of composition.
pub fn components<N>() -> impl NodeComponentsBuilder<N>
where
    N: FullNodeTypes<Types = DogeosNodeTypes>,
{
    EthereumNode::components()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requires_node_types<T: NodeTypes>() {}

    #[test]
    fn dogeos_node_types_satisfy_reths_public_contract() {
        requires_node_types::<DogeosNodeTypes>();
    }
}
