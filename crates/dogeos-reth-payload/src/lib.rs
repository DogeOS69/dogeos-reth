//! DogeOS payload-building policy and forced-transaction boundary.

mod config;
pub use config::{MIN_TRANSACTION_DATA_SIZE, PayloadBuildingBreaker, ScrollBuilderConfig};
mod forced;
pub use forced::decode_forced_transactions;
pub(crate) use forced::forced_transactions_da_bytes;
mod builder;
pub use builder::{ScrollPayloadBuilder, ScrollPayloadBuilderError};

use alloy_consensus::Transaction;
use alloy_eips::eip2718::Encodable2718;

/// Accumulated resources and priority fees while constructing a payload.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionInfo {
    pub cumulative_gas_used: u64,
    /// Uncompressed EIP-2718 transaction bytes, excluding network RLP wrapping.
    /// This construction budget is not the final encoded blob size.
    pub cumulative_da_bytes_used: u64,
    pub total_fees: alloy_primitives::U256,
}

impl ExecutionInfo {
    pub const fn new() -> Self {
        Self {
            cumulative_gas_used: 0,
            cumulative_da_bytes_used: 0,
            total_fees: alloy_primitives::U256::ZERO,
        }
    }

    /// Returns whether adding `tx` would exceed the gas limit or EIP-2718 byte budget.
    pub fn is_tx_over_limits(
        &self,
        tx: &(impl Encodable2718 + Transaction),
        block_gas_limit: u64,
        block_data_limit: Option<u64>,
    ) -> bool {
        block_data_limit.is_some_and(|limit| {
            self.cumulative_da_bytes_used
                .saturating_add(tx.encode_2718_len() as u64)
                > limit
        }) || self.cumulative_gas_used.saturating_add(tx.gas_limit()) > block_gas_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxLegacy};
    use alloy_primitives::{Bytes, Signature};
    use dogeos_chainspec::MAX_TX_PAYLOAD_BYTES_PER_BLOCK;
    use dogeos_protocol_types::ScrollTxEnvelope;
    use dogeos_reth_engine::ScrollPayloadAttributes;

    fn eip1559_with_encoded_len(len: usize) -> ScrollTxEnvelope {
        let envelope = |input_len| {
            ScrollTxEnvelope::Eip1559(
                TxEip1559 {
                    gas_limit: 3_000_000,
                    input: Bytes::from(vec![0; input_len]),
                    ..Default::default()
                }
                .into_signed(Signature::test_signature()),
            )
        };
        // Both sizes stay above the same RLP length-prefix threshold.
        let overhead = envelope(len).encode_2718_len() - len;
        let tx = envelope(len - overhead);
        assert_eq!(tx.encode_2718_len(), len);
        tx
    }

    #[test]
    fn typed_transactions_fit_through_the_eip2718_limit() {
        let limit = MAX_TX_PAYLOAD_BYTES_PER_BLOCK;
        for len in limit - 4..=limit + 1 {
            let tx = eip1559_with_encoded_len(len);
            assert_eq!(tx.encoded_2718().len(), len);
            assert_eq!(tx.network_len(), len + 4);
            assert_eq!(
                ExecutionInfo::new().is_tx_over_limits(&tx, u64::MAX, Some(limit as u64)),
                len > limit,
                "EIP-2718 transaction length {len}"
            );
        }
    }

    #[test]
    fn pooled_transaction_fits_remaining_eip2718_budget_after_forced_transactions() {
        let limit = MAX_TX_PAYLOAD_BYTES_PER_BLOCK as u64;
        let tx = eip1559_with_encoded_len(limit as usize / 2);
        let forced = decode_forced_transactions(&ScrollPayloadAttributes {
            transactions: Some(vec![tx.encoded_2718().into()]),
            ..Default::default()
        })
        .unwrap();
        let mut info = ExecutionInfo {
            cumulative_da_bytes_used: forced_transactions_da_bytes(&forced),
            ..Default::default()
        };
        assert_eq!(info.cumulative_da_bytes_used, limit / 2);
        assert!(!info.is_tx_over_limits(&tx, u64::MAX, Some(limit)));

        info.cumulative_da_bytes_used += 1;
        assert!(info.is_tx_over_limits(&tx, u64::MAX, Some(limit)));
    }

    #[test]
    fn legacy_transaction_keeps_the_same_byte_boundary() {
        let tx = ScrollTxEnvelope::Legacy(
            TxLegacy {
                gas_limit: 21_000,
                ..Default::default()
            }
            .into_signed(Signature::test_signature()),
        );
        let len = tx.encode_2718_len() as u64;
        assert_eq!(tx.network_len() as u64, len);
        assert!(!ExecutionInfo::new().is_tx_over_limits(&tx, u64::MAX, Some(len)));
        assert!(ExecutionInfo::new().is_tx_over_limits(&tx, u64::MAX, Some(len - 1)));
    }
}
