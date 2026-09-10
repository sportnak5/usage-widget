//! Everything the app does that isn't a window: own the index and settings,
//! refresh on demand, produce snapshots, take calibration input. The Tauri
//! layer and `ledger-cli` are both thin shells over this.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ledger::sessions;
use crate::ledger::snapshot::{apply_activity, build_snapshot, window_total, Limits};
use crate::ledger::usageapi::{self, FetchError};
use crate::ledger::usagecache::{self, Reading, Status, UsageCache};
use crate::ledger::windows::{resolve, Anchor, Window};
use crate::ledger::{Activity, Index, RemoteSession, ScanStats, Snapshot, WeeklyReset, WindowKind};
use crate::settings::{data_dir, Settings, ThreadSort};

pub struct Engine {
    pub settings: Settings,
    pub index: Index,
    pub settings_path: PathBuf,
    pub index_path: PathBuf,
    pub last_scan: ScanStats,
    pub last_error: Option<String>,
    pub usage_cache: Option<UsageCache>,
    pub cache_diag: CacheDiag,
    /// Transcript tails, remembered between refreshes. Not persisted: it is a
    /// cache over files we can always read again.
    pub activity: Activity,
    /// The account's session list, from every device. Not persisted: it is a
    /// live reading, and a stale one on disk would be worse than none.
    pub remote: Remote,
}

/// The last word from the account-wide session list.
///
/// Rows outlive a failed fetch on purpose: a dropped connection should not
/// empty the list the user is looking at, it should mark it as possibly stale.
#[derive(Clone, Debug, Default)]
pub struct Remote {
    pub rows: Vec<RemoteSession>,
    /// When we last *asked*, successfully or not — what the throttle is
    /// measured against, so a failing endpoint is not asked any harder than a
    /// working one.
    pub checked_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
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
    /// Largest percentage the reading carried — what "too low" was too low by.
    pub best_pct: Option<f64>,
    /// This reading came from the Usage endpoint, not from the file. Live
    /// readings are as fresh as the refresh that fetched them.
    pub live: bool,
    /// A token is stored, whether or not the last fetch worked.
    pub connected: bool,
    /// Why the last live fetch failed, when one was attempted and did.
    pub live_error: Option<String>,
    /// The live token was rejected, so reconnecting is the fix.
    pub live_auth_failed: bool,
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
            activity: Activity::default(),
            remote: Remote::default(),
        }
    }

    pub fn open() -> Engine {
        Engine::open_at(data_dir())
    }

    /// Switch live readings on only if they actually work. Turning the setting
    /// on when the credential can't be read would displace a fallback that
    /// does work, so the check happens before the flag moves.
    ///
    /// On macOS the first read raises the OS prompt asking whether Token
    /// Ledger may open Claude Code's Keychain item, so this is also the moment
    /// the user is asked — deliberately, while they are looking at settings.
    pub fn set_live_readings(&mut self, on: bool) -> Result<(), String> {
        if on {
            // Turning it on is the user saying "look again", so a failure
            // remembered from earlier in this run must not answer for the store.
            usageapi::forget_cached_credential();
            usageapi::fetch().map_err(|e| e.message())?;
        }
        self.settings.live_readings = on;
        Ok(())
    }

    /// Can live readings work here, without changing any setting? Drives the
    /// "Check" button and the first-run step.
    pub fn probe_live_readings(&self) -> Result<String, String> {
        // Same for Check: it is the way back from a remembered failure.
        usageapi::forget_cached_credential();
        let src = usageapi::credential_source().map_err(|e| e.message())?;
        let cache = usageapi::fetch().map_err(|e| e.message())?;
        let best = [cache.session, cache.weekly, cache.fable]
            .iter()
            .filter_map(|r| r.map(|r| r.percent))
            .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))));
        Ok(match best {
            Some(p) => format!("Working — read {} and got {p:.0}% on the fullest window.", src.label()),
            None => format!("Read {}, but Anthropic returned no percentages.", src.label()),
        })
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
        // Unread is measured against a baseline so an installed-today app
        // doesn't open on a wall of badges for conversations that ended last
        // week. Written through immediately: a baseline that isn't persisted
        // would move to "now" on every refresh, and nothing would ever be
        // unread.
        if self.settings.unread_since.is_none() {
            self.settings.unread_since = Some(now);
            let _ = self.settings.save(&self.settings_path);
        }
        self.absorb_usage_cache();
        self.absorb_remote_sessions(now);
        scan
    }

    /// Pull the account's session list, so conversations running on the user's
    /// other machines are visible here.
    ///
    /// Same shape as the Usage fetch and for the same reasons: gated on live
    /// readings, because it is the same credential and the same consent;
    /// throttled, because the endpoint next door rate-limits readily; and
    /// never able to break a snapshot, because a failed fetch only annotates
    /// the rows we already had.
    pub fn absorb_remote_sessions(&mut self, now: DateTime<Utc>) {
        if !self.settings.live_readings {
            // Off is not a failure, and it must not leave rows on screen that
            // nothing is refreshing any more.
            self.remote = Remote::default();
            return;
        }
        let just_checked = self
            .remote
            .checked_at
            .map_or(false, |t| now - t < Duration::seconds(30) && now >= t);
        if just_checked {
            return;
        }
        self.remote.checked_at = Some(now);
        match sessions::fetch() {
            Ok(mut rows) => {
                sessions::classify(&mut rows, &self.index.bridge_sessions());
                // The local lists hold 8 days; a remote row older than that
                // has nothing to sit beside.
                let cutoff = now - Duration::days(crate::ledger::index::RETENTION_DAYS);
                rows.retain(|r| r.last >= cutoff);
                self.remote.rows = rows;
                self.remote.error = None;
            }
            Err(FetchError::Off) => {}
            Err(e) => self.remote.error = Some(e.message()),
        }
    }

    /// Path of Claude Code's `.claude.json` for the configured directory.
    pub fn usage_cache_path(&self) -> Option<std::path::PathBuf> {
        self.settings.claude_dir().map(|d| usagecache::default_path(&d))
    }

    /// Re-anchor from the freshest Usage reading we can get: the endpoint
    /// when a token is connected, Claude Code's cached copy otherwise.
    ///
    /// Always records why it could or couldn't, even on the paths that change
    /// nothing: the entry screen leads with what to do next, so it needs to
    /// know whether any of it would actually help.
    pub fn absorb_usage_cache(&mut self) {
        let projects = self
            .settings
            .projects_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let path = self.usage_cache_path();

        // A live reading beats anything on disk, including a manual entry:
        // it is the number the user would read off the Usage tab themselves.
        let mut live_error = None;
        let mut live_auth_failed = false;
        // The refresh interval floors at 15s and a rescan can be triggered by
        // hand on top of that; a percentage that moves in whole numbers does
        // not need asking for that often.
        let just_fetched = self.cache_diag.live
            && self
                .usage_cache
                .as_ref()
                .map_or(false, |c| Utc::now() - c.fetched_at < Duration::seconds(30));
        if just_fetched {
            return;
        }
        if self.settings.live_readings {
            match usageapi::fetch() {
                Ok(cache) => {
                    self.apply_reading(cache, path.as_deref(), &projects, true, None, false, true);
                    return;
                }
                Err(FetchError::Off) => {}
                Err(e) => {
                    live_auth_failed = e.is_auth();
                    live_error = Some(e.message());
                }
            }
        }

        let Some(path) = path else {
            self.usage_cache = None;
            self.cache_diag = CacheDiag {
                reason: "no_dir".into(),
                projects_dir: projects,
                connected: self.settings.live_readings,
                live_error,
                live_auth_failed,
                ..CacheDiag::default()
            };
            return;
        };
        let fail = |eng: &mut Engine, reason: &str, detail: Option<String>| {
            eng.usage_cache = None;
            eng.cache_diag = CacheDiag {
                projects_dir: projects.clone(),
                connected: eng.settings.live_readings,
                live_error: live_error.clone(),
                live_auth_failed,
                ..CacheDiag::at(&path, reason, detail)
            };
        };
        let cache = match usagecache::read_status(&path) {
            Status::Ok(c) => c,
            Status::NoFile => return fail(self, "no_file", None),
            Status::NoKey => return fail(self, "no_key", None),
            Status::Unreadable(e) => return fail(self, "unreadable", Some(e)),
        };

        // The file is written whenever someone runs `/usage`, so it can be
        // older than what we already applied — and a manual entry made since
        // is the user's most recent word on the subject.
        let cal = &self.settings.calibration;
        let already = cal.auto_fetched_at.map_or(false, |t| t >= cache.fetched_at);
        let manual_newer = cal.manual_at.map_or(false, |m| m >= cache.fetched_at);
        let apply = !(already || manual_newer);
        self.apply_reading(cache, Some(&path), &projects, false, live_error, live_auth_failed, apply);
    }

    /// Diagnose a reading, and anchor from it unless told not to.
    fn apply_reading(
        &mut self,
        cache: UsageCache,
        path: Option<&std::path::Path>,
        projects: &str,
        live: bool,
        live_error: Option<String>,
        live_auth_failed: bool,
        anchor: bool,
    ) {
        let (reason, best_pct, anchors) = self.evaluate(&cache);
        self.usage_cache = Some(cache.clone());
        self.cache_diag = CacheDiag {
            path: path.map(|p| p.display().to_string()).unwrap_or_default(),
            projects_dir: projects.to_string(),
            reason,
            detail: None,
            fetched_at: Some(cache.fetched_at),
            best_pct,
            live,
            connected: self.settings.live_readings,
            live_error,
            live_auth_failed,
        };
        if anchor {
            self.apply_cache(&cache, anchors);
        }
    }

    /// What this reading would anchor, and why not, without touching settings.
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

        let mut any_limit = false;
        let mut any_reading = false;
        let mut anchors = Vec::new();
        for (kind, r) in readings {
            let Some(r) = r else { continue };
            any_reading = true;
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

            // Percentages are integers; below ~10% the rounding error swamps
            // the implied limit. That is a reason not to re-derive the limit —
            // never a reason to throw the reading away, which is what the dial
            // is actually built on. Keep whatever limit we already had.
            let derivable = r.percent >= 10.0 && total > 0.0;
            let implied_limit = if derivable {
                any_limit = true;
                total / (r.percent / 100.0)
            } else {
                self.settings.calibration.anchor(kind).map(|a| a.implied_limit).unwrap_or(0.0)
            };
            anchors.push((kind, Anchor { pct: r.percent, captured_at: at, implied_limit }));
        }
        let reason = if any_limit {
            "ok"
        } else if !any_reading {
            "no_readings"
        } else if best_pct.map_or(false, |p| p >= 10.0) {
            // The reading says we've used something; our transcripts disagree.
            "no_usage"
        } else {
            "too_low"
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

    pub fn snapshot(&mut self, now: DateTime<Utc>) -> Snapshot {
        let mut snap = build_snapshot(
            &self.index,
            &self.settings.prices,
            &self.settings.calibration,
            &self.settings.plan,
            self.settings.boost.as_deref(),
            now,
            self.last_scan.clone(),
            &Limits::from_settings(&self.settings),
            self.usage_cache.clone(),
            self.cache_diag.clone(),
            self.settings.thread_sort,
        );
        apply_activity(&mut snap, &self.index, &mut self.activity, &self.settings, now);
        // Not built from the index, so not `build_snapshot`'s to produce —
        // and republished as-is when a window asks for a snapshot without a
        // refresh, which is what keeps remote rows on screen between fetches.
        snap.remote_threads = self.remote.rows.clone();
        snap.remote_error = self.remote.error.clone();
        snap
    }

    /// Rank the conversation lists by weight or by recency. One setting for
    /// both windows — the snapshot carries it, so flipping it anywhere shows
    /// up everywhere.
    pub fn set_thread_sort(&mut self, sort: ThreadSort) {
        self.settings.thread_sort = sort;
    }

    /// The user opened a conversation: everything the assistant has said in it
    /// up to now counts as read.
    pub fn mark_thread_read(&mut self, session: &str, now: DateTime<Utc>) {
        let live = self.index.records().map(|r| r.session.clone()).collect();
        self.settings.mark_seen(session, now, &live);
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
        cache_json_at(session_pct, utc(NOW).timestamp_millis())
    }

    /// The same, fetched at a time the caller picks — a second reading has to
    /// be newer than the first or it is ignored as already applied.
    fn cache_json_at(session_pct: u32, ms: i64) -> String {
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
    fn readings_under_ten_percent_are_shown_but_imply_no_limit() {
        let eng = engine_at("low", Some(&cache_json(4)), true);
        assert_eq!(eng.cache_diag.reason, "too_low");
        assert_eq!(eng.cache_diag.best_pct, Some(4.0));
        let a = eng.settings.calibration.session.expect("4% still anchors the display");
        assert_eq!(a.pct, 4.0);
        assert_eq!(a.implied_limit, 0.0, "4% must not imply a limit");
    }

    #[test]
    fn a_low_reading_keeps_the_limit_an_earlier_one_implied() {
        let mut eng = engine_at("keeplimit", Some(&cache_json(50)), true);
        let first = eng.settings.calibration.session.unwrap().implied_limit;
        assert!(first > 0.0);

        // A later, lower reading: the percentage is worth having, the limit
        // it would imply is not.
        let path = eng.usage_cache_path().unwrap();
        std::fs::write(&path, cache_json_at(4, utc(NOW).timestamp_millis() + 1)).unwrap();
        eng.absorb_usage_cache();

        let a = eng.settings.calibration.session.unwrap();
        assert_eq!(a.pct, 4.0);
        assert_eq!(a.implied_limit, first, "the earlier limit carries forward");
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
        // Nothing to divide by, so no limit — but the reading still shows.
        assert_eq!(eng.settings.calibration.session.unwrap().implied_limit, 0.0);
    }

    /// The session list is the same credential and the same consent as the
    /// Usage endpoint, so the same switch has to govern it — and switching it
    /// off has to take the rows away, not freeze them on screen.
    #[test]
    fn remote_sessions_are_off_when_live_readings_are() {
        let mut eng = engine_at("remote-off", None, true);
        assert!(!eng.settings.live_readings);
        eng.remote.rows = vec![RemoteSession { id: "cse_stale".into(), ..Default::default() }];
        eng.remote.checked_at = Some(utc(NOW));
        eng.remote.error = Some("something".into());

        eng.absorb_remote_sessions(utc(NOW));
        assert!(eng.remote.rows.is_empty());
        assert!(eng.remote.checked_at.is_none(), "nothing was asked, so nothing was checked");
        assert!(eng.remote.error.is_none());
        assert!(eng.snapshot(utc(NOW)).remote_threads.is_empty());
        // The label stands on its own: local rows are tagged with it whether
        // or not any remote row ever arrives.
        assert!(!eng.snapshot(utc(NOW)).device_label.is_empty());
    }

    /// A fetch inside the throttle window must not go near the network — the
    /// endpoint's neighbour 429s readily, and a manual rescan can arrive on
    /// top of the timer.
    #[test]
    fn a_recent_check_is_not_repeated() {
        let mut eng = engine_at("remote-throttle", None, true);
        eng.settings.live_readings = true;
        eng.remote.rows = vec![RemoteSession { id: "cse_good".into(), ..Default::default() }];
        eng.remote.checked_at = Some(utc(NOW));

        eng.absorb_remote_sessions(utc(NOW) + Duration::seconds(5));
        assert_eq!(eng.remote.checked_at, Some(utc(NOW)), "no second ask within 30s");
        assert_eq!(eng.remote.rows.len(), 1);
        assert!(eng.remote.error.is_none());
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
