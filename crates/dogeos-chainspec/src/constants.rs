use crate::L1Config;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::{Address, B256, address, b256};

/// Inherited external fee-vault contract address.
pub const SCROLL_FEE_VAULT_ADDRESS: Address = address!("5300000000000000000000000000000000000005");

/// Maximum transaction payload size produced by a DogeOS block.
pub const MAX_TX_PAYLOAD_BYTES_PER_BLOCK: usize = 120 * 1024;

/// Feynman EIP-1559 parameters.
pub const DOGEOS_BASE_FEE_PARAMS_FEYNMAN: BaseFeeParams = BaseFeeParams::new(8, 2);

/// DogeOS development-network L1 configuration.
pub const DOGEOS_DEV_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Goerli as u64,
    l1_message_queue_address: Address::ZERO,
    l1_message_queue_v2_address: Address::ZERO,
    scroll_chain_address: Address::ZERO,
    l2_system_config_address: Address::ZERO,
    num_l1_messages_per_block: 10,
};

/// DogeOS mainnet L1 configuration retained from the current node.
pub const DOGEOS_MAINNET_L1_CONFIG: L1Config = DOGEOS_DEV_L1_CONFIG;

/// DogeOS mainnet genesis hash retained from the current node configuration.
///
/// This remains unresolved in the oracle repository and must be replaced with a
/// published hash before mainnet qualification.
pub const DOGEOS_MAINNET_GENESIS_HASH: B256 = B256::ZERO;

pub const DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_ADDRESS: Address =
    address!("3396BeD5adB7d83CC6C424264d229478556d3C02");
pub const DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_V2_ADDRESS: Address =
    address!("6B72155A3aC485Ea8C4CCacF243F6e634D4869bD");
pub const DOGEOS_CHIKYU_L1_PROXY_ADDRESS: Address =
    address!("8cB645a973e0C595aaAb55361fe917915b4E656c");
pub const DOGEOS_CHIKYU_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS: Address =
    address!("2669B071E88e272CBDA1e12182D8C754CB737400");

/// DogeOS Chikyu L1 configuration.
pub const DOGEOS_CHIKYU_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Goerli as u64,
    l1_message_queue_address: DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_ADDRESS,
    l1_message_queue_v2_address: DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_V2_ADDRESS,
    scroll_chain_address: DOGEOS_CHIKYU_L1_PROXY_ADDRESS,
    l2_system_config_address: DOGEOS_CHIKYU_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS,
    num_l1_messages_per_block: 10,
};

/// Published DogeOS Chikyu genesis hash.
pub const DOGEOS_CHIKYU_GENESIS_HASH: B256 =
    b256!("931467859726d2ca9b4401919bb54e3fffb41e24a0a3ec9ba9141e2d38a6357e");
