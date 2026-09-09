//! Everything the app does that isn't a window: own the index and settings,
//! refresh on demand, produce snapshots, take calibration input. The Tauri
//! layer and `ledger-cli` are both thin shells over this.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::ledger::snapshot::{build_snapshot, window_total, Limits};
use crate::ledger::usagecache::{self, Reading, UsageCache};
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
        let dir = self
            .settings
            .projects_dir()
            .ok_or_else(|| "could not locate the Claude Code config directory".to_string())?;
        if !dir.is_dir() {
            return Err(format!("no transcripts directory at {}", dir.display()));
        }
        let stats = self.index.refresh(&dir, now);
        self.last_scan = stats.clone();
        self.last_error = None;
        self.absorb_usage_cache();
        Ok(stats)
    }

    /// Path of Claude Code's `.claude.json` for the configured directory.
    pub fn usage_cache_path(&self) -> Option<std::path::PathBuf> {
        self.settings.claude_dir().map(|d| usagecache::default_path(&d))
    }

    /// Re-anchor from Claude Code's cached Usage-tab reading when it is newer
    /// than anything we've applied — and never over a later manual entry.
    pub fn absorb_usage_cache(&mut self) {
        let Some(path) = self.usage_cache_path() else { return };
        let Some(cache) = usagecache::read(&path) else { return };
        let cal = &self.settings.calibration;
        let already = cal.auto_fetched_at.map_or(false, |t| t >= cache.fetched_at);
        let manual_newer = cal.manual_at.map_or(false, |m| m >= cache.fetched_at);
        self.usage_cache = Some(cache.clone());
        if already || manual_newer {
            return;
        }
        self.apply_cache(&cache);
    }

    fn apply_cache(&mut self, cache: &UsageCache) {
        let at = cache.fetched_at;
        let prices = self.settings.prices.clone();

        // Reset instants are exact and always worth taking.
        if let Some(Reading { resets_at: Some(r), .. }) = cache.session {
            self.settings.calibration.session_reset_at = Some(r);
        }
        if let Some(Reading { resets_at: Some(r), .. }) = cache.weekly.or(cache.fable) {
            self.settings.calibration.weekly_reset = Some(WeeklyReset::from_instant(r));
        }

        // Percentages are integers; below ~10% the rounding error swamps the
        // implied limit, so keep whatever anchor we already have.
        let readings = [
            (WindowKind::Session, cache.session),
            (WindowKind::Weekly, cache.weekly),
            (WindowKind::Fable, cache.fable),
        ];
        for (kind, r) in readings {
            let Some(r) = r else { continue };
            if r.percent < 10.0 {
                continue;
            }
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
            let total = window_total(&self.index, &prices, &clipped);
            if total <= 0.0 {
                continue;
            }
            *self.settings.calibration.anchor_mut(kind) = Some(Anchor {
                pct: r.percent,
                captured_at: at,
                implied_limit: total / (r.percent / 100.0),
            });
        }
        self.settings.calibration.auto_fetched_at = Some(at);
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
