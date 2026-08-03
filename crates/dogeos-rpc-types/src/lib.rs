//! Exact inherited Scroll JSON-RPC compatibility types.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod network;
pub use network::Scroll;
mod receipt;
pub use receipt::{ScrollTransactionReceipt, ScrollTransactionReceiptFields};
mod transaction;
pub use transaction::{
    ScrollL1MessageTransactionFields, ScrollRpcTransaction, ScrollTransactionRequest,
};
