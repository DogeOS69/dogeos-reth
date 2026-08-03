//! Consensus checks owned by the standalone Feynman+ DogeOS node.

use alloy_consensus::{
    BlockHeader as _, EMPTY_OMMER_ROOT_HASH, TxReceipt, proofs::calculate_receipt_root,
};
use alloy_primitives::{B256, Bloom, U256};
use dogeos_protocol_types::ScrollTransaction;
use dogeos_reth_primitives::{DogeosBlock, DogeosPrimitives, ScrollReceipt};
use reth_consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator, ReceiptRootBloom};
use reth_consensus_common::validation::{
    validate_against_parent_hash_number, validate_body_against_header, validate_header_gas,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{
    BlockBody as _, GotExpected, RecoveredBlock, SealedBlock, SealedHeader,
    constants::{GAS_LIMIT_BOUND_DIVISOR, MINIMUM_GAS_LIMIT},
    receipt::gas_spent_by_transactions,
};

pub const DOGEOS_MAXIMUM_BASE_FEE: u64 = 10_000_000_000;
pub const LEGACY_IN_TURN_DIFFICULTY: U256 = U256::from_limbs([2, 0, 0, 0]);
pub const LEGACY_NO_TURN_DIFFICULTY: U256 = U256::from_limbs([1, 0, 0, 0]);

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DogeosConsensusError {
    #[error("invalid L1 message order")]
    InvalidL1MessageOrder,
    #[error("block mix hash is not zero: {0:?}")]
    MixHashNotZero(Option<B256>),
    #[error("block difficulty must be one or two: {0}")]
    InvalidDifficulty(U256),
    #[error("base fee missing")]
    BaseFeeMissing,
    #[error("base fee exceeds {DOGEOS_MAXIMUM_BASE_FEE}")]
    BaseFeeOverLimit,
    #[error("withdrawals are not supported")]
    WithdrawalsPresent,
    #[error("blob fields are not supported")]
    BlobFieldsPresent,
}

impl From<DogeosConsensusError> for ConsensusError {
    fn from(value: DogeosConsensusError) -> Self {
        Self::Other(value.to_string())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DogeosConsensus;

impl FullConsensus<DogeosPrimitives> for DogeosConsensus {
    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<DogeosBlock>,
        result: &BlockExecutionResult<ScrollReceipt>,
        receipt_root_bloom: Option<ReceiptRootBloom>,
    ) -> Result<(), ConsensusError> {
        let cumulative_gas_used = result
            .receipts
            .last()
            .map(TxReceipt::cumulative_gas_used)
            .unwrap_or_default();
        if block.gas_used() != cumulative_gas_used {
            return Err(ConsensusError::BlockGasUsed {
                gas: GotExpected {
                    got: cumulative_gas_used,
                    expected: block.gas_used(),
                },
                gas_spent_by_tx: gas_spent_by_transactions(&result.receipts),
            });
        }

        let (receipts_root, logs_bloom) = if let Some(value) = receipt_root_bloom {
            value
        } else {
            let receipts = result
                .receipts
                .iter()
                .map(TxReceipt::with_bloom_ref)
                .collect::<Vec<_>>();
            let root = calculate_receipt_root(&receipts);
            let bloom = receipts
                .iter()
                .fold(Bloom::ZERO, |bloom, receipt| bloom | receipt.bloom_ref());
            (root, bloom)
        };
        if receipts_root != block.receipts_root() {
            return Err(ConsensusError::BodyReceiptRootDiff(
                GotExpected {
                    got: receipts_root,
                    expected: block.receipts_root(),
                }
                .into(),
            ));
        }
        if logs_bloom != block.logs_bloom() {
            return Err(ConsensusError::BodyBloomLogDiff(
                GotExpected {
                    got: logs_bloom,
                    expected: block.logs_bloom(),
                }
                .into(),
            ));
        }
        Ok(())
    }
}

impl Consensus<DogeosBlock> for DogeosConsensus {
    fn validate_body_against_header(
        &self,
        body: &<DogeosBlock as reth_primitives_traits::Block>::Body,
        header: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), ConsensusError> {
        validate_body_against_header(body, header.header())
    }

    fn validate_block_pre_execution(
        &self,
        block: &SealedBlock<DogeosBlock>,
    ) -> Result<(), ConsensusError> {
        if block
            .body()
            .ommers()
            .is_some_and(|ommers| !ommers.is_empty())
        {
            return Err(ConsensusError::Other("uncles not allowed".into()));
        }
        if block.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
            return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty);
        }
        block
            .ensure_transaction_root_valid()
            .map_err(|error| ConsensusError::BodyTransactionRootDiff(error.into()))?;
        if block.body().withdrawals().is_some() {
            return Err(DogeosConsensusError::WithdrawalsPresent.into());
        }
        validate_l1_messages(block.body().transactions())?;
        Ok(())
    }
}

impl HeaderValidator<alloy_consensus::Header> for DogeosConsensus {
    fn validate_header(
        &self,
        header: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), ConsensusError> {
        let header = header.header();
        if header.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
            return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty);
        }
        if header.mix_hash() != Some(B256::ZERO) {
            return Err(DogeosConsensusError::MixHashNotZero(header.mix_hash()).into());
        }
        if header.difficulty() != LEGACY_NO_TURN_DIFFICULTY
            && header.difficulty() != LEGACY_IN_TURN_DIFFICULTY
        {
            return Err(DogeosConsensusError::InvalidDifficulty(header.difficulty()).into());
        }
        let Some(base_fee) = header.base_fee_per_gas() else {
            return Err(DogeosConsensusError::BaseFeeMissing.into());
        };
        if base_fee > DOGEOS_MAXIMUM_BASE_FEE {
            return Err(DogeosConsensusError::BaseFeeOverLimit.into());
        }
        if header.withdrawals_root().is_some()
            || header.blob_gas_used().is_some()
            || header.excess_blob_gas().is_some()
            || header.parent_beacon_block_root().is_some()
            || header.requests_hash().is_some()
        {
            return Err(DogeosConsensusError::BlobFieldsPresent.into());
        }
        validate_header_gas(header)?;
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<alloy_consensus::Header>,
        parent: &SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header.header(), parent)?;
        if header.timestamp() < parent.timestamp() {
            return Err(ConsensusError::TimestampIsInPast {
                parent_timestamp: parent.timestamp(),
                timestamp: header.timestamp(),
            });
        }
        validate_parent_gas_limit(header.header(), parent.header())
    }
}

pub fn validate_l1_messages<'a>(
    transactions: impl IntoIterator<Item = &'a dogeos_reth_primitives::ScrollTransactionSigned>,
) -> Result<(), DogeosConsensusError> {
    let mut saw_l2 = false;
    let mut next_queue_index = None;
    for transaction in transactions {
        if transaction.is_l1_message() {
            if saw_l2 {
                return Err(DogeosConsensusError::InvalidL1MessageOrder);
            }
            let queue_index = transaction
                .queue_index()
                .expect("L1 message has queue index");
            if next_queue_index.is_some_and(|expected| queue_index != expected) {
                return Err(DogeosConsensusError::InvalidL1MessageOrder);
            }
            next_queue_index = Some(queue_index + 1);
        } else {
            saw_l2 = true;
        }
    }
    Ok(())
}

fn validate_parent_gas_limit(
    header: &alloy_consensus::Header,
    parent: &alloy_consensus::Header,
) -> Result<(), ConsensusError> {
    let diff = header.gas_limit().abs_diff(parent.gas_limit());
    let limit = parent.gas_limit() / GAS_LIMIT_BOUND_DIVISOR;
    if diff > limit {
        return if header.gas_limit() > parent.gas_limit() {
            Err(ConsensusError::GasLimitInvalidIncrease {
                parent_gas_limit: parent.gas_limit(),
                child_gas_limit: header.gas_limit(),
            })
        } else {
            Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit: parent.gas_limit(),
                child_gas_limit: header.gas_limit(),
            })
        };
    }
    if header.gas_limit() < MINIMUM_GAS_LIMIT {
        return Err(ConsensusError::GasLimitInvalidMinimum {
            child_gas_limit: header.gas_limit(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Signed, TxEip1559};
    use alloy_primitives::Signature;
    use dogeos_protocol_types::TxL1Message;

    fn l2() -> dogeos_reth_primitives::ScrollTransactionSigned {
        Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            B256::ZERO,
        )
        .into()
    }

    fn l1(index: u64) -> dogeos_reth_primitives::ScrollTransactionSigned {
        TxL1Message {
            queue_index: index,
            ..Default::default()
        }
        .into()
    }

    #[test]
    fn l1_messages_must_be_a_sequential_prefix() {
        assert!(validate_l1_messages(&[l1(7), l1(8), l2()]).is_ok());
        assert_eq!(
            validate_l1_messages(&[l1(7), l1(9)]),
            Err(DogeosConsensusError::InvalidL1MessageOrder)
        );
        assert_eq!(
            validate_l1_messages(&[l2(), l1(7)]),
            Err(DogeosConsensusError::InvalidL1MessageOrder)
        );
    }

    #[test]
    fn equal_parent_timestamp_is_valid() {
        let parent = SealedHeader::seal_slow(alloy_consensus::Header {
            gas_limit: 20_000_000,
            timestamp: 10,
            ..Default::default()
        });
        let child = SealedHeader::seal_slow(alloy_consensus::Header {
            parent_hash: parent.hash(),
            number: 1,
            gas_limit: 20_000_000,
            timestamp: 10,
            ..Default::default()
        });
        assert!(
            DogeosConsensus
                .validate_header_against_parent(&child, &parent)
                .is_ok()
        );
    }
}
