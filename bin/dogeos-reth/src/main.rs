use clap::Parser;
use dogeos_reth::DogeosChainSpecParser;
use dogeos_reth_consensus::DogeosConsensus;
use dogeos_reth_evm::ScrollEvmConfig;
use dogeos_reth_node::{DogeosNodeTypes, DogeosRollupArgs};
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
                let handle = builder
                    .node(DogeosNodeTypes::new(args))
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
