//! Aggregate the index into what the UI draws. Shape mirrors the POC's
//! `usage.json` so the same rendering code can consume either.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Local, Timelike, Utc};
use serde::{Deserialize, Serialize};

use super::activity::Activity;
use super::index::{Index, ScanStats};
use super::pricing::PriceTable;
use super::record::{Rec, TitleKind, Usage};
use super::sessions::{device_label, RemoteSession};
use super::usagecache::UsageCache;
use crate::engine::CacheDiag;
use crate::settings::{Settings, ThreadSort};
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
    /// The assistant has spoken since you last typed and since you last opened
    /// this row. Conversations only; always false for models and projects.
    #[serde(default)]
    pub unread: bool,
    /// The agent is mid-turn right now — a tool call out, or a prompt with no
    /// answer yet. Conversations only.
    #[serde(default)]
    pub working: bool,
    pub models: Vec<ModelPart>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Kinds {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

/// One bucket of the usage-over-time series. Each list is
/// `(key, weighted cost, raw tokens)`; the session key is the session id, so
/// it lines up with `by_session`'s `[session_id, cwd]` entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeriesBucket {
    pub t: DateTime<Utc>,
    pub by_session: Vec<(String, f64, u64)>,
    pub by_model: Vec<(String, f64, u64)>,
}

/// Usage over the window, bucketed. Emitted hourly; the UI rolls these up to
/// the coarser sizes it offers, which is cheap at these volumes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Series {
    pub bucket: String,
    /// Window open — the first bucket's start.
    pub start: DateTime<Utc>,
    /// Window reset. The last bucket ends at `generated_at`, not here: the
    /// empty tail is the time remaining.
    pub end: DateTime<Utc>,
    pub buckets: Vec<SeriesBucket>,
}

/// Everything below the per-bucket cut, folded into one key.
pub const REST_KEY: &str = "__rest";

/// How many keys each bucket carries before the tail is folded into `REST_KEY`.
/// The chart draws at most 9 bands and the tooltip 6 rows, so this only has to
/// stay ahead of both — and it bounds a weekly snapshot's size.
const BUCKET_KEYS: usize = 12;

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
    pub series: Series,
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
    /// How the conversation lists below are ranked. Part of the payload rather
    /// than something each window remembers for itself, so a flip in one place
    /// arrives in the other with the list it produced.
    pub thread_sort: ThreadSort,
    /// How many conversations each window shows. Carried here rather than read
    /// from settings by each page, so a change reaches both on the next
    /// snapshot instead of on the next reload.
    pub list_rows: usize,
    pub widget_rows: usize,
    /// Why the cache did or didn't calibrate us — drives the setup guidance.
    pub cache_diag: CacheDiag,
    /// Conversations on the account, from every device signed into it — this
    /// one included, flagged as such. Empty when live readings are off, which
    /// is the only way to reach the endpoint they come from.
    #[serde(default)]
    pub remote_threads: Vec<RemoteSession>,
    /// What to call this machine on rows that ran here.
    #[serde(default)]
    pub device_label: String,
    /// Why the last session-list fetch failed, if one did. The rows above are
    /// the last good ones, so this says they may be stale, not that they lie.
    #[serde(default)]
    pub remote_error: Option<String>,
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
    /// How many conversations to keep — the larger of the two window sizes.
    pub sessions: usize,
    /// What each window will actually draw, passed through to the snapshot.
    pub list_rows: usize,
    pub widget_rows: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            projects: 12,
            sessions: crate::settings::DEFAULT_LIST_ROWS,
            list_rows: crate::settings::DEFAULT_LIST_ROWS,
            widget_rows: crate::settings::DEFAULT_WIDGET_ROWS,
        }
    }
}

impl Limits {
    pub fn from_settings(s: &Settings) -> Limits {
        Limits {
            sessions: s.session_rows(),
            list_rows: s.list_rows,
            widget_rows: s.widget_rows,
            ..Limits::default()
        }
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

/// Local boundaries every `grain` minutes, deliberately not `t - t % grain_ms`:
/// an epoch-modulo floor lands day, week and month edges on UTC midnight once
/// the UI rolls these up, which shows two buckets labelled with the same day in
/// any offset timezone.
fn floor_local(t: DateTime<Utc>, grain: u32) -> DateTime<Utc> {
    let l = t.with_timezone(&Local);
    l.with_minute(l.minute() / grain * grain)
        .and_then(|x| x.with_second(0))
        .and_then(|x| x.with_nanosecond(0))
        .map_or(t, |x| x.with_timezone(&Utc))
}

/// Bucket starts for `start..now`: the window open, then every local `grain`
/// boundary after it. The last bucket runs to `now` and is normally partial.
fn grain_edges(start: DateTime<Utc>, now: DateTime<Utc>, grain: u32) -> Vec<DateTime<Utc>> {
    let mut edges = vec![start];
    let step = Duration::minutes(grain as i64);
    let mut t = floor_local(start, grain) + step;
    // A 5 h session is 20 buckets and a week 168; the cap catches a bad clock.
    while t < now && edges.len() < 1500 {
        edges.push(t);
        t += step;
    }
    edges
}

/// One bucket's map into the wire list: ranked by cost, with everything past
/// `BUCKET_KEYS` — and anything outside `keep` — folded into `REST_KEY`.
fn fold_bucket(
    m: HashMap<String, (f64, u64)>,
    keep: Option<&std::collections::HashSet<String>>,
) -> Vec<(String, f64, u64)> {
    let mut rest = (0.0f64, 0u64);
    let mut v: Vec<(String, f64, u64)> = m
        .into_iter()
        .filter_map(|(k, (c, raw))| {
            if keep.map_or(true, |s| s.contains(&k)) {
                Some((k, c, raw))
            } else {
                rest.0 += c;
                rest.1 += raw;
                None
            }
        })
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (_, c, raw) in v.drain(BUCKET_KEYS.min(v.len())..) {
        rest.0 += c;
        rest.1 += raw;
    }
    let mut out: Vec<(String, f64, u64)> =
        v.into_iter().map(|(k, c, raw)| (k, round4(c), raw)).collect();
    if rest.1 > 0 {
        out.push((REST_KEY.to_string(), round4(rest.0), rest.1));
    }
    out
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
    cache_diag: CacheDiag,
    sort: ThreadSort,
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

        let anchor = cal.anchor(w.kind);
        // Cost accrued since the reading was taken, which is the only part of
        // the dial we have to estimate when a live reading anchors us.
        let mut since_anchor = 0.0;

        // The time series, filled in the same pass: every record already
        // carries its own timestamp, so this needs no extra parsing.
        // A 5 h block wants finer buckets than a week does, and emitting the
        // week at 15 minutes would quadruple the snapshot for nothing.
        let grain: u32 = if w.kind == super::windows::WindowKind::Session { 15 } else { 60 };
        let edges = grain_edges(w.start, now.max(w.start), grain);
        type Slot = (HashMap<String, (f64, u64)>, HashMap<String, (f64, u64)>);
        let mut slots: Vec<Slot> = vec![Default::default(); edges.len()];

        for r in index.records().filter(|r| w.contains(r.ts)) {
            let Some(e) = prices.lookup(&r.model) else { continue };
            if w.kind.fable_only() && !e.fable {
                continue;
            }
            let cost = e.rates.cost(&r.usage);
            if anchor.map_or(false, |a| r.ts >= a.captured_at) {
                since_anchor += cost;
            }
            let bi = edges.partition_point(|e| *e <= r.ts).saturating_sub(1);
            if let Some(slot) = slots.get_mut(bi) {
                let raw = r.usage.raw();
                let se = slot.0.entry(r.session.clone()).or_default();
                se.0 += cost;
                se.1 += raw;
                let me = slot.1.entry(r.model.clone()).or_default();
                me.0 += cost;
                me.1 += raw;
            }
            total.push(r, cost);
            by_model.entry(r.model.clone()).or_default().push(r, cost);
            by_project.entry(r.cwd.clone()).or_default().push(r, cost);
            let se = by_session.entry(r.session.clone()).or_default();
            se.0.push(r, cost);
            *se.1.entry(r.cwd.clone()).or_default() += 1;
        }

        let limit = anchor.map(|a| a.implied_limit).filter(|l| *l > 0.0);
        // Start from the reading itself and add only what we've spent since,
        // so the dial can be wrong by at most one refresh's worth of usage.
        // Deriving the whole percentage from local transcripts instead — as
        // this used to — drifts without bound, because the real meter counts
        // every surface on the account and we only see this machine's
        // Claude Code sessions.
        let pct = match anchor {
            // An anchor from before this window started describes a window
            // that has since reset: its percentage is spent, and all we can
            // carry forward is the limit it implied.
            Some(a) if a.captured_at >= w.start => match limit {
                Some(l) => Some(a.pct + since_anchor / l * 100.0),
                // Too low to have implied a limit and none inherited: show the
                // reading, flat, rather than nothing at all.
                None => Some(a.pct),
            },
            _ => limit.map(|l| total.cost / l * 100.0),
        };
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
                unread: false,
                working: false,
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
        // Ranking decides what survives the truncation, not just the order:
        // "most recent" has to be able to surface a thread that would never
        // make the top 25 by weight.
        if sort == ThreadSort::Recent {
            sessions.sort_by(|x, y| y.last.cmp(&x.last).then(y.cost.total_cmp(&x.cost)));
        }
        projects.truncate(limits.projects);
        sessions.truncate(limits.sessions);

        // Series keys track the ranked list, so a conversation the list dropped
        // doesn't reappear as a band of its own.
        let kept: std::collections::HashSet<String> = sessions
            .iter()
            .filter_map(|e| match &e.key {
                EntryKey::Session([sid, _]) => Some(sid.clone()),
                EntryKey::One(k) => Some(k.clone()),
            })
            .collect();
        let buckets: Vec<SeriesBucket> = slots
            .into_iter()
            .enumerate()
            .map(|(i, (sess, model))| SeriesBucket {
                t: edges[i],
                by_session: fold_bucket(sess, Some(&kept)),
                by_model: fold_bucket(model, None),
            })
            .collect();

        outs.push(WindowOut {
            id: w.kind.id().to_string(),
            label: w.kind.label().to_string(),
            sub: w.kind.sub().to_string(),
            start: w.start,
            reset: w.end,
            boundary_known: w.boundary_known,
            idle: w.idle,
            calibrated: pct.is_some(),
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
            series: Series {
                bucket: if grain == 15 { "15m".into() } else { "hour".to_string() },
                start: w.start,
                end: w.end,
                buckets,
            },
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
        cache_diag,
        // The engine fills these: they come off the network, not the index.
        remote_threads: Vec::new(),
        device_label: device_label(),
        remote_error: None,
        thread_sort: sort,
        list_rows: limits.list_rows,
        widget_rows: limits.widget_rows,
        calibration_source: match (cal.manual_at, cal.auto_fetched_at) {
            (Some(m), Some(a)) if m > a => "manual",
            (_, Some(_)) => "auto",
            (Some(_), None) => "manual",
            (None, None) => "none",
        }
        .to_string(),
    }
}

/// Fill in each conversation's unread and working flags, in place.
///
/// A separate pass, and deliberately after the ranking: it reads transcripts
/// off disk, and the ranked lists are what bound how many. Nothing here can
/// change which rows are shown, only what they say about themselves.
pub fn apply_activity(
    snap: &mut Snapshot,
    index: &Index,
    activity: &mut Activity,
    settings: &Settings,
    now: DateTime<Utc>,
) {
    let mut seen: HashMap<String, (bool, bool)> = HashMap::new();
    let mut used: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for w in &mut snap.windows {
        for e in &mut w.by_session {
            let EntryKey::Session([sid, _]) = &e.key else { continue };
            // The same conversation appears in all three windows; tail it once.
            let (unread, working) = *seen.entry(sid.clone()).or_insert_with(|| {
                match index.session_file(sid) {
                    Some(path) => {
                        used.insert(path.to_path_buf());
                        let t = activity.tail(path);
                        (t.unread(settings.seen_at(sid)), t.working(now))
                    }
                    None => (false, false),
                }
            });
            e.unread = unread;
            e.working = working;
        }
    }
    // Only the ranked lists are ever tailed, so this is what bounds the cache.
    activity.retain(&used);
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Bucket costs are small; two decimals would round most of them to zero.
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::windows::Anchor;
    use crate::settings::ThreadSort;

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
        let snap = build_snapshot(&idx, &PriceTable::default(), &cal, "Max", None, utc("2026-09-08T20:00:00Z"), ScanStats::default(), &Limits::default(), None, CacheDiag::default(), ThreadSort::Usage);
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
        let snap = build_snapshot(&idx, &PriceTable::default(), &cal, "Max", None, now, ScanStats::default(), &Limits::default(), None, CacheDiag::default(), ThreadSort::Usage);
        let s = &snap.windows[0];
        assert_eq!(s.pct, Some(50.0));
        assert_eq!(s.by_session[0].share, 75.0);
        assert_eq!(s.by_session[0].pct, Some(37.5));
        // 3h of a 5h window elapsed (17:00→22:00, now 20:00) = 60%; 50% used → 0.83× pace.
        assert!((s.pace.unwrap() - 0.83).abs() < 0.01);
    }

    #[test]
    fn series_buckets_on_local_boundaries_and_sum_to_the_window_total() {
        let idx = index_with("series", &[
            ("1", "2026-09-08T17:30:00Z", "s1", "claude-opus-5", 1_000_000),
            ("2", "2026-09-08T18:10:00Z", "s1", "claude-opus-5", 1_000_000),
            ("3", "2026-09-08T19:40:00Z", "s2", "claude-sonnet-5", 1_000_000),
        ]);
        let now = utc("2026-09-08T20:00:00Z");
        let cal = Calibration { session_reset_at: Some(utc("2026-09-08T22:00:00Z")), ..Default::default() };
        let snap = build_snapshot(&idx, &PriceTable::default(), &cal, "Max", None, now, ScanStats::default(), &Limits::default(), None, CacheDiag::default(), ThreadSort::Usage);
        let s = &snap.windows[0];
        // 17:00 open, twelve quarter-hours behind it.
        assert_eq!(s.series.bucket, "15m");
        assert_eq!(s.series.start, s.start);
        assert_eq!(s.series.end, s.reset);
        assert_eq!(s.series.buckets.len(), 12);
        assert_eq!(s.series.buckets[0].t, s.start);
        for b in &s.series.buckets {
            let l = b.t.with_timezone(&Local);
            assert!(b.t == s.start || (l.minute() % 15 == 0 && l.second() == 0), "edge {} is off the quarter hour", b.t);
        }
        // The weekly window stays hourly, so a week is 168 buckets, not 672.
        assert_eq!(snap.windows[1].series.bucket, "hour");
        let summed: f64 = s.series.buckets.iter().flat_map(|b| &b.by_model).map(|(_, c, _)| c).sum();
        assert!((summed - s.total_cost).abs() < 0.01, "{summed} vs {}", s.total_cost);
        let raw: u64 = s.series.buckets.iter().flat_map(|b| &b.by_session).map(|(_, _, r)| r).sum();
        assert_eq!(raw, s.total_raw);
        // Both keyings cover the same money.
        let by_sess: f64 = s.series.buckets.iter().flat_map(|b| &b.by_session).map(|(_, c, _)| c).sum();
        assert!((by_sess - summed).abs() < 0.01);
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
