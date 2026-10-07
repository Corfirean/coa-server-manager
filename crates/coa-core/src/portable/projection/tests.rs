use super::testing::*;
use super::*;
use crate::portable::ids::PortableItemId;
use crate::portable::merge::{merge3_with, MergeProjection, Mode};
use crate::portable::model::*;

#[test]
fn the_activation_matrix_follows_the_level_against_the_cap() {
    let at = |level: u8, cap: u32| activation(level, &progression(cap));
    assert_eq!(at(80, 80), Activation::None);
    assert_eq!(at(80, 70), Activation::Active { canonical_level: 80, projected_level: 70 });
    assert_eq!(at(80, 60), Activation::Active { canonical_level: 80, projected_level: 60 });
    assert_eq!(at(70, 60), Activation::Active { canonical_level: 70, projected_level: 60 });
    assert_eq!(at(60, 60), Activation::None);
    assert_eq!(at(1, 60), Activation::None);
}

#[test]
fn what_the_core_holds_is_taken_out_of_the_working_copy_and_nothing_else() {
    let c = canonical();
    let hold = hold_for(&c);
    hold.check_subject(&c).unwrap();
    let p = apply(&c, &hold);
    assert_eq!((p.progression.level, p.progression.xp), (60, 0));
    assert_eq!(c.progression.level, 80, "the canonical character is untouched");
    let bag = hold.held_items[1];
    assert!(p.items.iter().all(|i| !hold.held_items.contains(&i.id) && i.container != Some(bag)), "a held bag takes its contents with it");
    assert_eq!(p.items.len(), c.items.len() - 2 - 6);
    assert!(p.build.spells.iter().all(|(s, _)| *s != 500_006 && *s != 500_009));
    assert_eq!(p.build.spells.len(), c.build.spells.len() - 2);
    assert!(!p.actions.iter().any(|a| a.button == 1));
    assert_eq!(p.settings["core.ascension_slot.0"], vec![1, 21, 61, 1, 100, 1, 1, 0, 500_003]);
    assert_eq!(p.settings["core.ascension_build.61"], vec![1, 1001]);
    assert_eq!(p.settings["core.ascension_bar.61"], vec![1, 0, 500_003]);
    assert_eq!(p.settings["core.ascension_slot.active"], vec![0], "other settings are left alone");
    assert_eq!(p.wardrobe, c.wardrobe);
    assert_eq!(p.pets, c.pets);

    let mut changed = c.clone();
    changed.progression.money += 1;
    assert!(hold.check_subject(&changed).is_err(), "a decision belongs to the state it was made for");
    let mut again = hold.clone();
    again.subject.clear();
    again.check_subject(&changed).unwrap();
    assert_eq!(apply(&changed, &again).progression.money, changed.progression.money, "a stored decision can be applied to a later state");
}

#[test]
fn a_blocked_record_is_not_given_to_the_realm() {
    let c = canonical();
    let mut hold = hold_for(&c);
    hold.settings.retain(|s| s.source != "core.ascension_slot.0");
    hold.blocked_settings = vec!["core.ascension_slot.0".into()];
    let p = apply(&c, &hold);
    assert!(!p.settings.contains_key("core.ascension_slot.0"));
}

/// What the realm shows right after its own first load: the same character, but the core has rewritten the build record in its own order and
/// with its own padding.
fn realm_b0(p: &PortableCharacter) -> PortableCharacter {
    let mut b0 = p.clone();
    b0.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 61, 1, 100, 1, 1, 0, 500_003, 0, 0, 0]);
    b0.normalized()
}

fn session(c0: &PortableCharacter, b0: &PortableCharacter, b1: &PortableCharacter, blocked: &[&str]) -> PortableCharacter {
    let projection = MergeProjection { freeze_progression: true, blocked: blocked.iter().map(|s| s.to_string()).collect() };
    merge3_with(c0, b0, b1, Mode::Lenient, Some(&projection)).unwrap().model
}

#[test]
fn a_session_on_a_projected_copy_never_down_levels_or_loses_what_was_held() {
    let c0 = canonical();
    let hold = hold_for(&c0);
    let b0 = realm_b0(&apply(&c0, &hold));

    // nothing played: the realm's own rewrite of the record changes nothing canonical
    let idle = session(&c0, &b0, &b0, &[]);
    assert_eq!(idle, c0, "an idle session is the canonical character exactly");

    // play: gold, a new item, a new talent entry on the record, the realm's own level/xp (60, some xp), a new rep standing
    let mut b1 = b0.clone();
    b1.progression.money += 5_000;
    b1.progression.xp = 777;
    b1.reputation[0].standing += 250;
    b1.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 61, 2, 100, 1, 400, 1, 1, 0, 500_003, 0]);
    b1.settings.insert("core.ascension_build.61".into(), vec![2, 1001, 4001]);
    let mut gain = b1.items.iter().find(|i| i.container.is_none() && i.slot == 23).unwrap().clone();
    gain.id = PortableItemId::new();
    gain.slot = 36;
    gain.entry = crate::portable::ids::ContentId::new("coa", "item", 123_456).unwrap();
    b1.items.push(gain.clone());
    let b1 = b1.normalized();

    let c1 = session(&c0, &b0, &b1, &[]);
    assert_eq!((c1.progression.level, c1.progression.xp), (80, 12_345), "the canonical level and xp are not the realm's");
    assert_eq!(c1.progression.money, c0.progression.money + 5_000);
    assert_eq!(c1.reputation[0].standing, c0.reputation[0].standing + 250);
    assert!(c1.items.iter().any(|i| i.id == gain.id), "a gain arrives");
    for held in &hold.held_items {
        assert!(c1.items.iter().any(|i| i.id == *held), "a held item stays canonical");
    }
    assert_eq!(c1.items.len(), c0.items.len() + 1);
    for spell in &hold.held_spells {
        assert!(c1.build.spells.iter().any(|(s, _)| s == spell), "a held ability stays canonical");
    }
    assert!(c1.actions.iter().any(|a| a.button == 1), "a held button stays canonical");
    let slot = settings::Record::parse(settings::SettingKind::Slot, &c1.settings["core.ascension_slot.0"]).unwrap();
    assert_eq!(slot.entries.keys().copied().collect::<Vec<_>>(), vec![100, 200, 300, 400], "the record keeps what the realm could not hold and gains the new entry");
    assert_eq!(slot.entries[&200], 2);
    assert_eq!(slot.buttons.get(&1), Some(&500_006));
    let build = settings::Record::parse(settings::SettingKind::Build, &c1.settings["core.ascension_build.61"]).unwrap();
    assert_eq!(build.entries.keys().copied().collect::<Vec<_>>(), vec![100, 200, 300, 400]);
    assert_eq!(c1.settings["core.ascension_bar.61"], c0.settings["core.ascension_bar.61"]);

    // a checkpoint repeated, or a later one with the same play, counts nothing twice
    assert_eq!(session(&c0, &b0, &b1, &[]), c1);
    let mut later = b1.clone();
    later.progression.money += 100;
    let c2 = session(&c0, &b0, &later, &[]);
    assert_eq!(c2.progression.money, c0.progression.money + 5_100);
    assert_eq!(c2.items.len(), c1.items.len());
}

#[test]
fn a_talent_the_player_removed_on_the_realm_is_removed_but_a_held_one_is_not() {
    let c0 = canonical();
    let hold = hold_for(&c0);
    let b0 = realm_b0(&apply(&c0, &hold));
    let mut b1 = b0.clone();
    b1.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 61, 0, 1, 0, 500_003]);
    let c1 = session(&c0, &b0, &b1.normalized(), &[]);
    let slot = settings::Record::parse(settings::SettingKind::Slot, &c1.settings["core.ascension_slot.0"]).unwrap();
    assert_eq!(slot.entries.keys().copied().collect::<Vec<_>>(), vec![200, 300], "entry 100 was dropped by the player, 200 and 300 were never the realm's to drop");
}

#[test]
fn a_record_the_core_could_not_take_apart_is_never_merged() {
    let c0 = canonical();
    let mut hold = hold_for(&c0);
    hold.settings.retain(|s| s.source != "core.ascension_slot.0");
    hold.blocked_settings = vec!["core.ascension_slot.0".into()];
    let b0 = apply(&c0, &hold);
    let mut b1 = b0.clone();
    b1.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 61, 1, 100, 1, 0]);
    let c1 = session(&c0, &b0, &b1.normalized(), &["core.ascension_slot.0"]);
    assert_eq!(c1.settings["core.ascension_slot.0"], c0.settings["core.ascension_slot.0"], "the realm's rewrite of a blocked record never replaces the canonical one");
}

#[test]
fn without_a_projection_the_merge_is_what_it_was() {
    let c0 = canonical();
    let b0 = c0.clone();
    let mut b1 = c0.clone();
    b1.progression.level = 81;
    b1.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 61, 0, 0]);
    let plain = crate::portable::merge::merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert_eq!(plain.progression.level, 81);
    assert_eq!(plain.settings["core.ascension_slot.0"], vec![1, 21, 61, 0, 0], "a whole value, as before");
}

#[test]
fn the_core_answer_is_parsed_strictly() {
    let sig = SIGNATURE;
    let item = PortableItemId::new();
    let active = format!(r#"{{"status":"ok","projection":{{"active":true,"protocol":1,"policy_version":1,"progression_signature":"{sig}","max_player_level":60,"canonical_level":80,"projected_level":60,"held_items":["{item}"],"held_spells":[5,6],"held_actions":[[0,3]],"settings":[{{"source":"core.ascension_slot.0","kind":"slot","entries":[1],"buttons":[2]}}],"blocked_settings":["x"]}}}}"#);
    let Decision::Projected(h) = ProjectionAnswer::parse(&active, "abc").unwrap() else { panic!("projected") };
    assert_eq!((h.held_items.clone(), h.held_spells.clone(), h.held_actions.clone(), h.subject.clone()), (vec![item], vec![5, 6], vec![(0, 3)], "abc".to_string()));
    assert_eq!(h.settings[0].kind, settings::SettingKind::Slot);

    let native = format!(r#"{{"status":"ok","projection":{{"active":false,"protocol":1,"policy_version":1,"progression_signature":"{sig}","max_player_level":60,"canonical_level":50,"projected_level":50}}}}"#);
    assert_eq!(ProjectionAnswer::parse(&native, "").unwrap(), Decision::Native);
    assert!(ProjectionAnswer::parse(&format!("noise\n{native}\n"), "").is_ok(), "console noise around the line is ignored");

    for bad in [
        active.replace("\"protocol\":1", "\"protocol\":2"),
        active.replace("\"policy_version\":1", "\"policy_version\":9"),
        active.replace("\"projected_level\":60", "\"projected_level\":61"),
        active.replace("\"canonical_level\":80", "\"canonical_level\":60"),
        active.replace("\"status\":\"ok\"", "\"status\":\"maybe\""),
        active.replace("\"held_spells\":[5,6]", "\"held_spells\":[5,6],\"extra\":1"),
        active.replace(&item.to_string(), "not-a-uuid"),
        "Unknown command".to_string(),
    ] {
        assert!(ProjectionAnswer::parse(&bad, "").is_err(), "{bad}");
    }
    let refused = r#"{"status":"refused","problems":[{"code":"snapshot","detail":"broken"}]}"#;
    let err = ProjectionAnswer::parse(refused, "").unwrap_err().to_string();
    assert!(err.contains("snapshot") && err.contains("broken"), "{err}");
}

#[test]
fn the_pin_words_are_the_ones_the_core_stores() {
    let words = progression(60).pin_words();
    assert_eq!(words.len(), 10);
    assert_eq!(&words[..3], &[1, 60, 0xbd299b9e]);
    assert_eq!(words[9], 0x1ad6d76d);
}

#[test]
fn a_context_must_be_consistent() {
    let c = canonical();
    let hold = hold_for(&c);
    let ctx = ProjectionContext::new(hold.clone(), 4, &"ab".repeat(32));
    ctx.validate().unwrap();
    assert!(ctx.pin().projected);
    let mut bad = ctx.clone();
    bad.projected_level = 80;
    assert!(bad.validate().is_err());
    let mut bad = ctx.clone();
    bad.content_profile_hash = "nothex".into();
    assert!(bad.validate().is_err());
    let json = serde_json::to_string(&ctx).unwrap();
    assert_eq!(serde_json::from_str::<ProjectionContext>(&json).unwrap(), ctx);
    assert!(serde_json::from_str::<ProjectionContext>(&json.replace("\"canonical_level\":80", "\"canonical_level\":80,\"x\":1")).is_err());
}
