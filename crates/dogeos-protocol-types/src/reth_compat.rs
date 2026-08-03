//! Opt-in Reth trait implementations for Scroll protocol types.
//!
//! These implementations live with the types because Rust's orphan rules prevent
//! the Reth adapter crate from implementing a foreign trait for them.

use crate::{ScrollPooledTransaction, ScrollTxEnvelope, ScrollTypedTransaction};
use reth_primitives_traits::InMemorySize;

impl InMemorySize for ScrollTypedTransaction {
    fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
            Self::L1Message(tx) => tx.size(),
        }
    }
}

impl InMemorySize for ScrollPooledTransaction {
    fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
        }
    }
}

impl InMemorySize for ScrollTxEnvelope {
    fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
            Self::L1Message(tx) => tx.size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_transaction_types_implement_in_memory_size() {
        fn assert_impl<T: InMemorySize>() {}

        assert_impl::<ScrollTypedTransaction>();
        assert_impl::<ScrollPooledTransaction>();
        assert_impl::<ScrollTxEnvelope>();
    }
}
