//! Which `character_settings` sources travel with a character (decision D2).
//!
//! `character_settings` is a free-form `(guid, source) -> integers` store used by the core and by modules; CoA keeps its
//! specialisation, talent builds and per-spec action bars there. The list is versioned data, not scattered `if`s:
//!
//! * `Carry`      imported at the destination (known gameplay state);
//! * `Drop`       never carried (bots, tests, high-risk bookkeeping);
//! * `Quarantine` kept in the canonical snapshot as an opaque extension but **not** imported anywhere; an unknown
//!                source is never destroyed and never applied blindly.

use super::super::model::Ruleset;

/// Bump when the lists change; recorded with the quarantined extension.
pub const SETTINGS_POLICY_VERSION: u32 = 1;

/// Extension namespace holding the quarantined sources.
pub const QUARANTINE_EXTENSION: &str = "coa:unlisted-settings";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Carry,
    Drop,
    Quarantine,
}

const CARRY_EXACT: &[&str] = &[
    "core.ascension",
    "core.ascension_active_spec",
    "core.ascension_starter",
    "core.ascension_starter_live",
    "core.ascension_reset_credits",
    "core.ascension_slot.active",
    "core.destiny_weaver",
    "core.dynamic_xp.preset",
    "core.runemaster.echoes",
    "core.coa_prestige",
    "core.coa_prestige_bar",
    "core.coa_glory",
];

const CARRY_PREFIX: &[&str] = &["core.ascension_slot.", "core.ascension_build.", "core.ascension_bar.", "core.spell_charge.", "core.ascension.tutorial_"];

const DROP_PREFIX: &[&str] = &["coa.bot", "coa.gameplay_test", "coa.highrisk"];

pub fn classify_setting(source: &str, ruleset: Ruleset) -> Disposition {
    if DROP_PREFIX.iter().any(|p| source.starts_with(p)) {
        return Disposition::Drop;
    }
    if source == "core.wildcard" || source.starts_with("core.wildcard.") {
        // Wildcard state only means something to the Wildcard ruleset.
        return if ruleset == Ruleset::Wildcard { Disposition::Carry } else { Disposition::Quarantine };
    }
    if CARRY_EXACT.contains(&source) || CARRY_PREFIX.iter().any(|p| source.starts_with(p)) {
        return Disposition::Carry;
    }
    Disposition::Quarantine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_coa_gameplay_state_is_carried() {
        for s in [
            "core.ascension_active_spec",
            "core.ascension_starter",
            "core.ascension_slot.active",
            "core.ascension_slot.2",
            "core.ascension_slot.2.build.54",
            "core.ascension_build.54",
            "core.ascension_bar.54",
            "core.spell_charge.804197",
            "core.ascension.tutorial_verified",
            "core.destiny_weaver",
        ] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Carry, "{s}");
        }
    }

    #[test]
    fn bots_tests_and_bookkeeping_are_dropped() {
        for s in ["coa.bot.gear", "coa.bot_profile", "coa.gameplay_test", "coa.highrisk"] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Drop, "{s}");
        }
    }

    #[test]
    fn unknown_sources_are_quarantined_never_dropped() {
        for s in ["mod.someone.new_feature", "core.unheard_of", "coa.gear_pref_weapon", "core.ascensionx"] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Quarantine, "{s}");
        }
    }

    #[test]
    fn wildcard_state_belongs_to_the_wildcard_ruleset() {
        for s in ["core.wildcard", "core.wildcard.cards", "core.wildcard.scrolls.spec2"] {
            assert_eq!(classify_setting(s, Ruleset::Wildcard), Disposition::Carry, "{s}");
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Quarantine, "{s}");
        }
    }
}
