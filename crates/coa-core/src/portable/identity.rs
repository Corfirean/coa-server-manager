//! Content identity of an item: what it *was* when it was mapped to a portable item id.
//!
//! Local item guids are recycled by the realm. A mapping is only trusted while the item behind the guid still has the
//! same identity. The identity covers the traits that cannot change during an item's life (entry, random property or
//! suffix, crafter); count, durability, enchantments and position change all the time and are deliberately left out.
//! Two different items that share all of these are interchangeable for mapping purposes.

use sha2::{Digest, Sha256};

use super::ids::ContentId;

const VERSION: &str = "v1";

pub fn item_identity(entry: &ContentId, random_property_id: i32, creator_name: Option<&str>) -> String {
    let mut h = Sha256::new();
    h.update(VERSION.as_bytes());
    h.update([0]);
    h.update(entry.to_string().as_bytes());
    h.update([0]);
    h.update(random_property_id.to_be_bytes());
    h.update([0]);
    h.update(creator_name.unwrap_or("").as_bytes());
    format!("{VERSION}:{}", hex::encode(&h.finalize()[..16]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> ContentId {
        ContentId::new("coa", "item", id).unwrap()
    }

    #[test]
    fn same_traits_same_identity_and_any_difference_changes_it() {
        let base = item_identity(&entry(100), -5, Some("Smith"));
        assert_eq!(base, item_identity(&entry(100), -5, Some("Smith")));
        assert_ne!(base, item_identity(&entry(101), -5, Some("Smith")), "another entry");
        assert_ne!(base, item_identity(&entry(100), -6, Some("Smith")), "another suffix");
        assert_ne!(base, item_identity(&entry(100), -5, Some("Other")), "another crafter");
        assert_ne!(base, item_identity(&entry(100), -5, None), "no crafter");
        assert!(base.starts_with("v1:") && base.len() == 3 + 32);
    }

    #[test]
    fn the_namespace_is_part_of_the_identity() {
        let a = ContentId::new("coa", "item", 7).unwrap();
        let b = ContentId::new("mod:x", "item", 7).unwrap();
        assert_ne!(item_identity(&a, 0, None), item_identity(&b, 0, None));
    }
}
