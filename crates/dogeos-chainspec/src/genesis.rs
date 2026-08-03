//! DogeOS genesis extension fields.

use crate::{
    DOGEOS_CHIKYU_L1_CONFIG, DOGEOS_DEV_L1_CONFIG, DOGEOS_MAINNET_L1_CONFIG,
    MAX_TX_PAYLOAD_BYTES_PER_BLOCK, SCROLL_FEE_VAULT_ADDRESS,
};
use alloy_primitives::Address;
use alloy_serde::OtherFields;
use serde::{Deserialize, Deserializer, de::Error};

/// The inherited Scroll L1 configuration JSON contract.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1Config {
    #[serde(deserialize_with = "deserialize_u64_flexible")]
    pub l1_chain_id: u64,
    pub l1_message_queue_address: Address,
    pub l1_message_queue_v2_address: Address,
    pub scroll_chain_address: Address,
    pub l2_system_config_address: Address,
    #[serde(deserialize_with = "deserialize_u64_flexible")]
    pub num_l1_messages_per_block: u64,
}

fn deserialize_u64_flexible<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| D::Error::custom("expected an unsigned integer")),
        serde_json::Value::String(value) => {
            let (digits, radix) = value
                .strip_prefix("0x")
                .map_or((value.as_str(), 10), |digits| (digits, 16));
            u64::from_str_radix(digits, radix).map_err(D::Error::custom)
        }
        _ => Err(D::Error::custom("expected an unsigned integer or string")),
    }
}

/// DogeOS sequencer configuration encoded under the inherited `scroll` genesis key.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollChainConfig {
    pub fee_vault_address: Option<Address>,
    pub max_tx_payload_bytes_per_block: usize,
    pub l1_config: L1Config,
    #[serde(default)]
    pub l1_data_fee_buffer_check: bool,
}

impl ScrollChainConfig {
    pub const fn dev() -> Self {
        Self::new(DOGEOS_DEV_L1_CONFIG)
    }

    pub const fn dogeos_mainnet() -> Self {
        Self::new(DOGEOS_MAINNET_L1_CONFIG)
    }

    pub const fn dogeos_chikyu() -> Self {
        Self::new(DOGEOS_CHIKYU_L1_CONFIG)
    }

    const fn new(l1_config: L1Config) -> Self {
        Self {
            fee_vault_address: Some(SCROLL_FEE_VAULT_ADDRESS),
            max_tx_payload_bytes_per_block: MAX_TX_PAYLOAD_BYTES_PER_BLOCK,
            l1_config,
            l1_data_fee_buffer_check: false,
        }
    }

    pub fn extract_from(others: &OtherFields) -> Option<Self> {
        Self::try_from(others).ok()
    }
}

impl TryFrom<&OtherFields> for ScrollChainConfig {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        match others.get_deserialized::<Self>("scroll") {
            Some(Ok(config)) => Ok(config),
            _ => Err(serde_json::Error::missing_field("scroll")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_inherited_scroll_json_key() {
        let genesis: alloy_genesis::Genesis =
            serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
        let config = ScrollChainConfig::extract_from(&genesis.config.extra_fields).unwrap();
        assert_eq!(config.l1_config.l1_chain_id, 111111);
        assert_eq!(config.max_tx_payload_bytes_per_block, 120 * 1024);
    }
}
