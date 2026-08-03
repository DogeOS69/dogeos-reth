use super::ScrollWireEvent;
use crate::{ScrollWireConfig, connection::ScrollConnectionHandler};
use reth_network::protocol::ProtocolHandler;
use reth_network_api::PeerId;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct ScrollWireProtocolState {
    event_sender: mpsc::UnboundedSender<ScrollWireEvent>,
}

impl ScrollWireProtocolState {
    pub const fn event_sender(&self) -> &mpsc::UnboundedSender<ScrollWireEvent> {
        &self.event_sender
    }
}

#[derive(Debug)]
pub struct ScrollWireProtocolHandler {
    state: ScrollWireProtocolState,
    config: ScrollWireConfig,
}

impl ScrollWireProtocolHandler {
    pub fn new(config: ScrollWireConfig) -> (Self, mpsc::UnboundedReceiver<ScrollWireEvent>) {
        let (event_sender, events) = mpsc::unbounded_channel();
        (
            Self {
                state: ScrollWireProtocolState { event_sender },
                config,
            },
            events,
        )
    }
}

impl ProtocolHandler for ScrollWireProtocolHandler {
    type ConnectionHandler = ScrollConnectionHandler;

    fn on_incoming(&self, _socket_addr: std::net::SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(ScrollConnectionHandler::new(
            self.state.clone(),
            self.config,
        ))
    }

    fn on_outgoing(
        &self,
        _socket_addr: std::net::SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(ScrollConnectionHandler::new(
            self.state.clone(),
            self.config,
        ))
    }
}
