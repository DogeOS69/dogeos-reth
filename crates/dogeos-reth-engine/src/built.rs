use alloc::sync::Arc;
use core::iter;

use alloy_eips::eip7685::Requests;
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadFieldV2, ExecutionPayloadV1, ExecutionPayloadV3,
};
use dogeos_reth_primitives::{DogeosBlock, DogeosPrimitives};
use reth_payload_primitives::{BuiltPayload, BuiltPayloadExecutedBlock};
use reth_primitives_traits::SealedBlock;

/// A built DogeOS execution payload.
#[derive(Debug, Clone, Default)]
pub struct ScrollBuiltPayload {
    block: Arc<SealedBlock<DogeosBlock>>,
    executed_block: Option<BuiltPayloadExecutedBlock<DogeosPrimitives>>,
    fees: U256,
}

impl ScrollBuiltPayload {
    pub const fn new(
        block: Arc<SealedBlock<DogeosBlock>>,
        executed_block: Option<BuiltPayloadExecutedBlock<DogeosPrimitives>>,
        fees: U256,
    ) -> Self {
        Self {
            block,
            executed_block,
            fees,
        }
    }

    pub fn into_sealed_block(self) -> SealedBlock<DogeosBlock> {
        Arc::unwrap_or_clone(self.block)
    }
}

impl BuiltPayload for ScrollBuiltPayload {
    type Primitives = DogeosPrimitives;

    fn block(&self) -> &SealedBlock<DogeosBlock> {
        &self.block
    }

    fn fees(&self) -> U256 {
        self.fees
    }

    fn executed_block(&self) -> Option<BuiltPayloadExecutedBlock<Self::Primitives>> {
        self.executed_block.clone()
    }

    fn requests(&self) -> Option<Requests> {
        None
    }
}

impl From<ScrollBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: ScrollBuiltPayload) -> Self {
        Self::from_block_unchecked(
            value.block.hash(),
            &Arc::unwrap_or_clone(value.block).into_block(),
        )
    }
}

impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: ScrollBuiltPayload) -> Self {
        Self {
            block_value: value.fees,
            execution_payload: ExecutionPayloadFieldV2::from_block_unchecked(
                value.block.hash(),
                &Arc::unwrap_or_clone(value.block).into_block(),
            ),
        }
    }
}

impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: ScrollBuiltPayload) -> Self {
        Self {
            execution_payload: ExecutionPayloadV3::from_block_unchecked(
                value.block.hash(),
                &Arc::unwrap_or_clone(value.block).into_block(),
            ),
            block_value: value.fees,
            should_override_builder: false,
            blobs_bundle: BlobsBundleV1::new(iter::empty()),
        }
    }
}

impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    fn from(value: ScrollBuiltPayload) -> Self {
        Self {
            envelope_inner: value.into(),
            execution_requests: Default::default(),
        }
    }
}
