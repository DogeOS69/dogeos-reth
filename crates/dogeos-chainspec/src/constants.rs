use crate::L1Config;
use alloy_eips::eip1559::BaseFeeParams;
use alloy_primitives::{Address, B256, address, b256};

/// Inherited external fee-vault contract address.
pub const SCROLL_FEE_VAULT_ADDRESS: Address = address!("5300000000000000000000000000000000000005");

/// Maximum transaction payload size produced by a DogeOS block.
pub const MAX_TX_PAYLOAD_BYTES_PER_BLOCK: usize = 120 * 1024;

/// Feynman EIP-1559 parameters.
pub const DOGEOS_BASE_FEE_PARAMS_FEYNMAN: BaseFeeParams = BaseFeeParams::new(8, 2);

/// Pre-Tsuki protocol-enforced maximum L2 base fee.
pub const LEGACY_MAX_L2_BASE_FEE: u64 = 10_000_000_000;

/// Default minimum Tsuki utilization-controlled base-fee component.
///
/// A SystemConfig override may raise or lower this within the hard protocol maximum.
pub const BASE_FEE_FLOOR: u64 = 10_000_000_000;

/// Default utilization-controlled base-fee component used by the first Tsuki block.
///
/// A SystemConfig override may select a different activation seed.
pub const INITIAL_CONTROLLED_BASE_FEE: u64 = 500_000_000_000;

/// Desired controlled-fee ceiling used to derive [`MAX_L2_BASE_FEE`].
pub const DESIRED_CONTROLLED_FEE_CEILING: u64 = 999_900_000_000;

/// Provisional L1-congestion overhead allowance used to derive [`MAX_L2_BASE_FEE`].
///
/// This is a calibration input, not a separately enforced runtime limit.
pub const BASE_FEE_OVERHEAD_BUDGET: u64 = 100_000_000;

/// Tsuki hard safety maximum for both the controlled component and final L2 base fee.
///
/// SystemConfig may select a lower runtime maximum without a protocol upgrade.
pub const MAX_L2_BASE_FEE: u64 = DESIRED_CONTROLLED_FEE_CEILING + BASE_FEE_OVERHEAD_BUDGET;

/// Default long-run gas target for the Tsuki utilization controller.
pub const DYNAMIC_BASE_FEE_GAS_TARGET: u64 = 10_000_000;

/// Default maximum-change denominator for the Tsuki utilization controller.
pub const DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;

const _: () = {
    assert!(MAX_L2_BASE_FEE == DESIRED_CONTROLLED_FEE_CEILING + BASE_FEE_OVERHEAD_BUDGET);
    assert!(BASE_FEE_FLOOR <= INITIAL_CONTROLLED_BASE_FEE);
    assert!(INITIAL_CONTROLLED_BASE_FEE <= MAX_L2_BASE_FEE);
    assert!(DYNAMIC_BASE_FEE_GAS_TARGET != 0);
    assert!(DYNAMIC_BASE_FEE_MAX_CHANGE_DENOMINATOR != 0);
};

/// DogeOS development-network L1 configuration.
pub const DOGEOS_DEV_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: alloy_chains::NamedChain::Goerli as u64,
    l1_message_queue_address: Address::ZERO,
    l1_message_queue_v2_address: Address::ZERO,
    scroll_chain_address: Address::ZERO,
    l2_system_config_address: Address::ZERO,
    num_l1_messages_per_block: 10,
};

/// DogeOS mainnet L1 configuration frozen in `res/genesis/dogeos.json`.
pub const DOGEOS_MAINNET_L1_CONFIG: L1Config = L1Config {
    l1_chain_id: 111_111,
    l1_message_queue_address: DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_ADDRESS,
    l1_message_queue_v2_address: DOGEOS_CHIKYU_L1_MESSAGE_QUEUE_V2_ADDRESS,
    scroll_chain_address: DOGEOS_CHIKYU_L1_PROXY_ADDRESS,
    l2_system_config_address: DOGEOS_CHIKYU_L2_SYSTEM_CONFIG_CONTRACT_ADDRESS,
    num_l1_messages_per_block: 10,
};

/// Canonical header hash computed from the frozen DogeOS mainnet genesis document.
pub const DOGEOS_MAINNET_GENESIS_HASH: B256 =
    b256!("f9f7c524dce38b51a4d28ec2f18680773e5ba9d3f5f430d0e05f92cfeb65b1bc");

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
