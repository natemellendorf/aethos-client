use std::collections::BTreeMap;

use ciborium::value::Value;

use crate::aethos_core::protocol::{decode_cbor_value_exact, encode_cbor_value_deterministic};

const WAYFARER_CHAT_V1: &str = "wayfarer.chat.v1";
const WAYFARER_MEDIA_MANIFEST_V1: &str = "wayfarer.media_manifest.v1";
const RESERVED_TYPES: [&str; 5] = [
    "wayfarer.profile.v1",
    "wayfarer.reaction.v1",
    "wayfarer.message_update.v1",
    "wayfarer.status_event.v1",
    "wayfarer.notice.v1",
];
const MEDIA_KINDS: [&str; 4] = ["image", "video", "audio", "file"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatDisplayPayload {
    pub text: String,
    pub created_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreNoDisplayKind {
    WayfarerMediaManifestV1,
    ReservedType(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassificationOutcome {
    AcceptDisplay { chat: ChatDisplayPayload },
    AcceptStoreNoDisplay { kind: StoreNoDisplayKind },
    Reject { reason: String },
    UnsupportedSafeSkip { payload_type: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationResult {
    pub outcome: ClassificationOutcome,
    pub payload_type: Option<String>,
    pub routed_to: Option<String>,
    pub decoder_ran: bool,
}

impl ClassificationResult {
    fn reject(reason: impl Into<String>) -> Self {
        Self {
            outcome: ClassificationOutcome::Reject {
                reason: reason.into(),
            },
            payload_type: None,
            routed_to: None,
            decoder_ran: false,
        }
    }

    pub fn outcome_label(&self) -> &'static str {
        match self.outcome {
            ClassificationOutcome::AcceptDisplay { .. } => "accept/display",
            ClassificationOutcome::AcceptStoreNoDisplay { .. } => "accept/store-no-display",
            ClassificationOutcome::Reject { .. } => "reject",
            ClassificationOutcome::UnsupportedSafeSkip { .. } => "unsupported-safe-skip",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutboundMediaAsset {
    pub asset_ref: String,
    pub mime_type: String,
    pub byte_length: u64,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OutboundMediaManifestInput {
    pub transfer_ref: String,
    pub media_kind: String,
    pub assets: Vec<OutboundMediaAsset>,
    pub caption: Option<String>,
    pub created_at_unix_ms: u64,
}

pub fn build_wayfarer_chat_body(text: &str, created_at_unix_ms: u64) -> Result<Vec<u8>, String> {
    let mut map = BTreeMap::new();
    map.insert(
        "created_at_unix_ms".to_string(),
        Value::Integer(created_at_unix_ms.into()),
    );
    map.insert("text".to_string(), Value::Text(text.to_string()));
    map.insert(
        "type".to_string(),
        Value::Text(WAYFARER_CHAT_V1.to_string()),
    );
    let cbor = encode_cbor_value_deterministic(&Value::Map(
        map.into_iter()
            .map(|(key, value)| (Value::Text(key), value))
            .collect(),
    ))?;
    let classified = classify_wayfarer_app_body(&cbor);
    if !matches!(
        classified.outcome,
        ClassificationOutcome::AcceptDisplay { .. }
    ) {
        return Err("constructed wayfarer.chat.v1 body failed strict classification".to_string());
    }
    Ok(cbor)
}

pub fn build_wayfarer_media_manifest_body(
    input: &OutboundMediaManifestInput,
) -> Result<Vec<u8>, String> {
    let mut top = BTreeMap::new();
    top.insert(
        "type".to_string(),
        Value::Text(WAYFARER_MEDIA_MANIFEST_V1.to_string()),
    );
    top.insert(
        "transfer_ref".to_string(),
        Value::Text(input.transfer_ref.clone()),
    );
    top.insert(
        "media_kind".to_string(),
        Value::Text(input.media_kind.clone()),
    );

    let assets = input
        .assets
        .iter()
        .map(|asset| {
            let mut map = BTreeMap::new();
            map.insert(
                "asset_ref".to_string(),
                Value::Text(asset.asset_ref.clone()),
            );
            map.insert(
                "mime_type".to_string(),
                Value::Text(asset.mime_type.clone()),
            );
            map.insert(
                "byte_length".to_string(),
                Value::Integer(asset.byte_length.into()),
            );
            if let Some(name) = asset.name.as_ref() {
                map.insert("name".to_string(), Value::Text(name.clone()));
            }
            Value::Map(
                map.into_iter()
                    .map(|(key, value)| (Value::Text(key), value))
                    .collect(),
            )
        })
        .collect::<Vec<_>>();

    top.insert("assets".to_string(), Value::Array(assets));
    if let Some(caption) = input.caption.as_ref() {
        top.insert("caption".to_string(), Value::Text(caption.clone()));
    }
    top.insert(
        "created_at_unix_ms".to_string(),
        Value::Integer(input.created_at_unix_ms.into()),
    );

    let cbor = encode_cbor_value_deterministic(&Value::Map(
        top.into_iter()
            .map(|(key, value)| (Value::Text(key), value))
            .collect(),
    ))?;
    let classified = classify_wayfarer_app_body(&cbor);
    if !matches!(
        classified.outcome,
        ClassificationOutcome::AcceptStoreNoDisplay {
            kind: StoreNoDisplayKind::WayfarerMediaManifestV1
        }
    ) {
        return Err(
            "constructed wayfarer.media_manifest.v1 body failed strict classification".to_string(),
        );
    }
    Ok(cbor)
}

pub fn classify_wayfarer_app_body(body: &[u8]) -> ClassificationResult {
    let decoded = match decode_cbor_value_exact(body, "wayfarer app body") {
        Ok(value) => value,
        Err(err) => return ClassificationResult::reject(format!("invalid_cbor: {err}")),
    };

    let canonical = match encode_cbor_value_deterministic(&decoded) {
        Ok(value) => value,
        Err(err) => {
            return ClassificationResult::reject(format!("deterministic_reencode_failed: {err}"));
        }
    };
    if canonical.as_slice() != body {
        return ClassificationResult::reject("non_deterministic_cbor_encoding");
    }

    let top_level_map = match value_as_text_keyed_map(decoded, "top_level") {
        Ok(value) => value,
        Err(err) => return ClassificationResult::reject(err),
    };

    let payload_type = match top_level_map.get("type") {
        Some(Value::Text(value)) => value.clone(),
        Some(_) => return ClassificationResult::reject("type_must_be_text"),
        None => return ClassificationResult::reject("missing_type"),
    };

    match payload_type.as_str() {
        WAYFARER_CHAT_V1 => match decode_wayfarer_chat_v1(&top_level_map) {
            Ok(chat) => ClassificationResult {
                outcome: ClassificationOutcome::AcceptDisplay { chat },
                payload_type: Some(payload_type.clone()),
                routed_to: Some(payload_type),
                decoder_ran: true,
            },
            Err(err) => ClassificationResult {
                outcome: ClassificationOutcome::Reject {
                    reason: format!("malformed_wayfarer_chat_v1: {err}"),
                },
                payload_type: Some(WAYFARER_CHAT_V1.to_string()),
                routed_to: Some(WAYFARER_CHAT_V1.to_string()),
                decoder_ran: true,
            },
        },
        WAYFARER_MEDIA_MANIFEST_V1 => match decode_wayfarer_media_manifest_v1(&top_level_map) {
            Ok(()) => ClassificationResult {
                outcome: ClassificationOutcome::AcceptStoreNoDisplay {
                    kind: StoreNoDisplayKind::WayfarerMediaManifestV1,
                },
                payload_type: Some(payload_type.clone()),
                routed_to: Some(payload_type),
                decoder_ran: true,
            },
            Err(err) => ClassificationResult {
                outcome: ClassificationOutcome::Reject {
                    reason: format!("malformed_wayfarer_media_manifest_v1: {err}"),
                },
                payload_type: Some(WAYFARER_MEDIA_MANIFEST_V1.to_string()),
                routed_to: Some(WAYFARER_MEDIA_MANIFEST_V1.to_string()),
                decoder_ran: true,
            },
        },
        other if RESERVED_TYPES.contains(&other) => ClassificationResult {
            outcome: ClassificationOutcome::AcceptStoreNoDisplay {
                kind: StoreNoDisplayKind::ReservedType(other.to_string()),
            },
            payload_type: Some(other.to_string()),
            routed_to: None,
            decoder_ran: false,
        },
        other => ClassificationResult {
            outcome: ClassificationOutcome::UnsupportedSafeSkip {
                payload_type: other.to_string(),
            },
            payload_type: Some(other.to_string()),
            routed_to: None,
            decoder_ran: false,
        },
    }
}

fn decode_wayfarer_chat_v1(map: &BTreeMap<String, Value>) -> Result<ChatDisplayPayload, String> {
    let msg_type = map
        .get("type")
        .and_then(|value| match value {
            Value::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .ok_or_else(|| "type_must_be_text".to_string())?;
    if msg_type != WAYFARER_CHAT_V1 {
        return Err("type_mismatch".to_string());
    }

    let text = required_non_empty_text(map, "text")?;
    let created_at_unix_ms = required_u64_integer(map, "created_at_unix_ms")?;

    Ok(ChatDisplayPayload {
        text,
        created_at_unix_ms,
    })
}

fn decode_wayfarer_media_manifest_v1(map: &BTreeMap<String, Value>) -> Result<(), String> {
    let msg_type = map
        .get("type")
        .and_then(|value| match value {
            Value::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .ok_or_else(|| "type_must_be_text".to_string())?;
    if msg_type != WAYFARER_MEDIA_MANIFEST_V1 {
        return Err("type_mismatch".to_string());
    }

    let transfer_ref = map
        .get("transfer_ref")
        .ok_or_else(|| "transfer_ref_required".to_string())?;
    let Value::Text(transfer_ref) = transfer_ref else {
        return Err("transfer_ref_must_be_text".to_string());
    };
    if transfer_ref.is_empty() {
        return Err("transfer_ref_required".to_string());
    }

    let media_kind = required_non_empty_text(map, "media_kind")?;
    if !MEDIA_KINDS.contains(&media_kind.as_str()) {
        return Err("media_kind_invalid".to_string());
    }

    let assets = map
        .get("assets")
        .ok_or_else(|| "assets_required".to_string())?;
    let Value::Array(assets) = assets else {
        return Err("assets_must_be_array".to_string());
    };
    if assets.is_empty() {
        return Err("assets_must_be_non_empty".to_string());
    }

    for (index, asset) in assets.iter().enumerate() {
        let asset_map = value_as_text_keyed_map(asset.clone(), &format!("assets[{index}]"))?;
        let _asset_ref = required_non_empty_text(&asset_map, "asset_ref")?;
        let _mime_type = required_non_empty_text(&asset_map, "mime_type")?;
        let _byte_length = required_u64_integer(&asset_map, "byte_length")?;
        if let Some(name) = asset_map.get("name") {
            if !matches!(name, Value::Text(_)) {
                return Err(format!("assets[{index}].name_must_be_text"));
            }
        }
    }

    if let Some(caption) = map.get("caption") {
        if !matches!(caption, Value::Text(_)) {
            return Err("caption_must_be_text".to_string());
        }
    }

    let _created_at_unix_ms = required_u64_integer(map, "created_at_unix_ms")?;
    Ok(())
}

fn value_as_text_keyed_map(value: Value, context: &str) -> Result<BTreeMap<String, Value>, String> {
    let Value::Map(entries) = value else {
        if context == "top_level" {
            return Err("top_level_must_be_map".to_string());
        }
        return Err(format!("{context}_must_be_map"));
    };

    let mut out = BTreeMap::new();
    for (key, value) in entries {
        let Value::Text(key) = key else {
            if context == "top_level" {
                return Err("top_level_keys_must_be_text".to_string());
            }
            return Err(format!("{context}_keys_must_be_text"));
        };
        if out.insert(key, value).is_some() {
            return Err(format!("{context}_duplicate_keys"));
        }
    }

    Ok(out)
}

fn required_non_empty_text(map: &BTreeMap<String, Value>, key: &str) -> Result<String, String> {
    let value = map.get(key).ok_or_else(|| format!("{key}_required"))?;
    let Value::Text(text) = value else {
        return Err(format!("{key}_must_be_text"));
    };
    if text.is_empty() {
        return Err(format!("{key}_must_be_non_empty"));
    }
    Ok(text.clone())
}

fn required_u64_integer(map: &BTreeMap<String, Value>, key: &str) -> Result<u64, String> {
    let value = map.get(key).ok_or_else(|| format!("{key}_required"))?;
    let Value::Integer(integer) = value else {
        return Err(format!("{key}_must_be_integer"));
    };
    let signed = i128::try_from(integer.clone()).map_err(|_| format!("{key}_invalid_integer"))?;
    if signed < 0 {
        return Err(format!("{key}_must_be_non_negative"));
    }
    u64::try_from(signed).map_err(|_| format!("{key}_invalid_integer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct FixtureManifest {
        fixtures: Vec<FixtureManifestEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct FixtureManifestEntry {
        path: String,
        expected_outcome: String,
    }

    #[derive(Debug, Deserialize)]
    struct Fixture {
        id: String,
        body_cbor_hex: String,
        expected_decoded_map: Option<serde_json::Value>,
        expected_outcome: String,
    }

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/aethos/Fixtures/App/wayfarer-payload-taxonomy")
    }

    fn decode_hex(input: &str) -> Result<Vec<u8>, String> {
        let trimmed = input.trim();
        if trimmed.len() % 2 != 0 {
            return Err("hex_input_has_odd_length".to_string());
        }
        let mut out = Vec::with_capacity(trimmed.len() / 2);
        for idx in (0..trimmed.len()).step_by(2) {
            let byte = u8::from_str_radix(&trimmed[idx..idx + 2], 16)
                .map_err(|err| format!("invalid_hex_at_{idx}: {err}"))?;
            out.push(byte);
        }
        Ok(out)
    }

    fn assert_expected_subset(expected: &serde_json::Value, actual: &Value) {
        match expected {
            serde_json::Value::Object(expected_map) => {
                let actual_map = value_as_text_keyed_map(actual.clone(), "expected_subset_actual")
                    .expect("actual value must be a text-keyed map");
                for (key, expected_value) in expected_map {
                    let actual_value = actual_map
                        .get(key)
                        .unwrap_or_else(|| panic!("expected key `{key}` to exist in decoded map"));
                    assert_expected_subset(expected_value, actual_value);
                }
            }
            serde_json::Value::Array(expected_items) => {
                let Value::Array(actual_items) = actual else {
                    panic!("expected array, found non-array value")
                };
                assert_eq!(
                    expected_items.len(),
                    actual_items.len(),
                    "expected arrays to match exactly"
                );
                for (expected_item, actual_item) in expected_items.iter().zip(actual_items.iter()) {
                    assert_expected_subset(expected_item, actual_item);
                }
            }
            serde_json::Value::String(expected_text) => {
                let Value::Text(actual_text) = actual else {
                    panic!("expected text value `{expected_text}`")
                };
                assert_eq!(expected_text, actual_text);
            }
            serde_json::Value::Number(expected_number) => {
                let Value::Integer(actual_integer) = actual else {
                    panic!("expected integer value `{expected_number}`")
                };
                let actual_signed =
                    i128::try_from(actual_integer.clone()).expect("actual integer must fit i128");
                if let Some(expected_signed) = expected_number.as_i64() {
                    assert_eq!(actual_signed, expected_signed as i128);
                } else if let Some(expected_unsigned) = expected_number.as_u64() {
                    assert_eq!(actual_signed, expected_unsigned as i128);
                } else {
                    panic!("expected number `{expected_number}` must be integer");
                }
            }
            serde_json::Value::Bool(expected_bool) => {
                let Value::Bool(actual_bool) = actual else {
                    panic!("expected bool value `{expected_bool}`")
                };
                assert_eq!(expected_bool, actual_bool);
            }
            serde_json::Value::Null => {
                assert!(matches!(actual, Value::Null));
            }
        }
    }

    fn expected_outcome_label(result: &ClassificationResult) -> &'static str {
        result.outcome_label()
    }

    #[test]
    fn taxonomy_fixtures_classify_with_expected_outcomes() {
        let fixtures_dir = fixtures_dir();
        assert!(
            fixtures_dir.exists(),
            "fixtures directory missing: {fixtures_dir:?}"
        );

        let manifest_raw = std::fs::read_to_string(fixtures_dir.join("manifest.json"))
            .expect("read taxonomy manifest");
        let manifest: FixtureManifest =
            serde_json::from_str(&manifest_raw).expect("parse taxonomy manifest");

        for entry in manifest.fixtures {
            let fixture_raw = std::fs::read_to_string(fixtures_dir.join(&entry.path))
                .unwrap_or_else(|err| panic!("read fixture {}: {err}", entry.path));
            let fixture: Fixture = serde_json::from_str(&fixture_raw)
                .unwrap_or_else(|err| panic!("parse fixture {}: {err}", entry.path));
            let body = decode_hex(&fixture.body_cbor_hex)
                .unwrap_or_else(|err| panic!("decode fixture hex {}: {err}", fixture.id));
            let result = classify_wayfarer_app_body(&body);

            assert_eq!(
                entry.expected_outcome, fixture.expected_outcome,
                "manifest and fixture expected outcomes must match for {}",
                fixture.id
            );
            assert_eq!(
                entry.expected_outcome,
                expected_outcome_label(&result),
                "unexpected classification outcome for {}",
                fixture.id
            );

            if let Some(expected_decoded_map) = fixture.expected_decoded_map {
                let decoded = decode_cbor_value_exact(&body, "fixture body")
                    .unwrap_or_else(|err| panic!("decode fixture cbor {}: {err}", fixture.id));
                assert_expected_subset(&expected_decoded_map, &decoded);
            }
        }
    }

    #[test]
    fn unsupported_types_safe_skip_without_running_typed_decoders() {
        let unknown_future_fixture = fixtures_dir().join("unknown_future_payload_type_v1.json");
        let fixture_raw = std::fs::read_to_string(&unknown_future_fixture).expect("read fixture");
        let fixture: Fixture = serde_json::from_str(&fixture_raw).expect("parse fixture");
        let unknown_future_body = decode_hex(&fixture.body_cbor_hex).expect("decode fixture hex");

        let unknown_future = classify_wayfarer_app_body(&unknown_future_body);
        assert!(matches!(
            unknown_future.outcome,
            ClassificationOutcome::UnsupportedSafeSkip { .. }
        ));
        assert!(!unknown_future.decoder_ran);
        assert!(unknown_future.routed_to.is_none());

        let known_unsupported = encode_cbor_value_deterministic(&Value::Map(vec![
            (
                Value::Text("type".to_string()),
                Value::Text("wayfarer.chat.v2".to_string()),
            ),
            (
                Value::Text("text".to_string()),
                Value::Text("future chat payload".to_string()),
            ),
            (
                Value::Text("created_at_unix_ms".to_string()),
                Value::Integer(1u64.into()),
            ),
        ]))
        .expect("build deterministic cbor");
        let known_unsupported_result = classify_wayfarer_app_body(&known_unsupported);
        assert!(matches!(
            known_unsupported_result.outcome,
            ClassificationOutcome::UnsupportedSafeSkip { .. }
        ));
        assert!(!known_unsupported_result.decoder_ran);
        assert!(known_unsupported_result.routed_to.is_none());
    }

    #[test]
    fn decode_failure_rejects_without_running_decoders() {
        let fixture_raw = std::fs::read_to_string(fixtures_dir().join("binary_non_utf8_body.json"))
            .expect("read decode failure fixture");
        let fixture: Fixture =
            serde_json::from_str(&fixture_raw).expect("parse decode failure fixture");
        let body = decode_hex(&fixture.body_cbor_hex).expect("decode hex");

        let result = classify_wayfarer_app_body(&body);
        assert!(matches!(
            result.outcome,
            ClassificationOutcome::Reject { .. }
        ));
        assert!(!result.decoder_ran);
        assert!(result.routed_to.is_none());
    }

    #[test]
    fn reserved_types_accept_store_no_display() {
        let fixture_names = [
            "reserved_wayfarer_profile_v1.json",
            "reserved_wayfarer_reaction_v1.json",
            "reserved_wayfarer_message_update_v1.json",
            "reserved_wayfarer_status_event_v1.json",
            "reserved_wayfarer_notice_v1.json",
        ];

        for fixture_name in fixture_names {
            let fixture_raw = std::fs::read_to_string(fixtures_dir().join(fixture_name))
                .unwrap_or_else(|err| panic!("read {fixture_name}: {err}"));
            let fixture: Fixture = serde_json::from_str(&fixture_raw)
                .unwrap_or_else(|err| panic!("parse {fixture_name}: {err}"));
            let body = decode_hex(&fixture.body_cbor_hex)
                .unwrap_or_else(|err| panic!("decode {fixture_name}: {err}"));

            let result = classify_wayfarer_app_body(&body);
            assert!(matches!(
                result.outcome,
                ClassificationOutcome::AcceptStoreNoDisplay { .. }
            ));
            assert!(!result.decoder_ran);
        }
    }

    #[test]
    fn outbound_chat_body_is_deterministic_and_routes_to_chat() {
        let body = build_wayfarer_chat_body("hello wayfarer", 1_735_689_600_000)
            .expect("build outbound chat body");
        let decoded =
            decode_cbor_value_exact(&body, "outbound chat body").expect("decode outbound cbor");
        let reencoded =
            encode_cbor_value_deterministic(&decoded).expect("re-encode outbound chat cbor");
        assert_eq!(body, reencoded, "outbound chat cbor must be deterministic");

        let decoded_map = value_as_text_keyed_map(decoded, "outbound_chat")
            .expect("outbound cbor should decode to map");
        assert!(matches!(
            decoded_map.get("type"),
            Some(Value::Text(value)) if value == WAYFARER_CHAT_V1
        ));

        let classification = classify_wayfarer_app_body(&body);
        assert!(matches!(
            classification.outcome,
            ClassificationOutcome::AcceptDisplay { .. }
        ));
    }
}
