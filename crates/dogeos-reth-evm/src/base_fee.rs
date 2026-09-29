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
        let system_config = self.0.chain_config().l1_config.l2_system_config_address;
        let configured_overhead = db.storage(system_config, L2_BASE_FEE_OVERHEAD_SLOT)?;
        let overhead = if configured_overhead == U256::ZERO {
            DEFAULT_BASE_FEE_OVERHEAD
        } else {
            configured_overhead
        }
        .saturating_to::<u64>();

        let parent_base_fee = parent
            .base_fee_per_gas()
            .expect("Feynman+ parent headers carry a base fee");
        let parent_eip1559_base_fee = parent_base_fee.saturating_sub(overhead);
        let next_eip1559_base_fee = calc_next_block_base_fee(
            parent.gas_used(),
            parent.gas_limit(),
            parent_eip1559_base_fee,
            self.0.base_fee_params_at_timestamp(timestamp),
        );

        Ok(next_eip1559_base_fee
            .saturating_add(overhead)
            .min(MAX_L2_BASE_FEE))
    }
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
