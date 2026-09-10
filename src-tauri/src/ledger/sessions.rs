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

use super::usageapi::{self, clean, redact, FetchError};

pub const ENDPOINT: &str = "https://api.anthropic.com/v1/code/sessions";

/// Both headers are load-bearing: without the version the endpoint answers
/// 400, without the token 401. The Usage endpoint's `anthropic-beta` is
/// unrelated and not wanted here.
const API_VERSION: &str = "2023-06-01";
const UA: &str = concat!("token-ledger/", env!("CARGO_PKG_VERSION"));

/// The largest page the endpoint serves.
const PAGE: usize = 100;

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

    #[test]
    fn the_device_label_is_a_slug_not_a_fqdn() {
        let l = device_label();
        assert!(!l.is_empty());
        assert!(!l.contains('.'), "{l}");
        assert_eq!(l, l.to_ascii_lowercase());
    }
}
