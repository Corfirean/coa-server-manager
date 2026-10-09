//! The three shapes in which the core keeps a CoA build in `character_settings` (all integer vectors), as keyed records.
//!
//! ```text
//! core.ascension_slot.<n>                        [1, class, spec, N, (entry, rank) * N, M, (button, action) * M, 0 ...]
//! core.ascension_build.<spec>, ...slot.<n>.build.<spec>   [count, (entry * 10 + rank) * count, 0 ...]
//! core.ascension_bar.<spec>,   ...slot.<n>.bar.<spec>     [count, (button, spell) * count, 0 ...]
//! ```
//!
//! The codecs know **structure only**: which keys a record has and how to write it back. What a level cap allows is the core's
//! decision and arrives as a list of held keys; nothing here reads a level, a spell or an item.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Slot,
    Build,
    Bar,
}

/// The shape a settings source name stands for, if it is one of the build shapes.
pub fn classify(source: &str) -> Option<SettingKind> {
    fn number(s: &str) -> bool {
        !s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())
    }
    if let Some(rest) = source.strip_prefix("core.ascension_build.") {
        return number(rest).then_some(SettingKind::Build);
    }
    if let Some(rest) = source.strip_prefix("core.ascension_bar.") {
        return number(rest).then_some(SettingKind::Bar);
    }
    let rest = source.strip_prefix("core.ascension_slot.")?;
    let (slot, tail) = rest
        .split_once('.')
        .map_or((rest, None), |(s, t)| (s, Some(t)));
    if !number(slot) {
        return None;
    }
    match tail {
        None => Some(SettingKind::Slot),
        Some(t) => {
            if let Some(spec) = t.strip_prefix("build.") {
                number(spec).then_some(SettingKind::Build)
            } else if let Some(spec) = t.strip_prefix("bar.") {
                number(spec).then_some(SettingKind::Bar)
            } else {
                None
            }
        }
    }
}

/// A build record as keyed values. `order` keeps the order the keys had, so that a record written back reads like the one that was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: SettingKind,
    /// `(class, spec)` of a slot record, zeros for the other shapes.
    pub head: (u32, u32),
    /// Entry -> rank (slot, build).
    pub entries: BTreeMap<u32, u32>,
    /// Button -> action or spell (slot actions, bar).
    pub buttons: BTreeMap<u32, u32>,
    pub entry_order: Vec<u32>,
    pub button_order: Vec<u32>,
}

impl Record {
    fn empty(kind: SettingKind) -> Self {
        Self {
            kind,
            head: (0, 0),
            entries: BTreeMap::new(),
            buttons: BTreeMap::new(),
            entry_order: vec![],
            button_order: vec![],
        }
    }

    /// `None` when the values are not exactly one of the shapes (a record the codec cannot take apart is never edited).
    pub fn parse(kind: SettingKind, v: &[u32]) -> Option<Record> {
        let mut r = Record::empty(kind);
        match kind {
            SettingKind::Slot => {
                if v.len() < 5 || v[0] != 1 || !(12..=32).contains(&v[1]) {
                    return None;
                }
                r.head = (v[1], v[2]);
                let n = v[3] as usize;
                if n > (v.len() - 5) / 2 {
                    return None;
                }
                let mut at = 4;
                for _ in 0..n {
                    let (entry, rank) = (v[at], v[at + 1]);
                    at += 2;
                    if r.entries.insert(entry, rank).is_some() {
                        return None;
                    }
                    r.entry_order.push(entry);
                }
                let m = *v.get(at)? as usize;
                at += 1;
                if m > (v.len() - at) / 2 {
                    return None;
                }
                for _ in 0..m {
                    let (button, action) = (v[at], v[at + 1]);
                    at += 2;
                    if button >= 144 || action == 0 || r.buttons.insert(button, action).is_some() {
                        return None;
                    }
                    r.button_order.push(button);
                }
                v[at..].iter().all(|x| *x == 0).then_some(r)
            }
            SettingKind::Build => {
                let n = *v.first()? as usize;
                if n > v.len() - 1 || v[1 + n..].iter().any(|x| *x != 0) {
                    return None;
                }
                for pick in &v[1..=n] {
                    if *pick == 0 {
                        continue;
                    }
                    let (entry, rank) = (pick / 10, pick % 10);
                    if r.entries.insert(entry, rank).is_some() {
                        return None;
                    }
                    r.entry_order.push(entry);
                }
                Some(r)
            }
            SettingKind::Bar => {
                let n = *v.first()? as usize;
                if n > (v.len() - 1) / 2 || v[1 + 2 * n..].iter().any(|x| *x != 0) {
                    return None;
                }
                for i in 0..n {
                    let (button, spell) = (v[1 + 2 * i], v[2 + 2 * i]);
                    if spell == 0 {
                        continue;
                    }
                    if r.buttons.insert(button, spell).is_some() {
                        return None;
                    }
                    r.button_order.push(button);
                }
                Some(r)
            }
        }
    }

    pub fn write(&self) -> Vec<u32> {
        let entries: Vec<u32> = self.ordered(&self.entry_order, &self.entries);
        let buttons: Vec<u32> = self.ordered(&self.button_order, &self.buttons);
        match self.kind {
            SettingKind::Slot => {
                let mut out = vec![1, self.head.0, self.head.1, entries.len() as u32];
                for e in &entries {
                    out.extend([*e, self.entries[e]]);
                }
                out.push(buttons.len() as u32);
                for b in &buttons {
                    out.extend([*b, self.buttons[b]]);
                }
                out
            }
            SettingKind::Build => {
                let mut out = vec![entries.len() as u32];
                out.extend(entries.iter().map(|e| e * 10 + self.entries[e]));
                out
            }
            SettingKind::Bar => {
                let mut out = vec![buttons.len() as u32];
                for b in &buttons {
                    out.extend([*b, self.buttons[b]]);
                }
                out
            }
        }
    }

    fn ordered(&self, order: &[u32], map: &BTreeMap<u32, u32>) -> Vec<u32> {
        let mut keys: Vec<u32> = order
            .iter()
            .copied()
            .filter(|k| map.contains_key(k))
            .collect();
        let seen: BTreeSet<u32> = keys.iter().copied().collect();
        keys.extend(map.keys().copied().filter(|k| !seen.contains(k)));
        keys
    }

    /// The record without the keys the projection holds.
    pub fn without(&self, entries: &[u32], buttons: &[u32]) -> Record {
        let mut r = self.clone();
        for e in entries {
            r.entries.remove(e);
        }
        for b in buttons {
            r.buttons.remove(b);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_of_the_three_shapes_are_recognised_and_nothing_else() {
        assert_eq!(classify("core.ascension_slot.0"), Some(SettingKind::Slot));
        assert_eq!(
            classify("core.ascension_slot.12.build.34"),
            Some(SettingKind::Build)
        );
        assert_eq!(
            classify("core.ascension_slot.3.bar.61"),
            Some(SettingKind::Bar)
        );
        assert_eq!(
            classify("core.ascension_build.61"),
            Some(SettingKind::Build)
        );
        assert_eq!(classify("core.ascension_bar.0"), Some(SettingKind::Bar));
        for other in [
            "core.ascension_slot.active",
            "core.ascension_slot.1.x",
            "core.ascension_build.",
            "core.ascension_slot.1.build.",
            "core.ascension_active_spec",
            "core.ascension_slot.1.bar.1.2",
            "core.ascension_build.1234567890",
        ] {
            assert_eq!(classify(other), None, "{other}");
        }
    }

    #[test]
    fn a_slot_record_round_trips_and_a_bad_one_is_not_taken_apart() {
        let v = vec![
            1, 20, 61, 3, 100, 2, 101, 1, 102, 3, 2, 5, 900, 9, 901, 0, 0,
        ];
        let r = Record::parse(SettingKind::Slot, &v).unwrap();
        assert_eq!(r.head, (20, 61));
        assert_eq!(r.entries.len(), 3);
        assert_eq!(r.buttons.get(&5), Some(&900));
        assert_eq!(
            r.write(),
            vec![1, 20, 61, 3, 100, 2, 101, 1, 102, 3, 2, 5, 900, 9, 901],
            "trailing padding is not kept"
        );
        let held = r.without(&[101], &[9]);
        assert_eq!(held.write(), vec![1, 20, 61, 2, 100, 2, 102, 3, 1, 5, 900]);
        for bad in [
            vec![1, 20, 61, 3, 100, 2],
            vec![2, 20, 61, 0, 0],
            vec![1, 5, 61, 0, 0],
            vec![1, 20, 61, 1, 100, 2, 0, 7],
            vec![1, 20, 61, 2, 100, 2, 100, 3, 0],
            vec![1, 20, 61, 0, 1, 200, 5],
            vec![1, 20, 61, 0, 0, 3],
        ] {
            assert!(Record::parse(SettingKind::Slot, &bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn builds_and_bars_round_trip() {
        let build = Record::parse(SettingKind::Build, &[3, 1001, 2002, 2053, 0, 0]).unwrap();
        assert_eq!(build.entries.get(&100), Some(&1));
        assert_eq!(build.entries.get(&205), Some(&3));
        assert_eq!(build.without(&[100], &[]).write(), vec![2, 2002, 2053]);
        assert!(Record::parse(SettingKind::Build, &[5, 1, 2]).is_none());
        assert!(
            Record::parse(SettingKind::Build, &[1, 11, 7]).is_none(),
            "something after the picks"
        );
        let bar = Record::parse(SettingKind::Bar, &[2, 0, 500, 7, 501, 0, 0]).unwrap();
        assert_eq!(bar.without(&[], &[0]).write(), vec![1, 7, 501]);
        assert!(Record::parse(SettingKind::Bar, &[3, 0, 1]).is_none());
        assert_eq!(
            Record::parse(SettingKind::Bar, &[0]).unwrap().write(),
            vec![0]
        );
    }
}
