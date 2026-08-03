use crate::{ScrollTxCompressionInfo, spec_id_at_timestamp_and_number};
use alloy_evm::block::BlockExecutionError;
use alloy_primitives::U256;
use dogeos_hardforks::DogeosHardforks;
use revm_scroll::l1block::L1BlockInfo;

/// Reth-facing L1 data-fee calculation for `revm-scroll` block information.
pub trait RethL1BlockInfo {
    fn l1_tx_data_fee(
        &mut self,
        chain_spec: impl DogeosHardforks,
        timestamp: u64,
        block_number: u64,
        input: &[u8],
        compression_info: Option<ScrollTxCompressionInfo>,
        is_l1_message: bool,
    ) -> Result<U256, BlockExecutionError>;
}

impl RethL1BlockInfo for L1BlockInfo {
    fn l1_tx_data_fee(
        &mut self,
        chain_spec: impl DogeosHardforks,
        timestamp: u64,
        block_number: u64,
        input: &[u8],
        compression_info: Option<ScrollTxCompressionInfo>,
        is_l1_message: bool,
    ) -> Result<U256, BlockExecutionError> {
        if is_l1_message {
            return Ok(U256::ZERO);
        }
        let (compression_ratio, compressed_size) = compression_info
            .map(|(ratio, size)| (Some(ratio), Some(size)))
            .unwrap_or_default();
        let spec = spec_id_at_timestamp_and_number(timestamp, block_number, chain_spec);
        Ok(self.calculate_tx_l1_cost(input, spec, compression_ratio, compressed_size))
    }
}
