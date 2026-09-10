//! User settings: where Claude Code lives, calibration anchors, plan label,
//! refresh cadence, and the widget's remembered geometry. One JSON file in
//! the app's data directory; every write is atomic.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ledger::{Calibration, PriceTable};

pub const DEFAULT_REFRESH_SECS: u64 = 15;
pub const DEFAULT_LIST_ROWS: usize = 25;
pub const DEFAULT_WIDGET_ROWS: usize = 5;
/// Bounds on both. The ceilings are what the snapshot can carry without the
/// payload — which every refresh sends to both windows — growing teeth.
pub const MAX_LIST_ROWS: usize = 200;
pub const MAX_WIDGET_ROWS: usize = 25;

/// How the conversation lists in both windows are ranked. One setting, shared:
/// the widget and the ledger are two views of the same list, so flipping it in
/// either place has to flip it in both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadSort {
    /// Heaviest first — whose usage this window is actually made of.
    #[default]
    Usage,
    /// Most recently active first — what you were just doing.
    Recent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetGeometry {
    pub x: i32,
    pub y: i32,
    pub expanded: bool,
    /// How far the card is zoomed. The widget has no reflowing layout, so a
    /// resize scales the whole card instead of rearranging it.
    #[serde(default = "unit_scale")]
    pub scale: f64,
}

fn unit_scale() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Overrides auto-detection of `~/.claude` / `$CLAUDE_CONFIG_DIR`.
    pub claude_dir: Option<PathBuf>,
    pub plan: String,
    pub boost: Option<String>,
    /// How often the transcripts are rescanned. The dials would be happy with
    /// a minute — a percentage moves in whole numbers — but the conversation
    /// list now carries live state, and a spinner that takes a minute to
    /// appear or clear is worse than no spinner.
    pub refresh_secs: u64,
    pub calibration: Calibration,
    pub prices: PriceTable,
    pub widget: Option<WidgetGeometry>,
    pub show_widget: bool,
    /// Float the widget above every other window. Off by default: it sits at
    /// desktop level, so ordinary windows cover it like a homescreen widget.
    pub widget_on_top: bool,
    pub theme: Option<String>,
    /// Rank the conversation lists by weight or by recency.
    pub thread_sort: ThreadSort,
    /// How many conversations the ledger's list shows.
    pub list_rows: usize,
    /// How many the widget's expanded list shows. Its own number: the widget
    /// is a glance, the ledger is the place you go to read.
    pub widget_rows: usize,
    /// When each conversation was last opened in the ledger. Read receipts
    /// Claude Code doesn't keep, so the app keeps its own.
    pub seen: HashMap<String, DateTime<Utc>>,
    /// Nothing older than this counts as unread. Stamped the first time the
    /// app runs with unread tracking, so a history of finished conversations
    /// doesn't arrive as a wall of badges.
    pub unread_since: Option<DateTime<Utc>>,
    /// Read Claude Code's own login and ask Anthropic for the percentages
    /// directly. Off until the user turns it on: it reads a credential another
    /// application owns, so it is never something the app just starts doing.
    pub live_readings: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            claude_dir: None,
            plan: "Max (20x)".into(),
            boost: None,
            refresh_secs: DEFAULT_REFRESH_SECS,
            calibration: Calibration::default(),
            prices: PriceTable::default(),
            widget: None,
            show_widget: true,
            widget_on_top: false,
            theme: None,
            thread_sort: ThreadSort::default(),
            list_rows: DEFAULT_LIST_ROWS,
            widget_rows: DEFAULT_WIDGET_ROWS,
            seen: HashMap::new(),
            unread_since: None,
            live_readings: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, path)
    }

    /// `$CLAUDE_CONFIG_DIR`, else the override, else `~/.claude`.
    pub fn claude_dir(&self) -> Option<PathBuf> {
        if let Some(d) = &self.claude_dir {
            return Some(d.clone());
        }
        if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
            return Some(PathBuf::from(d));
        }
        dirs::home_dir().map(|h| h.join(".claude"))
    }

    /// Mark a conversation read as of `at`, and forget receipts for threads
    /// the ledger no longer holds — the map is bounded by the index, not by
    /// how long the app has been installed.
    pub fn mark_seen(&mut self, session: &str, at: DateTime<Utc>, live: &HashSet<String>) {
        self.seen.insert(session.to_string(), at);
        self.seen.retain(|sid, _| live.contains(sid.as_str()));
    }

    /// The instant a conversation's unread state is measured against: your
    /// last look at it, or the baseline, whichever is later.
    pub fn seen_at(&self, session: &str) -> Option<DateTime<Utc>> {
        match (self.seen.get(session).copied(), self.unread_since) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    /// How many conversations the snapshot has to carry: enough for whichever
    /// window wants more, since both read the same ranked list.
    pub fn session_rows(&self) -> usize {
        self.list_rows.max(self.widget_rows)
    }

    pub fn projects_dir(&self) -> Option<PathBuf> {
        self.claude_dir().map(|d| d.join("projects"))
    }
}

/// `~/Library/Application Support/dev.tokenledger.app` on macOS,
/// `%APPDATA%\dev.tokenledger.app` on Windows, `~/.local/share/...` on Linux.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dev.tokenledger.app")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_read_receipt_never_moves_backwards_past_the_baseline() {
        let mut s = Settings { unread_since: Some(utc("2026-09-08T12:00:00Z")), ..Default::default() };
        assert_eq!(s.seen_at("s1"), Some(utc("2026-09-08T12:00:00Z")), "the baseline covers every thread");

        let live: HashSet<String> = ["s1".to_string(), "s2".to_string()].into_iter().collect();
        // A receipt older than the baseline would un-read what the baseline
        // already covered, so the later of the two wins.
        s.mark_seen("s1", utc("2026-09-08T09:00:00Z"), &live);
        assert_eq!(s.seen_at("s1"), Some(utc("2026-09-08T12:00:00Z")));
        s.mark_seen("s1", utc("2026-09-08T15:00:00Z"), &live);
        assert_eq!(s.seen_at("s1"), Some(utc("2026-09-08T15:00:00Z")));
    }

    #[test]
    fn receipts_for_threads_the_ledger_forgot_are_dropped() {
        let mut s = Settings::default();
        let all: HashSet<String> = ["s1".into(), "s2".into()].into_iter().collect();
        s.mark_seen("s1", utc("2026-09-08T12:00:00Z"), &all);
        s.mark_seen("s2", utc("2026-09-08T12:00:00Z"), &all);
        // s1 has aged out of the index; only what is still live survives.
        let live: HashSet<String> = ["s2".into(), "s3".into()].into_iter().collect();
        s.mark_seen("s3", utc("2026-09-08T13:00:00Z"), &live);
        let mut keys: Vec<&str> = s.seen.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["s2", "s3"]);
    }
}
