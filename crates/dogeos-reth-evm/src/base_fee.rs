use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use reth_chainspec::EthChainSpec;
use revm::Database;

/// The formula's clamp before the Tsuki fee parameters.
pub use dogeos_protocol_types::LEGACY_MAX_L2_BASE_FEE;
/// Protocol-enforced maximum L2 base fee, and the formula's clamp from the Tsuki fee parameters.
pub use dogeos_protocol_types::MAX_L2_BASE_FEE;

/// L2 base-fee overhead slot in the system config contract.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Default overhead when the system config contract has not initialized the slot.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// Precision retained for external callers that share the inherited Scroll fee constants.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// The parent-header inputs of the Feynman+ base-fee rule.
///
/// Carries only the inputs needed by the executor, without cloning the header.
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

    /// Predicts the base fee of `parent`'s child before the child's timestamp is known.
    ///
    /// Uses the earliest timestamp a child can have, `parent.timestamp + 1`, so a fee schedule
    /// change activating at `parent.timestamp + 1` is already applied; a later child timestamp
    /// may still cross an activation. Fee predictors (pool maintenance, fee RPCs) use this; the
    /// payload builder and the import check use the real child timestamp.
    pub fn predict_next_block_base_fee<DB, H>(
        &self,
        db: &mut DB,
        parent: &H,
    ) -> Result<u64, DB::Error>
    where
        DB: Database,
        H: BlockHeader,
    {
        self.next_block_base_fee(db, parent, parent.timestamp().saturating_add(1))
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

/// The base-fee formula's clamp for a block at `timestamp`: 10 gwei before the Tsuki fee
/// parameters and 420,000 gwei from them.
pub fn max_l2_base_fee_at_timestamp<ChainSpec: EthChainSpec + DogeosHardforks>(
    chain_spec: &ChainSpec,
    timestamp: u64,
) -> u64 {
    if chain_spec.is_tsuki_fee_params_active_at_timestamp(timestamp) {
        MAX_L2_BASE_FEE
    } else {
        LEGACY_MAX_L2_BASE_FEE
    }
}

/// Calculates the next block's base fee from the parent's inputs and a known overhead.
///
/// The one place the formula lives; every other entry point reads the overhead and defers to
/// this. The EIP-1559 parameters and the clamp both follow the Tsuki fee-parameter activation
/// (`tsukiFeeParamsTime`) at the child's `timestamp`.
pub fn next_block_base_fee_with_overhead<ChainSpec: EthChainSpec + DogeosHardforks>(
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
        .min(max_l2_base_fee_at_timestamp(chain_spec, timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_MAINNET};
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

    /// Shared with SBV; both calculator entry points must match these expectations.
    /// Set `UPDATE_PARITY_VECTORS=1` to regenerate them for review.
    const PARITY_VECTORS_PATH: &str = "testdata/base-fee-parity-vectors.json";

    fn parity_spec(chain: &str) -> dogeos_chainspec::DogeosChainSpec {
        match chain {
            // Tsuki from genesis: denominator 48, elasticity 10.
            "mainnet" => (**DOGEOS_MAINNET).clone(),
            // Before Chikyu's fee-parameter activation: denominator 8, elasticity 2, 10 gwei cap.
            "chikyu" => (**DOGEOS_CHIKYU).clone(),
            // Mainnet with the fee parameters delayed past the Tsuki execution transition, so
            // rows on either side of `tsuki_fee_params_time` cross the boundary.
            "delayed-fee-params" => {
                let mut genesis = DOGEOS_MAINNET.genesis().clone();
                genesis
                    .config
                    .extra_fields
                    .insert("tsukiTime".into(), 0.into());
                genesis
                    .config
                    .extra_fields
                    .insert("tsukiFeeParamsTime".into(), 100.into());
                dogeos_chainspec::DogeosChainSpec::from_custom_genesis(genesis)
            }
            other => panic!("unknown parity chain {other}"),
        }
    }

    #[test]
    fn parity_vectors_hold_for_both_entry_points() -> eyre::Result<()> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(PARITY_VECTORS_PATH);
        let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let update = std::env::var_os("UPDATE_PARITY_VECTORS").is_some();

        for row in document["vectors"]
            .as_array_mut()
            .expect("vectors is an array")
        {
            let chain = row["chain"].as_str().expect("chain").to_owned();
            let spec = parity_spec(&chain);
            let timestamp = row["timestamp"].as_u64().expect("timestamp");
            let slot_101 = U256::from_str_radix(
                row["slot_101"]
                    .as_str()
                    .expect("slot_101")
                    .trim_start_matches("0x"),
                16,
            )?;
            let inputs = ScrollBaseFeeParent {
                base_fee_per_gas: row["parent_base_fee"].as_u64().expect("parent_base_fee"),
                gas_used: row["parent_gas_used"].as_u64().expect("parent_gas_used"),
                gas_limit: row["parent_gas_limit"].as_u64().expect("parent_gas_limit"),
            };
            let label = format!("{chain} {}", row["note"].as_str().unwrap_or_default());

            // Through the state, exactly as the payload builder calls it.
            let provider = ScrollBaseFeeProvider::new(spec.clone());
            let mut state = State::builder()
                .with_database(EmptyDB::default())
                .with_bundle_update()
                .build();
            if slot_101 != U256::ZERO {
                state.insert_account_with_storage(
                    spec.config.l1_config.l2_system_config_address,
                    Default::default(),
                    PlainStorage::from_iter([(L2_BASE_FEE_OVERHEAD_SLOT, slot_101)]),
                );
            }
            let mut header = parent(inputs.base_fee_per_gas, inputs.gas_used);
            header.gas_limit = inputs.gas_limit;
            header.timestamp = timestamp.saturating_sub(1);
            let from_state = provider.next_block_base_fee(&mut state, &header, timestamp)?;

            // And from extracted inputs, as the block executor calls it.
            let overhead = provider.base_fee_overhead(&mut state)?;
            let from_parts = next_block_base_fee_with_overhead(&spec, &inputs, overhead, timestamp);
            assert_eq!(from_state, from_parts, "{label}: entry points disagree");
            assert_eq!(
                overhead,
                row["overhead"].as_u64().expect("overhead"),
                "{label}: overhead"
            );

            if update {
                row["expected_base_fee"] = from_state.into();
            } else {
                assert_eq!(
                    from_state,
                    row["expected_base_fee"]
                        .as_u64()
                        .expect("expected_base_fee"),
                    "{label}"
                );
            }
        }

        if update {
            std::fs::write(&path, serde_json::to_string_pretty(&document)? + "\n")?;
        }
        Ok(())
    }

    /// Slot 101 is 32 bytes and the overhead is a `u64`, so an overhead the system config
    /// cannot represent saturates rather than wrapping. A guest that wrapped here would
    /// compute a different base fee and split the chain, so this is a parity vector too.
    #[test]
    fn an_oversized_overhead_saturates_to_u64_max() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_OVERHEAD_SLOT, U256::MAX)]),
        );
        assert_eq!(provider.base_fee_overhead(&mut state)?, u64::MAX);

        // Everything above the overhead saturates away, and the cap then applies.
        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1_000_000_000, GAS_LIMIT), 2)?,
            MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn cap_is_10_gwei_before_and_420_000_gwei_from_the_fee_params() {
        assert_eq!(LEGACY_MAX_L2_BASE_FEE, 10_000_000_000);
        assert_eq!(MAX_L2_BASE_FEE, 420_000_000_000_000);
        let activation = dogeos_chainspec::DOGEOS_CHIKYU_TSUKI_FEE_PARAMS_TIME;
        for (timestamp, cap) in [
            (0, LEGACY_MAX_L2_BASE_FEE),
            (1_790_030_900, LEGACY_MAX_L2_BASE_FEE),
            (activation - 1, LEGACY_MAX_L2_BASE_FEE),
            (activation, MAX_L2_BASE_FEE),
        ] {
            assert_eq!(
                max_l2_base_fee_at_timestamp(&**DOGEOS_CHIKYU, timestamp),
                cap
            );
        }
        assert_eq!(
            max_l2_base_fee_at_timestamp(&**DOGEOS_MAINNET, 0),
            MAX_L2_BASE_FEE
        );
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
        // A full block with a 0.1 gwei overhead grows the excess by 1/8 before activation and
        // 9/48 afterwards, and the clamp moves from 10 gwei to 420,000 gwei at the same child
        // timestamp. Use literal historical fee values.
        for (timestamp, parent_base_fee, expected) in [
            (tsuki - 1, 1_000_000_000, 1_112_500_000),
            (tsuki, 1_000_000_000, 1_112_500_000),
            (activation - 1, 1_000_000_000, 1_112_500_000),
            (activation, 1_000_000_000, 1_168_750_000),
            (activation + 1, 1_000_000_000, 1_168_750_000),
            (tsuki, 9_500_000_000, LEGACY_MAX_L2_BASE_FEE),
            (activation - 1, 9_500_000_000, LEGACY_MAX_L2_BASE_FEE),
            (activation, 9_500_000_000, 11_262_500_000),
        ] {
            let mut parent = parent(parent_base_fee, GAS_LIMIT);
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

    fn chikyu_state_with_overhead(overhead: u64) -> State<EmptyDB> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            DOGEOS_CHIKYU.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_OVERHEAD_SLOT, U256::from(overhead))]),
        );
        state
    }

    #[test]
    fn chikyu_clamps_at_10_gwei_until_its_fee_params_time() -> eyre::Result<()> {
        let provider = ScrollBaseFeeProvider::new(DOGEOS_CHIKYU.clone());
        let activation = dogeos_chainspec::DOGEOS_CHIKYU_TSUKI_FEE_PARAMS_TIME;
        for (timestamp, expected) in [
            (1_790_030_900, LEGACY_MAX_L2_BASE_FEE),
            (activation - 1, LEGACY_MAX_L2_BASE_FEE),
            (activation, 11_262_500_000),
        ] {
            let mut parent = parent(9_500_000_000, GAS_LIMIT);
            parent.timestamp = timestamp - 1;
            assert_eq!(
                provider.next_block_base_fee(
                    &mut chikyu_state_with_overhead(100_000_000),
                    &parent,
                    timestamp
                )?,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn prediction_applies_the_fee_params_to_a_parent_one_second_before_activation()
    -> eyre::Result<()> {
        let activation = dogeos_chainspec::DOGEOS_CHIKYU_TSUKI_FEE_PARAMS_TIME;
        let provider = ScrollBaseFeeProvider::new(DOGEOS_CHIKYU.clone());
        let mut state = chikyu_state_with_overhead(100_000_000);
        // A full parent at 9.5 gwei with a 0.1 gwei overhead: D8/E2 clamps at 10 gwei, while
        // D48/E10 under the 420,000 gwei cap gives 11.2625 gwei.
        for (parent_timestamp, expected) in [
            (activation - 2, LEGACY_MAX_L2_BASE_FEE),
            (activation - 1, 11_262_500_000),
            (activation, 11_262_500_000),
        ] {
            let mut parent = parent(9_500_000_000, GAS_LIMIT);
            parent.timestamp = parent_timestamp;
            assert_eq!(
                provider.predict_next_block_base_fee(&mut state, &parent)?,
                expected
            );
            assert_eq!(
                provider.predict_next_block_base_fee(&mut state, &parent)?,
                provider.next_block_base_fee(&mut state, &parent, parent_timestamp + 1)?
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
