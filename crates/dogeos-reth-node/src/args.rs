use crate::payload::DOGEOS_DEFAULT_PAYLOAD_SIZE_LIMIT;
use alloy_primitives::Address;
use dogeos_chainspec::{DOGEOS_DEV, DogeosChainSpec};
use dogeos_reth_consensus::HARD_MAX_L2_BASE_FEE;
use dogeos_reth_evm::{
    DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER, DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
    DEFAULT_OPERATING_MAX_L2_BASE_FEE, SequencerBaseFeePolicy,
};
use dogeos_reth_rpc::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE;
use reth_chainspec::EthChainSpec;

/// Scroll-compatible runtime policy exposed by the standalone DogeOS node.
#[derive(Clone, Debug, clap::Args, PartialEq, Eq)]
pub struct DogeosRollupArgs {
    #[command(flatten)]
    pub base_fee: SequencerBaseFeeArgs,

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
        self.sequencer_base_fee_policy()?;
        let is_dev = chain_spec.chain() == DOGEOS_DEV.chain();
        if self.enable_scroll_wire && self.scroll_wire_signer.is_none() && !is_dev {
            eyre::bail!(
                "--network.valid-signer is required when --network.scroll-wire=true on non-development chains"
            );
        }
        Ok(())
    }

    /// Returns the validated node-local policy used for payload construction and prediction.
    pub fn sequencer_base_fee_policy(&self) -> eyre::Result<SequencerBaseFeePolicy> {
        Ok(SequencerBaseFeePolicy::try_new(
            self.base_fee.max_change_denominator,
            self.base_fee.elasticity_multiplier,
            self.base_fee.operating_max_base_fee,
            HARD_MAX_L2_BASE_FEE,
        )?)
    }
}

/// CLI settings for the authorized producer's adjustable base-fee policy.
#[derive(Clone, Copy, Debug, clap::Args, PartialEq, Eq)]
pub struct SequencerBaseFeeArgs {
    /// Maximum-change denominator used for payload base-fee adjustment.
    #[arg(
        long = "builder.base-fee-denominator",
        default_value_t = DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR
    )]
    pub max_change_denominator: u128,

    /// Elasticity multiplier used to derive target gas from the parent gas limit.
    #[arg(
        long = "builder.base-fee-elasticity",
        default_value_t = DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER
    )]
    pub elasticity_multiplier: u128,

    /// Maximum base fee selected during payload production, in wei per gas.
    #[arg(
        long = "builder.max-base-fee",
        default_value_t = DEFAULT_OPERATING_MAX_L2_BASE_FEE
    )]
    pub operating_max_base_fee: u64,
}

impl Default for SequencerBaseFeeArgs {
    fn default() -> Self {
        Self {
            max_change_denominator: DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
            elasticity_multiplier: DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER,
            operating_max_base_fee: DEFAULT_OPERATING_MAX_L2_BASE_FEE,
        }
    }
}

pub(crate) fn default_sequencer_base_fee_policy() -> SequencerBaseFeePolicy {
    SequencerBaseFeePolicy::try_new(
        DEFAULT_BASE_FEE_MAX_CHANGE_DENOMINATOR,
        DEFAULT_BASE_FEE_ELASTICITY_MULTIPLIER,
        DEFAULT_OPERATING_MAX_L2_BASE_FEE,
        HARD_MAX_L2_BASE_FEE,
    )
    .expect("default sequencer base-fee policy is valid")
}

impl Default for DogeosRollupArgs {
    fn default() -> Self {
        Self {
            base_fee: SequencerBaseFeeArgs::default(),
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

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        rollup: DogeosRollupArgs,
    }

    #[test]
    fn producer_policy_is_configurable_from_builder_cli_flags() {
        let cli = TestCli::try_parse_from([
            "dogeos-reth",
            "--builder.base-fee-denominator",
            "768",
            "--builder.base-fee-elasticity",
            "8",
            "--builder.max-base-fee",
            "250000000000000",
        ])
        .unwrap();
        let policy = cli.rollup.sequencer_base_fee_policy().unwrap();

        assert_eq!(policy.base_fee_params().max_change_denominator, 768);
        assert_eq!(policy.base_fee_params().elasticity_multiplier, 8);
        assert_eq!(policy.operating_max_base_fee(), 250_000_000_000_000);
        assert!(policy.operating_max_base_fee() <= HARD_MAX_L2_BASE_FEE);
    }

    #[test]
    fn startup_rejects_an_operating_maximum_above_consensus() {
        let args = DogeosRollupArgs {
            base_fee: SequencerBaseFeeArgs {
                operating_max_base_fee: HARD_MAX_L2_BASE_FEE + 1,
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(args.sequencer_base_fee_policy().is_err());
    }
}
