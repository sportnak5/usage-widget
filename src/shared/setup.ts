// The not-calibrated path.
//
// Calibrating from Claude Code's own cached Usage reading is the whole design;
// typing three percentages by hand is the fallback for when that genuinely
// can't work. So everything here leads with the one command that fixes it —
// `/usage` in a terminal session — names the specific reason it hasn't
// happened yet, and only offers manual entry at the end.
import { esc, when } from "./format";
import type { CacheDiag, Snapshot } from "./types";

/** Snapshots from before this field existed (and the dev fixture) have none. */
const UNKNOWN: CacheDiag = {
  path: "", projects_dir: "", reason: "no_key", detail: null, fetched_at: null, best_pct: null,
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
      return `Claude Code's reading${d.fetched_at ? ` from ${when(d.fetched_at)}` : ""} tops out at ${pct}. The Usage tab reports whole numbers, so under 10% the rounding error is wider than the limit it would imply — anchoring on it would be worse than not anchoring at all.`;
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

function body(d: CacheDiag): string {
  return `<p class="swhy">${why(d)}</p>
    <ol class="ssteps">${steps(d).map((t) => `<li>${t}</li>`).join("")}</ol>`;
}

/** The banner on the ledger window. */
export function setupCard(s: Snapshot): string {
  const d = diagOf(s);
  return `<div class="notice warn setup">
    <div class="stitle"><b>Not calibrated yet.</b> The dials show each window's breakdown, but not how much of the limit it is.</div>
    ${body(d)}
    <div class="sfoot">
      <button type="button" class="btn primary" data-recheck>Check again</button>
      <button type="button" class="btn" data-open-settings>Enter the percentages by hand</button>
    </div>
  </div>`;
}

/** The same guidance inside the settings dialog, where manual entry lives. */
export function setupPanel(s: Snapshot | null, sourceNote: string): string {
  if (s && !s.calibrated) {
    return `<div class="notice warn setup">
      <div class="stitle"><b>Not calibrated yet.</b> Auto-calibration is the intended path — try it before typing anything.</div>
      ${body(diagOf(s))}
      <div class="sfoot"><button type="button" class="btn primary" data-recheck>Check again</button></div>
    </div>`;
  }
  return `<div class="notice">${esc(sourceNote)}</div>`;
}

/** Wire the buttons the two renderers share. `root` may contain neither. */
export function bindSetup(
  root: HTMLElement,
  on: { recheck: (btn: HTMLButtonElement) => void; settings?: () => void },
): void {
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
