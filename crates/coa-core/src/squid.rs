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
            .filter(|value| value.len() <= 80 && value.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))).map(str::to_owned);
        let tag = safe("tag").or_else(|| safe("version"));
        let commit = safe("commit").filter(|value| value.len() >= 7 && value.len() <= 40 && value.chars().all(|c| c.is_ascii_hexdigit()));
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
    settings: Vec<Field>,
    #[serde(default)]
    groups: std::collections::BTreeMap<String, String>,
}

pub fn fields(root: &Path) -> crate::Result<std::collections::BTreeMap<String, Field>> {
    let path = root.join("Core/configs/modules/playerbots.conf.settings.json");
    if !path.is_file() { return Ok(Default::default()); }
    let bytes = std::fs::read(path)?;
    let invalid = || crate::Error::Invalid("The SquidBots settings JSON is invalid or uses an unsupported format.".into());
    if bytes.len() > 1024 * 1024 { return Err(invalid()); }
    let settings: Settings = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if settings.format != 1 { return Err(invalid()); }
    let mut fields = std::collections::BTreeMap::new();
    for mut field in settings.settings {
        field.group_title = settings.groups.get(&field.group).cloned();
        if !matches!(field.kind.as_str(), "bool" | "int" | "float" | "string")
            || field.key.len() > 200 || field.key.is_empty()
            || !field.key.chars().all(|c| c.is_ascii_alphanumeric() || "._".contains(c))
            || field.min.zip(field.max).is_some_and(|(min,max)| min > max)
            || scalar(&field.default).is_none()
            || field.default_if_missing.as_ref().is_some_and(|value| scalar(value).is_none())
            || fields.contains_key(&field.key) { return Err(invalid()); }
        fields.insert(field.key.clone(), field);
    }
    Ok(fields)
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
    fn diagnostics_only_expose_release_identifiers() {
        let root = tempfile::tempdir().unwrap();
        crate::fsx::atomic_write(&root.path().join("CoA-Bots/release.json"), br#"{"tag":"v1.8","commit":"48c4786a","password":"secret"}"#).unwrap();
        assert!(imported_repack(root.path()));
        assert_eq!(release(root.path()).unwrap(), Release { tag: Some("v1.8".into()), commit: Some("48c4786a".into()) });
    }
}
