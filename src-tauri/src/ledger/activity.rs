//! Is a conversation waiting to be read, and is its agent still working?
//!
//! Neither question is answerable from the usage index: dedup drops the copies
//! of a turn that forks and resumes leave behind, and what matters here is the
//! *tail* of the file a session is currently writing to, not the set of unique
//! billable turns. So this reads the last few kilobytes of the handful of
//! transcripts the ledger is actually about to show — bounded by the ranked
//! lists, never by the whole history.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};

use super::record::peek_str;

/// How long after the last line of an unfinished turn we still call the agent
/// working. A session killed mid-tool-call leaves a transcript that looks
/// exactly like one whose build is still running; only the clock separates
/// them. Generous enough to cover a long tool call, short enough that an
/// abandoned session stops spinning within a few refreshes.
pub const WORKING_STALE: i64 = 300;

/// First read of a transcript's tail. Most turns are far smaller than this;
/// the fallback below covers the ones that aren't.
const TAIL_BYTES: u64 = 64 * 1024;
/// Second try, when the first window landed inside one enormous tool result.
const TAIL_BYTES_MAX: u64 = 512 * 1024;

/// What the tail of one transcript says about its conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tail {
    /// Last message the user actually typed — not a tool result, not a
    /// system-injected block.
    pub last_user: Option<DateTime<Utc>>,
    /// Last thing the assistant said in the main thread. Sub-agent turns are
    /// work, not a reply, so they don't count here.
    pub last_assistant: Option<DateTime<Utc>>,
    /// Newest timestamp of any kind, including sub-agent traffic — how we tell
    /// a running turn from an abandoned one.
    pub last_any: Option<DateTime<Utc>>,
    /// The last thing that happened leaves the turn unfinished: a tool call
    /// awaiting its result, a tool result awaiting the next assistant message,
    /// or a user prompt with no answer yet.
    pub in_flight: bool,
    /// The tail carried none of the three, so nothing above is trustworthy.
    pub decided: bool,
}

impl Tail {
    /// Is the agent working right now? An unfinished turn that stopped moving
    /// is a session someone walked away from, not one still running.
    pub fn working(&self, now: DateTime<Utc>) -> bool {
        self.decided
            && self.in_flight
            && self.last_any.is_some_and(|t| now - t < Duration::seconds(WORKING_STALE))
    }

    /// Has the assistant finished saying something since the user last spoke,
    /// and since the user last looked? `seen` is the later of "you opened this
    /// row" and the baseline stamped when unread tracking began, so a first
    /// run is quiet.
    ///
    /// An unfinished turn is never unread: the last thing the assistant did
    /// was call a tool, which is not a reply to catch up on — whether it is
    /// still running or was abandoned there.
    pub fn unread(&self, seen: Option<DateTime<Utc>>) -> bool {
        if self.in_flight {
            return false;
        }
        let Some(a) = self.last_assistant else { return false };
        if self.last_user.is_some_and(|u| u >= a) {
            return false;
        }
        seen.map_or(true, |s| a > s)
    }
}

/// What a transcript looked like when we last read it. A file that hasn't been
/// appended to since can't have a different tail, so the bytes are read once.
#[derive(Clone, Copy, PartialEq)]
struct Stamp {
    size: u64,
    mtime_ms: i64,
}

/// Tails, remembered across refreshes and keyed by the file they came from.
#[derive(Default)]
pub struct Activity {
    cache: HashMap<PathBuf, (Stamp, Tail)>,
}

impl Activity {
    /// The tail of `path`, from cache when the file hasn't moved.
    pub fn tail(&mut self, path: &Path) -> Tail {
        let Some(stamp) = stamp(path) else { return Tail::default() };
        if let Some((s, t)) = self.cache.get(path) {
            if *s == stamp {
                return *t;
            }
        }
        let t = read_tail(path, stamp.size);
        self.cache.insert(path.to_path_buf(), (stamp, t));
        t
    }

    /// Drop everything not in `live`, so the map can't outgrow the ledger.
    pub fn retain(&mut self, live: &std::collections::HashSet<PathBuf>) {
        self.cache.retain(|p, _| live.contains(p));
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(Stamp { size: meta.len(), mtime_ms })
}

fn read_tail(path: &Path, size: u64) -> Tail {
    let t = parse_tail(&read_last(path, TAIL_BYTES.min(size)));
    if t.decided || size <= TAIL_BYTES {
        return t;
    }
    // The window landed inside a single huge record; widen it once.
    parse_tail(&read_last(path, TAIL_BYTES_MAX.min(size)))
}

/// The last `n` bytes of the file, with the leading partial line dropped.
fn read_last(path: &Path, n: u64) -> String {
    let Ok(mut fh) = File::open(path) else { return String::new() };
    let Ok(size) = fh.metadata().map(|m| m.len()) else { return String::new() };
    let from = size.saturating_sub(n);
    if fh.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity(n as usize);
    if fh.take(n).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if from > 0 {
        match text.find('\n') {
            Some(i) => text = text[i + 1..].to_string(),
            None => text.clear(),
        }
    }
    text
}

/// Walk the tail forwards, keeping the last of each thing we care about. Every
/// test is a substring check on raw JSON: these lines are mostly tool output,
/// and parsing them would cost more than the whole scan does.
fn parse_tail(text: &str) -> Tail {
    let mut t = Tail::default();
    for line in text.lines() {
        let assistant = line.contains("\"type\":\"assistant\"");
        let user = !assistant && line.contains("\"type\":\"user\"");
        if !assistant && !user {
            continue;
        }
        let ts = peek_str(line, "timestamp").and_then(parse_ts);
        if let Some(ts) = ts {
            t.last_any = Some(t.last_any.map_or(ts, |p| p.max(ts)));
        }
        let sidechain = line.contains("\"isSidechain\":true");
        if assistant {
            if !sidechain {
                if let Some(ts) = ts {
                    t.last_assistant = Some(ts);
                }
            }
            // Anything but a tool call ends the turn: end_turn, max_tokens,
            // a refusal. Only a tool call leaves the agent mid-flight.
            t.in_flight = line.contains("\"stop_reason\":\"tool_use\"");
            t.decided = true;
            continue;
        }
        // User lines are mostly the results of the agent's own tool calls;
        // those say the turn is still running, not that anyone typed.
        if line.contains("\"toolUseResult\"") || line.contains("\"type\":\"tool_result\"") {
            t.in_flight = true;
            t.decided = true;
            continue;
        }
        if line.contains("\"isMeta\":true") {
            continue;
        }
        if !sidechain {
            if let Some(ts) = ts {
                t.last_user = Some(ts);
            }
        }
        t.in_flight = true;
        t.decided = true;
    }
    t
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn assistant(ts: &str, stop: &str) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":false,"timestamp":"{ts}","sessionId":"s1","message":{{"model":"claude-opus-5","stop_reason":"{stop}","content":[]}}}}"#
        ) + "\n"
    }

    fn prompt(ts: &str) -> String {
        format!(
            r#"{{"type":"user","isSidechain":false,"timestamp":"{ts}","sessionId":"s1","message":{{"role":"user","content":"do the thing"}}}}"#
        ) + "\n"
    }

    fn tool_result(ts: &str) -> String {
        format!(
            r#"{{"type":"user","isSidechain":false,"timestamp":"{ts}","sessionId":"s1","toolUseResult":{{"ok":true}},"message":{{"role":"user","content":[{{"type":"tool_result","content":"x"}}]}}}}"#
        ) + "\n"
    }

    const NOW: &str = "2026-09-08T20:00:00Z";

    #[test]
    fn a_finished_turn_is_unread_and_not_working() {
        let t = parse_tail(&(prompt("2026-09-08T19:58:00Z") + &assistant("2026-09-08T19:59:00Z", "end_turn")));
        assert!(!t.working(utc(NOW)));
        assert!(t.unread(None));
        // Reading it clears it; so does the user's next message.
        assert!(!t.unread(Some(utc("2026-09-08T19:59:30Z"))));
        let after = parse_tail(&(assistant("2026-09-08T19:59:00Z", "end_turn") + &prompt("2026-09-08T19:59:40Z")));
        assert!(!after.unread(None));
    }

    #[test]
    fn a_turn_mid_tool_call_is_working() {
        let t = parse_tail(&(assistant("2026-09-08T19:59:00Z", "tool_use") + &tool_result("2026-09-08T19:59:20Z")));
        assert!(t.working(utc(NOW)));
        assert!(!t.unread(None), "the agent is still talking; nothing to catch up on yet");
    }

    #[test]
    fn an_abandoned_turn_stops_spinning() {
        let t = parse_tail(&assistant("2026-09-08T18:00:00Z", "tool_use"));
        assert!(t.in_flight);
        assert!(!t.working(utc(NOW)), "two hours is not a long tool call");
    }

    #[test]
    fn sidechain_turns_are_work_but_not_a_reply() {
        let side = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-08T19:59:50Z","message":{"stop_reason":"tool_use"}}"#.to_string() + "\n";
        let t = parse_tail(&(prompt("2026-09-08T19:58:00Z") + &side));
        assert!(t.working(utc(NOW)));
        assert_eq!(t.last_assistant, None);
        assert_eq!(t.last_any, Some(utc("2026-09-08T19:59:50Z")));
    }

    #[test]
    fn a_tail_of_pure_tool_output_decides_nothing() {
        let t = parse_tail("{\"type\":\"attachment\",\"timestamp\":\"2026-09-08T19:59:00Z\"}\n");
        assert!(!t.decided);
        assert!(!t.working(utc(NOW)));
        assert!(!t.unread(None));
    }

    #[test]
    fn reading_the_tail_drops_the_partial_first_line() {
        let dir = std::env::temp_dir().join(format!("token-ledger-act-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jsonl");
        let body = prompt("2026-09-08T19:58:00Z") + &assistant("2026-09-08T19:59:00Z", "end_turn");
        fs::write(&p, &body).unwrap();

        let mut act = Activity::default();
        let full = act.tail(&p);
        assert_eq!(full.last_assistant, Some(utc("2026-09-08T19:59:00Z")));

        // A window that starts mid-line must not parse the fragment.
        let cut = (body.len() - 40) as u64;
        let t = parse_tail(&read_last(&p, cut));
        assert_eq!(t.last_user, None, "the truncated user line is not a message");
    }
}
