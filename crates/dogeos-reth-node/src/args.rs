use crate::payload::DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT;
use alloy_primitives::Address;
use dogeos_reth_rpc::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE;

/// Scroll-compatible runtime policy exposed by the standalone DogeOS node.
#[derive(Clone, Debug, clap::Args, PartialEq, Eq)]
pub struct DogeosRollupArgs {
    /// Endpoint for the sequencer mempool.
    #[arg(long = "scroll.sequencer")]
    pub sequencer: Option<String>,

    /// Minimum suggested priority fee (tip) in wei.
    #[arg(
        long = "scroll.min-suggested-priority-fee",
        default_value_t = DEFAULT_MIN_SUGGESTED_PRIORITY_FEE
    )]
    pub min_suggested_priority_fee: u64,

    /// Maximum encoded transaction bytes selected for one payload.
    #[arg(
        long = "scroll.payload-size-limit",
        default_value_t = DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT
    )]
    pub payload_size_limit: u64,

    /// Enables the inherited `scroll/1` RLPx sub-protocol.
    #[arg(long = "network.scroll-wire", default_value_t = true, action = clap::ArgAction::Set)]
    pub enable_scroll_wire: bool,

    /// Optional signer required for blocks received through `scroll/1`.
    #[arg(long = "network.valid-signer", value_name = "ADDRESS")]
    pub scroll_wire_signer: Option<Address>,
}

impl Default for DogeosRollupArgs {
    fn default() -> Self {
        Self {
            sequencer: None,
            min_suggested_priority_fee: DEFAULT_MIN_SUGGESTED_PRIORITY_FEE,
            payload_size_limit: DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT,
            enable_scroll_wire: true,
            scroll_wire_signer: None,
        }
    }
}
