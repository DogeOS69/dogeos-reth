use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use core::fmt;
pub use dogeos_chainspec::{
    BASE_FEE_FLOOR, DYNAMIC_BASE_FEE_GAS_TARGET, DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    INITIAL_CONTROLLED_BASE_FEE, LEGACY_MAX_L2_BASE_FEE, MAX_L2_BASE_FEE,
};
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
use dogeos_hardforks::DogeosHardforks;
use reth_chainspec::EthChainSpec;
use revm::Database;

/// Stable namespace for the next-block controlled base-fee storage slot.
pub const NEXT_CONTROLLED_BASE_FEE_SLOT_NAMESPACE: &str =
    "dogeos.storage.dynamic_base_fee.next_controlled_fee";

/// Keccak-256-derived system-config slot containing the next block's controlled base fee.
pub const NEXT_CONTROLLED_BASE_FEE_SLOT: U256 = U256::from_be_bytes([
    0x74, 0xae, 0x89, 0x7e, 0xd5, 0x75, 0x1d, 0xd3, 0x24, 0x19, 0xf1, 0xee, 0xe8, 0xd4, 0xec, 0x13,
    0xd2, 0x96, 0xad, 0xf0, 0xd7, 0x79, 0x78, 0xea, 0x55, 0xdf, 0x0d, 0xd1, 0x83, 0x45, 0xc8, 0xe3,
]);

/// L2 base-fee overhead slot in the system config contract.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Default overhead when the system config contract has not initialized the slot.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// Precision retained for external callers that share the inherited Scroll fee constants.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// Protocol-level failures in the Tsuki utilization controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynamicBaseFeeError {
    /// The persisted controlled component is outside its consensus range.
    ControlledFeeOutOfRange(U256),
    /// A checked arithmetic operation failed.
    ArithmeticOverflow,
}

impl fmt::Display for DynamicBaseFeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlledFeeOutOfRange(value) => write!(
                f,
                "controlled base fee {value} is outside protocol range {BASE_FEE_FLOOR}..={MAX_L2_BASE_FEE}"
            ),
            Self::ArithmeticOverflow => f.write_str("dynamic base-fee arithmetic overflow"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DynamicBaseFeeError {}

/// State access or protocol failure while deriving an L2 base fee.
#[derive(Debug)]
pub enum BaseFeeError<E> {
    /// Reading the L2 system-config account failed.
    Database(E),
    /// The state-backed controller encountered invalid protocol state or arithmetic.
    Protocol(DynamicBaseFeeError),
}

impl<E: fmt::Display> fmt::Display for BaseFeeError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "failed to read L2 system config: {error}"),
            Self::Protocol(error) => error.fmt(f),
        }
    }
}

#[cfg(feature = "std")]
impl<E> std::error::Error for BaseFeeError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Protocol(error) => Some(error),
        }
    }
}

impl<E> From<DynamicBaseFeeError> for BaseFeeError<E> {
    fn from(value: DynamicBaseFeeError) -> Self {
        Self::Protocol(value)
    }
}

/// State components used to validate or assemble a Tsuki base fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DynamicBaseFeeState {
    /// Utilization-controlled component accepted by the current block.
    pub controlled_fee: u64,
    /// L1-congestion adjustment read from system-config state.
    pub overhead: U256,
}

impl DynamicBaseFeeState {
    /// Composes the single header fee from the independently controlled components.
    pub fn header_base_fee(self) -> u64 {
        U256::from(self.controlled_fee)
            .saturating_add(self.overhead)
            .min(U256::from(MAX_L2_BASE_FEE))
            .to::<u64>()
    }
}

/// Calculates the controlled component for the block following `gas_used`.
pub fn calculate_next_controlled_base_fee(
    controlled_fee: u64,
    gas_used: u64,
) -> Result<u64, DynamicBaseFeeError> {
    if !(BASE_FEE_FLOOR..=MAX_L2_BASE_FEE).contains(&controlled_fee) {
        return Err(DynamicBaseFeeError::ControlledFeeOutOfRange(U256::from(
            controlled_fee,
        )));
    }

    let controlled_fee = u128::from(controlled_fee);
    let target = u128::from(DYNAMIC_BASE_FEE_GAS_TARGET);
    let denominator = u128::from(DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR);
    let raw = match gas_used.cmp(&DYNAMIC_BASE_FEE_GAS_TARGET) {
        core::cmp::Ordering::Equal => controlled_fee,
        core::cmp::Ordering::Greater => {
            let gas_delta = u128::from(gas_used - DYNAMIC_BASE_FEE_GAS_TARGET);
            let fee_delta = controlled_fee
                .checked_mul(gas_delta)
                .ok_or(DynamicBaseFeeError::ArithmeticOverflow)?
                / target
                / denominator;
            controlled_fee
                .checked_add(fee_delta.max(1))
                .ok_or(DynamicBaseFeeError::ArithmeticOverflow)?
        }
        core::cmp::Ordering::Less => {
            let gas_delta = u128::from(DYNAMIC_BASE_FEE_GAS_TARGET - gas_used);
            let fee_delta = controlled_fee
                .checked_mul(gas_delta)
                .ok_or(DynamicBaseFeeError::ArithmeticOverflow)?
                / target
                / denominator;
            controlled_fee
                .checked_sub(fee_delta)
                .ok_or(DynamicBaseFeeError::ArithmeticOverflow)?
        }
    };

    let clamped = raw.clamp(u128::from(BASE_FEE_FLOOR), u128::from(MAX_L2_BASE_FEE));
    u64::try_from(clamped).map_err(|_| DynamicBaseFeeError::ArithmeticOverflow)
}

/// State-aware Feynman+/Tsuki L2 base-fee calculator.
#[derive(Clone, Debug, Default)]
pub struct ScrollBaseFeeProvider<ChainSpec>(ChainSpec);

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec> {
    pub const fn new(chain_spec: ChainSpec) -> Self {
        Self(chain_spec)
    }
}

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec>
where
    ChainSpec: DogeosHardforks + ChainConfig<Config = ScrollChainConfig>,
{
    /// Reads the components used by a Tsuki-active block from parent state.
    pub fn dynamic_base_fee_state<DB>(
        &self,
        db: &mut DB,
    ) -> Result<DynamicBaseFeeState, BaseFeeError<DB::Error>>
    where
        DB: Database,
    {
        let system_config = self.0.chain_config().l1_config.l2_system_config_address;
        let configured_overhead = db
            .storage(system_config, L2_BASE_FEE_OVERHEAD_SLOT)
            .map_err(BaseFeeError::Database)?;
        let overhead = if configured_overhead == U256::ZERO {
            DEFAULT_BASE_FEE_OVERHEAD
        } else {
            configured_overhead
        };

        let stored_controlled = db
            .storage(system_config, NEXT_CONTROLLED_BASE_FEE_SLOT)
            .map_err(BaseFeeError::Database)?;
        let controlled_fee = if stored_controlled == U256::ZERO {
            INITIAL_CONTROLLED_BASE_FEE
        } else if stored_controlled < U256::from(BASE_FEE_FLOOR)
            || stored_controlled > U256::from(MAX_L2_BASE_FEE)
        {
            return Err(DynamicBaseFeeError::ControlledFeeOutOfRange(stored_controlled).into());
        } else {
            stored_controlled.to::<u64>()
        };

        Ok(DynamicBaseFeeState {
            controlled_fee,
            overhead,
        })
    }

    /// Calculates the next block's base fee using the current system-config storage.
    pub fn next_block_base_fee<DB, H>(
        &self,
        db: &mut DB,
        parent: &H,
        timestamp: u64,
    ) -> Result<u64, BaseFeeError<DB::Error>>
    where
        DB: Database,
        H: BlockHeader,
        ChainSpec: EthChainSpec,
    {
        if self.0.is_tsuki_active_at_timestamp(timestamp) {
            return Ok(self.dynamic_base_fee_state(db)?.header_base_fee());
        }

        let system_config = self.0.chain_config().l1_config.l2_system_config_address;
        let configured_overhead = db
            .storage(system_config, L2_BASE_FEE_OVERHEAD_SLOT)
            .map_err(BaseFeeError::Database)?;
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
            .min(LEGACY_MAX_L2_BASE_FEE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_protocol_storage_slot;
    use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_MAINNET};
    use revm::database::{EmptyDB, State, states::plain_account::PlainStorage};

    fn parent(base_fee: u64, gas_used: u64) -> alloy_consensus::Header {
        alloy_consensus::Header {
            base_fee_per_gas: Some(base_fee),
            gas_limit: 20_000_000,
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
    fn controlled_slot_is_derived_from_stable_namespace() {
        assert_eq!(
            derive_protocol_storage_slot(NEXT_CONTROLLED_BASE_FEE_SLOT_NAMESPACE),
            NEXT_CONTROLLED_BASE_FEE_SLOT
        );
    }

    #[test]
    fn pre_tsuki_default_overhead_preserves_fee_at_target_gas() -> eyre::Result<()> {
        let mut state = empty_state();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_CHIKYU.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1_000_000_000, 10_000_000), 2)?,
            1_000_000_000
        );
        Ok(())
    }

    #[test]
    fn tsuki_uses_activation_seed_and_default_overhead() -> eyre::Result<()> {
        let mut state = empty_state();
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2)?,
            INITIAL_CONTROLLED_BASE_FEE + DEFAULT_BASE_FEE_OVERHEAD.to::<u64>()
        );
        Ok(())
    }

    #[test]
    fn tsuki_reads_both_components_and_caps_the_sum() -> eyre::Result<()> {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([
                (L2_BASE_FEE_OVERHEAD_SLOT, U256::from(100_000_000)),
                (NEXT_CONTROLLED_BASE_FEE_SLOT, U256::from(MAX_L2_BASE_FEE)),
            ]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2)?,
            MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn tsuki_rejects_an_out_of_range_controlled_fee() {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let invalid = U256::from(MAX_L2_BASE_FEE) + U256::ONE;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(NEXT_CONTROLLED_BASE_FEE_SLOT, invalid)]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert!(matches!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::ControlledFeeOutOfRange(value)
            )) if value == invalid
        ));

        let mut state = empty_state();
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(
                NEXT_CONTROLLED_BASE_FEE_SLOT,
                U256::from(BASE_FEE_FLOOR - 1),
            )]),
        );
        assert!(matches!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::ControlledFeeOutOfRange(_)
            ))
        ));
    }

    #[test]
    fn controller_handles_target_directions_and_clamps() {
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 10_000_000).unwrap(),
            500_000_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 20_000_000).unwrap(),
            562_500_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 0).unwrap(),
            437_500_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(BASE_FEE_FLOOR, 0).unwrap(),
            BASE_FEE_FLOOR
        );
        assert_eq!(
            calculate_next_controlled_base_fee(MAX_L2_BASE_FEE, u64::MAX).unwrap(),
            MAX_L2_BASE_FEE
        );
        assert!(calculate_next_controlled_base_fee(BASE_FEE_FLOOR - 1, 0).is_err());
        assert_eq!(
            calculate_next_controlled_base_fee(BASE_FEE_FLOOR, DYNAMIC_BASE_FEE_GAS_TARGET + 1)
                .unwrap(),
            BASE_FEE_FLOOR + 125
        );
    }
}
