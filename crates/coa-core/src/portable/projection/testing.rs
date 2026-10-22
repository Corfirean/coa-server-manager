//! Shared fixtures of the projection tests: a level-80 character with build records, and the decision a core at cap 60 would make for it.

use super::*;
use crate::portable::capabilities::Progression;
use crate::portable::fixtures::geared_level_eighty;
use crate::portable::model::*;

pub(crate) const SIGNATURE: &str =
    "bd299b9ef5863c6bf006e88a3617fac57c514b5889ea8367f87879aa1ad6d76d";

pub(crate) fn progression(cap: u32) -> Progression {
    Progression {
        max_player_level: cap,
        projection_protocol: PROTOCOL,
        projection_policy_version: POLICY_VERSION,
        progression_signature: SIGNATURE.into(),
        scaling_enabled: true,
    }
}

/// A level-80 character with a build record shaped like the core's: entries 100 (rank 1), 200 (rank 2), 300 (rank 1), buttons 0 and 1.
pub(crate) fn canonical() -> PortableCharacter {
    let mut c = geared_level_eighty();
    c.settings.insert(
        "core.ascension_slot.0".into(),
        vec![
            1, 21, 61, 3, 100, 1, 200, 2, 300, 1, 2, 0, 500_003, 1, 500_006,
        ],
    );
    c.settings
        .insert("core.ascension_build.61".into(), vec![3, 1001, 2002, 3001]);
    c.settings.insert(
        "core.ascension_bar.61".into(),
        vec![2, 0, 500_003, 1, 500_006],
    );
    c.settings
        .insert("core.ascension_slot.active".into(), vec![0]);
    c.normalized()
}

/// The decision a core at cap 60 would make for `canonical()`: the helmet (slot 0) and the first bag (with its contents) cannot be worn,
/// two abilities are not granted at 60, entries 200 and 300 are held, and the buttons showing the ability 500_006.
pub(crate) fn hold_for(c: &PortableCharacter) -> ProjectionHold {
    let helmet = c
        .items
        .iter()
        .find(|i| i.container.is_none() && i.slot == 0)
        .unwrap()
        .id;
    let bag = c
        .items
        .iter()
        .find(|i| i.container.is_none() && i.slot == 19)
        .unwrap()
        .id;
    ProjectionHold {
        protocol: PROTOCOL,
        policy_version: POLICY_VERSION,
        progression_signature: SIGNATURE.into(),
        max_player_level: 60,
        canonical_level: 80,
        projected_level: 60,
        held_items: vec![helmet, bag],
        held_spells: vec![500_006, 500_009],
        held_actions: vec![(0, 1)],
        settings: vec![
            SettingHold {
                source: "core.ascension_slot.0".into(),
                kind: settings::SettingKind::Slot,
                entries: vec![200, 300],
                buttons: vec![1],
            },
            SettingHold {
                source: "core.ascension_build.61".into(),
                kind: settings::SettingKind::Build,
                entries: vec![200, 300],
                buttons: vec![],
            },
            SettingHold {
                source: "core.ascension_bar.61".into(),
                kind: settings::SettingKind::Bar,
                entries: vec![],
                buttons: vec![1],
            },
        ],
        blocked_settings: vec![],
        subject: subject_of(c).unwrap(),
    }
}
