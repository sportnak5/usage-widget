# Cross-device session list — the API, and what it does not carry

**Verdict:** the account-wide session list *is* reachable with the credential the ledger already
reads (`usageapi::claude_token()`). Device identity is **not** in it — no field in the list or the
detail response names the machine. Everything below was demonstrated live on 2026-09-10 against the
real keychain token, not read out of documentation.

---

## 1. The fetch

```
GET https://api.anthropic.com/v1/code/sessions?limit=100
Authorization: Bearer <claudeAiOauth.accessToken>
anthropic-version: 2023-06-01
```

- Both headers are load-bearing: no `Authorization` → 401, no `anthropic-version` → 400.
- `anthropic-beta` is **not** needed here (the usage endpoint's `oauth-2025-04-20` is unrelated).
- Same token, same keychain item, same read path as the usage fetch — no new credential.
- `include_trigger_sessions=true` (what Claude desktop sends) returned an identical 85 rows on this
  account. Harmless; not required.
- Returns sessions from **every device on the account**, which is the whole point: 37 of the 85 rows
  had no local transcript at all.

### Pagination

Response is `{ data: [...], next_cursor?, resume_token }`.

- `next_cursor` is **absent** when the page is the last one — do not test for null, test for missing.
- Page forward with `?cursor=<next_cursor>` (verified: page 2 of `limit=5` starts at the 6th row of
  the unpaged list). `after_id` and `starting_after` are silently ignored and re-serve page 1.
- `resume_token` is the SSE cursor, not a pagination cursor.

### Rate limiting

The neighbouring usage endpoint 429s readily. Poll on the existing refresh cadence, no faster.

---

## 2. Response shape

Every row (85/85 on this account, so treat rarities as unproven, not absent):

| Field | Observed | Use |
|---|---|---|
| `id` | `cse_01…` (26 chars) | stable session key; matches `bridgeSessionId` in local transcripts |
| `title` | human summary, e.g. "Ledger updates pull and app sync" | row label |
| `status` | `active` 54, `archived` 31 | archived = user closed it out |
| `status_bucket` | `review_ready` 53, `completed` 31, `blocked` 1 | coarse state |
| `worker_status` | `idle` 82, `running` 2, `requires_action` 1 | **the in-flight signal** |
| `connection_status` | `connected` 55, `disconnected` 30 | client attached or not |
| `unread` | bool | 67 true — server-side unread, independent of the ledger's own |
| `last_event_at`, `created_at` | RFC3339 UTC | sort key / "last activity" |
| `user_message_count` | string, e.g. `"0"` | note: **string**, not a number |
| `tags` | `remote-control-sdk`, `remote-control-auto`, `config:auto-create-pr:off` | |
| `config` | `model` (`claude-opus-5`, `claude-fable-5-1`, …), `effort_level`, `origin` (`claude_code_cli` on all 85) | |
| `external_metadata` | `rate_limit_info`, `container_cc_version`, `cross_session_inbound`, `effort_level`, and on 41 rows `worktree_state` / `current_branches` | repo hints, see §4 |
| `environment_id` / `environment_kind` | `""` / `bridge` on all 85 | useless, see §3 |
| `participants`, `relations` | empty arrays throughout | |


### No token counts, anywhere

Nothing in the list prices a conversation. `user_message_count` is a message tally (and a *string*),
and `rate_limit_info` — the only usage-shaped field — is account state, not session state:

```json
{"isUsingOverage": false, "rateLimitType": "five_hour", "resetsAt": 1789144200, "status": "allowed"}
```

Byte-identical on every row probed, same `resetsAt` throughout, no numbers in it. The ledger already
has this window with real percentages from `api/oauth/usage`, so it adds nothing. Per-turn token
usage is written only into the local transcript on the machine that ran the session, which is why a
remote row can show state and a name but never a cost. Verified live 2026-09-11.

**Worth having, not yet used:** `worktree_state` also carries `reported_at` (when that machine last
checked in — a freshness signal distinct from `last_event_at`), `is_dirty`, `unpushed_count`,
`branch` and `head_sha`. That is what a remote row can honestly say instead of a number: not how much
it spent, but that it is sitting on uncommitted or unpushed work.

**Status mapping for a finished/working mark:** `worker_status` is the only field that moves with
the work. `running` = still working; `requires_action` = waiting on the user (pairs with
`status_bucket: blocked`); `idle` + `review_ready` = finished, output waiting; `completed` =
archived/done. `connection_status` tracks the client socket, not the model, so a finished session on
a live device still reads `connected`.

### Session detail

`GET /v1/code/sessions/{id}` — same two headers — returns the object wrapped as
`{"response_shape": { … }}`. Adds `updated_at`, `security_tier`, `requires_action_details_list`,
`client_presence`. It adds **no** device field. `client_presence` was `[]` on every session probed,
including the ones actively running, so it is not a back door to device identity either.

### Live updates

`GET /v1/code/sessions/watch` (SSE, resumed with `Last-Event-ID: <resume_token>`) is what the
desktop app uses. On this account it answers **404 `endpoint not enabled`** — plain text, not JSON,
and not a header problem. Ingest must poll.

---

## 2b. `GET /v1/code/sessions/{id}/events` — the remote session's own transcript

Found by grepping the **CLI** binary (`/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe`),
which uses it for history prefetch. The desktop bundle does not mention it. Verified live 2026-09-11.

```
GET /v1/code/sessions/{id}/events?limit=200&sort_order=desc|asc[&cursor=…]
```

Same two headers, same OAuth token, **200 on a session that never ran on this machine**. Response is
`{data: [...], next_cursor?, resume_token}` — `next_cursor` absent on the last page, as with the list.
`resume_cursor` is the incremental handle: re-fetch only what arrived since.

Every event: `event_id`, `sequence_num`, `created_at`, `event_type`, `source`, `sent_by_account_id`,
`device_attestation_status`, `payload`.

- `event_type`: `assistant`, `user`, `system`, `result`, `rate_limit_event`.
- `source`: `worker` / `client` only — **not a device**. `device_attestation_status` was
  `UNSPECIFIED` on all 1,373 events probed. So this is not a back door to device identity either.

### Token counts DO travel — except output

`assistant` payloads carry `message.model` and a full `message.usage`. **A message id repeats across
2–3 events, so dedupe by `message.id` before summing.** Checked against the local transcript for the
same session (`cse_01Ffx3gEy1Eb…`, 97 events / 148 transcript lines, 25 distinct message ids):

| Field | API events | Local transcript |
|---|---|---|
| `input_tokens` | 50 | 50 ✅ |
| `cache_creation_input_tokens` | 33,636 | 33,636 ✅ |
| `cache_read_input_tokens` | 1,651,083 | 1,651,083 ✅ |
| `output_tokens` | **157** | **9,364** ❌ |

Input and both cache classes are exact. Output is a streaming-start placeholder — 1–3 tokens on every
message, where the transcript has 150–580 — and it never gets revised, so the final count is simply
not in the stream. Since `pricing.rs` weights output far above cache reads, a remote row priced from
events would understate cost by roughly a fifth and rank too low against local rows. Say so on the
row, or price it as a floor; do not present it as a like-for-like number.

`result` payloads are useless for this: `total_cost_usd` is `0` and `modelUsage` is `{}` on every
session probed.

### `rate_limit_event` — real utilization percentages

```json
{"rate_limit_info": {"rateLimitType": "five_hour", "status": "allowed", "isUsingOverage": false,
  "overageStatus": "rejected", "overageDisabledReason": "org_level_disabled",
  "unifiedWindows": {"five_hour": {"resetsAt": 1789144200, "utilization": 0.32},
                     "seven_day": {"resetsAt": 1789452000, "utilization": 0.48}}}}
```

This is the field the session list's flattened `rate_limit_info` lacks: `unifiedWindows.utilization`
for both windows, stamped at the moment the event was written. Account-wide, not per-session — so it
is a time-stamped cross-check on the Usage gauges (and a fallback when that endpoint 429s), not a
per-conversation number.

### Cost of ingesting this

One request per session per 200 events. 38 remote rows is ~40-80 requests for a cold sync against an
endpoint whose neighbour rate-limits readily — so this cannot ride the 30s refresh. Fetch on demand
or on a slow cadence, newest-first, skipping archived rows, and keep `resume_cursor` per session so
subsequent passes are incremental.

---

## 3. Device identity: not available

Every lead from the desktop app bundle was probed and closed:

| Lead | Result |
|---|---|
| `POST /v1/code/sessions/{id}/bridge` (desktop) | claims a session for the caller's bridge; write path, and gated on `X-Trusted-Device-Token`, which the ledger has no way to mint |
| `source` / `device_attestation_status` on session events (§2b) | `worker`/`client` and `UNSPECIFIED` — carry no machine identity |
| `GET /v1/environments/{id}/work/stats` (beta `managed-agents-2026-04-01`) | Managed Agents surface, keyed by an environment id; `/v1/environments` is empty for this account, so there is no id to ask about |
| `environment_id` on the session | `""` on all 85; `environment_kind` is `bridge` for everything CLI-originated |
| `GET /v1/environments` (needs `anthropic-beta: environments-2025-11-01`) | 200 `{"data":[]}` despite 85 live bridge sessions — bridge registrations are not listed |
| `POST /v1/environments/bridge` | registration call, body carries `machine_name`; write path, and the names never come back out of any read endpoint found |
| `/api/organizations/{org}/cowork/remote_devices` | `oauth_token_not_accepted` — desktop calls it cookie-authed, and for row encryption keys, not names |
| `X-Trusted-Device-Token` header | desktop-only attestation seen in the bundle; the ledger has no such token to send, so untested — and it is sent *to* the server, not a source of other devices' names |
| `claude.ai/api/*` | Cloudflare-challenged, browser cookies only |

Device names do travel — over the desktop's relay websocket (`{type:"connect", oauth_token,
device_name, client_type:"desktop"}`, found in `Claude.app/Contents/Resources/app.asar`). There is no
REST mirror of it. Anything short of speaking that websocket cannot learn a remote machine's name.

## 4. The fallback that works: this-device vs elsewhere

Local CLI transcripts record the bridge session id. Every remote-control-enabled session under
`~/.claude/projects/**/*.jsonl` contains `"bridgeSessionId":"cse_…"`:

```sh
grep -rhao '"bridgeSessionId":"cse_[A-Za-z0-9]*"' ~/.claude/projects | grep -o 'cse_[A-Za-z0-9]*' | sort -u
```

Measured against the live list: 48 of 85 matched → **this device**; 37 unmatched → **another
device**. Corroborated: the unmatched rows' `worktree_state` names a repo (`sportnak5/ff_model`) with
no checkout and no transcript directory anywhere on this machine.

**Match the key, never the bare id.** Grepping loose `cse_[A-Za-z0-9]*` over the transcript tree
inflates the local set: transcripts also contain ids that were merely *printed* into tool output
(this research run put two of them there itself), and those get counted as local. Only
`"bridgeSessionId":"cse_…"` — the CLI's own field — means "this session ran here". Parsing each
candidate line as JSON and taking a top-level `bridgeSessionId` key gives the same 48/37 split and is
cheap: only 57 of ~1,600 project dirs contain the string at all.

Known imprecision, do not over-trust it:

- 3 locally-recorded ids were absent from the API list (archived or deleted server-side) — harmless
  direction.
- 2 sessions titled with *this* machine's slug did not match a local transcript, i.e. the transcript
  was rotated or removed. They will label as remote. So the honest label is "not seen on this
  device", not "definitely another device".
- Only sessions with remote control on write a `bridgeSessionId` at all. A purely local session
  never reaches the API, so it is never in the list to mislabel.

**Weak name hint, not usable alone:** untitled sessions get a server-side default title of
`<machine-slug>-<word>-<word>` (`crawfords-m5-macbook-pro-local-vivid-spindle`,
`desktop-uj0sq9m-frolicking-popcorn`) — this machine's slug is its hostname lowercased. Only 5 of 85
rows had such a title and they disagreed with the transcript match in both directions. Treat as a
curiosity.

**Better than nothing for grouping:** `external_metadata.worktree_state` / `current_branches` are
keyed by repo slug (`sportnak5/ff_model`) with `branch`, `head_sha`, `is_dirty`, `reported_at`. Present
on 41 of 85. Not a device, but it tells the user *what* the remote session is working on.

---

## 5. Reproducing any of this

```sh
TOK=$(security find-generic-password -s "Claude Code-credentials" -w \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["claudeAiOauth"]["accessToken"])')
curl -s "https://api.anthropic.com/v1/code/sessions?limit=100" \
  -H "Authorization: Bearer $TOK" -H "anthropic-version: 2023-06-01" | python3 -m json.tool | head -60
```

The keychain read needs no password dialog from a terminal. How the app itself gets at the item —
signing, the codesign grant, the `Source` fallbacks — is already handled in
`src-tauri/src/ledger/usageapi.rs`; reuse `claude_token()` rather than re-deriving it.

The endpoint was found by grepping the desktop app's own bundle, which ships its API client in
plaintext — guessing endpoint names failed for a full round first:

```sh
LC_ALL=C strings -a /Applications/Claude.app/Contents/Resources/app.asar | grep -oE '.{40}v1/code/sessions.{40}'
```
