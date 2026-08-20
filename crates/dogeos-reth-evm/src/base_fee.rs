use alloy_consensus::BlockHeader;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::{Address, U256};
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use revm::Database;

/// Initial producer-policy maximum-change denominator.
pub const DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR: u128 = 512;

/// Initial producer-policy elasticity multiplier.
pub const DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER: u128 = 10;

/// Largest denominator that keeps the integer formula inside `u128` for all `u64` header inputs.
pub const MAX_BASE_FEE_MAX_CHANGE_DENOMINATOR: u128 = u64::MAX as u128;

/// Largest elasticity that leaves a non-zero target for every consensus-valid gas limit.
pub const MAX_BASE_FEE_ELASTICITY_MULTIPLIER: u128 = 5_000;

/// Initial producer-policy operating maximum (300,000 Gwei).
pub const DEFAULT_OPERATING_MAX_L2_BASE_FEE: u64 = 300_000_000_000_000;

/// Deterministic floor used only when no system-config contract is configured (420 Gwei).
pub const DEFAULT_BASE_FEE_FLOOR: U256 = U256::from_limbs([420_000_000_000, 0, 0, 0]);

/// Inherited `baseFeeOverhead` slot, reinterpreted by Tsuki producer policy as the total-fee floor.
const L2_BASE_FEE_FLOOR_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Precision retained for external callers that share the inherited Scroll fee constants.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// Invalid local sequencer base-fee policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SequencerBaseFeePolicyError {
    #[error("base-fee maximum-change denominator must be non-zero")]
    ZeroMaxChangeDenominator,
    #[error(
        "base-fee maximum-change denominator {denominator} exceeds safe maximum {MAX_BASE_FEE_MAX_CHANGE_DENOMINATOR}"
    )]
    MaxChangeDenominatorTooLarge { denominator: u128 },
    #[error("base-fee elasticity multiplier must be non-zero")]
    ZeroElasticityMultiplier,
    #[error(
        "base-fee elasticity multiplier {elasticity} exceeds safe maximum {MAX_BASE_FEE_ELASTICITY_MULTIPLIER}"
    )]
    ElasticityMultiplierTooLarge { elasticity: u128 },
    #[error("operating base-fee maximum must be non-zero")]
    ZeroOperatingMaximum,
    #[error("operating base-fee maximum {operating_max} exceeds hard maximum {hard_max}")]
    OperatingMaximumExceedsHardMaximum { operating_max: u64, hard_max: u64 },
}

/// Node-local policy used only to construct or predict the next payload's base fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencerBaseFeePolicy {
    params: BaseFeeParams,
    operating_max_base_fee: u64,
}

impl SequencerBaseFeePolicy {
    /// Builds and validates a producer policy against the supplied consensus hard envelope.
    ///
    /// The hard maximum is checked but deliberately not stored as producer configuration.
    pub const fn try_new(
        max_change_denominator: u128,
        elasticity_multiplier: u128,
        operating_max_base_fee: u64,
        hard_max_base_fee: u64,
    ) -> Result<Self, SequencerBaseFeePolicyError> {
        if max_change_denominator == 0 {
            return Err(SequencerBaseFeePolicyError::ZeroMaxChangeDenominator);
        }
        if max_change_denominator > MAX_BASE_FEE_MAX_CHANGE_DENOMINATOR {
            return Err(SequencerBaseFeePolicyError::MaxChangeDenominatorTooLarge {
                denominator: max_change_denominator,
            });
        }
        if elasticity_multiplier == 0 {
            return Err(SequencerBaseFeePolicyError::ZeroElasticityMultiplier);
        }
        if elasticity_multiplier > MAX_BASE_FEE_ELASTICITY_MULTIPLIER {
            return Err(SequencerBaseFeePolicyError::ElasticityMultiplierTooLarge {
                elasticity: elasticity_multiplier,
            });
        }
        if operating_max_base_fee == 0 {
            return Err(SequencerBaseFeePolicyError::ZeroOperatingMaximum);
        }
        if operating_max_base_fee > hard_max_base_fee {
            return Err(
                SequencerBaseFeePolicyError::OperatingMaximumExceedsHardMaximum {
                    operating_max: operating_max_base_fee,
                    hard_max: hard_max_base_fee,
                },
            );
        }
        Ok(Self {
            params: BaseFeeParams::new(max_change_denominator, elasticity_multiplier),
            operating_max_base_fee,
        })
    }

    pub const fn base_fee_params(&self) -> BaseFeeParams {
        self.params
    }

    pub const fn operating_max_base_fee(&self) -> u64 {
        self.operating_max_base_fee
    }

    /// Applies producer clamps after ordinary EIP-1559 movement on the total parent fee.
    pub fn clamp(&self, candidate: u64, floor: u64) -> u64 {
        candidate.max(floor).min(self.operating_max_base_fee)
    }
}

/// EIP-1559 integer movement with explicit saturation before producer clamps.
fn calculate_next_base_fee(
    gas_used: u64,
    gas_limit: u64,
    base_fee: u64,
    params: BaseFeeParams,
) -> u64 {
    // Consensus-valid headers have at least 5,000 gas, but keep this helper total for tests and
    // callers holding an unvalidated header as well.
    let gas_target = (gas_limit / params.elasticity_multiplier as u64).max(1);

    match gas_used.cmp(&gas_target) {
        core::cmp::Ordering::Equal => base_fee,
        core::cmp::Ordering::Greater => {
            let increase = (u128::from(base_fee) * u128::from(gas_used - gas_target)
                / (u128::from(gas_target) * params.max_change_denominator))
                .max(1);
            u128::from(base_fee)
                .saturating_add(increase)
                .min(u128::from(u64::MAX)) as u64
        }
        core::cmp::Ordering::Less => {
            let decrease = u128::from(base_fee) * u128::from(gas_target - gas_used)
                / (u128::from(gas_target) * params.max_change_denominator);
            base_fee.saturating_sub(decrease.min(u128::from(u64::MAX)) as u64)
        }
    }
}

/// State-aware next-payload base-fee calculator.
#[derive(Clone, Debug)]
pub struct ScrollBaseFeeProvider<ChainSpec> {
    chain_spec: ChainSpec,
    policy: SequencerBaseFeePolicy,
}

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec> {
    pub const fn new(chain_spec: ChainSpec, policy: SequencerBaseFeePolicy) -> Self {
        Self { chain_spec, policy }
    }
}

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec>
where
    ChainSpec: ChainConfig<Config = ScrollChainConfig>,
{
    /// Calculates the next payload's fee from the parent header and current system-config floor.
    pub fn next_block_base_fee<DB, H>(&self, db: &mut DB, parent: &H) -> Result<u64, DB::Error>
    where
        DB: Database,
        H: BlockHeader,
    {
        let system_config = self
            .chain_spec
            .chain_config()
            .l1_config
            .l2_system_config_address;
        let configured_floor = db.storage(system_config, L2_BASE_FEE_FLOOR_SLOT)?;
        let floor = if configured_floor == U256::ZERO && system_config == Address::ZERO {
            DEFAULT_BASE_FEE_FLOOR
        } else {
            configured_floor
        }
        .saturating_to::<u64>();

        let parent_base_fee = parent
            .base_fee_per_gas()
            .expect("Feynman+ parent headers carry a base fee");
        let candidate = calculate_next_base_fee(
            parent.gas_used(),
            parent.gas_limit(),
            parent_base_fee,
            self.policy.base_fee_params(),
        );

        Ok(self.policy.clamp(candidate, floor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::{DOGEOS_DEV, DOGEOS_MAINNET};
    use revm::database::{EmptyDB, State, states::plain_account::PlainStorage};

    const HARD_MAX: u64 = 1_000_000_000_000_000;

    fn policy(denominator: u128) -> SequencerBaseFeePolicy {
        SequencerBaseFeePolicy::try_new(
            denominator,
            DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER,
            DEFAULT_OPERATING_MAX_L2_BASE_FEE,
            HARD_MAX,
        )
        .unwrap()
    }

    fn parent(base_fee: u64, gas_used: u64) -> alloy_consensus::Header {
        alloy_consensus::Header {
            base_fee_per_gas: Some(base_fee),
            gas_limit: 30_000_000,
            gas_used,
            timestamp: 1,
            ..Default::default()
        }
    }

    fn empty_state() -> State<EmptyDB> {
        State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build()
    }

    #[test]
    fn policy_rejects_unsafe_values() {
        assert_eq!(
            SequencerBaseFeePolicy::try_new(0, 10, 1, HARD_MAX),
            Err(SequencerBaseFeePolicyError::ZeroMaxChangeDenominator)
        );
        assert_eq!(
            SequencerBaseFeePolicy::try_new(u128::from(u64::MAX) + 1, 10, 1, HARD_MAX),
            Err(SequencerBaseFeePolicyError::MaxChangeDenominatorTooLarge {
                denominator: u128::from(u64::MAX) + 1,
            })
        );
        assert_eq!(
            SequencerBaseFeePolicy::try_new(512, 0, 1, HARD_MAX),
            Err(SequencerBaseFeePolicyError::ZeroElasticityMultiplier)
        );
        assert_eq!(
            SequencerBaseFeePolicy::try_new(512, 5_001, 1, HARD_MAX),
            Err(SequencerBaseFeePolicyError::ElasticityMultiplierTooLarge { elasticity: 5_001 })
        );
        assert_eq!(
            SequencerBaseFeePolicy::try_new(512, 10, 0, HARD_MAX),
            Err(SequencerBaseFeePolicyError::ZeroOperatingMaximum)
        );
        assert_eq!(
            SequencerBaseFeePolicy::try_new(512, 10, HARD_MAX + 1, HARD_MAX),
            Err(
                SequencerBaseFeePolicyError::OperatingMaximumExceedsHardMaximum {
                    operating_max: HARD_MAX + 1,
                    hard_max: HARD_MAX,
                }
            )
        );
    }

    #[test]
    fn exact_target_preserves_total_base_fee() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(512));
        assert_eq!(
            provider
                .next_block_base_fee(&mut empty_state(), &parent(500_000_000_000, 3_000_000),)?,
            500_000_000_000
        );
        Ok(())
    }

    #[test]
    fn below_target_decreases_but_never_crosses_floor() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(512));
        assert_eq!(
            provider.next_block_base_fee(&mut empty_state(), &parent(840_000_000_000, 0))?,
            838_359_375_000
        );
        assert_eq!(
            provider.next_block_base_fee(&mut empty_state(), &parent(420_000_000_000, 0))?,
            420_000_000_000
        );
        Ok(())
    }

    #[test]
    fn above_target_increases_total_fee_immediately_from_floor() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(512));
        assert_eq!(
            provider
                .next_block_base_fee(&mut empty_state(), &parent(420_000_000_000, 30_000_000),)?,
            427_382_812_500
        );
        Ok(())
    }

    #[test]
    fn raising_the_live_floor_clamps_the_next_block() -> eyre::Result<()> {
        let mut state = empty_state();
        state.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_FLOOR_SLOT, U256::from(500_000_000_000u64))]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone(), policy(512));

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(420_000_000_000, 0))?,
            500_000_000_000
        );
        Ok(())
    }

    #[test]
    fn lowering_the_live_floor_allows_normal_decay() -> eyre::Result<()> {
        let mut state = empty_state();
        state.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_FLOOR_SLOT, U256::from(420_000_000_000u64))]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone(), policy(512));

        let next = provider.next_block_base_fee(&mut state, &parent(500_000_000_000, 0))?;
        assert_eq!(next, 499_023_437_500);
        assert!(next > 420_000_000_000);
        Ok(())
    }

    #[test]
    fn zero_live_floor_does_not_use_the_dev_fallback() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone(), policy(512));
        assert_eq!(
            provider.next_block_base_fee(&mut empty_state(), &parent(420_000_000_000, 0))?,
            419_179_687_500
        );
        Ok(())
    }

    #[test]
    fn floor_and_operating_clamps_are_applied_in_order() {
        let policy = SequencerBaseFeePolicy::try_new(512, 10, 300, 1_000).unwrap();
        assert_eq!(policy.clamp(2_000, 100), 300);
        assert_eq!(policy.clamp(50, 100), 100);
    }

    #[test]
    fn produced_fee_is_clamped_to_the_operating_maximum() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(512));
        assert_eq!(
            provider.next_block_base_fee(
                &mut empty_state(),
                &parent(DEFAULT_OPERATING_MAX_L2_BASE_FEE, 30_000_000),
            )?,
            DEFAULT_OPERATING_MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn elasticity_changes_the_produced_fee_target() -> eyre::Result<()> {
        let elasticity_five =
            SequencerBaseFeePolicy::try_new(512, 5, DEFAULT_OPERATING_MAX_L2_BASE_FEE, HARD_MAX)?;
        let elasticity_ten = policy(512);
        let parent = parent(420_000_000_000, 6_000_000);

        assert_eq!(
            ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), elasticity_five)
                .next_block_base_fee(&mut empty_state(), &parent)?,
            420_000_000_000
        );
        assert_eq!(
            ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), elasticity_ten)
                .next_block_base_fee(&mut empty_state(), &parent)?,
            420_820_312_500
        );
        Ok(())
    }

    #[test]
    fn denominator_vectors_match_integer_eip1559_math() -> eyre::Result<()> {
        let vectors = [
            (256, 434_765_625_000),
            (384, 429_843_750_000),
            (512, 427_382_812_500),
            (768, 424_921_875_000),
            (1024, 423_691_406_250),
        ];
        for (denominator, expected) in vectors {
            let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(denominator));
            assert_eq!(
                provider.next_block_base_fee(
                    &mut empty_state(),
                    &parent(420_000_000_000, 30_000_000),
                )?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn base_fee_calculation_is_safe_at_the_hard_limit() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_DEV.clone(), policy(512));
        assert_eq!(
            provider.next_block_base_fee(&mut empty_state(), &parent(HARD_MAX, 30_000_000),)?,
            DEFAULT_OPERATING_MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn raw_calculation_saturates_instead_of_overflowing() {
        let params = BaseFeeParams::new(1, MAX_BASE_FEE_ELASTICITY_MULTIPLIER);
        assert_eq!(
            calculate_next_base_fee(5_000, 5_000, HARD_MAX, params),
            5_000_000_000_000_000_000
        );
        assert_eq!(
            calculate_next_base_fee(5_000, 5_000, u64::MAX, params),
            u64::MAX
        );
    }
}
