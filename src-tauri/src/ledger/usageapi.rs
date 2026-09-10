//! Live readings from the endpoint Claude Code itself polls.
//!
//! `~/.claude.json` only carries a reading as fresh as the last time someone
//! ran `/usage`, and everything downstream of a stale anchor is extrapolation.
//! With a token we can ask the same question Claude Code asks and get the
//! authoritative number on our own cadence, at no token cost — this is a
//! metering read, not an inference call.
//!
//! The credential is Claude Code's own login, read in place. A token minted
//! with `claude setup-token` cannot do this job: it carries only the
//! `user:inference` scope and the endpoint rejects it, which we established by
//! trying one. The session credential is the only thing that works.
//!
//! Read-only, always. We never refresh it and never write it back — the
//! refresh token belongs to Claude Code, and rotating it out from under a
//! running CLI would cost the user their session. An expired credential is
//! reported as expired and the file cache takes over, which is safe: Claude
//! Code renews its own login every time it runs, so the credential only goes
//! stale when the user hasn't used Claude Code in hours — exactly when their
//! usage isn't moving either.

use std::sync::Mutex;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::usagecache::{parse_utilization, UsageCache};

pub const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1";

/// Claude Code's own Keychain item on macOS. The OS prompts the user once
/// before another application may open it; that prompt is the consent gate for
/// this whole feature, and we lean on it rather than working around it.
#[cfg(target_os = "macos")]
const CC_SERVICE: &str = "Claude Code-credentials";

const BETA: &str = "oauth-2025-04-20";
const UA: &str = concat!("token-ledger/", env!("CARGO_PKG_VERSION"));

/// Strip every whitespace character, not just the ends.
///
/// A token pasted into a terminal arrives wrapped across lines, newline and
/// leading space included, and a header value containing either is rejected
/// before it ever leaves the machine. The token alphabet has no whitespace in
/// it, so removing all of it can only repair a paste, never corrupt a token.
pub fn clean(tok: &str) -> String {
    tok.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Keep the token out of anything we might display. Transport errors quote
/// the header they choked on, which means they quote the token.
pub(crate) fn redact(msg: &str, tok: &str) -> String {
    if tok.is_empty() {
        return msg.to_string();
    }
    msg.replace(tok, "<token>")
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// Live readings are switched off. A normal state, not a failure.
    Off,
    /// No Claude Code login found in the Keychain or the credentials file.
    NoCredential,
    /// Found, but past its expiry. Claude Code renews it the next time it
    /// runs; we never renew it ourselves.
    Expired,
    /// 401/403 — revoked, or no longer valid.
    Unauthorized,
    Http(u16),
    Transport(String),
    /// Not a plausible token at all — empty, or carrying characters that
    /// can't go in a header. Caught here so it never reaches the wire, and
    /// so the error we show can't quote it back.
    Malformed,
    /// A 200 whose body wasn't the shape we know. Most likely the endpoint
    /// moved on; the file cache is still there to fall back to.
    Shape,
}

impl FetchError {
    /// One line for the settings panel, in terms of what to do about it.
    pub fn message(&self) -> String {
        match self {
            FetchError::Off => "Live readings are off.".into(),
            FetchError::NoCredential => {
                "No Claude Code login found on this machine. Sign in by running `claude` in a terminal, or leave live readings off and calibrate from `/usage` instead.".into()
            }
            FetchError::Expired => {
                "Claude Code's login has expired. Run `claude` once to renew it — Token Ledger only reads that login, it never renews it.".into()
            }
            FetchError::Unauthorized => {
                "Anthropic rejected Claude Code's login. Run `claude` in a terminal to sign in again.".into()
            }
            FetchError::Http(c) => format!("Anthropic returned HTTP {c}."),
            FetchError::Transport(e) => format!("Couldn't reach Anthropic: {e}"),
            FetchError::Malformed => {
                "Claude Code's stored login isn't in a shape Token Ledger can read.".into()
            }
            FetchError::Shape => {
                "Anthropic answered in a shape Token Ledger doesn't recognise — falling back to Claude Code's cached reading.".into()
            }
        }
    }

    /// Whether the credential is the thing at fault, so the UI can tell
    /// "sign in to Claude Code again" apart from "the network is down".
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            FetchError::Unauthorized | FetchError::Expired | FetchError::NoCredential
        )
    }
}

/// `$CLAUDE_CONFIG_DIR/.credentials.json`, else `~/.claude/.credentials.json`
/// — which on Windows is `%USERPROFILE%\.claude\.credentials.json`, the only
/// place Claude Code keeps it there. macOS normally uses the Keychain and
/// falls back to this file when the Keychain can't be written, such as a
/// locked SSH session. Linux uses the file too.
fn credentials_file() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(std::path::PathBuf::from(d).join(".credentials.json"));
    }
    dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
}

/// The account name Claude Code files its Keychain item under isn't
/// documented, so try the plausible ones rather than betting the feature on a
/// single guess. A wrong guess costs one failed lookup and nothing else.
#[cfg(target_os = "macos")]
fn keychain_accounts() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(u) = std::env::var("USER") {
        out.push(u);
    }
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        out.push(std::path::PathBuf::from(d).display().to_string());
    }
    out.push("Claude Code".into());
    out.push(String::new());
    out
}

/// Which store the credential came out of, so a failure can say where we
/// looked and a success can say what it is using.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Keychain,
    File,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Keychain => "Claude Code's login in the Keychain",
            Source::File => "Claude Code's credentials file",
        }
    }
}

/// Pull the access token out of Claude Code's credential blob.
///
/// Only the access token and the expiry are read. The refresh token sitting
/// next to them is deliberately ignored: using it would rotate a credential
/// this app doesn't own and could sign the user out of their CLI.
fn parse_credentials(raw: &str) -> Option<(String, Option<i64>)> {
    let v: Value = serde_json::from_str(raw).ok()?;
    // `{"claudeAiOauth": {...}}` is what Claude Code writes; accept a bare
    // object too, so a future flattening doesn't break this.
    let o = v.get("claudeAiOauth").unwrap_or(&v);
    let tok = o.get("accessToken")?.as_str()?.trim().to_string();
    if tok.is_empty() {
        return None;
    }
    Some((tok, o.get("expiresAt").and_then(Value::as_i64)))
}

/// The outcome of reading the credential, held for the life of the process —
/// the failures as well as the successes.
///
/// This exists for the user, not for speed. On macOS, opening an item another
/// application created raises a password dialog, and "Allow" — as opposed to
/// "Always Allow" — grants exactly one read. Reading per refresh put that
/// dialog on screen every sixty seconds, which is unusable.
///
/// Caching only the successes left two ways to do exactly that anyway: a
/// login that reads fine but is past its own `expiresAt` never reached the
/// cache, so the next refresh went back to the Keychain, and so on every
/// minute forever; and a token Anthropic rejects made `fetch` drop the cache
/// and read again, which then repeated per refresh. So a failure is now
/// remembered as firmly as a success, and the way back is to say so — toggling
/// live readings, or pressing Check — rather than to keep asking.
///
/// There is no time-based expiry because a clock is the wrong trigger: the
/// only things that invalidate this copy are the token passing its own
/// `expiresAt`, Anthropic rejecting it, and the user asking us to look again.
struct Cached {
    token: String,
    source: Source,
    /// The token's own expiry, when it carries one.
    expires: Option<i64>,
}

/// `None` means we have never looked. Otherwise it is whatever happened last
/// time, good or bad, and we stand by it until something explicitly clears it.
static CACHE: Mutex<Option<Result<Cached, FetchError>>> = Mutex::new(None);

/// Go back to the store on the next call. Called when Anthropic rejects what
/// we sent — Claude Code renews its own login, so a rejection usually means
/// the copy we are holding has been superseded — and whenever the user asks
/// for live readings to be turned on or checked, which is their way of saying
/// "the login is fixed now, look again".
pub fn forget_cached_credential() {
    if let Ok(mut c) = CACHE.lock() {
        *c = None;
    }
}

/// Stop going back to the store until something clears this.
fn remember_failure(e: FetchError) {
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(Err(e));
    }
}

/// What the remembered outcome is worth, or `None` to go back to the store.
/// Split out so the decision can be tested without a Keychain — calling the
/// real reader in a test raises the OS password dialog and hangs there.
fn from_cache(c: Option<&Result<Cached, FetchError>>) -> Option<Result<(String, Source), FetchError>> {
    match c {
        // A token past its own expiry is worth one more look: Claude Code
        // renews it in place, so the item may hold a newer one by now.
        Some(Ok(hit)) if !past_expiry(hit.expires) => Some(Ok((hit.token.clone(), hit.source))),
        Some(Err(e)) => Some(Err(e.clone())),
        _ => None,
    }
}

/// Claude Code's access token, read in place. Never written, never refreshed.
pub fn claude_token() -> Result<(String, Source), FetchError> {
    if let Ok(c) = CACHE.lock() {
        if let Some(hit) = from_cache(c.as_ref()) {
            return hit;
        }
    }
    let read = read_credential();
    if let Ok(mut c) = CACHE.lock() {
        *c = Some(match &read {
            Ok((token, source, expires)) => {
                Ok(Cached { token: token.clone(), source: *source, expires: *expires })
            }
            Err(e) => Err(e.clone()),
        });
    }
    read.map(|(token, source, _)| (token, source))
}

fn past_expiry(expires: Option<i64>) -> bool {
    matches!(expires, Some(ms) if ms > 0 && ms < Utc::now().timestamp_millis())
}

fn read_credential() -> Result<(String, Source, Option<i64>), FetchError> {
    // macOS only. Claude Code stores the credential as a file everywhere
    // else, so asking the Windows Credential Manager or the Linux secret
    // service for it can only ever miss — and on Linux a locked keyring can
    // make that miss slow, or raise an unlock prompt for nothing.
    #[cfg(target_os = "macos")]
    for account in keychain_accounts() {
        let Ok(e) = keyring::Entry::new(CC_SERVICE, &account) else { continue };
        match e.get_password() {
            Ok(raw) => {
                if let Some((tok, expires)) = parse_credentials(&raw) {
                    return check_expiry(tok, expires).map(|t| (t, Source::Keychain, expires));
                }
            }
            // Only a missing item justifies trying the next name. Every other
            // failure — above all a denied or cancelled password dialog — must
            // stop the loop: carrying on would raise the same dialog again for
            // each remaining candidate, which is why switching this on asked
            // for the password three or four times over.
            Err(keyring::Error::NoEntry) => continue,
            Err(_) => break,
        }
    }
    let path = credentials_file().ok_or(FetchError::NoCredential)?;
    let raw = std::fs::read_to_string(&path).map_err(|_| FetchError::NoCredential)?;
    let (tok, expires) = parse_credentials(&raw).ok_or(FetchError::NoCredential)?;
    check_expiry(tok, expires).map(|t| (t, Source::File, expires))
}

/// Reported, never acted on: renewing the login is Claude Code's job.
fn check_expiry(tok: String, expires: Option<i64>) -> Result<String, FetchError> {
    match expires {
        Some(ms) if ms > 0 && ms < Utc::now().timestamp_millis() => Err(FetchError::Expired),
        _ => Ok(tok),
    }
}

/// What each lookup did, for diagnosing a machine where this doesn't work.
/// Reports outcomes only — never the credential itself.
pub fn credential_attempts() -> Vec<(String, String)> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    for account in keychain_accounts() {
        let label = if account.is_empty() { "<empty>".to_string() } else { account.clone() };
        let (outcome, stop) = match keyring::Entry::new(CC_SERVICE, &account) {
            Err(e) => (format!("keychain entry error: {e}"), false),
            Ok(e) => match e.get_password() {
                Err(keyring::Error::NoEntry) => ("no such item".to_string(), false),
                Err(e) => (format!("keychain: {e}"), true),
                Ok(raw) => match parse_credentials(&raw) {
                    Some((_, exp)) => (format!("read ok, {} bytes, expires {exp:?}", raw.len()), true),
                    None => (format!("read ok, {} bytes, but no accessToken in it", raw.len()), true),
                },
            },
        };
        out.push((format!("keychain/{label}"), outcome));
        // Same rule as the real read: never raise a second password dialog
        // just to fill in a diagnostic table.
        if stop {
            break;
        }
    }
    match credentials_file() {
        None => out.push(("file".into(), "no home directory".into())),
        Some(p) => {
            let outcome = match std::fs::read_to_string(&p) {
                Err(e) => format!("{e}"),
                Ok(raw) => match parse_credentials(&raw) {
                    Some((_, exp)) => format!("read ok, {} bytes, expires {:?}", raw.len(), exp),
                    None => format!("read ok, {} bytes, but no accessToken in it", raw.len()),
                },
            };
            out.push((format!("file/{}", p.display()), outcome));
        }
    }
    out
}

/// Whether a credential is there to be read at all — what the settings panel
/// reports before the user switches live readings on.
pub fn credential_source() -> Result<Source, FetchError> {
    claude_token().map(|(_, src)| src)
}

/// Fetch using Claude Code's credential.
///
/// A rejection retries once with a freshly read credential: the cached copy
/// may simply have been superseded by Claude Code renewing its login, and
/// making the user do something about that would be wrong when re-reading
/// fixes it.
pub fn fetch() -> Result<UsageCache, FetchError> {
    let (tok, _) = claude_token()?;
    match fetch_with(&tok) {
        Err(e) if e.is_auth() => {
            forget_cached_credential();
            let (tok, _) = claude_token()?;
            let again = fetch_with(&tok);
            // Rejected again, on a copy we just re-read: the login itself is
            // the problem, not our copy of it. Stop going back to the store,
            // or every refresh from here on raises the dialog twice.
            if let Err(e) = &again {
                if e.is_auth() {
                    remember_failure(e.clone());
                }
            }
            again
        }
        other => other,
    }
}

/// Fetch with an explicit token, so the settings dialog can exercise the whole
/// path — credential, request, parse — before the feature is switched on.
pub fn fetch_with(tok: &str) -> Result<UsageCache, FetchError> {
    let tok = clean(tok);
    if tok.is_empty() || !tok.chars().all(|c| c.is_ascii_graphic()) {
        return Err(FetchError::Malformed);
    }
    let body = get(&tok)?;
    // The endpoint returns the utilization object itself; the file cache wraps
    // that same object under `utilization`. Accept either, so this keeps
    // working if the response ever gains an envelope.
    let u = body.get("utilization").unwrap_or(&body);
    let out = parse_utilization(u, Utc::now());
    if out.session.is_none() && out.weekly.is_none() && out.fable.is_none() {
        return Err(FetchError::Shape);
    }
    Ok(out)
}

fn get(tok: &str) -> Result<Value, FetchError> {
    let res = ureq::get(ENDPOINT)
        .set("Authorization", &format!("Bearer {tok}"))
        .set("anthropic-beta", BETA)
        .set("User-Agent", UA)
        .timeout(std::time::Duration::from_secs(10))
        .call();
    match res {
        Ok(r) => r.into_json::<Value>().map_err(|_| FetchError::Shape),
        Err(ureq::Error::Status(401 | 403, _)) => Err(FetchError::Unauthorized),
        Err(ureq::Error::Status(c, _)) => Err(FetchError::Http(c)),
        Err(ureq::Error::Transport(t)) => Err(FetchError::Transport(redact(&t.to_string(), tok))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The endpoint's body, without the cache's envelope.
    const BODY: &str = r#"{
      "five_hour": {"utilization": 8, "resets_at": "2026-09-09T18:40:00.303957+00:00"},
      "seven_day": {"utilization": 34, "resets_at": "2026-09-15T06:00:00.303983+00:00"},
      "limits": [
        {"kind":"session","percent":8,"resets_at":"2026-09-09T18:40:00.303957+00:00"},
        {"kind":"weekly_all","percent":34,"resets_at":"2026-09-15T06:00:00.303983+00:00"},
        {"kind":"weekly_scoped","percent":59,"resets_at":"2026-09-15T06:00:00.304391+00:00","scope":{"model":{"display_name":"Fable"}}}
      ]}"#;

    #[test]
    fn reads_the_access_token_and_leaves_the_refresh_token_alone() {
        let raw = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-aaa","refreshToken":"sk-ant-ort01-bbb","expiresAt":4102444800000,"scopes":["user:inference"]}}"#;
        let (tok, exp) = parse_credentials(raw).unwrap();
        assert_eq!(tok, "sk-ant-oat01-aaa");
        assert_eq!(exp, Some(4102444800000));
        // The refresh token is right there and must stay untouched: using it
        // would rotate a credential this app doesn't own.
        assert!(!tok.contains("ort01"));
    }

    #[test]
    fn accepts_a_credential_without_the_wrapper() {
        let raw = r#"{"accessToken":"sk-ant-oat01-aaa"}"#;
        assert_eq!(parse_credentials(raw).unwrap(), ("sk-ant-oat01-aaa".into(), None));
    }

    #[test]
    fn an_expired_credential_is_refused_rather_than_refreshed() {
        assert_eq!(check_expiry("t".into(), Some(1)), Err(FetchError::Expired));
        // No expiry recorded is not a reason to refuse.
        assert_eq!(check_expiry("t".into(), None), Ok("t".into()));
        assert!(check_expiry("t".into(), Some(4102444800000)).is_ok());
    }

    #[test]
    fn junk_credentials_are_none_not_a_panic() {
        assert!(parse_credentials("not json").is_none());
        assert!(parse_credentials(r#"{"claudeAiOauth":{"accessToken":"  "}}"#).is_none());
        assert!(parse_credentials("{}").is_none());
    }

    #[test]
    fn a_token_pasted_across_two_lines_is_repaired() {
        assert_eq!(clean("sk-ant-oat01-aaa\n bbb"), "sk-ant-oat01-aaabbb");
        assert_eq!(clean("  sk-ant-oat01-aaa\r\n\tbbb  "), "sk-ant-oat01-aaabbb");
    }

    #[test]
    fn an_unusable_token_never_reaches_the_wire() {
        assert_eq!(fetch_with("   "), Err(FetchError::Malformed));
        // A stray non-ASCII character — a smart quote from a copy-paste, say.
        assert_eq!(fetch_with("sk-ant-oat01-\u{201c}aaa"), Err(FetchError::Malformed));
    }

    #[test]
    fn errors_never_quote_the_token_back() {
        let tok = "sk-ant-oat01-secret";
        let leaked = format!("Bad Header: invalid header 'Authorization: Bearer {tok}'");
        assert!(!redact(&leaked, tok).contains(tok));
    }

    #[test]
    fn reads_the_bare_endpoint_body() {
        let v: Value = serde_json::from_str(BODY).unwrap();
        let u = v.get("utilization").unwrap_or(&v);
        let c = parse_utilization(u, Utc::now());
        assert_eq!(c.session.unwrap().percent, 8.0);
        assert_eq!(c.weekly.unwrap().percent, 34.0);
        assert_eq!(c.fable.unwrap().percent, 59.0);
    }

    #[test]
    fn also_reads_an_enveloped_body() {
        let inner: Value = serde_json::from_str(BODY).unwrap();
        let v = serde_json::json!({ "utilization": inner });
        let u = v.get("utilization").unwrap_or(&v);
        assert_eq!(parse_utilization(u, Utc::now()).session.unwrap().percent, 8.0);
    }

    #[test]
    fn an_unrecognised_body_is_a_shape_error_not_a_zero_reading() {
        let v = serde_json::json!({"something_else": true});
        let u = v.get("utilization").unwrap_or(&v);
        let c = parse_utilization(u, Utc::now());
        assert!(c.session.is_none() && c.weekly.is_none() && c.fable.is_none());
    }

    /// The whole point of the cache is that the OS password dialog appears
    /// once. A failure that isn't remembered brings it back every refresh, so
    /// both outcomes have to stick — and only an explicit ask clears them.
    #[test]
    fn both_outcomes_stick_until_something_asks_again() {
        let good = Ok(Cached { token: "tok".into(), source: Source::File, expires: None });
        assert_eq!(from_cache(Some(&good)).unwrap().unwrap(), ("tok".to_string(), Source::File));

        let bad: Result<Cached, FetchError> = Err(FetchError::Expired);
        assert_eq!(from_cache(Some(&bad)).unwrap().unwrap_err(), FetchError::Expired,
            "a remembered failure has to answer, or the dialog is back every refresh");

        // A token past its own expiry is worth one more look — Claude Code
        // renews in place — so this one goes back to the store exactly once,
        // after which the reader's own verdict is what sticks.
        let stale = Ok(Cached { token: "old".into(), source: Source::File, expires: Some(1) });
        assert!(from_cache(Some(&stale)).is_none());

        // Never looked at all.
        assert!(from_cache(None).is_none());
    }

    #[test]
    fn forgetting_is_the_way_back() {
        *CACHE.lock().unwrap() = Some(Err(FetchError::Unauthorized));
        forget_cached_credential();
        assert!(CACHE.lock().unwrap().is_none(), "Check and the live-readings toggle need a way back");
    }
}
