//! Setting schemas (`schemas/bots.json`, `schemas/server.json`) and value validation/formatting.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingType {
    Bool,
    Int,
    Float,
    String,
    Enum,
}

/// How much has to be restarted for a change to take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Restart {
    Runtime,
    World,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: Value,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub key: String,
    #[serde(rename = "type")]
    pub ty: SettingType,
    pub category: String,
    pub title: String,
    pub description: String,
    pub default: Value,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<EnumOption>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub advanced: bool,
    pub restart_required: Restart,
    #[serde(default)]
    pub dangerous: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub schema: u32,
    pub scope: String,
    pub categories: Vec<Category>,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub description: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetFile {
    pub schema: u32,
    pub presets: Vec<Preset>,
}

impl Schema {
    pub fn parse(json: &str) -> Result<Schema> {
        let s: Schema = serde_json::from_str(json)?;
        s.check()?;
        Ok(s)
    }

    pub fn get(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// Structural sanity of the schema itself (caught by tests, not at user runtime).
    fn check(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Invalid(format!("schema {}: {m}", self.scope)));
        let cats: Vec<&str> = self.categories.iter().map(|c| c.id.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        for s in &self.settings {
            if !seen.insert(&s.key) {
                return bad(format!("duplicate key {}", s.key));
            }
            if !cats.contains(&s.category.as_str()) {
                return bad(format!("{}: unknown category {}", s.key, s.category));
            }
            if s.ty == SettingType::Enum && s.options.is_empty() {
                return bad(format!("{}: enum without options", s.key));
            }
            if let Err(e) = s.to_raw(&s.default) {
                return bad(format!("{}: default is invalid: {e}", s.key));
            }
        }
        Ok(())
    }
}

/// JSON equality that treats 1 and 1.0 as the same number.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn num_fmt(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        let s = format!("{f}");
        s
    }
}

impl Setting {
    /// Validate `value` and format it the way it is written to the `.conf` (strings are quoted).
    pub fn to_raw(&self, value: &Value) -> std::result::Result<String, String> {
        let range = |n: f64| -> std::result::Result<(), String> {
            if let Some(min) = self.min {
                if n < min {
                    return Err(format!("must be at least {}", num_fmt(min)));
                }
            }
            if let Some(max) = self.max {
                if n > max {
                    return Err(format!("must be at most {}", num_fmt(max)));
                }
            }
            Ok(())
        };
        match self.ty {
            SettingType::Bool => match value {
                Value::Bool(b) => Ok(if *b { "1" } else { "0" }.into()),
                _ => Err("must be on or off".into()),
            },
            SettingType::Int => match value.as_i64() {
                Some(i) => range(i as f64).map(|_| i.to_string()),
                None => Err("must be a whole number".into()),
            },
            SettingType::Float => match value.as_f64() {
                Some(f) if f.is_finite() => range(f).map(|_| num_fmt(f)),
                _ => Err("must be a number".into()),
            },
            SettingType::String => match value.as_str() {
                Some(s) if s.contains(['"', '\n', '\r']) => Err("must not contain quotes or line breaks".into()),
                Some(s) => Ok(format!("\"{s}\"")),
                None => Err("must be text".into()),
            },
            SettingType::Enum => {
                let opt = self.options.iter().find(|o| o.value == *value).ok_or("is not one of the allowed choices")?;
                Ok(match &opt.value {
                    Value::String(s) => format!("\"{s}\""),
                    other => other.to_string(),
                })
            }
        }
    }

    /// Decode a raw `.conf` value into JSON according to the setting's type.
    pub fn from_raw(&self, raw: &str) -> std::result::Result<Value, String> {
        let t = raw.trim();
        let unq = super::parser::unquote(t);
        match self.ty {
            SettingType::Bool => match unq.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(Value::Bool(true)),
                "0" | "false" | "no" | "off" => Ok(Value::Bool(false)),
                _ => Err(format!("{t:?} is not on/off")),
            },
            SettingType::Int => unq.parse::<i64>().map(Value::from).map_err(|_| format!("{t:?} is not a whole number")),
            SettingType::Float => unq
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| format!("{t:?} is not a number")),
            SettingType::String => Ok(Value::String(unq.to_string())),
            SettingType::Enum => {
                for o in &self.options {
                    let matches = match &o.value {
                        Value::String(s) => s == unq,
                        other => other.to_string() == unq,
                    };
                    if matches {
                        return Ok(o.value.clone());
                    }
                }
                Err(format!("{t:?} is not one of the known choices"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(ty: SettingType, min: Option<f64>, max: Option<f64>) -> Setting {
        Setting {
            key: "K".into(),
            ty,
            category: "c".into(),
            title: "t".into(),
            description: "d".into(),
            default: json!(1),
            min,
            max,
            options: vec![],
            unit: None,
            advanced: false,
            restart_required: Restart::World,
            dangerous: false,
        }
    }

    #[test]
    fn int_range_and_type_validation() {
        let st = s(SettingType::Int, Some(0.0), Some(100.0));
        assert_eq!(st.to_raw(&json!(50)).unwrap(), "50");
        assert!(st.to_raw(&json!(101)).is_err());
        assert!(st.to_raw(&json!(-1)).is_err());
        assert!(st.to_raw(&json!(1.5)).is_err());
        assert!(st.to_raw(&json!("5")).is_err());
    }

    #[test]
    fn float_bool_string_formatting() {
        let f = s(SettingType::Float, Some(0.0), None);
        assert_eq!(f.to_raw(&json!(1.5)).unwrap(), "1.5");
        assert_eq!(f.to_raw(&json!(2.0)).unwrap(), "2");
        assert!(f.to_raw(&json!(-0.1)).is_err());
        assert_eq!(s(SettingType::Bool, None, None).to_raw(&json!(true)).unwrap(), "1");
        assert!(s(SettingType::Bool, None, None).to_raw(&json!(1)).is_err());
        let st = s(SettingType::String, None, None);
        assert_eq!(st.to_raw(&json!("a b")).unwrap(), "\"a b\"");
        assert!(st.to_raw(&json!("bad\"quote")).is_err());
        assert!(st.to_raw(&json!("multi\nline")).is_err());
    }

    #[test]
    fn enum_and_decode() {
        let mut e = s(SettingType::Enum, None, None);
        e.options = vec![EnumOption { value: json!(0), label: "A".into() }, EnumOption { value: json!(2), label: "B".into() }];
        assert_eq!(e.to_raw(&json!(2)).unwrap(), "2");
        assert!(e.to_raw(&json!(1)).is_err());
        assert_eq!(e.from_raw("2").unwrap(), json!(2));
        assert!(e.from_raw("9").is_err());
        assert_eq!(s(SettingType::Bool, None, None).from_raw(" 1 ").unwrap(), json!(true));
        assert_eq!(s(SettingType::String, None, None).from_raw("\"x y\"").unwrap(), json!("x y"));
        assert_eq!(s(SettingType::Float, None, None).from_raw("1.5").unwrap(), json!(1.5));
        assert!(s(SettingType::Int, None, None).from_raw("abc").is_err());
    }
}
