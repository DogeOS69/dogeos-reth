//! DogeOS genesis extension fields.

use crate::{
    DOGEOS_CHIKYU_L1_CONFIG, DOGEOS_DEV_L1_CONFIG, DOGEOS_MAINNET_L1_CONFIG,
    MAX_TX_PAYLOAD_BYTES_PER_BLOCK, SCROLL_FEE_VAULT_ADDRESS,
};
use alloy_primitives::{Address, B256};
use alloy_serde::OtherFields;
use serde::{Deserialize, Deserializer, de::Error};

use dogeos_hardforks::{DogeosHardfork, ForkCondition};

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
    /// Default maximum number of L1 messages per L2 block for the rollup-node sequencer.
    /// Overridden by `--sequencer.max-l1-messages`; this is local sequencer policy, not a
    /// consensus or proof-enforced per-block count limit.
    /// The JSON field remains `numL1MessagesPerBlock` for configuration compatibility.
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

fn deserialize_optional_u64_flexible<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    value
        .map(|value| match value {
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
        })
        .transpose()
}

/// Retired genesis key for the fee parameters; [`DogeosHardfork::TsukiV2`] replaces it.
const RETIRED_TSUKI_FEE_PARAMS_TIME: &str = "tsukiFeeParamsTime";

/// DogeOS hardfork timestamps encoded in a genesis config.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DogeosHardforkInfo {
    #[serde(default, deserialize_with = "deserialize_optional_u64_flexible")]
    pub feynman_time: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_flexible")]
    pub galileo_time: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_flexible")]
    pub galileo_v2_time: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_flexible")]
    pub tsuki_time: Option<u64>,
    /// Required whenever `tsuki_time` is set, and no earlier than it.
    #[serde(default, deserialize_with = "deserialize_optional_u64_flexible")]
    pub tsuki_v2_time: Option<u64>,
}

impl DogeosHardforkInfo {
    /// Converts execution-fork timestamps into an explicit, fail-closed activation schedule.
    pub const fn activation_schedule(self) -> [(DogeosHardfork, ForkCondition); 5] {
        const fn condition(timestamp: Option<u64>) -> ForkCondition {
            match timestamp {
                Some(timestamp) => ForkCondition::Timestamp(timestamp),
                None => ForkCondition::Never,
            }
        }

        [
            (DogeosHardfork::Feynman, condition(self.feynman_time)),
            (DogeosHardfork::Galileo, condition(self.galileo_time)),
            (DogeosHardfork::GalileoV2, condition(self.galileo_v2_time)),
            (DogeosHardfork::Tsuki, condition(self.tsuki_time)),
            (DogeosHardfork::TsukiV2, condition(self.tsuki_v2_time)),
        ]
    }

    /// Rejects a TsukiV2 timestamp that is missing, early, or set without Tsuki.
    fn validate(self) -> Result<Self, serde_json::Error> {
        match (self.tsuki_time, self.tsuki_v2_time) {
            (Some(_), None) => Err(serde_json::Error::custom(
                "`tsukiV2Time` is required when `tsukiTime` is set",
            )),
            (None, Some(_)) => Err(serde_json::Error::custom(
                "`tsukiV2Time` requires `tsukiTime`",
            )),
            (Some(tsuki), Some(tsuki_v2)) if tsuki_v2 < tsuki => Err(serde_json::Error::custom(
                "`tsukiV2Time` must not be earlier than `tsukiTime`",
            )),
            _ => Ok(self),
        }
    }
}

impl TryFrom<&OtherFields> for DogeosHardforkInfo {
    type Error = serde_json::Error;

    fn try_from(others: &OtherFields) -> Result<Self, Self::Error> {
        if others.contains_key(RETIRED_TSUKI_FEE_PARAMS_TIME) {
            return Err(serde_json::Error::custom(
                "`tsukiFeeParamsTime` is retired; the fee parameters switch at `tsukiV2Time`",
            ));
        }
        others.deserialize_as::<Self>()?.validate()
    }
}

/// DogeOS sequencer configuration encoded under the inherited `scroll` genesis key.
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", from = "ScrollChainConfigJson")]
pub struct ScrollChainConfig {
    pub fee_vault_address: Option<Address>,
    pub max_tx_payload_bytes_per_block: usize,
    pub l1_config: L1Config,
}

/// Strict input schema including recognized metadata from published l2geth genesis documents.
///
/// The four legacy metadata fields do not configure Reth execution and are not emitted when
/// serializing the runtime configuration. All other unrecognized fields are rejected.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScrollChainConfigJson {
    fee_vault_address: Option<Address>,
    max_tx_payload_bytes_per_block: usize,
    l1_config: L1Config,
    #[serde(rename = "useZktrie")]
    _use_zktrie: Option<bool>,
    #[serde(rename = "maxTxPerBlock")]
    _max_tx_per_block: Option<u64>,
    #[serde(rename = "genesisStateRoot")]
    _genesis_state_root: Option<B256>,
    #[serde(rename = "missingHeaderFieldsSHA256")]
    _missing_header_fields_sha256: Option<B256>,
}

impl From<ScrollChainConfigJson> for ScrollChainConfig {
    fn from(config: ScrollChainConfigJson) -> Self {
        Self {
            fee_vault_address: config.fee_vault_address,
            max_tx_payload_bytes_per_block: config.max_tx_payload_bytes_per_block,
            l1_config: config.l1_config,
        }
    }
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
            Some(config) => config,
            None => Err(serde_json::Error::missing_field("scroll")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsuki_v2_time_accepts_flexible_unsigned_timestamps() {
        for (json, expected) in [
            (r#"{"tsukiTime":0,"tsukiV2Time":0}"#, 0),
            (r#"{"tsukiTime":100,"tsukiV2Time":100}"#, 100),
            (r#"{"tsukiTime":100,"tsukiV2Time":"200"}"#, 200),
            (r#"{"tsukiTime":100,"tsukiV2Time":"0xc8"}"#, 200),
            (
                r#"{"tsukiTime":100,"tsukiV2Time":"18446744073709551615"}"#,
                u64::MAX,
            ),
        ] {
            let fields: OtherFields = serde_json::from_str(json).unwrap();
            assert_eq!(
                DogeosHardforkInfo::try_from(&fields).unwrap().tsuki_v2_time,
                Some(expected),
                "{json}"
            );
        }
        let fields: OtherFields = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(
            DogeosHardforkInfo::try_from(&fields).unwrap().tsuki_v2_time,
            None
        );
    }

    #[test]
    fn tsuki_v2_time_is_required_with_tsuki_and_never_precedes_it() {
        for json in [
            r#"{"tsukiTime":100}"#,
            r#"{"tsukiTime":100,"tsukiV2Time":null}"#,
            r#"{"tsukiTime":100,"tsukiV2Time":99}"#,
            r#"{"tsukiTime":"0x64","tsukiV2Time":"0x63"}"#,
            r#"{"tsukiV2Time":100}"#,
        ] {
            let fields: OtherFields = serde_json::from_str(json).unwrap();
            assert!(DogeosHardforkInfo::try_from(&fields).is_err(), "{json}");
        }
    }

    #[test]
    fn invalid_tsuki_v2_time_is_rejected() {
        for value in [
            "-1",
            "1.5",
            "true",
            r#""""#,
            r#""18446744073709551616""#,
            "[]",
            "{}",
        ] {
            let json = format!(r#"{{"tsukiTime":0,"tsukiV2Time":{value}}}"#);
            let fields: OtherFields = serde_json::from_str(&json).unwrap();
            assert!(DogeosHardforkInfo::try_from(&fields).is_err(), "{json}");
        }
    }

    #[test]
    fn retired_fee_params_time_is_rejected_in_any_form() {
        for value in ["null", "0", "100", r#""0x64""#] {
            let json = format!(r#"{{"tsukiTime":0,"tsukiV2Time":0,"tsukiFeeParamsTime":{value}}}"#);
            let fields: OtherFields = serde_json::from_str(&json).unwrap();
            let error = DogeosHardforkInfo::try_from(&fields).unwrap_err();
            assert!(error.to_string().contains("tsukiFeeParamsTime"), "{json}");
        }
    }

    #[test]
    fn scroll_config_round_trips_without_retired_fields() {
        let config = ScrollChainConfig::dev();
        let value = serde_json::to_value(config).unwrap();
        assert!(value.get("l1DataFeeBufferCheck").is_none());
        let fields: OtherFields =
            serde_json::from_value(serde_json::json!({ "scroll": value })).unwrap();
        assert_eq!(ScrollChainConfig::try_from(&fields).unwrap(), config);
    }

    #[test]
    fn legacy_genesis_metadata_does_not_change_runtime_config_or_serialization() {
        let config = ScrollChainConfig::dev();
        let serialized = serde_json::to_value(config).unwrap();
        let mut with_metadata = serialized.clone();
        with_metadata["useZktrie"] = serde_json::json!(false);
        with_metadata["maxTxPerBlock"] = serde_json::json!(100);
        with_metadata["genesisStateRoot"] = serde_json::json!(B256::repeat_byte(0x11));
        with_metadata["missingHeaderFieldsSHA256"] = serde_json::json!(B256::repeat_byte(0x22));

        let parsed: ScrollChainConfig = serde_json::from_value(with_metadata).unwrap();
        assert_eq!(parsed, config);
        assert_eq!(serde_json::to_value(parsed).unwrap(), serialized);
    }

    #[test]
    fn scroll_config_rejects_unknown_fields() {
        for (key, value) in [
            ("l1DataFeeBufferCheck", serde_json::json!(false)),
            ("l1DataFeeBufferCheck", serde_json::json!(true)),
            ("unknownPolicy", serde_json::json!(true)),
        ] {
            let mut config = serde_json::to_value(ScrollChainConfig::dev()).unwrap();
            config[key] = value;
            let fields: OtherFields =
                serde_json::from_value(serde_json::json!({ "scroll": config })).unwrap();
            let error = ScrollChainConfig::try_from(&fields).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{key}`"))
            );
        }
    }

    #[test]
    fn extracts_inherited_scroll_json_key() {
        let genesis: alloy_genesis::Genesis =
            serde_json::from_str(include_str!("../res/genesis/chikyu_dogeos.json")).unwrap();
        let config = ScrollChainConfig::extract_from(&genesis.config.extra_fields).unwrap();
        assert_eq!(config.l1_config.l1_chain_id, 111111);
        assert_eq!(config.max_tx_payload_bytes_per_block, 120 * 1024);
    }

    #[test]
    fn extracts_flexible_hardfork_timestamps_and_fails_closed_when_absent() {
        let fields: OtherFields = serde_json::from_str(
            r#"{
                "feynmanTime": 10,
                "galileoTime": "20",
                "galileoV2Time": "0x1e"
            }"#,
        )
        .unwrap();
        let info = DogeosHardforkInfo::try_from(&fields).unwrap();

        assert_eq!(
            info.activation_schedule(),
            [
                (DogeosHardfork::Feynman, ForkCondition::Timestamp(10)),
                (DogeosHardfork::Galileo, ForkCondition::Timestamp(20)),
                (DogeosHardfork::GalileoV2, ForkCondition::Timestamp(30)),
                (DogeosHardfork::Tsuki, ForkCondition::Never),
                (DogeosHardfork::TsukiV2, ForkCondition::Never),
            ]
        );
    }
}
