use std::collections::BTreeMap;

use base64::Engine;
use ciborium::value::Value;
use ciborium::{de::from_reader, ser::into_writer};
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

        let mut out = Vec::new();
        into_writer(&payload_fields, &mut out)
            .map_err(|err| format!("failed serializing envelope cbor: {err}"))?;
        Ok(out)
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
    let fields: Value =
        from_reader(raw).map_err(|err| format!("envelope cbor decode failed: {err}"))?;

    let mut canonical = Vec::new();
    into_writer(&fields, &mut canonical)
        .map_err(|err| format!("envelope cbor canonical re-encode failed: {err}"))?;
    if canonical != raw {
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
