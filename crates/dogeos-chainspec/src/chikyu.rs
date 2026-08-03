use crate::{DOGEOS_CHIKYU_GENESIS_HASH, DogeosChainSpec, LazyLock, ScrollChainConfig, build_spec};
use alloc::sync::Arc;

/// DogeOS Chikyu testnet specification.
pub static DOGEOS_CHIKYU: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json"))
        .expect("valid DogeOS Chikyu genesis");
    build_spec(
        genesis,
        ScrollChainConfig::dogeos_chikyu(),
        Some(DOGEOS_CHIKYU_GENESIS_HASH),
        None,
    )
    .into()
});
