use crate::{DogeosChainSpec, LazyLock};
use alloc::sync::Arc;

/// DogeOS Chikyu testnet specification derived from the network's genesis document.
pub static DOGEOS_CHIKYU: LazyLock<Arc<DogeosChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json"))
        .expect("valid DogeOS Chikyu genesis");
    DogeosChainSpec::from_custom_genesis(genesis).into()
});
