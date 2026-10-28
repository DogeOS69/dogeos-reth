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
    /// DogeOS Chikyu activation schedule from the operator-provided genesis document.
    pub const fn chikyu() -> [(Self, ForkCondition); 4] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(1_790_030_800)),
            (Self::GalileoV2, ForkCondition::Timestamp(1_790_030_800)),
            (Self::Tsuki, ForkCondition::Timestamp(1_790_030_900)),
        ]
    }

    /// DogeOS mainnet activation schedule.
    pub const fn mainnet() -> [(Self, ForkCondition); 4] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(0)),
            (Self::GalileoV2, ForkCondition::Timestamp(0)),
            (Self::Tsuki, ForkCondition::Timestamp(0)),
        ]
    }

    /// Development-network activation schedule.
    pub const fn dev() -> [(Self, ForkCondition); 4] {
        [
            (Self::Feynman, ForkCondition::Timestamp(0)),
            (Self::Galileo, ForkCondition::Timestamp(0)),
            (Self::GalileoV2, ForkCondition::Timestamp(0)),
            (Self::Tsuki, ForkCondition::Timestamp(0)),
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

    /// Activation of the Tsuki fee parameters: D48/E10 and the 420,000 gwei base-fee cap.
    ///
    /// Required, so every chain spec states it: the cap must switch together with the
    /// parameters its base-fee schedule uses.
    fn tsuki_fee_params_activation(&self) -> ForkCondition;

    /// Returns whether the Tsuki fee parameters are active at `timestamp`.
    fn is_tsuki_fee_params_active_at_timestamp(&self, timestamp: u64) -> bool {
        self.tsuki_fee_params_activation()
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

    /// The plain schedule has no separate fee-parameter time, so the parameters follow Tsuki.
    fn tsuki_fee_params_activation(&self) -> ForkCondition {
        self.dogeos_fork_activation(DogeosHardfork::Tsuki)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_and_dev_activate_feynman_and_tsuki_at_genesis() {
        for hardforks in [DogeosChainHardforks::mainnet(), DogeosChainHardforks::dev()] {
            assert!(hardforks.is_feynman_active_at_timestamp(0));
            assert!(hardforks.is_tsuki_active_at_timestamp(0));
        }
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
    fn fee_params_default_to_the_tsuki_activation() {
        for hardforks in [
            DogeosChainHardforks::mainnet(),
            DogeosChainHardforks::chikyu(),
            DogeosChainHardforks::dev(),
        ] {
            assert_eq!(
                hardforks.tsuki_fee_params_activation(),
                hardforks.dogeos_fork_activation(DogeosHardfork::Tsuki)
            );
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
