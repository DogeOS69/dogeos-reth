use crate::{DOGEOS_CHIKYU_GENESIS_HASH, DogeosChainSpec, LazyLock, ScrollChainConfig, build_spec};
use alloc::sync::Arc;
use dogeos_hardforks::DogeosHardfork;

/// DogeOS Chikyu testnet specification.
pub static DOGEOS_CHIKYU: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json"))
        .expect("valid DogeOS Chikyu genesis");
    build_spec(
        genesis,
        ScrollChainConfig::dogeos_chikyu(),
        DogeosHardfork::chikyu(),
        Some(DOGEOS_CHIKYU_GENESIS_HASH),
        None,
    )
    .into()
});
