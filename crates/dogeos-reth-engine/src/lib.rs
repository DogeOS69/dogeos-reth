//! DogeOS Engine API and built-payload types.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod attributes;
pub use attributes::{BlockDataHint, ScrollPayloadAttributes};
mod built;
pub use built::ScrollBuiltPayload;

use core::marker::PhantomData;

use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadV1,
};
use dogeos_reth_primitives::DogeosBlock;
use reth_engine_primitives::EngineTypes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};

/// Engine types used by the standalone DogeOS node.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct DogeosEngineTypes<T: PayloadTypes = DogeosPayloadTypes> {
    _marker: PhantomData<T>,
}

impl<T> PayloadTypes for DogeosEngineTypes<T>
where
    T: PayloadTypes<
            ExecutionData = ExecutionData,
            BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = DogeosBlock>>,
        >,
{
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        T::block_to_payload(block)
    }
}

impl<T> EngineTypes for DogeosEngineTypes<T>
where
    T: PayloadTypes<ExecutionData = ExecutionData>,
    T::BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = DogeosBlock>>
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<ExecutionPayloadEnvelopeV3>
        + TryInto<ExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;
    // DogeOS is Feynman+ without Prague/Osaka payload fields. Keep the latest accepted envelope
    // shape at V4 until those forks are deliberately introduced.
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV4;
    type ExecutionPayloadEnvelopeV6 = ExecutionPayloadEnvelopeV4;
}

/// Default DogeOS payload family.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct DogeosPayloadTypes;

impl PayloadTypes for DogeosPayloadTypes {
    type ExecutionData = ExecutionData;
    type BuiltPayload = ScrollBuiltPayload;
    type PayloadAttributes = ScrollPayloadAttributes;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> Self::ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}
