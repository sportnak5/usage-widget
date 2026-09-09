//! Everything the app does that isn't a window: own the index and settings,
//! refresh on demand, produce snapshots, take calibration input. The Tauri
//! layer and `ledger-cli` are both thin shells over this.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ledger::snapshot::{build_snapshot, window_total, Limits};
use crate::ledger::usagecache::{self, Reading, Status, UsageCache};
use crate::ledger::windows::{resolve, Anchor, Window};
use crate::ledger::{Index, ScanStats, Snapshot, WeeklyReset, WindowKind};
use crate::settings::{data_dir, Settings};

pub struct Engine {
    pub settings: Settings,
    pub index: Index,
    pub settings_path: PathBuf,
    pub index_path: PathBuf,
    pub last_scan: ScanStats,
    pub last_error: Option<String>,
    pub usage_cache: Option<UsageCache>,
    pub cache_diag: CacheDiag,
}

/// Why auto-calibration did or didn't take, in enough detail for the UI to
/// tell someone what to do about it. Manual entry is the fallback of last
/// resort, so every failure here has to name its own remedy.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CacheDiag {
    /// The `.claude.json` we looked at, so a wrong config dir is visible.
    pub path: String,
    /// The transcripts directory the same setting resolves to.
    pub projects_dir: String,
    /// `ok` · `no_dir` · `no_file` · `unreadable` · `no_key` · `too_low` ·
    /// `no_readings` · `no_usage`.
    pub reason: String,
    pub detail: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
    /// Largest percentage the cache carried — what "too low" was too low by.
    pub best_pct: Option<f64>,
}

impl CacheDiag {
    fn at(path: &std::path::Path, reason: &str, detail: Option<String>) -> CacheDiag {
        CacheDiag {
            path: path.display().to_string(),
            reason: reason.into(),
            detail,
            ..CacheDiag::default()
        }
    }
}

/// What the user types into the calibration pane. Every field optional so a
/// partial update (just a new session countdown, say) leaves the rest alone.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CalibrationInput {
    pub session_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    pub fable_pct: Option<f64>,
    /// "Resets in 3 hr 46 min" → 226.
    pub session_resets_in_minutes: Option<i64>,
    pub weekly_reset: Option<WeeklyReset>,
    pub plan: Option<String>,
    pub boost: Option<String>,
}

impl Engine {
    pub fn open_at(data: PathBuf) -> Engine {
        let settings_path = data.join("settings.json");
        let index_path = data.join("index.json");
        Engine {
            settings: Settings::load(&settings_path),
            index: Index::load(&index_path),
            settings_path,
            index_path,
            last_scan: ScanStats::default(),
            last_error: None,
            usage_cache: None,
            cache_diag: CacheDiag::default(),
        }
    }

    pub fn open() -> Engine {
        Engine::open_at(data_dir())
    }

    pub fn save(&mut self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.last_error = Some(format!("saving settings: {e}"));
        }
        if let Err(e) = self.index.save(&self.index_path) {
            self.last_error = Some(format!("saving index: {e}"));
        }
    }

    pub fn refresh(&mut self, now: DateTime<Utc>) -> Result<ScanStats, String> {
        // The Usage cache sits outside the transcripts tree, so read it even
        // when the scan can't run: a config dir pointing at the wrong place is
        // exactly when the entry screen needs something to diagnose.
        let scan = match self.settings.projects_dir() {
            None => Err("could not locate the Claude Code config directory".to_string()),
            Some(d) if !d.is_dir() => Err(format!("no transcripts directory at {}", d.display())),
            Some(d) => {
                let stats = self.index.refresh(&d, now);
                self.last_scan = stats.clone();
                Ok(stats)
            }
        };
        self.last_error = scan.as_ref().err().cloned();
        self.absorb_usage_cache();
        scan
    }

    /// Path of Claude Code's `.claude.json` for the configured directory.
    pub fn usage_cache_path(&self) -> Option<std::path::PathBuf> {
        self.settings.claude_dir().map(|d| usagecache::default_path(&d))
    }

    /// Re-anchor from Claude Code's cached Usage-tab reading when it is newer
    /// than anything we've applied — and never over a later manual entry.
    ///
    /// Always records why it could or couldn't, even on the paths that change
    /// nothing: the entry screen leads with "run /usage", so it needs to know
    /// whether that would actually help.
    pub fn absorb_usage_cache(&mut self) {
        let Some(path) = self.usage_cache_path() else {
            self.usage_cache = None;
            self.cache_diag =
                CacheDiag { reason: "no_dir".into(), ..CacheDiag::default() };
            return;
        };
        let projects = self
            .settings
            .projects_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let cache = match usagecache::read_status(&path) {
            Status::Ok(c) => c,
            Status::NoFile => {
                self.usage_cache = None;
                self.cache_diag = CacheDiag { projects_dir: projects, ..CacheDiag::at(&path, "no_file", None) };
                return;
            }
            Status::NoKey => {
                self.usage_cache = None;
                self.cache_diag = CacheDiag { projects_dir: projects, ..CacheDiag::at(&path, "no_key", None) };
                return;
            }
            Status::Unreadable(e) => {
                self.usage_cache = None;
                self.cache_diag =
                    CacheDiag { projects_dir: projects, ..CacheDiag::at(&path, "unreadable", Some(e)) };
                return;
            }
        };

        let (reason, best_pct, anchors) = self.evaluate(&cache);
        self.usage_cache = Some(cache.clone());
        self.cache_diag = CacheDiag {
            path: path.display().to_string(),
            projects_dir: projects,
            reason,
            detail: None,
            fetched_at: Some(cache.fetched_at),
            best_pct,
        };

        let cal = &self.settings.calibration;
        let already = cal.auto_fetched_at.map_or(false, |t| t >= cache.fetched_at);
        let manual_newer = cal.manual_at.map_or(false, |m| m >= cache.fetched_at);
        if already || manual_newer {
            return;
        }
        self.apply_cache(&cache, anchors);
    }

    /// What this cache would anchor, and why not, without touching settings.
    /// Split out from `apply_cache` so the diagnosis is the same whether or
    /// not we go on to apply it.
    fn evaluate(&self, cache: &UsageCache) -> (String, Option<f64>, Vec<(WindowKind, Anchor)>) {
        let at = cache.fetched_at;
        let readings = [
            (WindowKind::Session, cache.session),
            (WindowKind::Weekly, cache.weekly),
            (WindowKind::Fable, cache.fable),
        ];
        let best_pct = readings
            .iter()
            .filter_map(|(_, r)| r.map(|r| r.percent))
            .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))));

        // Percentages are integers; below ~10% the rounding error swamps the
        // implied limit, so keep whatever anchor we already have.
        let mut usable = false;
        let mut anchors = Vec::new();
        for (kind, r) in readings {
            let Some(r) = r else { continue };
            if r.percent < 10.0 {
                continue;
            }
            usable = true;
            // The reading describes the window that was current at fetch
            // time; total our usage over exactly that window, up to then.
            let window = match r.resets_at {
                Some(end) => Window::ending_at(kind, end),
                None => {
                    let ts = self.index.timestamps_sorted();
                    let ws = resolve(&self.settings.calibration, at, &ts);
                    ws.into_iter().find(|w| w.kind == kind).expect("all three")
                }
            };
            let clipped = Window { end: window.end.min(at), ..window };
            let total = window_total(&self.index, &self.settings.prices, &clipped);
            if total <= 0.0 {
                continue;
            }
            anchors.push((
                kind,
                Anchor {
                    pct: r.percent,
                    captured_at: at,
                    implied_limit: total / (r.percent / 100.0),
                },
            ));
        }
        let reason = if !anchors.is_empty() {
            "ok"
        } else if usable {
            // Claude Code says we've used something; our transcripts disagree.
            "no_usage"
        } else if best_pct.is_some() {
            "too_low"
        } else {
            "no_readings"
        };
        (reason.into(), best_pct, anchors)
    }

    fn apply_cache(&mut self, cache: &UsageCache, anchors: Vec<(WindowKind, Anchor)>) {
        // Reset instants are exact and always worth taking.
        if let Some(Reading { resets_at: Some(r), .. }) = cache.session {
            self.settings.calibration.session_reset_at = Some(r);
        }
        if let Some(Reading { resets_at: Some(r), .. }) = cache.weekly.or(cache.fable) {
            self.settings.calibration.weekly_reset = Some(WeeklyReset::from_instant(r));
        }
        for (kind, anchor) in anchors {
            *self.settings.calibration.anchor_mut(kind) = Some(anchor);
        }
        self.settings.calibration.auto_fetched_at = Some(cache.fetched_at);
    }

    pub fn snapshot(&self, now: DateTime<Utc>) -> Snapshot {
        build_snapshot(
            &self.index,
            &self.settings.prices,
            &self.settings.calibration,
            &self.settings.plan,
            self.settings.boost.as_deref(),
            now,
            self.last_scan.clone(),
            &Limits::default(),
            self.usage_cache.clone(),
            self.cache_diag.clone(),
        )
    }

    /// Anchor the implied limits to what the Usage tab says right now.
    pub fn calibrate(&mut self, input: CalibrationInput, now: DateTime<Utc>) -> Result<(), String> {
        let cal = &mut self.settings.calibration;
        if let Some(m) = input.session_resets_in_minutes {
            if !(0..=5 * 60).contains(&m) {
                return Err("session reset must be between 0 and 300 minutes away".into());
            }
            cal.session_reset_at = Some(now + Duration::minutes(m));
        }
        if let Some(w) = input.weekly_reset {
            if w.hour > 23 || w.minute > 59 {
                return Err("weekly reset time is out of range".into());
            }
            cal.weekly_reset = Some(w);
        }
        if let Some(p) = input.plan {
            self.settings.plan = p;
        }
        if input.boost.is_some() {
            self.settings.boost = input.boost.filter(|b| !b.trim().is_empty());
        }

        let ts = self.index.timestamps_sorted();
        let windows = resolve(&self.settings.calibration, now, &ts);
        let inputs = [
            (WindowKind::Session, input.session_pct),
            (WindowKind::Weekly, input.weekly_pct),
            (WindowKind::Fable, input.fable_pct),
        ];
        for (kind, pct) in inputs {
            let Some(pct) = pct else { continue };
            if !(0.0..=1000.0).contains(&pct) {
                return Err(format!("{} percentage out of range", kind.label()));
            }
            let w = windows.iter().find(|w| w.kind == kind).expect("resolve returns all three");
            let total = window_total(&self.index, &self.settings.prices, w);
            if pct <= 0.0 {
                // 0% is a legitimate reading with nothing to anchor to.
                *self.settings.calibration.anchor_mut(kind) = None;
                continue;
            }
            if total <= 0.0 {
                return Err(format!(
                    "{} shows {pct}% but no weighted usage was found in that window — refresh first, or check the transcripts directory",
                    kind.label()
                ));
            }
            *self.settings.calibration.anchor_mut(kind) = Some(Anchor {
                pct,
                captured_at: now,
                implied_limit: total / (pct / 100.0),
            });
        }
        self.settings.calibration.manual_at = Some(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-09-08T20:00:00Z";

    /// A throwaway home: `root/.claude/projects` for transcripts, plus the
    /// `root/.claude.json` the Usage cache is read out of.
    fn engine_at(name: &str, claude_json: Option<&str>, turns: bool) -> Engine {
        let root = std::env::temp_dir().join(format!("token-ledger-eng-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".claude/projects/p")).unwrap();
        if turns {
            std::fs::write(
                root.join(".claude/projects/p/a.jsonl"),
                format!(
                    "{}\n",
                    r#"{"type":"assistant","timestamp":"2026-09-08T19:00:00Z","cwd":"/w","sessionId":"s1","message":{"model":"claude-opus-5","id":"msg_1","usage":{"input_tokens":0,"output_tokens":1000000,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#
                ),
            )
            .unwrap();
        }
        if let Some(body) = claude_json {
            std::fs::write(root.join(".claude.json"), body).unwrap();
        }
        let mut eng = Engine::open_at(root.join("data"));
        eng.settings.claude_dir = Some(root.join(".claude"));
        let _ = eng.refresh(utc(NOW));
        eng
    }

    /// `cachedUsageUtilization` fetched at NOW, with one session percentage.
    fn cache_json(session_pct: u32) -> String {
        let ms = utc(NOW).timestamp_millis();
        format!(
            r#"{{"cachedUsageUtilization":{{"fetchedAtMs":{ms},"utilization":{{"limits":[
              {{"kind":"session","percent":{session_pct},"resets_at":"2026-09-08T22:00:00+00:00"}}]}}}}}}"#
        )
    }

    #[test]
    fn missing_claude_json_is_diagnosed_as_no_file() {
        let eng = engine_at("nofile", None, true);
        assert_eq!(eng.cache_diag.reason, "no_file");
        assert!(eng.cache_diag.path.ends_with(".claude.json"));
        assert!(eng.usage_cache.is_none());
    }

    #[test]
    fn config_without_a_usage_reading_is_diagnosed_as_no_key() {
        let eng = engine_at("nokey", Some(r#"{"numStartups":3}"#), true);
        assert_eq!(eng.cache_diag.reason, "no_key");
    }

    #[test]
    fn readings_under_ten_percent_report_what_they_topped_out_at() {
        let eng = engine_at("low", Some(&cache_json(4)), true);
        assert_eq!(eng.cache_diag.reason, "too_low");
        assert_eq!(eng.cache_diag.best_pct, Some(4.0));
        assert!(eng.settings.calibration.session.is_none(), "4% must not anchor");
    }

    #[test]
    fn a_usable_reading_anchors_and_reports_ok() {
        let eng = engine_at("ok", Some(&cache_json(50)), true);
        assert_eq!(eng.cache_diag.reason, "ok");
        let a = eng.settings.calibration.session.expect("anchored");
        assert_eq!(a.pct, 50.0);
        assert!((a.implied_limit - 50.0).abs() < 1e-6, "$25 of output at 50% implies a $50 limit");
    }

    #[test]
    fn a_usable_reading_with_no_transcripts_is_diagnosed_as_no_usage() {
        let eng = engine_at("nousage", Some(&cache_json(50)), false);
        assert_eq!(eng.cache_diag.reason, "no_usage");
        assert!(eng.settings.calibration.session.is_none());
    }

    #[test]
    fn a_bad_config_dir_still_diagnoses_the_cache() {
        let mut eng = engine_at("baddir", Some(&cache_json(50)), true);
        eng.settings.claude_dir = Some(std::path::PathBuf::from("/nonexistent/token-ledger"));
        // The scan fails, but the entry screen still needs something to say.
        assert!(eng.refresh(utc(NOW)).is_err());
        assert_eq!(eng.cache_diag.reason, "no_file");
    }
}
