//! Conventions for DogeOS protocol-owned storage outside inherited Solidity layouts.

use alloy_primitives::{U256, keccak256};

/// Derives a protocol-owned storage slot from a stable, domain-separated namespace.
///
/// New DogeOS protocol slots must use this convention instead of allocating sequential integer
/// slots. Consensus code should store a precomputed constant and test it against this function.
pub fn derive_protocol_storage_slot(namespace: &str) -> U256 {
    U256::from_be_bytes(keccak256(namespace).0)
}
