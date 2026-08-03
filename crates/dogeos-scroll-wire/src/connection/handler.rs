use super::ScrollWireConnection;
use crate::{
    ScrollWireConfig, ScrollWireEvent,
    protocol::{ScrollMessage, ScrollWireProtocolState},
};
use reth_network::protocol::{ConnectionHandler, OnNotSupported};
use tracing::trace;

#[derive(Debug)]
pub struct ScrollConnectionHandler {
    state: ScrollWireProtocolState,
    config: ScrollWireConfig,
}

impl ScrollConnectionHandler {
    pub const fn new(state: ScrollWireProtocolState, config: ScrollWireConfig) -> Self {
        Self { state, config }
    }
}

impl ConnectionHandler for ScrollConnectionHandler {
    type Connection = ScrollWireConnection;

    fn protocol(&self) -> reth_eth_wire::protocol::Protocol {
        ScrollMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &reth_eth_wire::capability::SharedCapabilities,
        _direction: reth_network::Direction,
        _peer_id: reth_network_api::PeerId,
    ) -> OnNotSupported {
        if self.config.connect_unsupported_peer() {
            OnNotSupported::KeepAlive
        } else {
            OnNotSupported::Disconnect
        }
    }

    fn into_connection(
        self,
        direction: reth_network::Direction,
        peer_id: reth_network_api::PeerId,
        conn: reth_eth_wire::multiplex::ProtocolConnection,
    ) -> Self::Connection {
        trace!(target: "dogeos::scroll_wire", %peer_id, ?direction, "scroll-wire connection established");
        let (to_connection, outbound) = tokio::sync::mpsc::unbounded_channel();
        let _ = self
            .state
            .event_sender()
            .send(ScrollWireEvent::connection_established(
                direction,
                peer_id,
                to_connection,
            ));
        ScrollWireConnection::new(peer_id, conn, direction, outbound, self.state)
    }
}
