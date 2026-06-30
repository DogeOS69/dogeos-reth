//! Chain specification for the Dogeos Chikyū Testnet network.

use crate::{
    constants::SCROLL_BASE_FEE_PARAMS_FEYNMAN, make_genesis_header, LazyLock, ScrollChainConfig,
    ScrollChainSpec, DOGEOS_CHIKYU_GENESIS_HASH,
};
use alloc::{sync::Arc, vec};

use alloy_chains::Chain;
use reth_chainspec::{BaseFeeParamsKind, ChainSpec, Hardfork};
use reth_primitives_traits::SealedHeader;
use reth_scroll_forks::DOGEOS_CHIKYU_HARDFORKS;
use scroll_alloy_hardforks::ScrollHardfork;

/// The Dogeos Mainnet spec
pub static DOGEOS_CHIKYU: LazyLock<Arc<ScrollChainSpec>> = LazyLock::new(|| {
    let genesis = serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json"))
        .expect("Can't deserialize Dogeos Mainnet genesis json");
    ScrollChainSpec {
        inner: ChainSpec {
            chain: Chain::from_id(0x5fdaf3),
            genesis_header: SealedHeader::new(
                make_genesis_header(&genesis),
                DOGEOS_CHIKYU_GENESIS_HASH,
            ),
            genesis,
            hardforks: DOGEOS_CHIKYU_HARDFORKS.clone(),
            base_fee_params: BaseFeeParamsKind::Variable(
                vec![(ScrollHardfork::Feynman.boxed(), SCROLL_BASE_FEE_PARAMS_FEYNMAN)].into(),
            ),
            ..Default::default()
        },
        config: ScrollChainConfig::dogeos_chikyu(),
    }
    .into()
});
