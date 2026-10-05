use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Release {
    pub tag: Option<String>,
    pub commit: Option<String>,
}

pub fn imported_repack(root: &Path) -> bool {
    root.join("CoA-Bots/release.json").is_file()
}

pub fn release(root: &Path) -> Option<Release> {
    for name in ["Extras/SquidPlayerbots/release.json", "Core/configs/modules/playerbots.conf.settings.json", "CoA-Bots/release.json"] {
        let Ok(bytes) = std::fs::read(root.join(name)) else { continue; };
        if bytes.len() > 1024 * 1024 { continue; }
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue; };
        let safe = |key: &str| json.get(key).and_then(|value| value.as_str())
            .filter(|value| !value.is_empty() && value.len() <= 80 && value.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))).map(str::to_owned);
        let tag = safe("tag").or_else(|| safe("version"));
        let revision = if name == "CoA-Bots/release.json" {
            json.get("botsModuleRevision").and_then(|value| value.as_str()).and_then(|value| value.split_whitespace().next()).map(str::to_owned).or_else(|| safe("commit"))
        } else if name.ends_with(".settings.json") { None } else { safe("commit") };
        let commit = revision.filter(|value| value.len() >= 7 && value.len() <= 40 && value.chars().all(|c| c.is_ascii_hexdigit()));
        if tag.is_some() || commit.is_some() { return Some(Release { tag, commit }); }
    }
    None
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Field {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub group_title: Option<String>,
    pub default: serde_json::Value,
    #[serde(default)]
    pub default_if_missing: Option<serde_json::Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Deserialize)]
struct Settings {
    format: u32,
    settings: Vec<serde_json::Value>,
    #[serde(default)]
    groups: std::collections::BTreeMap<String, String>,
}

pub fn fields(root: &Path) -> crate::Result<std::collections::BTreeMap<String, Field>> {
    let path = root.join("Core/configs/modules/playerbots.conf.settings.json");
    if !path.is_file() { return Ok(Default::default()); }
    let Ok(bytes) = std::fs::read(path) else { return Ok(Default::default()); };
    if bytes.len() > 1024 * 1024 { return Ok(Default::default()); }
    let Ok(settings) = serde_json::from_slice::<Settings>(&bytes) else { return Ok(Default::default()); };
    if settings.format != 1 { return Ok(Default::default()); }
    let mut fields = std::collections::BTreeMap::new();
    for value in settings.settings {
        let Ok(mut field) = serde_json::from_value::<Field>(value) else { continue; };
        field.group_title = settings.groups.get(&field.group).cloned();
        if !matches!(field.kind.as_str(), "bool" | "int" | "float" | "string")
            || field.key.len() > 200 || field.key.is_empty()
            || !field.key.chars().all(|c| c.is_ascii_alphanumeric() || "._".contains(c))
            || field.min.zip(field.max).is_some_and(|(min,max)| min > max)
            || !default_type(&field.kind, &field.default)
            || field.default_if_missing.as_ref().is_some_and(|value| !default_type(&field.kind, value))
            || fields.contains_key(&field.key) { continue; }
        fields.insert(field.key.clone(), field);
    }
    Ok(fields)
}

fn default_type(kind: &str, value: &serde_json::Value) -> bool {
    match kind {
        "bool" => value.is_boolean(),
        "int" => value.is_i64() || value.is_u64(),
        "float" => value.is_number(),
        "string" => value.is_string(),
        _ => false,
    }
}

pub fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Bool(value) => Some(if *value { "1" } else { "0" }.into()),
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

pub fn validate(field: &Field, value: &str) -> crate::Result<()> {
    let valid = match field.kind.as_str() {
        "bool" => matches!(value, "0" | "1"),
        "int" | "float" => value.parse::<f64>().is_ok_and(|number| number.is_finite()
            && (field.kind != "int" || number.fract() == 0.0)
            && field.min.is_none_or(|min| number >= min) && field.max.is_none_or(|max| number <= max)),
        "string" => true,
        _ => false,
    };
    if valid { Ok(()) } else { Err(crate::Error::Invalid(format!("{} must match its documented SquidBots type and range.", field.key))) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_settings_preserve_types_ranges_and_distinct_defaults() {
        let root = tempfile::tempdir().unwrap();
        crate::fsx::atomic_write(&root.path().join("Core/configs/modules/playerbots.conf.settings.json"), br#"{"format":1,"settings":[{"key":"AiPlayerbot.MinRandomBots","type":"int","group":"population","title":"Minimum bots","description":"Lower bound","default":500,"default_if_missing":50,"min":0,"max":5000}]}"#).unwrap();
        let fields = fields(root.path()).unwrap();
        let field = &fields["AiPlayerbot.MinRandomBots"];
        assert_eq!(scalar(&field.default).as_deref(), Some("500"));
        assert_eq!(scalar(field.default_if_missing.as_ref().unwrap()).as_deref(), Some("50"));
        assert!(validate(field, "5000").is_ok());
        for value in ["5001", "-1", "0.5", "NaN"] { assert!(validate(field, value).is_err()); }
    }
    #[test]
    fn unsupported_metadata_falls_back_and_bad_entries_do_not_hide_valid_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Core/configs/modules/playerbots.conf.settings.json");
        for data in [r#"{"format":99,"settings":[]}"#, "invalid json", r#"{"format":1,"settings":{}}"#] {
            crate::fsx::atomic_write(&path, data.as_bytes()).unwrap();
            assert!(fields(root.path()).unwrap().is_empty());
        }
        crate::fsx::atomic_write(&path, br#"{"format":1,"settings":[{"key":"Broken","type":"enum","title":"Broken","description":"Unknown type","default":1},{"key":"Malformed"},{"key":"WrongDefault","type":"int","title":"Wrong","description":"Bad default","default":"oops"},{"key":"Good","type":"bool","title":"Good","description":"Good field","default":true}]}"#).unwrap();
        let parsed = fields(root.path()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed.contains_key("Good"));
    }

    #[test]
    fn imported_revision_uses_its_first_word_and_empty_tags_are_hidden() {
        let root = tempfile::tempdir().unwrap();
        crate::fsx::atomic_write(&root.path().join("CoA-Bots/release.json"), br#"{"tag":"","botsModuleRevision":"48c4786a 2026-10-01 (coa branch)","commit":"abcdef01"}"#).unwrap();
        assert_eq!(release(root.path()).unwrap(), Release { tag: None, commit: Some("48c4786a".into()) });
        crate::fsx::atomic_write(&root.path().join("Core/configs/modules/playerbots.conf.settings.json"), br#"{"tag":"v1.8.1","commit":"11111111"}"#).unwrap();
        assert_eq!(release(root.path()).unwrap(), Release { tag: Some("v1.8.1".into()), commit: None });
    }

    #[test]
    fn diagnostics_only_expose_release_identifiers() {
        let root = tempfile::tempdir().unwrap();
        crate::fsx::atomic_write(&root.path().join("CoA-Bots/release.json"), br#"{"tag":"v1.8","commit":"48c4786a","password":"secret"}"#).unwrap();
        assert!(imported_repack(root.path()));
        assert_eq!(release(root.path()).unwrap(), Release { tag: Some("v1.8".into()), commit: Some("48c4786a".into()) });
    }
}
