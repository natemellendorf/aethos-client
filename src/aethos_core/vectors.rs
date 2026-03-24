#[cfg(test)]
use serde::Deserialize;

#[cfg(test)]
const ENVELOPE_VECTORS_SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
const ENVELOPE_VECTORS_JSON: &str = include_str!("../../test-data/gossip-v1/envelope_vectors.json");

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct EnvelopeVectorSet {
    pub(crate) version: u32,
    pub(crate) vectors: Vec<EnvelopeVector>,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct EnvelopeVector {
    pub(crate) payload_b64: String,
    pub(crate) item_id_hex: String,
    pub(crate) canonical_envelope_cbor_hex: String,
    pub(crate) expected_decoded: ExpectedDecoded,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
pub(crate) struct ExpectedDecoded {
    pub(crate) to_wayfarer_id: String,
    pub(crate) manifest_id: String,
    pub(crate) body_utf8_preview: String,
}

#[cfg(test)]
pub(crate) fn load_envelope_vectors() -> EnvelopeVectorSet {
    let vector_set: EnvelopeVectorSet =
        serde_json::from_str(ENVELOPE_VECTORS_JSON).expect("parse vector json");
    assert_eq!(
        vector_set.version, ENVELOPE_VECTORS_SCHEMA_VERSION,
        "envelope vector schema version must match"
    );
    assert!(
        !vector_set.vectors.is_empty(),
        "envelope vectors must not be empty"
    );
    vector_set
}
