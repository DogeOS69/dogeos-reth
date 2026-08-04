use alloy_primitives::Address;
use alloy_rpc_types_engine::PayloadStatus;
use dogeos_reth_engine::DogeosEngineTypes;
use dogeos_scroll_wire::{
    BlockSignatureError, LRU_CACHE_SIZE, ScrollWireEvent, ScrollWireManager, verify_block_signature,
};
use reth_engine_primitives::{BeaconOnNewPayloadError, ConsensusEngineHandle};
use reth_network::cache::LruCache;
use reth_payload_primitives::PayloadTypes;
use reth_primitives_traits::Block;

#[derive(Debug, thiserror::Error)]
pub enum ScrollWireImportError {
    #[error(transparent)]
    Signature(#[from] BlockSignatureError),
    #[error(transparent)]
    Engine(#[from] BeaconOnNewPayloadError),
}

/// Converts verified `scroll/1` block announcements into Engine `new_payload` messages.
#[derive(Debug)]
pub struct DogeosScrollWireEngineImporter {
    manager: ScrollWireManager,
    engine: ConsensusEngineHandle<DogeosEngineTypes>,
    authorized_signer: Option<Address>,
    blocks_seen: LruCache<(alloy_primitives::B256, alloy_primitives::Signature)>,
}

impl DogeosScrollWireEngineImporter {
    pub fn new(
        manager: ScrollWireManager,
        engine: ConsensusEngineHandle<DogeosEngineTypes>,
        authorized_signer: Option<Address>,
    ) -> Self {
        Self {
            manager,
            engine,
            authorized_signer,
            blocks_seen: LruCache::new(LRU_CACHE_SIZE),
        }
    }

    pub async fn import_event(
        &mut self,
        event: ScrollWireEvent,
    ) -> Result<Option<PayloadStatus>, ScrollWireImportError> {
        let ScrollWireEvent::NewBlock {
            block, signature, ..
        } = event
        else {
            return Ok(None);
        };
        verify_block_signature(&block.header, &signature, self.authorized_signer)?;
        let block_key = (block.header.hash_slow(), signature);
        if self.blocks_seen.contains(&block_key) {
            return Ok(None);
        }
        self.blocks_seen.insert(block_key);
        let payload = <DogeosEngineTypes as PayloadTypes>::block_to_payload(block.seal_slow());
        Ok(Some(self.engine.new_payload(payload).await?))
    }

    /// Runs until the task is cancelled, logging rejected announcements without stopping import.
    pub async fn run(mut self) {
        loop {
            let event = (&mut self.manager).await;
            if let Err(error) = self.import_event(event).await {
                tracing::warn!(target: "dogeos::scroll_wire", %error, "failed to import announced block");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Signature, U256};
    use dogeos_reth_primitives::DogeosBlock;
    use reth_engine_primitives::BeaconEngineMessage;
    use reth_network_api::PeerId;

    fn importer(
        authorized_signer: Option<Address>,
    ) -> (
        DogeosScrollWireEngineImporter,
        tokio::sync::mpsc::UnboundedReceiver<BeaconEngineMessage<DogeosEngineTypes>>,
    ) {
        let (_events_tx, events) = tokio::sync::mpsc::unbounded_channel();
        let (engine_tx, engine_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            DogeosScrollWireEngineImporter::new(
                ScrollWireManager::new(events),
                ConsensusEngineHandle::new(engine_tx),
                authorized_signer,
            ),
            engine_rx,
        )
    }

    #[test]
    fn verified_announcement_is_sent_to_engine_as_new_payload() {
        let (mut importer, mut engine_rx) = importer(None);
        let block = DogeosBlock::default();
        let expected_hash = block.header.hash_slow();
        let event = ScrollWireEvent::new_block(
            PeerId::random(),
            block,
            Signature::new(U256::from(1), U256::from(2), false),
        );

        let (result, ()) = futures::executor::block_on(async {
            futures::join!(importer.import_event(event), async {
                let BeaconEngineMessage::NewPayload { payload, tx } =
                    engine_rx.recv().await.unwrap()
                else {
                    panic!("expected new-payload message")
                };
                assert_eq!(payload.block_hash(), expected_hash);
                drop(tx);
            })
        });
        assert!(matches!(result, Err(ScrollWireImportError::Engine(_))));
    }

    #[test]
    fn unauthorized_announcement_never_reaches_engine() {
        let (mut importer, mut engine_rx) = importer(Some(Address::repeat_byte(0x11)));
        let event = ScrollWireEvent::new_block(
            PeerId::random(),
            DogeosBlock::default(),
            Signature::new(U256::from(1), U256::from(2), false),
        );
        let result = futures::executor::block_on(importer.import_event(event));
        assert!(matches!(result, Err(ScrollWireImportError::Signature(_))));
        assert!(matches!(
            engine_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn duplicate_announcement_is_not_sent_to_engine_twice() {
        let (mut importer, mut engine_rx) = importer(None);
        let peer_id = PeerId::random();
        let block = DogeosBlock::default();
        let signature = Signature::new(U256::from(1), U256::from(2), false);

        let first = ScrollWireEvent::new_block(peer_id, block.clone(), signature);
        let (result, ()) = futures::executor::block_on(async {
            futures::join!(importer.import_event(first), async {
                let BeaconEngineMessage::NewPayload { tx, .. } = engine_rx.recv().await.unwrap()
                else {
                    panic!("expected new-payload message")
                };
                drop(tx);
            })
        });
        assert!(matches!(result, Err(ScrollWireImportError::Engine(_))));

        let duplicate = ScrollWireEvent::new_block(peer_id, block, signature);
        assert_eq!(
            futures::executor::block_on(importer.import_event(duplicate)).unwrap(),
            None
        );
        assert!(matches!(
            engine_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}
