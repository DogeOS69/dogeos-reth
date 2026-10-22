use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use reth_chainspec::EthChainSpec;
use revm::Database;

/// Protocol-enforced maximum L2 base fee.
pub use dogeos_protocol_types::MAX_L2_BASE_FEE;

/// L2 base-fee overhead slot in the system config contract.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Default overhead when the system config contract has not initialized the slot.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// Precision retained for external callers that share the inherited Scroll fee constants.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// The parent-header inputs of the Feynman+ base-fee rule.
///
/// Carried separately from the header itself so that a block executor, which is handed the
/// parent by its caller, can run the rule without cloning a whole header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollBaseFeeParent {
    /// Parent base fee per gas.
    pub base_fee_per_gas: u64,
    /// Parent gas used.
    pub gas_used: u64,
    /// Parent gas limit.
    pub gas_limit: u64,
}

impl ScrollBaseFeeParent {
    /// Extracts the rule's inputs from a parent header.
    ///
    /// Returns `None` for a header without a base fee, which no Feynman+ header may be.
    pub fn from_header<H: BlockHeader>(header: &H) -> Option<Self> {
        Some(Self {
            base_fee_per_gas: header.base_fee_per_gas()?,
            gas_used: header.gas_used(),
            gas_limit: header.gas_limit(),
        })
    }
}

/// State-aware Feynman+ L2 base-fee calculator.
#[derive(Clone, Debug, Default)]
pub struct ScrollBaseFeeProvider<ChainSpec>(ChainSpec);

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec> {
    pub const fn new(chain_spec: ChainSpec) -> Self {
        Self(chain_spec)
    }
}

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec>
where
    ChainSpec: EthChainSpec + DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    /// Calculates the next block's base fee using the current system-config storage.
    pub fn next_block_base_fee<DB, H>(
        &self,
        db: &mut DB,
        parent: &H,
        timestamp: u64,
    ) -> Result<u64, DB::Error>
    where
        DB: Database,
        H: BlockHeader,
    {
        let parent = ScrollBaseFeeParent::from_header(parent)
            .expect("Feynman+ parent headers carry a base fee");
        self.next_block_base_fee_for_parent(db, &parent, timestamp)
    }

    /// Reads the L2 base-fee overhead from the system config contract in `db`.
    pub fn base_fee_overhead<DB: Database>(&self, db: &mut DB) -> Result<u64, DB::Error> {
        base_fee_overhead(&self.0, db)
    }

    /// Calculates the next block's base fee from the parent's already-extracted inputs.
    pub fn next_block_base_fee_for_parent<DB: Database>(
        &self,
        db: &mut DB,
        parent: &ScrollBaseFeeParent,
        timestamp: u64,
    ) -> Result<u64, DB::Error> {
        let overhead = self.base_fee_overhead(db)?;
        Ok(next_block_base_fee_with_overhead(
            &self.0, parent, overhead, timestamp,
        ))
    }
}

/// Reads the L2 base-fee overhead from the system config contract in `db`.
///
/// A zero (or unset) slot means the default overhead. This is a pure read: it loads the system
/// config account and its slot 101 into the state it is given and changes nothing, which is
/// what puts the slot into an execution witness.
///
/// Takes the chain spec by reference so that a block executor, which only borrows its spec, can
/// call it.
pub fn base_fee_overhead<ChainSpec, DB>(
    chain_spec: &ChainSpec,
    db: &mut DB,
) -> Result<u64, DB::Error>
where
    ChainSpec: ChainConfig<Config = ScrollChainConfig>,
    DB: Database,
{
    let system_config = chain_spec.chain_config().l1_config.l2_system_config_address;
    let configured_overhead = db.storage(system_config, L2_BASE_FEE_OVERHEAD_SLOT)?;
    Ok(if configured_overhead == U256::ZERO {
        DEFAULT_BASE_FEE_OVERHEAD
    } else {
        configured_overhead
    }
    .saturating_to::<u64>())
}

/// Calculates the next block's base fee from the parent's inputs and a known overhead.
///
/// The one place the formula lives; every other entry point reads the overhead and defers to
/// this.
pub fn next_block_base_fee_with_overhead<ChainSpec: EthChainSpec>(
    chain_spec: &ChainSpec,
    parent: &ScrollBaseFeeParent,
    overhead: u64,
    timestamp: u64,
) -> u64 {
    let parent_eip1559_base_fee = parent.base_fee_per_gas.saturating_sub(overhead);
    let next_eip1559_base_fee = calc_next_block_base_fee(
        parent.gas_used,
        parent.gas_limit,
        parent_eip1559_base_fee,
        chain_spec.base_fee_params_at_timestamp(timestamp),
    );

    next_eip1559_base_fee
        .saturating_add(overhead)
        .min(MAX_L2_BASE_FEE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::DOGEOS_MAINNET;
    use revm::database::{EmptyDB, State, states::plain_account::PlainStorage};

    /// Mainnet-scale block gas limit; with elasticity 10 the target is 3M gas.
    const GAS_LIMIT: u64 = 30_000_000;
    const GAS_TARGET: u64 = GAS_LIMIT / 10;
    /// The 420 gwei overhead planned for activation.
    const OVERHEAD: u64 = 420_000_000_000;

    fn parent(base_fee: u64, gas_used: u64) -> alloy_consensus::Header {
        alloy_consensus::Header {
            base_fee_per_gas: Some(base_fee),
            gas_limit: GAS_LIMIT,
            gas_used,
            timestamp: 1,
            ..Default::default()
        }
    }

    fn state_with_overhead(overhead: u64) -> State<EmptyDB> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_OVERHEAD_SLOT, U256::from(overhead))]),
        );
        state
    }

    fn next_base_fee(overhead: u64, parent_base_fee: u64, gas_used: u64) -> eyre::Result<u64> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());
        Ok(provider.next_block_base_fee(
            &mut state_with_overhead(overhead),
            &parent(parent_base_fee, gas_used),
            2,
        )?)
    }

    /// Parity vectors shared with the guest's base-fee rule (sbv) and the payload builder.
    ///
    /// Every row is `(overhead, parent base fee, parent gas used, expected child base fee)` on
    /// mainnet at a Tsuki timestamp: denominator 48, elasticity 10, 30M gas limit, so the gas
    /// target is 3M. An overhead of zero means slot 101 is unset and the default applies.
    /// Keep these in step with the guest; a divergence here is a chain split.
    const PARITY_VECTORS: &[(u64, u64, u64, u64)] = &[
        // Unset slot: the default overhead, idle, at target and full.
        (0, 15_680_000, 0, 15_680_000),
        (0, 15_680_000, GAS_TARGET, 15_680_000),
        (
            0,
            15_680_000 + 1_000_000_000,
            GAS_TARGET,
            15_680_000 + 1_000_000_000,
        ),
        (
            0,
            15_680_000 + 1_000_000_000,
            GAS_LIMIT,
            15_680_000 + 1_187_500_000,
        ),
        (0, 15_680_000 + 1_000_000_000, 0, 15_680_000 + 979_166_667),
        // A configured overhead replaces the default, and the fee never falls below it.
        (1, 1_000_000_000, GAS_TARGET, 1_000_000_000),
        (
            OVERHEAD,
            OVERHEAD + 1_000_000_000,
            2 * GAS_TARGET,
            OVERHEAD + 1_020_833_333,
        ),
        // Nothing above the overhead to grow, yet EIP-1559 still bumps a full block by one wei.
        (OVERHEAD, OVERHEAD, GAS_LIMIT, OVERHEAD + 1),
        (OVERHEAD, OVERHEAD, 0, OVERHEAD),
        // A parent below its own overhead saturates to zero before the EIP-1559 step.
        (OVERHEAD, OVERHEAD - 1, GAS_LIMIT, OVERHEAD + 1),
        (OVERHEAD, 0, GAS_LIMIT, OVERHEAD + 1),
        // The cap holds on the way up and is itself a fixed point.
        (OVERHEAD, MAX_L2_BASE_FEE, GAS_LIMIT, MAX_L2_BASE_FEE),
        (OVERHEAD, MAX_L2_BASE_FEE - 1, GAS_LIMIT, MAX_L2_BASE_FEE),
        (0, MAX_L2_BASE_FEE, GAS_TARGET, MAX_L2_BASE_FEE),
    ];

    #[test]
    fn parity_vectors_hold_for_both_entry_points() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());
        for &(overhead, parent_base_fee, gas_used, expected) in PARITY_VECTORS {
            let parent = parent(parent_base_fee, gas_used);
            let label = format!("overhead={overhead}, parent={parent_base_fee}, gas={gas_used}");

            // Through the state, exactly as the payload builder calls it.
            let mut state = if overhead == 0 {
                State::builder()
                    .with_database(EmptyDB::default())
                    .with_bundle_update()
                    .build()
            } else {
                state_with_overhead(overhead)
            };
            assert_eq!(
                provider.next_block_base_fee(&mut state, &parent, 2)?,
                expected,
                "{label}"
            );

            // And from extracted inputs, as the block executor calls it.
            let resolved = if overhead == 0 {
                DEFAULT_BASE_FEE_OVERHEAD.saturating_to::<u64>()
            } else {
                overhead
            };
            assert_eq!(
                next_block_base_fee_with_overhead(
                    &**DOGEOS_MAINNET,
                    &ScrollBaseFeeParent::from_header(&parent).expect("parent has a base fee"),
                    resolved,
                    2,
                ),
                expected,
                "{label}"
            );
        }
        Ok(())
    }

    #[test]
    fn cap_is_420_000_gwei() {
        assert_eq!(MAX_L2_BASE_FEE, 420_000_000_000_000);
    }

    #[test]
    fn default_overhead_preserves_fee_at_target_gas() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1_000_000_000, GAS_TARGET), 2)?,
            1_000_000_000
        );
        Ok(())
    }

    #[test]
    fn configured_overhead_is_read_from_state() -> eyre::Result<()> {
        assert_eq!(next_base_fee(1, 1_000_000_000, GAS_TARGET)?, 1_000_000_000);
        Ok(())
    }

    #[test]
    fn escalates_with_denominator_48_and_elasticity_10() -> eyre::Result<()> {
        // DOGEOS_MAINNET is Tsuki-active from genesis, so the Tsuki params apply.
        // 2x target: the part above the overhead grows by 1/48.
        assert_eq!(
            next_base_fee(OVERHEAD, OVERHEAD + 1_000_000_000, 2 * GAS_TARGET)?,
            OVERHEAD + 1_020_833_333
        );
        // Full block (10x target): the part above the overhead grows by 9/48.
        assert_eq!(
            next_base_fee(OVERHEAD, OVERHEAD + 1_000_000_000, GAS_LIMIT)?,
            OVERHEAD + 1_187_500_000
        );
        // Empty block: the part above the overhead shrinks by 1/48.
        assert_eq!(
            next_base_fee(OVERHEAD, OVERHEAD + 1_000_000_000, 0)?,
            OVERHEAD + 979_166_667
        );
        // Idle fixed point: overhead + 47 wei.
        assert_eq!(next_base_fee(OVERHEAD, OVERHEAD + 48, 0)?, OVERHEAD + 47);
        assert_eq!(next_base_fee(OVERHEAD, OVERHEAD + 47, 0)?, OVERHEAD + 47);
        Ok(())
    }

    #[test]
    fn delayed_fee_params_preserve_historical_fees_until_the_child_activation() -> eyre::Result<()>
    {
        let mut genesis = DOGEOS_MAINNET.genesis().clone();
        let tsuki = genesis.timestamp + 40;
        let activation = genesis.timestamp + 100;
        genesis
            .config
            .extra_fields
            .insert("tsukiTime".into(), tsuki.into());
        genesis
            .config
            .extra_fields
            .insert("tsukiFeeParamsTime".into(), activation.into());
        let spec = dogeos_chainspec::DogeosChainSpec::from_custom_genesis(genesis);
        assert!(spec.is_tsuki_active_at_timestamp(tsuki));
        let provider = ScrollBaseFeeProvider::new(spec);
        // A full block with a 1 gwei fee and 0.1 gwei overhead grows the excess by
        // 1/8 before activation, and 9/48 afterwards. Use literal historical fee values.
        for (timestamp, expected) in [
            (tsuki - 1, 1_112_500_000),
            (tsuki, 1_112_500_000),
            (activation - 1, 1_112_500_000),
            (activation, 1_168_750_000),
            (activation + 1, 1_168_750_000),
        ] {
            let mut parent = parent(1_000_000_000, GAS_LIMIT);
            parent.timestamp = timestamp - 1;
            assert_eq!(
                provider.next_block_base_fee(
                    &mut state_with_overhead(100_000_000),
                    &parent,
                    timestamp
                )?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn base_fee_is_capped() -> eyre::Result<()> {
        assert_eq!(
            next_base_fee(OVERHEAD, MAX_L2_BASE_FEE, GAS_LIMIT)?,
            MAX_L2_BASE_FEE
        );
        assert_eq!(
            next_base_fee(OVERHEAD, MAX_L2_BASE_FEE - 1, GAS_LIMIT)?,
            MAX_L2_BASE_FEE
        );
        Ok(())
    }
}
