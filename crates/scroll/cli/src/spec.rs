use reth_cli::chainspec::{parse_genesis, ChainSpecParser};
use reth_scroll_chainspec::{
    ScrollChainSpec, DOGEOS_CHIKYU, DOGEOS_MAINNET, SCROLL_DEV, SCROLL_MAINNET, SCROLL_SEPOLIA,
};
use std::sync::Arc;

/// The parser for the Scroll chain specification.
#[derive(Debug, Clone)]
pub struct ScrollChainSpecParser;

impl ChainSpecParser for ScrollChainSpecParser {
    type ChainSpec = ScrollChainSpec;
    const SUPPORTED_CHAINS: &'static [&'static str] =
        &["dev", "scroll-mainnet", "scroll-sepolia", "dogeos-mainnet", "dogeos-chikyu"];

    fn parse(s: &str) -> eyre::Result<Arc<Self::ChainSpec>> {
        Ok(match s {
            "dev" => SCROLL_DEV.clone(),
            "scroll-mainnet" => SCROLL_MAINNET.clone(),
            "scroll-sepolia" => SCROLL_SEPOLIA.clone(),
            "dogeos-mainnet" => DOGEOS_MAINNET.clone(),
            "dogeos-chikyu" => DOGEOS_CHIKYU.clone(),
            _ => Arc::new(ScrollChainSpec::from_custom_genesis(parse_genesis(s)?)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dogeos_builtin_chains() {
        assert!(ScrollChainSpecParser::SUPPORTED_CHAINS.contains(&"dogeos-mainnet"));
        assert!(ScrollChainSpecParser::SUPPORTED_CHAINS.contains(&"dogeos-chikyu"));

        let mainnet = ScrollChainSpecParser::parse("dogeos-mainnet").unwrap();
        let chikyu = ScrollChainSpecParser::parse("dogeos-chikyu").unwrap();

        assert!(Arc::ptr_eq(&mainnet, &DOGEOS_MAINNET.clone()));
        assert!(Arc::ptr_eq(&chikyu, &DOGEOS_CHIKYU.clone()));
    }
}
