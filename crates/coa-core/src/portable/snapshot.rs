//! Snapshot encoding: canonical JSON -> SHA-256 -> zstd.
//!
//! * The **content hash** is the SHA-256 of the canonical (normalised) JSON, i.e. of the data, not of the compressed
//!   bytes, so it does not depend on the compressor version.
//! * Decoding is hostile-input safe: compressed and decompressed sizes are capped *while reading*
//!   (decompression bombs), the format version is checked before the shape, unknown fields are refused, and the
//!   hash and every structural limit are verified.

use std::io::Read;

use sha2::{Digest, Sha256};

use super::error::{PortableError, Result};
use super::model::{limits::*, PortableCharacter};
use super::versions::{PORTABLE_CHARACTER_FORMAT_VERSION, PORTABLE_CHARACTER_MIN_READ_VERSION};

const ZSTD_LEVEL: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedSnapshot {
    pub content_hash: [u8; 32],
    pub uncompressed_size: u64,
    pub payload: Vec<u8>,
}

pub fn hash_hex(hash: &[u8; 32]) -> String {
    hex::encode(hash)
}

/// The canonical JSON of a character (normalised first).
pub fn canonical_json(model: &PortableCharacter) -> Result<Vec<u8>> {
    let normalized = model.clone().normalized();
    let json = serde_json::to_vec(&normalized)?;
    if json.len() > MAX_SNAPSHOT_BYTES {
        return Err(PortableError::LimitExceeded(format!("the snapshot is larger than {MAX_SNAPSHOT_BYTES} bytes")));
    }
    Ok(json)
}

pub fn content_hash(model: &PortableCharacter) -> Result<[u8; 32]> {
    Ok(Sha256::digest(canonical_json(model)?).into())
}

/// Validate, then encode.
pub fn encode(model: &PortableCharacter) -> Result<EncodedSnapshot> {
    let model = model.clone().normalized();
    model.validate()?;
    let json = canonical_json(&model)?;
    let content_hash: [u8; 32] = Sha256::digest(&json).into();
    let payload = zstd::stream::encode_all(json.as_slice(), ZSTD_LEVEL)?;
    if payload.len() > MAX_COMPRESSED_BYTES {
        return Err(PortableError::LimitExceeded(format!("the compressed snapshot is larger than {MAX_COMPRESSED_BYTES} bytes")));
    }
    Ok(EncodedSnapshot { content_hash, uncompressed_size: json.len() as u64, payload })
}

/// A decoded snapshot with where it came from: the character is always in the current format, `source_version` and
/// `source_hash` say what was stored (the hash of the payload as it was written, verified before any migration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub model: PortableCharacter,
    pub source_version: u32,
    pub source_hash: [u8; 32],
}

/// Decode and fully verify. `expected_hash` is the hash recorded next to the payload.
pub fn decode(payload: &[u8], expected_hash: Option<&[u8; 32]>) -> Result<PortableCharacter> {
    decode_verified(payload, expected_hash).map(|d| d.model)
}

/// The hash of the character **as the current format writes it**, for a stored payload of any readable version. A payload that was
/// written in format 1 has another hash than the same character in format 2; whoever asks "is this the same character?" compares
/// these, never the stored hash with a freshly encoded one.
pub fn semantic_hash(payload: &[u8], stored_hash: &[u8; 32]) -> Result<[u8; 32]> {
    let decoded = decode_verified(payload, Some(stored_hash))?;
    if decoded.source_version == PORTABLE_CHARACTER_FORMAT_VERSION {
        return Ok(decoded.source_hash);
    }
    content_hash(&decoded.model)
}

/// [`decode`] that also reports the stored version and the stored hash.
pub fn decode_verified(payload: &[u8], expected_hash: Option<&[u8; 32]>) -> Result<Decoded> {
    if payload.len() > MAX_COMPRESSED_BYTES {
        return Err(PortableError::LimitExceeded(format!("the compressed snapshot is larger than {MAX_COMPRESSED_BYTES} bytes")));
    }
    let decoder = zstd::stream::read::Decoder::new(payload).map_err(|e| PortableError::CorruptSnapshot(format!("not a compressed snapshot: {e}")))?;
    let mut json = Vec::new();
    // Read one byte more than allowed: if it is there, the payload would have inflated past the cap.
    decoder
        .take(MAX_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut json)
        .map_err(|e| PortableError::CorruptSnapshot(format!("decompression failed: {e}")))?;
    if json.len() > MAX_SNAPSHOT_BYTES {
        return Err(PortableError::LimitExceeded(format!("the snapshot inflates beyond {MAX_SNAPSHOT_BYTES} bytes")));
    }
    // the hash of the payload as it was written is verified BEFORE anything is migrated or interpreted
    let source_hash: [u8; 32] = Sha256::digest(&json).into();
    if let Some(expected) = expected_hash {
        if &source_hash != expected {
            return Err(PortableError::CorruptSnapshot("content hash mismatch".into()));
        }
    }
    let (model, source_version) = decode_json_versioned(&json)?;
    Ok(Decoded { model, source_version, source_hash })
}

/// Decode canonical JSON (already decompressed and, if needed, hash-checked).
pub fn decode_json(json: &[u8]) -> Result<PortableCharacter> {
    decode_json_versioned(json).map(|(model, _)| model)
}

/// Decode canonical JSON of any readable version into the current model, and say which version it was.
///
/// The version is read and checked before the shape. A newer one is refused. Version 1 is migrated strictly: it is the current
/// shape without `wardrobe`, so a version 1 payload that carries a `wardrobe` is not a version 1 payload and is refused, and a
/// genuine one becomes a character with an empty wardrobe. Every other field is still checked by `deny_unknown_fields`.
pub fn decode_json_versioned(json: &[u8]) -> Result<(PortableCharacter, u32)> {
    let mut value: serde_json::Value = serde_json::from_slice(json)?;
    let version = value
        .get("format_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| PortableError::CorruptSnapshot("format_version is missing".into()))?;
    if version > PORTABLE_CHARACTER_FORMAT_VERSION as u64 {
        return Err(PortableError::UnsupportedFormat { found: version.min(u32::MAX as u64) as u32, supported: PORTABLE_CHARACTER_FORMAT_VERSION });
    }
    if version < PORTABLE_CHARACTER_MIN_READ_VERSION as u64 {
        return Err(PortableError::UnsupportedFormat { found: version as u32, supported: PORTABLE_CHARACTER_FORMAT_VERSION });
    }
    if version == 1 {
        let object = value.as_object_mut().ok_or_else(|| PortableError::CorruptSnapshot("a snapshot is not an object".into()))?;
        if object.contains_key("wardrobe") {
            return Err(PortableError::CorruptSnapshot("a format 1 snapshot cannot contain a wardrobe (it was added in format 2)".into()));
        }
        object.insert("format_version".into(), serde_json::json!(PORTABLE_CHARACTER_FORMAT_VERSION));
    }
    let model: PortableCharacter = serde_json::from_value(value)?;
    model.validate()?;
    Ok((model, version as u32))
}

/// What format 1 wrote for a character that has no wardrobe (the one thing the migration needs to read): the same canonical JSON with
/// `format_version` 1. Used by the tests of the migration and of the stores that still hold such snapshots.
#[cfg(test)]
pub(crate) fn encode_as_format_1(model: &PortableCharacter) -> EncodedSnapshot {
    assert!(model.wardrobe.is_empty(), "format 1 has no wardrobe");
    let mut value: serde_json::Value = serde_json::from_slice(&canonical_json(model).unwrap()).unwrap();
    value["format_version"] = serde_json::json!(1);
    let json = serde_json::to_vec(&value).unwrap();
    EncodedSnapshot { content_hash: Sha256::digest(&json).into(), uncompressed_size: json.len() as u64, payload: zstd::stream::encode_all(json.as_slice(), ZSTD_LEVEL).unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::fixtures;

    #[test]
    fn roundtrip_is_structurally_equal() {
        for model in [fixtures::naked_level_one(), fixtures::geared_level_eighty()] {
            let model = model.normalized();
            let encoded = encode(&model).unwrap();
            let back = decode(&encoded.payload, Some(&encoded.content_hash)).unwrap();
            assert_eq!(back, model);
            assert_eq!(content_hash(&back).unwrap(), encoded.content_hash);
        }
    }

    #[test]
    fn unsorted_input_is_canonicalised_before_hashing() {
        let mut shuffled = fixtures::geared_level_eighty();
        shuffled.build.spells.reverse();
        shuffled.reputation.reverse();
        shuffled.items.reverse();
        let sorted = fixtures::geared_level_eighty();
        assert_eq!(content_hash(&shuffled).unwrap(), content_hash(&sorted).unwrap());
        assert_eq!(encode(&shuffled).unwrap().payload, encode(&sorted).unwrap().payload);
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = fixtures::geared_level_eighty();
        let mut b = a.clone();
        assert_eq!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
        b.progression.money += 1;
        assert_ne!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    #[test]
    fn unknown_extension_survives_byte_for_byte() {
        let mut model = fixtures::geared_level_eighty();
        let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        model.extensions.insert("mod:unknown-module".into(), crate::portable::model::Extension::new("9.9.9", 7, payload.clone()));
        let encoded = encode(&model).unwrap();
        let back = decode(&encoded.payload, Some(&encoded.content_hash)).unwrap();
        let ext = &back.extensions["mod:unknown-module"];
        assert_eq!(ext.payload.0, payload);
        assert_eq!((ext.module_version.as_str(), ext.format_version), ("9.9.9", 7));
    }

    #[test]
    fn corrupted_extension_is_refused() {
        let mut model = fixtures::geared_level_eighty();
        let mut ext = crate::portable::model::Extension::new("1", 1, vec![1, 2, 3]);
        ext.payload.0[0] ^= 0xFF;
        model.extensions.insert("mod:broken".into(), ext);
        assert!(matches!(encode(&model), Err(PortableError::CorruptSnapshot(_))));
    }

    #[test]
    fn wrong_hash_is_refused() {
        let encoded = encode(&fixtures::geared_level_eighty()).unwrap();
        let mut wrong = encoded.content_hash;
        wrong[0] ^= 1;
        assert!(matches!(decode(&encoded.payload, Some(&wrong)), Err(PortableError::CorruptSnapshot(_))));
    }

    #[test]
    fn garbage_and_truncation_are_refused() {
        assert!(decode(b"definitely not zstd", None).is_err());
        let encoded = encode(&fixtures::geared_level_eighty()).unwrap();
        assert!(decode(&encoded.payload[..encoded.payload.len() / 2], None).is_err());
    }

    #[test]
    fn decompression_bomb_is_stopped_at_the_cap() {
        // 20 MiB of spaces compress to a few KB; the decoder must refuse instead of inflating it.
        let bomb = zstd::stream::encode_all(vec![b' '; MAX_SNAPSHOT_BYTES + 4 * 1024 * 1024].as_slice(), 19).unwrap();
        assert!(bomb.len() < 4096, "the test needs a genuinely small payload, got {}", bomb.len());
        assert!(matches!(decode(&bomb, None), Err(PortableError::LimitExceeded(_))));
    }

    #[test]
    fn newer_format_is_refused_before_the_shape_is_checked() {
        let mut value = serde_json::to_value(fixtures::geared_level_eighty()).unwrap();
        value["format_version"] = serde_json::json!(PORTABLE_CHARACTER_FORMAT_VERSION + 1);
        value["a_field_of_the_future"] = serde_json::json!(true);
        let json = serde_json::to_vec(&value).unwrap();
        assert!(matches!(decode_json(&json), Err(PortableError::UnsupportedFormat { .. })));
    }

    #[test]
    fn unknown_fields_in_the_current_format_are_refused() {
        let mut value = serde_json::to_value(fixtures::geared_level_eighty()).unwrap();
        value["identity"]["secret_flag"] = serde_json::json!(1);
        assert!(decode_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn oversized_content_is_refused() {
        let mut model = fixtures::geared_level_eighty();
        model.build.spells = (1..=(MAX_SPELLS as u32 + 1)).map(|s| (s, 1)).collect();
        assert!(matches!(encode(&model), Err(PortableError::LimitExceeded(_))));
        let mut model = fixtures::geared_level_eighty();
        model.items[0].text = Some("x".repeat(MAX_ITEM_TEXT_BYTES + 1));
        assert!(matches!(encode(&model), Err(PortableError::LimitExceeded(_))));
        let mut model = fixtures::geared_level_eighty();
        model.progression.money = MAX_MONEY + 1;
        assert!(encode(&model).is_err());
    }

    #[test]
    fn broken_item_structure_is_refused() {
        // an item inside a container that does not exist
        let mut model = fixtures::geared_level_eighty();
        let orphan = model.items.iter().position(|i| i.container.is_some()).unwrap();
        model.items[orphan].container = Some(crate::portable::ids::PortableItemId::new());
        assert!(encode(&model).is_err());
        // two items in the same place
        let mut model = fixtures::geared_level_eighty();
        let mut copy = model.items[0].clone();
        copy.id = crate::portable::ids::PortableItemId::new();
        model.items.push(copy);
        assert!(encode(&model).is_err());
        // duplicate item ids
        let mut model = fixtures::geared_level_eighty();
        let dup = model.items[0].clone();
        model.items.push(dup);
        assert!(encode(&model).is_err());
    }

    #[test]
    fn a_geared_snapshot_is_small() {
        let encoded = encode(&fixtures::geared_level_eighty()).unwrap();
        assert!(encoded.payload.len() < 64 * 1024, "{} bytes", encoded.payload.len());
    }

    #[test]
    fn a_genuine_format_1_snapshot_migrates_with_an_empty_wardrobe_and_its_original_hash_is_verified_first() {
        let model = fixtures::geared_level_eighty();
        let v1 = encode_as_format_1(&model);
        let decoded = decode_verified(&v1.payload, Some(&v1.content_hash)).unwrap();
        assert_eq!((decoded.source_version, decoded.source_hash), (1, v1.content_hash));
        assert_eq!(decoded.model.format_version, PORTABLE_CHARACTER_FORMAT_VERSION);
        assert!(decoded.model.wardrobe.is_empty());
        assert_eq!(decoded.model, model.clone().normalized(), "nothing but the version changed");
        assert_ne!(v1.content_hash, content_hash(&decoded.model).unwrap(), "the same character has another hash in format 2");
        assert_eq!(semantic_hash(&v1.payload, &v1.content_hash).unwrap(), content_hash(&decoded.model).unwrap());
        let v2 = encode(&model).unwrap();
        assert_eq!(semantic_hash(&v2.payload, &v2.content_hash).unwrap(), v2.content_hash);

        // the stored hash is checked against the payload as it was written, before the migration can hide a difference
        let mut wrong = v1.content_hash;
        wrong[0] ^= 1;
        assert!(matches!(decode(&v1.payload, Some(&wrong)), Err(PortableError::CorruptSnapshot(m)) if m.contains("hash")));
        // the migrated form's hash is not accepted for the old payload
        assert!(decode(&v1.payload, Some(&content_hash(&decoded.model).unwrap())).is_err());
    }

    #[test]
    fn a_format_1_payload_with_a_wardrobe_is_not_a_format_1_payload() {
        let mut model = fixtures::geared_level_eighty();
        model.wardrobe.active.insert(1, 100);
        let mut value: serde_json::Value = serde_json::from_slice(&canonical_json(&model).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(1);
        let json = serde_json::to_vec(&value).unwrap();
        let payload = zstd::stream::encode_all(json.as_slice(), ZSTD_LEVEL).unwrap();
        let hash: [u8; 32] = Sha256::digest(&json).into();
        // even with a correct hash, an old version cannot carry what the old version did not have
        assert!(matches!(decode(&payload, Some(&hash)), Err(PortableError::CorruptSnapshot(m)) if m.contains("wardrobe")));
        assert!(decode_json(&json).is_err());
        // an empty wardrobe object written by hand is still a wardrobe in a version that had none
        let mut value: serde_json::Value = serde_json::from_slice(&canonical_json(&fixtures::geared_level_eighty()).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(1);
        value["wardrobe"] = serde_json::json!({});
        assert!(decode_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn format_1_keeps_every_other_strictness_and_version_0_is_not_a_version() {
        let mut value: serde_json::Value = serde_json::from_slice(&canonical_json(&fixtures::geared_level_eighty()).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(1);
        value["identity"]["secret_flag"] = serde_json::json!(1);
        assert!(decode_json(&serde_json::to_vec(&value).unwrap()).is_err(), "unknown fields are refused in the old format too");
        let mut value: serde_json::Value = serde_json::from_slice(&canonical_json(&fixtures::geared_level_eighty()).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(0);
        assert!(matches!(decode_json(&serde_json::to_vec(&value).unwrap()), Err(PortableError::UnsupportedFormat { .. })));
        value["format_version"] = serde_json::json!("1");
        assert!(decode_json(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
