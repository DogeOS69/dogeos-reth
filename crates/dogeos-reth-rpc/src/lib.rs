//! Reth RPC adapters for DogeOS' inherited Scroll protocol.

mod receipt;
pub use receipt::{ScrollReceiptBuilder, ScrollReceiptConverter};

mod transaction;
pub use transaction::{
    DogeosRpcConverter, ScrollRpcTxConverter, ScrollSimTxConverter, ScrollTxEnvConverter,
    ScrollTxInfoMapper, dogeos_rpc_converter,
};
