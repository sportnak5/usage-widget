//! User settings: where Claude Code lives, calibration anchors, plan label,
//! refresh cadence, and the widget's remembered geometry. One JSON file in
//! the app's data directory; every write is atomic.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ledger::{Calibration, PriceTable};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetGeometry {
    pub x: i32,
    pub y: i32,
    pub expanded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Overrides auto-detection of `~/.claude` / `$CLAUDE_CONFIG_DIR`.
    pub claude_dir: Option<PathBuf>,
    pub plan: String,
    pub boost: Option<String>,
    pub refresh_secs: u64,
    pub calibration: Calibration,
    pub prices: PriceTable,
    pub widget: Option<WidgetGeometry>,
    pub show_widget: bool,
    pub theme: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            claude_dir: None,
            plan: "Max (20x)".into(),
            boost: None,
            refresh_secs: 60,
            calibration: Calibration::default(),
            prices: PriceTable::default(),
            widget: None,
            show_widget: true,
            theme: None,
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
