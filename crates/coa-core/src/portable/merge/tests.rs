use super::*;
use crate::portable::fixtures::{geared_level_eighty, id7};
use crate::portable::ids::ContentId;

fn c0() -> PortableCharacter {
    geared_level_eighty()
}

fn new_item(n: u64, slot: u8) -> PortableItem {
    PortableItem {
        id: PortableItemId::from_uuid(id7(900_000 + n)).unwrap(),
        container: None,
        slot,
        entry: ContentId::new("coa", "item", 77_000 + n).unwrap(),
        count: 1,
        duration: 0,
        charges: vec![],
        flags: 0,
        enchantments: vec![],
        random_property_id: 0,
        durability: 50,
        played_time: 0,
        text: None,
        creator_name: None,
        gift: None,
    }
}

/// Ids of the equipped / backpack items of the fixture that are not bags and hold nothing: safe to filter away.
fn plain_items(m: &PortableCharacter) -> Vec<PortableItemId> {
    let containers: HashSet<PortableItemId> = m.items.iter().filter_map(|i| i.container).collect();
    m.items.iter().filter(|i| !containers.contains(&i.id) && i.container.is_none() && i.slot < 40).map(|i| i.id).collect()
}

/// What a realm does to a freshly imported character by itself, before the player has done anything.
fn normalized_by_the_realm(c0: &PortableCharacter) -> (PortableCharacter, Vec<PortableItemId>) {
    let mut b = c0.clone();
    // adds default spells and skills, removes spells its DBC does not know
    b.build.spells.extend([(6654, 255), (23250, 255)]);
    b.build.spells.retain(|(s, _)| s % 9 != 0);
    b.build.skills.push(Skill { skill: 9999, value: 1, max: 1 });
    // resets what it resets
    b.progression.honor.today_honor = 0;
    b.progression.honor.yesterday_honor = 0;
    b.progression.honor.today_kills = 0;
    b.progression.chosen_title = 0;
    b.progression.known_currencies |= 1 << 40;
    b.progression.taxi_mask = "827019776 269484704 ".into();
    b.progression.explored_zones = format!("{} 16384 ", b.progression.explored_zones.trim());
    b.reputation.push(ReputationEntry { faction: 9001, standing: 0, flags: 64 });
    b.settings.insert("core.ascension_slot.0".into(), vec![1, 21, 0, 0, 0]);
    // mails away what it cannot place
    let filtered: Vec<PortableItemId> = plain_items(c0).into_iter().take(7).collect();
    b.items.retain(|i| !filtered.contains(&i.id));
    // and gives the character things of its own
    b.items.push(new_item(1, 30));
    b.items.push(new_item(2, 31));
    // the realm also normalises the pet
    (b.normalized(), filtered)
}

#[test]
fn normalisation_alone_changes_nothing_in_the_canonical_character() {
    let c0 = c0();
    let (b0, filtered) = normalized_by_the_realm(&c0);
    assert_ne!(b0, c0, "the realm really changed things");
    let merged = merge3(&c0, &b0, &b0, Mode::Lenient).unwrap();
    assert_eq!(merged.model, c0, "no session progress: the canonical character is exactly as it was");
    assert!(merged.changes.is_empty(), "{:?}", merged.changes);
    assert!(!merged.left_alone.is_empty(), "the realm's own changes are reported as left alone");
    assert_eq!(merged.items.filtered_kept.len(), filtered.len());
    assert_eq!(merged.items.realm_local.len(), 2);
    assert!(merged.items.removed.is_empty() && merged.items.added.is_empty());
    assert!(merged.conflicts.is_empty());
}

#[test]
fn only_the_session_delta_is_applied() {
    let c0 = c0();
    let (b0, filtered) = normalized_by_the_realm(&c0);
    let mut b1 = b0.clone();

    // the player plays
    b1.progression.money += 12_345;
    b1.progression.xp += 50;
    b1.progression.honor.total_honor += 100;
    b1.progression.honor.today_honor = 7;
    b1.build.spells.push((133, 255));
    let unlearned = c0.build.spells.iter().map(|s| s.0).find(|s| s % 9 != 0).unwrap();
    b1.build.spells.retain(|(s, _)| *s != unlearned);
    b1.quests.rewarded.push(12_010);
    let finished = b1.quests.active.remove(0);
    b1.reputation.iter_mut().find(|r| r.faction == 3).unwrap().standing += 500;
    b1.settings.insert("core.ascension_build.54".into(), vec![3, 1, 2, 3]);
    b1.pets[0].level = 81;
    b1.pets[0].exp += 500;
    b1.pets[0].spells.push(PetSpell { spell: 777, active: 1 });
    // items: one sold, one with a changed stack, one moved, one new
    let sold = plain_items(&c0).into_iter().find(|id| !filtered.contains(id)).unwrap();
    b1.items.retain(|i| i.id != sold);
    let restacked = b1.items.iter().position(|i| i.container.is_some()).unwrap();
    b1.items[restacked].count += 4;
    let restacked_id = b1.items[restacked].id;
    let loot = new_item(50, 33);
    b1.items.push(loot.clone());
    let b1 = b1.normalized();

    let m = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    let r = &m.model;
    // progress is in ...
    assert_eq!(r.progression.money, c0.progression.money + 12_345);
    assert_eq!(r.progression.xp, c0.progression.xp + 50);
    assert_eq!(r.progression.honor.total_honor, c0.progression.honor.total_honor + 100);
    assert_eq!(r.progression.honor.today_honor, 7);
    assert!(r.build.spells.contains(&(133, 255)));
    assert!(!r.build.spells.iter().any(|(s, _)| *s == unlearned), "what the player unlearned is gone");
    assert!(r.quests.rewarded.contains(&12_010));
    assert!(!r.quests.active.iter().any(|q| q.quest == finished.quest));
    assert_eq!(r.reputation.iter().find(|x| x.faction == 3).unwrap().standing, c0.reputation.iter().find(|x| x.faction == 3).unwrap().standing + 500);
    assert_eq!(r.settings["core.ascension_build.54"], vec![3, 1, 2, 3]);
    assert_eq!((r.pets[0].level, r.pets[0].exp), (81, c0.pets[0].exp + 500));
    assert!(r.pets[0].spells.iter().any(|s| s.spell == 777));
    assert!(!r.items.iter().any(|i| i.id == sold), "the sold item is gone");
    assert_eq!(r.items.iter().find(|i| i.id == restacked_id).unwrap().count, c0.items.iter().find(|i| i.id == restacked_id).unwrap().count + 4);
    assert!(r.items.contains(&loot), "the new item is in");
    // ... and none of the realm's own normalisation is
    assert!(!r.build.spells.iter().any(|(s, _)| *s == 6654 || *s == 23250), "default spells stay out");
    assert!(c0.build.spells.iter().filter(|(s, _)| s % 9 == 0).all(|sp| r.build.spells.contains(sp)), "spells its DBC dropped stay canonical");
    assert_eq!(r.progression.chosen_title, c0.progression.chosen_title, "a title reset by the realm is not the player's doing");
    assert_eq!(r.progression.known_currencies, c0.progression.known_currencies);
    assert_eq!(r.progression.taxi_mask, c0.progression.taxi_mask);
    assert!(!r.reputation.iter().any(|x| x.faction == 9001));
    assert!(!r.settings.contains_key("core.ascension_slot.0"));
    assert!(!r.build.skills.iter().any(|s| s.skill == 9999));
    for id in &filtered {
        assert!(r.items.iter().any(|i| i.id == *id), "an item the realm filtered away stays canonical");
    }
    assert!(!r.items.iter().any(|i| i.entry.id() == 77_001 || i.entry.id() == 77_002), "the realm's own items are not merged");

    assert_eq!(m.items.removed, vec![sold]);
    assert_eq!(m.items.added, vec![loot.id]);
    assert_eq!(m.items.changed, vec![restacked_id]);
    assert_eq!(m.items.filtered_kept.len(), filtered.len());
    r.validate().unwrap();
}

#[test]
fn the_merge_is_repeatable() {
    let c0 = c0();
    let (b0, _) = normalized_by_the_realm(&c0);
    let mut b1 = b0.clone();
    b1.progression.money += 99;
    b1.items.push(new_item(51, 34));
    let b1 = b1.normalized();
    let first = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    let second = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert_eq!(first, second, "a checkpoint can be recomputed from the same baseline any number of times");
    // and a later checkpoint is computed from the same C0, not stacked on the earlier result
    let mut b2 = b1.clone();
    b2.progression.money += 1;
    let third = merge3(&c0, &b0, &b2, Mode::Lenient).unwrap().model;
    assert_eq!(third.progression.money, c0.progression.money + 100);
}

#[test]
fn money_is_a_delta_so_a_realm_that_started_elsewhere_does_not_overwrite_it() {
    let c0 = c0();
    let mut b0 = c0.clone();
    b0.progression.money = c0.progression.money - 1_000; // the realm normalised money a little (e.g. a deposit)
    let mut b1 = b0.clone();
    b1.progression.money += 250;
    let m = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert_eq!(m.progression.money, c0.progression.money + 250);
    // clamped at the realm's limits
    let mut huge = b0.clone();
    huge.progression.money = limits::MAX_MONEY;
    assert_eq!(merge3(&c0, &b0, &huge, Mode::Lenient).unwrap().model.progression.money, limits::MAX_MONEY);
    let mut broke = b0.clone();
    broke.progression.money = 0;
    assert_eq!(merge3(&c0, &b0, &broke, Mode::Lenient).unwrap().model.progression.money, 1_000, "the realm lost all it had: the canonical character loses the same amount");
    let (mut poor, mut poor_b0) = (c0.clone(), c0.clone());
    (poor.progression.money, poor_b0.progression.money) = (10, 100);
    let mut poor_b1 = poor_b0.clone();
    poor_b1.progression.money = 0;
    assert_eq!(merge3(&poor, &poor_b0, &poor_b1, Mode::Lenient).unwrap().model.progression.money, 0, "never below zero");
}

#[test]
fn a_level_up_takes_the_realms_level_and_xp_together() {
    let c0 = c0();
    let b0 = c0.clone();
    let mut b1 = b0.clone();
    b1.progression.level = 81;
    b1.progression.xp = 17;
    let m = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert_eq!((m.progression.level, m.progression.xp), (81, 17));
}

#[test]
fn bit_sets_only_gain_what_the_session_gained() {
    let mut c0 = c0();
    c0.progression.explored_zones = "3 0 8 ".into();
    c0.progression.known_titles = "1 0 ".into();
    let mut b0 = c0.clone();
    b0.progression.explored_zones = "3 64 ".into(); // the realm added a bit of its own and dropped bit 3 of word 2
    let mut b1 = b0.clone();
    b1.progression.explored_zones = "7 64 ".into(); // the player explored bit 2 of word 0
    b1.progression.known_titles = "3 0 ".into();
    let m = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert_eq!(m.progression.explored_zones.trim(), "7 0 8", "gained bit in, the realm's own bit out, the canonical bit the realm lacked stays");
    assert_eq!(m.progression.known_titles.trim(), "3 0");
}

#[test]
fn items_the_realm_filtered_away_stay_while_items_the_player_got_rid_of_go() {
    let c0 = c0();
    let (b0, filtered) = normalized_by_the_realm(&c0);
    // session: the player deletes one present item; nothing else
    let gone = plain_items(&c0).into_iter().find(|id| !filtered.contains(id)).unwrap();
    let mut b1 = b0.clone();
    b1.items.retain(|i| i.id != gone);
    let m = merge3(&c0, &b0, &b1.normalized(), Mode::Lenient).unwrap();
    assert_eq!(m.items.removed, vec![gone]);
    assert_eq!(m.items.filtered_kept.len(), 7);
    for id in &filtered {
        assert!(m.model.items.iter().any(|i| i.id == *id));
    }
    assert!(!m.model.items.iter().any(|i| i.id == gone));
    assert_eq!(m.model.items.len(), c0.items.len() - 1);
}

#[test]
fn a_filtered_item_that_comes_back_is_taken_from_the_realm_but_keeps_its_crafter() {
    let c0 = c0();
    let (b0, filtered) = normalized_by_the_realm(&c0);
    let back = filtered[0];
    let mut returned = c0.items.iter().find(|i| i.id == back).unwrap().clone();
    returned.count = 3;
    returned.slot = 36;
    returned.creator_name = None; // the realm never knew the crafter
    let mut b1 = b0.clone();
    b1.items.push(returned);
    let m = merge3(&c0, &b0, &b1.normalized(), Mode::Lenient).unwrap();
    assert_eq!(m.items.reappeared, vec![back]);
    let item = m.model.items.iter().find(|i| i.id == back).unwrap();
    assert_eq!((item.count, item.slot), (3, 36));
    assert_eq!(item.creator_name, c0.items.iter().find(|i| i.id == back).unwrap().creator_name);
}

#[test]
fn a_place_taken_by_the_session_moves_the_item_that_was_filtered_there() {
    let c0 = c0();
    let (b0, filtered) = normalized_by_the_realm(&c0);
    let victim = c0.items.iter().find(|i| i.id == filtered[0]).unwrap().clone();
    // the player puts a new item exactly where the filtered one canonically is
    let mut occupant = new_item(60, victim.slot);
    occupant.container = victim.container;
    let mut b1 = b0.clone();
    b1.items.push(occupant.clone());
    let m = merge3(&c0, &b0, &b1.normalized(), Mode::Lenient).unwrap();
    m.model.validate().unwrap();
    let moved = m.model.items.iter().find(|i| i.id == victim.id).unwrap();
    assert_ne!((moved.container, moved.slot), (victim.container, victim.slot));
    assert!(moved.container.is_none() && (23..=66).contains(&moved.slot));
    assert_eq!(m.items.relocated, vec![victim.id]);
    let kept = m.model.items.iter().find(|i| i.id == occupant.id).unwrap();
    assert_eq!((kept.container, kept.slot), (occupant.container, occupant.slot), "the session's item keeps its place");
}

#[test]
fn pets_are_matched_by_their_portable_id() {
    let c0 = c0();
    let b0 = c0.clone();
    let mut b1 = b0.clone();
    let mut second = b1.pets[0].clone();
    second.id = PortablePetId::new();
    second.name = "Newcomer".into();
    second.slot = 1;
    b1.pets.push(second.clone());
    b1.pets[0].name = "Renamed".into();
    let m = merge3(&c0, &b0, &b1.normalized(), Mode::Lenient).unwrap();
    assert_eq!(m.pets.added, vec![second.id]);
    assert_eq!(m.pets.changed, vec![c0.pets[0].id]);
    assert!(m.model.pets.iter().any(|p| p.name == "Renamed") && m.model.pets.iter().any(|p| p.name == "Newcomer"));
    // abandoned pet
    let mut b2 = b0.clone();
    b2.pets.clear();
    let m = merge3(&c0, &b0, &b2, Mode::Lenient).unwrap();
    assert_eq!(m.pets.removed, vec![c0.pets[0].id]);
    assert!(m.model.pets.is_empty());
    // a pet the realm filtered away is kept
    let mut b0f = c0.clone();
    b0f.pets.clear();
    let m = merge3(&c0, &b0f, &b0f, Mode::Lenient).unwrap();
    assert_eq!(m.pets.filtered_kept, vec![c0.pets[0].id]);
    assert_eq!(m.model.pets.len(), 1);
}

#[test]
fn quarantined_extensions_and_stock_talents_are_never_touched_by_a_realm() {
    let mut c0 = c0();
    c0.extensions.insert("coa:unlisted-settings".into(), Extension::new("2", 1, b"{}".to_vec()));
    c0.build.talents = vec![(900_001, 1)];
    let b0 = c0.clone();
    let mut b1 = b0.clone();
    b1.extensions.clear();
    b1.build.talents.clear();
    let m = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap().model;
    assert!(m.extensions.contains_key("coa:unlisted-settings"));
    assert_eq!(m.build.talents, vec![(900_001, 1)]);
}

#[test]
fn a_session_never_changes_who_the_character_is() {
    let c0 = c0();
    let (b0, _) = normalized_by_the_realm(&c0);
    let m = merge3(&c0, &b0, &b0, Mode::Lenient).unwrap().model;
    assert_eq!((m.character_id, m.ruleset, m.content_namespace.as_str()), (c0.character_id, c0.ruleset, c0.content_namespace.as_str()));
    let mut wild = b0.clone();
    wild.ruleset = Ruleset::Wildcard;
    assert!(merge3(&c0, &b0, &wild, Mode::Lenient).is_err(), "CoA and Wildcard never mix");
}

// ---- the other direction: canonical -> realm, strict ------------------------------------------------------------------------

#[test]
fn strict_mode_reports_only_real_conflicts() {
    let s = c0(); // what the realm was last synced with
    let mut canonical = s.clone();
    canonical.progression.money += 500;
    canonical.progression.chosen_title = 9;
    canonical.build.spells.push((133, 255));
    let canonical = canonical.normalized();

    // the realm drifted on its own in other places: no conflict, its drift is kept
    let mut realm = s.clone();
    realm.progression.honor.today_honor = 0;
    realm.build.spells.push((6654, 255));
    realm.progression.money += 7; // an accumulator: both deltas add
    let m = merge3(&realm.clone().normalized(), &s, &canonical, Mode::Strict).unwrap();
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    assert_eq!(m.model.progression.money, s.progression.money + 7 + 500);
    assert_eq!(m.model.progression.chosen_title, 9);
    assert!(m.model.build.spells.contains(&(6654, 255)) && m.model.build.spells.contains(&(133, 255)));
    assert_eq!(m.model.progression.honor.today_honor, 0, "the realm's own value is kept");

    // both sides moved the same title elsewhere: a conflict, and nothing is applied for it
    let mut realm = s.clone();
    realm.progression.chosen_title = 4;
    let m = merge3(&realm.normalized(), &s, &canonical, Mode::Strict).unwrap();
    assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
    assert_eq!(m.conflicts[0].path, "progression.chosen_title");
    assert_eq!(m.model.progression.chosen_title, 4);

    // both sides ended on the same value: no conflict
    let mut realm = s.clone();
    realm.progression.chosen_title = 9;
    assert!(merge3(&realm.normalized(), &s, &canonical, Mode::Strict).unwrap().conflicts.is_empty());

    // canonical removed a spell the realm changed
    let mut canonical2 = s.clone();
    let victim = s.build.spells[3].0;
    canonical2.build.spells.retain(|(x, _)| *x != victim);
    let mut realm = s.clone();
    realm.build.spells.iter_mut().find(|(x, _)| *x == victim).unwrap().1 = 255;
    let m = merge3(&realm.normalized(), &s, &canonical2.normalized(), Mode::Strict).unwrap();
    assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
}

#[test]
fn items_filtered_by_the_target_are_not_resurrected_by_an_update() {
    let s = c0();
    let filtered_at_realm = plain_items(&s)[0];
    let mut realm = s.clone();
    realm.items.retain(|i| i.id != filtered_at_realm); // this realm never had it
    let mut canonical = s.clone();
    canonical.items.iter_mut().find(|i| i.id == filtered_at_realm).unwrap().count = 9; // the player changed it elsewhere
    canonical.items.push(new_item(70, 35));
    let m = merge3(&realm.normalized(), &s, &canonical.normalized(), Mode::Strict).unwrap();
    assert!(!m.model.items.iter().any(|i| i.id == filtered_at_realm), "an update does not put a filtered item into a realm");
    assert_eq!(m.items.added.len(), 1);
    assert_eq!(m.items.realm_local, vec![filtered_at_realm]);
}

// ---- properties -------------------------------------------------------------------------------------------------------------

/// Whatever the realm does by itself, a session without progress changes nothing.
#[test]
fn random_realm_normalisation_never_leaks_into_the_canonical_character() {
    let mut x = 0x9E37_79B9u32;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    for round in 0..300 {
        let c0 = c0();
        let mut b0 = c0.clone();
        for _ in 0..1 + next() % 6 {
            match next() % 9 {
                0 => b0.build.spells.push((10_000 + next() % 5_000, 255)),
                1 => {
                    let n = next() as usize % b0.build.spells.len();
                    b0.build.spells.remove(n);
                }
                2 => b0.progression.honor.today_honor = next() % 50,
                3 => b0.progression.known_currencies ^= 1 << (next() % 60),
                4 => b0.reputation.push(ReputationEntry { faction: 20_000 + next() % 100, standing: 0, flags: 1 }),
                5 => {
                    let n = next() as usize % b0.items.len();
                    if b0.items[n].container.is_none() && b0.items.iter().all(|i| i.container != Some(b0.items[n].id)) {
                        b0.items.remove(n);
                    }
                }
                6 => b0.items.push(new_item(1_000 + round, 40 + (next() % 20) as u8)),
                7 => {
                    b0.settings.insert(format!("core.ascension_slot.{}", next() % 5), vec![next() % 9]);
                }
                _ => b0.progression.taxi_mask = format!("{} ", next()),
            }
        }
        let b0 = b0.normalized();
        let m = merge3(&c0, &b0, &b0, Mode::Lenient).unwrap();
        assert_eq!(m.model, c0, "round {round}");
        assert!(m.changes.is_empty(), "round {round}: {:?}", m.changes);
    }
}

// ---- Phase 6: the selected appearances travel with the character and never touch an item ---------------------------------

fn with_wardrobe(mut m: PortableCharacter, active: &[(u8, u32)], outfits: &[(&str, &[u32])]) -> PortableCharacter {
    m.wardrobe.active = active.iter().copied().collect();
    m.wardrobe.outfits = outfits.iter().map(|(n, ids)| (n.to_string(), ids.to_vec())).collect();
    m.normalized()
}

#[test]
fn a_selected_appearance_made_in_a_session_reaches_the_canonical_character_without_touching_an_item() {
    let c0 = with_wardrobe(c0(), &[(1, 100)], &[]);
    let (b0, _) = normalized_by_the_realm(&c0);
    let mut b1 = b0.clone();
    b1.wardrobe.active.insert(3, 303);
    b1.wardrobe.active.insert(56, 5600);
    b1.wardrobe.outfits.insert("Sunday".into(), vec![100, 0, 303]);
    b1.wardrobe.can_see_spell = false;
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(merged.model.wardrobe.active, [(1, 100), (3, 303), (56, 5600)].into_iter().collect());
    assert_eq!(merged.model.wardrobe.outfits["Sunday"], vec![100, 0, 303]);
    assert!(merged.model.wardrobe.can_see_item && !merged.model.wardrobe.can_see_spell);
    assert!(merged.items.added.is_empty() && merged.items.removed.is_empty() && merged.items.changed.is_empty());
    assert_eq!(merged.model.items, c0.items, "reconciling an appearance can never create, change or delete a gameplay item");
}

#[test]
fn an_appearance_the_realm_does_not_know_stays_canonical_through_any_number_of_sessions() {
    // the canonical character wears 999 and 1000, which the realm never showed (B0 and B1 lack them)
    let c0 = with_wardrobe(c0(), &[(1, 100), (2, 999)], &[("Far away", &[999, 1000])]);
    let (mut b0, _) = normalized_by_the_realm(&c0);
    b0.wardrobe = Default::default();
    let mut b1 = b0.clone();
    b1.wardrobe.active.insert(3, 303);
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(merged.model.wardrobe.active.get(&1), Some(&100));
    assert_eq!(merged.model.wardrobe.active.get(&2), Some(&999), "kept");
    assert_eq!(merged.model.wardrobe.active.get(&3), Some(&303), "and the session's choice is added");
    assert!(merged.model.wardrobe.outfits.contains_key("Far away"));
    // again, from the merged result as the next session's canonical character
    let again = merge3(&merged.model, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(again.model.wardrobe, merged.model.wardrobe, "repeatable");
}

#[test]
fn deselecting_and_deleting_in_a_session_removes_them_from_the_canonical_character() {
    let c0 = with_wardrobe(c0(), &[(1, 100), (3, 303)], &[("A", &[100]), ("B", &[303])]);
    let b0 = c0.clone();
    let mut b1 = b0.clone();
    b1.wardrobe.active.remove(&3);
    b1.wardrobe.outfits.remove("B");
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(merged.model.wardrobe.active, [(1, 100)].into_iter().collect());
    assert_eq!(merged.model.wardrobe.outfits.keys().collect::<Vec<_>>(), ["A"]);
}

#[test]
fn an_update_conflicts_only_where_both_sides_changed_the_same_category_differently() {
    // target = the realm now, base = what it was synced with, ours = the new canonical character
    let base = with_wardrobe(c0(), &[(1, 100), (3, 303)], &[]);
    let mut target = base.clone();
    target.wardrobe.active.insert(1, 101);
    target.wardrobe.active.insert(9, 909);
    let mut ours = base.clone();
    ours.wardrobe.active.insert(3, 304);
    let merged = merge3(&target, &base, &ours, Mode::Strict).unwrap();
    assert!(merged.conflicts.is_empty(), "{:?}", merged.conflicts);
    assert_eq!(merged.model.wardrobe.active, [(1, 101), (3, 304), (9, 909)].into_iter().collect(), "each side's own change survives");
    let mut ours = base.clone();
    ours.wardrobe.active.insert(1, 102);
    let merged = merge3(&target, &base, &ours, Mode::Strict).unwrap();
    assert_eq!(merged.conflicts.len(), 1);
    assert!(merged.conflicts[0].path.contains("wardrobe.active"));
}

#[test]
fn a_character_without_a_wardrobe_has_the_bytes_it_had_before_the_section_existed() {
    let model = c0();
    assert!(model.wardrobe.is_empty());
    let json = String::from_utf8(crate::portable::snapshot::canonical_json(&model).unwrap()).unwrap();
    assert!(!json.contains("wardrobe"), "an empty section is not serialised, so every older snapshot keeps its hash");
    let worn = with_wardrobe(model, &[(1, 100)], &[]);
    let json = String::from_utf8(crate::portable::snapshot::canonical_json(&worn).unwrap()).unwrap();
    assert!(json.contains("\"wardrobe\":{\"active\":{\"1\":100}}"), "{json}");
    let back: PortableCharacter = serde_json::from_str(&json).unwrap();
    assert_eq!(back, worn);
}

// ---- Phase 7: module extensions ----------------------------------------------------------------------------------------------

fn ext(format: u32, bytes: &[u8]) -> Extension {
    Extension::new("1.0", format, bytes.to_vec())
}

#[test]
fn an_extension_nobody_on_this_realm_understands_survives_any_session() {
    let mut c0 = c0();
    c0.extensions.insert("mod:somebody-elses".into(), ext(5, b"opaque payload of another module"));
    c0.extensions.insert("coa:unlisted-settings".into(), ext(1, b"{}"));
    let c0 = c0.normalized();
    let (b0, _) = normalized_by_the_realm(&c0);
    let mut b0 = b0;
    b0.extensions.clear();
    let mut b1 = b0.clone();
    b1.progression.money += 10;
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(merged.model.extensions, c0.extensions, "the realm exported none of them, so none of them is touched");
    assert_eq!(merged.model.progression.money, c0.progression.money + 10);
}

#[test]
fn what_a_realms_adapter_exports_is_carried_and_a_change_replaces_it_but_the_managers_own_blobs_are_never_contributed() {
    let c0 = c0();
    let mut b0 = c0.clone();
    b0.extensions.insert("mod:fake".into(), ext(2, b"one"));
    b0.extensions.insert("coa:unlisted-settings".into(), ext(1, b"realm side"));
    let mut b1 = b0.clone();
    b1.extensions.insert("mod:fake".into(), ext(2, b"two"));
    b1.extensions.insert("coa:unlisted-settings".into(), ext(1, b"realm side changed"));
    let merged = merge3(&c0, &b0, &b1, Mode::Lenient).unwrap();
    assert_eq!(merged.model.extensions["mod:fake"].payload.0, b"two");
    assert!(!merged.model.extensions.contains_key("coa:unlisted-settings"), "the Manager's own blobs are canonical-only");

    // an unchanged export is not a change (the realm's own representation of what the canonical character already had)
    let mut c0b = c0.clone();
    c0b.extensions.insert("mod:fake".into(), ext(2, b"canonical"));
    let mut b0 = c0.clone();
    b0.extensions.insert("mod:fake".into(), ext(2, b"as the realm rewrote it"));
    let merged = merge3(&c0b, &b0, &b0.clone(), Mode::Lenient).unwrap();
    assert_eq!(merged.model.extensions["mod:fake"].payload.0, b"canonical");

    // a realm that deleted its module data says so: B0 had it, B1 does not
    let mut b1 = b0.clone();
    b1.extensions.remove("mod:fake");
    let merged = merge3(&c0b, &b0, &b1, Mode::Lenient).unwrap();
    assert!(!merged.model.extensions.contains_key("mod:fake"));
}
