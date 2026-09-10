// The widget window. Readouts, not controls: the only interactions are the
// dial labels (re-scope the rows), the chevron (expand in place), the ranking
// toggle over the rows, ring hover (model tooltip), drag anywhere but those
// and the grip, and double-click to open the ledger.
import { applySavedTheme } from "./shared/theme";
import {
  getHome, getSettings, getSnapshot, onSchemeChanged, onSnapshot, openMain, openSettings,
  setThreadSort, setWidgetExpanded, startDragging, startResizing,
} from "./shared/bridge";
import { modelColor, paint, paceNote } from "./shared/color";
import { basename, esc, hhmm, setHome, short, tidy, tok, until } from "./shared/format";
import { grow, numText, ringArcs } from "./shared/ring";
import { applyScheme } from "./shared/schemes";
import { sortOf, sortToggle, statusMark, statusNote, widgetRows } from "./shared/threads";
import type { Snapshot, ThreadSort, WindowOut } from "./shared/types";

applySavedTheme();
applyScheme();

const R = 24.5;
const SHORT = ["Session", "Weekly", "Fable"];
const SCOPE = ["5h window", "this week", "Fable, this week"];

let snap: Snapshot | null = null;
let sel = 0;
let expanded = false;
// The card has no reflowing layout — nothing here would read well at a
// different aspect — so resizing zooms it whole rather than rearranging it.
let scale = 1;
const MIN_SCALE = 0.6;
const MAX_SCALE = 3;

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
  document.querySelectorAll<HTMLButtonElement>(".dial").forEach(armDial);
  grow($("dials"));
}

function renderRows(): void {
  if (!snap) return;
  const w = snap.windows[sel];
  // The backend keeps whichever window wants more, so the widget takes its
  // own cut of the same ranked list.
  const rows = w.by_session.slice(0, widgetRows(snap));
  const sort = sortOf(snap);
  const cap = `<span class="cap">Conversations · ${SCOPE[sel]}${sortToggle(sort)}</span>`;
  const body = rows.length === 0
    ? `<div class="wempty">Nothing in this window yet.</div>`
    : rows.map((e) => {
        const [sid, cwd] = e.key as [string, string];
        const pct = e.pct;
        const name = e.title || basename(cwd) || "untitled";
        const tip = `${name}\n${tidy(cwd)}\n${pct === null ? e.share.toFixed(0) + "% of usage in this window" : pct.toFixed(1) + "% of the " + SCOPE[sel] + " limit"}${statusNote(e)}`;
        return `<div class="wr" style="${paint(pct === null ? null : pct / 100)}" title="${esc(tip)}" data-sid="${esc(sid)}">
          ${statusMark(e)}<i style="background:${modelColor(e.models[0].model)}"></i>
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
  // The ranking lives in the backend, so the snapshot that comes back is also
  // the one the ledger window is handed: flipping it here flips it there.
  $("wtop5").querySelectorAll<HTMLButtonElement>("#wsort button").forEach((b) =>
    b.addEventListener("click", async (e) => {
      e.stopPropagation();
      const want = b.dataset.s as ThreadSort;
      if (want === sortOf(snap)) return;
      apply(await setThreadSort(want));
    }));
}

// A dial is a control and part of the card's surface at once: a plain click
// re-scopes the rows, but press-and-move drags the window and a double-press
// opens the ledger, so the gauges aren't dead zones for either gesture.
function armDial(b: HTMLButtonElement): void {
  b.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    if (e.detail >= 2) { void openMain(); return; }
    const x0 = e.screenX, y0 = e.screenY;
    let dragged = false;
    const off = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    const move = (m: MouseEvent) => {
      if (Math.abs(m.screenX - x0) + Math.abs(m.screenY - y0) < 4) return;
      dragged = true;
      off();
      void startDragging();
    };
    const up = () => { off(); if (!dragged) setWindow(Number(b.dataset.i)); };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
}

function setWindow(i: number): void {
  sel = i;
  document.querySelectorAll<HTMLElement>(".dial").forEach((d) => d.setAttribute("aria-selected", String(Number(d.dataset.i) === i)));
  renderRows();
}

function applyScale(): void {
  // `zoom`, not `transform`: it scales layout, so measuring the card afterwards
  // still gives the size the window has to be.
  document.documentElement.style.zoom = String(scale);
}

async function fitWindow(): Promise<void> {
  // The OS window doesn't grow with the page; measure and ask Rust to resize.
  // Width too: system fonts differ per platform, so the card decides its size.
  await new Promise((r) => setTimeout(r, 20));
  const box = $("widget").getBoundingClientRect();
  await setWidgetExpanded(expanded, Math.ceil(box.width) + 2, Math.ceil(box.height) + 2, scale);
}

// A drag on the grip resizes the OS window; the card then takes the zoom that
// makes it fill the new width, and the window snaps back to the card's box so
// no transparent margin is left over.
$("grip").addEventListener("mousedown", (e) => { e.stopPropagation(); void startResizing(); });

let settle: number | undefined;
window.addEventListener("resize", () => {
  window.clearTimeout(settle);
  settle = window.setTimeout(() => {
    const box = $("widget").getBoundingClientRect();
    if (box.width < 1) return;
    const next = Math.max(MIN_SCALE, Math.min(MAX_SCALE, scale * (window.innerWidth / box.width)));
    if (Math.abs(next - scale) < 0.01) return;
    scale = next;
    applyScale();
    void fitWindow();
  }, 140);
});

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
  if (t.closest("button, .grip")) return;
  if (e.detail >= 2) { void openMain(); return; }
  void startDragging();
});
document.addEventListener("contextmenu", (e) => e.preventDefault());

(async () => {
  try { setHome(await getHome()); } catch { /* fine */ }
  try {
    const g = (await getSettings()).widget;
    if (g) { scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, g.scale || 1)); applyScale(); }
  } catch { /* first run, or no backend */ }
  const s = await getSnapshot();
  if (s) apply(s);
  else $("dials").innerHTML = `<span class="wempty">Reading transcripts…</span>`;
  await onSnapshot(apply);
  await onSchemeChanged(applyScheme);
})();
