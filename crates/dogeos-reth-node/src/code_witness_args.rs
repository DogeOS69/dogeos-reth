use dogeos_reth_txpool::CodeWitnessValidationConfig;

/// Local code witness policy shared by transaction admission and payload building.
#[derive(Clone, Copy, Debug, clap::Args, PartialEq, Eq)]
pub struct CodeWitnessArgs {
    /// Maximum distinct bytecode bytes per block and per admission simulation.
    /// This is an operator policy, not a measured prover capacity guarantee.
    #[arg(
        long = "scroll.max-code-witness-bytes",
        default_value_t = CodeWitnessValidationConfig::default().max_code_witness_bytes,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub max_code_witness_bytes: u64,

    /// Maximum EVM instruction steps per transaction admission simulation.
    #[arg(
        long = "txpool.code-witness-max-steps",
        default_value_t = CodeWitnessValidationConfig::default().max_steps,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub max_steps: u64,

    /// Maximum queued and running admission validations. Validation workers control concurrency.
    #[arg(
        long = "txpool.code-witness-max-inflight",
        default_value_t = CodeWitnessValidationConfig::default().max_inflight,
        value_parser = parse_nonzero_usize
    )]
    pub max_inflight: usize,
}

impl CodeWitnessArgs {
    /// Builds the transaction admission policy using the same byte limit as payload building.
    pub const fn validation_config(&self) -> CodeWitnessValidationConfig {
        CodeWitnessValidationConfig {
            max_code_witness_bytes: self.max_code_witness_bytes,
            max_steps: self.max_steps,
            max_inflight: self.max_inflight,
        }
    }
}

impl Default for CodeWitnessArgs {
    fn default() -> Self {
        let config = CodeWitnessValidationConfig::default();
        Self {
            max_code_witness_bytes: config.max_code_witness_bytes,
            max_steps: config.max_steps,
            max_inflight: config.max_inflight,
        }
    }
}

fn parse_nonzero_usize(value: &str) -> Result<usize, String> {
    let value = value.parse::<usize>().map_err(|error| error.to_string())?;
    if value == 0 || value > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(format!(
            "value must be between 1 and {}",
            tokio::sync::Semaphore::MAX_PERMITS
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: crate::DogeosRollupArgs,
    }

    #[test]
    fn defaults_match_admission_policy() {
        let args = Cli::parse_from(["node"]).args.code_witness;
        assert_eq!(args, CodeWitnessArgs::default());
        let config = args.validation_config();
        let defaults = CodeWitnessValidationConfig::default();
        assert_eq!(
            config.max_code_witness_bytes,
            defaults.max_code_witness_bytes
        );
        assert_eq!(config.max_steps, defaults.max_steps);
        assert_eq!(config.max_inflight, defaults.max_inflight);
        assert_eq!(
            crate::DogeosPayloadBuilderBuilder::default().max_code_witness_bytes,
            config.max_code_witness_bytes
        );
    }

    #[test]
    fn configured_values_reach_admission_policy() {
        let args = Cli::parse_from([
            "node",
            "--scroll.max-code-witness-bytes",
            "4096",
            "--txpool.code-witness-max-steps",
            "2000",
            "--txpool.code-witness-max-inflight",
            "16",
        ])
        .args
        .code_witness;
        let config = args.validation_config();
        assert_eq!(config.max_code_witness_bytes, 4096);
        assert_eq!(config.max_steps, 2000);
        assert_eq!(config.max_inflight, 16);
    }

    #[test]
    fn rejects_zero_and_out_of_range_limits() {
        for flag in [
            "--scroll.max-code-witness-bytes",
            "--txpool.code-witness-max-steps",
            "--txpool.code-witness-max-inflight",
        ] {
            for value in ["0", "-1", "18446744073709551616"] {
                assert!(Cli::try_parse_from(["node", flag, value]).is_err());
            }
        }
        assert!(
            Cli::try_parse_from([
                "node",
                "--txpool.code-witness-max-inflight",
                &usize::MAX.to_string(),
            ])
            .is_err()
        );
    }
}
