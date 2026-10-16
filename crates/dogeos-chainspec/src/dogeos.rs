use crate::{DogeosChainSpec, LazyLock};
use alloc::sync::Arc;

/// DogeOS mainnet specification derived from the frozen genesis document.
pub static DOGEOS_MAINNET: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/dogeos.json"))
        .expect("valid DogeOS mainnet genesis");
    DogeosChainSpec::from_custom_genesis(genesis).into()
});
