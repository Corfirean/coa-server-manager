//! Deterministic test characters. Ids are fixed UUIDv7 values so two calls build equal characters.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::ids::{CharacterId, ContentId, PortableItemId, PortablePetId};
use super::model::*;
use super::versions::PORTABLE_CHARACTER_FORMAT_VERSION;

pub fn id7(n: u64) -> Uuid {
    let mut random = [0u8; 10];
    random[..8].copy_from_slice(&n.to_be_bytes());
    uuid::Builder::from_unix_timestamp_millis(1_790_000_000_000 + n, &random).into_uuid()
}

pub fn character_id(n: u64) -> CharacterId {
    CharacterId::from_uuid(id7(n)).unwrap()
}

fn item_id(n: u64) -> PortableItemId {
    PortableItemId::from_uuid(id7(10_000 + n)).unwrap()
}

fn content(kind: &str, id: u64) -> ContentId {
    ContentId::new("coa", kind, id).unwrap()
}

fn empty_progression(level: u8) -> Progression {
    Progression {
        level,
        xp: 0,
        money: 0,
        honor: Honor {
            arena_points: 0,
            total_honor: 0,
            today_honor: 0,
            yesterday_honor: 0,
            total_kills: 0,
            today_kills: 0,
            yesterday_kills: 0,
        },
        known_currencies: 0,
        chosen_title: 0,
        known_titles: "0 0 0 0 0 0".into(),
        explored_zones: "0 0 0 0".into(),
        taxi_mask: "0 0 0 0".into(),
        bank_slots: 0,
        stable_slots: 0,
        watched_faction: 0,
        action_bars_mask: 0,
    }
}

fn empty_build() -> Build {
    Build {
        spells: Vec::new(),
        talents: Vec::new(),
        skills: Vec::new(),
        glyphs: Vec::new(),
        talent_groups_count: 1,
        active_talent_group: 0,
        extra_bonus_talent_count: 0,
        reset_talents_cost: 0,
    }
}

pub fn naked_level_one() -> PortableCharacter {
    PortableCharacter {
        format_version: PORTABLE_CHARACTER_FORMAT_VERSION,
        character_id: character_id(1),
        ruleset: Ruleset::Coa,
        content_namespace: "coa".into(),
        identity: Identity {
            name: "Newbie".into(),
            race: content("race", 1),
            class: content("class", 12),
            gender: 0,
            appearance: Appearance {
                skin: 1,
                face: 2,
                hair_style: 3,
                hair_color: 4,
                facial_style: 0,
            },
            cosmetic_flags: 0,
        },
        progression: empty_progression(1),
        build: empty_build(),
        items: Vec::new(),
        quests: Quests {
            active: Vec::new(),
            rewarded: Vec::new(),
        },
        reputation: Vec::new(),
        actions: Vec::new(),
        pets: Vec::new(),
        settings: BTreeMap::new(),
        wardrobe: Default::default(),
        client_data: BTreeMap::new(),
        extensions: BTreeMap::new(),
    }
}

fn simple_item(
    n: u64,
    container: Option<PortableItemId>,
    slot: u8,
    entry: u64,
    count: u32,
) -> PortableItem {
    PortableItem {
        id: item_id(n),
        container,
        slot,
        entry: content("item", entry),
        count,
        duration: 0,
        charges: vec![0, 0, 0, 0, 0],
        flags: 1,
        enchantments: Vec::new(),
        random_property_id: 0,
        durability: 100,
        played_time: 3600,
        text: None,
        creator_name: None,
        gift: None,
    }
}

/// A level 80 character shaped like the real data of the audit: ~1,300 spells, 105 reputations, 23 skills,
/// equipment, four bags with contents, enchants, gems, a pet, CoA settings and macros.
pub fn geared_level_eighty() -> PortableCharacter {
    let mut c = naked_level_one();
    c.character_id = character_id(2);
    c.identity.name = "Geared".into();
    c.identity.class = content("class", 21);
    c.progression = empty_progression(80);
    c.progression.xp = 12_345;
    c.progression.money = 200_060_523;
    c.progression.honor.total_honor = 4092;
    c.progression.honor.arena_points = 150;
    c.progression.known_currencies = 0b1010_0101;
    c.progression.chosen_title = 5;
    c.progression.explored_zones = (0..128)
        .map(|i| (i * 7919u32).to_string())
        .collect::<Vec<_>>()
        .join(" ");
    c.progression.bank_slots = 4;

    c.build.spells = (1..=1284u32)
        .map(|n| (500_000 + n * 3, if n % 5 == 0 { 3 } else { 1 }))
        .collect();
    c.build.skills = (1..=23u32)
        .map(|n| Skill {
            skill: n * 11,
            value: 300 + n as u16,
            max: 450,
        })
        .collect();
    c.build.glyphs = vec![GlyphSet {
        talent_group: 0,
        glyphs: [1, 2, 3, 4, 5, 6],
    }];
    c.build.extra_bonus_talent_count = 2;

    // equipment 0..18
    let mut n = 0u64;
    let mut equipped_ids = Vec::new();
    for slot in 0..19u8 {
        n += 1;
        let mut item = simple_item(n, None, slot, 30_000 + n * 17, 1);
        item.flags = 1;
        item.durability = 90 + slot as u32;
        if slot % 3 == 0 {
            item.enchantments = vec![
                Enchantment {
                    slot: 0,
                    id: 3_000 + slot as u32,
                    duration: 0,
                    charges: 0,
                },
                Enchantment {
                    slot: 2,
                    id: 3_878,
                    duration: 0,
                    charges: 0,
                },
                Enchantment {
                    slot: 3,
                    id: 3_879,
                    duration: 0,
                    charges: 0,
                },
            ];
        }
        if slot == 5 {
            item.random_property_id = -57;
            item.creator_name = Some("Crafter".into());
        }
        equipped_ids.push(item.id);
        c.items.push(item);
    }
    // four bags (slots 19..22), each with six items
    for bag_slot in 19..23u8 {
        n += 1;
        let bag = simple_item(n, None, bag_slot, 41_000 + bag_slot as u64, 1);
        let bag_id = bag.id;
        c.items.push(bag);
        for slot in 0..6u8 {
            n += 1;
            c.items.push(simple_item(
                n,
                Some(bag_id),
                slot,
                50_000 + n,
                1 + (slot as u32 % 3) * 4,
            ));
        }
    }
    // backpack
    for slot in 23..31u8 {
        n += 1;
        c.items.push(simple_item(n, None, slot, 60_000 + n, 20));
    }
    // bank bag with a book that has text, and a currency token
    n += 1;
    let mut book = simple_item(n, None, 40, 70_001, 1);
    book.text = Some("A very short book.\nSecond line.".into());
    c.items.push(book);
    n += 1;
    let mut token = simple_item(n, None, 118, 375_250, 12);
    token.flags = 0;
    c.items.push(token);
    n += 1;
    let mut wrapped = simple_item(n, None, 41, 5_042, 1);
    wrapped.gift = Some(Gift {
        entry: content("item", 8_000),
        flags: 8,
    });
    c.items.push(wrapped);

    c.quests.active = vec![
        ActiveQuest {
            quest: 12_001,
            status: 3,
            explored: false,
            timer: 0,
            mob_counts: [3, 0, 0, 0],
            item_counts: [0; 6],
            player_count: 0,
        },
        ActiveQuest {
            quest: 12_002,
            status: 1,
            explored: true,
            timer: 600,
            mob_counts: [0; 4],
            item_counts: [2, 0, 0, 0, 0, 0],
            player_count: 0,
        },
    ];
    c.quests.rewarded = vec![12_000, 12_003, 12_004, 99_001];
    c.reputation = (1..=105u32)
        .map(|f| ReputationEntry {
            faction: f * 3,
            standing: (f as i32) * 100 - 3_000,
            flags: if f % 7 == 0 { 17 } else { 1 },
        })
        .collect();
    c.actions = (0..12u8)
        .map(|b| ActionButton {
            spec: 0,
            button: b,
            action: 500_003 + b as u32 * 3,
            kind: 0,
        })
        .collect();

    c.pets = vec![PortablePet {
        id: PortablePetId::from_uuid(id7(20_000)).unwrap(),
        entry: content("creature", 416),
        model_id: 4_449,
        created_by_spell: 688,
        pet_type: 0,
        level: 80,
        exp: 1_000,
        react_state: 1,
        name: "Fuzzy".into(),
        renamed: true,
        slot: 0,
        health: 5_000,
        mana: 800,
        happiness: 0,
        action_bar: "7 2 0 0 1 0 1 1 0".into(),
        spells: vec![
            PetSpell {
                spell: 3110,
                active: 1,
            },
            PetSpell {
                spell: 6307,
                active: 0,
            },
        ],
        declined_names: None,
    }];

    c.settings
        .insert("core.ascension_active_spec".into(), vec![54]);
    c.settings.insert("core.ascension_starter".into(), vec![1]);
    c.settings.insert(
        "core.ascension_build.54".into(),
        vec![3, 56_710, 56_320, 90_101],
    );
    c.settings.insert(
        "core.ascension_bar.54".into(),
        vec![2, 0, 500_003, 1, 500_006],
    );
    c.client_data.insert(
        5,
        ClientBlob {
            time: 1_790_000_000,
            data: Bytes(b"macro data \x00\x01\x02 with binary".to_vec()),
        },
    );
    c.normalized()
}
