# Token Ledger

Where your Claude Code tokens went. A desktop widget and ledger window for
macOS and Windows that reads Claude Code's local transcripts and shows the
same three limits the Usage tab does — the 5-hour session, the weekly cap,
and the weekly Fable cap — with each ring divided by the model or
conversation that filled it.

One Rust + web codebase (Tauri 2). No server, no credentials, no network:
the app reads `~/.claude/projects/**/*.jsonl` as the current user and keeps
only token counts, timestamps, and conversation titles in its index.

## Layout

```
src-tauri/src/ledger/   the data layer — no Tauri, testable, shared with the CLI
  record.rs             one transcript line → usage record or title
  index.rs              incremental per-file offsets, global dedup, retention
  pricing.rs            API list-price weights (cache reads are 95% of raw volume)
  windows.rs            session / weekly windows, calibration anchors, pace ramp
  snapshot.rs           group-by model / project / conversation → JSON for the UI
  activity.rs           tails the shown conversations: unread, and still working
src-tauri/src/engine.rs owns index + settings; refresh, snapshot, calibrate
src-tauri/src/app.rs    Tauri: two windows, tray, refresh loop, commands
src-tauri/src/bin/cli.rs `ledger-cli` — the data layer end to end, no GUI
src/widget.ts           the frameless desktop widget (widget.html)
src/main.ts             the full ledger window + calibration dialog (index.html)
src/shared/timeline.ts  usage over time: bucketing, the ribbon, the hover tooltip
src/shared/threads.ts   the conversation list both windows share: ranking, status marks
src/shared/             ring drawing, pace colors, formatting, the Rust bridge
docs/HANDOFF.md         the verified schema, dedup rule, calibration math, open questions
```

## Run it

Prerequisites: Rust (stable), Node 20+, and the Tauri platform deps
(Xcode Command Line Tools on macOS; on Windows, the MSVC build tools and
WebView2, which Windows 11 already has).

```bash
npm install
npm run tauri dev
```

Calibration is automatic when it can be: Claude Code caches its own Usage-tab
reading in `~/.claude.json` (`cachedUsageUtilization` — percentages and exact
reset times), and the app anchors to that whenever the reading is fresh and
large enough to be precise (integer percentages under 10% are skipped). That
cache is written when you run `/usage` inside a terminal `claude` session
(the desktop app's Usage tab reads live and does not write it), so it can be
days old. When it is stale or missing, the ledger window names the specific
reason — no config file at the path it looked at, no reading cached in it yet,
a reading too low to anchor on — and walks you through running `/usage` and
re-checking. Typing the three percentages in yourself is the fallback behind a
disclosure, for machines `/usage` can't reach (Claude Code inside WSL or a
container). A manual entry always wins over an older cached one.

Data layer only, no GUI:

```bash
cd src-tauri
cargo run --bin ledger-cli                 # scan + summary
cargo run --bin ledger-cli -- --json       # the snapshot the UI consumes
cargo run --bin ledger-cli -- --calibrate session=78,weekly=28,fable=53,resets-in=226,weekly-reset=Tue@01:00
cargo test
```

`ledger-cli` shares the app's settings and index, so a calibration done
there shows up in the widget.

## Build installers

```bash
npm run tauri build
```

macOS: `.app` and `.dmg` under `src-tauri/target/release/bundle/`. Windows:
`.msi` and NSIS `.exe` (build on Windows, or let the GitHub workflow do both).
The app is **not** sandboxed and cannot ship through the Mac App Store — it
has to read `~/.claude`. Distribute as a notarized `.dmg`.

## Usage over time

Above the dials, the ledger window plots cumulative % of the selected limit
against time, from the window opening to its reset — so the empty right-hand
part of the plot is the time you have left. The line is drawn as several
touching parallel bands, one per conversation (or model) that was spending in
that bucket, so its thickness reads as concurrency: one conversation is a plain
line, three at once is a ribbon. Bands peel out of the line when a conversation
starts and melt back in when it goes quiet.

Two dotted lines cross at *now*. To its left is the average rate so far. To its
right is the **budget line**: the rate that spends exactly what's left over the
time that's left, so it always arrives at 100% at the reset. One straight line
through both means you're spending exactly on budget; a kink down means you
spent early and have less per hour from here; a kink up means you have room.
Its color is the same burn-rate ramp as the dial's number.

Hover a bucket for its breakdown — the top 3 conversations *in that bucket*,
ranked the way the list's **Top / Recent** toggle is set, with the bands running
through it thickened to match. Hover a list row to isolate one band, click a dial to
re-scope, and change the bucket size to re-aggregate. The backend emits 15-minute
buckets for the session block and hourly ones for a week, always on local
calendar edges; coarser sizes are rolled up in the browser, and nothing finer
than the emitted grain is offered.

## The widget

- Three dials: ring segments by model (hover for the name and its share of
  that limit), the percentage in the middle colored by **burn rate**, not fill —
  green while you're under pace to hit the cap as the window closes, red well
  over.
- Dial labels re-scope the expandable top-5 list. Chevron expands it in place
  and resizes the window. Drag anywhere on the chrome; double-click opens the
  ledger. The tray icon toggles the widget; right-click for Refresh, Calibrate,
  Quit.
- **TOP | RECENT** over the list ranks conversations by weight or by last
  activity, the live side outlined. It is one setting in the backend, not a
  per-window preference, so flipping it in the widget flips it in the ledger
  and it survives a restart.
- How many rows each list shows is yours to set — Settings → App. The widget
  defaults to 5 and the ledger to 25; the backend keeps whichever is larger and
  each window takes its own cut of that one ranked list.

## Unread and still-working

Conversations carry two marks, in the widget list and the ledger's:

- A **spinner** — the agent is mid-turn: a tool call is out, or your prompt
  has no answer yet. Read from the tail of that conversation's transcript, and
  it stops after five quiet minutes, so a session killed mid-tool-call doesn't
  spin forever.
- A **dot** — the assistant has finished saying something since you last typed
  and since you last opened the row. Claude Code keeps no read receipts, so the
  app keeps its own: opening a row in the ledger clears it, and so does your
  next message in that conversation. Everything that happened before unread
  tracking started counts as read, so a first run isn't a wall of badges.

Only the conversations actually on screen are tailed — at most a few dozen
files, cached on `(size, mtime)` between refreshes — so this costs nothing on a
7,000-file history. Both marks are only as fresh as the refresh interval, which
is why it defaults to **15 seconds** rather than a minute: rescans read only the
bytes appended since the last one, and a spinner that takes a minute to appear
is worse than none.

## How the numbers work

See `docs/HANDOFF.md`. The short version: ~70% of transcript usage rows are
duplicates (forks, resumes, compaction) and must be deduplicated on
`message.id`; raw tokens are dominated by cheap cache reads so everything is
cost-weighted at API list price; and since no limit or reset time exists on
disk, the percentage you read off the Usage tab anchors an *implied* limit
that the app then tracks on its own.

Percentages are estimates. When the three dials stop agreeing with the Usage
tab, re-enter them.
