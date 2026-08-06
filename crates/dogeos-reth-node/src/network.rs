use crate::DogeosCompatibleNodeTypes;
use reth_network::{
    NetworkHandle, NetworkManager, PeersInfo,
    primitives::BasicNetworkPrimitives,
    protocol::{IntoRlpxSubProtocol, RlpxSubProtocol},
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

    /// Attaches the inherited `scroll/1` protocol and returns its event/announcement manager.
    pub fn with_scroll_wire(
        self,
        config: dogeos_scroll_wire::ScrollWireConfig,
    ) -> (Self, dogeos_scroll_wire::ScrollWireManager) {
        let (handler, events) = dogeos_scroll_wire::ScrollWireProtocolHandler::new(config);
        (
            self.with_sub_protocol(handler.into_rlpx_sub_protocol()),
            dogeos_scroll_wire::ScrollWireManager::new(events),
        )
    }
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for DogeosNetworkBuilder
where
    Node: FullNodeTypes<Types: DogeosCompatibleNodeTypes>,
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
        tracing::info!(target: "reth::cli", "Initializing DogeOS P2P networking");
        let network = NetworkManager::builder(ctx.build_network_config(config)).await?;
        let handle = ctx.start_network(network, pool, None);
        tracing::info!(
            target: "reth::cli",
            enode = %handle.local_node_record(),
            "DogeOS P2P networking initialized"
        );
        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_wire_is_attached_through_the_public_rlpx_hook() {
        let (builder, manager) = DogeosNetworkBuilder::new()
            .with_scroll_wire(dogeos_scroll_wire::ScrollWireConfig::new(true));
        assert_eq!(builder.extra_protocols.len(), 1);
        assert_eq!(manager.connected_peers().count(), 0);
    }
}
