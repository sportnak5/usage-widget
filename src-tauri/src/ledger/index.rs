//! Incremental index over `~/.claude/projects/**/*.jsonl`.
//!
//! Transcript files are append-only, so we remember `(size, mtime, offset)`
//! per file and read only the bytes that arrived since. A file that shrank or
//! whose mtime went backwards was rewritten; its records are dropped and it is
//! read again from the start. Files whose mtime predates the widest window are
//! never opened — on a 7,000-file history that is the difference between a
//! cold scan and a steady state of a few hundred KB per refresh.
//!
//! Dedup is global across files on `message.id`: forks, resumes and compaction
//! copy the same assistant turn into new files, and about 70% of usage rows
//! are such copies.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::record::{parse_line, Parsed, Rec, Title, TitleKind};

/// How far back we keep records. Must exceed the widest window (7 days) so no
/// window can ever see a record whose dedup key was pruned.
pub const RETENTION_DAYS: i64 = 8;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileState {
    pub size: u64,
    pub mtime_ms: i64,
    pub offset: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileEntry {
    path: PathBuf,
    state: FileState,
    /// Files vanish on `claude rm` and project cleanup; set false during a
    /// scan when the file is present, and prune the rest.
    #[serde(skip)]
    seen_this_scan: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stored {
    #[serde(rename = "f")]
    file: u32,
    #[serde(flatten)]
    rec: Rec,
}

/// A captured `bridgeSessionId`, remembered against the transcript it came
/// from. It outlives that session's usage records deliberately: the line is
/// written once, at the top of the file, and the offset is past it forever
/// after, so pruning it with the records would lose it for good.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct BridgeRef {
    #[serde(rename = "f")]
    file: u32,
    #[serde(rename = "b")]
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Index {
    version: u32,
    files: Vec<FileEntry>,
    recs: Vec<Stored>,
    titles: HashMap<String, Title>,
    /// Local session id → the account-wide session id it is mirrored under.
    /// Only sessions with remote control on ever write one.
    #[serde(default)]
    bridge: HashMap<String, BridgeRef>,
    #[serde(skip)]
    seen: HashSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub files_total: usize,
    pub files_in_window: usize,
    pub files_read: usize,
    pub bytes_read: u64,
    pub lines_seen: u64,
    pub records_added: usize,
    pub duplicates_skipped: usize,
    pub records_retained: usize,
    pub duration_ms: u128,
}

/// Bumped when the parser starts caring about a line it used to skip: offsets
/// are per-file, so anything already read is never looked at again unless the
/// stored index is thrown away. Version 2 added `bridge-session` capture.
const INDEX_VERSION: u32 = 2;

impl Default for Index {
    fn default() -> Self {
        Index {
            version: INDEX_VERSION,
            files: Vec::new(),
            recs: Vec::new(),
            titles: HashMap::new(),
            bridge: HashMap::new(),
            seen: HashSet::new(),
        }
    }
}

impl Index {
    pub fn load(path: &Path) -> Index {
        let mut idx: Index = fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if idx.version != INDEX_VERSION {
            idx = Index::default();
        }
        idx.version = INDEX_VERSION;
        idx.rebuild_seen();
        idx
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(tmp, path)
    }

    fn rebuild_seen(&mut self) {
        self.seen = self.recs.iter().map(|s| s.rec.id.clone()).collect();
    }

    pub fn records(&self) -> impl Iterator<Item = &Rec> {
        self.recs.iter().map(|s| &s.rec)
    }

    /// Every retained record's timestamp, ascending. Drives session-block detection.
    pub fn timestamps_sorted(&self) -> Vec<DateTime<Utc>> {
        let mut v: Vec<_> = self.recs.iter().map(|s| s.rec.ts).collect();
        v.sort_unstable();
        v
    }

    pub fn title(&self, session: &str) -> Option<&Title> {
        self.titles.get(session)
    }

    /// Account-wide session id → the local session that wrote it. A row in the
    /// account's session list that appears here ran on this machine; one that
    /// doesn't has not been seen here, which is as close to device identity as
    /// the API allows.
    pub fn bridge_sessions(&self) -> HashMap<&str, &str> {
        self.bridge.iter().map(|(sid, b)| (b.id.as_str(), sid.as_str())).collect()
    }

    /// The transcript a session most recently wrote a billable turn to. A
    /// resumed conversation keeps its id across files, so the newest record
    /// names the file whose tail is the live one.
    pub fn session_file(&self, session: &str) -> Option<&Path> {
        self.recs
            .iter()
            .filter(|s| s.rec.session == session)
            .max_by_key(|s| s.rec.ts)
            .and_then(|s| self.files.get(s.file as usize))
            .map(|f| f.path.as_path())
    }

    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// Walk `projects_dir`, ingest what changed, prune what aged out.
    pub fn refresh(&mut self, projects_dir: &Path, now: DateTime<Utc>) -> ScanStats {
        let started = Instant::now();
        let mut stats = ScanStats::default();
        let cutoff = now - Duration::days(RETENTION_DAYS);
        let cutoff_ms = cutoff.timestamp_millis();

        for f in &mut self.files {
            f.seen_this_scan = false;
        }
        let mut by_path: HashMap<PathBuf, usize> =
            self.files.iter().enumerate().map(|(i, f)| (f.path.clone(), i)).collect();

        for path in list_transcripts(projects_dir) {
            stats.files_total += 1;
            let Ok(meta) = fs::metadata(&path) else { continue };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            if mtime_ms < cutoff_ms {
                continue;
            }
            stats.files_in_window += 1;
            let size = meta.len();

            let idx = match by_path.get(&path) {
                Some(&i) => i,
                None => {
                    self.files.push(FileEntry { path: path.clone(), state: FileState::default(), seen_this_scan: true });
                    let i = self.files.len() - 1;
                    by_path.insert(path.clone(), i);
                    i
                }
            };
            self.files[idx].seen_this_scan = true;
            let st = self.files[idx].state.clone();
            if st.size == size && st.mtime_ms == mtime_ms {
                continue;
            }

            let start = if size >= st.size && st.offset <= size {
                st.offset
            } else {
                // Rewritten in place: forget everything we took from it.
                self.drop_file(idx as u32);
                0
            };

            match self.ingest(&path, idx as u32, start, cutoff, &mut stats) {
                Ok(new_offset) => {
                    stats.files_read += 1;
                    self.files[idx].state = FileState { size, mtime_ms, offset: new_offset };
                }
                Err(_) => {
                    // Leave the old state; we'll try again next refresh.
                }
            }
        }

        // Files that no longer exist.
        let gone: Vec<u32> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.seen_this_scan && !f.path.exists())
            .map(|(i, _)| i as u32)
            .collect();
        for g in gone {
            self.drop_file(g);
        }

        self.prune(cutoff);
        stats.records_retained = self.recs.len();
        stats.duration_ms = started.elapsed().as_millis();
        stats
    }

    fn drop_file(&mut self, file: u32) {
        self.recs.retain(|s| s.file != file);
        // Rewritten from the start, or gone: either way the line is re-read or
        // it no longer describes anything.
        self.bridge.retain(|_, b| b.file != file);
        self.rebuild_seen();
    }

    fn prune(&mut self, cutoff: DateTime<Utc>) {
        let before = self.recs.len();
        self.recs.retain(|s| s.rec.ts >= cutoff);
        if self.recs.len() != before {
            self.rebuild_seen();
        }
        // Titles for sessions we no longer hold any record of. Bridge ids are
        // not pruned here — they are tied to their file instead, see BridgeRef.
        let live: HashSet<&str> = self.recs.iter().map(|s| s.rec.session.as_str()).collect();
        self.titles.retain(|sid, _| live.contains(sid.as_str()));
    }

    /// Read complete lines from `start`; return the offset just past the last
    /// newline consumed, so a half-written trailing line is re-read next time.
    fn ingest(
        &mut self,
        path: &Path,
        file: u32,
        start: u64,
        cutoff: DateTime<Utc>,
        stats: &mut ScanStats,
    ) -> std::io::Result<u64> {
        let mut fh = File::open(path)?;
        fh.seek(SeekFrom::Start(start))?;
        let mut reader = BufReader::with_capacity(1 << 16, fh);
        let mut offset = start;
        let mut buf: Vec<u8> = Vec::with_capacity(1 << 14);

        loop {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }
            if buf.last() != Some(&b'\n') {
                break; // partial line: leave it for the next refresh
            }
            offset += n as u64;
            stats.bytes_read += n as u64;
            stats.lines_seen += 1;

            let Ok(line) = std::str::from_utf8(&buf) else { continue };
            let titles = &self.titles;
            let want_prompt = |sid: &str| !matches!(titles.get(sid), Some(t) if t.kind == TitleKind::Custom) && !titles.contains_key(sid);
            match parse_line(line, want_prompt) {
                Parsed::Usage(rec) => {
                    if rec.ts < cutoff {
                        continue;
                    }
                    if self.seen.contains(&rec.id) {
                        stats.duplicates_skipped += 1;
                        continue;
                    }
                    self.seen.insert(rec.id.clone());
                    self.recs.push(Stored { file, rec });
                    stats.records_added += 1;
                }
                Parsed::Title { session, title } => {
                    let replace = match self.titles.get(&session) {
                        None => true,
                        Some(existing) => title.kind == TitleKind::Custom && existing.kind != TitleKind::Custom
                            || title.kind == TitleKind::Custom && existing.kind == TitleKind::Custom,
                    };
                    if replace {
                        self.titles.insert(session, title);
                    }
                }
                Parsed::Bridge { session, bridge_id } => {
                    self.bridge.insert(session, BridgeRef { file, id: bridge_id });
                }
                Parsed::Skip => {}
            }
        }
        // Consume anything we might have buffered but not returned.
        let _ = reader.read(&mut [0u8; 0]);
        Ok(offset)
    }
}

/// `projects/<slug>/<session>.jsonl` — one level of project dirs, files inside.
pub fn list_transcripts(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(dirs) = fs::read_dir(projects_dir) else { return out };
    for d in dirs.flatten() {
        let p = d.path();
        if !p.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&p) else { continue };
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().map(|e| e == "jsonl").unwrap_or(false) {
                out.push(fp);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn usage_line(id: &str, ts: &str, sid: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"req_{id}","timestamp":"{ts}","cwd":"/p","sessionId":"{sid}","message":{{"model":"claude-opus-5","id":"msg_{id}","usage":{{"input_tokens":1,"output_tokens":{out},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
        ) + "\n"
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("token-ledger-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("proj-a")).unwrap();
        d
    }

    #[test]
    fn dedups_across_files_and_reads_incrementally() {
        let root = tmpdir("dedup");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        let b = root.join("proj-a/b.jsonl");
        fs::write(&a, usage_line("1", "2026-09-08T19:00:00Z", "s1", 100) + &usage_line("2", "2026-09-08T19:01:00Z", "s1", 200)).unwrap();
        // b is a fork of a: repeats msg_1, adds msg_3
        fs::write(&b, usage_line("1", "2026-09-08T19:00:00Z", "s2", 100) + &usage_line("3", "2026-09-08T19:02:00Z", "s2", 300)).unwrap();

        let mut idx = Index::default();
        let s = idx.refresh(&root, now);
        assert_eq!(s.records_added, 3);
        assert_eq!(s.duplicates_skipped, 1);
        assert_eq!(idx.len(), 3);

        // Nothing changed: nothing read.
        let s = idx.refresh(&root, now);
        assert_eq!(s.files_read, 0);
        assert_eq!(s.bytes_read, 0);

        // Append to a; only the new bytes are read.
        let mut fh = fs::OpenOptions::new().append(true).open(&a).unwrap();
        fh.write_all(usage_line("4", "2026-09-08T19:05:00Z", "s1", 400).as_bytes()).unwrap();
        // Force a distinct mtime on coarse filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        drop(fh);
        let s = idx.refresh(&root, now);
        assert_eq!(s.files_read, 1);
        assert_eq!(s.records_added, 1);
        assert!(s.bytes_read < 400);
        assert_eq!(idx.len(), 4);
    }

    #[test]
    fn partial_trailing_line_is_left_for_next_time() {
        let root = tmpdir("partial");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        let full = usage_line("1", "2026-09-08T19:00:00Z", "s1", 100);
        let half = usage_line("2", "2026-09-08T19:01:00Z", "s1", 200);
        let half = &half[..half.len() / 2];
        fs::write(&a, full.clone() + half).unwrap();

        let mut idx = Index::default();
        let s = idx.refresh(&root, now);
        assert_eq!(s.records_added, 1);
        assert_eq!(idx.files[0].state.offset as usize, full.len());

        fs::write(&a, full.clone() + &usage_line("2", "2026-09-08T19:01:00Z", "s1", 200)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let s = idx.refresh(&root, now);
        assert_eq!(s.records_added, 1);
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn rewritten_file_is_reread_and_old_records_dropped() {
        let root = tmpdir("rewrite");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        fs::write(&a, usage_line("1", "2026-09-08T19:00:00Z", "s1", 100) + &usage_line("2", "2026-09-08T19:01:00Z", "s1", 200)).unwrap();
        let mut idx = Index::default();
        idx.refresh(&root, now);
        assert_eq!(idx.len(), 2);

        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&a, usage_line("9", "2026-09-08T19:00:00Z", "s1", 900)).unwrap(); // shrank
        idx.refresh(&root, now);
        let ids: Vec<_> = idx.records().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["msg_9"]);
    }

    #[test]
    fn old_records_and_old_files_are_ignored() {
        let root = tmpdir("old");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        fs::write(&a, usage_line("1", "2026-08-01T19:00:00Z", "s1", 100) + &usage_line("2", "2026-09-08T19:01:00Z", "s1", 200)).unwrap();
        let mut idx = Index::default();
        let s = idx.refresh(&root, now);
        assert_eq!(s.records_added, 1, "record older than retention is dropped at ingest");
        assert_eq!(idx.records().next().unwrap().id, "msg_2");
    }

    #[test]
    fn titles_prefer_custom_over_prompt() {
        let root = tmpdir("titles");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        let mut body = String::new();
        body += r#"{"type":"user","sessionId":"s1","message":{"role":"user","content":"first prompt here"}}"#;
        body += "\n";
        body += &usage_line("1", "2026-09-08T19:00:00Z", "s1", 100);
        body += r#"{"type":"custom-title","sessionId":"s1","customTitle":"A proper title"}"#;
        body += "\n";
        fs::write(&a, body).unwrap();
        let mut idx = Index::default();
        idx.refresh(&root, now);
        assert_eq!(idx.title("s1").unwrap().text, "A proper title");
        assert_eq!(idx.title("s1").unwrap().kind, TitleKind::Custom);
    }

    #[test]
    fn bridge_ids_survive_their_records_aging_out() {
        let root = tmpdir("bridge");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        let a = root.join("proj-a/a.jsonl");
        let mut body = String::new();
        body += r#"{"type":"bridge-session","sessionId":"s1","bridgeSessionId":"cse_aaa","lastSequenceNum":0}"#;
        body += "\n";
        // Older than retention, so the session holds no records at all — the
        // case that used to lose the id and mislabel the row as remote.
        body += &usage_line("1", "2026-08-01T19:00:00Z", "s1", 100);
        fs::write(&a, body).unwrap();

        let mut idx = Index::default();
        idx.refresh(&root, now);
        assert_eq!(idx.bridge_sessions().get("cse_aaa"), Some(&"s1"));

        // A second refresh reads no new bytes; the id has to still be there.
        idx.refresh(&root, now);
        assert_eq!(idx.bridge_sessions().get("cse_aaa"), Some(&"s1"));

        // The transcript is gone: so is the claim that it ran here.
        fs::remove_file(&a).unwrap();
        idx.refresh(&root, now);
        assert!(idx.bridge_sessions().is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let root = tmpdir("persist");
        let now = DateTime::parse_from_rfc3339("2026-09-08T20:00:00Z").unwrap().with_timezone(&Utc);
        fs::write(root.join("proj-a/a.jsonl"), usage_line("1", "2026-09-08T19:00:00Z", "s1", 100)).unwrap();
        let mut idx = Index::default();
        idx.refresh(&root, now);
        let p = root.join("index.json");
        idx.save(&p).unwrap();
        let mut again = Index::load(&p);
        assert_eq!(again.len(), 1);
        let s = again.refresh(&root, now);
        assert_eq!(s.files_read, 0, "loaded index knows the file is unchanged");
        assert_eq!(s.duplicates_skipped, 0);
    }
}
