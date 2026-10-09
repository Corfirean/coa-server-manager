//! The three-way merge of portable characters.
//!
//! `merge3(target, base, ours)` applies **what changed between `base` and `ours`** onto `target`, and nothing else. It is
//! the one engine behind both directions of a round trip:
//!
//! | use | target | base | ours |
//! |---|---|---|---|
//! | **session reconcile** (realm -> canonical) | `C0` canonical at join | `B0` the realm after its own first load/save normalisation, before any progression | `B1` the realm now |
//! | **in-place update** (canonical -> realm) | `A_now` the realm's current state | `S` the canonical snapshot the realm was last synced with | `C_new` the new canonical |
//!
//! Only the `base -> ours` delta is "progress". Whatever differs between `target` and `base` already (the realm's own
//! defaults, DBC removals, mailed items, daily honor resets; or, in the other direction, the realm's own additions) is
//! **left exactly as it is in `target`**: automatic normalisation never overwrites the character it normalised.
//!
//! Rules:
//! * scalars: `ours` wins where `base != ours`; otherwise `target` is kept;
//! * accumulators (money, honor totals, kills, arena points, xp inside a level): the *difference* `ours - base` is added;
//! * bit sets (known currencies, titles, explored zones, taxi nodes): bits gained are added, bits lost are removed;
//! * keyed collections (spells, skills, reputation, quests, action buttons, settings, selected appearances and outfits, ...): keys added in `ours` are added,
//!   keys removed in `ours` are removed, values changed in `ours` are replaced;
//! * items and pets are matched by their **portable id**: present in `target` and `base`: delta applied, or removed when
//!   `ours` lacks it (the player got rid of it); in `target` only (filtered away by the realm before `base`): **kept**, or
//!   taken from `ours` if it has come back; in `base` only (the realm's own addition / filtered at the target): ignored;
//!   new in `ours`: added.
//!
//! **Under a level-cap projection** ([`MergeProjection`]) two things change: the level and the xp of the working copy are the realm's own
//! (the cap, xp 0) and never reach the canonical character, and the build records the core rewrites at every save
//! (`core.ascension_slot.*`, `core.ascension_build.*`, `core.ascension_bar.*`) are merged **key by key** instead of as one value, so that
//! what the realm could not hold survives in the canonical record. Everything else is as above: what the projection holds is absent from
//! the realm's `B0` and `B1`, which is exactly "filtered away by the realm: kept".
//!
//! [`Mode::Strict`] additionally reports a **conflict** when `target` and `ours` both departed from `base` and disagree;
//! [`Mode::Lenient`] (sessions) takes `ours`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Debug;

use super::error::{PortableError, Result};
use super::ids::{PortableItemId, PortablePetId};
use super::model::*;
use super::projection::settings::{classify, Record};

/// How a projected character is merged: see the module documentation.
#[derive(Clone, Debug, Default)]
pub struct MergeProjection {
    /// The canonical level and xp stay as they are (a session reconciling a working copy into its canonical character).
    pub freeze_progression: bool,
    /// The realm's level and xp are those of a working copy (the character was projected there): when the new view moves them, the new view
    /// wins over whatever the player did to the working copy's level, instead of being a conflict.
    pub adopt_progression: bool,
    /// Build records the core could not take apart: the realm's value is ignored.
    pub blocked: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Take `ours` where it changed; never report conflicts.
    Lenient,
    /// Report a conflict where `target` and `ours` both changed the same thing differently.
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct Merged {
    pub model: PortableCharacter,
    /// What was applied (`base -> ours`).
    pub changes: Vec<String>,
    /// What differs between `target` and `base` and was therefore deliberately left alone.
    pub left_alone: Vec<String>,
    pub conflicts: Vec<Conflict>,
    pub items: ItemOutcome,
    pub pets: PetOutcome,
}

/// How the items fell out, by portable id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemOutcome {
    /// New in `ours`.
    pub added: Vec<PortableItemId>,
    /// Present in `target` and `base`, gone from `ours`: the player got rid of them.
    pub removed: Vec<PortableItemId>,
    /// Present in all three and different in `ours`.
    pub changed: Vec<PortableItemId>,
    /// In `target` only and still absent from `ours`: filtered away by the realm, kept.
    pub filtered_kept: Vec<PortableItemId>,
    /// In `target` only and back in `ours`.
    pub reappeared: Vec<PortableItemId>,
    /// In `base` only: the realm's own additions (or filtered at the target), never merged.
    pub realm_local: Vec<PortableItemId>,
    /// Items moved to a free slot because the place they were in was taken.
    pub relocated: Vec<PortableItemId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PetOutcome {
    pub added: Vec<PortablePetId>,
    pub removed: Vec<PortablePetId>,
    pub changed: Vec<PortablePetId>,
    pub filtered_kept: Vec<PortablePetId>,
    pub reappeared: Vec<PortablePetId>,
    pub realm_local: Vec<PortablePetId>,
}

struct Ctx {
    mode: Mode,
    conflicts: Vec<Conflict>,
    changes: Vec<String>,
    left_alone: Vec<String>,
}

fn short<T: Debug>(v: &T) -> String {
    let s = format!("{v:?}");
    if s.len() > 60 {
        format!("{}...", s.chars().take(57).collect::<String>())
    } else {
        s
    }
}

impl Ctx {
    fn conflict<T: Debug>(&mut self, path: &str, target: &T, base: &T, ours: &T) {
        self.conflicts.push(Conflict {
            path: path.to_string(),
            detail: format!(
                "target {} / base {} / ours {}",
                short(target),
                short(base),
                short(ours)
            ),
        });
    }

    /// `ours` wins where it moved away from `base`.
    fn scalar<T: PartialEq + Clone + Debug>(
        &mut self,
        path: &str,
        target: &mut T,
        base: &T,
        ours: &T,
    ) {
        if base == ours {
            if target != base {
                self.left_alone.push(format!(
                    "{path}: realm-side {} (was {})",
                    short(target),
                    short(base)
                ));
            }
            return;
        }
        if self.mode == Mode::Strict && target != base && target != ours {
            self.conflict(path, target, base, ours);
            return;
        }
        self.changes
            .push(format!("{path}: {} -> {}", short(base), short(ours)));
        *target = ours.clone();
    }

    /// `target + (ours - base)`, clamped to `0..=max`.
    fn add(&mut self, path: &str, target: &mut u64, base: u64, ours: u64, max: u64) {
        if base == ours {
            return;
        }
        let value = (*target as i128 + ours as i128 - base as i128).clamp(0, max as i128) as u64;
        self.changes.push(format!(
            "{path}: {} {:+} -> {value}",
            *target,
            ours as i128 - base as i128
        ));
        *target = value;
    }

    /// Bit sets in words: `(target | gained) & !lost`.
    fn bit_words(&mut self, path: &str, target: &mut String, base: &str, ours: &str) {
        if base == ours {
            return;
        }
        let parse = |s: &str| {
            s.split_whitespace()
                .map(|w| w.parse::<u64>())
                .collect::<std::result::Result<Vec<u64>, _>>()
        };
        let (Ok(t), Ok(b), Ok(o)) = (parse(target), parse(base), parse(ours)) else {
            return self.scalar(path, target, &base.to_string(), &ours.to_string());
        };
        let words = t.len().max(b.len()).max(o.len());
        let get = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
        let merged: Vec<u64> = (0..words)
            .map(|i| (get(&t, i) | (get(&o, i) & !get(&b, i))) & !(get(&b, i) & !get(&o, i)))
            .collect();
        let text: String = merged.iter().map(|w| format!("{w} ")).collect();
        if text.trim() != target.trim() {
            self.changes.push(format!("{path}: bits changed"));
        }
        *target = text;
    }

    /// Keyed values: added / removed / changed keys of `ours` relative to `base` are applied to `target`.
    fn map<K: Ord + Clone + Debug, V: PartialEq + Clone + Debug>(
        &mut self,
        path: &str,
        target: &mut BTreeMap<K, V>,
        base: &BTreeMap<K, V>,
        ours: &BTreeMap<K, V>,
    ) {
        for (k, o) in ours {
            match base.get(k) {
                None => {
                    // added in ours
                    if let Some(t) = target.get(k) {
                        if self.mode == Mode::Strict && t != o {
                            self.conflicts.push(Conflict {
                                path: format!("{path}[{k:?}]"),
                                detail: format!(
                                    "target {} / ours {} (not in base)",
                                    short(t),
                                    short(o)
                                ),
                            });
                            continue;
                        }
                    }
                    if target.get(k) != Some(o) {
                        self.changes.push(format!("{path}[{k:?}]: added"));
                    }
                    target.insert(k.clone(), o.clone());
                }
                Some(b) if b != o => {
                    if self.mode == Mode::Strict {
                        if let Some(t) = target.get(k) {
                            if t != b && t != o {
                                self.conflict(&format!("{path}[{k:?}]"), t, b, o);
                                continue;
                            }
                        }
                    }
                    self.changes
                        .push(format!("{path}[{k:?}]: {} -> {}", short(b), short(o)));
                    target.insert(k.clone(), o.clone());
                }
                Some(_) => {}
            }
        }
        for (k, b) in base {
            if ours.contains_key(k) {
                continue;
            }
            // removed in ours
            match target.get(k) {
                Some(t) if self.mode == Mode::Strict && t != b => self.conflicts.push(Conflict {
                    path: format!("{path}[{k:?}]"),
                    detail: format!("removed in ours but changed in target ({})", short(t)),
                }),
                Some(_) => {
                    self.changes.push(format!("{path}[{k:?}]: removed"));
                    target.remove(k);
                }
                None => {}
            }
        }
        // what the target has on its own (not in base): left alone, counted for the report
        let own = target
            .keys()
            .filter(|k| !base.contains_key(*k) && !ours.contains_key(*k))
            .count();
        let lost = base
            .keys()
            .filter(|k| !target.contains_key(*k) && ours.contains_key(*k))
            .count();
        if own > 0 {
            self.left_alone.push(format!(
                "{path}: {own} entr{} only on the target side",
                if own == 1 { "y" } else { "ies" }
            ));
        }
        if lost > 0 {
            self.left_alone.push(format!(
                "{path}: {lost} entr{} the target side does not have",
                if lost == 1 { "y" } else { "ies" }
            ));
        }
    }

    /// The settings of a projected character: build records key by key, blocked ones untouched, the rest as one value each.
    fn projected_settings(
        &mut self,
        target: &mut BTreeMap<String, Vec<u32>>,
        base: &BTreeMap<String, Vec<u32>>,
        ours: &BTreeMap<String, Vec<u32>>,
        projection: &MergeProjection,
    ) {
        let special = |k: &String| classify(k).is_some() || projection.blocked.contains(k);
        let plain = |m: &BTreeMap<String, Vec<u32>>| -> BTreeMap<String, Vec<u32>> {
            m.iter()
                .filter(|(k, _)| !special(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut rest = plain(target);
        self.map("settings", &mut rest, &plain(base), &plain(ours));
        target.retain(|k, _| special(k));
        target.extend(rest);

        let sources: BTreeSet<&String> = ours.keys().filter(|k| special(k)).collect();
        for source in sources {
            if projection.blocked.contains(source) {
                self.left_alone.push(format!(
                    "settings[{source:?}]: not merged, the projection could not take it apart"
                ));
                continue;
            }
            let Some(kind) = classify(source) else {
                continue;
            };
            let (Some(o), t) = (ours.get(source), target.get(source)) else {
                continue;
            };
            let Some(ours_record) = Record::parse(kind, o) else {
                self.left_alone.push(format!(
                    "settings[{source:?}]: the realm's value is not a build record, left alone"
                ));
                continue;
            };
            let Some(t) = t else {
                self.changes.push(format!("settings[{source:?}]: added"));
                target.insert(source.clone(), ours_record.write());
                continue;
            };
            let Some(mut merged) = Record::parse(kind, t) else {
                self.left_alone.push(format!(
                    "settings[{source:?}]: the canonical value is not a build record, left alone"
                ));
                continue;
            };
            let base_record = base
                .get(source)
                .and_then(|b| Record::parse(kind, b))
                .unwrap_or_else(|| {
                    Record::parse(kind, &empty_record(kind)).expect("an empty record parses")
                });
            self.scalar(
                &format!("settings[{source:?}].head"),
                &mut merged.head,
                &base_record.head,
                &ours_record.head,
            );
            self.map(
                &format!("settings[{source:?}].entries"),
                &mut merged.entries,
                &base_record.entries,
                &ours_record.entries,
            );
            self.map(
                &format!("settings[{source:?}].buttons"),
                &mut merged.buttons,
                &base_record.buttons,
                &ours_record.buttons,
            );
            let written = merged.write();
            if Record::parse(kind, t).map(|r| r.write()).as_ref() != Some(&written) {
                target.insert(source.clone(), written);
            }
        }
    }

    fn set<K: Ord + Clone + Debug>(
        &mut self,
        path: &str,
        target: &mut BTreeSet<K>,
        base: &BTreeSet<K>,
        ours: &BTreeSet<K>,
    ) {
        let mut t: BTreeMap<K, ()> = target.iter().map(|k| (k.clone(), ())).collect();
        let b: BTreeMap<K, ()> = base.iter().map(|k| (k.clone(), ())).collect();
        let o: BTreeMap<K, ()> = ours.iter().map(|k| (k.clone(), ())).collect();
        self.map(path, &mut t, &b, &o);
        *target = t.into_keys().collect();
    }
}

fn to_map<T, K: Ord, V>(items: &[T], f: impl Fn(&T) -> (K, V)) -> BTreeMap<K, V> {
    items.iter().map(f).collect()
}

fn empty_record(kind: super::projection::settings::SettingKind) -> Vec<u32> {
    use super::projection::settings::SettingKind;
    match kind {
        SettingKind::Slot => vec![1, 12, 0, 0, 0],
        SettingKind::Build | SettingKind::Bar => vec![0],
    }
}

/// Apply `base -> ours` onto `target`.
pub fn merge3(
    target: &PortableCharacter,
    base: &PortableCharacter,
    ours: &PortableCharacter,
    mode: Mode,
) -> Result<Merged> {
    merge3_with(target, base, ours, mode, None)
}

/// [`merge3`], optionally of a projected character.
pub fn merge3_with(
    target: &PortableCharacter,
    base: &PortableCharacter,
    ours: &PortableCharacter,
    mode: Mode,
    projection: Option<&MergeProjection>,
) -> Result<Merged> {
    if target.ruleset != base.ruleset || target.ruleset != ours.ruleset {
        return Err(PortableError::Invalid(
            "three characters of different rulesets cannot be merged".into(),
        ));
    }
    let mut cx = Ctx {
        mode,
        conflicts: Vec::new(),
        changes: Vec::new(),
        left_alone: Vec::new(),
    };
    let mut out = target.clone();

    // ---- identity ------------------------------------------------------------------------------------------------
    cx.scalar(
        "identity.name",
        &mut out.identity.name,
        &base.identity.name,
        &ours.identity.name,
    );
    cx.scalar(
        "identity.race",
        &mut out.identity.race,
        &base.identity.race,
        &ours.identity.race,
    );
    cx.scalar(
        "identity.class",
        &mut out.identity.class,
        &base.identity.class,
        &ours.identity.class,
    );
    cx.scalar(
        "identity.gender",
        &mut out.identity.gender,
        &base.identity.gender,
        &ours.identity.gender,
    );
    cx.scalar(
        "identity.appearance",
        &mut out.identity.appearance,
        &base.identity.appearance,
        &ours.identity.appearance,
    );
    cx.scalar(
        "identity.cosmetic_flags",
        &mut out.identity.cosmetic_flags,
        &base.identity.cosmetic_flags,
        &ours.identity.cosmetic_flags,
    );

    // ---- progression ---------------------------------------------------------------------------------------------
    let (t, b, o) = (&mut out.progression, &base.progression, &ours.progression);
    let leveled = b.level != o.level;
    if projection.is_some_and(|p| p.freeze_progression) {
        if b.level != o.level || b.xp != o.xp {
            cx.left_alone.push(format!("progression: the realm's level {} xp {} is the working copy's own and never reaches the canonical character", o.level, o.xp));
        }
    } else if projection.is_some_and(|p| p.adopt_progression)
        && (b.level != o.level || b.xp != o.xp)
    {
        cx.changes.push(format!("progression: level {} xp {} -> level {} xp {} (the realm's working copy takes the new view's)", t.level, t.xp, o.level, o.xp));
        t.level = o.level;
        t.xp = o.xp;
    } else {
        cx.scalar("progression.level", &mut t.level, &b.level, &o.level);
        if leveled {
            cx.scalar("progression.xp", &mut t.xp, &b.xp, &o.xp);
        } else {
            let mut xp = t.xp as u64;
            cx.add(
                "progression.xp",
                &mut xp,
                b.xp as u64,
                o.xp as u64,
                u32::MAX as u64,
            );
            t.xp = xp as u32;
        }
    }
    macro_rules! accumulate {
        ($path:literal, $field:expr, $b:expr, $o:expr, $max:expr) => {{
            let mut v = $field as u64;
            cx.add($path, &mut v, $b as u64, $o as u64, $max as u64);
            $field = v as _;
        }};
    }
    accumulate!(
        "progression.money",
        t.money,
        b.money,
        o.money,
        limits::MAX_MONEY
    );
    accumulate!(
        "progression.honor.arena_points",
        t.honor.arena_points,
        b.honor.arena_points,
        o.honor.arena_points,
        u32::MAX
    );
    accumulate!(
        "progression.honor.total_honor",
        t.honor.total_honor,
        b.honor.total_honor,
        o.honor.total_honor,
        u32::MAX
    );
    accumulate!(
        "progression.honor.total_kills",
        t.honor.total_kills,
        b.honor.total_kills,
        o.honor.total_kills,
        u32::MAX
    );
    // the daily counters are reset by the realm at midnight: the realm's current value is the truth when it moved
    cx.scalar(
        "progression.honor.today_honor",
        &mut t.honor.today_honor,
        &b.honor.today_honor,
        &o.honor.today_honor,
    );
    cx.scalar(
        "progression.honor.yesterday_honor",
        &mut t.honor.yesterday_honor,
        &b.honor.yesterday_honor,
        &o.honor.yesterday_honor,
    );
    cx.scalar(
        "progression.honor.today_kills",
        &mut t.honor.today_kills,
        &b.honor.today_kills,
        &o.honor.today_kills,
    );
    cx.scalar(
        "progression.honor.yesterday_kills",
        &mut t.honor.yesterday_kills,
        &b.honor.yesterday_kills,
        &o.honor.yesterday_kills,
    );
    {
        let (mut tv, bv, ov) = (t.known_currencies, b.known_currencies, o.known_currencies);
        if bv != ov {
            let merged = (tv | (ov & !bv)) & !(bv & !ov);
            if merged != tv {
                cx.changes.push(format!(
                    "progression.known_currencies: bits {bv:#x} -> {ov:#x}"
                ));
            }
            tv = merged;
        }
        t.known_currencies = tv;
    }
    cx.scalar(
        "progression.chosen_title",
        &mut t.chosen_title,
        &b.chosen_title,
        &o.chosen_title,
    );
    cx.bit_words(
        "progression.known_titles",
        &mut t.known_titles,
        &b.known_titles,
        &o.known_titles,
    );
    cx.bit_words(
        "progression.explored_zones",
        &mut t.explored_zones,
        &b.explored_zones,
        &o.explored_zones,
    );
    cx.bit_words(
        "progression.taxi_mask",
        &mut t.taxi_mask,
        &b.taxi_mask,
        &o.taxi_mask,
    );
    cx.scalar(
        "progression.bank_slots",
        &mut t.bank_slots,
        &b.bank_slots,
        &o.bank_slots,
    );
    cx.scalar(
        "progression.stable_slots",
        &mut t.stable_slots,
        &b.stable_slots,
        &o.stable_slots,
    );
    cx.scalar(
        "progression.watched_faction",
        &mut t.watched_faction,
        &b.watched_faction,
        &o.watched_faction,
    );
    cx.scalar(
        "progression.action_bars_mask",
        &mut t.action_bars_mask,
        &b.action_bars_mask,
        &o.action_bars_mask,
    );

    // ---- build ---------------------------------------------------------------------------------------------------
    {
        let mut spells = to_map(&out.build.spells, |(s, m)| (*s, *m));
        cx.map(
            "build.spells",
            &mut spells,
            &to_map(&base.build.spells, |(s, m)| (*s, *m)),
            &to_map(&ours.build.spells, |(s, m)| (*s, *m)),
        );
        out.build.spells = spells.into_iter().collect();

        let mut skills = to_map(&out.build.skills, |s| (s.skill, (s.value, s.max)));
        cx.map(
            "build.skills",
            &mut skills,
            &to_map(&base.build.skills, |s| (s.skill, (s.value, s.max))),
            &to_map(&ours.build.skills, |s| (s.skill, (s.value, s.max))),
        );
        out.build.skills = skills
            .into_iter()
            .map(|(skill, (value, max))| Skill { skill, value, max })
            .collect();

        let mut glyphs = to_map(&out.build.glyphs, |g| (g.talent_group, g.glyphs));
        cx.map(
            "build.glyphs",
            &mut glyphs,
            &to_map(&base.build.glyphs, |g| (g.talent_group, g.glyphs)),
            &to_map(&ours.build.glyphs, |g| (g.talent_group, g.glyphs)),
        );
        out.build.glyphs = glyphs
            .into_iter()
            .map(|(talent_group, glyphs)| GlyphSet {
                talent_group,
                glyphs,
            })
            .collect();

        // stock talents are never written to a realm (see PORTABLE_IMPORT.md): they are canonical-only and never merged
        cx.scalar(
            "build.talent_groups_count",
            &mut out.build.talent_groups_count,
            &base.build.talent_groups_count,
            &ours.build.talent_groups_count,
        );
        cx.scalar(
            "build.active_talent_group",
            &mut out.build.active_talent_group,
            &base.build.active_talent_group,
            &ours.build.active_talent_group,
        );
        cx.scalar(
            "build.extra_bonus_talent_count",
            &mut out.build.extra_bonus_talent_count,
            &base.build.extra_bonus_talent_count,
            &ours.build.extra_bonus_talent_count,
        );
        cx.scalar(
            "build.reset_talents_cost",
            &mut out.build.reset_talents_cost,
            &base.build.reset_talents_cost,
            &ours.build.reset_talents_cost,
        );
    }

    // ---- quests, reputation, actions, settings, macros ---------------------------------------------------------------
    {
        let mut active = to_map(&out.quests.active, |q| (q.quest, q.clone()));
        cx.map(
            "quests.active",
            &mut active,
            &to_map(&base.quests.active, |q| (q.quest, q.clone())),
            &to_map(&ours.quests.active, |q| (q.quest, q.clone())),
        );
        out.quests.active = active.into_values().collect();

        let mut rewarded: BTreeSet<u32> = out.quests.rewarded.iter().copied().collect();
        cx.set(
            "quests.rewarded",
            &mut rewarded,
            &base.quests.rewarded.iter().copied().collect(),
            &ours.quests.rewarded.iter().copied().collect(),
        );
        out.quests.rewarded = rewarded.into_iter().collect();

        let mut rep = to_map(&out.reputation, |r| (r.faction, (r.standing, r.flags)));
        cx.map(
            "reputation",
            &mut rep,
            &to_map(&base.reputation, |r| (r.faction, (r.standing, r.flags))),
            &to_map(&ours.reputation, |r| (r.faction, (r.standing, r.flags))),
        );
        out.reputation = rep
            .into_iter()
            .map(|(faction, (standing, flags))| ReputationEntry {
                faction,
                standing,
                flags,
            })
            .collect();

        let mut actions = to_map(&out.actions, |a| ((a.spec, a.button), (a.action, a.kind)));
        cx.map(
            "actions",
            &mut actions,
            &to_map(&base.actions, |a| ((a.spec, a.button), (a.action, a.kind))),
            &to_map(&ours.actions, |a| ((a.spec, a.button), (a.action, a.kind))),
        );
        out.actions = actions
            .into_iter()
            .map(|((spec, button), (action, kind))| ActionButton {
                spec,
                button,
                action,
                kind,
            })
            .collect();

        match projection {
            Some(p) => cx.projected_settings(&mut out.settings, &base.settings, &ours.settings, p),
            None => cx.map(
                "settings",
                &mut out.settings,
                &base.settings,
                &ours.settings,
            ),
        }
        cx.map(
            "wardrobe.active",
            &mut out.wardrobe.active,
            &base.wardrobe.active,
            &ours.wardrobe.active,
        );
        cx.map(
            "wardrobe.outfits",
            &mut out.wardrobe.outfits,
            &base.wardrobe.outfits,
            &ours.wardrobe.outfits,
        );
        cx.scalar(
            "wardrobe.can_see_item",
            &mut out.wardrobe.can_see_item,
            &base.wardrobe.can_see_item,
            &ours.wardrobe.can_see_item,
        );
        cx.scalar(
            "wardrobe.can_see_spell",
            &mut out.wardrobe.can_see_spell,
            &base.wardrobe.can_see_spell,
            &ours.wardrobe.can_see_spell,
        );
        cx.map(
            "client_data",
            &mut out.client_data,
            &base.client_data,
            &ours.client_data,
        );
    }
    // Module extensions: a realm contributes only what an adapter of its own exported (its B0 and B1 hold nothing else), and only
    // what changed between them; everything else of the canonical character, unknown namespaces included, is left exactly as it is.
    // The Manager's own `coa:*` blobs (quarantined settings) are canonical-only: a realm never contributes to them.
    {
        let module = |m: &BTreeMap<String, Extension>| -> BTreeMap<String, Extension> {
            m.iter()
                .filter(|(k, _)| super::extension::is_module_namespace(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        let mut merged = module(&out.extensions);
        cx.map(
            "extensions",
            &mut merged,
            &module(&base.extensions),
            &module(&ours.extensions),
        );
        out.extensions
            .retain(|k, _| !super::extension::is_module_namespace(k));
        out.extensions.extend(merged);
    }

    // ---- items and pets ------------------------------------------------------------------------------------------------
    let items = merge_items(&mut cx, &mut out, target, base, ours)?;
    let pets = merge_pets(&mut cx, &mut out, target, base, ours);

    out = out.normalized();
    Ok(Merged {
        model: out,
        changes: cx.changes,
        left_alone: cx.left_alone,
        conflicts: cx.conflicts,
        items,
        pets,
    })
}

fn merge_items(
    cx: &mut Ctx,
    out: &mut PortableCharacter,
    target: &PortableCharacter,
    base: &PortableCharacter,
    ours: &PortableCharacter,
) -> Result<ItemOutcome> {
    let by_id = |m: &PortableCharacter| -> HashMap<PortableItemId, PortableItem> {
        m.items.iter().map(|i| (i.id, i.clone())).collect()
    };
    let (t, b, o) = (by_id(target), by_id(base), by_id(ours));
    let mut result: HashMap<PortableItemId, PortableItem> = t.clone();
    let mut outcome = ItemOutcome::default();
    let mut touched: HashSet<PortableItemId> = HashSet::new();

    for (id, titem) in &t {
        match (b.get(id), o.get(id)) {
            (Some(bitem), Some(oitem)) => {
                let merged = merge_item(cx, titem, bitem, oitem, &id.to_string());
                if &merged != titem {
                    outcome.changed.push(*id);
                    touched.insert(*id);
                    cx.changes.push(format!("item {id}: changed"));
                }
                result.insert(*id, merged);
            }
            (Some(_), None) => {
                // the target and the base had it, `ours` does not: the player got rid of it
                outcome.removed.push(*id);
                cx.changes
                    .push(format!("item {id}: removed ({})", titem.entry));
                result.remove(id);
            }
            (None, Some(oitem)) => {
                // only the target knew it (filtered away by the realm before the base) and it has come back
                let mut back = oitem.clone();
                back.creator_name = titem.creator_name.clone();
                result.insert(*id, back);
                outcome.reappeared.push(*id);
                touched.insert(*id);
                cx.changes
                    .push(format!("item {id}: reappeared ({})", titem.entry));
            }
            (None, None) => outcome.filtered_kept.push(*id),
        }
    }
    for (id, oitem) in &o {
        if t.contains_key(id) || b.contains_key(id) {
            continue;
        }
        result.insert(*id, oitem.clone());
        outcome.added.push(*id);
        touched.insert(*id);
        cx.changes.push(format!("item {id}: new ({})", oitem.entry));
    }
    outcome.realm_local = b
        .keys()
        .filter(|id| !t.contains_key(*id))
        .copied()
        .collect();
    if !outcome.filtered_kept.is_empty() {
        cx.left_alone.push(format!(
            "items: {} item(s) filtered away by the realm stay canonical",
            outcome.filtered_kept.len()
        ));
    }
    if !outcome.realm_local.is_empty() {
        cx.left_alone.push(format!(
            "items: {} item(s) only on the realm side are not merged",
            outcome.realm_local.len()
        ));
    }

    let mut list: Vec<PortableItem> = result.into_values().collect();
    outcome.relocated = settle_places(&mut list, &touched)?;
    out.items = list;
    for v in [
        &mut outcome.added,
        &mut outcome.removed,
        &mut outcome.changed,
        &mut outcome.filtered_kept,
        &mut outcome.reappeared,
        &mut outcome.realm_local,
        &mut outcome.relocated,
    ] {
        v.sort();
    }
    Ok(outcome)
}

fn merge_item(
    cx: &mut Ctx,
    target: &PortableItem,
    base: &PortableItem,
    ours: &PortableItem,
    path: &str,
) -> PortableItem {
    let mut out = target.clone();
    macro_rules! field {
        ($name:ident) => {
            cx.scalar(
                &format!("item {path}.{}", stringify!($name)),
                &mut out.$name,
                &base.$name,
                &ours.$name,
            );
        };
    }
    field!(container);
    field!(slot);
    field!(count);
    field!(duration);
    field!(charges);
    field!(flags);
    field!(enchantments);
    field!(random_property_id);
    field!(durability);
    field!(played_time);
    field!(text);
    field!(gift);
    out
}

/// Free places of the character for relocations: backpack, then bank.
fn free_place(taken: &HashSet<(Option<PortableItemId>, u8)>) -> Option<u8> {
    (23..=38u8)
        .chain(39..=66u8)
        .find(|slot| !taken.contains(&(None, *slot)))
}

/// Make the item list valid: containers must exist and be directly on the character, no two items in one place. Items the
/// session touched keep their place; the others (typically items the realm filtered away, kept at their canonical place)
/// move to a free slot when their place was taken.
fn settle_places(
    items: &mut [PortableItem],
    touched: &HashSet<PortableItemId>,
) -> Result<Vec<PortableItemId>> {
    let mut relocated = Vec::new();
    let ids: HashSet<PortableItemId> = items.iter().map(|i| i.id).collect();
    let top: HashSet<PortableItemId> = items
        .iter()
        .filter(|i| i.container.is_none())
        .map(|i| i.id)
        .collect();
    let mut taken: HashSet<(Option<PortableItemId>, u8)> = HashSet::new();
    // a stable order: touched first, then by id
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (!touched.contains(&items[i].id), items[i].id));
    // pass 1: everybody who can keep their place keeps it
    let mut losers = Vec::new();
    for i in order {
        let orphan = items[i]
            .container
            .is_some_and(|c| !ids.contains(&c) || !top.contains(&c));
        if orphan {
            items[i].container = None;
        }
        if orphan || taken.contains(&(items[i].container, items[i].slot)) {
            losers.push(i);
        } else {
            taken.insert((items[i].container, items[i].slot));
        }
    }
    // pass 2: the others move to a free slot (only now are all the kept places known)
    for i in losers {
        let slot = free_place(&taken).ok_or_else(|| {
            PortableError::LimitExceeded("no free slot left for an item that lost its place".into())
        })?;
        items[i].container = None;
        items[i].slot = slot;
        taken.insert((None, slot));
        relocated.push(items[i].id);
    }
    Ok(relocated)
}

fn merge_pets(
    cx: &mut Ctx,
    out: &mut PortableCharacter,
    target: &PortableCharacter,
    base: &PortableCharacter,
    ours: &PortableCharacter,
) -> PetOutcome {
    let by_id = |m: &PortableCharacter| -> HashMap<PortablePetId, PortablePet> {
        m.pets.iter().map(|p| (p.id, p.clone())).collect()
    };
    let (t, b, o) = (by_id(target), by_id(base), by_id(ours));
    let mut result = t.clone();
    let mut outcome = PetOutcome::default();
    for (id, tpet) in &t {
        match (b.get(id), o.get(id)) {
            (Some(bpet), Some(opet)) => {
                let merged = merge_pet(cx, tpet, bpet, opet, &id.to_string());
                if &merged != tpet {
                    outcome.changed.push(*id);
                    cx.changes
                        .push(format!("pet {id}: changed ({})", tpet.name));
                }
                result.insert(*id, merged);
            }
            (Some(_), None) => {
                outcome.removed.push(*id);
                cx.changes
                    .push(format!("pet {id}: removed ({})", tpet.name));
                result.remove(id);
            }
            (None, Some(opet)) => {
                result.insert(*id, opet.clone());
                outcome.reappeared.push(*id);
                cx.changes
                    .push(format!("pet {id}: reappeared ({})", tpet.name));
            }
            (None, None) => outcome.filtered_kept.push(*id),
        }
    }
    for (id, opet) in &o {
        if t.contains_key(id) || b.contains_key(id) {
            continue;
        }
        result.insert(*id, opet.clone());
        outcome.added.push(*id);
        cx.changes.push(format!("pet {id}: new ({})", opet.name));
    }
    outcome.realm_local = b
        .keys()
        .filter(|id| !t.contains_key(*id))
        .copied()
        .collect();
    out.pets = result.into_values().collect();
    for v in [
        &mut outcome.added,
        &mut outcome.removed,
        &mut outcome.changed,
        &mut outcome.filtered_kept,
        &mut outcome.reappeared,
        &mut outcome.realm_local,
    ] {
        v.sort();
    }
    outcome
}

fn merge_pet(
    cx: &mut Ctx,
    target: &PortablePet,
    base: &PortablePet,
    ours: &PortablePet,
    path: &str,
) -> PortablePet {
    let mut out = target.clone();
    macro_rules! field {
        ($name:ident) => {
            cx.scalar(
                &format!("pet {path}.{}", stringify!($name)),
                &mut out.$name,
                &base.$name,
                &ours.$name,
            );
        };
    }
    field!(model_id);
    field!(created_by_spell);
    field!(pet_type);
    field!(level);
    field!(exp);
    field!(react_state);
    field!(name);
    field!(renamed);
    field!(slot);
    field!(health);
    field!(mana);
    field!(happiness);
    field!(action_bar);
    field!(declined_names);
    let mut spells = to_map(&out.spells, |s| (s.spell, s.active));
    cx.map(
        &format!("pet {path}.spells"),
        &mut spells,
        &to_map(&base.spells, |s| (s.spell, s.active)),
        &to_map(&ours.spells, |s| (s.spell, s.active)),
    );
    out.spells = spells
        .into_iter()
        .map(|(spell, active)| PetSpell { spell, active })
        .collect();
    out
}

#[cfg(test)]
mod tests;
