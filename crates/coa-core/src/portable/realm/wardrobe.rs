//! Writing a character's selected appearances (`PortableCharacter::wardrobe`) to a realm: shared by the importer (a new
//! character) and the in-place update (a delta). The three tables belong to the appearance module, so they are written
//! only where all three exist, and only what the realm **knows** (see [`super::knowledge`]).

use super::super::error::Result;
use super::super::model::PortableAppearance;
use super::knowledge::RealmKnowledge;
use super::script::SchemaProbe;
use super::sqlenc::{Insert, Val};

pub const TABLES: [&str; 3] = [
    "character_appearance",
    "character_appearance_settings",
    "character_appearance_outfit",
];

pub fn tables_present(probe: &SchemaProbe) -> bool {
    TABLES.iter().all(|t| probe.has(t))
}

/// The part of `desired` this realm can hold, or `None` when it cannot hold any (no knowledge of its client data, or no
/// appearance tables). Whatever the realm already shows (`present`) is always kept writable: the realm itself put it there.
pub fn writable(
    desired: &PortableAppearance,
    present: Option<&PortableAppearance>,
    knowledge: Option<&RealmKnowledge>,
    probe: &SchemaProbe,
) -> Option<PortableAppearance> {
    let knowledge = knowledge?;
    if !tables_present(probe) {
        return None;
    }
    let held = present.map(PortableAppearance::ids).unwrap_or_default();
    Some(desired.restricted_to(|id| knowledge.knows_appearance(id) || held.contains(&id)))
}

/// The rows of `w` for the character `@char`, in the realm's own formats: a missing settings row means "both visible", and an
/// outfit is a space separated list.
pub fn inserts(w: &PortableAppearance) -> Result<[Insert; 3]> {
    let mut active = Insert::new(
        "character_appearance",
        &["guid", "category_id", "appearance_id"],
    );
    for (category, appearance) in &w.active {
        active.row(vec![
            Val::Expr("@char"),
            Val::u(*category),
            Val::u(*appearance),
        ])?;
    }
    let mut settings = Insert::new(
        "character_appearance_settings",
        &["guid", "can_see_item", "can_see_spell"],
    );
    if !(w.can_see_item && w.can_see_spell) {
        settings.row(vec![
            Val::Expr("@char"),
            Val::u(w.can_see_item as u8),
            Val::u(w.can_see_spell as u8),
        ])?;
    }
    let mut outfits = Insert::new(
        "character_appearance_outfit",
        &["guid", "name", "appearances"],
    );
    for (name, ids) in &w.outfits {
        let text = ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
        outfits.row(vec![
            Val::Expr("@char"),
            Val::text(name.clone()),
            Val::text(text),
        ])?;
    }
    Ok([active, settings, outfits])
}

/// How many rows [`inserts`] writes.
pub fn row_count(w: &PortableAppearance) -> usize {
    w.active.len() + w.outfits.len() + usize::from(!(w.can_see_item && w.can_see_spell))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::collection::IdSet;

    fn probe(all: bool) -> SchemaProbe {
        SchemaProbe {
            tables: TABLES
                .iter()
                .take(if all { 3 } else { 2 })
                .map(|t| t.to_string())
                .collect(),
            character_columns: Default::default(),
        }
    }

    fn sample() -> PortableAppearance {
        let mut w = PortableAppearance::default();
        w.active.insert(1, 100);
        w.active.insert(3, 300);
        w.outfits.insert("Sunday".into(), vec![100, 0, 300]);
        w.outfits.insert("Mixed".into(), vec![100, 999]);
        w
    }

    #[test]
    fn only_known_ids_are_written_and_what_the_realm_already_shows_is_kept() {
        let k = RealmKnowledge::new(IdSet::from_ids([100, 300]).unwrap(), IdSet::new());
        let w = writable(&sample(), None, Some(&k), &probe(true)).unwrap();
        assert_eq!(w.active.len(), 2);
        assert!(
            w.outfits.contains_key("Sunday") && !w.outfits.contains_key("Mixed"),
            "an outfit that mentions an unknown id is held back whole"
        );

        // the realm shows 999 itself (a synthesised appearance): it stays writable
        let mut present = PortableAppearance::default();
        present.outfits.insert("Mixed".into(), vec![100, 999]);
        let w = writable(&sample(), Some(&present), Some(&k), &probe(true)).unwrap();
        assert!(w.outfits.contains_key("Mixed"));

        assert!(
            writable(&sample(), None, None, &probe(true)).is_none(),
            "no knowledge of the client data: nothing is written"
        );
        assert!(
            writable(&sample(), None, Some(&k), &probe(false)).is_none(),
            "no appearance tables: nothing is written"
        );
    }

    #[test]
    fn rows_use_the_realm_formats_and_hostile_names_are_hex() {
        let mut w = sample();
        w.can_see_spell = false;
        w.outfits
            .insert("x'); DROP TABLE characters; --".into(), vec![1]);
        let [a, s, o] = inserts(&w).unwrap();
        assert_eq!(a.len() + s.len() + o.len(), row_count(&w));
        let sql = o.sql().unwrap();
        assert!(
            !sql.contains("DROP") && sql.contains(&hex::encode("100 0 300")),
            "{sql}"
        );
        assert_eq!(s.len(), 1);
        let default = inserts(&PortableAppearance::default()).unwrap();
        assert!(
            default.iter().all(Insert::is_empty),
            "the defaults are the absence of rows, like the realm's own"
        );
    }
}
