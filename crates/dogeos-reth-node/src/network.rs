use crate::DogeosNodeTypes;
use reth_network::{
    NetworkHandle, NetworkManager, PeersInfo, primitives::BasicNetworkPrimitives,
    protocol::RlpxSubProtocol,
};
use reth_node_builder::{BuilderContext, FullNodeTypes, components::NetworkBuilder};
use reth_transaction_pool::{PoolPooledTx, PoolTransaction, TransactionPool};

/// Builds the canonical eth-wire network; `scroll-wire` is attached as an extra RLPx protocol.
#[derive(Debug, Default)]
pub struct DogeosNetworkBuilder {
    extra_protocols: Vec<RlpxSubProtocol>,
}

impl DogeosNetworkBuilder {
    pub const fn new() -> Self {
        Self {
            extra_protocols: Vec::new(),
        }
    }

    /// Registers an inherited compatibility protocol such as `scroll-wire` through Reth's public
    /// RLPx extension point.
    pub fn with_sub_protocol(mut self, protocol: RlpxSubProtocol) -> Self {
        self.extra_protocols.push(protocol);
        self
    }
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for DogeosNetworkBuilder
where
    Node: FullNodeTypes<Types = DogeosNodeTypes>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = dogeos_reth_primitives::ScrollTransactionSigned,
            >,
        > + Unpin
        + 'static,
{
    type Network = NetworkHandle<
        BasicNetworkPrimitives<dogeos_reth_primitives::DogeosPrimitives, PoolPooledTx<Pool>>,
    >;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
        let mut config =
            ctx.network_config_builder::<BasicNetworkPrimitives<
                dogeos_reth_primitives::DogeosPrimitives,
                PoolPooledTx<Pool>,
            >>()?;
        for protocol in self.extra_protocols {
            config = config.add_rlpx_sub_protocol(protocol);
        }
        let network = NetworkManager::builder(ctx.build_network_config(config)).await?;
        let handle = ctx.start_network(network, pool);
        tracing::info!(
            target: "reth::cli",
            enode = %handle.local_node_record(),
            "DogeOS P2P networking initialized"
        );
        Ok(handle)
    }
}
