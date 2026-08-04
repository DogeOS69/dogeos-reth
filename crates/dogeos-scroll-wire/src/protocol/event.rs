use crate::protocol::ScrollMessage;
use alloy_primitives::Signature;
use dogeos_reth_primitives::DogeosBlock;
use reth_network::Direction;
use reth_network_api::PeerId;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // NewBlock is forwarded once; avoid a second heap allocation.
pub enum ScrollWireEvent {
    ConnectionEstablished {
        direction: Direction,
        peer_id: PeerId,
        to_connection: UnboundedSender<ScrollMessage>,
    },
    NewBlock {
        peer_id: PeerId,
        block: DogeosBlock,
        signature: Signature,
    },
}

impl ScrollWireEvent {
    pub const fn connection_established(
        direction: Direction,
        peer_id: PeerId,
        to_connection: UnboundedSender<ScrollMessage>,
    ) -> Self {
        Self::ConnectionEstablished {
            direction,
            peer_id,
            to_connection,
        }
    }

    pub const fn new_block(peer_id: PeerId, block: DogeosBlock, signature: Signature) -> Self {
        Self::NewBlock {
            peer_id,
            block,
            signature,
        }
    }
}
