use crate::{
    AnnounceBlockError,
    protocol::{NewBlock, ScrollMessage, ScrollWireEvent},
};
use futures::StreamExt;
use reth_network_api::PeerId;
use std::{
    collections::{HashMap, hash_map::Entry},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

pub const LRU_CACHE_SIZE: u32 = 100;

#[derive(Debug)]
pub struct ScrollWireManager {
    events: UnboundedReceiverStream<ScrollWireEvent>,
    connections: HashMap<PeerId, UnboundedSender<ScrollMessage>>,
}

impl ScrollWireManager {
    pub fn new(events: UnboundedReceiver<ScrollWireEvent>) -> Self {
        Self {
            events: events.into(),
            connections: HashMap::new(),
        }
    }

    pub fn announce_block(
        &mut self,
        peer_id: PeerId,
        block: &NewBlock,
    ) -> Result<(), AnnounceBlockError> {
        let Entry::Occupied(connection) = self.connections.entry(peer_id) else {
            return Err(AnnounceBlockError::PeerNotConnected(peer_id));
        };
        if connection
            .get()
            .send(ScrollMessage::new_block(block.clone()))
            .is_err()
        {
            connection.remove();
            return Err(AnnounceBlockError::SendFailed(peer_id));
        }
        Ok(())
    }

    pub fn connected_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.connections.keys()
    }

    pub fn is_connected(&self, peer_id: PeerId) -> bool {
        self.connections.contains_key(&peer_id)
    }
}

impl Future for ScrollWireManager {
    type Output = ScrollWireEvent;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        while let Poll::Ready(event) = this.events.poll_next_unpin(cx) {
            match event {
                Some(event @ ScrollWireEvent::NewBlock { .. }) => return Poll::Ready(event),
                Some(ScrollWireEvent::ConnectionEstablished {
                    peer_id,
                    to_connection,
                    ..
                }) => {
                    this.connections.insert(peer_id, to_connection);
                }
                None => break,
            }
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Signature, U256};
    use dogeos_reth_primitives::DogeosBlock;
    use reth_network::Direction;

    #[test]
    fn tracks_connection_and_announces_after_signed_block_event() {
        let peer_id = PeerId::random();
        let (event_sender, events) = tokio::sync::mpsc::unbounded_channel();
        let (to_connection, mut outbound) = tokio::sync::mpsc::unbounded_channel();
        event_sender
            .send(ScrollWireEvent::connection_established(
                Direction::Incoming,
                peer_id,
                to_connection,
            ))
            .unwrap();
        event_sender
            .send(ScrollWireEvent::new_block(
                peer_id,
                DogeosBlock::default(),
                Signature::new(U256::from(1), U256::from(2), false),
            ))
            .unwrap();

        let mut manager = ScrollWireManager::new(events);
        let event = futures::executor::block_on(&mut manager);
        assert!(matches!(event, ScrollWireEvent::NewBlock { peer_id: id, .. } if id == peer_id));
        assert!(manager.is_connected(peer_id));

        let announcement = NewBlock::new(
            Signature::new(U256::from(3), U256::from(4), true),
            DogeosBlock::default(),
        );
        manager.announce_block(peer_id, &announcement).unwrap();
        assert!(matches!(
            outbound.try_recv().unwrap().payload,
            crate::protocol::ScrollMessagePayload::NewBlock(block) if block == announcement
        ));
    }

    #[test]
    fn reconnect_replaces_the_stale_connection() {
        let peer_id = PeerId::random();
        let (event_sender, events) = tokio::sync::mpsc::unbounded_channel();
        let (old_connection, mut old_outbound) = tokio::sync::mpsc::unbounded_channel();
        let (new_connection, mut new_outbound) = tokio::sync::mpsc::unbounded_channel();
        event_sender
            .send(ScrollWireEvent::connection_established(
                Direction::Incoming,
                peer_id,
                old_connection,
            ))
            .unwrap();
        event_sender
            .send(ScrollWireEvent::connection_established(
                Direction::Incoming,
                peer_id,
                new_connection,
            ))
            .unwrap();
        event_sender
            .send(ScrollWireEvent::new_block(
                peer_id,
                DogeosBlock::default(),
                Signature::new(U256::from(1), U256::from(2), false),
            ))
            .unwrap();

        let mut manager = ScrollWireManager::new(events);
        let _ = futures::executor::block_on(&mut manager);
        let announcement = NewBlock::new(
            Signature::new(U256::from(3), U256::from(4), true),
            DogeosBlock::default(),
        );
        manager.announce_block(peer_id, &announcement).unwrap();

        assert!(matches!(
            old_outbound.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ));
        assert!(new_outbound.try_recv().is_ok());
    }

    #[test]
    fn failed_send_removes_the_connection() {
        let peer_id = PeerId::random();
        let (event_sender, events) = tokio::sync::mpsc::unbounded_channel();
        let (to_connection, outbound) = tokio::sync::mpsc::unbounded_channel();
        event_sender
            .send(ScrollWireEvent::connection_established(
                Direction::Incoming,
                peer_id,
                to_connection,
            ))
            .unwrap();
        event_sender
            .send(ScrollWireEvent::new_block(
                peer_id,
                DogeosBlock::default(),
                Signature::new(U256::from(1), U256::from(2), false),
            ))
            .unwrap();

        let mut manager = ScrollWireManager::new(events);
        let _ = futures::executor::block_on(&mut manager);
        drop(outbound);
        let announcement = NewBlock::new(
            Signature::new(U256::from(3), U256::from(4), true),
            DogeosBlock::default(),
        );

        assert!(matches!(
            manager.announce_block(peer_id, &announcement),
            Err(AnnounceBlockError::SendFailed(id)) if id == peer_id
        ));
        assert!(!manager.is_connected(peer_id));
        assert!(matches!(
            manager.announce_block(peer_id, &announcement),
            Err(AnnounceBlockError::PeerNotConnected(id)) if id == peer_id
        ));
    }
}
