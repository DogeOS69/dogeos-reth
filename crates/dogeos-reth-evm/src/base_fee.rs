use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use reth_chainspec::EthChainSpec;
use revm::Database;

/// Default operating maximum selected by node base-fee policy (300,000 Gwei).
pub const DEFAULT_OPERATING_MAX_L2_BASE_FEE: u64 = 300_000_000_000_000;

/// L2 base-fee overhead slot in the system config contract.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Default overhead (420 Gwei), and thus effective floor, when the system-config slot is zero.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([420_000_000_000, 0, 0, 0]);

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
            .min(DEFAULT_OPERATING_MAX_L2_BASE_FEE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogeos_chainspec::DOGEOS_MAINNET;
    use revm::database::{EmptyDB, State, states::plain_account::PlainStorage};

    fn parent(base_fee: u64, gas_used: u64) -> alloy_consensus::Header {
        alloy_consensus::Header {
            base_fee_per_gas: Some(base_fee),
            gas_limit: 30_000_000,
            gas_used,
            timestamp: 1,
            ..Default::default()
        }
    }

    #[test]
    fn default_overhead_sets_the_base_fee_floor() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1_000_000_000, 3_000_000), 2)?,
            DEFAULT_BASE_FEE_OVERHEAD.saturating_to::<u64>()
        );
        Ok(())
    }

    #[test]
    fn configured_overhead_is_read_from_state() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        state.insert_account_with_storage(
            DOGEOS_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            PlainStorage::from_iter([(L2_BASE_FEE_OVERHEAD_SLOT, U256::ONE)]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1_000_000_000, 3_000_000), 2)?,
            1_000_000_000
        );
        Ok(())
    }

    #[test]
    fn base_fee_is_capped() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(
                &mut state,
                &parent(DEFAULT_OPERATING_MAX_L2_BASE_FEE, 30_000_000),
                2,
            )?,
            DEFAULT_OPERATING_MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn base_fee_calculation_is_safe_at_the_hard_limit() -> eyre::Result<()> {
        let mut state = State::builder()
            .with_database(EmptyDB::default())
            .with_bundle_update()
            .build();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(
                &mut state,
                &parent(1_000_000_000_000_000, 30_000_000),
                2,
            )?,
            DEFAULT_OPERATING_MAX_L2_BASE_FEE
        );
        Ok(())
    }
}
