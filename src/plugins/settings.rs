//! Plugin settings (`contributes.settings`): shown on the Settings page and
//! stored in settings.json as top-level `"<plugin id>.<key>"` values, so the
//! file stays the one source of truth.

use gpui::SharedString;
use serde_json::{Map, Value, json};

use crate::settings::Settings;

#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    Boolean,
    /// `secret` hides the text on the Settings page.
    Text {
        secret: bool,
        placeholder: SharedString,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
    },
    /// Values and their labels.
    Select(Vec<(Value, SharedString)>),
}

#[derive(Clone, Debug)]
pub struct PluginSetting {
    pub plugin_id: SharedString,
    pub key: SharedString,
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub kind: SettingKind,
    pub default: Value,
}

impl PluginSetting {
    /// The key in settings.json.
    pub fn full_key(&self) -> String {
        format!("{}.{}", self.plugin_id, self.key)
    }

    /// The stored value when it fits the setting, else the default.
    pub fn value(&self, settings: &Settings) -> Value {
        settings
            .plugin_values
            .get(&self.full_key())
            .and_then(|value| self.validate(value))
            .unwrap_or_else(|| self.default.clone())
    }

    /// The value as this setting stores it, or `None` when it does not fit.
    pub fn validate(&self, value: &Value) -> Option<Value> {
        match &self.kind {
            SettingKind::Boolean => value.as_bool().map(Value::from),
            SettingKind::Text { .. } => value.as_str().map(Value::from),
            SettingKind::Number { min, max } => {
                let number = value.as_f64()?;
                let fits =
                    min.is_none_or(|min| number >= min) && max.is_none_or(|max| number <= max);
                fits.then(|| value.clone())
            }
            SettingKind::Select(options) => options
                .iter()
                .any(|(option, _)| option == value)
                .then(|| value.clone()),
        }
    }

    /// What a text field shows for a value.
    pub fn text(&self, value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        }
    }

    /// A text field's content as a value, `None` when it does not fit.
    pub fn parse_text(&self, text: &str) -> Option<Value> {
        let value = match self.kind {
            SettingKind::Number { .. } => {
                let number: f64 = text.trim().parse().ok()?;
                if number.fract() == 0. && number.abs() < 9e15 {
                    json!(number as i64)
                } else {
                    json!(number)
                }
            }
            _ => Value::from(text),
        };
        self.validate(&value)
    }
}

/// The current values of one plugin's settings, as sent to it.
pub fn values(settings: &[PluginSetting], plugin_id: &str, current: &Settings) -> Value {
    let values: Map<String, Value> = settings
        .iter()
        .filter(|setting| setting.plugin_id == plugin_id)
        .map(|setting| (setting.key.to_string(), setting.value(current)))
        .collect();
    Value::Object(values)
}

/// `contributes.settings` of a manifest.
pub fn parse(manifest: &Value, plugin_id: &SharedString) -> Result<Vec<PluginSetting>, String> {
    let mut settings: Vec<PluginSetting> = Vec::new();
    for entry in manifest["contributes"]["settings"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let key = entry["key"].as_str().unwrap_or_default();
        if key.is_empty()
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        {
            return Err("settings: each needs a \"key\" of ASCII letters, digits, - _ .".into());
        }
        if settings.iter().any(|setting| setting.key == key) {
            return Err(format!("settings: \"{key}\" is declared twice"));
        }
        let text = |name: &str| {
            entry[name]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| SharedString::from(text.to_string()))
        };
        let kind = match entry["type"].as_str().unwrap_or("string") {
            "boolean" => SettingKind::Boolean,
            "string" => SettingKind::Text {
                secret: entry["secret"].as_bool().unwrap_or(false),
                placeholder: text("placeholder").unwrap_or_default(),
            },
            "number" => SettingKind::Number {
                min: entry["min"].as_f64(),
                max: entry["max"].as_f64(),
            },
            "select" => {
                let options: Vec<(Value, SharedString)> = entry["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|option| match option {
                        Value::Object(option) => {
                            let value = option.get("value")?.clone();
                            let label = option
                                .get("label")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                                .unwrap_or_else(|| PluginSetting::text_of(&value));
                            Some((value, label.into()))
                        }
                        value => Some((value.clone(), PluginSetting::text_of(value).into())),
                    })
                    .collect();
                if options.is_empty() {
                    return Err(format!("settings: \"{key}\" needs \"options\""));
                }
                SettingKind::Select(options)
            }
            other => return Err(format!("settings: \"{key}\" has unknown type \"{other}\"")),
        };
        let mut setting = PluginSetting {
            plugin_id: plugin_id.clone(),
            key: key.to_string().into(),
            title: text("title").unwrap_or_else(|| key.to_string().into()),
            description: text("description"),
            kind,
            default: Value::Null,
        };
        let fallback = match &setting.kind {
            SettingKind::Boolean => json!(false),
            SettingKind::Text { .. } => json!(""),
            SettingKind::Number { min, .. } => json!(min.unwrap_or(0.) as i64),
            SettingKind::Select(options) => options[0].0.clone(),
        };
        setting.default = match entry.get("default") {
            None => fallback,
            Some(default) => setting.validate(default).ok_or_else(|| {
                format!("settings: the default of \"{key}\" does not fit its type")
            })?,
        };
        settings.push(setting);
    }
    Ok(settings)
}

impl PluginSetting {
    fn text_of(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Value {
        json!({ "contributes": { "settings": [
            { "key": "port", "title": "Port", "type": "number", "default": 7319, "min": 1, "max": 65535 },
            { "key": "token", "type": "string", "secret": true },
            { "key": "relay", "type": "boolean", "default": true },
            { "key": "mode", "type": "select", "options": ["local", { "value": "remote", "label": "Remote" }] },
        ] } })
    }

    #[test]
    fn parses_types_and_defaults() {
        let settings = parse(&manifest(), &"mcp".into()).unwrap();
        assert_eq!(settings.len(), 4);
        assert_eq!(settings[0].full_key(), "mcp.port");
        assert_eq!(settings[0].default, json!(7319));
        assert_eq!(settings[1].default, json!(""));
        assert_eq!(settings[2].default, json!(true));
        assert_eq!(settings[3].default, json!("local"));
        let SettingKind::Select(options) = &settings[3].kind else {
            panic!("select expected");
        };
        assert_eq!(options[1].1.as_ref(), "Remote");
    }

    #[test]
    fn stored_values_are_checked() {
        let settings = parse(&manifest(), &"mcp".into()).unwrap();
        let mut current = Settings::default();
        current
            .plugin_values
            .insert("mcp.port".into(), json!(99999));
        current
            .plugin_values
            .insert("mcp.mode".into(), json!("remote"));
        let values = values(&settings, "mcp", &current);
        assert_eq!(values["port"], json!(7319));
        assert_eq!(values["mode"], json!("remote"));
        assert_eq!(settings[0].parse_text(" 8080 "), Some(json!(8080)));
        assert_eq!(settings[0].parse_text("http"), None);
    }

    #[test]
    fn rejects_bad_declarations() {
        let bad_default = json!({ "contributes": { "settings": [
            { "key": "n", "type": "number", "default": "x" } ] } });
        assert!(parse(&bad_default, &"p".into()).is_err());
        let unknown = json!({ "contributes": { "settings": [ { "key": "n", "type": "color" } ] } });
        assert!(parse(&unknown, &"p".into()).is_err());
    }
}
