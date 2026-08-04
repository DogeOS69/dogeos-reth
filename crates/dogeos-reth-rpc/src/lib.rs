//! Reth RPC adapters for DogeOS' inherited Scroll protocol.

mod receipt;
pub use receipt::{ScrollReceiptBuilder, ScrollReceiptConverter};

mod sequencer;
pub use sequencer::{SequencerClient, SequencerClientError, SequencerConnectError};
mod forwarder;
pub use forwarder::DogeosRawTransactionForwarder;

mod witness;
pub use witness::DogeosDebugWitnessApi;

mod priority_fee;
pub use priority_fee::{DEFAULT_MIN_SUGGESTED_PRIORITY_FEE, DogeosPriorityFeeApi};

mod transaction;
pub use transaction::{
    DogeosRpcConverter, ScrollRpcTxConverter, ScrollSimTxConverter, ScrollTxEnvConverter,
    ScrollTxInfoMapper, dogeos_rpc_converter,
};
