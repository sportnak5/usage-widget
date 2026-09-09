// The segmented ring, shared by the widget dials and the big ledger dials.
import { esc } from "./format";

export interface Segment { color: string; frac: number; label: string }

/** Arcs for one ring. `fill` is 0..1 of the circle the segments occupy together. */
export function ringArcs(r: number, segs: Segment[], fill: number, restLabel: string | null, muted = false): string {
  const C = 2 * Math.PI * r;
  const total = segs.reduce((a, s) => a + s.frac, 0) || 1;
  let at = 0;
  const arcs = segs.map((s) => {
    const len = (s.frac / total) * Math.min(1, fill) * C;
    const a = `<circle class="arc${muted ? " muted" : ""}" r="${r}" cx="0" cy="0" stroke="${s.color}"
      stroke-dasharray="0 999" data-len="${len.toFixed(2)}" stroke-dashoffset="${(-at).toFixed(2)}"><title>${esc(s.label)}</title></circle>`;
    at += len;
    return a;
  });
  const restLen = Math.min(1, fill) * C - at;
  if (restLabel && restLen > 1) {
    arcs.push(`<circle class="arc" r="${r}" cx="0" cy="0" stroke="var(--m-rest)"
      stroke-dasharray="0 999" data-len="${restLen.toFixed(2)}" stroke-dashoffset="${(-at).toFixed(2)}"><title>${esc(restLabel)}</title></circle>`);
  }
  return arcs.join("");
}

const still = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

interface Prev { arcs: Record<string, [string, string]>; nums: Record<string, number> }

// Where each re-render leaves its rings, so the next one can start there
// instead of snapping back to zero. Keyed by container element.
const lastState = new WeakMap<ParentNode, Prev>();

/** Stable key for an arc: which ring it belongs to, its colour, and its rank among same-coloured arcs. */
function keyOf(root: ParentNode, el: Element, seen: Map<string, number>): string {
  const svgs = Array.from(root.querySelectorAll("svg"));
  const ring = svgs.indexOf(el.closest("svg") as SVGSVGElement);
  const base = `${ring}:${el.getAttribute("stroke") ?? ""}`;
  const n = seen.get(base) ?? 0;
  seen.set(base, n + 1);
  return `${base}:${n}`;
}

function numKey(root: ParentNode, el: Element): string {
  const svgs = Array.from(root.querySelectorAll("svg"));
  return String(svgs.indexOf(el.closest("svg") as SVGSVGElement));
}

/**
 * Animate every arc and number under `root` to its data value — from where the
 * previous render of this container left them, so a refresh nudges the ring
 * forward instead of redrawing it from zero. First render starts at zero.
 */
export function grow(root: ParentNode, dur = 900): void {
  const quiet = still();
  const prev = lastState.get(root);
  const next: Prev = { arcs: {}, nums: {} };
  const seen = new Map<string, number>();

  root.querySelectorAll<SVGCircleElement>(".arc[data-len]").forEach((a) => {
    const key = keyOf(root, a, seen);
    const len = a.dataset.len ?? "0";
    const off = a.getAttribute("stroke-dashoffset") ?? "0";
    next.arcs[key] = [len, off];
    const from = prev?.arcs[key];
    if (from && !quiet) {
      // Start where the last render ended, then transition to the new value.
      a.setAttribute("stroke-dasharray", `${from[0]} 999`);
      a.setAttribute("stroke-dashoffset", from[1]);
    }
    const set = () => {
      a.setAttribute("stroke-dasharray", `${len} 999`);
      a.setAttribute("stroke-dashoffset", off);
    };
    // A timer, not rAF: rAF is paused in a hidden window, and a widget that
    // was hidden when data arrived would show empty rings until it repainted.
    quiet ? set() : setTimeout(set, 30);
  });

  root.querySelectorAll<SVGTextElement>(".num[data-v]").forEach((t) => {
    const digits = t.querySelector<SVGTSpanElement>(".v");
    if (!digits) return;
    const key = numKey(root, t);
    if (t.dataset.v === "") { digits.textContent = "\u2014"; return; }
    const v = Number(t.dataset.v);
    next.nums[key] = v;
    if (quiet) { digits.textContent = String(Math.round(v)); return; }
    const from = prev?.nums[key] ?? 0;
    if (Math.round(from) === Math.round(v)) { digits.textContent = String(Math.round(v)); return; }
    digits.textContent = String(Math.round(from));
    const t0 = performance.now();
    const step = (now: number) => {
      const k = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - k, 3);
      digits.textContent = String(Math.round(from + (v - from) * e));
      if (k < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });

  lastState.set(root, next);
}

/** `<text>` with a big number and a small unit, or an em dash when unknown. */
export const numText = (pct: number | null, y: number, unitY = y): string =>
  pct === null
    ? `<text class="num" x="0" y="${y}" text-anchor="middle" data-v=""><tspan class="v">—</tspan></text>`
    : `<text class="num" x="0" y="${y}" text-anchor="middle" data-v="${pct}"><tspan class="v">0</tspan><tspan class="pcs" dy="${unitY - y}">%</tspan></text>`;
