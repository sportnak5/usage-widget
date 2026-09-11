//! The account's session list — every device, not just this one.
//!
//! Claude Code mirrors a remote-control session to the account, and the same
//! credential the Usage endpoint takes will read that list back. This is the
//! only way the ledger can know a conversation on another machine has
//! finished, because nothing on this disk ever mentions it.
//!
//! What the list will not tell us is *which* machine. No field in the row, the
//! detail response, or the environments endpoint names one — the desktop app
//! learns device names over a relay websocket that has no REST mirror
//! (`docs/sessions-api.md` records every lead that was tried and closed). So
//! the honest classification is "this device" — the local transcript recorded
//! the same bridge id — against "not seen on this device", and the local label
//! is this machine's hostname.

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::record::Usage;
use super::usageapi::{self, clean, redact, FetchError};

pub const ENDPOINT: &str = "https://api.anthropic.com/v1/code/sessions";

/// Both headers are load-bearing: without the version the endpoint answers
/// 400, without the token 401. The Usage endpoint's `anthropic-beta` is
/// unrelated and not wanted here.
const API_VERSION: &str = "2023-06-01";
const UA: &str = concat!("token-ledger/", env!("CARGO_PKG_VERSION"));

/// The largest page the endpoint serves.
const PAGE: usize = 100;

/// One session's event stream, per request. The endpoint accepts larger, but
/// the walk below is bounded by pages, so a bigger page is a bigger blast
/// radius on a retry for no gain.
const EVENTS_PAGE: usize = 200;

/// How far back through one session's stream a single pass will walk. A cold
/// session of 2,400 events costs twelve requests once and nothing thereafter,
/// because the cursor is kept.
const MAX_EVENT_PAGES: usize = 12;

/// A ceiling on the walk, not an expectation: an account with thousands of
/// sessions must not turn one refresh into thirty requests against an endpoint
/// whose neighbour rate-limits readily.
const MAX_PAGES: usize = 5;

/// One conversation on the account, as the list reports it.
///
/// Field names match the local `Entry` where they mean the same thing, so a
/// row renders the same way whichever list it came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoteSession {
    /// `cse_…` — the account-wide id, and the join key against transcripts.
    pub id: String,
    pub title: Option<String>,
    /// `last_event_at`: the freshest thing that happened in it, anywhere.
    pub last: DateTime<Utc>,
    /// The agent is mid-turn — `worker_status: running`.
    pub working: bool,
    /// Stopped and waiting on the user — `worker_status: requires_action`.
    pub requires_action: bool,
    /// The user closed it out — `status: archived`.
    pub archived: bool,
    /// Anthropic's own unread flag, which is not the ledger's: it counts
    /// against whatever surface last opened the conversation.
    pub unread: bool,
    /// `owner/repo` from the reported worktree state, when there is one. Not a
    /// device, but it is the only hint at *where* a remote session is running.
    pub repo: Option<String>,
    /// A local transcript recorded this bridge id, so it ran on this machine.
    pub this_device: bool,
    /// The local session id it ran as, when `this_device`.
    pub local_session: Option<String>,
    /// Tokens the session's own event stream reports, per model, once it has
    /// been walked. Empty means not walked yet, which is not the same as zero:
    /// a row with no entry here shows dashes, not a total of nothing.
    #[serde(default)]
    pub models: Vec<RemoteModel>,
    /// Weighted dollars across `models`. A **floor**: the event stream's output
    /// token counts are a streaming placeholder, so the real figure is higher
    /// (`docs/sessions-api.md` §2b).
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub raw: u64,
}

/// What one model spent inside a remote conversation. Mirrors the local
/// `ModelPart` closely enough that a row renders the same way, without
/// reaching into `snapshot`, which reaches back here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoteModel {
    pub model: String,
    pub cost: f64,
    pub raw: u64,
}

/// Fetch the whole list with Claude Code's credential.
///
/// One attempt on the cached credential, deliberately: the Usage fetch runs
/// first each refresh and already owns the forget-and-retry dance. Repeating
/// it here would raise the Keychain dialog twice per refresh on a login that
/// has genuinely gone bad.
pub fn fetch() -> Result<Vec<RemoteSession>, FetchError> {
    let (tok, _) = usageapi::claude_token()?;
    fetch_with(&tok)
}

/// Fetch with an explicit token, so the whole path can be exercised without
/// touching the credential store.
pub fn fetch_with(tok: &str) -> Result<Vec<RemoteSession>, FetchError> {
    let tok = clean(tok);
    if tok.is_empty() || !tok.chars().all(|c| c.is_ascii_graphic()) {
        return Err(FetchError::Malformed);
    }
    let mut out: Vec<RemoteSession> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let body = get(&tok, cursor.as_deref())?;
        let (rows, next) = parse_page(&body).ok_or(FetchError::Shape)?;
        out.extend(rows);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    // Newest first, so a truncated list keeps what is still moving.
    out.sort_by(|a, b| b.last.cmp(&a.last));
    Ok(out)
}

/// Rows, plus the next cursor if there is another page.
///
/// `next_cursor` is *absent* on the last page rather than null, so testing for
/// a missing key is what stops the walk.
fn parse_page(body: &Value) -> Option<(Vec<RemoteSession>, Option<String>)> {
    let data = body.get("data")?.as_array()?;
    let rows = data.iter().filter_map(parse_row).collect();
    let next = body.get("next_cursor").and_then(Value::as_str).filter(|c| !c.is_empty());
    Some((rows, next.map(str::to_string)))
}

fn parse_row(v: &Value) -> Option<RemoteSession> {
    let id = v.get("id")?.as_str()?.to_string();
    let last = v
        .get("last_event_at")
        .or_else(|| v.get("created_at"))
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))?;
    let worker = v.get("worker_status").and_then(Value::as_str).unwrap_or("");
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .map(super::record::clean_title)
        .filter(|t| !t.is_empty());
    Some(RemoteSession {
        id,
        title,
        last,
        // `worker_status` is the only field that moves with the work.
        // `connection_status` tracks the client socket and still reads
        // "connected" on a session that finished hours ago.
        working: worker == "running",
        requires_action: worker == "requires_action",
        archived: v.get("status").and_then(Value::as_str) == Some("archived"),
        unread: v.get("unread").and_then(Value::as_bool).unwrap_or(false),
        repo: repo_of(v),
        this_device: false,
        local_session: None,
        models: Vec::new(),
        cost: 0.0,
        raw: 0,
    })
}

/// The repo the session last reported working in, e.g. `answerrocket/CIQ`.
/// Present on about half the rows.
fn repo_of(v: &Value) -> Option<String> {
    let meta = v.get("external_metadata")?;
    for key in ["worktree_state", "current_branches"] {
        if let Some(o) = meta.get(key).and_then(Value::as_object) {
            if let Some(k) = o.keys().next() {
                return Some(k.clone());
            }
        }
    }
    None
}

/// What one session's event stream has told us so far.
///
/// Keyed by assistant message id because the stream is *not* a set of distinct
/// messages: each one is written 2-3 times as it streams, and the copies carry
/// different partial usage. Summing the events double-counts; the map is the
/// de-duplication, and `merge` keeps the largest value seen per field, since a
/// later copy of a message has seen more of it than an earlier one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionEvents {
    msgs: HashMap<String, (String, Usage)>,
    /// `sequence_num` of the last event read. The endpoint's cursor *is* that
    /// number, so the next pass asks only for what arrived since — which is
    /// what makes keeping this per session cheaper than re-walking.
    pub cursor: Option<String>,
    /// `last_event_at` of the row when we last walked it. Re-walking a session
    /// that has not moved would spend a request to learn nothing.
    pub synced_last: Option<DateTime<Utc>>,
}

impl SessionEvents {
    /// Tokens per model, ordered by model so a snapshot does not reshuffle.
    pub fn by_model(&self) -> Vec<(String, Usage)> {
        let mut out: HashMap<&str, Usage> = HashMap::new();
        for (model, u) in self.msgs.values() {
            out.entry(model.as_str()).or_default().add(u);
        }
        let mut v: Vec<(String, Usage)> = out.into_iter().map(|(m, u)| (m.to_string(), u)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    pub fn usage(&self) -> Usage {
        let mut u = Usage::default();
        for (_, m) in self.msgs.values() {
            u.add(m);
        }
        u
    }

    pub fn messages(&self) -> usize {
        self.msgs.len()
    }

    fn merge(&mut self, id: String, model: String, u: Usage) {
        let slot = self.msgs.entry(id).or_insert_with(|| (model.clone(), Usage::default()));
        slot.0 = model;
        let c = &mut slot.1;
        c.input = c.input.max(u.input);
        c.output = c.output.max(u.output);
        c.cache_5m = c.cache_5m.max(u.cache_5m);
        c.cache_1h = c.cache_1h.max(u.cache_1h);
        c.cache_read = c.cache_read.max(u.cache_read);
    }
}

/// Walk one session's event stream forward from where the last pass stopped.
///
/// Ascending order is not a preference: the cursor is a high-water mark, so
/// only a forward walk can be resumed. The output token counts that come back
/// are a streaming placeholder and not the real figure — see
/// `docs/sessions-api.md` §2b — so what this yields is a floor on the cost,
/// which is why the UI marks it as one.
///
/// Returns whether the stream was read to its end. A long conversation can
/// exhaust the page budget first, and the caller must not record it as caught
/// up on the strength of a partial read — the cursor is kept either way, so the
/// next pass picks the walk up rather than starting it again.
pub fn fetch_events(tok: &str, id: &str, st: &mut SessionEvents) -> Result<bool, FetchError> {
    let tok = clean(tok);
    if tok.is_empty() || !tok.chars().all(|c| c.is_ascii_graphic()) {
        return Err(FetchError::Malformed);
    }
    if !id.starts_with("cse_") || !id[4..].chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(FetchError::Malformed);
    }
    for _ in 0..MAX_EVENT_PAGES {
        let body = get_events(&tok, id, st.cursor.as_deref())?;
        match absorb_events_page(&body, st).ok_or(FetchError::Shape)? {
            Some(c) => st.cursor = Some(c),
            None => return Ok(true),
        }
    }
    Ok(false)
}

/// Fold one page into the state, returning the cursor to ask from next, or
/// `None` when the page was the last.
fn absorb_events_page(body: &Value, st: &mut SessionEvents) -> Option<Option<String>> {
    let data = body.get("data")?.as_array()?;
    for e in data {
        if let Some((id, model, u)) = event_usage(e) {
            st.merge(id, model, u);
        }
    }
    // `next_cursor` is absent on the last page, as on the session list. The
    // high-water mark still has to advance past what we just read, or a
    // resumed walk would re-read it — so fall back to the last sequence number.
    if let Some(c) = body.get("next_cursor").and_then(cursor_str) {
        return Some(Some(c));
    }
    if let Some(last) = data.last().and_then(|e| e.get("sequence_num")).and_then(cursor_str) {
        st.cursor = Some(last);
    }
    Some(None)
}

/// The cursor is a sequence number, which the endpoint sends as a string but
/// is not obliged to.
fn cursor_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The tokens an `assistant` event reports, with the model that spent them.
/// Every other event type carries no usage at all — `result` has a `usage`
/// block but it is zeroed, so reading it would only add noise.
fn event_usage(e: &Value) -> Option<(String, String, Usage)> {
    if e.get("event_type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let msg = e.get("payload")?.get("message")?;
    let id = msg.get("id").and_then(Value::as_str)?.to_string();
    let model = msg.get("model").and_then(Value::as_str)?.to_string();
    let usage = msg.get("usage")?;
    let total_write = u64_at(usage, "cache_creation_input_tokens");
    let (mut c5, c1) = match usage.get("cache_creation") {
        Some(cc) => (u64_at(cc, "ephemeral_5m_input_tokens"), u64_at(cc, "ephemeral_1h_input_tokens")),
        None => (0, 0),
    };
    if c5 + c1 == 0 {
        c5 = total_write;
    }
    Some((
        id,
        model,
        Usage {
            input: u64_at(usage, "input_tokens"),
            output: u64_at(usage, "output_tokens"),
            cache_5m: c5,
            cache_1h: c1,
            cache_read: u64_at(usage, "cache_read_input_tokens"),
        },
    ))
}

fn u64_at(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// Mark the rows a local transcript can vouch for. `bridge` maps an account
/// session id to the local session that wrote it — see `Index::bridge_sessions`.
pub fn classify(rows: &mut [RemoteSession], bridge: &HashMap<&str, &str>) {
    for r in rows.iter_mut() {
        match bridge.get(r.id.as_str()) {
            Some(sid) => {
                r.this_device = true;
                r.local_session = Some((*sid).to_string());
            }
            None => {
                r.this_device = false;
                r.local_session = None;
            }
        }
    }
}

/// What to call this machine on its own rows. The hostname, lowercased and cut
/// at the first dot — `Crawfords-M5-MacBook-Pro.local` is the same machine as
/// the `crawfords-m5-macbook-pro` slug Anthropic builds default titles from.
///
/// Shelling out rather than taking a crate for one string: `hostname` is
/// present on macOS, Linux and Windows alike.
pub fn device_label() -> String {
    static LABEL: OnceLock<String> = OnceLock::new();
    LABEL.get_or_init(|| read_hostname().unwrap_or_else(|| "this device".to_string())).clone()
}

fn read_hostname() -> Option<String> {
    let out = std::process::Command::new("hostname").output().ok()?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let name = raw.trim().split('.').next().unwrap_or("").to_ascii_lowercase();
    (!name.is_empty()).then_some(name)
}

fn get(tok: &str, cursor: Option<&str>) -> Result<Value, FetchError> {
    let mut req = ureq::get(ENDPOINT)
        .query("limit", &PAGE.to_string())
        .set("Authorization", &format!("Bearer {tok}"))
        .set("anthropic-version", API_VERSION)
        .set("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(c) = cursor {
        req = req.query("cursor", c);
    }
    match req.call() {
        Ok(r) => r.into_json::<Value>().map_err(|_| FetchError::Shape),
        Err(ureq::Error::Status(401 | 403, _)) => Err(FetchError::Unauthorized),
        Err(ureq::Error::Status(c, _)) => Err(FetchError::Http(c)),
        Err(ureq::Error::Transport(t)) => Err(FetchError::Transport(redact(&t.to_string(), tok))),
    }
}

fn get_events(tok: &str, id: &str, cursor: Option<&str>) -> Result<Value, FetchError> {
    let mut req = ureq::get(&format!("{ENDPOINT}/{id}/events"))
        .query("limit", &EVENTS_PAGE.to_string())
        // Forward, so the cursor kept between passes means "everything after".
        .query("sort_order", "asc")
        .set("Authorization", &format!("Bearer {tok}"))
        .set("anthropic-version", API_VERSION)
        .set("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(15));
    if let Some(c) = cursor {
        req = req.query("cursor", c);
    }
    match req.call() {
        Ok(r) => r.into_json::<Value>().map_err(|_| FetchError::Shape),
        Err(ureq::Error::Status(401 | 403, _)) => Err(FetchError::Unauthorized),
        Err(ureq::Error::Status(c, _)) => Err(FetchError::Http(c)),
        Err(ureq::Error::Transport(t)) => Err(FetchError::Transport(redact(&t.to_string(), tok))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page trimmed from the live response, keeping every field this reads.
    const PAGE_BODY: &str = r#"{
      "data": [
        {"id":"cse_aaa","title":"Custom pages exploration","status":"active","status_bucket":"working",
         "worker_status":"running","connection_status":"connected","unread":true,
         "last_event_at":"2026-09-10T23:10:25.753657Z","created_at":"2026-09-10T20:36:20.114918Z",
         "user_message_count":"0",
         "external_metadata":{"worktree_state":{"answerrocket/CIQ":{"branch":"develop"}}}},
        {"id":"cse_bbb","title":"Ledger updates pull and app sync","status":"archived",
         "status_bucket":"completed","worker_status":"idle","connection_status":"connected",
         "unread":false,"last_event_at":"2026-09-09T10:00:00Z","user_message_count":"12"},
        {"id":"cse_ccc","title":"Waiting on me","status":"active","status_bucket":"blocked",
         "worker_status":"requires_action","unread":true,"last_event_at":"2026-09-10T09:00:00Z"}
      ],
      "next_cursor": "cse_ccc",
      "resume_token": "tok"
    }"#;

    fn page() -> Value {
        serde_json::from_str(PAGE_BODY).unwrap()
    }

    #[test]
    fn reads_a_real_page() {
        let (rows, next) = parse_page(&page()).unwrap();
        assert_eq!(next.as_deref(), Some("cse_ccc"));
        assert_eq!(rows.len(), 3);

        let a = &rows[0];
        assert_eq!(a.id, "cse_aaa");
        assert_eq!(a.title.as_deref(), Some("Custom pages exploration"));
        assert_eq!(a.last, DateTime::parse_from_rfc3339("2026-09-10T23:10:25.753657Z").unwrap());
        assert!(a.working && !a.requires_action && !a.archived);
        assert!(a.unread);
        assert_eq!(a.repo.as_deref(), Some("answerrocket/CIQ"));

        // Finished: idle worker, even though the socket still reads connected.
        assert!(!rows[1].working);
        assert!(rows[1].archived);
        // Stopped for the user.
        assert!(rows[2].requires_action && !rows[2].working);
    }

    #[test]
    fn a_last_page_has_no_cursor_at_all() {
        let mut v = page();
        v.as_object_mut().unwrap().remove("next_cursor");
        assert_eq!(parse_page(&v).unwrap().1, None, "absent, not null, is what ends the walk");

        // And a null one, should the endpoint ever start sending one.
        let mut v = page();
        v["next_cursor"] = Value::Null;
        assert_eq!(parse_page(&v).unwrap().1, None);
    }

    #[test]
    fn a_body_without_data_is_a_shape_error_not_an_empty_list() {
        let v = serde_json::json!({"error": {"type": "not_found"}});
        assert!(parse_page(&v).is_none());
    }

    #[test]
    fn a_row_without_a_timestamp_is_dropped_not_defaulted() {
        let v = serde_json::json!({"data": [{"id": "cse_x", "worker_status": "idle"}]});
        assert!(parse_page(&v).unwrap().0.is_empty(), "a row with no time can't be placed in a list");
        // created_at stands in when the session has had no events yet.
        let v = serde_json::json!({"data": [{"id": "cse_x", "created_at": "2026-09-10T09:00:00Z"}]});
        assert_eq!(parse_page(&v).unwrap().0.len(), 1);
    }

    #[test]
    fn transcripts_decide_which_rows_ran_here() {
        let (mut rows, _) = parse_page(&page()).unwrap();
        let bridge = HashMap::from([("cse_bbb", "sess-local-1")]);
        classify(&mut rows, &bridge);
        assert!(!rows[0].this_device);
        assert!(rows[1].this_device);
        assert_eq!(rows[1].local_session.as_deref(), Some("sess-local-1"));
        assert_eq!(rows[0].local_session, None);
    }

    #[test]
    fn an_unusable_token_never_reaches_the_wire() {
        assert_eq!(fetch_with("   "), Err(FetchError::Malformed));
        assert_eq!(fetch_with("sk-ant-oat01-\u{201c}aaa"), Err(FetchError::Malformed));
    }

    /// Two pages of one session's stream, trimmed from the live response. The
    /// same message id appears three times with growing usage — which is how
    /// the endpoint really writes it, and why summing events is wrong.
    const EVENTS_P1: &str = r#"{
      "data": [
        {"event_type":"user","sequence_num":"1","payload":{"type":"user"}},
        {"event_type":"assistant","sequence_num":"2","payload":{"message":{"id":"msg_a",
          "model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":1,
          "cache_creation_input_tokens":1214,"cache_read_input_tokens":106580,
          "cache_creation":{"ephemeral_1h_input_tokens":1214,"ephemeral_5m_input_tokens":0}}}}},
        {"event_type":"assistant","sequence_num":"3","payload":{"message":{"id":"msg_a",
          "model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":3,
          "cache_creation_input_tokens":1214,"cache_read_input_tokens":106580,
          "cache_creation":{"ephemeral_1h_input_tokens":1214,"ephemeral_5m_input_tokens":0}}}}},
        {"event_type":"rate_limit_event","sequence_num":"4","payload":{"rate_limit_info":{}}}
      ],
      "next_cursor": "4",
      "resume_cursor": "4"
    }"#;

    const EVENTS_P2: &str = r#"{
      "data": [
        {"event_type":"assistant","sequence_num":"5","payload":{"message":{"id":"msg_a",
          "model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":2,
          "cache_creation_input_tokens":1214,"cache_read_input_tokens":106580}}}},
        {"event_type":"assistant","sequence_num":"6","payload":{"message":{"id":"msg_b",
          "model":"claude-fable-5-1","usage":{"input_tokens":10,"output_tokens":5,
          "cache_creation_input_tokens":40,"cache_read_input_tokens":900,
          "cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":40}}}}},
        {"event_type":"result","sequence_num":"7","payload":{"usage":{"input_tokens":0},
          "total_cost_usd":0}}
      ],
      "resume_cursor": "7"
    }"#;

    fn absorb(body: &str, st: &mut SessionEvents) -> Option<String> {
        let v: Value = serde_json::from_str(body).unwrap();
        absorb_events_page(&v, st).unwrap()
    }

    #[test]
    fn a_repeated_message_is_counted_once_at_its_largest() {
        let mut st = SessionEvents::default();
        let next = absorb(EVENTS_P1, &mut st);
        assert_eq!(next.as_deref(), Some("4"));
        assert_eq!(st.messages(), 1, "three events, one message");

        let u = st.usage();
        assert_eq!(u.input, 2, "not 4 — the copies are the same message");
        assert_eq!(u.cache_read, 106_580);
        assert_eq!(u.cache_1h, 1214);
        assert_eq!(u.output, 3, "the largest copy seen, not the sum and not the first");
    }

    #[test]
    fn the_walk_resumes_where_it_stopped_and_stops_on_the_last_page() {
        let mut st = SessionEvents::default();
        st.cursor = absorb(EVENTS_P1, &mut st);
        let next = absorb(EVENTS_P2, &mut st);
        assert!(next.is_none(), "no next_cursor means the stream is caught up");
        assert_eq!(
            st.cursor.as_deref(),
            Some("7"),
            "the mark still advances past the last page, or it would be re-read"
        );

        // Still one `msg_a`, even though it spanned the page boundary.
        assert_eq!(st.messages(), 2);
        let by = st.by_model();
        assert_eq!(by.len(), 2);
        assert_eq!(by[0].0, "claude-fable-5-1");
        assert_eq!(by[0].1.cache_5m, 40);
        assert_eq!(by[1].0, "claude-opus-5");
        assert_eq!(by[1].1.output, 3);
    }

    #[test]
    fn only_assistant_events_carry_usage() {
        let v: Value = serde_json::from_str(EVENTS_P2).unwrap();
        let rows = v["data"].as_array().unwrap();
        // The `result` event has a `usage` block, but it is zeroed on every
        // session — reading it would add noise, so it is skipped by type.
        assert!(event_usage(&rows[2]).is_none());
        assert!(event_usage(&rows[1]).is_some());
    }

    #[test]
    fn a_session_id_that_is_not_one_is_refused_before_the_wire() {
        let mut st = SessionEvents::default();
        assert!(matches!(
            fetch_events("tok", "cse_abc/../../v1", &mut st),
            Err(FetchError::Malformed)
        ));
        assert!(matches!(fetch_events("", "cse_abc", &mut st), Err(FetchError::Malformed)));
        assert!(matches!(fetch_events("tok", "nonsense", &mut st), Err(FetchError::Malformed)));
    }

    #[test]
    fn the_device_label_is_a_slug_not_a_fqdn() {
        let l = device_label();
        assert!(!l.is_empty());
        assert!(!l.contains('.'), "{l}");
        assert_eq!(l, l.to_ascii_lowercase());
    }
}
