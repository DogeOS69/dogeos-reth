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
        /// Tsuki fee and gas-pricing rules: D48/E10 EIP-1559 parameters, the 420,000 gwei
        /// base-fee cap, EIP-7709 BLOCKHASH pricing and the combined L1-fee balance check.
        TsukiV2,
    }
);

/// Placeholder Chikyu TsukiV2 activation (2027-01-01T00:00:00Z); ops sets the real time before
/// rollout. It must match `tsukiV2Time` in the Chikyu genesis document.
pub const CHIKYU_TSUKI_V2_TIMESTAMP: u64 = 1_798_761_600;

impl DogeosHardfork {
    /// DogeOS Chikyu activation schedule from the operator-provided genesis document.
    pub const fn chikyu() -> [(Self, ForkCondition); 5] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(1_790_030_800)),
            (Self::GalileoV2, ForkCondition::Timestamp(1_790_030_800)),
            (Self::Tsuki, ForkCondition::Timestamp(1_790_030_900)),
            (
                Self::TsukiV2,
                ForkCondition::Timestamp(CHIKYU_TSUKI_V2_TIMESTAMP),
            ),
        ]
    }

    /// DogeOS mainnet activation schedule.
    pub const fn mainnet() -> [(Self, ForkCondition); 5] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(0)),
            (Self::GalileoV2, ForkCondition::Timestamp(0)),
            (Self::Tsuki, ForkCondition::Timestamp(0)),
            (Self::TsukiV2, ForkCondition::Timestamp(0)),
        ]
    }

    /// Development-network activation schedule.
    pub const fn dev() -> [(Self, ForkCondition); 5] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(0)),
            (Self::GalileoV2, ForkCondition::Timestamp(0)),
            (Self::Tsuki, ForkCondition::Timestamp(0)),
            (Self::TsukiV2, ForkCondition::Timestamp(0)),
        ]
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

    /// Returns whether TsukiV2 is active at `timestamp`.
    fn is_tsuki_v2_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.dogeos_fork_activation(DogeosHardfork::TsukiV2)
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
        // Match the Ethereum baseline in dogeos-chainspec's feynman_hardforks.
        match fork {
            EthereumHardfork::Frontier
            | EthereumHardfork::Homestead
            | EthereumHardfork::Tangerine
            | EthereumHardfork::SpuriousDragon
            | EthereumHardfork::Byzantium
            | EthereumHardfork::Constantinople
            | EthereumHardfork::Petersburg
            | EthereumHardfork::Istanbul
            | EthereumHardfork::Berlin
            | EthereumHardfork::London => ForkCondition::Block(0),
            EthereumHardfork::Shanghai => ForkCondition::Timestamp(0),
            _ => ForkCondition::Never,
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
    fn mainnet_and_dev_activate_feynman_through_tsuki_v2_at_genesis() {
        for hardforks in [DogeosChainHardforks::mainnet(), DogeosChainHardforks::dev()] {
            assert!(hardforks.is_feynman_active_at_timestamp(0));
            assert!(hardforks.is_tsuki_active_at_timestamp(0));
            assert!(hardforks.is_tsuki_v2_active_at_timestamp(0));
        }
    }

    #[test]
    fn tsuki_v2_is_ordered_directly_after_tsuki() {
        assert!(DogeosHardfork::Tsuki < DogeosHardfork::TsukiV2);
        assert_eq!(DogeosHardfork::TsukiV2.name(), "TsukiV2");
    }

    #[test]
    fn chikyu_schedules_tsuki_v2_at_the_placeholder_after_tsuki() {
        let hardforks = DogeosChainHardforks::chikyu();
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::TsukiV2),
            ForkCondition::Timestamp(CHIKYU_TSUKI_V2_TIMESTAMP)
        );
        const { assert!(CHIKYU_TSUKI_V2_TIMESTAMP > 1_790_030_900) };
        assert!(!hardforks.is_tsuki_v2_active_at_timestamp(1_790_030_900));
        assert!(!hardforks.is_tsuki_v2_active_at_timestamp(CHIKYU_TSUKI_V2_TIMESTAMP - 1));
        assert!(hardforks.is_tsuki_v2_active_at_timestamp(CHIKYU_TSUKI_V2_TIMESTAMP));
    }

    #[test]
    fn chikyu_preserves_the_checkpoint_and_activates_at_network_timestamps() {
        let hardforks = DogeosChainHardforks::chikyu();
        assert!(hardforks.is_feynman_active_at_timestamp(0));
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::Galileo),
            ForkCondition::Timestamp(1_790_030_800)
        );
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::GalileoV2),
            ForkCondition::Timestamp(1_790_030_800)
        );
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::Tsuki),
            ForkCondition::Timestamp(1_790_030_900)
        );
        assert!(!hardforks.is_tsuki_active_at_timestamp(0));
        assert!(!hardforks.is_tsuki_active_at_timestamp(1_785_892_970));
        assert!(!hardforks.is_tsuki_active_at_timestamp(1_790_030_899));
        assert!(hardforks.is_tsuki_active_at_timestamp(1_790_030_900));
        assert!(hardforks.is_tsuki_active_at_timestamp(u64::MAX));
    }

    #[test]
    fn absent_fork_fails_closed() {
        let hardforks =
            DogeosChainHardforks::new([(DogeosHardfork::Feynman, ForkCondition::Timestamp(0))]);
        assert_eq!(
            hardforks.dogeos_fork_activation(DogeosHardfork::Tsuki),
            ForkCondition::Never
        );
        assert!(!hardforks.is_tsuki_v2_active_at_timestamp(u64::MAX));
    }
}
