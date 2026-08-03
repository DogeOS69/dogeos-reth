//! Scroll primitives transaction types.

pub mod tx_type;

/// Signed transaction.
pub type ScrollTransactionSigned = dogeos_protocol_types::ScrollTxEnvelope;
