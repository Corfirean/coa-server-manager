//! Which `character_settings` sources travel with a character (decision D2, tightened in Phase 2.1).
//!
//! `character_settings` is a free-form `(guid, source) -> integers` store used by the core and by modules; CoA keeps its
//! specialisation, talent builds and per-spec action bars there. Only the **minimal build / spec / starter / reset
//! state** is carried, by **strict key patterns** (no broad prefixes):
//!
//! * `Carry`      applied at the destination: exactly the keys in [`CARRY`];
//! * `Drop`       never carried (bots, tests, high-risk bookkeeping, this importer's own marker);
//! * `Quarantine` everything else: kept in the canonical snapshot as an opaque extension, **never applied** at a realm
//!                (spell charges, Destiny Weaver, dynamic XP, Runemaster, prestige/glory, tutorial flags, Wildcard
//!                state and anything unknown).
//!
//! In a pattern `#` stands for one run of 1-9 ASCII digits and every other character must match exactly, so
//! `core.ascension_build.54` is carried while `core.ascension_build.54x`, `core.ascension_build.`,
//! `core.ascension_build.54.extra` and `core.ascension_build.-1` are not.

use super::super::model::Ruleset;

/// Bump when the lists change.
pub const SETTINGS_POLICY_VERSION: u32 = 2;

/// Extension namespace holding the quarantined sources.
pub const QUARANTINE_EXTENSION: &str = "coa:unlisted-settings";

/// The source the offline importer writes to mark "this character was imported by import N": see `import.rs`.
pub const IMPORT_MARKER_SOURCE: &str = "coa.portable.import";

/// Written next to the marker by an in-place update: the first item guid and pet number the update allocated (recovery needs
/// them when the realm's answer was lost).
pub const ALLOC_MARKER_SOURCE: &str = "coa.portable.alloc";

/// The progression a session character was prepared for (policy version, level cap, signature words): the core refuses to let a session
/// character in when its own progression is another one.
pub const PIN_SOURCE: &str = "coa.portable.pin";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Carry,
    Drop,
    Quarantine,
}

/// The complete list of carried keys. `#` = digits.
pub const CARRY: &[&str] = &[
    "core.ascension_active_spec",
    "core.ascension_starter",
    "core.ascension_starter_live",
    "core.ascension_reset_credits",
    "core.ascension_slot.active",
    "core.ascension_slot.#",
    "core.ascension_slot.#.build.#",
    "core.ascension_slot.#.bar.#",
    "core.ascension_build.#",
    "core.ascension_bar.#",
];

const DROP_PREFIX: &[&str] = &["coa.bot", "coa.gameplay_test", "coa.highrisk", "coa.portable."];

/// Does `source` match `pattern`? `#` = 1-9 ASCII digits, anything else literal.
pub fn matches_pattern(source: &str, pattern: &str) -> bool {
    fn go(s: &[u8], p: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'#', rest)) => {
                let digits = s.iter().take_while(|b| b.is_ascii_digit()).count().min(9);
                // the digit run must be maximal: a digit right after it would make it longer than 9 digits
                if digits == 0 || s.get(digits).is_some_and(|b| b.is_ascii_digit()) {
                    return false;
                }
                go(&s[digits..], rest)
            }
            Some((c, rest)) => s.first() == Some(c) && go(&s[1..], rest),
        }
    }
    go(source.as_bytes(), pattern.as_bytes())
}

/// The ruleset is accepted for forward compatibility: today every ruleset uses the same minimal list and Wildcard
/// state is quarantined until its keys have been reviewed one by one.
pub fn classify_setting(source: &str, _ruleset: Ruleset) -> Disposition {
    if DROP_PREFIX.iter().any(|p| source.starts_with(p)) {
        return Disposition::Drop;
    }
    if CARRY.iter().any(|p| matches_pattern(source, p)) {
        return Disposition::Carry;
    }
    Disposition::Quarantine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_the_minimal_build_spec_starter_and_reset_state_is_carried() {
        for s in [
            "core.ascension_active_spec",
            "core.ascension_starter",
            "core.ascension_starter_live",
            "core.ascension_reset_credits",
            "core.ascension_slot.active",
            "core.ascension_slot.2",
            "core.ascension_slot.12",
            "core.ascension_slot.2.build.54",
            "core.ascension_slot.2.bar.54",
            "core.ascension_build.54",
            "core.ascension_bar.54",
            "core.ascension_build.0",
        ] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Carry, "{s}");
        }
    }

    #[test]
    fn nonessential_state_is_quarantined_not_applied() {
        for s in [
            "core.spell_charge.804197",
            "core.spell_charge.712389",
            "core.destiny_weaver",
            "core.dynamic_xp.preset",
            "core.runemaster.echoes",
            "core.coa_prestige",
            "core.coa_prestige_bar",
            "core.coa_glory",
            "core.ascension.tutorial_verified",
            "core.ascension.tutorial_tracking",
            "core.ascension",
            "coa.gear_pref_weapon",
            "coa.gear_pref_armor",
            "mod.someone.new_feature",
            "core.unheard_of",
        ] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Quarantine, "{s}");
        }
    }

    #[test]
    fn keys_are_matched_strictly_not_by_prefix() {
        for s in [
            "core.ascension_build.",
            "core.ascension_build.54x",
            "core.ascension_build.x54",
            "core.ascension_build.54.extra",
            "core.ascension_build.-1",
            "core.ascension_build.+1",
            "core.ascension_build.5 4",
            "core.ascension_build.1234567890",
            "core.ascension_slot.",
            "core.ascension_slot.active.x",
            "core.ascension_slot.2.build.",
            "core.ascension_slot.2.build.54.1",
            "core.ascension_slot.2.talent.54",
            "core.ascension_slot..build.54",
            "core.ascension_active_spec.1",
            "core.ascension_active_spec ",
            "core.ascension_bar",
            "Core.Ascension_Active_Spec",
            "core.ascension_starter_liveX",
            "core.ascension_resetcredits",
        ] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Quarantine, "{s:?} must not be carried");
        }
    }

    #[test]
    fn the_pattern_matcher_itself() {
        assert!(matches_pattern("a.5", "a.#"));
        assert!(matches_pattern("a.123456789", "a.#"));
        assert!(!matches_pattern("a.1234567890", "a.#"), "more than nine digits");
        assert!(!matches_pattern("a.", "a.#"));
        assert!(matches_pattern("a.1.b.22", "a.#.b.#"));
        assert!(!matches_pattern("a.1.b.22.", "a.#.b.#"));
        assert!(!matches_pattern("a.1b", "a.#.b"));
        assert!(matches_pattern("", ""));
        assert!(!matches_pattern("x", ""));
    }

    #[test]
    fn bots_tests_bookkeeping_and_the_importers_own_marker_are_dropped() {
        for s in ["coa.bot.gear", "coa.bot_profile", "coa.gameplay_test", "coa.highrisk", "coa.portable.import"] {
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Drop, "{s}");
        }
    }

    #[test]
    fn wildcard_state_is_quarantined_on_every_ruleset_until_reviewed() {
        for s in ["core.wildcard", "core.wildcard.cards", "core.wildcard.spec", "core.wildcard.scrolls.spec2"] {
            assert_eq!(classify_setting(s, Ruleset::Wildcard), Disposition::Quarantine, "{s}");
            assert_eq!(classify_setting(s, Ruleset::Coa), Disposition::Quarantine, "{s}");
        }
        // the carried keys are the same on a Wildcard realm
        assert_eq!(classify_setting("core.ascension_active_spec", Ruleset::Wildcard), Disposition::Carry);
    }

    #[test]
    fn the_carry_list_is_exactly_what_was_approved() {
        assert_eq!(CARRY.len(), 10);
        assert!(CARRY.iter().all(|p| p.starts_with("core.ascension_")), "nothing outside the ascension build/spec/starter/reset family");
    }
}
