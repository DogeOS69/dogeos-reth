use crate::protocol_storage::{ProtocolStorageError, define_protocol_storage_slots};
use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use core::fmt;
use dogeos_chainspec::{ChainConfig, ScrollChainConfig};
pub use dogeos_chainspec::{LEGACY_MAX_L2_BASE_FEE, MAX_L2_BASE_FEE};
use dogeos_hardforks::DogeosHardforks;
use reth_chainspec::EthChainSpec;
use revm::Database;

/// Default minimum Tsuki utilization-controlled base-fee component.
///
/// A SystemConfig override may raise this within the hard protocol maximum.
pub const BASE_FEE_FLOOR: u64 = 10_000_000_000;

/// Default utilization-controlled base-fee component used by the first Tsuki block.
///
/// A SystemConfig override may select a different activation seed.
pub const INITIAL_CONTROLLED_BASE_FEE: u64 = 500_000_000_000;

/// Desired controlled-fee ceiling used to derive [`DEFAULT_MAX_L2_BASE_FEE`].
const DESIRED_CONTROLLED_FEE_CEILING: u64 = 999_900_000_000;

/// Provisional L1-congestion overhead allowance used to derive [`DEFAULT_MAX_L2_BASE_FEE`].
///
/// This is a calibration input, not a separately enforced runtime limit.
const BASE_FEE_OVERHEAD_BUDGET: u64 = 100_000_000;

/// Default SystemConfig maximum for the controlled component and final L2 base fee.
///
/// This remains at the provisional 1,000 Gwei operating limit. SystemConfig may select another
/// value up to [`MAX_L2_BASE_FEE`] without a protocol upgrade.
pub const DEFAULT_MAX_L2_BASE_FEE: u64 = DESIRED_CONTROLLED_FEE_CEILING + BASE_FEE_OVERHEAD_BUDGET;

/// Default long-run gas target for the Tsuki utilization controller.
pub const DYNAMIC_BASE_FEE_GAS_TARGET: u64 = 10_000_000;

/// Default maximum-change denominator for the Tsuki utilization controller.
pub const DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;

const _: () = {
    assert!(DEFAULT_MAX_L2_BASE_FEE == DESIRED_CONTROLLED_FEE_CEILING + BASE_FEE_OVERHEAD_BUDGET);
    assert!(BASE_FEE_FLOOR <= INITIAL_CONTROLLED_BASE_FEE);
    assert!(INITIAL_CONTROLLED_BASE_FEE <= DEFAULT_MAX_L2_BASE_FEE);
    assert!(DEFAULT_MAX_L2_BASE_FEE <= MAX_L2_BASE_FEE);
};

define_protocol_storage_slots! {
    /// Protocol-owned SystemConfig slots used by the Tsuki base-fee controller.
    pub mod dynamic_base_fee_slots {
        /// Controlled base-fee component to use in the next block.
        ///
        /// Zero means uninitialized; the controller then uses [`INITIAL_CONTROLLED_FEE`].
        pub const NEXT_CONTROLLED_FEE: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.next_controlled_fee",
            slot: b256!("74ae897ed5751dd32419f1eee8d4ec13d296adf0d77978ea55df0dd18345c8e3"),
            default: ZeroIsValue,
            min: 0,
            max: super::MAX_L2_BASE_FEE,
        }
        /// Runtime minimum for the controlled component.
        pub const FLOOR: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.floor",
            slot: b256!("eed251e51f4817ab65995267f3e37a3746fff8a6a19d33fe361f1d8ead402881"),
            default: ZeroMeansDefault(super::BASE_FEE_FLOOR),
            min: super::BASE_FEE_FLOOR,
            max: super::MAX_L2_BASE_FEE,
        }
        /// Controlled component used when the next-fee slot has not been initialized.
        pub const INITIAL_CONTROLLED_FEE: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.initial_controlled_fee",
            slot: b256!("d31e852ef679d22e5c189bd0df546bb2a2505ab0c0b1e86eaec6b30d945fe7a4"),
            default: ZeroMeansDefault(super::INITIAL_CONTROLLED_BASE_FEE),
            min: super::BASE_FEE_FLOOR,
            max: super::MAX_L2_BASE_FEE,
        }
        /// Runtime maximum for the controlled component and final header base fee.
        pub const MAXIMUM: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.maximum",
            slot: b256!("aff0674342f28138b41ae357d08382d17f47f8b28fc9d766808e750a6118abb2"),
            default: ZeroMeansDefault(super::DEFAULT_MAX_L2_BASE_FEE),
            min: super::BASE_FEE_FLOOR,
            max: super::MAX_L2_BASE_FEE,
        }
        /// Long-run controller gas target.
        pub const GAS_TARGET: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.gas_target",
            slot: b256!("a7c3d77fa3161deb87f2ccaa84ea1a58c150834dbadc810f9e5d90a6edf1b6b5"),
            default: ZeroMeansDefault(super::DYNAMIC_BASE_FEE_GAS_TARGET),
            min: 1,
            max: 20_000_000,
        }
        /// Denominator limiting the controller's per-block rate of change.
        pub const MAX_CHANGE_DENOMINATOR: u64 {
            namespace: "dogeos.storage.dynamic_base_fee.max_change_denominator",
            slot: b256!("b194ffa17ad5a2b564c8cfef3b3c81bffdf2d555c7d656487014a1692c5e77b4"),
            default: ZeroMeansDefault(super::DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR),
            min: 1,
            // Provisional liveness bound; requires further controller research.
            max: 1_024,
        }
    }
}

use dynamic_base_fee_slots as slots;

/// L2 base-fee overhead slot in the system config contract.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// Default overhead when the system config contract has not initialized the slot.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// Precision retained for external callers that share the inherited Scroll fee constants.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// Predicts the timestamp used by local next-block views when no payload attributes exist.
///
/// The local payload builder enforces a timestamp of at least `parent + 1`. Pending RPC, txpool,
/// and the trailing prediction in `eth_feeHistory` use that deterministic lower bound instead of
/// wall-clock time. Canonical historical queries still prefer the actual successor timestamp and
/// fee whenever that block exists.
pub const fn predict_next_payload_timestamp(parent_timestamp: u64) -> u64 {
    parent_timestamp.saturating_add(1)
}

/// State-backed parameters for the Tsuki utilization controller.
///
/// A zero value in the corresponding SystemConfig slot selects the field's default. The hard
/// [`MAX_L2_BASE_FEE`] bound remains protocol-enforced while the runtime maximum can be lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DynamicBaseFeeParams {
    pub floor: u64,
    pub initial_controlled_fee: u64,
    pub maximum: u64,
    pub gas_target: u64,
    pub max_change_denominator: u64,
}

impl DynamicBaseFeeParams {
    pub const DEFAULT: Self = Self {
        floor: slots::FLOOR.default_value(),
        initial_controlled_fee: slots::INITIAL_CONTROLLED_FEE.default_value(),
        maximum: slots::MAXIMUM.default_value(),
        gas_target: slots::GAS_TARGET.default_value(),
        max_change_denominator: slots::MAX_CHANGE_DENOMINATOR.default_value(),
    };

    fn validate(self) -> Result<Self, DynamicBaseFeeError> {
        if !slots::FLOOR.contains(self.floor)
            || !slots::INITIAL_CONTROLLED_FEE.contains(self.initial_controlled_fee)
            || !slots::MAXIMUM.contains(self.maximum)
            || !slots::GAS_TARGET.contains(self.gas_target)
            || !slots::MAX_CHANGE_DENOMINATOR.contains(self.max_change_denominator)
            || self.floor > self.initial_controlled_fee
            || self.initial_controlled_fee > self.maximum
        {
            return Err(DynamicBaseFeeError::InvalidParameters(self));
        }
        Ok(self)
    }

    /// Rebase a previously valid controller value into a newly configured range.
    pub fn rebase_controlled_fee(self, controlled_fee: u64) -> Result<u64, DynamicBaseFeeError> {
        let params = self.validate()?;
        Ok(controlled_fee.clamp(params.floor, params.maximum))
    }
}

impl Default for DynamicBaseFeeParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Protocol-level failures in the Tsuki utilization controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynamicBaseFeeError {
    /// A configured parameter cannot be represented by the controller.
    ParameterOutOfRange {
        parameter: &'static str,
        value: U256,
    },
    /// A configured parameter falls outside its slot's absolute inclusive bounds.
    ParameterOutsideBounds {
        parameter: &'static str,
        value: U256,
        min: U256,
        max: U256,
    },
    /// The configured parameters violate controller invariants.
    InvalidParameters(DynamicBaseFeeParams),
    /// The persisted controlled component is outside its configured range.
    ControlledFeeOutOfRange {
        value: U256,
        floor: u64,
        maximum: u64,
    },
    /// A checked arithmetic operation failed.
    ArithmeticOverflow,
}

impl fmt::Display for DynamicBaseFeeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParameterOutOfRange { parameter, value } => {
                write!(
                    f,
                    "dynamic base-fee parameter {parameter} does not fit in u64: {value}"
                )
            }
            Self::ParameterOutsideBounds {
                parameter,
                value,
                min,
                max,
            } => write!(
                f,
                "dynamic base-fee parameter {parameter}={value} is outside {min}..={max}"
            ),
            Self::InvalidParameters(params) => write!(
                f,
                "invalid dynamic base-fee parameters: floor={}, initial={}, maximum={}, gas_target={}, denominator={} (hard maximum={MAX_L2_BASE_FEE})",
                params.floor,
                params.initial_controlled_fee,
                params.maximum,
                params.gas_target,
                params.max_change_denominator
            ),
            Self::ControlledFeeOutOfRange {
                value,
                floor,
                maximum,
            } => write!(
                f,
                "controlled base fee {value} is outside configured range {floor}..={maximum}"
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

impl<E> From<ProtocolStorageError<E>> for BaseFeeError<E> {
    fn from(value: ProtocolStorageError<E>) -> Self {
        match value {
            ProtocolStorageError::Database(error) => Self::Database(error),
            ProtocolStorageError::ValueOutOfRange { namespace, value } => {
                Self::Protocol(DynamicBaseFeeError::ParameterOutOfRange {
                    parameter: namespace,
                    value,
                })
            }
            ProtocolStorageError::ValueOutsideBounds {
                namespace,
                value,
                min,
                max,
            } => Self::Protocol(DynamicBaseFeeError::ParameterOutsideBounds {
                parameter: namespace,
                value,
                min,
                max,
            }),
        }
    }
}

/// State components used to validate or assemble a Tsuki base fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DynamicBaseFeeState {
    /// Utilization-controlled component accepted by the current block.
    pub controlled_fee: u64,
    /// L1-congestion adjustment read from system-config state.
    pub overhead: U256,
    /// Controller parameters read from the same parent state.
    pub params: DynamicBaseFeeParams,
}

impl DynamicBaseFeeState {
    /// Composes the single header fee from the independently controlled components.
    pub fn header_base_fee(self) -> u64 {
        U256::from(self.controlled_fee)
            .saturating_add(self.overhead)
            .min(U256::from(self.params.maximum))
            .to::<u64>()
    }
}

/// Calculates the controlled component for the block following `gas_used`.
pub fn calculate_next_controlled_base_fee(
    controlled_fee: u64,
    gas_used: u64,
    params: DynamicBaseFeeParams,
) -> Result<u64, DynamicBaseFeeError> {
    let params = params.validate()?;
    if !(params.floor..=params.maximum).contains(&controlled_fee) {
        return Err(DynamicBaseFeeError::ControlledFeeOutOfRange {
            value: U256::from(controlled_fee),
            floor: params.floor,
            maximum: params.maximum,
        });
    }

    let controlled_fee = u128::from(controlled_fee);
    let target = u128::from(params.gas_target);
    let denominator = u128::from(params.max_change_denominator);
    let raw = match gas_used.cmp(&params.gas_target) {
        core::cmp::Ordering::Equal => controlled_fee,
        core::cmp::Ordering::Greater => {
            let gas_delta = u128::from(gas_used - params.gas_target);
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
            let gas_delta = u128::from(params.gas_target - gas_used);
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

    let clamped = raw.clamp(u128::from(params.floor), u128::from(params.maximum));
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
    /// Reads and validates the Tsuki controller parameters from SystemConfig state.
    pub fn dynamic_base_fee_params<DB>(
        &self,
        db: &mut DB,
    ) -> Result<DynamicBaseFeeParams, BaseFeeError<DB::Error>>
    where
        DB: Database,
    {
        let system_config = self.0.chain_config().l1_config.l2_system_config_address;
        let params = DynamicBaseFeeParams {
            floor: slots::FLOOR.read_parameter(db, system_config)?,
            initial_controlled_fee: slots::INITIAL_CONTROLLED_FEE
                .read_parameter(db, system_config)?,
            maximum: slots::MAXIMUM.read_parameter(db, system_config)?,
            gas_target: slots::GAS_TARGET.read_parameter(db, system_config)?,
            max_change_denominator: slots::MAX_CHANGE_DENOMINATOR
                .read_parameter(db, system_config)?,
        };
        params.validate().map_err(Into::into)
    }

    /// Reads the components used by a Tsuki-active block from parent state.
    pub fn dynamic_base_fee_state<DB>(
        &self,
        db: &mut DB,
    ) -> Result<DynamicBaseFeeState, BaseFeeError<DB::Error>>
    where
        DB: Database,
    {
        let system_config = self.0.chain_config().l1_config.l2_system_config_address;
        let params = self.dynamic_base_fee_params(db)?;
        let configured_overhead = db
            .storage(system_config, L2_BASE_FEE_OVERHEAD_SLOT)
            .map_err(BaseFeeError::Database)?;
        let overhead = if configured_overhead == U256::ZERO {
            DEFAULT_BASE_FEE_OVERHEAD
        } else {
            configured_overhead
        };

        let stored_controlled = db
            .storage(system_config, slots::NEXT_CONTROLLED_FEE.value())
            .map_err(BaseFeeError::Database)?;
        let controlled_fee = if stored_controlled == U256::ZERO {
            params.initial_controlled_fee
        } else if stored_controlled < U256::from(params.floor)
            || stored_controlled > U256::from(params.maximum)
        {
            return Err(DynamicBaseFeeError::ControlledFeeOutOfRange {
                value: stored_controlled,
                floor: params.floor,
                maximum: params.maximum,
            }
            .into());
        } else {
            stored_controlled.to::<u64>()
        };

        Ok(DynamicBaseFeeState {
            controlled_fee,
            overhead,
            params,
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
    use crate::ProtocolStorageDefault::{ZeroIsValue, ZeroMeansDefault};
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
    fn controller_slots_declare_their_zero_value_policies() {
        assert_eq!(slots::NEXT_CONTROLLED_FEE.default_policy(), ZeroIsValue);
        assert_eq!(slots::NEXT_CONTROLLED_FEE.min(), 0);
        assert_eq!(slots::NEXT_CONTROLLED_FEE.max(), MAX_L2_BASE_FEE);
        for (slot, default, min, max) in [
            (
                slots::FLOOR,
                BASE_FEE_FLOOR,
                BASE_FEE_FLOOR,
                MAX_L2_BASE_FEE,
            ),
            (
                slots::INITIAL_CONTROLLED_FEE,
                INITIAL_CONTROLLED_BASE_FEE,
                BASE_FEE_FLOOR,
                MAX_L2_BASE_FEE,
            ),
            (
                slots::MAXIMUM,
                DEFAULT_MAX_L2_BASE_FEE,
                BASE_FEE_FLOOR,
                MAX_L2_BASE_FEE,
            ),
            (
                slots::GAS_TARGET,
                DYNAMIC_BASE_FEE_GAS_TARGET,
                1,
                20_000_000,
            ),
            (
                slots::MAX_CHANGE_DENOMINATOR,
                DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR,
                1,
                1_024,
            ),
        ] {
            assert_eq!(slot.default_policy(), ZeroMeansDefault(default));
            assert_eq!(slot.min(), min);
            assert_eq!(slot.max(), max);
        }
    }

    #[test]
    fn parameter_slots_reject_values_outside_their_absolute_bounds() {
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        for (slot, invalid) in [
            (slots::FLOOR, BASE_FEE_FLOOR - 1),
            (slots::INITIAL_CONTROLLED_FEE, BASE_FEE_FLOOR - 1),
            (slots::MAXIMUM, MAX_L2_BASE_FEE + 1),
            (slots::GAS_TARGET, 20_000_001),
            (slots::MAX_CHANGE_DENOMINATOR, 1_025),
        ] {
            let mut state = empty_state();
            state.insert_account_with_storage(
                address,
                Default::default(),
                PlainStorage::from_iter([(slot.value(), U256::from(invalid))]),
            );

            assert!(matches!(
                slot.read_parameter(&mut state, address),
                Err(ProtocolStorageError::ValueOutsideBounds {
                    namespace,
                    value,
                    min,
                    max,
                }) if namespace == slot.namespace()
                    && value == U256::from(invalid)
                    && min == U256::from(slot.min())
                    && max == U256::from(slot.max())
            ));
        }
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
                (
                    slots::NEXT_CONTROLLED_FEE.value(),
                    U256::from(DEFAULT_MAX_L2_BASE_FEE),
                ),
            ]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2)?,
            DEFAULT_MAX_L2_BASE_FEE
        );
        Ok(())
    }

    #[test]
    fn tsuki_reads_state_backed_controller_parameters() -> eyre::Result<()> {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let expected = DynamicBaseFeeParams {
            floor: 20_000_000_000,
            initial_controlled_fee: 300_000_000_000,
            maximum: 600_000_000_000,
            gas_target: 5_000_000,
            max_change_denominator: 4,
        };
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([
                (slots::FLOOR.value(), U256::from(expected.floor)),
                (
                    slots::INITIAL_CONTROLLED_FEE.value(),
                    U256::from(expected.initial_controlled_fee),
                ),
                (slots::MAXIMUM.value(), U256::from(expected.maximum)),
                (slots::GAS_TARGET.value(), U256::from(expected.gas_target)),
                (
                    slots::MAX_CHANGE_DENOMINATOR.value(),
                    U256::from(expected.max_change_denominator),
                ),
            ]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(provider.dynamic_base_fee_params(&mut state)?, expected);
        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2)?,
            expected.initial_controlled_fee + DEFAULT_BASE_FEE_OVERHEAD.to::<u64>()
        );
        Ok(())
    }

    #[test]
    fn tsuki_caps_header_at_state_backed_maximum() -> eyre::Result<()> {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([
                (L2_BASE_FEE_OVERHEAD_SLOT, U256::from(20_000_000_000u64)),
                (
                    slots::NEXT_CONTROLLED_FEE.value(),
                    U256::from(590_000_000_000u64),
                ),
                (slots::MAXIMUM.value(), U256::from(600_000_000_000u64)),
            ]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert_eq!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2)?,
            600_000_000_000
        );
        Ok(())
    }

    #[test]
    fn tsuki_rejects_invalid_controller_parameters() {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(
                slots::FLOOR.value(),
                U256::from(INITIAL_CONTROLLED_BASE_FEE + 1),
            )]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert!(matches!(
            provider.dynamic_base_fee_params(&mut state),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::InvalidParameters(_)
            ))
        ));

        let mut state = empty_state();
        let oversized = U256::from(u64::MAX) + U256::ONE;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(slots::MAXIMUM.value(), oversized)]),
        );
        assert!(matches!(
            provider.dynamic_base_fee_params(&mut state),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::ParameterOutOfRange {
                    parameter: "dogeos.storage.dynamic_base_fee.maximum",
                    value,
                }
            )) if value == oversized
        ));
    }

    #[test]
    fn tsuki_rejects_an_out_of_range_controlled_fee() {
        let mut state = empty_state();
        let address = DOGEOS_MAINNET.config.l1_config.l2_system_config_address;
        let invalid = U256::from(MAX_L2_BASE_FEE) + U256::ONE;
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(slots::NEXT_CONTROLLED_FEE.value(), invalid)]),
        );
        let provider = ScrollBaseFeeProvider::new(DOGEOS_MAINNET.clone());

        assert!(matches!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::ControlledFeeOutOfRange { value, .. }
            )) if value == invalid
        ));

        let mut state = empty_state();
        state.insert_account_with_storage(
            address,
            Default::default(),
            PlainStorage::from_iter([(
                slots::NEXT_CONTROLLED_FEE.value(),
                U256::from(BASE_FEE_FLOOR - 1),
            )]),
        );
        assert!(matches!(
            provider.next_block_base_fee(&mut state, &parent(1, 0), 2),
            Err(BaseFeeError::Protocol(
                DynamicBaseFeeError::ControlledFeeOutOfRange { .. }
            ))
        ));
    }

    #[test]
    fn controller_handles_target_directions_and_clamps() {
        let params = DynamicBaseFeeParams::default();
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 10_000_000, params).unwrap(),
            500_000_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 20_000_000, params).unwrap(),
            562_500_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(500_000_000_000, 0, params).unwrap(),
            437_500_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(BASE_FEE_FLOOR, 0, params).unwrap(),
            BASE_FEE_FLOOR
        );
        assert_eq!(
            calculate_next_controlled_base_fee(DEFAULT_MAX_L2_BASE_FEE, u64::MAX, params).unwrap(),
            DEFAULT_MAX_L2_BASE_FEE
        );
        assert!(calculate_next_controlled_base_fee(BASE_FEE_FLOOR - 1, 0, params).is_err());
        assert_eq!(
            calculate_next_controlled_base_fee(
                BASE_FEE_FLOOR,
                DYNAMIC_BASE_FEE_GAS_TARGET + 1,
                params,
            )
            .unwrap(),
            BASE_FEE_FLOOR + 125
        );
    }

    #[test]
    fn controller_formula_uses_state_backed_target_and_denominator() {
        let params = DynamicBaseFeeParams {
            floor: 10_000_000_000,
            initial_controlled_fee: 40_000_000_000,
            maximum: 100_000_000_000,
            gas_target: 5_000_000,
            max_change_denominator: 4,
        };

        assert_eq!(
            calculate_next_controlled_base_fee(40_000_000_000, 10_000_000, params).unwrap(),
            50_000_000_000
        );
        assert_eq!(
            calculate_next_controlled_base_fee(40_000_000_000, 0, params).unwrap(),
            30_000_000_000
        );
        assert_eq!(
            params.rebase_controlled_fee(200_000_000_000).unwrap(),
            100_000_000_000
        );
    }

    #[test]
    fn controller_rejects_direct_parameters_outside_slot_bounds() {
        let params = DynamicBaseFeeParams {
            floor: BASE_FEE_FLOOR - 1,
            ..Default::default()
        };
        assert!(calculate_next_controlled_base_fee(BASE_FEE_FLOOR, 0, params).is_err());

        let params = DynamicBaseFeeParams {
            gas_target: slots::GAS_TARGET.max() + 1,
            ..Default::default()
        };
        assert!(calculate_next_controlled_base_fee(BASE_FEE_FLOOR, 0, params).is_err());

        let params = DynamicBaseFeeParams {
            max_change_denominator: slots::MAX_CHANGE_DENOMINATOR.max() + 1,
            ..Default::default()
        };
        assert!(calculate_next_controlled_base_fee(BASE_FEE_FLOOR, 0, params).is_err());

        let params = DynamicBaseFeeParams {
            floor: 100_000_000_000,
            initial_controlled_fee: 100_000_000_000,
            maximum: 10_000_000_000,
            ..Default::default()
        };
        assert!(params.rebase_controlled_fee(50_000_000_000).is_err());
    }

    #[test]
    fn hard_maximum_is_safe_across_controller_and_header_arithmetic() {
        let params = DynamicBaseFeeParams {
            floor: BASE_FEE_FLOOR,
            initial_controlled_fee: INITIAL_CONTROLLED_BASE_FEE,
            maximum: MAX_L2_BASE_FEE,
            gas_target: 1,
            max_change_denominator: 1,
        };

        assert_eq!(
            calculate_next_controlled_base_fee(MAX_L2_BASE_FEE, u64::MAX, params).unwrap(),
            MAX_L2_BASE_FEE
        );
        assert!(
            u128::from(MAX_L2_BASE_FEE)
                .checked_mul(u128::from(u64::MAX))
                .is_some()
        );
        assert_eq!(
            DynamicBaseFeeState {
                controlled_fee: MAX_L2_BASE_FEE,
                overhead: U256::MAX,
                params,
            }
            .header_base_fee(),
            MAX_L2_BASE_FEE
        );
    }
}
