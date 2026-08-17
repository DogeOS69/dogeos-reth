//! Conventions for DogeOS protocol-owned storage outside inherited Solidity layouts.

use alloy_primitives::{Address, B256, U256, keccak256};
use core::fmt;
use revm::Database;

/// Controls how an all-zero storage word is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolStorageDefault<T> {
    /// Treat zero as an unset slot and return the supplied default.
    ZeroMeansDefault(T),
    /// Return zero as an explicitly meaningful value.
    ZeroIsValue,
}

/// Failure while reading a typed protocol storage slot.
#[derive(Debug)]
pub enum ProtocolStorageError<E> {
    /// Reading the backing account storage failed.
    Database(E),
    /// The stored value does not fit the slot's declared value type.
    ValueOutOfRange {
        namespace: &'static str,
        value: U256,
    },
    /// The typed value falls outside the slot's absolute inclusive bounds.
    ValueOutsideBounds {
        namespace: &'static str,
        value: U256,
        min: U256,
        max: U256,
    },
}

impl<E: fmt::Display> fmt::Display for ProtocolStorageError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(f, "failed to read protocol storage: {error}"),
            Self::ValueOutOfRange { namespace, value } => {
                write!(
                    f,
                    "protocol storage value {value} for {namespace:?} does not fit its type"
                )
            }
            Self::ValueOutsideBounds {
                namespace,
                value,
                min,
                max,
            } => write!(
                f,
                "protocol storage value {value} for {namespace:?} is outside {min}..={max}"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<E> std::error::Error for ProtocolStorageError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::ValueOutOfRange { .. } | Self::ValueOutsideBounds { .. } => None,
        }
    }
}

/// A protocol-owned storage slot paired with the namespace from which it was derived.
///
/// The precomputed hash keeps consensus paths const-friendly, while retaining the namespace makes
/// the derivation auditable and testable instead of scattering parallel string and integer
/// constants throughout protocol modules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolStorageSlot<T> {
    namespace: &'static str,
    value: U256,
    default: ProtocolStorageDefault<T>,
    min: T,
    max: T,
}

/// Declares a cohesive group of Keccak-256-derived protocol storage slots.
///
/// Each declaration binds the namespace, precomputed hash, zero policy, and absolute bounds in one
/// place. The generated unit test checks the derivation, bound ordering, and effective default.
macro_rules! define_protocol_storage_slots {
    (
        $(#[$module_meta:meta])*
        $module_visibility:vis mod $module:ident {
            $(
                $(#[$slot_meta:meta])*
                $slot_visibility:vis const $slot:ident: $value_type:ty {
                    namespace: $namespace:literal,
                    slot: $hash:expr,
                    default: $default_policy:expr,
                    min: $min:expr,
                    max: $max:expr $(,)?
                }
            )+
        }
    ) => {
        $(#[$module_meta])*
        $module_visibility mod $module {
            use alloy_primitives::b256;
            use $crate::protocol_storage::{
                ProtocolStorageDefault::{ZeroIsValue, ZeroMeansDefault},
                ProtocolStorageSlot,
            };

            $(
                $(#[$slot_meta])*
                $slot_visibility const $slot: ProtocolStorageSlot<$value_type> =
                    ProtocolStorageSlot::new($namespace, $hash, $default_policy, $min, $max);
            )+

            #[cfg(test)]
            mod tests {
                use super::*;

                #[test]
                fn slots_match_namespaces_and_have_valid_bounds() {
                    $(
                        assert!(
                            $slot.has_valid_derivation(),
                            "{} does not match keccak256({:?})",
                            stringify!($slot),
                            $slot.namespace(),
                        );
                        assert!(
                            $slot.has_valid_bounds(),
                            "{} has invalid absolute bounds",
                            stringify!($slot),
                        );
                        assert!(
                            $slot.contains($slot.default_value()),
                            "{} has a default outside its absolute bounds",
                            stringify!($slot),
                        );
                    )+
                }
            }
        }
    };
}

pub(crate) use define_protocol_storage_slots;

impl<T> ProtocolStorageSlot<T> {
    /// Creates a slot with a stable namespace, precomputed hash, zero policy, and absolute bounds.
    pub const fn new(
        namespace: &'static str,
        hash: B256,
        default: ProtocolStorageDefault<T>,
        min: T,
        max: T,
    ) -> Self {
        Self {
            namespace,
            value: U256::from_be_bytes(hash.0),
            default,
            min,
            max,
        }
    }

    /// Returns the domain-separated namespace documenting this slot.
    pub const fn namespace(&self) -> &'static str {
        self.namespace
    }

    /// Returns the storage key used by the database.
    pub const fn value(&self) -> U256 {
        self.value
    }

    /// Verifies that the precomputed value is the Keccak-256 hash of the namespace.
    pub fn has_valid_derivation(&self) -> bool {
        derive_protocol_storage_slot(self.namespace) == self.value
    }
}

impl<T: Copy> ProtocolStorageSlot<T> {
    /// Returns the slot's declared zero-value policy.
    pub const fn default_policy(&self) -> ProtocolStorageDefault<T> {
        self.default
    }

    /// Returns the slot's absolute inclusive minimum.
    pub const fn min(&self) -> T {
        self.min
    }

    /// Returns the slot's absolute inclusive maximum.
    pub const fn max(&self) -> T {
        self.max
    }
}

impl<T: Copy + Ord> ProtocolStorageSlot<T> {
    /// Returns whether the slot's absolute inclusive bounds are well ordered.
    pub fn has_valid_bounds(&self) -> bool {
        self.min <= self.max
    }

    /// Returns whether a value satisfies the slot's absolute inclusive bounds.
    pub fn contains(&self, value: T) -> bool {
        (self.min..=self.max).contains(&value)
    }
}

impl ProtocolStorageSlot<u64> {
    /// Returns the effective value selected for an all-zero storage word.
    pub const fn default_value(&self) -> u64 {
        match self.default {
            ProtocolStorageDefault::ZeroMeansDefault(value) => value,
            ProtocolStorageDefault::ZeroIsValue => 0,
        }
    }

    /// Reads a `u64` parameter using this slot's declared zero-value policy.
    pub fn read_parameter<DB>(
        &self,
        db: &mut DB,
        address: Address,
    ) -> Result<u64, ProtocolStorageError<DB::Error>>
    where
        DB: Database,
    {
        let value = db
            .storage(address, self.value)
            .map_err(ProtocolStorageError::Database)?;
        let value = if value == U256::ZERO {
            self.default_value()
        } else if value > U256::from(u64::MAX) {
            return Err(ProtocolStorageError::ValueOutOfRange {
                namespace: self.namespace,
                value,
            });
        } else {
            value.to::<u64>()
        };
        if !self.contains(value) {
            return Err(ProtocolStorageError::ValueOutsideBounds {
                namespace: self.namespace,
                value: U256::from(value),
                min: U256::from(self.min),
                max: U256::from(self.max),
            });
        }
        Ok(value)
    }
}

/// Derives a protocol-owned storage slot from a stable, domain-separated namespace.
///
/// New DogeOS protocol slots must use this convention instead of allocating sequential integer
/// slots. Consensus code should declare a [`ProtocolStorageSlot`] and test its derivation.
pub fn derive_protocol_storage_slot(namespace: &str) -> U256 {
    U256::from_be_bytes(keccak256(namespace).0)
}
