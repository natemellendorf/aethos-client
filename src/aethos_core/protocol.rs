use std::collections::BTreeMap;
use std::io::Cursor;

use base64::Engine;
use ciborium::value::Value;
use ciborium::{de::from_reader, ser::into_writer};
use serde::Serialize;
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha256};

pub fn is_valid_wayfarer_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn is_valid_payload_b64(value: &str) -> bool {
    !value.contains('=')
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .is_ok()
}

#[derive(Debug, Clone)]
pub struct EnvelopeV1 {
    pub to_wayfarer_id: [u8; 32],
    pub manifest_id: Vec<u8>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct DecodedEnvelopeV1 {
    pub to_wayfarer_id_hex: String,
    pub manifest_id_hex: String,
    pub body: Vec<u8>,
}

impl EnvelopeV1 {
    pub fn canonical_bytes_v1(&self) -> Result<Vec<u8>, String> {
        let mut payload_fields = BTreeMap::<String, ByteBuf>::new();
        payload_fields.insert(
            "to_wayfarer_id".to_string(),
            ByteBuf::from(self.to_wayfarer_id.to_vec()),
        );
        payload_fields.insert(
            "manifest_id".to_string(),
            ByteBuf::from(self.manifest_id.clone()),
        );
        payload_fields.insert("body".to_string(), ByteBuf::from(self.body.clone()));
        let value = to_cbor_value(&payload_fields)
            .map_err(|err| format!("failed serializing envelope cbor: {err}"))?;
        encode_cbor_value_deterministic(&value)
    }
}

pub fn build_envelope_payload_b64_from_utf8(
    to_wayfarer_id_hex: &str,
    body_utf8: &str,
) -> Result<String, String> {
    build_envelope_payload_b64(to_wayfarer_id_hex, body_utf8.as_bytes())
}

pub fn build_envelope_payload_b64(to_wayfarer_id_hex: &str, body: &[u8]) -> Result<String, String> {
    let to_wayfarer_id = parse_wayfarer_id_hex(to_wayfarer_id_hex)?;
    let manifest_id = Sha256::digest(body).to_vec();
    let envelope = EnvelopeV1 {
        to_wayfarer_id,
        manifest_id,
        body: body.to_vec(),
    };
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope.canonical_bytes_v1()?))
}

pub fn decode_envelope_payload_b64(payload_b64: &str) -> Result<DecodedEnvelopeV1, String> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|err| format!("failed to decode payload_b64: {err}"))?;

    parse_envelope_cbor(&raw)
}

fn parse_envelope_cbor(raw: &[u8]) -> Result<DecodedEnvelopeV1, String> {
    let fields = decode_cbor_value_exact(raw, "envelope")?;
    let canonical = encode_cbor_value_deterministic(&fields)
        .map_err(|err| format!("envelope cbor canonical re-encode failed: {err}"))?;
    if canonical.as_slice() != raw {
        return Err("envelope is not canonical CBOR encoding".to_string());
    }

    let Value::Map(map_entries) = fields else {
        return Err("envelope cbor root must be a map".to_string());
    };

    let mut field_map = BTreeMap::<String, Vec<u8>>::new();
    for (key, value) in map_entries {
        let Value::Text(key_text) = key else {
            return Err("envelope cbor keys must be UTF-8 strings".to_string());
        };
        if let Value::Bytes(bytes) = value {
            field_map.insert(key_text, bytes);
        }
    }

    if !field_map.contains_key("to_wayfarer_id")
        || !field_map.contains_key("manifest_id")
        || !field_map.contains_key("body")
    {
        return Err(
            "envelope cbor missing required keys: to_wayfarer_id, manifest_id, body".to_string(),
        );
    }

    let to_wayfarer_id = field_map
        .get("to_wayfarer_id")
        .cloned()
        .ok_or_else(|| "missing envelope field to_wayfarer_id".to_string())?
        .to_vec();
    if to_wayfarer_id.len() != 32 {
        return Err("invalid to_wayfarer_id length in envelope".to_string());
    }

    let manifest_id = field_map
        .get("manifest_id")
        .cloned()
        .ok_or_else(|| "missing envelope field manifest_id".to_string())?
        .to_vec();
    if manifest_id.len() != 32 {
        return Err("invalid manifest_id length in envelope".to_string());
    }

    let body = field_map
        .get("body")
        .cloned()
        .ok_or_else(|| "missing envelope field body".to_string())?
        .to_vec();

    Ok(DecodedEnvelopeV1 {
        to_wayfarer_id_hex: bytes_to_hex_lower(&to_wayfarer_id),
        manifest_id_hex: bytes_to_hex_lower(&manifest_id),
        body,
    })
}

pub fn to_cbor_value<T: Serialize>(value: &T) -> Result<Value, String> {
    let mut raw = Vec::new();
    into_writer(value, &mut raw).map_err(|err| format!("CBOR encode failed: {err}"))?;
    from_reader(raw.as_slice()).map_err(|err| format!("CBOR decode failed: {err}"))
}

pub fn decode_cbor_value_exact(raw: &[u8], context: &str) -> Result<Value, String> {
    let mut cursor = Cursor::new(raw);
    let value: Value =
        from_reader(&mut cursor).map_err(|err| format!("{context} cbor decode failed: {err}"))?;
    if cursor.position() as usize != raw.len() {
        return Err(format!(
            "{context} must contain exactly one complete CBOR value"
        ));
    }
    Ok(value)
}

pub fn encode_cbor_value_deterministic(value: &Value) -> Result<Vec<u8>, String> {
    let canonical = canonicalize_cbor_value(value.clone())?;
    let mut out = Vec::new();
    into_writer(&canonical, &mut out)
        .map_err(|err| format!("deterministic CBOR encode failed: {err}"))?;
    Ok(out)
}

fn canonicalize_cbor_value(mut value: Value) -> Result<Value, String> {
    match &mut value {
        Value::Array(items) => {
            for item in items.iter_mut() {
                *item = canonicalize_cbor_value(item.clone())?;
            }
        }
        Value::Map(entries) => {
            for (key, entry_value) in entries.iter_mut() {
                *key = canonicalize_cbor_value(key.clone())?;
                *entry_value = canonicalize_cbor_value(entry_value.clone())?;
            }
            entries.sort_by(|(left_key, _), (right_key, _)| {
                let left_encoded = encode_cbor_value_raw(left_key).unwrap_or_default();
                let right_encoded = encode_cbor_value_raw(right_key).unwrap_or_default();
                left_encoded.cmp(&right_encoded)
            });
        }
        _ => {}
    }
    Ok(value)
}

fn encode_cbor_value_raw(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    into_writer(value, &mut out).map_err(|err| format!("CBOR key encode failed: {err}"))?;
    Ok(out)
}

pub fn decode_envelope_payload_utf8_preview(payload_b64: &str) -> Result<String, String> {
    let decoded = decode_envelope_payload_b64(payload_b64)?;
    String::from_utf8(decoded.body).map_err(|_| "envelope body is not valid UTF-8".to_string())
}

fn parse_wayfarer_id_hex(hex_lower: &str) -> Result<[u8; 32], String> {
    if !is_valid_wayfarer_id(hex_lower) {
        return Err("invalid wayfarer_id; expected 64 lowercase hex chars".to_string());
    }

    let mut out = [0u8; 32];
    for (idx, slot) in out.iter_mut().enumerate() {
        let start = idx * 2;
        let end = start + 2;
        let byte = u8::from_str_radix(&hex_lower[start..end], 16)
            .map_err(|err| format!("failed to parse wayfarer_id hex byte: {err}"))?;
        *slot = byte;
    }
    Ok(out)
}

pub fn bytes_to_hex_lower(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        build_envelope_payload_b64_from_utf8, bytes_to_hex_lower, decode_envelope_payload_b64,
        encode_cbor_value_deterministic, parse_envelope_cbor,
    };
    use base64::Engine;
    use ciborium::value::Value;
    use sha2::{Digest, Sha256};

    const VECTOR_TO_WAYFARER_ID: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    const VECTOR_BODY: &str = "hello";
    const VECTOR_ENVELOPE_HEX: &str = "a364626f64794568656c6c6f6b6d616e69666573745f696458202cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b98246e746f5f77617966617265725f696458201111111111111111111111111111111111111111111111111111111111111111";
    const VECTOR_ITEM_ID_HEX: &str =
        "462e8f38ba0e88386d4b547fd6d3e63d8c263431bc19ea3bd2fb788ee6fcb702";
    const VECTOR_ENVELOPE_B64URL: &str = "o2Rib2R5RWhlbGxva21hbmlmZXN0X2lkWCAs8k26X7CjDiboOyrFueKeGxYeXB-nQl5zBDNik4uYJG50b193YXlmYXJlcl9pZFggERERERERERERERERERERERERERERERERERERERERERE";

    #[test]
    fn envelope_v1_matches_canonical_vector() {
        let envelope_b64 = build_envelope_payload_b64_from_utf8(VECTOR_TO_WAYFARER_ID, VECTOR_BODY)
            .expect("build vector envelope");
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&envelope_b64)
            .expect("decode envelope bytes");

        assert_eq!(bytes_to_hex_lower(&raw), VECTOR_ENVELOPE_HEX);
        assert_eq!(
            bytes_to_hex_lower(&Sha256::digest(&raw)),
            VECTOR_ITEM_ID_HEX
        );
        assert_eq!(envelope_b64, VECTOR_ENVELOPE_B64URL);
    }

    #[test]
    fn envelope_decode_rejects_noncanonical_map_order() {
        let to_wayfarer = vec![0x11u8; 32];
        let manifest = Sha256::digest(VECTOR_BODY.as_bytes()).to_vec();
        let body = VECTOR_BODY.as_bytes().to_vec();
        let noncanonical = Value::Map(vec![
            (
                Value::Text("to_wayfarer_id".to_string()),
                Value::Bytes(to_wayfarer),
            ),
            (
                Value::Text("manifest_id".to_string()),
                Value::Bytes(manifest),
            ),
            (Value::Text("body".to_string()), Value::Bytes(body)),
        ]);
        let noncanonical_raw = {
            let mut out = Vec::new();
            ciborium::ser::into_writer(&noncanonical, &mut out).expect("encode noncanonical");
            out
        };
        assert!(parse_envelope_cbor(&noncanonical_raw)
            .expect_err("must reject noncanonical ordering")
            .contains("not canonical"));
    }

    #[test]
    fn deterministic_encoder_orders_map_keys_by_encoded_bytes() {
        let value = Value::Map(vec![
            (
                Value::Text("payload".to_string()),
                Value::Integer(1u8.into()),
            ),
            (
                Value::Text("type".to_string()),
                Value::Text("HELLO".to_string()),
            ),
        ]);
        let raw = encode_cbor_value_deterministic(&value).expect("encode deterministic");
        let expected = vec![
            0xa2, 0x64, b't', b'y', b'p', b'e', 0x65, b'H', b'E', b'L', b'L', b'O', 0x67, b'p',
            b'a', b'y', b'l', b'o', b'a', b'd', 0x01,
        ];
        assert_eq!(raw, expected);
    }

    #[test]
    fn envelope_payload_decoder_accepts_vector() {
        let decoded = decode_envelope_payload_b64(VECTOR_ENVELOPE_B64URL).expect("decode envelope");
        assert_eq!(decoded.to_wayfarer_id_hex, VECTOR_TO_WAYFARER_ID);
        assert_eq!(decoded.body, VECTOR_BODY.as_bytes());
    }
}
