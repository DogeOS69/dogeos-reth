//! Feynman+ DogeOS chain specifications.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use alloy_chains::Chain;
use alloy_consensus::Header;
use alloy_eips::eip7840::BlobParams;
use alloy_genesis::Genesis;
use alloy_primitives::{B256, U256};
use derive_more::{Constructor, Deref, Into};
use dogeos_hardforks::{DogeosHardfork, DogeosHardforks};
use reth_chainspec::{
    BaseFeeParams, BaseFeeParamsKind, ChainSpec, ChainSpecBuilder, DepositContract, EthChainSpec,
    EthereumHardforks, ForkFilter, ForkFilterKey, ForkId, Hardforks, Head,
};
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};
use reth_network_peers::NodeRecord;
use reth_primitives_traits::SealedHeader;

#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
#[cfg(feature = "std")]
use std::sync::LazyLock;

mod constants;
pub use constants::*;
mod genesis;
pub use genesis::{DogeosHardforkInfo, L1Config, ScrollChainConfig};

mod chikyu;
pub use chikyu::DOGEOS_CHIKYU;
mod dev;
pub use dev::DOGEOS_DEV;
mod dogeos;
pub use dogeos::DOGEOS_MAINNET;

pub use reth_chainspec::ChainSpecProvider;

/// Builder for a Feynman+ DogeOS chain specification.
#[derive(Debug, Default)]
pub struct DogeosChainSpecBuilder {
    inner: ChainSpecBuilder,
}

impl DogeosChainSpecBuilder {
    pub fn dogeos_mainnet() -> Self {
        Self::from_spec(&DOGEOS_MAINNET)
    }

    pub fn dogeos_chikyu() -> Self {
        Self::from_spec(&DOGEOS_CHIKYU)
    }

    pub fn dev() -> Self {
        Self::from_spec(&DOGEOS_DEV)
    }

    fn from_spec(spec: &DogeosChainSpec) -> Self {
        Self {
            inner: ChainSpecBuilder::default()
                .chain(spec.chain)
                .genesis(spec.genesis.clone())
                .with_forks(spec.hardforks.clone()),
        }
    }

    pub fn chain(mut self, chain: Chain) -> Self {
        self.inner = self.inner.chain(chain);
        self
    }

    pub fn genesis(mut self, genesis: Genesis) -> Self {
        self.inner = self.inner.genesis(genesis);
        self
    }

    pub fn with_fork<H: Hardfork>(mut self, fork: H, condition: ForkCondition) -> Self {
        self.inner = self.inner.with_fork(fork, condition);
        self
    }

    pub fn build(self, config: ScrollChainConfig) -> DogeosChainSpec {
        DogeosChainSpec {
            inner: self.inner.build(),
            config,
        }
    }
}

/// Returns the chain-specific sequencer configuration.
pub trait ChainConfig {
    type Config;
    fn chain_config(&self) -> &Self::Config;
}

impl<T> ChainConfig for Arc<T>
where
    T: ChainConfig + ?Sized,
{
    type Config = T::Config;

    fn chain_config(&self) -> &Self::Config {
        (**self).chain_config()
    }
}

/// DogeOS Reth chain spec with the inherited external Scroll genesis fields.
#[derive(Debug, Clone, Deref, Into, Constructor, PartialEq, Eq)]
pub struct DogeosChainSpec {
    #[deref]
    pub inner: ChainSpec,
    pub config: ScrollChainConfig,
}

impl DogeosChainSpec {
    /// Builds a supported Feynman+ custom chain from its genesis document.
    pub fn from_custom_genesis(genesis: Genesis) -> Self {
        let config = ScrollChainConfig::extract_from(&genesis.config.extra_fields)
            .expect("custom DogeOS genesis must contain the inherited scroll config");
        let dogeos_forks = DogeosHardforkInfo::try_from(&genesis.config.extra_fields)
            .expect("custom DogeOS genesis hardfork timestamps must be valid")
            .activation_schedule();
        build_spec(genesis, config, dogeos_forks, None, None)
    }

    /// Preserve legacy peer compatibility through Tsuki; only later timestamp forks contribute.
    fn fork_filter_keys(&self) -> impl Iterator<Item = ForkFilterKey> + '_ {
        // An unscheduled Tsuki provides no timestamp boundary for subsequent forks.
        let tsuki = self.fork(DogeosHardfork::Tsuki).as_timestamp();
        self.forks_iter()
            .filter_map(move |(_, condition)| match condition {
                ForkCondition::Block(block)
                | ForkCondition::TTD {
                    fork_block: Some(block),
                    ..
                } => Some(ForkFilterKey::Block(block)),
                ForkCondition::Timestamp(time)
                    if tsuki.is_some_and(|tsuki| time > tsuki)
                        && time > self.genesis_timestamp() =>
                {
                    Some(ForkFilterKey::Time(time))
                }
                _ => None,
            })
    }
}

impl ChainConfig for DogeosChainSpec {
    type Config = ScrollChainConfig;

    fn chain_config(&self) -> &Self::Config {
        &self.config
    }
}

impl EthChainSpec for DogeosChainSpec {
    type Header = Header;

    fn chain(&self) -> Chain {
        self.inner.chain()
    }
    fn base_fee_params_at_timestamp(&self, timestamp: u64) -> BaseFeeParams {
        self.inner.base_fee_params_at_timestamp(timestamp)
    }
    fn blob_params_at_timestamp(&self, timestamp: u64) -> Option<BlobParams> {
        self.inner.blob_params_at_timestamp(timestamp)
    }
    fn deposit_contract(&self) -> Option<&DepositContract> {
        self.inner.deposit_contract()
    }
    fn genesis_hash(&self) -> B256 {
        self.inner.genesis_hash()
    }
    fn prune_delete_limit(&self) -> usize {
        self.inner.prune_delete_limit()
    }
    fn display_hardforks(&self) -> Box<dyn alloc::fmt::Display> {
        Box::new(self.inner.display_hardforks())
    }
    fn genesis_header(&self) -> &Header {
        self.inner.genesis_header()
    }
    fn genesis(&self) -> &Genesis {
        self.inner.genesis()
    }
    fn bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.inner.bootnodes()
    }
    fn final_paris_total_difficulty(&self) -> Option<U256> {
        self.inner.final_paris_total_difficulty()
    }
}

impl Hardforks for DogeosChainSpec {
    fn fork<H: Hardfork>(&self, fork: H) -> ForkCondition {
        self.inner.fork(fork)
    }
    fn forks_iter(&self) -> impl Iterator<Item = (&dyn Hardfork, ForkCondition)> {
        self.inner.forks_iter()
    }
    fn fork_id(&self, head: &Head) -> ForkId {
        // ForkFilter sorts and deduplicates activation keys, keeping announcements and peer
        // validation consistent even when the spec's forks were inserted out of order.
        self.fork_filter(*head).current()
    }
    fn latest_fork_id(&self) -> ForkId {
        self.fork_id(&Head {
            number: u64::MAX,
            timestamp: u64::MAX,
            ..Default::default()
        })
    }
    fn fork_filter(&self, head: Head) -> ForkFilter {
        ForkFilter::new(
            head,
            self.genesis_hash(),
            self.genesis_timestamp(),
            self.fork_filter_keys(),
        )
    }
}

impl EthereumHardforks for DogeosChainSpec {
    fn ethereum_fork_activation(&self, fork: EthereumHardfork) -> ForkCondition {
        self.fork(fork)
    }
}

impl DogeosHardforks for DogeosChainSpec {
    fn dogeos_fork_activation(&self, fork: DogeosHardfork) -> ForkCondition {
        self.fork(fork)
    }
}

impl From<Genesis> for DogeosChainSpec {
    fn from(genesis: Genesis) -> Self {
        Self::from_custom_genesis(genesis)
    }
}

pub(crate) fn build_spec(
    genesis: Genesis,
    config: ScrollChainConfig,
    dogeos_forks: [(DogeosHardfork, ForkCondition); 4],
    known_hash: Option<B256>,
    chain_override: Option<Chain>,
) -> DogeosChainSpec {
    let chain = chain_override.unwrap_or_else(|| Chain::from_id(genesis.config.chain_id));
    let header = make_genesis_header(&genesis);
    let genesis_header = match known_hash {
        Some(hash) => SealedHeader::new(header, hash),
        None => SealedHeader::new_unhashed(header),
    };

    DogeosChainSpec {
        inner: ChainSpec {
            chain,
            genesis_header,
            genesis,
            hardforks: feynman_hardforks(dogeos_forks),
            base_fee_params: BaseFeeParamsKind::Variable(
                alloc::vec![
                    (
                        DogeosHardfork::Feynman.boxed(),
                        DOGEOS_BASE_FEE_PARAMS_FEYNMAN,
                    ),
                    (DogeosHardfork::Tsuki.boxed(), DOGEOS_BASE_FEE_PARAMS_TSUKI),
                ]
                .into(),
            ),
            paris_block_and_final_difficulty: Some((0, U256::ZERO)),
            ..Default::default()
        },
        config,
    }
}

fn feynman_hardforks(
    dogeos_forks: impl IntoIterator<Item = (DogeosHardfork, ForkCondition)>,
) -> ChainHardforks {
    let mut forks: Vec<(Box<dyn Hardfork>, ForkCondition)> = alloc::vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::SpuriousDragon.boxed(),
            ForkCondition::Block(0)
        ),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Constantinople.boxed(),
            ForkCondition::Block(0)
        ),
        (
            EthereumHardfork::Petersburg.boxed(),
            ForkCondition::Block(0)
        ),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Shanghai.boxed(),
            ForkCondition::Timestamp(0)
        ),
    ];
    forks.extend(
        dogeos_forks
            .into_iter()
            .map(|(fork, condition)| (Box::new(fork) as Box<dyn Hardfork>, condition)),
    );
    ChainHardforks::new(forks)
}

fn make_genesis_header(genesis: &Genesis) -> Header {
    Header {
        gas_limit: genesis.gas_limit,
        difficulty: genesis.difficulty,
        nonce: genesis.nonce.into(),
        extra_data: genesis.extra_data.clone(),
        state_root: reth_trie_common::root::state_root_ref_unhashed(&genesis.alloc),
        timestamp: genesis.timestamp,
        mix_hash: genesis.mix_hash,
        beneficiary: genesis.coinbase,
        base_fee_per_gas: Some(
            genesis
                .base_fee_per_gas
                .unwrap_or_default()
                .try_into()
                .expect("base fee should fit in u64"),
        ),
        withdrawals_root: None,
        parent_beacon_block_root: None,
        blob_gas_used: None,
        excess_blob_gas: None,
        requests_hash: None,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ForkHash;

    #[test]
    fn supported_specs_are_feynman_baseline_without_withdrawals() {
        for spec in [&*DOGEOS_MAINNET, &*DOGEOS_CHIKYU, &*DOGEOS_DEV] {
            assert!(spec.is_feynman_active_at_timestamp(0));
            assert!(spec.genesis_header().base_fee_per_gas.is_some());
        }
        assert!(DOGEOS_MAINNET.is_tsuki_active_at_timestamp(0));
        assert!(DOGEOS_DEV.is_tsuki_active_at_timestamp(0));
        assert!(!DOGEOS_CHIKYU.is_tsuki_active_at_timestamp(0));
        assert!(!DOGEOS_CHIKYU.is_tsuki_active_at_timestamp(u64::MAX));
    }

    #[test]
    fn base_fee_params_switch_from_feynman_to_tsuki() {
        let feynman = BaseFeeParams::new(8, 2);
        let tsuki = BaseFeeParams::new(48, 10);
        assert_eq!(DOGEOS_BASE_FEE_PARAMS_FEYNMAN, feynman);
        assert_eq!(DOGEOS_BASE_FEE_PARAMS_TSUKI, tsuki);

        // Tsuki from genesis.
        for spec in [&*DOGEOS_MAINNET, &*DOGEOS_DEV] {
            assert_eq!(spec.base_fee_params_at_timestamp(0), tsuki);
            assert_eq!(spec.base_fee_params_at_timestamp(u64::MAX), tsuki);
        }

        // Tsuki activated after genesis.
        let mut genesis: Genesis =
            serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
        genesis
            .config
            .extra_fields
            .insert("feynmanTime".into(), 10.into());
        genesis
            .config
            .extra_fields
            .insert("tsukiTime".into(), 40.into());
        let spec = DogeosChainSpec::from_custom_genesis(genesis);
        assert_eq!(spec.base_fee_params_at_timestamp(10), feynman);
        assert_eq!(spec.base_fee_params_at_timestamp(39), feynman);
        assert_eq!(spec.base_fee_params_at_timestamp(40), tsuki);
        assert_eq!(spec.base_fee_params_at_timestamp(u64::MAX), tsuki);
    }

    #[test]
    fn chikyu_preserves_published_genesis_hash() {
        assert_eq!(DOGEOS_CHIKYU.genesis_hash(), DOGEOS_CHIKYU_GENESIS_HASH);
    }

    #[test]
    fn mainnet_genesis_hash_and_l1_config_match_the_frozen_document() {
        let genesis: Genesis =
            serde_json::from_str(include_str!("../res/genesis/dogeos.json")).unwrap();
        assert_eq!(
            make_genesis_header(&genesis).hash_slow(),
            DOGEOS_MAINNET_GENESIS_HASH
        );
        assert_eq!(DOGEOS_MAINNET.genesis_hash(), DOGEOS_MAINNET_GENESIS_HASH);
        assert_eq!(
            ScrollChainConfig::extract_from(&genesis.config.extra_fields).unwrap(),
            DOGEOS_MAINNET.config
        );
    }

    #[test]
    fn supported_chain_ids_match_the_current_node() {
        assert_eq!(DOGEOS_MAINNET.chain().id(), 0xff);
        assert_eq!(DOGEOS_CHIKYU.chain().id(), 0x5fdaf3);
        assert_eq!(DOGEOS_DEV.chain(), Chain::dev());
    }

    #[test]
    fn custom_genesis_preserves_dogeos_hardfork_boundaries() {
        let mut genesis: Genesis =
            serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
        genesis
            .config
            .extra_fields
            .insert("feynmanTime".into(), 10.into());
        genesis
            .config
            .extra_fields
            .insert("galileoTime".into(), 20.into());
        genesis
            .config
            .extra_fields
            .insert("galileoV2Time".into(), 30.into());
        genesis
            .config
            .extra_fields
            .insert("tsukiTime".into(), 40.into());

        let spec = DogeosChainSpec::from_custom_genesis(genesis);
        assert!(!spec.is_feynman_active_at_timestamp(9));
        assert!(spec.is_feynman_active_at_timestamp(10));
        assert!(!spec.is_tsuki_active_at_timestamp(39));
        assert!(spec.is_tsuki_active_at_timestamp(40));
    }

    #[test]
    fn timestamp_hardforks_through_tsuki_do_not_change_p2p_fork_compatibility() {
        let mut genesis: Genesis =
            serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
        genesis.timestamp = 0;
        genesis
            .config
            .extra_fields
            .insert("feynmanTime".into(), 10.into());
        genesis
            .config
            .extra_fields
            .insert("galileoTime".into(), 20.into());
        genesis
            .config
            .extra_fields
            .insert("galileoV2Time".into(), 30.into());
        genesis
            .config
            .extra_fields
            .insert("tsukiTime".into(), 40.into());

        let spec = DogeosChainSpec::from_custom_genesis(genesis);
        let expected_fork_id = ForkId {
            hash: ForkHash::from(spec.genesis_hash()),
            next: 0,
        };

        for timestamp in [0, 10, 20, 30, 40, u64::MAX] {
            let head = Head {
                number: 1,
                timestamp,
                ..Default::default()
            };
            assert_eq!(spec.fork_id(&head), expected_fork_id);
            assert_eq!(
                spec.fork_filter(head),
                ForkFilter::new(
                    head,
                    spec.genesis_hash(),
                    spec.genesis_timestamp(),
                    [ForkFilterKey::Block(0)],
                )
            );
        }

        assert!(!spec.is_feynman_active_at_timestamp(9));
        assert!(spec.is_feynman_active_at_timestamp(10));
        assert!(!spec.is_tsuki_active_at_timestamp(39));
        assert!(spec.is_tsuki_active_at_timestamp(40));
    }

    #[test]
    fn existing_network_fork_ids_are_unchanged() {
        for (spec, hash) in [
            (&*DOGEOS_MAINNET, ForkHash([0x62, 0xd8, 0x20, 0xb2])),
            (&*DOGEOS_CHIKYU, ForkHash([0x6c, 0x55, 0x6e, 0x50])),
            (&*DOGEOS_DEV, ForkHash::from(DOGEOS_DEV.genesis_hash())),
        ] {
            let expected = ForkId { hash, next: 0 };
            for head in [
                Head {
                    timestamp: spec.genesis_timestamp(),
                    ..Default::default()
                },
                Head {
                    number: u64::MAX,
                    timestamp: u64::MAX,
                    ..Default::default()
                },
            ] {
                assert_eq!(spec.fork_id(&head), expected);
                assert_eq!(spec.fork_filter(head).current(), expected);
            }
            // Chikyu ends in Tsuki = Never; this must still return the legacy ID.
            assert_eq!(spec.latest_fork_id(), expected);
        }
    }

    #[test]
    fn timestamp_forks_are_counted_strictly_after_tsuki_and_genesis() {
        for (genesis_time, tsuki_time, fork_time, counted) in [
            (0, 40, 30, false),
            (0, 40, 40, false),
            (0, 40, 60, true),
            (60, 40, 50, false),
            (60, 40, 60, false),
            (60, 40, 70, true),
        ] {
            let mut genesis: Genesis =
                serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
            genesis.timestamp = genesis_time;
            for (field, time) in [
                ("feynmanTime", 10),
                ("galileoTime", 20),
                ("tsukiTime", tsuki_time),
                // The activation timestamp, not the fork's enum order, determines inclusion.
                ("galileoV2Time", fork_time),
            ] {
                genesis
                    .config
                    .extra_fields
                    .insert(field.into(), time.into());
            }
            let spec = DogeosChainSpec::from_custom_genesis(genesis);
            let base = ForkHash::from(spec.genesis_hash());
            for timestamp in [
                genesis_time,
                fork_time - 1,
                fork_time,
                fork_time + 1,
                u64::MAX,
            ] {
                let head = Head {
                    number: 1,
                    timestamp,
                    ..Default::default()
                };
                let expected = ForkId {
                    hash: if counted && timestamp >= fork_time {
                        base + fork_time
                    } else {
                        base
                    },
                    next: if counted && timestamp < fork_time {
                        fork_time
                    } else {
                        0
                    },
                };
                assert_eq!(spec.fork_id(&head), expected);
                assert_eq!(spec.fork_filter(head).current(), expected);
            }
            assert_eq!(
                spec.latest_fork_id(),
                ForkId {
                    hash: if counted { base + fork_time } else { base },
                    next: 0,
                }
            );
        }
    }

    #[test]
    fn unscheduled_tsuki_and_empty_schedules_preserve_genesis_fork_id() {
        let mut spec = (**DOGEOS_CHIKYU).clone();
        spec.inner.hardforks.insert(
            DogeosHardfork::GalileoV2,
            ForkCondition::Timestamp(u64::MAX),
        );
        let expected = ForkId {
            hash: ForkHash::from(spec.genesis_hash()),
            next: 0,
        };
        assert_eq!(spec.latest_fork_id(), expected);

        spec.inner.hardforks.remove(&DogeosHardfork::Tsuki);
        assert_eq!(spec.latest_fork_id(), expected);

        spec.inner.hardforks = ChainHardforks::default();
        assert_eq!(spec.latest_fork_id(), expected);
        assert_eq!(spec.fork_filter(Head::default()).current(), expected);
    }

    #[test]
    fn fork_ids_sort_and_deduplicate_block_and_timestamp_activations() {
        let time = DOGEOS_DEV.genesis_timestamp() + 100;
        let forks: Vec<(Box<dyn Hardfork>, ForkCondition)> = alloc::vec![
            (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
            (
                EthereumHardfork::Homestead.boxed(),
                ForkCondition::Block(10)
            ),
            (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(10)),
            (EthereumHardfork::London.boxed(), ForkCondition::Block(20)),
            (
                EthereumHardfork::Paris.boxed(),
                ForkCondition::TTD {
                    activation_block_number: 20,
                    fork_block: Some(20),
                    total_difficulty: U256::ZERO,
                }
            ),
            (DogeosHardfork::Tsuki.boxed(), ForkCondition::Timestamp(0)),
            (
                DogeosHardfork::Galileo.boxed(),
                ForkCondition::Timestamp(time)
            ),
            (
                DogeosHardfork::GalileoV2.boxed(),
                ForkCondition::Timestamp(time)
            ),
            (
                EthereumHardfork::Cancun.boxed(),
                ForkCondition::Timestamp(time + 100)
            ),
            (EthereumHardfork::Prague.boxed(), ForkCondition::Never),
        ];
        let mut ordered = (**DOGEOS_DEV).clone();
        ordered.inner.hardforks = ChainHardforks::new(forks.clone());
        let mut reversed = ordered.clone();
        reversed.inner.hardforks = ChainHardforks::new(forks.into_iter().rev().collect());
        let base = ForkHash::from(ordered.genesis_hash());
        for (number, timestamp, hash, next) in [
            (0, 0, base, 10),
            (10, 0, base + 10_u64, 20),
            (20, time - 1, base + 10_u64 + 20_u64, time),
            (20, time, base + 10_u64 + 20_u64 + time, time + 100),
            (
                20,
                time + 100,
                base + 10_u64 + 20_u64 + time + (time + 100),
                0,
            ),
        ] {
            let head = Head {
                number,
                timestamp,
                ..Default::default()
            };
            let expected = ForkId { hash, next };
            for spec in [&ordered, &reversed] {
                assert_eq!(spec.fork_id(&head), expected);
                assert_eq!(spec.fork_filter(head).current(), expected);
            }
        }
        let expected = ForkId {
            hash: base + 10_u64 + 20_u64 + time + (time + 100),
            next: 0,
        };
        assert_eq!(ordered.latest_fork_id(), expected);
        assert_eq!(reversed.latest_fork_id(), expected);
    }

    #[test]
    fn post_tsuki_filter_accepts_syncing_peers_and_rejects_stale_peers() {
        // Use a real Unix timestamp so the peer's untagged `next` is recognized as time-based.
        let time = 2_000_000_000;
        let spec = DogeosChainSpecBuilder::dev()
            .with_fork(DogeosHardfork::GalileoV2, ForkCondition::Timestamp(time))
            .build(ScrollChainConfig::dev());
        let base = ForkHash::from(spec.genesis_hash());
        let before = Head {
            number: 1,
            timestamp: time - 1,
            ..Default::default()
        };
        let after = Head {
            timestamp: time,
            ..before
        };
        let stale = ForkId {
            hash: base,
            next: 0,
        };
        let upcoming = ForkId {
            hash: base,
            next: time,
        };
        let activated = ForkId {
            hash: base + time,
            next: 0,
        };
        let mut filter = spec.fork_filter(before);
        assert_eq!(filter.current(), upcoming);
        assert!(filter.validate(stale).is_ok());
        assert!(filter.validate(activated).is_ok());

        assert!(filter.set_head(after).is_some());
        assert_eq!(filter.current(), activated);
        assert!(filter.validate(upcoming).is_ok());
        assert!(filter.validate(activated).is_ok());
        assert!(filter.validate(stale).is_err());
        assert!(filter.set_head(before).is_some());
        assert_eq!(filter.current(), upcoming);
    }
}
