// The not-calibrated path.
//
// There are three ways to get real percentages, and they differ in accuracy,
// so this screen says so plainly instead of picking one for the user:
//
//   1. Turn on live readings — the endpoint is asked on every refresh, using
//      the login Claude Code already has, so it always matches the Usage tab.
//      Optional, and the only route that never goes stale.
//   2. Run `/usage` — Claude Code caches a reading we pick up. Exact when it
//      is fetched; extrapolated from local transcripts after that.
//   3. Type the percentages in — same as 2, by hand, for when Claude Code's
//      config file can't be reached from here at all.
//
// None of them is required to *use* the app: uncalibrated, the dials still
// show each window's breakdown, just not what share of the limit it is.
import { esc, when } from "./format";
import type { CacheDiag, Snapshot } from "./types";

/** Snapshots from before this field existed (and the dev fixture) have none. */
const UNKNOWN: CacheDiag = {
  path: "", projects_dir: "", reason: "no_key", detail: null, fetched_at: null, best_pct: null,
  live: false, connected: false, live_error: null, live_auth_failed: false,
};

export const diagOf = (s: Snapshot): CacheDiag => s.cache_diag ?? UNKNOWN;

const path = (p: string) => (p ? `<code>${esc(p)}</code>` : "the expected location");

/** Why we aren't calibrated, in terms of what the user can act on. */
export function why(d: CacheDiag): string {
  const pct = d.best_pct === null ? "" : `${d.best_pct.toFixed(0)}%`;
  switch (d.reason) {
    case "no_dir":
      return `Token Ledger can't work out where your home directory is, so it doesn't know where Claude Code keeps its config.`;
    case "no_file":
      return `No Claude Code config at ${path(d.path)}. If you run Claude Code inside WSL, a container, or with <code>CLAUDE_CONFIG_DIR</code> set, its config lives in that filesystem instead.`;
    case "unreadable":
      return `${path(d.path)} couldn't be read${d.detail ? `: ${esc(d.detail)}` : ""}.`;
    case "no_key":
      return `Found ${path(d.path)}, but Claude Code hasn't cached a Usage reading in it yet. Only <code>/usage</code> in a terminal session writes one — the desktop app's Usage tab doesn't.`;
    case "no_readings":
      return `Claude Code's cached reading${d.fetched_at ? ` from ${when(d.fetched_at)}` : ""} carries no percentages.`;
    case "too_low":
      return `The reading${d.fetched_at ? ` from ${when(d.fetched_at)}` : ""} tops out at ${pct}. That percentage is shown as-is, but it's too small to work a limit back out of — the Usage tab reports whole numbers, so under 10% the rounding error is wider than the limit it would imply. Until a window passes 10%, the dial can't move ahead of the last reading.`;
    case "no_usage":
      return `Claude Code reports ${pct} used, but no transcripts covering that window were found under ${path(d.projects_dir)}.`;
    default:
      return `Claude Code's cached Usage reading hasn't been applied yet.`;
  }
}

/** The ordered fix. First entry is the one that usually does it. */
export function steps(d: CacheDiag): string[] {
  const usage = `Run <code>/usage</code> <button type="button" class="btn tiny" data-copy="/usage">Copy</button>`;
  const back = `Come back here and press <b>Check again</b>.`;
  if (d.reason === "too_low") {
    return [
      `Keep using Claude Code until at least one window passes 10%.`,
      usage,
      back,
    ];
  }
  if (d.reason === "no_dir" || d.reason === "no_file" || d.reason === "no_usage") {
    return [
      `Point <b>Claude config dir</b> (below, under App) at the directory Claude Code actually uses. WSL and containers keep it inside their own filesystem, where a Windows or macOS app can't reach the default path.`,
      `Open Claude Code in a terminal there and ${usage.replace("Run ", "run ")}`,
      back,
    ];
  }
  return [
    `Open Claude Code in a terminal — the CLI, not the desktop app's Usage tab.`,
    usage,
    back,
  ];
}

/** What went wrong with a connected token, when something did. */
function liveNote(d: CacheDiag): string {
  if (!d.connected || !d.live_error) return "";
  const what = d.live_auth_failed
    ? `Live readings are on, but Claude Code's login couldn't be used — ${esc(d.live_error)} Token Ledger fell back to the cached reading.`
    : `Live readings are on but the last fetch didn't land — ${esc(d.live_error)} Token Ledger fell back to Claude Code's cached reading, and will try again on the next refresh.`;
  return `<p class="swhy warn">${what}</p>`;
}

/** The three ways in, most accurate first. Shown whether or not any of them
 *  has been tried, because the ordering is the point. */
export function routes(d: CacheDiag): string {
  const connected = d.connected && !d.live_error;
  return `<div class="sroutes">
    <div class="srhead">Three ways to get real percentages, most accurate first. Any one of them is enough.</div>
    <ol>
      <li>
        <b>Turn on live readings.</b> <span class="stag">best accuracy${connected ? " · in use" : ""}</span>
        Token Ledger asks Anthropic for your percentages on every refresh, so the dials show exactly what the Usage tab shows and never go stale.
        It reads the login Claude Code already keeps on this machine — read only, never copied or renewed. On macOS that is the Keychain, and the OS asks your permission the first time; on Windows and Linux it is a file in your Claude Code config directory. Costs no tokens: it reads the meter, it doesn't talk to a model.
        ${connected ? "" : `<div class="sact"><button type="button" class="btn primary" data-live>Turn on live readings</button><span class="shint">Nothing changes if it can't work — you'll get told why.</span></div>`}
      </li>
      <li>
        <b>Run <code>/usage</code> in a Claude Code terminal session.</b> <span class="stag">no setup</span>
        Claude Code caches the reading in its own config file and Token Ledger picks it up. Exact at the moment you run it; after that the dials extrapolate from this machine's transcripts, and drift until you run it again.
      </li>
      <li>
        <b>Type the percentages in.</b> <span class="stag">always available</span>
        Read them off the Usage tab and enter them by hand. As accurate as <code>/usage</code> at the moment you type them, and the way in when Claude Code's config file can't be reached from here at all — WSL, a container, a custom <code>CLAUDE_CONFIG_DIR</code>.
      </li>
    </ol>
  </div>`;
}

function body(d: CacheDiag): string {
  return `<p class="swhy">${why(d)}</p>
    ${liveNote(d)}
    <ol class="ssteps">${steps(d).map((t) => `<li>${t}</li>`).join("")}</ol>
    ${routes(d)}`;
}

/** The banner on the ledger window. */
export function setupCard(s: Snapshot): string {
  const d = diagOf(s);
  return `<div class="notice warn setup">
    <div class="stitle"><b>Not calibrated yet.</b> The dials show each window's breakdown, but not how much of the limit it is. Nothing here is required — the app works uncalibrated, it just can't tell you how close you are to a limit.</div>
    ${body(d)}
    <div class="sfoot">
      <button type="button" class="btn primary" data-recheck>Check again</button>
      <button type="button" class="btn" data-open-settings>Open settings to connect or type them in</button>
    </div>
  </div>`;
}

/** The same guidance inside the settings dialog, where manual entry lives. */
export function setupPanel(s: Snapshot | null, sourceNote: string): string {
  if (s && !s.calibrated) {
    return `<div class="notice warn setup">
      <div class="stitle"><b>Not calibrated yet.</b> Connecting live readings or running <code>/usage</code> both beat typing numbers in — try those first.</div>
      ${body(diagOf(s))}
      <div class="sfoot"><button type="button" class="btn primary" data-recheck>Check again</button></div>
    </div>`;
  }
  return `<div class="notice">${esc(sourceNote)}</div>`;
}

/** Wire the buttons the two renderers share. `root` may contain neither. */
export function bindSetup(
  root: HTMLElement,
  on: {
    recheck: (btn: HTMLButtonElement) => void;
    settings?: () => void;
    live?: (btn: HTMLButtonElement) => void;
  },
): void {
  if (on.live) {
    root.querySelectorAll<HTMLButtonElement>("[data-live]").forEach((b) =>
      b.addEventListener("click", () => on.live!(b)));
  }
  root.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((b) =>
    b.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(b.dataset.copy!);
        b.textContent = "Copied";
        setTimeout(() => { b.textContent = "Copy"; }, 1200);
      } catch { /* no clipboard permission; the text is right there to type */ }
    }));
  root.querySelectorAll<HTMLButtonElement>("[data-recheck]").forEach((b) =>
    b.addEventListener("click", () => on.recheck(b)));
  if (on.settings) {
    root.querySelectorAll<HTMLButtonElement>("[data-open-settings]").forEach((b) =>
      b.addEventListener("click", on.settings!));
  }
}
