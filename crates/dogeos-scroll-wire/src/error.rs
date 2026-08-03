use reth_network_api::PeerId;

#[derive(Debug, Clone, thiserror::Error)]
pub enum AnnounceBlockError {
    #[error("peer {0} is not connected")]
    PeerNotConnected(PeerId),
    #[error("failed to send block to peer {0}")]
    SendFailed(PeerId),
}
