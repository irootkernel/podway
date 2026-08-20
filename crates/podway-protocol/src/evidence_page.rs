//! The opaque `evidence.read` continuation token.
//!
//! [ADR-0024](../../../../docs/architecture-decision-records/0024-bounded-evidence-scale-and-paged-read-back.md)
//! defines the token as a fixed-order binary payload in unpadded base64url. It is not an
//! authentication or authorization credential: it carries no secret, it is not signed, and the
//! daemon re-validates every field against current authoritative state on each read. Its only job
//! is to name exactly which snapshot a continuation belongs to, so a caller can never resume into a
//! different value, a different attempt, or a different session without being told.

use crate::ProtocolError;

/// The current page-token payload version.
pub const EVIDENCE_PAGE_TOKEN_VERSION_V1: u8 = 1;

/// Maximum base64url characters an encoded page token may occupy.
pub const MAX_EVIDENCE_PAGE_TOKEN_CHARS_V1: usize = podway_core::MAX_EVIDENCE_PAGE_TOKEN_CHARS_V2;

const UUID_BYTES: usize = 16;
const DIGEST_BYTES: usize = 32;
const OFFSET_BYTES: usize = 8;
const MAX_ITEM_ID_BYTES: usize = 64;
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The decoded contents of one page token.
///
/// Every field is a fence. The daemon compares each one against current state before serving a
/// page, so a token that survives decoding still proves nothing on its own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePageTokenV1 {
    session_id: [u8; UUID_BYTES],
    consumer_attempt_id: [u8; UUID_BYTES],
    source_attempt_id: [u8; UUID_BYTES],
    item_id: String,
    value_digest: [u8; DIGEST_BYTES],
    offset: u64,
}

impl serde::Serialize for EvidencePageTokenV1 {
    /// Serializes as its canonical encoding, so a decoded token still renders as the wire text.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.encode())
    }
}

impl EvidencePageTokenV1 {
    /// Builds a token from the identities a page belongs to.
    ///
    /// UUID and digest inputs are the canonical text forms Podway uses everywhere else; this is the
    /// single place they are reduced to bytes, so an encoder and a decoder cannot disagree on the
    /// payload layout.
    pub fn new(
        session_id: &str,
        consumer_attempt_id: &str,
        source_attempt_id: &str,
        item_id: &str,
        value_digest: &str,
        offset: u64,
    ) -> Result<Self, ProtocolError> {
        if item_id.is_empty() || item_id.len() > MAX_ITEM_ID_BYTES {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }
        Ok(Self {
            session_id: uuid_bytes_v1(session_id)?,
            consumer_attempt_id: uuid_bytes_v1(consumer_attempt_id)?,
            source_attempt_id: uuid_bytes_v1(source_attempt_id)?,
            item_id: item_id.to_owned(),
            value_digest: digest_bytes_v1(value_digest)?,
            offset,
        })
    }

    pub fn session_id(&self) -> String {
        uuid_text_v1(&self.session_id)
    }

    pub fn consumer_attempt_id(&self) -> String {
        uuid_text_v1(&self.consumer_attempt_id)
    }

    pub fn source_attempt_id(&self) -> String {
        uuid_text_v1(&self.source_attempt_id)
    }

    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    pub fn value_digest(&self) -> String {
        let mut text = String::from("sha256:");
        for byte in self.value_digest {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }

    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Encodes the fixed-order payload as unpadded base64url.
    pub fn encode(&self) -> String {
        let mut payload = Vec::with_capacity(
            1 + UUID_BYTES * 3 + 1 + self.item_id.len() + DIGEST_BYTES + OFFSET_BYTES,
        );
        payload.push(EVIDENCE_PAGE_TOKEN_VERSION_V1);
        payload.extend_from_slice(&self.session_id);
        payload.extend_from_slice(&self.consumer_attempt_id);
        payload.extend_from_slice(&self.source_attempt_id);
        payload.push(
            u8::try_from(self.item_id.len()).expect("item identifiers are bounded to 64 bytes"),
        );
        payload.extend_from_slice(self.item_id.as_bytes());
        payload.extend_from_slice(&self.value_digest);
        payload.extend_from_slice(&self.offset.to_be_bytes());
        encode_base64url_v1(&payload)
    }

    /// Decodes a page token, rejecting anything that is not exactly one canonical payload.
    ///
    /// A malformed or oversized token is a decoding failure, not a state conflict: it is refused
    /// here, before any comparison against current state, so a caller can never learn anything
    /// about a session from a token it made up.
    pub fn decode(token: &str) -> Result<Self, ProtocolError> {
        if token.is_empty() || token.chars().count() > MAX_EVIDENCE_PAGE_TOKEN_CHARS_V1 {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }
        let payload = decode_base64url_v1(token)?;
        let mut cursor = 0usize;
        let version = *payload
            .get(cursor)
            .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        if version != EVIDENCE_PAGE_TOKEN_VERSION_V1 {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }
        cursor += 1;

        let session_id = take_array_v1::<UUID_BYTES>(&payload, &mut cursor)?;
        let consumer_attempt_id = take_array_v1::<UUID_BYTES>(&payload, &mut cursor)?;
        let source_attempt_id = take_array_v1::<UUID_BYTES>(&payload, &mut cursor)?;

        let length = usize::from(
            *payload
                .get(cursor)
                .ok_or(ProtocolError::InvalidEvidencePageToken)?,
        );
        cursor += 1;
        if length == 0 || length > MAX_ITEM_ID_BYTES {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }
        let item_bytes = payload
            .get(cursor..cursor + length)
            .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        let item_id = core::str::from_utf8(item_bytes)
            .map_err(|_| ProtocolError::InvalidEvidencePageToken)?
            .to_owned();
        cursor += length;

        let value_digest = take_array_v1::<DIGEST_BYTES>(&payload, &mut cursor)?;
        let offset_bytes = take_array_v1::<OFFSET_BYTES>(&payload, &mut cursor)?;
        if cursor != payload.len() {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }

        Ok(Self {
            session_id,
            consumer_attempt_id,
            source_attempt_id,
            item_id,
            value_digest,
            offset: u64::from_be_bytes(offset_bytes),
        })
    }
}

fn take_array_v1<const N: usize>(
    payload: &[u8],
    cursor: &mut usize,
) -> Result<[u8; N], ProtocolError> {
    let slice = payload
        .get(*cursor..*cursor + N)
        .ok_or(ProtocolError::InvalidEvidencePageToken)?;
    let mut value = [0u8; N];
    value.copy_from_slice(slice);
    *cursor += N;
    Ok(value)
}

fn uuid_bytes_v1(value: &str) -> Result<[u8; UUID_BYTES], ProtocolError> {
    // Only the canonical 8-4-4-4-12 lowercase form is accepted. Admitting other spellings would
    // make `session_id()` silently re-normalize an identifier the rest of Podway never rewrites.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut hex = String::with_capacity(UUID_BYTES * 2);
    let mut rest = value;
    for (index, width) in GROUPS.iter().enumerate() {
        if index > 0 {
            rest = rest
                .strip_prefix('-')
                .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        }
        let group = rest
            .get(..*width)
            .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        if !group
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
        {
            return Err(ProtocolError::InvalidEvidencePageToken);
        }
        hex.push_str(group);
        rest = &rest[*width..];
    }
    if !rest.is_empty() {
        return Err(ProtocolError::InvalidEvidencePageToken);
    }
    hex_bytes_v1::<UUID_BYTES>(&hex)
}

fn uuid_text_v1(bytes: &[u8; UUID_BYTES]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn digest_bytes_v1(value: &str) -> Result<[u8; DIGEST_BYTES], ProtocolError> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or(ProtocolError::InvalidEvidencePageToken)?;
    // Lowercase only, for the same reason as the UUID form: one digest text is one token.
    if hex.len() != DIGEST_BYTES * 2
        || !hex
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
    {
        return Err(ProtocolError::InvalidEvidencePageToken);
    }
    hex_bytes_v1::<DIGEST_BYTES>(hex)
}

fn hex_bytes_v1<const N: usize>(hex: &str) -> Result<[u8; N], ProtocolError> {
    let mut bytes = [0u8; N];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let pair = hex
            .get(index * 2..index * 2 + 2)
            .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        *slot =
            u8::from_str_radix(pair, 16).map_err(|_| ProtocolError::InvalidEvidencePageToken)?;
    }
    Ok(bytes)
}

fn encode_base64url_v1(payload: &[u8]) -> String {
    let mut encoded = String::with_capacity(payload.len().div_ceil(3) * 4);
    for chunk in payload.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value =
            (u32::from(buffer[0]) << 16) | (u32::from(buffer[1]) << 8) | u32::from(buffer[2]);
        let characters = [
            BASE64URL[(value >> 18) as usize & 0x3f],
            BASE64URL[(value >> 12) as usize & 0x3f],
            BASE64URL[(value >> 6) as usize & 0x3f],
            BASE64URL[value as usize & 0x3f],
        ];
        // Unpadded: a 1- or 2-byte tail contributes only the characters it actually encodes.
        let kept = chunk.len() + 1;
        for character in characters.iter().take(kept) {
            encoded.push(char::from(*character));
        }
    }
    encoded
}

fn decode_base64url_v1(token: &str) -> Result<Vec<u8>, ProtocolError> {
    let mut sextets = Vec::with_capacity(token.len());
    for character in token.bytes() {
        let value = BASE64URL
            .iter()
            .position(|candidate| *candidate == character)
            .ok_or(ProtocolError::InvalidEvidencePageToken)?;
        sextets.push(u8::try_from(value).expect("base64url alphabet indices fit in a byte"));
    }
    // An unpadded group of exactly one sextet cannot encode any byte, so it is malformed.
    if sextets.len() % 4 == 1 {
        return Err(ProtocolError::InvalidEvidencePageToken);
    }
    let mut payload = Vec::with_capacity(sextets.len() / 4 * 3);
    for chunk in sextets.chunks(4) {
        let mut value = 0u32;
        for (index, sextet) in chunk.iter().enumerate() {
            value |= u32::from(*sextet) << (18 - 6 * index);
        }
        let kept = chunk.len() - 1;
        // A short final group carries bits the decoded bytes do not use. A canonical encoder always
        // leaves them zero, so a non-zero tail is a second spelling of the same payload and is
        // refused: exactly one encoding must decode to one token.
        if kept < 3 {
            let unused_bits = 8 * (3 - kept);
            if value & ((1u32 << unused_bits) - 1) != 0 {
                return Err(ProtocolError::InvalidEvidencePageToken);
            }
        }
        let bytes = [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        for byte in bytes.iter().take(kept) {
            payload.push(*byte);
        }
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "00000000-0000-4000-8000-000000000001";
    const CONSUMER: &str = "00000000-0000-4000-8000-000000000002";
    const SOURCE: &str = "00000000-0000-4000-8000-000000000003";
    const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn token(offset: u64) -> EvidencePageTokenV1 {
        EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, "notes", DIGEST, offset)
            .expect("a canonical token is constructible")
    }

    #[test]
    fn v2scl004_a_token_round_trips_every_bound_field() {
        let original = token(4_096);
        let encoded = original.encode();
        let decoded = EvidencePageTokenV1::decode(&encoded).expect("the token decodes");
        assert_eq!(decoded, original);
        assert_eq!(decoded.session_id(), SESSION);
        assert_eq!(decoded.consumer_attempt_id(), CONSUMER);
        assert_eq!(decoded.source_attempt_id(), SOURCE);
        assert_eq!(decoded.item_id(), "notes");
        assert_eq!(decoded.value_digest(), DIGEST);
        assert_eq!(decoded.offset(), 4_096);
    }

    #[test]
    fn v2scl004_encoding_is_deterministic_and_within_the_published_bound() {
        assert_eq!(token(0).encode(), token(0).encode());
        assert_ne!(token(0).encode(), token(1).encode());
        let maximum_item = "a".repeat(MAX_ITEM_ID_BYTES);
        let widest =
            EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, &maximum_item, DIGEST, u64::MAX)
                .expect("the widest token is constructible")
                .encode();
        assert!(widest.chars().count() <= MAX_EVIDENCE_PAGE_TOKEN_CHARS_V1);
        assert!(widest.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        }));
    }

    #[test]
    fn v2scl004_malformed_oversized_and_truncated_tokens_are_refused() {
        assert!(EvidencePageTokenV1::decode("").is_err());
        assert!(EvidencePageTokenV1::decode(&"A".repeat(257)).is_err());
        // Padding and any other character outside the unpadded base64url alphabet.
        assert!(EvidencePageTokenV1::decode("AAAA=").is_err());
        assert!(EvidencePageTokenV1::decode("AA*A").is_err());

        let encoded = token(7).encode();
        assert!(EvidencePageTokenV1::decode(&encoded[..encoded.len() - 4]).is_err());
        let mut extended = encoded.clone();
        extended.push_str("AAAA");
        assert!(EvidencePageTokenV1::decode(&extended).is_err());

        // A version byte the build does not serve is refused before any state comparison.
        let mut payload = vec![EVIDENCE_PAGE_TOKEN_VERSION_V1 + 1];
        payload.extend_from_slice(&[0u8; UUID_BYTES * 3]);
        payload.push(5);
        payload.extend_from_slice(b"notes");
        payload.extend_from_slice(&[0u8; DIGEST_BYTES]);
        payload.extend_from_slice(&0u64.to_be_bytes());
        assert!(EvidencePageTokenV1::decode(&encode_base64url_v1(&payload)).is_err());
    }

    #[test]
    fn v2scl004_construction_rejects_malformed_identities() {
        assert!(EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, "", DIGEST, 0).is_err());
        assert!(
            EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, &"a".repeat(65), DIGEST, 0)
                .is_err()
        );
        assert!(EvidencePageTokenV1::new("not-a-uuid", CONSUMER, SOURCE, "n", DIGEST, 0).is_err());
        assert!(EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, "n", "sha256:zz", 0).is_err());
        assert!(EvidencePageTokenV1::new(SESSION, CONSUMER, SOURCE, "n", &DIGEST[7..], 0).is_err());
    }

    #[test]
    fn v2scl004_non_canonical_identifier_spellings_are_refused() {
        let canonical = |session: &str, digest: &str| {
            EvidencePageTokenV1::new(session, CONSUMER, SOURCE, "notes", digest, 0)
        };
        assert!(canonical(SESSION, DIGEST).is_ok());
        // An undashed UUID, a misplaced dash, and an uppercase digest all name the same bytes as a
        // canonical spelling, so admitting them would give one page two tokens.
        assert!(canonical(&SESSION.replace('-', ""), DIGEST).is_err());
        assert!(canonical("00000000-0000-4000-800000000000000000ff", DIGEST).is_err());
        let lettered = "0000000a-000b-4000-8000-00000000000c";
        assert!(canonical(lettered, DIGEST).is_ok());
        assert!(canonical(&lettered.to_uppercase(), DIGEST).is_err());
        assert!(canonical(SESSION, &format!("sha256:{}", "A".repeat(64))).is_err());
    }

    #[test]
    fn v2scl004_non_canonical_tail_bits_are_refused() {
        // "AA" decodes one zero byte; the second sextet's low four bits are unused. "AB" sets one
        // of them and would decode to the same byte, so only the canonical spelling is admitted.
        assert_eq!(decode_base64url_v1("AA").unwrap(), vec![0u8]);
        assert!(decode_base64url_v1("AB").is_err());
        assert_eq!(decode_base64url_v1("AAA").unwrap(), vec![0u8, 0u8]);
        assert!(decode_base64url_v1("AAB").is_err());
    }

    #[test]
    fn v2scl004_base64url_round_trips_every_tail_length() {
        for length in 0..=8 {
            let payload: Vec<u8> = (0..length).map(|index| index as u8).collect();
            let encoded = encode_base64url_v1(&payload);
            assert!(!encoded.contains('='));
            assert_eq!(decode_base64url_v1(&encoded).expect("decodes"), payload);
        }
    }
}
