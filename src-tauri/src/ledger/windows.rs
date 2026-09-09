//! The three limit windows and how we know where they are.
//!
//! Nothing on disk says where a window starts or what its cap is. The 5-hour
//! session boundary comes from the countdown the Usage tab shows; the weekly
//! boundary is a fixed weekday and hour; the caps are *implied* from a
//! percentage the user reads off that tab once:
//!
//! ```text
//! implied_limit = weighted_total_at_capture / (pct / 100)
//! ```
//!
//! After that, every refresh estimates `pct = weighted_total_now / implied_limit`
//! with no further input. Re-entering a percentage re-anchors the limit.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    Session,
    Weekly,
    Fable,
}

impl WindowKind {
    pub const ALL: [WindowKind; 3] = [WindowKind::Session, WindowKind::Weekly, WindowKind::Fable];

    pub fn id(self) -> &'static str {
        match self {
            WindowKind::Session => "session",
            WindowKind::Weekly => "weekly",
            WindowKind::Fable => "fable",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            WindowKind::Session => "Current session",
            WindowKind::Weekly => "Weekly · all models",
            WindowKind::Fable => "Weekly · Fable",
        }
    }
    pub fn sub(self) -> &'static str {
        match self {
            WindowKind::Session => "5-hour rolling window",
            WindowKind::Weekly | WindowKind::Fable => "resets weekly",
        }
    }
    pub fn fable_only(self) -> bool {
        matches!(self, WindowKind::Fable)
    }
}

/// Local weekday + time the weekly counters reset. Read off the Usage tab
/// ("Resets Tue 1:00 AM").
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WeeklyReset {
    pub weekday: Weekday,
    pub hour: u32,
    pub minute: u32,
}

impl Default for WeeklyReset {
    fn default() -> Self {
        WeeklyReset { weekday: Weekday::Tue, hour: 1, minute: 0 }
    }
}

impl WeeklyReset {
    /// The most recent boundary at or before `now`, in local time.
    /// Derive the weekly cadence from one exact reset instant.
    pub fn from_instant(t: DateTime<Utc>) -> WeeklyReset {
        use chrono::Timelike;
        let l = t.with_timezone(&Local);
        WeeklyReset { weekday: l.weekday(), hour: l.hour(), minute: l.minute() }
    }

    pub fn last_boundary(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        let local = now.with_timezone(&Local);
        let t = NaiveTime::from_hms_opt(self.hour, self.minute, 0).unwrap_or_default();
        let mut day = local.date_naive();
        for _ in 0..8 {
            if day.weekday() == self.weekday {
                if let Some(b) = Local.from_local_datetime(&day.and_time(t)).earliest() {
                    if b <= local {
                        return b.with_timezone(&Utc);
                    }
                }
            }
            day -= Duration::days(1);
        }
        now - Duration::days(7)
    }
}

/// One calibration point per window: the official percentage, when it was
/// read, and the cap it implies in weighted dollars.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub pct: f64,
    pub captured_at: DateTime<Utc>,
    pub implied_limit: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Absolute end of the current 5-hour block, from "resets in N min".
    pub session_reset_at: Option<DateTime<Utc>>,
    /// `fetchedAtMs` of the last Claude Code usage cache we anchored to, so a
    /// stale cache is never re-applied over a newer manual entry.
    pub auto_fetched_at: Option<DateTime<Utc>>,
    /// When the user last typed values in. Manual entries newer than the
    /// cache win.
    pub manual_at: Option<DateTime<Utc>>,
    pub weekly_reset: Option<WeeklyReset>,
    pub session: Option<Anchor>,
    pub weekly: Option<Anchor>,
    pub fable: Option<Anchor>,
}

impl Calibration {
    pub fn anchor(&self, k: WindowKind) -> Option<&Anchor> {
        match k {
            WindowKind::Session => self.session.as_ref(),
            WindowKind::Weekly => self.weekly.as_ref(),
            WindowKind::Fable => self.fable.as_ref(),
        }
    }
    pub fn anchor_mut(&mut self, k: WindowKind) -> &mut Option<Anchor> {
        match k {
            WindowKind::Session => &mut self.session,
            WindowKind::Weekly => &mut self.weekly,
            WindowKind::Fable => &mut self.fable,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub kind: WindowKind,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    /// False only when nothing at all is known (no records, no reset time).
    pub boundary_known: bool,
    /// Session only: the last block has closed and nothing has started a new
    /// one. The window is the block that *would* start with the next message.
    #[serde(default)]
    pub idle: bool,
}

impl Window {
    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }

    /// Fraction of the window elapsed at `now`, floored so a just-opened
    /// window doesn't divide by zero in the pace calculation.
    pub fn elapsed_frac(&self, now: DateTime<Utc>) -> f64 {
        let total = (self.end - self.start).num_seconds().max(1) as f64;
        let done = (now - self.start).num_seconds() as f64;
        (done / total).clamp(0.02, 1.0)
    }
}

pub const SESSION_HOURS: i64 = 5;

impl Window {
    /// The window that ends at an exact, known reset time.
    pub fn ending_at(kind: WindowKind, end: DateTime<Utc>) -> Window {
        let len = match kind {
            WindowKind::Session => Duration::hours(SESSION_HOURS),
            WindowKind::Weekly | WindowKind::Fable => Duration::days(7),
        };
        Window { kind, start: end - len, end, boundary_known: true, idle: false }
    }
}

/// Resolve all three windows at `now`. `ts` is every retained record's
/// timestamp, sorted ascending; it drives the session block.
pub fn resolve(cal: &Calibration, now: DateTime<Utc>, ts: &[DateTime<Utc>]) -> [Window; 3] {
    let session = session_block(cal, now, ts);
    let wr = cal.weekly_reset.unwrap_or_default();
    let wstart = wr.last_boundary(now);
    let weekly = Window {
        kind: WindowKind::Weekly,
        start: wstart,
        end: wstart + Duration::days(7),
        boundary_known: cal.weekly_reset.is_some(),
        idle: false,
    };
    let fable = Window { kind: WindowKind::Fable, ..weekly.clone() };
    [session, weekly, fable]
}

/// The 5-hour block is not on a fixed cadence: the first message after the
/// previous block closes opens the next one. That is fully reconstructible
/// from the transcripts — verified against the server's own reset time to
/// within the request/response latency — so chain blocks forward from the
/// records. A server-reported reset that is still in the future wins as the
/// exact value; one in the past anchors where the chain resumes.
pub fn session_block(cal: &Calibration, now: DateTime<Utc>, ts: &[DateTime<Utc>]) -> Window {
    let len = Duration::hours(SESSION_HOURS);
    if let Some(end) = cal.session_reset_at {
        if now < end && now >= end - len {
            return Window { kind: WindowKind::Session, start: end - len, end, boundary_known: true, idle: false };
        }
    }
    let resume_from = cal.session_reset_at.filter(|e| now >= *e);
    let mut cur: Option<(DateTime<Utc>, DateTime<Utc>)> = None;
    for &t in ts {
        if t > now {
            break;
        }
        if let Some(r) = resume_from {
            if t < r {
                continue;
            }
        }
        match cur {
            Some((_, end)) if t < end => {}
            _ => cur = Some((t, t + len)),
        }
    }
    match cur {
        Some((start, end)) if now < end => Window { kind: WindowKind::Session, start, end, boundary_known: true, idle: false },
        Some(_) | None if !ts.is_empty() || resume_from.is_some() => {
            // Between blocks: nothing is being metered until the next message.
            Window { kind: WindowKind::Session, start: now, end: now + len, boundary_known: true, idle: true }
        }
        _ => Window { kind: WindowKind::Session, start: now - len, end: now, boundary_known: false, idle: false },
    }
}

/// Position on the green→red ramp from burn rate, not raw fill.
/// 1.0× pace = on track to hit the cap exactly as the window closes.
/// Logarithmic above pace: bursty work early in a window routinely runs
/// several times the linear target, and a linear ramp pegs everything red.
pub fn ramp(pct: f64, pace: f64) -> f64 {
    if pct >= 100.0 {
        return 1.0;
    }
    if pace <= 0.8 {
        0.0
    } else if pace <= 1.0 {
        (pace - 0.8) / 0.2 * 0.4
    } else {
        0.4 + 0.6 * (pace.log2() / 5f64.log2()).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn session_window_is_five_hours_ending_at_reset() {
        let cal = Calibration { session_reset_at: Some(utc("2026-09-08T23:22:00Z")), ..Default::default() };
        let [s, _, _] = resolve(&cal, utc("2026-09-08T21:00:00Z"), &[]);
        assert!(s.boundary_known);
        assert_eq!(s.start, utc("2026-09-08T18:22:00Z"));
        assert_eq!(s.end, utc("2026-09-08T23:22:00Z"));
    }

    #[test]
    fn blocks_chain_from_records() {
        let ts: Vec<_> = ["2026-09-08T13:20:16Z", "2026-09-08T15:00:00Z", "2026-09-08T18:20:24Z", "2026-09-08T19:00:00Z"]
            .iter().map(|s| utc(s)).collect();
        // Mid first block.
        let s = session_block(&Calibration::default(), utc("2026-09-08T16:00:00Z"), &ts);
        assert_eq!((s.start, s.end, s.idle), (utc("2026-09-08T13:20:16Z"), utc("2026-09-08T18:20:16Z"), false));
        // The message at 18:20:24 opened a new block.
        let s = session_block(&Calibration::default(), utc("2026-09-08T19:30:00Z"), &ts);
        assert_eq!((s.start, s.end, s.idle), (utc("2026-09-08T18:20:24Z"), utc("2026-09-08T23:20:24Z"), false));
        // Long after everything: idle.
        let s = session_block(&Calibration::default(), utc("2026-09-09T05:00:00Z"), &ts);
        assert!(s.idle && s.boundary_known);
        assert_eq!(s.start, utc("2026-09-09T05:00:00Z"));
    }

    #[test]
    fn expired_server_reset_anchors_the_chain() {
        // Server said the block ended 18:20:00; the 18:20:24 message must start the next one,
        // and the 15:00 message must not be mistaken for a block start.
        let cal = Calibration { session_reset_at: Some(utc("2026-09-08T18:20:00Z")), ..Default::default() };
        let ts: Vec<_> = ["2026-09-08T15:00:00Z", "2026-09-08T18:20:24Z"].iter().map(|s| utc(s)).collect();
        let s = session_block(&cal, utc("2026-09-08T20:00:00Z"), &ts);
        assert_eq!(s.start, utc("2026-09-08T18:20:24Z"));
        assert!(!s.idle);
    }

    #[test]
    fn nothing_known_is_rolling() {
        let s = session_block(&Calibration::default(), utc("2026-09-08T20:00:00Z"), &[]);
        assert!(!s.boundary_known && !s.idle);
    }

    #[test]
    fn weekly_boundary_is_most_recent_matching_weekday() {
        let wr = WeeklyReset { weekday: Weekday::Tue, hour: 1, minute: 0 };
        // Some Tuesday 14:36 local → boundary is that same Tuesday 01:00.
        let tue = Local.with_ymd_and_hms(2026, 9, 8, 14, 36, 0).unwrap();
        assert_eq!(tue.weekday(), Weekday::Tue);
        let b = wr.last_boundary(tue.with_timezone(&Utc)).with_timezone(&Local);
        assert_eq!((b.weekday(), b.hour(), b.day()), (Weekday::Tue, 1, 8));

        // Tuesday 00:30 local → boundary is the *previous* Tuesday.
        let early = Local.with_ymd_and_hms(2026, 9, 8, 0, 30, 0).unwrap();
        let b = wr.last_boundary(early.with_timezone(&Utc)).with_timezone(&Local);
        assert_eq!((b.weekday(), b.day()), (Weekday::Tue, 1));
    }

    #[test]
    fn ramp_shape() {
        assert_eq!(ramp(50.0, 0.5), 0.0);
        assert!((ramp(50.0, 1.0) - 0.4).abs() < 1e-9);
        assert!(ramp(50.0, 2.0) > 0.6 && ramp(50.0, 2.0) < 0.7);
        assert_eq!(ramp(50.0, 5.0), 1.0);
        assert_eq!(ramp(50.0, 50.0), 1.0);
        assert_eq!(ramp(100.0, 0.1), 1.0);
    }

    #[test]
    fn elapsed_fraction_is_floored() {
        let w = Window { kind: WindowKind::Session, start: utc("2026-09-08T00:00:00Z"), end: utc("2026-09-08T05:00:00Z"), boundary_known: true, idle: false };
        assert_eq!(w.elapsed_frac(utc("2026-09-08T00:00:00Z")), 0.02);
        assert!((w.elapsed_frac(utc("2026-09-08T02:30:00Z")) - 0.5).abs() < 1e-9);
        assert_eq!(w.elapsed_frac(utc("2026-09-09T00:00:00Z")), 1.0);
    }
}
