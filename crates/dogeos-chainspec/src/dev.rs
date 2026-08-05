use crate::{DogeosChainSpec, LazyLock, ScrollChainConfig, build_spec};
use alloc::sync::Arc;
use alloy_chains::Chain;
use dogeos_hardforks::DogeosHardfork;

/// DogeOS development-network specification.
pub static DOGEOS_DEV: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/dev.json"))
        .expect("valid DogeOS development genesis");
    build_spec(
        genesis,
        ScrollChainConfig::dev(),
        DogeosHardfork::dev(),
        None,
        Some(Chain::dev()),
    )
    .into()
});
