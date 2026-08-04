//! Command-line integration for the standalone DogeOS Reth node.

use dogeos_chainspec::{DOGEOS_CHIKYU, DOGEOS_DEV, DOGEOS_MAINNET, DogeosChainSpec};
use reth_cli::chainspec::{ChainSpecParser, parse_genesis};
use std::sync::Arc;

/// Built-in chain names accepted by the DogeOS node.
pub const SUPPORTED_CHAINS: &[&str] = &["dogeos-mainnet", "dogeos-chikyu", "dev"];

/// Parses built-in DogeOS networks and custom genesis JSON files or strings.
#[derive(Clone, Debug, Default)]
pub struct DogeosChainSpecParser;

impl ChainSpecParser for DogeosChainSpecParser {
    type ChainSpec = DogeosChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = SUPPORTED_CHAINS;

    fn parse(value: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        Ok(match value {
            "dogeos-mainnet" => DOGEOS_MAINNET.clone(),
            "dogeos-chikyu" => DOGEOS_CHIKYU.clone(),
            "dev" => DOGEOS_DEV.clone(),
            _ => Arc::new(DogeosChainSpec::from_custom_genesis(parse_genesis(value)?)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::EthChainSpec;

    #[test]
    fn parses_all_builtin_chains() {
        for chain in SUPPORTED_CHAINS {
            assert!(DogeosChainSpecParser::parse(chain).is_ok(), "{chain}");
        }
    }

    #[test]
    fn mainnet_is_the_default_chain() {
        assert_eq!(
            DogeosChainSpecParser::default_value(),
            Some("dogeos-mainnet")
        );
        assert_eq!(
            DogeosChainSpecParser::parse("dogeos-mainnet")
                .unwrap()
                .chain(),
            DOGEOS_MAINNET.chain()
        );
    }
}
