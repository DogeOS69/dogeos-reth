use crate::DogeosCompatibleNodeTypes;
use dogeos_reth_consensus::DogeosConsensus;
use reth_node_builder::{BuilderContext, FullNodeTypes, components::ConsensusBuilder};
use std::sync::Arc;

/// Installs DogeOS header, body, and receipt validation into the node pipeline.
#[derive(Debug, Default, Clone, Copy)]
pub struct DogeosConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for DogeosConsensusBuilder
where
    Node: FullNodeTypes<Types: DogeosCompatibleNodeTypes>,
{
    type Consensus = Arc<DogeosConsensus>;

    async fn build_consensus(self, _ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(DogeosConsensus))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_reth_primitives::DogeosPrimitives;
    use reth_consensus::FullConsensus;

    fn requires_consensus<T: FullConsensus<DogeosPrimitives>>() {}

    #[test]
    fn consensus_matches_node_primitives() {
        requires_consensus::<DogeosConsensus>();
    }
}
