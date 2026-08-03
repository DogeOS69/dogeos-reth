use crate::protocol::{
    ScrollMessage, ScrollMessagePayload, ScrollWireEvent, ScrollWireProtocolState,
};
use alloy_primitives::{Signature, bytes::BytesMut};
use futures::{Stream, StreamExt};
use reth_eth_wire::multiplex::ProtocolConnection;
use reth_network::Direction;
use reth_network_api::PeerId;
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::trace;

mod handler;
pub(crate) use handler::ScrollConnectionHandler;

#[derive(Debug)]
pub struct ScrollWireConnection {
    conn: ProtocolConnection,
    pub direction: Direction,
    outbound: UnboundedReceiverStream<ScrollMessage>,
    events: UnboundedSender<ScrollWireEvent>,
    peer_id: PeerId,
}

impl ScrollWireConnection {
    pub fn new(
        peer_id: PeerId,
        conn: ProtocolConnection,
        direction: Direction,
        outbound: UnboundedReceiver<ScrollMessage>,
        state: ScrollWireProtocolState,
    ) -> Self {
        Self {
            conn,
            direction,
            outbound: outbound.into(),
            events: state.event_sender().clone(),
            peer_id,
        }
    }
}

impl Stream for ScrollWireConnection {
    type Item = BytesMut;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Poll::Ready(Some(message)) = this.outbound.poll_next_unpin(cx) {
                return Poll::Ready(Some(message.encoded()));
            }

            let Some(message) = ready!(this.conn.poll_next_unpin(cx)) else {
                return Poll::Ready(None);
            };
            let Some(message) = ScrollMessage::decode(&mut &message[..]) else {
                return Poll::Ready(None);
            };

            match message.payload {
                ScrollMessagePayload::NewBlock(new_block) => {
                    let Ok(signature) = Signature::from_raw(&new_block.signature) else {
                        trace!(target: "dogeos::scroll_wire", peer_id = %this.peer_id, "invalid block signature; closing scroll-wire connection");
                        return Poll::Ready(None);
                    };
                    if this
                        .events
                        .send(ScrollWireEvent::new_block(
                            this.peer_id,
                            new_block.block,
                            signature,
                        ))
                        .is_err()
                    {
                        return Poll::Ready(None);
                    }
                }
            }
        }
    }
}
