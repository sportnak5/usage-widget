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
src-tauri/src/engine.rs owns index + settings; refresh, snapshot, calibrate
src-tauri/src/app.rs    Tauri: two windows, tray, refresh loop, commands
src-tauri/src/bin/cli.rs `ledger-cli` — the data layer end to end, no GUI
src/widget.ts           the frameless desktop widget (widget.html)
src/main.ts             the full ledger window + calibration dialog (index.html)
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
days old. When it is stale or missing, the ledger window
tells you and you can type the three percentages in yourself — a manual entry
always wins over an older cached one.

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

## The widget

- Three dials: ring segments by model (hover for the name and its share of
  that limit), the percentage in the middle colored by **burn rate**, not fill —
  green while you're under pace to hit the cap as the window closes, red well
  over.
- Dial labels re-scope the expandable top-5 list. Chevron expands it in place
  and resizes the window. Drag anywhere on the chrome; double-click opens the
  ledger. The tray icon toggles the widget; right-click for Refresh, Calibrate,
  Quit.

## How the numbers work

See `docs/HANDOFF.md`. The short version: ~70% of transcript usage rows are
duplicates (forks, resumes, compaction) and must be deduplicated on
`message.id`; raw tokens are dominated by cheap cache reads so everything is
cost-weighted at API list price; and since no limit or reset time exists on
disk, the percentage you read off the Usage tab anchors an *implied* limit
that the app then tracks on its own.

Percentages are estimates. When the three dials stop agreeing with the Usage
tab, re-enter them.
