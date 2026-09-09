//! `ledger-cli` — run the whole data layer without a GUI.
//!
//!   ledger-cli                       refresh, print a summary
//!   ledger-cli --json                refresh, print the full snapshot JSON
//!   ledger-cli --calibrate session=78,weekly=28,fable=53,resets-in=226,weekly-reset=Tue@01:00
//!   ledger-cli --data DIR            use a separate settings/index directory
//!   ledger-cli --projects DIR        read transcripts from DIR instead of ~/.claude/projects
//!
//! Reads and writes the same settings/index files as the app, so a
//! calibration done here shows up in the widget.

use std::path::PathBuf;

use chrono::{Utc, Weekday};
use token_ledger::engine::{CalibrationInput, Engine};
use token_ledger::ledger::WeeklyReset;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut json = false;
    let mut data: Option<PathBuf> = None;
    let mut projects: Option<PathBuf> = None;
    let mut calibrate: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--data" => data = args.next().map(PathBuf::from),
            "--projects" => projects = args.next().map(PathBuf::from),
            "--calibrate" => calibrate = args.next(),
            "-h" | "--help" => {
                eprintln!("{}", include_str!("cli.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!").trim_start()).collect::<Vec<_>>().join("\n"));
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let mut engine = match data {
        Some(d) => Engine::open_at(d),
        None => Engine::open(),
    };
    if let Some(p) = projects {
        engine.settings.claude_dir = Some(p.parent().map(|x| x.to_path_buf()).unwrap_or(p.clone()));
        if p.file_name().map(|n| n != "projects").unwrap_or(true) {
            // Allow pointing straight at a directory of <slug>/<session>.jsonl.
            engine.settings.claude_dir = Some(p.clone());
        }
    }

    let now = Utc::now();
    match engine.refresh(now) {
        Ok(s) => eprintln!(
            "scan: {} files total, {} in window, {} read, {:.1} MB, {} lines, +{} records, {} dupes skipped, {} retained, {} ms",
            s.files_total, s.files_in_window, s.files_read, s.bytes_read as f64 / 1e6, s.lines_seen,
            s.records_added, s.duplicates_skipped, s.records_retained, s.duration_ms
        ),
        Err(e) => {
            eprintln!("refresh failed: {e}");
            std::process::exit(1);
        }
    }

    if let Some(spec) = calibrate {
        let mut input = CalibrationInput::default();
        for part in spec.split(',') {
            let Some((k, v)) = part.split_once('=') else { continue };
            match k.trim() {
                "session" => input.session_pct = v.parse().ok(),
                "weekly" => input.weekly_pct = v.parse().ok(),
                "fable" => input.fable_pct = v.parse().ok(),
                "resets-in" => input.session_resets_in_minutes = v.parse().ok(),
                "plan" => input.plan = Some(v.to_string()),
                "boost" => input.boost = Some(v.to_string()),
                "weekly-reset" => input.weekly_reset = parse_weekly(v),
                _ => eprintln!("ignoring unknown calibration key {k}"),
            }
        }
        if let Err(e) = engine.calibrate(input, now) {
            eprintln!("calibration failed: {e}");
            std::process::exit(1);
        }
        eprintln!("calibrated: {:?}", engine.settings.calibration);
    }

    engine.save();
    if let Some(e) = &engine.last_error {
        eprintln!("warning: {e}");
    }

    match &engine.usage_cache {
        Some(c) => eprintln!(
            "usage cache: fetched {} — session {:?} weekly {:?} fable {:?}",
            c.fetched_at.with_timezone(&chrono::Local).format("%a %b %d %H:%M"),
            c.session.map(|r| r.percent), c.weekly.map(|r| r.percent), c.fable.map(|r| r.percent)
        ),
        None => eprintln!("usage cache: none found at {:?}", engine.usage_cache_path()),
    }

    let snap = engine.snapshot(now);
    if json {
        println!("{}", serde_json::to_string_pretty(&snap).unwrap());
        return;
    }

    for w in &snap.windows {
        let pct = w.pct.map(|p| format!("{p:.0}%")).unwrap_or_else(|| "—".into());
        let pace = w.pace.map(|p| format!("{p:.2}×")).unwrap_or_else(|| "—".into());
        println!(
            "\n== {:<22} {:>4} used  pace {:>6}  ${:>8.2} weighted  {:>5} msgs  {:>6.0}M raw  [{} → {}]{}",
            w.label, pct, pace, w.total_cost, w.messages, w.total_raw as f64 / 1e6,
            w.start.with_timezone(&chrono::Local).format("%a %H:%M"),
            w.reset.with_timezone(&chrono::Local).format("%a %H:%M"),
            if w.idle { "  (idle: next block starts with your next message)" } else if w.boundary_known { "" } else { "  (rolling; boundary unknown)" }
        );
        println!("   {:<44}{:>10}{:>8}{:>9}", "MODEL", "weighted", "share", "of cap");
        for e in &w.by_model {
            println!("   {:<44}{:>10.2}{:>7.1}%{:>8}", key(e), e.cost, e.share, e.pct.map(|p| format!("{p:.1}%")).unwrap_or_else(|| "—".into()));
        }
        println!("   {:<44}{:>10}{:>8}{:>9}", "SESSION", "weighted", "share", "of cap");
        for e in w.by_session.iter().take(8) {
            let name = e.title.clone().unwrap_or_else(|| key(e));
            println!("   {:<44}{:>10.2}{:>7.1}%{:>8}", trunc(&name, 43), e.cost, e.share, e.pct.map(|p| format!("{p:.1}%")).unwrap_or_else(|| "—".into()));
        }
    }
}

fn key(e: &token_ledger::ledger::snapshot::Entry) -> String {
    match &e.key {
        token_ledger::ledger::snapshot::EntryKey::One(s) => s.clone(),
        token_ledger::ledger::snapshot::EntryKey::Session([s, _]) => s[..8.min(s.len())].to_string(),
    }
}

fn trunc(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// `Tue@01:00`
fn parse_weekly(v: &str) -> Option<WeeklyReset> {
    let (day, time) = v.split_once('@')?;
    let weekday = match day.to_ascii_lowercase().as_str() {
        "mon" => Weekday::Mon, "tue" => Weekday::Tue, "wed" => Weekday::Wed, "thu" => Weekday::Thu,
        "fri" => Weekday::Fri, "sat" => Weekday::Sat, "sun" => Weekday::Sun, _ => return None,
    };
    let (h, m) = time.split_once(':')?;
    Some(WeeklyReset { weekday, hour: h.parse().ok()?, minute: m.parse().ok()? })
}
