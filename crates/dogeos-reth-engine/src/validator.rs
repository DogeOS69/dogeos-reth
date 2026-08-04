use crate::{DogeosEngineTypes, ScrollPayloadAttributes};
use alloc::sync::Arc;
use alloy_consensus::BlockHeader;
use alloy_primitives::U256;
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
use dogeos_reth_primitives::DogeosBlock;
use reth_engine_primitives::{EngineApiValidator, PayloadValidator};
use reth_payload_primitives::{
    EngineApiMessageVersion, EngineObjectValidationError, InvalidPayloadAttributesError,
    MessageValidationKind, NewPayloadError, PayloadAttributes, PayloadOrAttributes,
    VersionSpecificValidationError,
};
use reth_primitives_traits::{Block, SealedBlock};

/// Post-Feynman Engine API validator for DogeOS.
#[derive(Debug, Clone)]
pub struct DogeosEngineValidator<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> DogeosEngineValidator<ChainSpec> {
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec> EngineApiValidator<DogeosEngineTypes> for DogeosEngineValidator<ChainSpec>
where
    ChainSpec: Send + Sync + Unpin + 'static,
{
    fn validate_version_specific_fields(
        &self,
        _version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, ExecutionData, ScrollPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_dogeos_fields(
            &payload_or_attrs,
            payload_or_attrs.message_validation_kind(),
        )
    }

    fn ensure_well_formed_attributes(
        &self,
        _version: EngineApiMessageVersion,
        attributes: &ScrollPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_dogeos_fields(
            &PayloadOrAttributes::<ExecutionData, _>::from_attributes(attributes),
            MessageValidationKind::PayloadAttributes,
        )
    }
}

fn validate_dogeos_fields<Payload: reth_payload_primitives::ExecutionPayload>(
    payload_or_attrs: &PayloadOrAttributes<'_, Payload, ScrollPayloadAttributes>,
    kind: MessageValidationKind,
) -> Result<(), EngineObjectValidationError> {
    if payload_or_attrs.parent_beacon_block_root().is_some() {
        return Err(kind
            .to_error(VersionSpecificValidationError::ParentBeaconBlockRootNotSupportedBeforeV3));
    }
    if payload_or_attrs.withdrawals().is_some() {
        return Err(kind.to_error(VersionSpecificValidationError::HasWithdrawalsPreShanghai));
    }
    Ok(())
}

impl<ChainSpec> PayloadValidator<DogeosEngineTypes> for DogeosEngineValidator<ChainSpec>
where
    ChainSpec: Send + Sync + Unpin + 'static,
{
    type Block = DogeosBlock;

    fn convert_payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Self::Block>, NewPayloadError> {
        let expected_hash = payload.payload.block_hash();
        let ExecutionData { payload, sidecar } = payload;
        let mut block = payload.try_into_block_with_sidecar(&sidecar)?;

        // Difficulty is not carried by the Engine payload. The supported post-Euclid baseline
        // fixes it to one; do not reconstruct legacy Clique difficulty-two headers.
        block.header.difficulty = U256::ONE;
        let block_hash = block.hash_slow();
        if block_hash == expected_hash {
            return Ok(block.seal_unchecked(block_hash));
        }

        Err(PayloadError::BlockHash {
            execution: block_hash,
            consensus: expected_hash,
        }
        .into())
    }

    fn validate_payload_attributes_against_header(
        &self,
        attributes: &ScrollPayloadAttributes,
        header: &<Self::Block as Block>::Header,
    ) -> Result<(), InvalidPayloadAttributesError> {
        // DogeOS permits multiple L2 blocks at the same L1-derived timestamp.
        if attributes.timestamp() < header.timestamp() {
            return Err(InvalidPayloadAttributesError::InvalidTimestamp);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH, Header};

    #[test]
    fn equal_timestamp_is_valid() {
        let validator = DogeosEngineValidator::new(Arc::new(()));
        let attributes = ScrollPayloadAttributes {
            payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                timestamp: 42,
                ..Default::default()
            },
            ..Default::default()
        };
        let header = Header {
            timestamp: 42,
            ..Default::default()
        };

        assert!(
            <DogeosEngineValidator<()> as PayloadValidator<DogeosEngineTypes>>::
                validate_payload_attributes_against_header(&validator, &attributes, &header)
                .is_ok()
        );
    }

    #[test]
    fn earlier_timestamp_is_rejected() {
        let validator = DogeosEngineValidator::new(Arc::new(()));
        let attributes = ScrollPayloadAttributes {
            payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                timestamp: 41,
                ..Default::default()
            },
            ..Default::default()
        };
        let header = Header {
            timestamp: 42,
            ..Default::default()
        };

        assert!(
            <DogeosEngineValidator<()> as PayloadValidator<DogeosEngineTypes>>::
                validate_payload_attributes_against_header(&validator, &attributes, &header)
                .is_err()
        );
    }

    #[test]
    fn engine_reconstructs_only_post_euclid_difficulty() {
        let validator = DogeosEngineValidator::new(Arc::new(()));
        let mut block = DogeosBlock::default();
        block.header.ommers_hash = EMPTY_OMMER_ROOT_HASH;
        block.header.transactions_root = EMPTY_ROOT_HASH;
        block.header.base_fee_per_gas = Some(0);
        block.header.difficulty = U256::ONE;
        let payload = ExecutionData::from_block_unchecked(block.header.hash_slow(), &block);
        let converted = <DogeosEngineValidator<()> as PayloadValidator<DogeosEngineTypes>>::
            convert_payload_to_block(&validator, payload)
            .unwrap();
        assert_eq!(converted.difficulty, U256::ONE);

        block.header.difficulty = U256::from(2);
        let legacy_payload = ExecutionData::from_block_unchecked(block.header.hash_slow(), &block);
        assert!(
            <DogeosEngineValidator<()> as PayloadValidator<DogeosEngineTypes>>::
                convert_payload_to_block(&validator, legacy_payload)
                .is_err()
        );
    }
}
