//! Update-only comparison of existing unsupported character starts. Never repairs or deletes rows.
use crate::{db::Db, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MissingStart {
    pub guid: u32,
    pub race: u32,
    pub class: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub schema: u32,
    pub realms: BTreeMap<String, BTreeSet<MissingStart>>,
}

pub fn parse_missing(text: &str) -> Result<BTreeSet<MissingStart>> {
    text.lines()
        .map(|line| {
            let values = line
                .split('\t')
                .map(str::parse::<u32>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| {
                    Error::Invalid("Cannot read existing character start identities.".into())
                })?;
            if values.len() != 3 {
                return Err(Error::Invalid(
                    "Incomplete character start identity.".into(),
                ));
            }
            Ok(MissingStart {
                guid: values[0],
                race: values[1],
                class: values[2],
            })
        })
        .collect()
}

pub fn missing(db: &Db) -> Result<BTreeSet<MissingStart>> {
    parse_missing(&db.query("SELECT c.guid,c.race,c.class FROM acore_characters.characters c LEFT JOIN acore_world.playercreateinfo p ON p.race=c.race AND p.class=c.class WHERE p.race IS NULL;")?)
}

pub fn capture(db: &Db, root: &Path) -> Result<Baseline> {
    let mut realms = BTreeMap::new();
    for mode in crate::realms::state(root)?.modes() {
        realms.insert(
            mode.name().to_owned(),
            missing(&db.clone().for_realm(mode))?,
        );
    }
    Ok(Baseline { schema: 1, realms })
}

/// Match identity and race/class, never just totals: one new failure cannot replace an old one.
pub fn introduced(
    current: &BTreeSet<MissingStart>,
    before: &BTreeSet<MissingStart>,
) -> BTreeSet<MissingStart> {
    current.difference(before).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_gaps_are_not_new_character_loss() {
        let before = parse_missing("1\t44\t14\n2\t44\t14\n3\t44\t14").unwrap();
        assert!(introduced(&before, &before).is_empty());
        assert!(introduced(&BTreeSet::new(), &before).is_empty());
        let replaced = parse_missing("1\t44\t14\n2\t44\t14\n4\t44\t14").unwrap();
        assert_eq!(
            introduced(&replaced, &before),
            parse_missing("4\t44\t14").unwrap()
        );
        let changed = parse_missing("1\t44\t15").unwrap();
        assert_eq!(introduced(&changed, &before), changed);
        assert_eq!(introduced(&before, &BTreeSet::new()), before);
    }
    #[test]
    fn malformed_identity_is_not_silently_ignored() {
        for text in ["1\t44", "bad\t44\t14", "1\t44\t14\t0", "1\t44\t-1"] {
            assert!(parse_missing(text).is_err());
        }
        assert!(parse_missing("").unwrap().is_empty());
    }

    #[test]
    fn reported_174_old_records_cannot_mask_one_newly_hidden_character() {
        // Reproduce the report's scale without shipping another person's diagnostics or identities.
        let before: BTreeSet<_> = (1..=174)
            .map(|guid| MissingStart {
                guid,
                race: 44,
                class: 14,
            })
            .collect();
        assert!(introduced(&before, &before).is_empty());
        let mut after = before.clone();
        after.remove(&MissingStart {
            guid: 1,
            race: 44,
            class: 14,
        });
        after.insert(MissingStart {
            guid: 175,
            race: 44,
            class: 14,
        });
        assert_eq!(after.len(), before.len());
        assert_eq!(
            introduced(&after, &before),
            BTreeSet::from([MissingStart {
                guid: 175,
                race: 44,
                class: 14
            }])
        );
    }
}
