//! The table registry: every table of the characters database is classified, so that a new per-character table can
//! never be silently left out of an export (audit risk R5). The classification below is the audit's section 3 (163
//! tables of the `coa-schema-fixture-20261005` schema) with the owner's decisions applied.
//!
//! * `Portable`        exported in v1;
//! * `Deferred`        portable later by decision (equipment sets, per-character wardrobe, personal bank, codex);
//! * `Appearance`      the character's selected appearances, outfits and visibility switches (Phase 6, carried in
//!                     `PortableCharacter::wardrobe`; the tables belong to an optional module);
//! * `Collection`      account-wide permanent unlocks (Phase 6, a profile collection, not part of the character);
//! * `AccountLocal`    account / realm-account state, never carried;
//! * `CharacterLocal`  per-character state that is deliberately not portable (cooldowns, auras, mail links, ...);
//! * `Blocking`        per-character state of an active challenge / game mode / Manastorm: while it exists the
//!                     character may not be made portable (decision D6);
//! * `Realm`           realm operations (guilds, mail, auctions, logs, ...).
//!
//! At export time the tables of the *actual* schema are compared with this list. A table that is not listed and has a
//! per-character column holding rows of this character blocks the export instead of being ignored.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TableClass {
    Portable,
    Deferred,
    Appearance,
    Collection,
    AccountLocal,
    CharacterLocal,
    Blocking,
    Realm,
}

pub fn classify(table: &str) -> Option<TableClass> {
    TABLES
        .binary_search_by(|(name, _)| (*name).cmp(table))
        .ok()
        .map(|i| TABLES[i].1)
}

/// The tables of `actual` that the registry does not know (new modules, newer core).
pub fn unclassified<'a>(actual: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    actual
        .into_iter()
        .filter(|t| classify(t).is_none())
        .map(str::to_string)
        .collect()
}

/// Column names that identify "this table holds data of one character". `account` is deliberately not here: account
/// state cannot be attributed to a single character.
pub fn is_character_column(column: &str) -> bool {
    matches!(
        column.to_ascii_lowercase().as_str(),
        "guid"
            | "owner_guid"
            | "owner"
            | "character_guid"
            | "char_guid"
            | "characterguid"
            | "charguid"
            | "player_guid"
            | "playerguid"
            | "owner_id"
            | "bot_guid"
            | "character_id"
    )
}

/// Sorted by name (binary search).
pub const TABLES: &[(&str, TableClass)] = &[
    ("account_appearance_collection", TableClass::Collection),
    ("account_ascension_settings", TableClass::AccountLocal),
    ("account_data", TableClass::AccountLocal),
    ("account_instance_times", TableClass::AccountLocal),
    ("account_tutorial", TableClass::AccountLocal),
    ("account_vanity_collection", TableClass::Collection),
    ("active_arena_season", TableClass::Realm),
    ("addons", TableClass::Realm),
    ("arena_team", TableClass::Realm),
    ("arena_team_member", TableClass::Realm),
    ("ascension_manastorm_bonus", TableClass::CharacterLocal),
    ("ascension_manastorm_cache", TableClass::Blocking),
    ("ascension_manastorm_clear", TableClass::CharacterLocal),
    ("ascension_manastorm_loadout", TableClass::CharacterLocal),
    ("ascension_manastorm_xp", TableClass::CharacterLocal),
    ("ascension_player_ticket", TableClass::CharacterLocal),
    (
        "ascension_player_ticket_message",
        TableClass::CharacterLocal,
    ),
    ("auctionhouse", TableClass::Realm),
    ("banned_addons", TableClass::Realm),
    ("battleground_deserters", TableClass::Realm),
    ("bugreport", TableClass::Realm),
    ("calendar_events", TableClass::Realm),
    ("calendar_invites", TableClass::Realm),
    ("channels", TableClass::Realm),
    ("channels_bans", TableClass::Realm),
    ("channels_rights", TableClass::Realm),
    ("character_account_data", TableClass::Portable),
    ("character_achievement", TableClass::CharacterLocal),
    (
        "character_achievement_offline_updates",
        TableClass::CharacterLocal,
    ),
    ("character_achievement_progress", TableClass::CharacterLocal),
    ("character_action", TableClass::Portable),
    ("character_appearance", TableClass::Appearance),
    ("character_appearance_outfit", TableClass::Appearance),
    ("character_appearance_settings", TableClass::Appearance),
    ("character_arena_stats", TableClass::CharacterLocal),
    ("character_ascension_state", TableClass::CharacterLocal),
    ("character_aura", TableClass::CharacterLocal),
    ("character_banned", TableClass::CharacterLocal),
    ("character_battleground_random", TableClass::CharacterLocal),
    ("character_brew_of_the_month", TableClass::CharacterLocal),
    ("character_coa_lfg_settings", TableClass::CharacterLocal),
    ("character_declinedname", TableClass::CharacterLocal),
    ("character_entry_point", TableClass::CharacterLocal),
    ("character_equipmentsets", TableClass::Deferred),
    ("character_gifts", TableClass::Portable),
    ("character_glyphs", TableClass::Portable),
    ("character_homebind", TableClass::CharacterLocal),
    ("character_instance", TableClass::CharacterLocal),
    ("character_inventory", TableClass::Portable),
    ("character_pet", TableClass::Portable),
    ("character_pet_declinedname", TableClass::Portable),
    ("character_queststatus", TableClass::Portable),
    ("character_queststatus_daily", TableClass::CharacterLocal),
    ("character_queststatus_monthly", TableClass::CharacterLocal),
    ("character_queststatus_rewarded", TableClass::Portable),
    ("character_queststatus_seasonal", TableClass::CharacterLocal),
    ("character_queststatus_weekly", TableClass::CharacterLocal),
    ("character_reputation", TableClass::Portable),
    ("character_settings", TableClass::Portable),
    ("character_skills", TableClass::Portable),
    ("character_social", TableClass::CharacterLocal),
    ("character_spell", TableClass::Portable),
    ("character_spell_cooldown", TableClass::CharacterLocal),
    ("character_stats", TableClass::CharacterLocal),
    ("character_talent", TableClass::Portable),
    ("character_worldforged_loot", TableClass::CharacterLocal),
    ("characters", TableClass::Portable),
    ("chat_filter", TableClass::Realm),
    ("coa_account_warchest", TableClass::AccountLocal),
    ("coa_bot_gear_history", TableClass::CharacterLocal),
    ("coa_bot_gear_queue", TableClass::CharacterLocal),
    ("coa_challenge_completion", TableClass::CharacterLocal),
    ("coa_challenge_failure", TableClass::CharacterLocal),
    ("coa_character_challenge", TableClass::Blocking),
    ("coa_character_condition", TableClass::CharacterLocal),
    ("coa_character_fatigue", TableClass::CharacterLocal),
    ("coa_character_gamemode", TableClass::Blocking),
    ("coa_character_gamemode_lives", TableClass::CharacterLocal),
    ("coa_character_looted_item", TableClass::CharacterLocal),
    ("coa_character_objective", TableClass::CharacterLocal),
    ("coa_character_survival", TableClass::CharacterLocal),
    ("coa_custom_trial", TableClass::CharacterLocal),
    ("coa_custom_trial_active", TableClass::Blocking),
    ("coa_custom_trial_completion", TableClass::CharacterLocal),
    ("coa_custom_trial_entry", TableClass::CharacterLocal),
    ("coa_custom_trial_vote", TableClass::CharacterLocal),
    ("coa_keepers_scroll_blessing", TableClass::Realm),
    ("coa_portable_session", TableClass::CharacterLocal),
    ("coa_squid_migrations", TableClass::Realm),
    ("coa_wildcard_skill_card", TableClass::Collection),
    ("coa_wildcard_skill_card_account", TableClass::Collection),
    ("coa_wildcard_skill_card_pending", TableClass::Collection),
    ("coa_wildcard_skill_card_purchase", TableClass::Collection),
    (
        "coa_wildcard_specialization_cache",
        TableClass::AccountLocal,
    ),
    ("corpse", TableClass::CharacterLocal),
    ("creature_respawn", TableClass::Realm),
    ("ethereal_bazaar_meta", TableClass::Realm),
    ("ethereal_bazaar_stock", TableClass::Realm),
    ("game_event_condition_save", TableClass::Realm),
    ("game_event_save", TableClass::Realm),
    ("gameobject_respawn", TableClass::Realm),
    ("gm_subsurvey", TableClass::Realm),
    ("gm_survey", TableClass::Realm),
    ("gm_ticket", TableClass::Realm),
    ("group_member", TableClass::Realm),
    ("groups", TableClass::Realm),
    ("guild", TableClass::Realm),
    ("guild_bank_eventlog", TableClass::Realm),
    ("guild_bank_item", TableClass::Realm),
    ("guild_bank_right", TableClass::Realm),
    ("guild_bank_tab", TableClass::Realm),
    ("guild_eventlog", TableClass::Realm),
    ("guild_member", TableClass::Realm),
    ("guild_member_withdraw", TableClass::Realm),
    ("guild_rank", TableClass::Realm),
    ("highrisk_chest", TableClass::CharacterLocal),
    ("highrisk_chest_item", TableClass::CharacterLocal),
    ("instance", TableClass::Realm),
    ("instance_reset", TableClass::Realm),
    ("instance_saved_go_state_data", TableClass::Realm),
    ("item_instance", TableClass::Portable),
    ("item_loot_storage", TableClass::CharacterLocal),
    ("item_refund_instance", TableClass::CharacterLocal),
    ("item_soulbound_trade_data", TableClass::CharacterLocal),
    ("lag_reports", TableClass::Realm),
    ("lfg_data", TableClass::CharacterLocal),
    ("log_arena_fights", TableClass::Realm),
    ("log_arena_memberstats", TableClass::Realm),
    ("log_encounter", TableClass::Realm),
    ("log_money", TableClass::Realm),
    ("mail", TableClass::Realm),
    ("mail_items", TableClass::Realm),
    ("mail_server_character", TableClass::Realm),
    ("mail_server_template", TableClass::Realm),
    ("mail_server_template_conditions", TableClass::Realm),
    ("mail_server_template_items", TableClass::Realm),
    ("mod_ascension_bank_item", TableClass::Deferred),
    ("mod_ascension_bank_log", TableClass::Deferred),
    ("mod_ascension_bank_money", TableClass::Deferred),
    ("mod_ascension_bank_tab", TableClass::Deferred),
    (
        "mod_coa_bot_guild_gather_orders",
        TableClass::CharacterLocal,
    ),
    ("mod_craftsmans_codex", TableClass::Deferred),
    ("pet_aura", TableClass::CharacterLocal),
    ("pet_spell", TableClass::Portable),
    ("pet_spell_cooldown", TableClass::CharacterLocal),
    ("petition", TableClass::Realm),
    ("petition_sign", TableClass::Realm),
    ("player_anticheat_alert", TableClass::CharacterLocal),
    ("playerbots_arena_team_names", TableClass::Realm),
    ("playerbots_guild_names", TableClass::Realm),
    ("playerbots_names", TableClass::Realm),
    ("pool_quest_save", TableClass::Realm),
    ("profanity_name", TableClass::Realm),
    ("pvpstats_battlegrounds", TableClass::Realm),
    ("pvpstats_players", TableClass::Realm),
    ("quest_tracker", TableClass::CharacterLocal),
    ("recovery_item", TableClass::CharacterLocal),
    ("reserved_name", TableClass::Realm),
    ("spam_reports", TableClass::Realm),
    ("updates", TableClass::Realm),
    ("updates_include", TableClass::Realm),
    ("warden_action", TableClass::Realm),
    ("world_state", TableClass::Realm),
    ("worldstates", TableClass::Realm),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const SCHEMA_TABLES: &str = include_str!("testdata/characters-tables.txt");

    fn of(class: TableClass) -> BTreeSet<&'static str> {
        TABLES
            .iter()
            .filter(|(_, c)| *c == class)
            .map(|(t, _)| *t)
            .collect()
    }

    #[test]
    fn the_registry_is_sorted_and_free_of_duplicates() {
        assert!(
            TABLES.windows(2).all(|w| w[0].0 < w[1].0),
            "TABLES must be strictly sorted for the binary search"
        );
    }

    #[test]
    fn every_table_of_the_real_schema_is_classified_and_nothing_is_stale() {
        let schema: BTreeSet<&str> = SCHEMA_TABLES
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(schema.len(), 164);
        let missing: Vec<_> = schema.iter().filter(|t| classify(t).is_none()).collect();
        assert!(missing.is_empty(), "unclassified tables: {missing:?}");
        let stale: Vec<_> = TABLES
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| !schema.contains(t))
            .collect();
        assert!(
            stale.is_empty(),
            "classified tables that are not in the schema: {stale:?}"
        );
    }

    #[test]
    fn a_new_module_table_is_reported_as_unclassified() {
        assert_eq!(
            unclassified(["characters", "mod_shiny_new_thing", "item_instance"]),
            vec!["mod_shiny_new_thing".to_string()]
        );
    }

    #[test]
    fn the_v1_scope_is_exactly_the_approved_minimal_set() {
        let expected: BTreeSet<&str> = [
            "characters",
            "character_inventory",
            "item_instance",
            "character_gifts",
            "character_spell",
            "character_talent",
            "character_skills",
            "character_glyphs",
            "character_reputation",
            "character_queststatus",
            "character_queststatus_rewarded",
            "character_action",
            "character_pet",
            "pet_spell",
            "character_pet_declinedname",
            "character_settings",
            "character_account_data",
        ]
        .into_iter()
        .collect();
        assert_eq!(of(TableClass::Portable), expected);
    }

    #[test]
    fn deferred_and_blocking_tables_match_the_decisions() {
        let deferred: BTreeSet<&str> = [
            "character_equipmentsets",
            "mod_ascension_bank_tab",
            "mod_ascension_bank_item",
            "mod_ascension_bank_money",
            "mod_ascension_bank_log",
            "mod_craftsmans_codex",
        ]
        .into_iter()
        .collect();
        assert_eq!(of(TableClass::Deferred), deferred);
        let appearance: BTreeSet<&str> = [
            "character_appearance",
            "character_appearance_outfit",
            "character_appearance_settings",
        ]
        .into_iter()
        .collect();
        assert_eq!(of(TableClass::Appearance), appearance);
        let blocking: BTreeSet<&str> = [
            "coa_character_challenge",
            "coa_character_gamemode",
            "coa_custom_trial_active",
            "ascension_manastorm_cache",
        ]
        .into_iter()
        .collect();
        assert_eq!(of(TableClass::Blocking), blocking);
        let collections: BTreeSet<&str> =
            ["account_appearance_collection", "account_vanity_collection"]
                .into_iter()
                .collect();
        assert!(collections.is_subset(&of(TableClass::Collection)));
        assert_eq!(classify("mail"), Some(TableClass::Realm));
        assert_eq!(classify("character_aura"), Some(TableClass::CharacterLocal));
        assert_eq!(classify("account_data"), Some(TableClass::AccountLocal));
        assert_eq!(classify("nope"), None);
    }

    #[test]
    fn character_columns_are_recognised() {
        for c in [
            "guid",
            "GUID",
            "owner_guid",
            "owner",
            "character_guid",
            "bot_guid",
        ] {
            assert!(is_character_column(c), "{c}");
        }
        for c in ["account", "entry", "id", "item_guid", "name"] {
            assert!(!is_character_column(c), "{c}");
        }
    }
}
