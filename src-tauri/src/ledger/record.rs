//! One line of a Claude Code transcript, reduced to what the ledger needs.
//!
//! Transcripts mix many record types. Two matter: assistant turns that carry
//! `message.usage` (billable), and the records that name a conversation.
//! Everything else — tool results, attachments, queue operations — is skipped
//! before JSON parsing via a cheap substring check, because most of a 1.5 GB
//! history is tool output we never need to decode.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(rename = "i")]
    pub input: u64,
    #[serde(rename = "o")]
    pub output: u64,
    #[serde(rename = "c5")]
    pub cache_5m: u64,
    #[serde(rename = "c1")]
    pub cache_1h: u64,
    #[serde(rename = "cr")]
    pub cache_read: u64,
}

impl Usage {
    pub fn raw(&self) -> u64 {
        self.input + self.output + self.cache_5m + self.cache_1h + self.cache_read
    }
    pub fn cache_write(&self) -> u64 {
        self.cache_5m + self.cache_1h
    }
    pub fn add(&mut self, o: &Usage) {
        self.input += o.input;
        self.output += o.output;
        self.cache_5m += o.cache_5m;
        self.cache_1h += o.cache_1h;
        self.cache_read += o.cache_read;
    }
}

/// A billable assistant turn. Field names are shortened because tens of
/// thousands of these are persisted to the on-disk index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rec {
    /// `message.id`, falling back to `requestId`. The global dedup key.
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "t")]
    pub ts: DateTime<Utc>,
    #[serde(rename = "s")]
    pub session: String,
    #[serde(rename = "d")]
    pub cwd: String,
    #[serde(rename = "m")]
    pub model: String,
    #[serde(rename = "u")]
    pub usage: Usage,
    #[serde(rename = "sc", default)]
    pub sidechain: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TitleKind {
    /// `custom-title` record: what the user (or Claude) named the thread.
    Custom,
    /// First user prompt, cleaned. Used only when no custom title exists.
    Prompt,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Title {
    pub kind: TitleKind,
    pub text: String,
}

pub enum Parsed {
    Usage(Rec),
    Title { session: String, title: Title },
    Skip,
}

/// Pull a string field's value out of raw JSON text without parsing it.
/// Good enough for the few top-level identifiers we peek at, and the reason
/// a cold scan is seconds rather than minutes.
pub fn peek_str<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn u64_at(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// Collapse whitespace, strip the `@"path"` attachment syntax, cap the length.
pub fn clean_title(raw: &str) -> String {
    let joined: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = joined.replace("@\"", "").replace('"', "");
    let mut out: String = cleaned.chars().take(52).collect();
    if cleaned.chars().count() > 53 {
        out.push('…');
    }
    out
}

/// Text of a user message: either a bare string or the first text block.
fn user_text(content: &Value) -> Option<&str> {
    match content {
        Value::String(s) => Some(s.as_str()),
        Value::Array(blocks) => blocks.iter().find_map(|b| {
            (b.get("type")?.as_str()? == "text").then(|| b.get("text")?.as_str())?
        }),
        _ => None,
    }
}

/// Decide what a line is and extract it. `want_prompt_title(session)` lets the
/// caller say whether a user line is even worth decoding — first prompts are
/// only useful for sessions with no custom title yet.
pub fn parse_line(line: &str, want_prompt_title: impl Fn(&str) -> bool) -> Parsed {
    let line = line.trim_end();
    if line.is_empty() {
        return Parsed::Skip;
    }

    if line.contains("\"usage\"") {
        return parse_usage(line).map(Parsed::Usage).unwrap_or(Parsed::Skip);
    }

    if line.contains("\"custom-title\"") {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let (Some(sid), Some(t)) = (
                v.get("sessionId").and_then(Value::as_str),
                v.get("customTitle").and_then(Value::as_str),
            ) {
                if !t.trim().is_empty() {
                    return Parsed::Title {
                        session: sid.to_string(),
                        title: Title { kind: TitleKind::Custom, text: clean_title(t) },
                    };
                }
            }
        }
        return Parsed::Skip;
    }

    if line.contains("\"type\":\"user\"") {
        let Some(sid) = peek_str(line, "sessionId") else { return Parsed::Skip };
        if !want_prompt_title(sid) {
            return Parsed::Skip;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(text) = v.get("message").and_then(|m| m.get("content")).and_then(user_text) {
                // System-injected prompts start with a tag; they are not the user's words.
                if !text.trim_start().starts_with('<') && !text.trim().is_empty() {
                    return Parsed::Title {
                        session: sid.to_string(),
                        title: Title { kind: TitleKind::Prompt, text: clean_title(text) },
                    };
                }
            }
        }
    }

    Parsed::Skip
}

fn parse_usage(line: &str) -> Option<Rec> {
    let v: Value = serde_json::from_str(line).ok()?;
    let msg = v.get("message")?;
    let usage = msg.get("usage")?;
    if !usage.is_object() {
        return None;
    }
    let model = msg.get("model")?.as_str()?.to_string();
    let id = msg
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| v.get("requestId").and_then(Value::as_str))?
        .to_string();
    let ts = v.get("timestamp")?.as_str()?;
    let ts = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);

    let total_write = u64_at(usage, "cache_creation_input_tokens");
    let (mut c5, c1) = match usage.get("cache_creation") {
        Some(cc) => (u64_at(cc, "ephemeral_5m_input_tokens"), u64_at(cc, "ephemeral_1h_input_tokens")),
        None => (0, 0),
    };
    // Older records only report the total; attribute it to the 5-minute rate.
    if c5 + c1 == 0 {
        c5 = total_write;
    }

    Some(Rec {
        id,
        ts,
        session: v.get("sessionId").and_then(Value::as_str).unwrap_or("?").to_string(),
        cwd: v.get("cwd").and_then(Value::as_str).unwrap_or("").to_string(),
        model,
        usage: Usage {
            input: u64_at(usage, "input_tokens"),
            output: u64_at(usage, "output_tokens"),
            cache_5m: c5,
            cache_1h: c1,
            cache_read: u64_at(usage, "cache_read_input_tokens"),
        },
        sidechain: v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE_LINE: &str = r#"{"parentUuid":"x","isSidechain":false,"requestId":"req_1","type":"assistant","uuid":"u1","timestamp":"2026-09-08T19:19:05.900Z","cwd":"/Users/me/proj","sessionId":"sess-1","message":{"model":"claude-opus-5","id":"msg_1","usage":{"input_tokens":2,"cache_creation_input_tokens":19614,"cache_read_input_tokens":39088,"output_tokens":334,"cache_creation":{"ephemeral_1h_input_tokens":19614,"ephemeral_5m_input_tokens":0}}}}"#;

    #[test]
    fn parses_a_real_usage_record() {
        let Parsed::Usage(r) = parse_line(USAGE_LINE, |_| false) else { panic!("expected usage") };
        assert_eq!(r.id, "msg_1");
        assert_eq!(r.session, "sess-1");
        assert_eq!(r.cwd, "/Users/me/proj");
        assert_eq!(r.model, "claude-opus-5");
        assert_eq!(r.usage, Usage { input: 2, output: 334, cache_5m: 0, cache_1h: 19614, cache_read: 39088 });
        assert_eq!(r.usage.raw(), 2 + 334 + 19614 + 39088);
    }

    #[test]
    fn total_only_cache_write_goes_to_5m() {
        let line = USAGE_LINE.replace(r#","cache_creation":{"ephemeral_1h_input_tokens":19614,"ephemeral_5m_input_tokens":0}"#, "");
        let Parsed::Usage(r) = parse_line(&line, |_| false) else { panic!() };
        assert_eq!((r.usage.cache_5m, r.usage.cache_1h), (19614, 0));
    }

    #[test]
    fn falls_back_to_request_id() {
        let line = USAGE_LINE.replace(r#""id":"msg_1","#, "");
        let Parsed::Usage(r) = parse_line(&line, |_| false) else { panic!() };
        assert_eq!(r.id, "req_1");
    }

    #[test]
    fn custom_title_wins_and_is_cleaned() {
        let line = r#"{"type":"custom-title","sessionId":"sess-1","customTitle":"  Packages   and\nallowances "}"#;
        let Parsed::Title { session, title } = parse_line(line, |_| true) else { panic!() };
        assert_eq!(session, "sess-1");
        assert_eq!(title.kind, TitleKind::Custom);
        assert_eq!(title.text, "Packages and allowances");
    }

    #[test]
    fn prompt_title_only_when_wanted_and_not_system() {
        let line = r#"{"type":"user","sessionId":"sess-2","message":{"role":"user","content":[{"type":"text","text":"can you search the catalog?"}]}}"#;
        assert!(matches!(parse_line(line, |_| false), Parsed::Skip));
        let Parsed::Title { title, .. } = parse_line(line, |_| true) else { panic!() };
        assert_eq!(title.kind, TitleKind::Prompt);
        assert_eq!(title.text, "can you search the catalog?");

        let sys = r#"{"type":"user","sessionId":"sess-3","message":{"role":"user","content":"<system-reminder>x</system-reminder>"}}"#;
        assert!(matches!(parse_line(sys, |_| true), Parsed::Skip));
    }

    #[test]
    fn long_titles_are_capped() {
        let t = clean_title(&"word ".repeat(40));
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 53);
    }

    #[test]
    fn peek_finds_top_level_strings() {
        assert_eq!(peek_str(USAGE_LINE, "sessionId"), Some("sess-1"));
        assert_eq!(peek_str(USAGE_LINE, "nope"), None);
    }
}
