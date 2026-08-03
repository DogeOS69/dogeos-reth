//! Reth RPC adapters for DogeOS' inherited Scroll protocol.

mod receipt;
pub use receipt::{ScrollReceiptBuilder, ScrollReceiptConverter};

mod transaction;
pub use transaction::{ScrollRpcTxConverter, ScrollSimTxConverter, ScrollTxInfoMapper};
