# Claude Code Usage Widget — Engineering Handoff v2

**Status:** Working POC published as a web artifact. Data pipeline verified against ~1.5 GB of
real Claude Code transcripts. Ready for native implementation.

**Supersedes:** `claude-code-widget-handoff.md` (Sept 8, 2026). Read the corrections in §1 before
using anything from that document — four of its core assumptions are wrong.

**Deliverables in this folder**

| File | What it is |
|---|---|
| `HANDOFF.md` | This document |
| `gen.py` | Reference implementation of the whole data pipeline (~120 lines, stdlib only) |
| `usage.json` | Example output — the exact shape the UI consumes |
| `usage-widget.html` | The working POC (self-contained, data embedded) |

---

## 1. Corrections to the original handoff

Everything below was verified by reading the actual files, not by inspecting documentation.

| Original doc claimed | Reality |
|---|---|
| Logs at `~/.claude/logs/*.jsonl` | **Does not exist.** Real path is `~/.claude/projects/<cwd-slug>/<sessionId>.jsonl` — 7,459 files, 1.5 GB on the reference machine |
| Flat fields: `tokens`, `model`, `input_tokens` | Nested under `message.usage`; see §2 for the verified schema |
| "Fable total" is a third time window | **Fable is a model** (`claude-fable-5`, `claude-fable-5-1`). The third gauge is the Fable-specific *weekly* cap, exactly as the Usage tab shows it |
| Sum the rows to get usage | **~70% of usage rows are duplicates.** Naive summing overcounts ~2.4×. See §3 |
| Sessions identified by file name | Correct, but a session can span multiple files (resume/fork). Dedupe globally by `message.id` |

**A fifth issue the original doc did not anticipate:** raw token counts are a misleading ranking
metric. 95% of raw tokens are cache reads, which cost ~1/10 to 1/50 of other token kinds.
Ranking by raw tokens ranks conversations by *context length*, not by cost. Everything
user-facing must be cost-weighted. See §4.

---

## 2. Data source — verified schema

**Location:** `~/.claude/projects/<slug>/<sessionId>.jsonl`, where `<slug>` is the working
directory with `/` replaced by `-`. Honor `$CLAUDE_CONFIG_DIR` if set, else `~/.claude`.

**Format:** JSONL, append-only. One JSON object per line. Multiple record types share the file;
filter on what you need.

### Usage records

Any line where `message.usage` exists is a billable assistant turn.

```json
{
  "type": "assistant",
  "uuid": "26bdd524-886c-4456-bcb0-a937344a1652",
  "requestId": "req_011CerWfNa4CfrEsP88PwMYx",
  "sessionId": "f61278b4-7287-4e15-9ed7-ed9b4ce75075",
  "timestamp": "2026-09-08T19:19:05.900Z",
  "cwd": "/Users/crawfordnakayama",
  "isSidechain": false,
  "apiBlockIndex": 0,
  "version": "2.1.260",
  "gitBranch": "HEAD",
  "message": {
    "id": "msg_011CerWfPWcEbamSwhsEQB9Q",
    "model": "claude-opus-5",
    "usage": {
      "input_tokens": 2,
      "output_tokens": 334,
      "cache_creation_input_tokens": 19614,
      "cache_read_input_tokens": 39088,
      "cache_creation": {
        "ephemeral_5m_input_tokens": 0,
        "ephemeral_1h_input_tokens": 19614
      },
      "output_tokens_details": { "thinking_tokens": 140 },
      "service_tier": "standard"
    }
  }
}
```

Fields that matter, and nothing else does:

| Path | Use |
|---|---|
| `message.id` | **Dedup key.** Fall back to `requestId` if absent |
| `message.model` | Pricing tier and segment color |
| `message.usage.input_tokens` | Fresh input |
| `message.usage.output_tokens` | Generated output — the expensive one |
| `message.usage.cache_creation_input_tokens` | Total cache writes |
| `message.usage.cache_creation.ephemeral_5m_input_tokens` | 5-minute TTL writes (1.25× input rate) |
| `message.usage.cache_creation.ephemeral_1h_input_tokens` | 1-hour TTL writes (2× input rate) |
| `message.usage.cache_read_input_tokens` | Cache hits — cheap, but 95% of raw volume |
| `timestamp` | ISO 8601, always UTC with `Z`. Window bucketing |
| `sessionId` | Conversation grouping |
| `cwd` | Absolute project path. Prefer this over parsing the directory slug |
| `isSidechain` | Subagent turns. See §8 — currently always false, unresolved |

If `cache_creation` is missing, attribute all of `cache_creation_input_tokens` to the 5m rate.

### Title records

Two other record types give you human-readable conversation names:

```json
{ "type": "custom-title", "sessionId": "...", "customTitle": "Packages and allowances in catalog" }
{ "type": "user", "sessionId": "...", "message": { "content": "..." } }
```

Resolution order: `customTitle` → first `user` message text (trimmed, collapsed whitespace,
truncated to ~52 chars) → the `cwd` basename. Skip user content starting with `<` (system-injected).
`content` is either a string or an array of blocks — take the first block with `type: "text"`.

---

## 3. The dedup rule (do not skip this)

Claude Code rewrites history into new files on fork, resume, and compaction. The same assistant
turn — same `message.id`, same `requestId`, byte-identical `usage` — appears 2–3 times across
files, and sometimes multiple times within one file.

Measured on the reference machine: **1,254 duplicate groups out of 1,800 unique requests** over a
two-day window. Summing without dedup inflates every figure by roughly 2.4×.

```python
seen = set()
key = message.get("id") or record.get("requestId")
if key in seen: continue
seen.add(key)
```

The set must be **global across all files**, not per-file. On the reference dataset a weekly
window holds ~17k unique messages, so the set is small; memory is not a concern.

Also filter out `message.model == "<synthetic>"` — these are local error/interrupt placeholders
with zero usage.

---

## 4. Cost weighting

Ranking by raw tokens is wrong. Weight every message by published API rates before any
display or sort. Rates in **$ per 1M tokens**:

| Model | Input | Output | Cache write 5m | Cache write 1h | Cache read |
|---|---:|---:|---:|---:|---:|
| `claude-fable-5-1` | 10.00 | 50.00 | 12.50 | 20.00 | **0.25** |
| `claude-fable-5` | 10.00 | 50.00 | 12.50 | 20.00 | 1.00 |
| `claude-opus-5` | 5.00 | 25.00 | 6.25 | 10.00 | 0.50 |
| `claude-opus-4-8` | 5.00 | 25.00 | 6.25 | 10.00 | 0.50 |
| `claude-sonnet-5` | 2.00 | 10.00 | 2.50 | 4.00 | 0.20 |
| `claude-haiku-4-5-20251001` | 1.00 | 5.00 | 1.25 | 2.00 | 0.10 |

General rule for models not listed: cache write 5m = 1.25× input, cache write 1h = 2× input,
cache read = 0.1× input. Fable 5.1's cache read is a documented exception at a flat $0.25.

**These are a proxy, not ground truth.** Anthropic meters subscription usage its own way and does
not publish the formula. The proxy is validated in §5 — when all three gauges agree, it's close.

Keep the rate table in config, not compiled in. Model IDs and prices change.

---

## 5. Windows and calibration — the hard part

**Correction (Sept 8, evening):** there *is* a local copy. `~/.claude.json` — the file next to
the `.claude` directory, not inside it — carries `cachedUsageUtilization`:

```jsonc
{ "fetchedAtMs": 1788555619606,
  "utilization": {
    "five_hour": { "utilization": 6, "resets_at": "2026-09-05T00:59:59Z" },
    "seven_day": { "utilization": 1, "resets_at": "2026-09-08T05:59:59Z" },
    "limits": [
      { "kind": "session",       "percent": 6, "resets_at": "..." },
      { "kind": "weekly_all",    "percent": 1, "resets_at": "..." },
      { "kind": "weekly_scoped", "percent": 0, "scope": { "model": { "display_name": "Fable" } } }
    ] } }
```

That is the Usage tab, machine-readable, with exact reset instants. Two limits: Claude Code writes
it opportunistically (the reference machine's copy was four days old), and the percentages are
integers, so a reading under ~10% carries too much rounding error to anchor a limit on. The app
therefore auto-calibrates from it whenever `fetchedAtMs` is newer than the last reading applied
and the percentage is ≥10, takes the reset instants unconditionally, and falls back to manual
entry otherwise. A manual entry newer than the cache always wins. There is still no `claude usage` CLI command. **Verified refresh path:** typing `/usage` inside an
interactive terminal `claude` session rewrites the cache immediately (fetchedAtMs moved from Sep 4 to
the current minute). The desktop app's Usage tab does *not* write it.

Everything below still applies for the manual path and for turning percentages into limits.

### The three windows

| Window | Duration | Boundary |
|---|---|---|
| Session | 5 hours, rolling | Derived from the countdown the Usage tab shows |
| Weekly, all models | 7 days | Resets a fixed weekday/hour (Tue 1:00 AM local on the reference account) |
| Weekly, Fable | 7 days | Same boundary, separate and smaller cap |

**Deriving the session start — verified rule:** the 5-hour block opens with the first message
after the previous block closed and lasts exactly five hours. Simulating that over the deduplicated
records reproduced the server's own reset instant (from `cachedUsageUtilization`) to within 16
seconds — the request/response latency. So the boundary needs no external input at all: chain
blocks forward from the transcripts, and use a server-reported `resets_at` as the exact value when
it is still in the future, or as the point to resume the chain from when it has passed. Between
blocks the session is *idle*: nothing is metered until the next message.

**Deriving the weekly start:** the tab says "Resets Tue 1:00 AM" — ambiguous when today is
Tuesday. Resolve it by consistency check, not by guessing:

> If the weekly window is assumed to have opened a week ago, the implied weekly cap works out to
> ~70 five-hour blocks. A week contains only 33.6 such blocks, so the weekly cap could never
> bind — which contradicts its existence. The correct reading is the recent boundary, which
> implies a cap of ~8.2 maxed session blocks.

Build this check in. It catches the ambiguity automatically.

### Calibration

You cannot compute an absolute limit, but you don't need one. Given the official percentage `P`
for a window and your weighted total `T` for that same window:

```
implied_limit          = T / (P / 100)
contributor_pct_of_cap = contributor_weighted_cost / T * P
```

This converts "share of my own usage" into "share of that limit" without ever knowing the limit.
It is exact regardless of how wrong the pricing proxy is, as long as the proxy is *consistently*
wrong across models — which is the assumption to keep testing.

**Validation:** on the reference account the three independent gauges (78% / 28% / 53%) produced
mutually consistent implied caps — session ≈ $72, weekly ≈ $590, Fable weekly ≈ $272, i.e. the
Fable cap is ~46% of the all-models cap and the weekly cap is ~8× the session cap. Three
independent numbers agreeing is meaningful evidence the weighting is sound.

### Where the official percentages come from — **the main open decision**

The POC has them hardcoded, refreshed by hand from a screenshot. For a shipped app, pick one:

1. **User-entered, with a reminder to refresh.** Honest, zero risk, degrades gradually.
2. **Scrape the authenticated endpoint** the desktop app uses, with the OAuth token from the
   macOS Keychain / Windows Credential Manager. Undocumented, unsupported, may break without
   notice, and means the widget touches auth credentials — which the original handoff explicitly
   ruled out under its privacy constraint. **Get an explicit decision before building this.**
3. **Ship uncalibrated.** Drop the percentages, show relative breakdown only. Loses the gauges
   but needs nothing external.

---

## 6. Pace-based color

Fill percentage alone is a bad signal: 99% used with a minute left is fine; 99% with ten hours
left is not. Color by **burn rate**:

```
elapsed_fraction = clamp((now - window_start) / (window_end - window_start), 0.02, 1.0)
pace             = pct_used / (elapsed_fraction * 100)
```

`pace = 1.0` means exactly on track to reach the cap as the window closes.

```
if pct_used >= 100:  position = 1.0
elif pace <= 0.8:    position = 0.0
elif pace <= 1.0:    position = (pace - 0.8) / 0.2 * 0.4
else:                position = 0.4 + 0.6 * min(1, log2(pace) / log2(5))
```

The logarithm above 1.0× matters. A linear ramp pegs everything at red, because real work is
bursty and a window sampled early routinely shows several times the linear target. Measured on
the reference account: session 2.3×, weekly 3.3×, Fable 6.3× — three distinct colors under the
log scale, three identical reds under a linear one.

**Hue ramp** — stops on `position`, interpolated linearly between them:

```
0.00 → 148°   0.30 → 126°   0.50 → 70°   0.70 → 38°   0.85 → 18°   1.00 → 2°
```

Non-uniform on purpose: a linear 142°→0° sweep spends its middle in washed-out yellow-green where
green, yellow and orange are hard to tell apart. Saturation 76% / lightness 40% in light theme,
74% / 60% in dark. Yellows also need a lightness dip — they read pale at the lightness that suits
greens and reds: `lightness × (1 − 0.16 × max(0, 1 − |hue − 68| / 46))`.

---

## 7. Output shape

`gen.py` emits `usage.json`; `usage.json` in this folder is a real example. Structure:

```jsonc
{
  "generated_at": "2026-09-08T15:04:00-05:00",
  "plan": "Max (20x)",
  "boost": "Weekly Claude Code limit is 50% higher through September 13",
  "windows": [
    {
      "id": "session",
      "label": "Current session",
      "sub": "5-hour rolling window",
      "start": "2026-09-08T13:22:00-05:00",
      "reset": "2026-09-08T18:22:00-05:00",
      "pct": 78,                    // official, hand-fed — the calibration anchor
      "total_cost": 85.29,          // weighted dollars
      "total_raw": 106651886,
      "messages": 259,
      "limit": 109.35,              // implied: total_cost / (pct/100)
      "kinds": { "input": 2864, "output": 257820,
                 "cache_write": 2738796, "cache_read": 103652406 },
      "by_model":   [ /* entries */ ],
      "by_project": [ /* entries, top 12 */ ],
      "by_session": [ /* entries, top 15 */ ]
    }
    // ... "weekly", "fable"
  ]
}
```

Every entry in the three breakdown arrays has the same shape:

```jsonc
{
  "key": ["7e80391a-...", "/Users/me/DTCloudSource"],  // string, or [sessionId, cwd]
  "title": "Packages and allowances in catalog",       // null for model/project rows
  "cost": 40.23,                                       // weighted dollars
  "raw": 59777872,
  "n": 98,                                             // message count
  "first": "2026-09-08T13:22:04-05:00",
  "last":  "2026-09-08T15:01:16-05:00",
  "share": 47.17,                                      // % of this window's usage
  "pct": 36.79,                                        // % of this window's LIMIT
  "models": [ { "model": "claude-fable-5-1", "cost": 25.4,
                "input": 712, "output": 22031,
                "cache_write": 495815, "cache_read": 14965976 } ]
}
```

`share` and `pct` are the two numbers the UI displays. Keep both — `share` answers "what
dominated my work", `pct` answers "what is eating my limit".

---

## 8. Unresolved

1. **`isSidechain` is always `false`** across the entire reference history, including sessions
   that definitely ran subagents. Either the flag moved, subagent turns land elsewhere, or they
   are folded into the parent transcript. Until this is resolved, subagent cost is silently
   attributed to the parent conversation. **Investigate before shipping** — for heavy subagent
   users this could misattribute a large fraction of spend.
2. **`apiBlockIndex`** (observed values 0–10) looks like it indexes the 5-hour billing block.
   If it does, it may give the session window boundary directly and remove the dependence on the
   countdown text. Worth ten minutes of investigation.
3. **Pricing proxy vs. actual metering** — see §4. Re-validate whenever the three gauges stop
   agreeing.
4. **Resumed/forked sessions** currently appear as separate rows sharing a title. Dedup by
   `message.id` prevents double-counting the tokens, but the UI shows two entries. Decide whether
   to merge them.
5. **Mini gauges in the widget rows** color by plain percentage, not pace. Pace is not meaningful
   for a single conversation's share of a limit, but the mixed semantics may confuse. Open design
   question.

---

## 9. Native implementation notes

### Drop the Python backend

The original handoff specified Python + Flask + a localhost port + a Tauri-managed subprocess.
Don't. The entire pipeline is a file walk, a JSON parse, and a group-by — do it in Rust inside
Tauri. One process, one binary, no port, no subprocess lifecycle, no orphaned-process bug on
quit, no Python runtime to bundle. `gen.py` is a reference for the *logic*, not the architecture.

### Incremental parsing is mandatory

A cold scan reads 1.5 GB. Acceptable once; unacceptable every minute.

- Persist a per-file `(path, size, mtime, byte_offset)` index. Files are append-only, so seek to
  the stored offset and read only new bytes.
- Skip any file whose `mtime` predates the widest window before opening it. On the reference
  machine this cuts 7,459 files to ~900 for a weekly window and ~40 for the session window.
- Persist the dedup set alongside the offsets, scoped to the widest window.
- Budget a one-time index build of 30–60 s on first run, with a progress state in the UI.
  Steady-state is a few hundred KB per minute.
- Watch for `mtime` going backwards or `size` shrinking — that means the file was rewritten, so
  drop its offset and re-read it.

### Refresh

Once per minute is enough, matching the original spec. Nothing here needs to be faster, and
`log2`-scaled pace colors do not visibly move at higher rates.

---

## 10. UI specification

The POC at `usage-widget.html` is the reference. Open it and match it; the notes below cover
what the file cannot tell you.

### Widget view — the desktop surface

Roughly **400 × 82 collapsed, 400 × 180 expanded**. Wide and short beats tall and narrow.

- Three horizontal units: donut ring on the left, two-line label beside it (short name, `↺ reset
  time`). Hairline dividers between them.
- Ring segments are colored by model and carry the model name plus its % of that limit on hover.
  No legend — the hover is the legend.
- Center of each ring: the percentage, with a **smaller** percent sign (17px number, 9.5px sign).
  Color from the pace ramp in §6.
- Rings and center numbers animate from zero on load (~0.9 s, cubic ease-out). Honor
  `prefers-reduced-motion`.
- Labels are the only interactive affordance — no padded buttons, no hover fills, no selection
  boxes. Active label takes full-contrast ink plus a 1px accent underline; inactive is faint.
  This was an explicit design requirement: it must read as a dashboard, not a control panel.
- Chevron top-right expands 5 rows: model chip · conversation title · raw tokens · mini gauge ·
  % of limit. Clicking a dial label re-scopes those rows to that window.

### Expanded view

- Same three dials at 150px with a 15px ring, plus per-window metadata: opened, resets, message
  count, raw tokens, burn rate.
- **By model / By conversation** toggle re-segments all three rings. Conversation mode uses an
  8-hue categorical palette; anything past the top 8 folds into a grey "everything else" arc.
- Legend below reflects the selected window and current mode.
- Full conversation list with group-by (Sessions / Projects / Models) and expandable rows showing
  input / output / cache-write / cache-read per model.

### Theme

Three states, not two. `data-theme="dark"` / `="light"` on the root when the user chooses; nothing
stamped when following the system, where only `prefers-color-scheme` applies. Every color comes
from a token declared in the base `:root`. Persist an explicit choice; wrap storage access in
try/catch.

---

## 11. Suggested build order

1. Port `gen.py` to Rust. Verify output matches `usage.json` on the same input before touching UI.
2. Add the incremental offset index. Verify a warm run touches <50 files.
3. Tauri shell + widget view only, calibration percentages from a settings pane.
4. Expanded view.
5. Tray icon and platform packaging.

Steps 1–2 are where the correctness risk lives. Everything after is presentation.
