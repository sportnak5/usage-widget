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

/** Animate every arc and number under `root` from zero to its data value. */
export function grow(root: ParentNode, dur = 900): void {
  const quiet = still();
  root.querySelectorAll<SVGCircleElement>(".arc[data-len]").forEach((a) => {
    const set = () => a.setAttribute("stroke-dasharray", `${a.dataset.len} 999`);
    // A timer, not rAF: rAF is paused in a hidden window, and a widget that
    // was hidden when data arrived would show empty rings until it repainted.
    quiet ? set() : setTimeout(set, 30);
  });
  root.querySelectorAll<SVGTextElement>(".num[data-v]").forEach((t) => {
    const digits = t.querySelector<SVGTSpanElement>(".v");
    if (!digits) return;
    if (t.dataset.v === "") { digits.textContent = "—"; return; }
    const v = Number(t.dataset.v);
    if (quiet) { digits.textContent = String(Math.round(v)); return; }
    const t0 = performance.now();
    const step = (now: number) => {
      const k = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - k, 3);
      digits.textContent = String(Math.round(v * e));
      if (k < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}

/** `<text>` with a big number and a small unit, or an em dash when unknown. */
export const numText = (pct: number | null, y: number, unitY = y): string =>
  pct === null
    ? `<text class="num" x="0" y="${y}" text-anchor="middle" data-v=""><tspan class="v">—</tspan></text>`
    : `<text class="num" x="0" y="${y}" text-anchor="middle" data-v="${pct}"><tspan class="v">0</tspan><tspan class="pcs" dy="${unitY - y}">%</tspan></text>`;
