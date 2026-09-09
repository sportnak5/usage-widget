//! Aggregate the index into what the UI draws. Shape mirrors the POC's
//! `usage.json` so the same rendering code can consume either.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::index::{Index, ScanStats};
use super::pricing::PriceTable;
use super::record::{Rec, TitleKind, Usage};
use super::usagecache::UsageCache;
use super::windows::{ramp, resolve, Calibration, Window};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntryKey {
    One(String),
    /// `[session_id, cwd]`
    Session([String; 2]),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelPart {
    pub model: String,
    pub cost: f64,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub key: EntryKey,
    pub title: Option<String>,
    pub title_kind: Option<String>,
    /// Weighted dollars.
    pub cost: f64,
    pub raw: u64,
    /// Message count.
    pub n: u64,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
    /// % of this window's usage.
    pub share: f64,
    /// % of this window's *limit*. None until calibrated.
    pub pct: Option<f64>,
    pub models: Vec<ModelPart>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Kinds {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowOut {
    pub id: String,
    pub label: String,
    pub sub: String,
    pub start: DateTime<Utc>,
    pub reset: DateTime<Utc>,
    pub boundary_known: bool,
    pub idle: bool,
    pub calibrated: bool,
    /// Estimated % of the limit used. None until calibrated.
    pub pct: Option<f64>,
    /// used-fraction ÷ elapsed-fraction; 1.0 = exactly on pace. None until calibrated.
    pub pace: Option<f64>,
    /// 0..1 position on the green→red ramp. None until calibrated.
    pub ramp: Option<f64>,
    pub elapsed_frac: f64,
    pub total_cost: f64,
    pub total_raw: u64,
    pub messages: u64,
    pub limit: Option<f64>,
    pub kinds: Kinds,
    pub by_model: Vec<Entry>,
    pub by_project: Vec<Entry>,
    pub by_session: Vec<Entry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub generated_at: DateTime<Utc>,
    pub plan: String,
    pub boost: Option<String>,
    pub calibrated: bool,
    pub windows: Vec<WindowOut>,
    pub scan: ScanStats,
    pub index_records: usize,
    /// Claude Code's own cached Usage-tab reading, if the file exists.
    pub usage_cache: Option<UsageCache>,
    /// Where the anchors came from: "auto" (Claude Code's cache), "manual", or "none".
    pub calibration_source: String,
}

#[derive(Default)]
struct Acc {
    cost: f64,
    usage: Usage,
    n: u64,
    first: Option<DateTime<Utc>>,
    last: Option<DateTime<Utc>>,
    models: HashMap<String, (f64, Usage)>,
}

impl Acc {
    fn push(&mut self, r: &Rec, cost: f64) {
        self.cost += cost;
        self.usage.add(&r.usage);
        self.n += 1;
        self.first = Some(self.first.map_or(r.ts, |f| f.min(r.ts)));
        self.last = Some(self.last.map_or(r.ts, |l| l.max(r.ts)));
        let m = self.models.entry(r.model.clone()).or_default();
        m.0 += cost;
        m.1.add(&r.usage);
    }
}

pub struct Limits {
    pub projects: usize,
    pub sessions: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { projects: 12, sessions: 25 }
    }
}

/// Weighted total for one window — what calibration anchors against.
pub fn window_total(index: &Index, prices: &PriceTable, w: &Window) -> f64 {
    index
        .records()
        .filter(|r| w.contains(r.ts))
        .filter_map(|r| {
            let e = prices.lookup(&r.model)?;
            (!w.kind.fable_only() || e.fable).then(|| e.rates.cost(&r.usage))
        })
        .sum()
}

pub fn build_snapshot(
    index: &Index,
    prices: &PriceTable,
    cal: &Calibration,
    plan: &str,
    boost: Option<&str>,
    now: DateTime<Utc>,
    scan: ScanStats,
    limits: &Limits,
    usage_cache: Option<UsageCache>,
) -> Snapshot {
    let ts = index.timestamps_sorted();
    let windows = resolve(cal, now, &ts);
    let mut outs = Vec::with_capacity(3);

    for w in &windows {
        let mut total = Acc::default();
        let mut by_model: HashMap<String, Acc> = HashMap::new();
        let mut by_project: HashMap<String, Acc> = HashMap::new();
        // A conversation is one session id. Its cwd can change mid-session
        // (worktrees, `cd`), so remember every cwd it touched and show the
        // one most of its turns ran in.
        let mut by_session: HashMap<String, (Acc, HashMap<String, u64>)> = HashMap::new();

        for r in index.records().filter(|r| w.contains(r.ts)) {
            let Some(e) = prices.lookup(&r.model) else { continue };
            if w.kind.fable_only() && !e.fable {
                continue;
            }
            let cost = e.rates.cost(&r.usage);
            total.push(r, cost);
            by_model.entry(r.model.clone()).or_default().push(r, cost);
            by_project.entry(r.cwd.clone()).or_default().push(r, cost);
            let se = by_session.entry(r.session.clone()).or_default();
            se.0.push(r, cost);
            *se.1.entry(r.cwd.clone()).or_default() += 1;
        }

        let anchor = cal.anchor(w.kind);
        let limit = anchor.map(|a| a.implied_limit).filter(|l| *l > 0.0);
        let pct = limit.map(|l| total.cost / l * 100.0);
        let elapsed = w.elapsed_frac(now);
        let pace = pct.map(|p| p / (elapsed * 100.0));
        let rp = pct.zip(pace).map(|(p, pc)| ramp(p, pc));

        let tot = if total.cost > 0.0 { total.cost } else { f64::EPSILON };
        let entry = |key: EntryKey, title: Option<&super::record::Title>, a: Acc| -> Entry {
            let mut models: Vec<ModelPart> = a
                .models
                .into_iter()
                .map(|(model, (cost, u))| ModelPart {
                    model,
                    cost: round2(cost),
                    input: u.input,
                    output: u.output,
                    cache_write: u.cache_write(),
                    cache_read: u.cache_read,
                })
                .collect();
            models.sort_by(|x, y| y.cost.total_cmp(&x.cost));
            let share = a.cost / tot * 100.0;
            Entry {
                key,
                title: title.map(|t| t.text.clone()),
                title_kind: title.map(|t| match t.kind { TitleKind::Custom => "custom".into(), TitleKind::Prompt => "prompt".into() }),
                cost: round2(a.cost),
                raw: a.usage.raw(),
                n: a.n,
                first: a.first.unwrap_or(now),
                last: a.last.unwrap_or(now),
                share: round2(share),
                pct: pct.map(|p| round2(share / 100.0 * p)),
                models,
            }
        };

        let mut models: Vec<Entry> = by_model.into_iter().map(|(k, a)| entry(EntryKey::One(k), None, a)).collect();
        let mut projects: Vec<Entry> = by_project.into_iter().map(|(k, a)| entry(EntryKey::One(k), None, a)).collect();
        let mut sessions: Vec<Entry> = by_session
            .into_iter()
            .map(|(sid, (a, cwds))| {
                let cwd = cwds.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c).unwrap_or_default();
                let t = index.title(&sid);
                entry(EntryKey::Session([sid, cwd]), t, a)
            })
            .collect();
        for v in [&mut models, &mut projects, &mut sessions] {
            v.sort_by(|x, y| y.cost.total_cmp(&x.cost));
        }
        projects.truncate(limits.projects);
        sessions.truncate(limits.sessions);

        outs.push(WindowOut {
            id: w.kind.id().to_string(),
            label: w.kind.label().to_string(),
            sub: w.kind.sub().to_string(),
            start: w.start,
            reset: w.end,
            boundary_known: w.boundary_known,
            idle: w.idle,
            calibrated: limit.is_some(),
            pct: pct.map(round2),
            pace: pace.map(round2),
            ramp: rp.map(|x| (x * 1000.0).round() / 1000.0),
            elapsed_frac: (elapsed * 1000.0).round() / 1000.0,
            total_cost: round2(total.cost),
            total_raw: total.usage.raw(),
            messages: total.n,
            limit: limit.map(round2),
            kinds: Kinds {
                input: total.usage.input,
                output: total.usage.output,
                cache_write: total.usage.cache_write(),
                cache_read: total.usage.cache_read,
            },
            by_model: models,
            by_project: projects,
            by_session: sessions,
        });
    }

    Snapshot {
        generated_at: now,
        plan: plan.to_string(),
        boost: boost.map(str::to_string),
        calibrated: outs.iter().all(|o| o.calibrated),
        windows: outs,
        scan,
        index_records: index.len(),
        usage_cache,
        calibration_source: match (cal.manual_at, cal.auto_fetched_at) {
            (Some(m), Some(a)) if m > a => "manual",
            (_, Some(_)) => "auto",
            (Some(_), None) => "manual",
            (None, None) => "none",
        }
        .to_string(),
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::windows::Anchor;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn index_with(name: &str, recs: &[(&str, &str, &str, &str, u64)]) -> Index {
        // (id, ts, session, model, output_tokens) — build via the public path
        // so the test also covers ingest. Tests run in parallel: own dir each.
        let root = std::env::temp_dir().join(format!("token-ledger-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("p")).unwrap();
        let mut body = String::new();
        for (id, ts, sid, model, out) in recs {
            body += &format!(
                r#"{{"type":"assistant","timestamp":"{ts}","cwd":"/w/{sid}","sessionId":"{sid}","message":{{"model":"{model}","id":"msg_{id}","usage":{{"input_tokens":0,"output_tokens":{out},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
            );
            body += "\n";
        }
        std::fs::write(root.join("p/a.jsonl"), body).unwrap();
        let mut idx = Index::default();
        idx.refresh(&root, utc("2026-09-08T20:00:00Z"));
        idx
    }

    #[test]
    fn fable_window_only_counts_fable_models() {
        let idx = index_with("fable", &[
            ("1", "2026-09-08T19:00:00Z", "s1", "claude-opus-5", 1_000_000),
            ("2", "2026-09-08T19:00:00Z", "s2", "claude-fable-5-1", 1_000_000),
        ]);
        let cal = Calibration { session_reset_at: Some(utc("2026-09-08T22:00:00Z")), ..Default::default() };
        let snap = build_snapshot(&idx, &PriceTable::default(), &cal, "Max", None, utc("2026-09-08T20:00:00Z"), ScanStats::default(), &Limits::default(), None);
        let [s, w, f] = <[WindowOut; 3]>::try_from(snap.windows).ok().unwrap();
        assert_eq!(s.messages, 2);
        assert!((w.total_cost - 75.0).abs() < 1e-6, "25 + 50 dollars of output");
        assert!((f.total_cost - 50.0).abs() < 1e-6);
        assert_eq!(f.by_model.len(), 1);
        assert_eq!(f.by_model[0].key_str(), "claude-fable-5-1");
        assert!(!snap.calibrated);
        assert!(s.pct.is_none());
    }

    #[test]
    fn calibration_turns_share_into_percent_of_limit() {
        let idx = index_with("calib", &[
            ("1", "2026-09-08T19:00:00Z", "s1", "claude-opus-5", 3_000_000), // $75
            ("2", "2026-09-08T19:00:00Z", "s2", "claude-opus-5", 1_000_000), // $25
        ]);
        let now = utc("2026-09-08T20:00:00Z");
        let mut cal = Calibration { session_reset_at: Some(utc("2026-09-08T22:00:00Z")), ..Default::default() };
        // The tab said 50% while our weighted total was $100 → cap is $200.
        cal.session = Some(Anchor { pct: 50.0, captured_at: now, implied_limit: 200.0 });
        let snap = build_snapshot(&idx, &PriceTable::default(), &cal, "Max", None, now, ScanStats::default(), &Limits::default(), None);
        let s = &snap.windows[0];
        assert_eq!(s.pct, Some(50.0));
        assert_eq!(s.by_session[0].share, 75.0);
        assert_eq!(s.by_session[0].pct, Some(37.5));
        // 3h of a 5h window elapsed (17:00→22:00, now 20:00) = 60%; 50% used → 0.83× pace.
        assert!((s.pace.unwrap() - 0.83).abs() < 0.01);
    }

    impl Entry {
        fn key_str(&self) -> &str {
            match &self.key {
                EntryKey::One(s) => s,
                EntryKey::Session([s, _]) => s,
            }
        }
    }
}
