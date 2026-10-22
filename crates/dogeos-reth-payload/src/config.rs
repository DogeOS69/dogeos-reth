use alloy_eips::eip1559::calculate_block_gas_limit;
use core::time::Duration;
use dogeos_reth_evm::SequencerBaseFeePolicy;
use reth_chainspec::MIN_TRANSACTION_GAS;
use std::time::Instant;

/// Minimal encoded DA size reserved when deciding whether another transaction may be attempted.
pub const MIN_TRANSACTION_DATA_SIZE: u64 = 115;

/// Moves a block gas limit toward the producer's desired value without violating its parent bound.
pub fn next_block_gas_limit(parent_gas_limit: u64, desired_gas_limit: u64) -> u64 {
    calculate_block_gas_limit(parent_gas_limit, desired_gas_limit)
}

/// Limits applied to one payload build attempt.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ScrollBuilderConfig {
    /// Desired gas limit; payload production approaches it within the parent-valid bound.
    pub gas_limit: Option<u64>,
    pub base_fee_policy: SequencerBaseFeePolicy,
    pub time_limit: Duration,
    pub max_da_block_size: Option<u64>,
}

impl ScrollBuilderConfig {
    pub const fn new(
        gas_limit: Option<u64>,
        base_fee_policy: SequencerBaseFeePolicy,
        time_limit: Duration,
        max_da_block_size: Option<u64>,
    ) -> Self {
        Self {
            gas_limit,
            base_fee_policy,
            time_limit,
            max_da_block_size,
        }
    }

    pub fn breaker(&self) -> PayloadBuildingBreaker {
        PayloadBuildingBreaker::new(self.time_limit, self.gas_limit, self.max_da_block_size)
    }
}

/// Stops pool transaction execution before the configured resource boundary is crossed.
#[derive(Debug, Clone)]
pub struct PayloadBuildingBreaker {
    start: Instant,
    time_limit: Duration,
    gas_limit: Option<u64>,
    max_da_block_size: Option<u64>,
}

impl PayloadBuildingBreaker {
    pub fn new(
        time_limit: Duration,
        gas_limit: Option<u64>,
        max_da_block_size: Option<u64>,
    ) -> Self {
        Self {
            start: Instant::now(),
            time_limit,
            gas_limit,
            max_da_block_size,
        }
    }

    pub fn should_break(&self, cumulative_gas_used: u64, cumulative_da_size_used: u64) -> bool {
        self.start.elapsed() >= self.time_limit
            || self.gas_limit.is_some_and(|limit| {
                cumulative_gas_used > limit.saturating_sub(MIN_TRANSACTION_GAS)
            })
            || self.max_da_block_size.is_some_and(|limit| {
                cumulative_da_size_used > limit.saturating_sub(MIN_TRANSACTION_DATA_SIZE)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_enforces_gas_and_da_reserves() {
        let breaker = PayloadBuildingBreaker::new(
            Duration::from_secs(60),
            Some(2 * MIN_TRANSACTION_GAS),
            Some(2 * MIN_TRANSACTION_DATA_SIZE),
        );
        assert!(!breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS + 1, MIN_TRANSACTION_DATA_SIZE));
        assert!(breaker.should_break(MIN_TRANSACTION_GAS, MIN_TRANSACTION_DATA_SIZE + 1));
    }

    #[test]
    fn zero_duration_stops_immediately() {
        let breaker = PayloadBuildingBreaker::new(Duration::ZERO, None, None);
        assert!(breaker.should_break(0, 0));
    }

    #[test]
    fn gas_limit_ramps_from_twenty_to_thirty_million_without_overshoot() {
        let mut gas_limit = 20_000_000;
        let mut blocks = 0;
        while gas_limit != 30_000_000 {
            let next = next_block_gas_limit(gas_limit, 30_000_000);
            assert!(next > gas_limit);
            assert!(next <= 30_000_000);
            assert!(next - gas_limit < gas_limit / 1024);
            gas_limit = next;
            blocks += 1;
        }
        assert_eq!(blocks, 416);
    }
}
