//! Identifiers. Everything that must be globally unique is a UUIDv7 (time-ordered, generated locally, never an
//! auto-increment). Numeric ids of game content are never global: they live in a namespace.

use std::fmt;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use super::error::{PortableError, Result};

macro_rules! uuid7_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Uuid);

        impl $name {
            /// A new, time-ordered id.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wrap an existing UUID. Only version-7 ids are accepted: an id from outside this system that is not
            /// time-ordered is a sign of a foreign or forged source.
            pub fn from_uuid(uuid: Uuid) -> Result<Self> {
                if uuid.get_version_num() != 7 || uuid.get_variant() != uuid::Variant::RFC4122 {
                    return Err(PortableError::Invalid(format!("{} is not a UUIDv7", uuid)));
                }
                Ok(Self(uuid))
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.hyphenated())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.hyphenated())
            }
        }

        impl FromStr for $name {
            type Err = PortableError;
            fn from_str(s: &str) -> Result<Self> {
                let uuid = Uuid::parse_str(s).map_err(|_| PortableError::Invalid(format!("{s:?} is not a UUID")))?;
                Self::from_uuid(uuid)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                text.parse().map_err(de::Error::custom)
            }
        }
    };
}

uuid7_id!(
    /// The global identity of a portable character. Unlike the local AzerothCore guid it is the same on every realm.
    CharacterId
);
uuid7_id!(
    /// The owner's profile (their characters and collections).
    ProfileId
);
uuid7_id!(
    /// The global identity of one item of a portable character. Local item guids differ per realm; this does not.
    PortableItemId
);
uuid7_id!(
    /// The global identity of one pet of a portable character.
    PortablePetId
);
uuid7_id!(
    /// One import of a portable character into a realm (the key of the import journal).
    ImportId
);
uuid7_id!(
    /// One runtime session of a portable character on a realm: from its automatic baseline to its final checkpoint.
    SessionId
);

/// A reference to a piece of game content that cannot be mistaken for another namespace's numbering:
/// `core:wotlk:item:19019`, `coa:wardrobe:10581`, `mod:<module>:item:7`.
///
/// The textual form is `<namespace>:<kind>:<id>` where the namespace may itself contain colons; the last two
/// segments are always the kind and the numeric id.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId {
    namespace: String,
    kind: String,
    id: u64,
}

const MAX_SEGMENT: usize = 48;

fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SEGMENT
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' || b == b'.'
        })
}

/// A namespace is one or more segments joined by colons.
pub fn valid_namespace(ns: &str) -> bool {
    ns.len() <= 96 && ns.split(':').all(valid_segment)
}

impl ContentId {
    pub fn new(namespace: &str, kind: &str, id: u64) -> Result<Self> {
        if !valid_namespace(namespace) || !valid_segment(kind) {
            return Err(PortableError::Invalid(format!(
                "invalid content id {namespace}:{kind}:{id}"
            )));
        }
        Ok(Self {
            namespace: namespace.to_string(),
            kind: kind.to_string(),
            id,
        })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.namespace, self.kind, self.id)
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentId({self})")
    }
}

impl FromStr for ContentId {
    type Err = PortableError;
    fn from_str(s: &str) -> Result<Self> {
        let bad =
            || PortableError::Invalid(format!("{s:?} is not a content id (namespace:kind:id)"));
        let (rest, id) = s.rsplit_once(':').ok_or_else(bad)?;
        let (namespace, kind) = rest.rsplit_once(':').ok_or_else(bad)?;
        // `u64::from_str` accepts a leading '+', which would make two spellings of one id.
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let id: u64 = id.parse().map_err(|_| bad())?;
        Self::new(namespace, kind, id)
    }
}

impl Serialize for ContentId {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_uuid_v7_and_strictly_time_ordered() {
        let ids: Vec<CharacterId> = (0..2000).map(|_| CharacterId::new()).collect();
        assert!(ids.iter().all(|id| id.as_uuid().get_version_num() == 7));
        assert!(
            ids.windows(2).all(|w| w[0] < w[1]),
            "ids generated in sequence must sort in sequence"
        );
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn only_uuid_v7_is_accepted_from_text() {
        let good = CharacterId::new();
        assert_eq!(good.to_string().parse::<CharacterId>().unwrap(), good);
        assert!(
            Uuid::new_v4().to_string().parse::<CharacterId>().is_err(),
            "a v4 UUID is not a portable id"
        );
        assert!(Uuid::nil().to_string().parse::<CharacterId>().is_err());
        assert!("not-a-uuid".parse::<CharacterId>().is_err());
    }

    #[test]
    fn content_ids_keep_their_namespace() {
        for text in [
            "core:wotlk:item:19019",
            "coa:wardrobe:10581",
            "mod:craftsmans-codex:item:7",
            "coa:class:12",
        ] {
            let id: ContentId = text.parse().unwrap();
            assert_eq!(id.to_string(), text);
        }
        let id: ContentId = "core:wotlk:item:19019".parse().unwrap();
        assert_eq!(
            (id.namespace(), id.kind(), id.id()),
            ("core:wotlk", "item", 19019)
        );
        let other: ContentId = "mod:x:item:19019".parse().unwrap();
        assert_ne!(
            id, other,
            "the same number in another namespace is a different thing"
        );
    }

    #[test]
    fn malformed_content_ids_are_refused() {
        for text in [
            "",
            "item:1",
            "a:b",
            "a:b:c",
            "a:b:-1",
            "a:b:+1",
            "A:b:1",
            "a b:c:1",
            "a::1",
            ":b:1",
            "a:b:18446744073709551616",
        ] {
            assert!(
                text.parse::<ContentId>().is_err(),
                "{text:?} must not parse"
            );
        }
    }

    #[test]
    fn ids_serialise_as_plain_strings() {
        let id = CharacterId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<CharacterId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<CharacterId>(&format!("\"{}\"", Uuid::new_v4())).is_err());
    }
}
