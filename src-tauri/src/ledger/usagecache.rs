//! Claude Code's own cached copy of the Usage tab.
//!
//! `~/.claude.json` (next to, not inside, the `.claude` directory) carries
//! `cachedUsageUtilization`: the percentages and reset times the Usage tab
//! shows, as last fetched by Claude Code. It is written opportunistically —
//! it can be minutes old or days old — so it seeds and re-anchors calibration
//! whenever it is fresher than what we last used, and never replaces a manual
//! entry made after it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageCache {
    pub fetched_at: DateTime<Utc>,
    pub session: Option<Reading>,
    pub weekly: Option<Reading>,
    pub fable: Option<Reading>,
}

/// `$CLAUDE_CONFIG_DIR/.claude.json` if set, else `~/.claude.json`.
pub fn default_path(claude_dir: &Path) -> PathBuf {
    if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        return claude_dir.join(".claude.json");
    }
    claude_dir
        .parent()
        .map(|p| p.join(".claude.json"))
        .unwrap_or_else(|| claude_dir.join(".claude.json"))
}

/// Why a read produced nothing — the three failures look identical to the
/// caller of `read`, but the user needs different advice for each.
pub enum Status {
    /// No `.claude.json` at all: Claude Code has never run for this user, or
    /// it lives somewhere we aren't looking (WSL, a custom config dir).
    NoFile,
    Unreadable(String),
    /// The file is there but carries no `cachedUsageUtilization`: nothing has
    /// fetched the Usage numbers yet.
    NoKey,
    Ok(UsageCache),
}

pub fn read_status(path: &Path) -> Status {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Status::NoFile,
        Err(e) => return Status::Unreadable(e.to_string()),
    };
    let v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Status::Unreadable(e.to_string()),
    };
    match v.get("cachedUsageUtilization").and_then(parse) {
        Some(c) => Status::Ok(c),
        None => Status::NoKey,
    }
}

pub fn read(path: &Path) -> Option<UsageCache> {
    match read_status(path) {
        Status::Ok(c) => Some(c),
        _ => None,
    }
}

fn reading(v: &Value, pct_key: &str) -> Option<Reading> {
    let percent = v.get(pct_key)?.as_f64()?;
    let resets_at = v
        .get("resets_at")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc));
    Some(Reading { percent, resets_at })
}

pub fn parse(c: &Value) -> Option<UsageCache> {
    let ms = c.get("fetchedAtMs")?.as_i64()?;
    let fetched_at = Utc.timestamp_millis_opt(ms).single()?;
    let u = c.get("utilization")?;
    Some(parse_utilization(u, fetched_at))
}

/// The `utilization` object on its own, which is also exactly what the
/// `/api/oauth/usage` endpoint returns — the cache is that body plus a
/// timestamp Claude Code stamps on locally. Kept separate so a live fetch and
/// a cache read produce the same `UsageCache` through the same code.
pub fn parse_utilization(u: &Value, fetched_at: DateTime<Utc>) -> UsageCache {
    let mut out = UsageCache { fetched_at, session: None, weekly: None, fable: None };

    // Current shape: a `limits` array with typed entries.
    if let Some(limits) = u.get("limits").and_then(Value::as_array) {
        for l in limits {
            let kind = l.get("kind").and_then(Value::as_str).unwrap_or("");
            match kind {
                "session" => out.session = reading(l, "percent"),
                "weekly_all" => out.weekly = reading(l, "percent"),
                "weekly_scoped" => {
                    let name = l
                        .pointer("/scope/model/display_name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if name.contains("fable") {
                        out.fable = reading(l, "percent");
                    }
                }
                _ => {}
            }
        }
    }
    // Older shape / fallback: named buckets.
    if out.session.is_none() {
        out.session = u.get("five_hour").and_then(|v| reading(v, "utilization"));
    }
    if out.weekly.is_none() {
        out.weekly = u.get("seven_day").and_then(|v| reading(v, "utilization"));
    }
    if out.fable.is_none() {
        out.fable = u.get("seven_day_fable").and_then(|v| reading(v, "utilization"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"{"fetchedAtMs": 1788555619606, "accountUuid": "x", "utilization": {
      "five_hour": {"utilization": 6, "resets_at": "2026-09-05T00:59:59.548565+00:00"},
      "seven_day": {"utilization": 1, "resets_at": "2026-09-08T05:59:59.548593+00:00"},
      "seven_day_opus": null,
      "limits": [
        {"kind":"session","group":"session","percent":6,"severity":"normal","resets_at":"2026-09-05T00:59:59.548565+00:00","scope":null,"is_active":true},
        {"kind":"weekly_all","group":"weekly","percent":1,"severity":"normal","resets_at":"2026-09-08T05:59:59.548593+00:00","scope":null,"is_active":false},
        {"kind":"weekly_scoped","group":"weekly","percent":53,"severity":"normal","resets_at":"2026-09-08T05:59:59.548593+00:00","scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}
      ]}}"#;

    #[test]
    fn parses_the_real_shape() {
        let c = parse(&serde_json::from_str(REAL).unwrap()).unwrap();
        assert_eq!(c.fetched_at.timestamp_millis(), 1788555619606);
        assert_eq!(c.session.unwrap().percent, 6.0);
        assert_eq!(c.weekly.unwrap().percent, 1.0);
        assert_eq!(c.fable.unwrap().percent, 53.0);
        assert_eq!(c.session.unwrap().resets_at.unwrap().to_rfc3339(), "2026-09-05T00:59:59.548565+00:00");
        assert!(c.fable.unwrap().resets_at.is_some());
    }

    #[test]
    fn falls_back_to_named_buckets_without_limits() {
        let mut v: Value = serde_json::from_str(REAL).unwrap();
        v["utilization"].as_object_mut().unwrap().remove("limits");
        let c = parse(&v).unwrap();
        assert_eq!(c.session.unwrap().percent, 6.0);
        assert_eq!(c.weekly.unwrap().percent, 1.0);
        assert!(c.fable.is_none());
    }

    #[test]
    fn missing_cache_is_none() {
        assert!(parse(&serde_json::json!({})).is_none());
    }
}
