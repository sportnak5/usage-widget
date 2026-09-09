// The widget window. Readouts, not controls: the only interactions are the
// dial labels (re-scope the rows), the chevron (expand in place), ring hover
// (model tooltip), drag on the chrome, and double-click to open the ledger.
import { applySavedTheme } from "./shared/theme";
import { getHome, getSnapshot, onSchemeChanged, onSnapshot, openMain, openSettings, setWidgetExpanded, startDragging } from "./shared/bridge";
import { modelColor, paint, paceNote } from "./shared/color";
import { basename, esc, hhmm, setHome, short, tidy, tok, until } from "./shared/format";
import { grow, numText, ringArcs } from "./shared/ring";
import { applyScheme } from "./shared/schemes";
import type { Snapshot, WindowOut } from "./shared/types";

applySavedTheme();
applyScheme();

const R = 24.5;
const SHORT = ["Session", "Weekly", "Fable"];
const SCOPE = ["5h window", "this week", "Fable, this week"];

let snap: Snapshot | null = null;
let sel = 0;
let expanded = false;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

function dial(w: WindowOut, i: number): string {
  const segs = w.by_model.map((m) => ({
    color: modelColor(m.key as string),
    frac: m.share,
    label: `${short(m.key as string)} — ${m.pct === null ? m.share.toFixed(0) + "% of usage" : m.pct.toFixed(1) + "% of this limit"}`,
  }));
  const fill = w.pct === null ? 1 : Math.min(1, w.pct / 100);
  const title = w.pct === null
    ? `${w.label} — not calibrated. Run /usage in a Claude Code terminal session; the ledger window explains the rest.`
    : `${w.label} — ${w.pct.toFixed(0)}% used, ${paceNote(w.pace)}, resets ${until(w.reset)}`;
  return `<button class="dial" data-i="${i}" aria-selected="${i === sel}" style="${paint(w.ramp)}" title="${esc(title)}">
    <svg width="64" height="64" viewBox="-32 -32 64 64" aria-hidden="true">
      <g transform="rotate(-90)"><circle class="ring-bg" r="${R}" cx="0" cy="0"></circle>${ringArcs(R, segs, fill, null, w.pct === null)}</g>
      <g class="gauge-fill">${numText(w.pct, 5.5)}</g>
    </svg>
    <span class="dlabel"><span class="dname">${SHORT[i]}</span>
      <span class="dsub">${w.idle ? "idle" : "↺ " + hhmm(w.reset) + (w.boundary_known ? "" : "?")}</span></span>
  </button>`;
}

function renderDials(): void {
  if (!snap) return;
  $("dials").innerHTML = snap.windows.map(dial).join("");
  document.querySelectorAll<HTMLButtonElement>(".dial").forEach((b) =>
    b.addEventListener("click", (e) => { e.stopPropagation(); setWindow(Number(b.dataset.i)); }));
  grow($("dials"));
}

function renderRows(): void {
  if (!snap) return;
  const w = snap.windows[sel];
  const rows = w.by_session.slice(0, 5);
  const cap = `<span class="cap">Top sessions · ${SCOPE[sel]}</span>`;
  const body = rows.length === 0
    ? `<div class="wempty">Nothing in this window yet.</div>`
    : rows.map((e) => {
        const [sid, cwd] = e.key as [string, string];
        const pct = e.pct;
        const name = e.title || basename(cwd) || "untitled";
        const tip = `${name}\n${tidy(cwd)}\n${pct === null ? e.share.toFixed(0) + "% of usage in this window" : pct.toFixed(1) + "% of the " + SCOPE[sel] + " limit"}`;
        return `<div class="wr" style="${paint(pct === null ? null : pct / 100)}" title="${esc(tip)}" data-sid="${esc(sid)}">
          <i style="background:${modelColor(e.models[0].model)}"></i>
          <span class="n">${esc(name)}</span>
          <span class="t">${tok(e.raw)}</span>
          <span class="g"><i style="width:${Math.max(2, Math.min(100, pct ?? e.share)).toFixed(1)}%"></i></span>
          <span class="p gauge-color">${pct === null ? e.share.toFixed(0) + "%" : pct.toFixed(1) + "%"}</span>
        </div>`;
      }).join("");
  const note = snap.calibrated
    ? `<div class="wnote"><span>${esc(snap.plan)}</span><span>${hhmm(snap.generated_at)}</span></div>`
    : `<div class="wnote"><span>not calibrated</span><button id="calib">run /usage</button></div>`;
  $("wtop5").innerHTML = cap + body + note;
  $("wtop5").querySelector("#calib")?.addEventListener("click", (e) => { e.stopPropagation(); openSettings(); });
}

function setWindow(i: number): void {
  sel = i;
  document.querySelectorAll<HTMLElement>(".dial").forEach((d) => d.setAttribute("aria-selected", String(Number(d.dataset.i) === i)));
  renderRows();
}

async function fitWindow(): Promise<void> {
  // The OS window doesn't grow with the page; measure and ask Rust to resize.
  // Width too: system fonts differ per platform, so the card decides its size.
  await new Promise((r) => setTimeout(r, 20));
  const box = $("widget").getBoundingClientRect();
  await setWidgetExpanded(expanded, Math.ceil(box.width) + 2, Math.ceil(box.height) + 2);
}

function apply(s: Snapshot): void {
  snap = s;
  renderDials();
  renderRows();
  void fitWindow();
}

$("chev").addEventListener("click", async (e) => {
  e.stopPropagation();
  expanded = !expanded;
  const c = $("chev");
  c.setAttribute("aria-expanded", String(expanded));
  c.setAttribute("aria-label", expanded ? "Hide top sessions" : "Show top sessions");
  $("wtop5").hidden = !expanded;
  await fitWindow();
});

// Drag on chrome; double-click opens the ledger. Controls stop propagation.
$("widget").addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  const t = e.target as HTMLElement;
  if (t.closest("button, .arc, .wr")) return;
  if (e.detail >= 2) { void openMain(); return; }
  void startDragging();
});
$("wtop5").addEventListener("dblclick", () => void openMain());
document.addEventListener("contextmenu", (e) => e.preventDefault());

(async () => {
  try { setHome(await getHome()); } catch { /* fine */ }
  const s = await getSnapshot();
  if (s) apply(s);
  else $("dials").innerHTML = `<span class="wempty">Reading transcripts…</span>`;
  await onSnapshot(apply);
  await onSchemeChanged(applyScheme);
})();
