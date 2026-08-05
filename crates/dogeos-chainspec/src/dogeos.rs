use crate::{
    DOGEOS_MAINNET_GENESIS_HASH, DogeosChainSpec, LazyLock, ScrollChainConfig, build_spec,
};
use alloc::sync::Arc;
use alloy_chains::Chain;
use dogeos_hardforks::DogeosHardfork;

/// DogeOS mainnet specification retained from the current node.
pub static DOGEOS_MAINNET: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/dogeos.json"))
        .expect("valid DogeOS mainnet genesis");
    build_spec(
        genesis,
        ScrollChainConfig::dogeos_mainnet(),
        DogeosHardfork::mainnet(),
        Some(DOGEOS_MAINNET_GENESIS_HASH),
        Some(Chain::from_id(0xff)),
    )
    .into()
});
