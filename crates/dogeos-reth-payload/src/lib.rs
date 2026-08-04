//! DogeOS payload-building policy and forced-transaction boundary.

mod config;
pub use config::{MIN_TRANSACTION_DATA_SIZE, PayloadBuildingBreaker, ScrollBuilderConfig};
mod forced;
pub use forced::decode_forced_transactions;
pub(crate) use forced::forced_transactions_da_bytes;
mod builder;
pub use builder::{ScrollPayloadBuilder, ScrollPayloadBuilderError};

use alloy_consensus::Transaction;
use alloy_rlp::Encodable;

/// Accumulated resources and priority fees while constructing a payload.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionInfo {
    pub cumulative_gas_used: u64,
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

    /// Returns whether adding `tx` would exceed either consensus construction limit.
    pub fn is_tx_over_limits(
        &self,
        tx: &(impl Encodable + Transaction),
        block_gas_limit: u64,
        block_data_limit: Option<u64>,
    ) -> bool {
        block_data_limit.is_some_and(|limit| {
            self.cumulative_da_bytes_used
                .saturating_add(tx.length() as u64)
                > limit
        }) || self.cumulative_gas_used.saturating_add(tx.gas_limit()) > block_gas_limit
    }
}
