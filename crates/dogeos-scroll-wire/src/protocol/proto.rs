use alloy_primitives::{
    Signature,
    bytes::{Buf, BufMut, Bytes, BytesMut},
};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use dogeos_reth_primitives::DogeosBlock;
use reth_eth_wire::{Capability, protocol::Protocol};

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScrollMessageId {
    NewBlock = 0,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrollMessagePayload {
    NewBlock(NewBlock),
}

/// A signed block announcement. Its RLP field order is `[signature, block]`.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct NewBlock {
    /// Signature encoded as `r || s || y_parity`, preserving the inherited wire format.
    pub signature: Bytes,
    pub block: DogeosBlock,
}

impl NewBlock {
    pub fn new(signature: Signature, block: DogeosBlock) -> Self {
        Self {
            signature: Bytes::copy_from_slice(&signature.as_rsy()),
            block,
        }
    }
}

impl TryFrom<u8> for ScrollMessageId {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::NewBlock),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScrollMessage {
    pub id: ScrollMessageId,
    pub payload: ScrollMessagePayload,
}

impl ScrollMessage {
    pub const fn capability() -> Capability {
        Capability::new_static("scroll", 1)
    }

    pub const fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 1)
    }

    pub const fn new_block(block: NewBlock) -> Self {
        Self {
            id: ScrollMessageId::NewBlock,
            payload: ScrollMessagePayload::NewBlock(block),
        }
    }

    pub fn encoded(&self) -> BytesMut {
        let mut buffer = BytesMut::new();
        buffer.put_u8(self.id as u8);
        match &self.payload {
            ScrollMessagePayload::NewBlock(block) => block.encode(&mut buffer),
        }
        buffer
    }

    pub fn decode(buffer: &mut &[u8]) -> Option<Self> {
        let id = (*buffer.first()?).try_into().ok()?;
        buffer.advance(1);
        let payload = match id {
            ScrollMessageId::NewBlock => {
                ScrollMessagePayload::NewBlock(NewBlock::decode(buffer).ok()?)
            }
        };
        Some(Self { id, payload })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    fn protocol_identity_is_scroll_v1() {
        assert_eq!(ScrollMessage::capability().name, "scroll");
        assert_eq!(ScrollMessage::capability().version, 1);
        assert_eq!(ScrollMessage::protocol().messages(), 1);
    }

    #[test]
    fn signature_uses_raw_parity_byte() {
        let signature = Signature::new(U256::from(1), U256::from(2), true);
        let announcement = NewBlock::new(signature, DogeosBlock::default());
        assert_eq!(announcement.signature.len(), 65);
        assert_eq!(announcement.signature[64], 1);

        let message = ScrollMessage::new_block(announcement.clone());
        let encoded = message.encoded();
        assert_eq!(encoded[0], ScrollMessageId::NewBlock as u8);
        let decoded = ScrollMessage::decode(&mut &encoded[..]).unwrap();
        assert_eq!(
            decoded.payload,
            ScrollMessagePayload::NewBlock(announcement)
        );
    }
}
