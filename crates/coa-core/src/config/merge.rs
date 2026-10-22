//! Non-destructive `.dist` merge: add keys a newer `.dist` introduced, never touch existing values,
//! and only *report* keys the newer `.dist` no longer has.

use serde::Serialize;

use super::parser::ConfFile;

#[derive(Debug, Clone, Default, Serialize)]
pub struct MergePlan {
    pub added: Vec<String>,
    /// Present in the user's file but unknown to the `.dist` (possibly removed upstream, possibly a custom setting).
    pub not_in_dist: Vec<String>,
}

pub fn plan(conf: &ConfFile, dist: &ConfFile) -> MergePlan {
    let added = dist
        .entries()
        .map(|(k, _)| k)
        .filter(|k| !conf.contains(k))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let not_in_dist = conf
        .entries()
        .map(|(k, _)| k)
        .filter(|k| !dist.contains(k))
        .map(str::to_string)
        .collect();
    MergePlan {
        added: dedup(added),
        not_in_dist: dedup(not_in_dist),
    }
}

fn dedup(mut v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    v.retain(|k| seen.insert(k.clone()));
    v
}

/// Apply the additive part of the plan to `conf`; returns what was added.
pub fn apply(conf: &mut ConfFile, dist: &ConfFile) -> MergePlan {
    let plan = plan(conf, dist);
    for key in &plan.added {
        let value = dist.get(key).unwrap_or_default().to_string();
        let mut doc = dist.doc_for(key);
        doc.push("(added by CoA Server Manager from the updated .dist)".into());
        let refs: Vec<&str> = doc.iter().map(String::as_str).collect();
        conf.set(key, &value, &refs);
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_new_keys_with_docs_and_preserves_user_values_and_unknowns() {
        let mut conf = ConfFile::parse("[worldserver]\nA = 42\n# mine\nCustom.Key = 1\n");
        let dist = ConfFile::parse(
            "[worldserver]\n# About A\nA = 1\n# About B\n# second line\nB = \"x\"\n",
        );
        let before = conf.to_text();
        let p = apply(&mut conf, &dist);
        assert_eq!(p.added, ["B"]);
        assert_eq!(p.not_in_dist, ["Custom.Key"]);
        let out = conf.to_text();
        assert!(out.starts_with(&before), "existing content is untouched");
        assert!(out.contains("# About B\n# second line\n# (added by CoA Server Manager from the updated .dist)\nB = \"x\"\n"), "{out}");
        assert_eq!(conf.get("A"), Some("42"));
        assert_eq!(conf.get("Custom.Key"), Some("1"));
        // idempotent
        assert!(apply(&mut conf, &dist).added.is_empty());
    }

    #[test]
    fn removed_upstream_keys_are_reported_not_deleted() {
        let mut conf = ConfFile::parse("Old.Key = 5\nKeep = 1\n");
        let dist = ConfFile::parse("Keep = 1\n");
        let p = apply(&mut conf, &dist);
        assert_eq!(p.not_in_dist, ["Old.Key"]);
        assert_eq!(conf.get("Old.Key"), Some("5"));
    }
}
