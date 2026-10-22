use clap::Parser;
use dogeos_reth::DogeosChainSpecParser;
use dogeos_reth_consensus::DogeosConsensus;
use dogeos_reth_evm::ScrollEvmConfig;
use dogeos_reth_node::{DOGEOS_DEFAULT_GAS_LIMIT, DogeosNodeTypes, DogeosRollupArgs};
use reth_ethereum_cli::Cli;
use std::sync::Arc;

fn main() {
    if let Err(error) = Cli::<DogeosChainSpecParser, DogeosRollupArgs>::parse()
        .run_with_components::<DogeosNodeTypes>(
            |chain_spec| {
                (
                    ScrollEvmConfig::dogeos(chain_spec),
                    Arc::new(DogeosConsensus),
                )
            },
            async move |builder, args| {
                args.validate_for_chain(builder.config().chain.as_ref())?;
                let desired_gas_limit = builder
                    .config()
                    .builder
                    .gas_limit
                    .unwrap_or(DOGEOS_DEFAULT_GAS_LIMIT);
                let handle = builder
                    .node(DogeosNodeTypes::new(args).with_desired_gas_limit(desired_gas_limit))
                    .launch_with_debug_capabilities()
                    .await?;
                handle.wait_for_node_exit().await
            },
        )
    {
        eprintln!("Error: {error:?}");
        std::process::exit(1);
    }
}
