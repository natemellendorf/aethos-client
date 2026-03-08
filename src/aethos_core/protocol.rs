use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloFrame {
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub wayfarer_id: String,
    pub device_id: String,
}

impl HelloFrame {
    pub fn new(wayfarer_id: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            frame_type: "hello",
            wayfarer_id: wayfarer_id.into(),
            device_id: device_id.into(),
        }
    }

    #[allow(dead_code)]
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendFrame {
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub to: String,
    pub payload_b64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<i64>,
}

impl SendFrame {
    pub fn new(
        to: impl Into<String>,
        payload_b64: impl Into<String>,
        client_msg_id: Option<String>,
        ttl_seconds: Option<i64>,
    ) -> Result<Self, String> {
        let to = to.into();
        let payload_b64 = payload_b64.into();

        if !is_valid_wayfarer_id(&to) {
            return Err("invalid send.to format; expected 64 lowercase hex chars".to_string());
        }
        if !is_valid_payload_b64(&payload_b64) {
            return Err("invalid payload_b64 format; expected unpadded base64url".to_string());
        }
        if let Some(id) = &client_msg_id {
            if !is_valid_client_msg_id(id) {
                return Err(
                    "invalid client_msg_id format; expected 1..128 visible ASCII chars".to_string(),
                );
            }
        }
        if let Some(ttl) = ttl_seconds {
            if ttl <= 0 {
                return Err("invalid ttl_seconds; expected positive integer".to_string());
            }
        }

        Ok(Self {
            frame_type: "send",
            to,
            payload_b64,
            client_msg_id,
            ttl_seconds,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullFrame {
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

impl PullFrame {
    pub fn new(limit: Option<i64>) -> Result<Self, String> {
        if let Some(value) = limit {
            if value <= 0 {
                return Err("invalid pull.limit; expected positive integer".to_string());
            }
        }

        Ok(Self {
            frame_type: "pull",
            limit,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckFrame {
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub msg_id: String,
}

impl AckFrame {
    pub fn new(msg_id: impl Into<String>) -> Result<Self, String> {
        let msg_id = msg_id.into();
        if !is_valid_msg_id(&msg_id) {
            return Err("invalid ack.msg_id; expected non-empty value".to_string());
        }

        Ok(Self {
            frame_type: "ack",
            msg_id,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageItem {
    pub msg_id: String,
    #[serde(rename = "from")]
    pub from_wayfarer_id: String,
    pub payload_b64: String,
    pub received_at: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum RelayInboundFrame {
    #[serde(rename = "hello_ok")]
    HelloOk { relay_id: Option<String> },
    #[serde(rename = "send_ok")]
    SendOk {
        msg_id: String,
        received_at: Option<i64>,
        expires_at: Option<i64>,
    },
    #[serde(rename = "message")]
    Message {
        msg_id: String,
        #[serde(rename = "from")]
        from_wayfarer_id: String,
        payload_b64: String,
        received_at: i64,
    },
    #[serde(rename = "messages")]
    Messages { messages: Vec<MessageItem> },
    #[serde(rename = "ack_ok")]
    AckOk { msg_id: String },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

impl RelayInboundFrame {
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

pub fn is_valid_wayfarer_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn is_valid_device_id(value: &str) -> bool {
    !value.trim().is_empty()
}

pub fn is_valid_client_msg_id(value: &str) -> bool {
    (1..=128).contains(&value.len()) && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}

pub fn is_valid_payload_b64(value: &str) -> bool {
    !value.contains('=')
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .is_ok()
}

pub fn is_valid_msg_id(value: &str) -> bool {
    !value.trim().is_empty()
}

#[derive(Debug, Clone)]
pub struct EnvelopeV1 {
    pub to_wayfarer_id: [u8; 32],
    pub manifest_id: Vec<u8>,
    pub body: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DecodedEnvelopeV1 {
    pub to_wayfarer_id_hex: String,
    pub manifest_id_hex: String,
    pub body: Vec<u8>,
}

impl EnvelopeV1 {
    pub fn canonical_bytes_v1(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(1);
        out.push(1);

        encode_field(&mut out, 1, &self.to_wayfarer_id);
        encode_field(&mut out, 2, &self.manifest_id);
        encode_field(&mut out, 3, &self.body);

        out
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

    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope.canonical_bytes_v1()))
}

#[allow(dead_code)]
pub fn decode_envelope_payload_b64(payload_b64: &str) -> Result<DecodedEnvelopeV1, String> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|err| format!("failed to decode payload_b64: {err}"))?;

    if raw.len() < 2 {
        return Err("envelope too short".to_string());
    }
    if raw[0] != 1 || raw[1] != 1 {
        return Err("unsupported envelope canonical version".to_string());
    }

    let mut cursor = 2usize;
    let mut to_wayfarer_id: Option<Vec<u8>> = None;
    let mut manifest_id: Option<Vec<u8>> = None;
    let mut body: Option<Vec<u8>> = None;

    while cursor < raw.len() {
        if cursor + 5 > raw.len() {
            return Err("truncated envelope field header".to_string());
        }

        let field_id = raw[cursor];
        cursor += 1;

        let len_bytes: [u8; 4] = raw[cursor..cursor + 4]
            .try_into()
            .map_err(|_| "failed to read envelope field length".to_string())?;
        cursor += 4;

        let field_len = u32::from_be_bytes(len_bytes) as usize;
        if cursor + field_len > raw.len() {
            return Err("truncated envelope field payload".to_string());
        }

        let value = raw[cursor..cursor + field_len].to_vec();
        cursor += field_len;

        match field_id {
            1 => to_wayfarer_id = Some(value),
            2 => manifest_id = Some(value),
            3 => body = Some(value),
            _ => {}
        }
    }

    let to_wayfarer_id =
        to_wayfarer_id.ok_or_else(|| "missing envelope field to_wayfarer_id".to_string())?;
    if to_wayfarer_id.len() != 32 {
        return Err("invalid to_wayfarer_id length in envelope".to_string());
    }

    let manifest_id =
        manifest_id.ok_or_else(|| "missing envelope field manifest_id".to_string())?;
    let body = body.ok_or_else(|| "missing envelope field body".to_string())?;

    Ok(DecodedEnvelopeV1 {
        to_wayfarer_id_hex: bytes_to_hex_lower(&to_wayfarer_id),
        manifest_id_hex: bytes_to_hex_lower(&manifest_id),
        body,
    })
}

#[allow(dead_code)]
pub fn decode_envelope_payload_utf8_preview(payload_b64: &str) -> Result<String, String> {
    let decoded = decode_envelope_payload_b64(payload_b64)?;
    match String::from_utf8(decoded.body) {
        Ok(text) => Ok(text),
        Err(_) => Err("envelope body is not valid UTF-8".to_string()),
    }
}

fn encode_field(out: &mut Vec<u8>, field_id: u8, raw: &[u8]) {
    out.push(field_id);
    out.extend((raw.len() as u32).to_be_bytes());
    out.extend(raw);
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

#[allow(dead_code)]
fn bytes_to_hex_lower(input: &[u8]) -> String {
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
        build_envelope_payload_b64_from_utf8, decode_envelope_payload_b64,
        decode_envelope_payload_utf8_preview, is_valid_client_msg_id, is_valid_payload_b64,
        is_valid_wayfarer_id, AckFrame, EnvelopeV1, HelloFrame, PullFrame, RelayInboundFrame,
        SendFrame,
    };

    #[test]
    fn hello_frame_serializes_to_v1_shape() {
        let frame = HelloFrame::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "device-1",
        );
        let serialized = frame.to_json().expect("serialize frame");

        assert!(serialized.contains("\"type\":\"hello\""));
        assert!(serialized.contains("\"wayfarer_id\""));
        assert!(serialized.contains("\"device_id\""));
    }

    #[test]
    fn validates_wayfarer_id_format() {
        assert!(is_valid_wayfarer_id(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_valid_wayfarer_id("abc"));
        assert!(!is_valid_wayfarer_id(
            "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF"
        ));
    }

    #[test]
    fn send_frame_requires_valid_v1_fields() {
        let frame = SendFrame::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "SGVsbG8",
            Some("550e8400-e29b-41d4-a716-446655440000".to_string()),
            Some(3600),
        )
        .expect("build send frame");

        let serialized = serde_json::to_string(&frame).expect("serialize send frame");
        assert!(serialized.contains("\"type\":\"send\""));
        assert!(serialized.contains("\"payload_b64\":\"SGVsbG8\""));
    }

    #[test]
    fn pull_and_ack_frame_validate_inputs() {
        assert!(PullFrame::new(Some(50)).is_ok());
        assert!(PullFrame::new(Some(0)).is_err());
        assert!(AckFrame::new("msg-1").is_ok());
        assert!(AckFrame::new(" ").is_err());
    }

    #[test]
    fn validates_client_msg_id_and_payload_b64() {
        assert!(is_valid_client_msg_id("client-1"));
        assert!(!is_valid_client_msg_id(""));
        assert!(!is_valid_client_msg_id("contains spaces"));

        assert!(is_valid_payload_b64("SGVsbG8"));
        assert!(!is_valid_payload_b64("SGVsbG8="));
    }

    #[test]
    fn parses_relay_inbound_frames() {
        let frame = RelayInboundFrame::from_json(
            "{\"type\":\"messages\",\"messages\":[{\"msg_id\":\"m1\",\"from\":\"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\",\"payload_b64\":\"SGVsbG8\",\"received_at\":123}]}"
        )
        .expect("parse inbound messages frame");

        match frame {
            RelayInboundFrame::Messages { messages } => assert_eq!(messages.len(), 1),
            _ => panic!("unexpected frame variant"),
        }
    }

    #[test]
    fn envelope_v1_canonical_bytes_follow_field_order() {
        let envelope = EnvelopeV1 {
            to_wayfarer_id: [0x11; 32],
            manifest_id: vec![0x22; 32],
            body: b"hello".to_vec(),
        };

        let bytes = envelope.canonical_bytes_v1();
        assert_eq!(bytes[0], 1);
        assert_eq!(bytes[1], 1);
        assert_eq!(bytes[2], 1);
    }

    #[test]
    fn builds_payload_b64_for_valid_wayfarer_and_body() {
        let payload = build_envelope_payload_b64_from_utf8(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "hello from linux",
        )
        .expect("build payload");

        assert!(is_valid_payload_b64(&payload));
        assert!(!payload.is_empty());
    }

    #[test]
    fn decodes_payload_b64_back_to_utf8_body() {
        let payload = build_envelope_payload_b64_from_utf8(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "hello decode",
        )
        .expect("build payload");

        let preview = decode_envelope_payload_utf8_preview(&payload).expect("decode utf8 preview");
        assert_eq!(preview, "hello decode");

        let decoded = decode_envelope_payload_b64(&payload).expect("decode envelope");
        assert_eq!(decoded.to_wayfarer_id_hex.len(), 64);
        assert!(!decoded.manifest_id_hex.is_empty());
    }
}
