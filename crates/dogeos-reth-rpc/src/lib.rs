//! Reth RPC adapters for DogeOS' inherited Scroll protocol.

mod receipt;
pub use receipt::{ScrollReceiptBuilder, ScrollReceiptConverter};

mod sequencer;
pub use sequencer::{SequencerClient, SequencerClientError, SequencerConnectError};

mod witness;
pub use witness::DogeosDebugWitnessApi;

mod transaction;
pub use transaction::{
    DogeosRpcConverter, ScrollRpcTxConverter, ScrollSimTxConverter, ScrollTxEnvConverter,
    ScrollTxInfoMapper, dogeos_rpc_converter,
};
