//! Feynman+ DogeOS hardfork policy.

#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(not(feature = "std"))]
extern crate alloc as std;

pub use alloy_hardforks::ForkCondition;
use alloy_hardforks::{EthereumHardfork, EthereumHardforks, hardfork};
use std::vec::Vec;

hardfork!(
    /// DogeOS hardfork identifiers retained by the standalone client.
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    DogeosHardfork {
        /// Feynman is the baseline for every supported DogeOS network.
        Feynman,
        /// Galileo protocol transition.
        Galileo,
        /// Galileo v2 protocol transition.
        GalileoV2,
        /// DogeOS native-token protocol transition.
        Tsuki,
    }
);

impl DogeosHardfork {
    /// DogeOS Chikyu activation schedule.
    pub const fn chikyu() -> [(Self, ForkCondition); 4] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(0)),
            (Self::GalileoV2, ForkCondition::Timestamp(0)),
            (Self::Tsuki, ForkCondition::Timestamp(0)),
        ]
    }

    /// DogeOS mainnet activation schedule.
    pub const fn mainnet() -> [(Self, ForkCondition); 4] {
        Self::chikyu()
    }

    /// Development-network activation schedule.
    pub const fn dev() -> [(Self, ForkCondition); 4] {
        Self::chikyu()
    }
}

/// Extends [`EthereumHardforks`] with DogeOS fork activation helpers.
#[auto_impl::auto_impl(&, std::sync::Arc)]
pub trait DogeosHardforks: EthereumHardforks {
    /// Retrieves the activation condition for a DogeOS hardfork.
    fn dogeos_fork_activation(&self, fork: DogeosHardfork) -> ForkCondition;

    /// Returns whether Feynman is active at `timestamp`.
    fn is_feynman_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.dogeos_fork_activation(DogeosHardfork::Feynman)
            .active_at_timestamp(timestamp)
    }

    /// Returns whether Tsuki is active at `timestamp`.
    fn is_tsuki_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.dogeos_fork_activation(DogeosHardfork::Tsuki)
            .active_at_timestamp(timestamp)
    }
}

/// Ordered DogeOS hardfork activation configuration.
#[derive(Debug, Clone)]
pub struct DogeosChainHardforks {
    forks: Vec<(DogeosHardfork, ForkCondition)>,
}

impl DogeosChainHardforks {
    /// Creates a schedule from fork activation conditions.
    pub fn new(forks: impl IntoIterator<Item = (DogeosHardfork, ForkCondition)>) -> Self {
        let mut forks = forks.into_iter().collect::<Vec<_>>();
        forks.sort();
        Self { forks }
    }

    /// Returns the DogeOS mainnet schedule.
    pub fn mainnet() -> Self {
        Self::new(DogeosHardfork::mainnet())
    }

    /// Returns the DogeOS Chikyu schedule.
    pub fn chikyu() -> Self {
        Self::new(DogeosHardfork::chikyu())
    }

    /// Returns the DogeOS development schedule.
    pub fn dev() -> Self {
        Self::new(DogeosHardfork::dev())
    }
}

impl EthereumHardforks for DogeosChainHardforks {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        if fork <= EthereumHardfork::Shanghai {
            self.dogeos_fork_activation(DogeosHardfork::Feynman)
        } else {
            ForkCondition::Never
        }
    }
}

impl DogeosHardforks for DogeosChainHardforks {
    fn dogeos_fork_activation(&self, fork: DogeosHardfork) -> ForkCondition {
        self.forks
            .binary_search_by(|(configured, _)| configured.cmp(&fork))
            .map(|index| self.forks[index].1)
            .unwrap_or(ForkCondition::Never)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_networks_activate_feynman_and_tsuki_at_genesis() {
        for hardforks in [
            DogeosChainHardforks::mainnet(),
            DogeosChainHardforks::chikyu(),
            DogeosChainHardforks::dev(),
        ] {
            assert!(hardforks.is_feynman_active_at_timestamp(0));
            assert!(hardforks.is_tsuki_active_at_timestamp(0));
        }
    }

    #[test]
    fn absent_fork_fails_closed() {
        let hardforks =
            DogeosChainHardforks::new([(DogeosHardfork::Feynman, ForkCondition::Timestamp(0))]);
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::Tsuki),
            ForkCondition::Never
        );
    }
}
