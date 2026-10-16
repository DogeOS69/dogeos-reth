#![doc = include_str!("../README.md")]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/alloy.jpg",
    html_favicon_url = "https://raw.githubusercontent.com/alloy-rs/core/main/assets/favicon.ico"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(not(feature = "std"))]
extern crate alloc as std;

/// Maximum L2 base fee (420,000 gwei): the sequencer clamp and the consensus header limit.
pub const MAX_L2_BASE_FEE: u64 = 420_000_000_000_000;

mod transaction;
pub use transaction::{
    L1_MESSAGE_TRANSACTION_TYPE, L1_MESSAGE_TX_TYPE_ID, ScrollAdditionalInfo,
    ScrollL1MessageTransactionFields, ScrollPooledTransaction, ScrollTransaction,
    ScrollTransactionInfo, ScrollTxEnvelope, ScrollTxType, ScrollTypedTransaction, TxL1Message,
};

mod receipt;
pub use receipt::{ScrollReceiptEnvelope, ScrollReceiptWithBloom, ScrollTransactionReceipt};

#[cfg(feature = "reth-compat")]
mod reth_compat;

#[cfg(feature = "serde")]
pub use transaction::serde_l1_message_tx_rpc;

/// Bincode-compatible serde implementations.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    pub use super::transaction::serde_bincode_compat::*;
}
