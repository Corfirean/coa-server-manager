//! Account-wide collections (wardrobe, vanity, mounts, pets, ...): sets of numeric ids that only ever **grow**.
//!
//! * `canonical = canonical U incoming`: an id is never removed because another realm did not return it.
//! * Compact form: sorted ids, delta + LEB128 varint, ~1-2 bytes per id; never a verbose JSON array.
//! * `collection_hash` makes "did anything change?" a 32-byte comparison; when it is equal nothing is transferred.

use sha2::{Digest, Sha256};

use super::error::{PortableError, Result};
use super::versions::PORTABLE_COLLECTION_FORMAT_VERSION;

/// Largest number of ids in one collection (a realm's whole wardrobe is a few thousand).
pub const MAX_SET_IDS: usize = 2_000_000;
/// Largest encoded size accepted when decoding.
pub const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;

/// Collection kinds are namespaced names such as `coa:wardrobe`, `coa:vanity`, `mod:foo:mounts`.
pub fn valid_kind(kind: &str) -> bool {
    super::ids::valid_namespace(kind) && kind.contains(':')
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct IdSet(Vec<u32>);

impl IdSet {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Build from any ids: sorted and de-duplicated.
    pub fn from_ids(ids: impl IntoIterator<Item = u32>) -> Result<Self> {
        let mut v: Vec<u32> = ids.into_iter().collect();
        v.sort_unstable();
        v.dedup();
        if v.len() > MAX_SET_IDS {
            return Err(PortableError::LimitExceeded(format!(
                "a collection cannot hold more than {MAX_SET_IDS} ids"
            )));
        }
        Ok(Self(v))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn contains(&self, id: u32) -> bool {
        self.0.binary_search(&id).is_ok()
    }
    pub fn ids(&self) -> &[u32] {
        &self.0
    }

    /// `self U other`.
    pub fn union(&self, other: &IdSet) -> IdSet {
        let (a, b) = (&self.0, &other.0);
        let mut out = Vec::with_capacity(a.len() + b.len());
        let (mut i, mut j) = (0, 0);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Less => {
                    out.push(a[i]);
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    out.push(b[j]);
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    out.push(a[i]);
                    i += 1;
                    j += 1;
                }
            }
        }
        out.extend_from_slice(&a[i..]);
        out.extend_from_slice(&b[j..]);
        IdSet(out)
    }

    /// How many ids of `other` are not in `self`.
    pub fn count_new(&self, other: &IdSet) -> usize {
        other.0.iter().filter(|id| !self.contains(**id)).count()
    }

    /// `[format version][count varint][first id varint][gap varint ...]` where every gap is the difference to the
    /// previous id (always at least 1).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.0.len() * 2);
        out.push(PORTABLE_COLLECTION_FORMAT_VERSION as u8);
        put_varint(&mut out, self.0.len() as u64);
        let mut previous = 0u32;
        for (index, id) in self.0.iter().enumerate() {
            put_varint(
                &mut out,
                if index == 0 {
                    *id as u64
                } else {
                    (*id - previous) as u64
                },
            );
            previous = *id;
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<IdSet> {
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err(PortableError::LimitExceeded(
                "an encoded collection is too large".into(),
            ));
        }
        let bad = |what: &str| PortableError::CorruptSnapshot(format!("collection: {what}"));
        let (&version, mut rest) = bytes.split_first().ok_or_else(|| bad("empty"))?;
        if version as u32 > PORTABLE_COLLECTION_FORMAT_VERSION {
            return Err(PortableError::UnsupportedFormat {
                found: version as u32,
                supported: PORTABLE_COLLECTION_FORMAT_VERSION,
            });
        }
        if version == 0 {
            return Err(bad("version 0"));
        }
        let count = take_varint(&mut rest).ok_or_else(|| bad("truncated count"))?;
        if count > MAX_SET_IDS as u64 {
            return Err(PortableError::LimitExceeded(format!(
                "a collection cannot hold more than {MAX_SET_IDS} ids"
            )));
        }
        let mut ids = Vec::with_capacity(count as usize);
        let mut previous = 0u64;
        for index in 0..count {
            let step = take_varint(&mut rest).ok_or_else(|| bad("truncated ids"))?;
            let id = if index == 0 {
                step
            } else {
                if step == 0 {
                    return Err(bad("ids are not strictly ascending"));
                }
                previous.checked_add(step).ok_or_else(|| bad("overflow"))?
            };
            if id > u32::MAX as u64 {
                return Err(bad("id out of range"));
            }
            ids.push(id as u32);
            previous = id;
        }
        if !rest.is_empty() {
            return Err(bad("trailing bytes"));
        }
        Ok(IdSet(ids))
    }

    /// Hash of one collection: binds the kind and the exact content.
    pub fn hash(&self, kind: &str) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"coa-collection-v1\0");
        h.update(kind.as_bytes());
        h.update([0]);
        h.update(self.encode());
        h.finalize().into()
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn take_varint(rest: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, tail) = rest.split_first()?;
        *rest = tail;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random_ids(n: usize, spread: u32) -> Vec<u32> {
        // deterministic xorshift, no dependency
        let mut x = 0x9E37_79B9u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x % spread
            })
            .collect()
    }

    #[test]
    fn roundtrip_and_size_at_realistic_and_extreme_sizes() {
        for (count, spread) in [
            (0usize, 1u32),
            (1, 10),
            (2_000, 40_000),
            (10_000, 200_000),
            (50_000, 1_000_000),
        ] {
            let set = IdSet::from_ids(pseudo_random_ids(count, spread)).unwrap();
            let encoded = set.encode();
            assert_eq!(IdSet::decode(&encoded).unwrap(), set);
            // a compact binary form: well under 3 bytes per id even for scattered ids, never a JSON array
            assert!(
                encoded.len() <= 8 + set.len() * 3,
                "{} ids -> {} bytes",
                set.len(),
                encoded.len()
            );
        }
    }

    #[test]
    fn union_never_removes_and_is_idempotent() {
        let a = IdSet::from_ids([5, 1, 9, 9]).unwrap();
        let b = IdSet::from_ids([2, 9, 11]).unwrap();
        let u = a.union(&b);
        assert_eq!(u.ids(), &[1, 2, 5, 9, 11]);
        assert!(
            a.ids().iter().all(|id| u.contains(*id)),
            "nothing of the old set may disappear"
        );
        assert_eq!(u.union(&b), u);
        assert_eq!(u.union(&IdSet::new()), u);
        assert_eq!(a.count_new(&b), 2);
        assert_eq!(u.count_new(&a), 0);
    }

    #[test]
    fn hash_changes_with_content_and_kind() {
        let a = IdSet::from_ids([1, 2, 3]).unwrap();
        let b = IdSet::from_ids([1, 2, 4]).unwrap();
        assert_eq!(
            a.hash("coa:wardrobe"),
            IdSet::from_ids([3, 2, 1]).unwrap().hash("coa:wardrobe")
        );
        assert_ne!(a.hash("coa:wardrobe"), b.hash("coa:wardrobe"));
        assert_ne!(a.hash("coa:wardrobe"), a.hash("coa:vanity"));
    }

    #[test]
    fn malformed_encodings_are_refused() {
        let good = IdSet::from_ids([10, 20, 30]).unwrap().encode();
        assert!(IdSet::decode(&[]).is_err());
        assert!(IdSet::decode(&good[..good.len() - 1]).is_err(), "truncated");
        let mut trailing = good.clone();
        trailing.push(0);
        assert!(IdSet::decode(&trailing).is_err());
        assert!(
            IdSet::decode(&[1, 3, 10, 0, 5]).is_err(),
            "a zero gap means a duplicate"
        );
        assert!(IdSet::decode(&[9, 0]).is_err(), "newer format");
        assert!(IdSet::decode(&[0, 0]).is_err(), "version zero");
        // count far beyond the limit
        let mut huge = vec![1];
        put_varint(&mut huge, MAX_SET_IDS as u64 + 1);
        assert!(matches!(
            IdSet::decode(&huge),
            Err(PortableError::LimitExceeded(_))
        ));
        // an id above u32
        let mut big = vec![1, 1];
        put_varint(&mut big, u32::MAX as u64 + 1);
        assert!(IdSet::decode(&big).is_err());
        // an over-long varint must not loop or overflow
        assert!(IdSet::decode(&[
            1, 1, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80
        ])
        .is_err());
    }

    #[test]
    fn kinds_are_namespaced() {
        assert!(valid_kind("coa:wardrobe"));
        assert!(valid_kind("mod:foo:mounts"));
        assert!(!valid_kind("wardrobe"));
        assert!(!valid_kind("Coa:Wardrobe"));
        assert!(!valid_kind(""));
    }
}
