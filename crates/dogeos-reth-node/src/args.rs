use crate::payload::DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT;
use alloy_primitives::Address;
use dogeos_chainspec::{DOGEOS_DEV, DogeosChainSpec};
use dogeos_reth_rpc::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE;
use reth_chainspec::EthChainSpec;

/// Scroll-compatible runtime policy exposed by the standalone DogeOS node.
#[derive(Clone, Debug, clap::Args, PartialEq, Eq)]
pub struct DogeosRollupArgs {
    /// Enables bounded experimental parent-state proofs on Eth-selected RPC transports.
    #[arg(long = "rpc.experimental-multiproof", default_value_t = false)]
    pub experimental_multiproof: bool,
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

impl DogeosRollupArgs {
    /// Rejects an unauthenticated `scroll/1` importer on non-development chains.
    pub fn validate_for_chain(&self, chain_spec: &DogeosChainSpec) -> eyre::Result<()> {
        let is_dev = chain_spec.chain() == DOGEOS_DEV.chain();
        if self.enable_scroll_wire && self.scroll_wire_signer.is_none() && !is_dev {
            eyre::bail!(
                "--network.valid-signer is required when --network.scroll-wire=true on non-development chains"
            );
        }
        Ok(())
    }
}

impl Default for DogeosRollupArgs {
    fn default() -> Self {
        Self {
            experimental_multiproof: false,
            sequencer: None,
            min_suggested_priority_fee: DEFAULT_MIN_SUGGESTED_PRIORITY_FEE,
            payload_size_limit: DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT,
            enable_scroll_wire: true,
            scroll_wire_signer: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: DogeosRollupArgs,
    }

    #[test]
    fn multiproof_requires_explicit_flag() {
        assert!(
            !Cli::try_parse_from(["node"])
                .unwrap()
                .args
                .experimental_multiproof
        );
        let enabled = Cli::try_parse_from(["node", "--rpc.experimental-multiproof"]).unwrap();
        assert!(enabled.args.experimental_multiproof);
        assert_eq!(enabled.args.sequencer, None);
        assert_eq!(enabled.args.scroll_wire_signer, None);
    }
}
