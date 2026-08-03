use core::time::Duration;
use reth_chainspec::MIN_TRANSACTION_GAS;
use std::time::Instant;

/// Minimal encoded DA size reserved when deciding whether another transaction may be attempted.
pub const MIN_TRANSACTION_DATA_SIZE: u64 = 115;

/// Limits applied to one payload build attempt.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ScrollBuilderConfig {
    pub gas_limit: Option<u64>,
    pub time_limit: Duration,
    pub max_da_block_size: Option<u64>,
}

impl ScrollBuilderConfig {
    pub const fn new(
        gas_limit: Option<u64>,
        time_limit: Duration,
        max_da_block_size: Option<u64>,
    ) -> Self {
        Self {
            gas_limit,
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
}
