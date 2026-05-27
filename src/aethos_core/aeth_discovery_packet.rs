pub const AETH_DISCOVERY_MAGIC: u32 = 0x4145_5448;
pub const AETH_DISCOVERY_VERSION: u8 = 0x01;
pub const AETH_DISCOVERY_HEADER_LEN: usize = 29;
pub const AETH_DISCOVERY_CAPABILITIES: u16 = 0x0001;
pub const AETH_DISCOVERY_GOSSIP_VERSION: u8 = 0x01;
pub const AETH_DISCOVERY_GOSSIP_PORT: u16 = 47_655;
pub const AETH_DISCOVERY_MULTICAST_PORT: u16 = 47_656;
pub const AETH_DISCOVERY_IPV4_MULTICAST_GROUP: [u8; 4] = [239, 255, 37, 105];
pub const AETH_DISCOVERY_IPV6_MULTICAST_GROUP: &str = "ff02::a37:105";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AethDiscoveryMessageType {
    Probe = 0x01,
    Response = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AethDiscoveryPacket {
    pub message_type: AethDiscoveryMessageType,
    pub nonce: [u8; 16],
    pub sender_port: u16,
    pub gossip_version: u8,
    pub capabilities: u16,
}

impl AethDiscoveryPacket {
    pub fn probe(nonce: [u8; 16], sender_port: u16) -> Self {
        Self {
            message_type: AethDiscoveryMessageType::Probe,
            nonce,
            sender_port,
            gossip_version: AETH_DISCOVERY_GOSSIP_VERSION,
            capabilities: AETH_DISCOVERY_CAPABILITIES,
        }
    }

    pub fn response(nonce: [u8; 16], sender_port: u16) -> Self {
        Self {
            message_type: AethDiscoveryMessageType::Response,
            nonce,
            sender_port,
            gossip_version: AETH_DISCOVERY_GOSSIP_VERSION,
            capabilities: AETH_DISCOVERY_CAPABILITIES,
        }
    }

    pub fn encode(self) -> [u8; AETH_DISCOVERY_HEADER_LEN] {
        let mut out = [0u8; AETH_DISCOVERY_HEADER_LEN];
        out[0..4].copy_from_slice(&AETH_DISCOVERY_MAGIC.to_be_bytes());
        out[4] = AETH_DISCOVERY_VERSION;
        out[5] = self.message_type as u8;
        out[6..22].copy_from_slice(&self.nonce);
        out[22..24].copy_from_slice(&self.sender_port.to_be_bytes());
        out[24] = self.gossip_version;
        out[25..27].copy_from_slice(&self.capabilities.to_be_bytes());
        out[27..29].copy_from_slice(&0u16.to_be_bytes());
        out
    }

    pub fn decode(raw: &[u8]) -> Result<Self, String> {
        if raw.len() < AETH_DISCOVERY_HEADER_LEN {
            return Err("aeth discovery packet too short".to_string());
        }
        let magic = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
        if magic != AETH_DISCOVERY_MAGIC {
            return Err("invalid aeth discovery magic".to_string());
        }
        if raw[4] != AETH_DISCOVERY_VERSION {
            return Err("invalid aeth discovery version".to_string());
        }
        let message_type = match raw[5] {
            0x01 => AethDiscoveryMessageType::Probe,
            0x02 => AethDiscoveryMessageType::Response,
            _ => return Err("invalid aeth discovery message type".to_string()),
        };
        let body_len = u16::from_be_bytes([raw[27], raw[28]]);
        if body_len != 0 {
            return Err("non-zero aeth discovery body length".to_string());
        }
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&raw[6..22]);
        Ok(Self {
            message_type,
            nonce,
            sender_port: u16::from_be_bytes([raw[22], raw[23]]),
            gossip_version: raw[24],
            capabilities: u16::from_be_bytes([raw[25], raw[26]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_encoding_matches_ios_aeth_fixture_shape() {
        let packet = AethDiscoveryPacket::probe([0x01; 16], AETH_DISCOVERY_MULTICAST_PORT);
        let encoded = packet.encode();
        assert_eq!(encoded.len(), AETH_DISCOVERY_HEADER_LEN);
        assert_eq!(&encoded[0..4], b"AETH");
        assert_eq!(encoded[4], 0x01);
        assert_eq!(encoded[5], 0x01);
        assert_eq!(
            &encoded[22..24],
            &AETH_DISCOVERY_MULTICAST_PORT.to_be_bytes()
        );
        assert_eq!(&encoded[27..29], &[0x00, 0x00]);
    }

    #[test]
    fn response_decode_rejects_non_zero_body() {
        let mut encoded =
            AethDiscoveryPacket::response([0x02; 16], AETH_DISCOVERY_GOSSIP_PORT).encode();
        encoded[28] = 1;
        assert!(AethDiscoveryPacket::decode(&encoded).is_err());
    }
}
