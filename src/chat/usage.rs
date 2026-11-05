//! Context, prompt cache, plan limits and token totals for the usage popover
//! next to the model chip.
//!
//! Sources, all owned by pi:
//! - RPC `get_session_stats`: context tokens and window, totals, cost;
//! - pi's settings files (read only): when automatic compaction starts;
//! - the app's bridge extension (`extensions/pi-gpui-bridge.ts`), which
//!   publishes the rest through `setStatus`: the last turn's tokens, the
//!   prompt cache lifetime, whether the provider bills a subscription, and
//!   plan limits from provider response headers.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// The `setStatus` key the bridge publishes under.
pub(super) const STATUS_KEY: &str = "pi-gpui:usage";
const DEFAULT_RESERVE_TOKENS: u64 = 16_384;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Stats {
    /// `None` right after a compaction, until the model answers again.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
}

impl Stats {
    pub fn from_session_stats(data: &Value) -> Self {
        let tokens = |key: &str| data["tokens"][key].as_u64().unwrap_or_default();
        Self {
            context_tokens: data["contextUsage"]["tokens"].as_u64(),
            context_window: data["contextUsage"]["contextWindow"].as_u64(),
            input: tokens("input"),
            output: tokens("output"),
            cache_read: tokens("cacheRead"),
            cache_write: tokens("cacheWrite"),
            cost: data["cost"].as_f64().unwrap_or_default(),
        }
    }

    /// Share of the context window in use, 0 to 1.
    pub fn context_fraction(&self) -> Option<f32> {
        let (tokens, window) = (self.context_tokens?, self.context_window?);
        (window > 0).then(|| (tokens as f32 / window as f32).clamp(0., 1.))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Turn {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Turn {
    /// Share of the prompt served from the cache.
    pub fn cached_fraction(&self) -> Option<f32> {
        let prompt = self.input + self.cache_read + self.cache_write;
        (prompt > 0).then(|| self.cache_read as f32 / prompt as f32)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PlanLimit {
    pub window_minutes: u64,
    pub used_percent: f32,
    /// Unix milliseconds.
    pub resets_at: Option<u64>,
}

/// What the bridge extension reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Extra {
    pub subscription: bool,
    pub last_turn: Option<Turn>,
    /// Prompt cache lifetime the model declares.
    pub cache_seconds: Option<u64>,
    /// Unix milliseconds of the last request that touched the cache.
    pub cache_touched_at: Option<u64>,
    pub limits: Vec<PlanLimit>,
}

impl Extra {
    pub fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let turn = &value["lastTurn"];
        Some(Self {
            subscription: value["subscription"].as_bool().unwrap_or(false),
            last_turn: turn.is_object().then(|| Turn {
                input: turn["input"].as_u64().unwrap_or_default(),
                cache_read: turn["cacheRead"].as_u64().unwrap_or_default(),
                cache_write: turn["cacheWrite"].as_u64().unwrap_or_default(),
            }),
            cache_seconds: value["cacheSeconds"].as_u64(),
            cache_touched_at: value["cacheTouchedAt"].as_u64(),
            limits: value["limits"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|limit| {
                    Some(PlanLimit {
                        window_minutes: limit["windowMinutes"].as_u64()?,
                        used_percent: limit["usedPercent"].as_f64()? as f32,
                        resets_at: limit["resetsAt"].as_u64(),
                    })
                })
                .collect(),
        })
    }

    /// Time left before the prompt cache lapses, if known.
    pub fn cache_left(&self, now_ms: u64) -> Option<Duration> {
        let expires = self.cache_touched_at? + self.cache_seconds? * 1000;
        Some(Duration::from_millis(expires.saturating_sub(now_ms)))
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Free tokens below which pi compacts automatically for this model, or
/// `None` when automatic compaction is off. Mirrors pi's settings lookup:
/// the project file overrides the global one; `modelOverrides` apply per
/// `provider/model`.
pub(super) fn compaction_reserve(workspace: &Path, provider: &str, model: &str) -> Option<u64> {
    let read = |path: PathBuf| -> Value {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .map(|settings| settings["compaction"].clone())
            .unwrap_or(Value::Null)
    };
    let agent_dir = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::sessions::home_directory().map(|home| home.join(".pi/agent")));
    let global = agent_dir.map_or(Value::Null, |dir| read(dir.join("settings.json")));
    let project = read(workspace.join(".pi/settings.json"));
    let layers = [&project, &global];

    let enabled = layers
        .iter()
        .find_map(|layer| layer["enabled"].as_bool())
        .unwrap_or(true);
    if !enabled {
        return None;
    }
    let key = format!("{provider}/{model}");
    let overridden = layers
        .iter()
        .find_map(|layer| layer["modelOverrides"][&key]["reserveTokens"].as_u64());
    let ordinary = layers
        .iter()
        .find_map(|layer| layer["reserveTokens"].as_u64());
    Some(overridden.or(ordinary).unwrap_or(DEFAULT_RESERVE_TOKENS))
}

/// "4 min", "3 hr 25 min", "3 d 16 h".
pub(super) fn format_until(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    match minutes {
        0 => "under a minute".to_string(),
        1..60 => format!("{minutes} min"),
        60..1440 => format!("{} hr {} min", minutes / 60, minutes % 60),
        _ => format!("{} d {} h", minutes / 1440, minutes / 60 % 24),
    }
}

/// "1:10" (minutes and seconds) for the cache countdown.
pub(super) fn format_countdown(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

/// "5-hour limit", "weekly limit".
pub(super) fn limit_name(window_minutes: u64) -> String {
    match window_minutes {
        10_080 => "weekly limit".to_string(),
        1_440 => "daily limit".to_string(),
        minutes if minutes % 60 == 0 => format!("{}-hour limit", minutes / 60),
        minutes => format!("{minutes}-minute limit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stats_and_bridge_report_parse() {
        let stats = Stats::from_session_stats(&json!({
            "tokens": { "input": 509_000, "output": 493_000, "cacheRead": 155_000_000, "cacheWrite": 7_900_000 },
            "cost": 12.5,
            "contextUsage": { "tokens": 328_000, "contextWindow": 1_000_000, "percent": 32.8 }
        }));
        assert_eq!(stats.context_fraction(), Some(0.328));
        assert_eq!(stats.cache_read, 155_000_000);

        let extra = Extra::parse(
            &json!({
                "subscription": true,
                "lastTurn": { "input": 10, "output": 5, "cacheRead": 990, "cacheWrite": 0 },
                "cacheSeconds": 300,
                "cacheTouchedAt": 1_000_000,
                "limits": [{ "windowMinutes": 300, "usedPercent": 36.0, "resetsAt": 2_000_000 }]
            })
            .to_string(),
        )
        .expect("valid report");
        assert!(extra.subscription);
        assert_eq!(
            extra.last_turn.as_ref().and_then(Turn::cached_fraction),
            Some(0.99)
        );
        assert_eq!(extra.cache_left(1_230_000), Some(Duration::from_secs(70)));
        assert_eq!(extra.cache_left(9_000_000), Some(Duration::ZERO));
        assert_eq!(extra.limits[0].window_minutes, 300);
    }

    #[test]
    fn durations_and_limit_names_read_naturally() {
        assert_eq!(format_countdown(Duration::from_secs(70)), "1:10");
        assert_eq!(
            format_until(Duration::from_secs(3 * 3600 + 25 * 60)),
            "3 hr 25 min"
        );
        assert_eq!(
            format_until(Duration::from_secs(3 * 86_400 + 16 * 3600)),
            "3 d 16 h"
        );
        assert_eq!(limit_name(300), "5-hour limit");
        assert_eq!(limit_name(10_080), "weekly limit");
    }

    #[test]
    fn compaction_settings_layer_project_over_global() {
        let workspace = std::env::temp_dir().join(format!("pi-gpui-usage-{}", std::process::id()));
        std::fs::create_dir_all(workspace.join(".pi")).unwrap();
        std::fs::write(
            workspace.join(".pi/settings.json"),
            json!({ "compaction": { "reserveTokens": 50_000, "modelOverrides": { "a/big": { "reserveTokens": 400_000 } } } })
                .to_string(),
        )
        .unwrap();
        assert_eq!(compaction_reserve(&workspace, "a", "big"), Some(400_000));
        assert_eq!(compaction_reserve(&workspace, "a", "small"), Some(50_000));
        std::fs::write(
            workspace.join(".pi/settings.json"),
            json!({ "compaction": { "enabled": false } }).to_string(),
        )
        .unwrap();
        assert_eq!(compaction_reserve(&workspace, "a", "big"), None);
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
