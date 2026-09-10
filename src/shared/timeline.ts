// Usage over time: one cumulative line, drawn as several touching parallel
// bands wherever more than one conversation (or model) was spending at once.
// The line's thickness is therefore concurrency, and the ribbon stays centred
// on the true total.
//
// The backend emits one fixed grain per window (snapshot.rs — 15 minutes for
// the session block, hourly for a week); everything coarser is rolled up here,
// on local calendar boundaries.
import { convColor, modelColor, paint } from "./color";
import { esc, short, tok, until, usd } from "./format";
import { REST_KEY } from "./types";
import type { BucketId, Entry, Series, ThreadSort, WindowOut } from "./types";

export interface View {
  mode: "by_model" | "by_session";
  bucket: BucketId;
  hoverKey: string | null;
  hoverBucket: number | null;
  /** `snapshot.generated_at` — the last bucket's end, and the "now" marker. */
  now: number;
  /** The chart's pixel size. The viewBox is 1:1 with it, so the chart grows
      with the window instead of scaling its type up along with it. */
  size: [number, number];
  /** The list's own Top / Recent toggle. `by_session` arrives already in this
      order, so the bands follow it for nothing; the tooltip has to be told. */
  sort: ThreadSort;
}

export const BUCKETS: { id: BucketId; label: string; ms: number }[] = [
  { id: "15m", label: "Every 15 minutes", ms: 9e5 },
  { id: "30m", label: "Every 30 minutes", ms: 1.8e6 },
  { id: "hour", label: "Hourly", ms: 3.6e6 },
  { id: "6h", label: "Every 6 hours", ms: 21.6e6 },
  { id: "day", label: "Daily", ms: 86.4e6 },
  { id: "week", label: "Weekly", ms: 604.8e6 },
  { id: "month", label: "Monthly", ms: 2592e6 },
];

const grainMs = (w: WindowOut): number =>
  BUCKETS.find((b) => b.id === (w.series?.bucket ?? "hour"))?.ms ?? 3.6e6;

/** Nothing finer than what the backend emitted for this window, and nothing so
    coarse it can't show a shape. Never empty: the finest available survives. */
export function allowedBuckets(w: WindowOut): typeof BUCKETS {
  const span = Date.parse(w.reset) - Date.parse(w.start);
  const grain = grainMs(w);
  const ok = BUCKETS.filter((b) => b.ms >= grain && b.ms <= span / 3);
  return ok.length ? ok : BUCKETS.filter((b) => b.ms >= grain).slice(0, 1);
}

export const bucketOf = (w: WindowOut, id: BucketId): BucketId => {
  const ok = allowedBuckets(w);
  return (ok.find((b) => b.id === id) ?? ok[ok.length - 1]).id;
};

export const bucketLabel = (w: WindowOut, id: BucketId): string =>
  BUCKETS.find((b) => b.id === bucketOf(w, id))!.label;

/** The default granularity: the 5-hour block at its finest, a week by day. */
export const defaultBucket = (w: WindowOut): BucketId =>
  bucketOf(w, w.id === "session" ? "15m" : "day");

// ---------- keys ----------

const REST_NAME = "everything else";

interface Key { key: string; name: string; color: string; cost: number }

/** Every conversation (or model) the window kept, by key. The ribbon can only
    draw nine, but the tooltip names anything in here — which is nearly all of
    it: folding at the top 8 buried a fifth of a typical week under
    "everything else" even though the titles were right there. */
function named(w: WindowOut, mode: View["mode"]): Map<string, { name: string; rank: number }> {
  const m = new Map<string, { name: string; rank: number }>();
  (w[mode] as Entry[]).forEach((e, rank) => {
    m.set(mode === "by_model" ? (e.key as string) : (e.key as [string, string])[0], {
      name: mode === "by_model" ? short(e.key as string) : e.title || "untitled session",
      rank,
    });
  });
  return m;
}

/** Top 8 by weighted cost, in the order the ribbon and the list both use — a
    stable order keeps a band from jumping lanes when a neighbour starts or
    stops — plus one "everything else" band for the remainder. */
function keys(w: WindowOut, mode: View["mode"]): Key[] {
  const out: Key[] = w[mode].slice(0, 8).map((e: Entry, i) => ({
    key: mode === "by_model" ? (e.key as string) : (e.key as [string, string])[0],
    name: mode === "by_model" ? short(e.key as string) : e.title || "untitled session",
    color: mode === "by_model" ? modelColor(e.key as string) : convColor(i),
    cost: e.cost,
  }));
  // Anything the top 8 leaves over, including snapshot.rs's own per-bucket
  // tail. Present whenever there is a ninth entry at all, so a band never
  // appears with nothing to name it.
  const rest = Math.max(0, w.total_cost - out.reduce((a, k) => a + k.cost, 0));
  if (w[mode].length > 8 || rest > 0) {
    out.push({ key: REST_KEY, name: REST_NAME, color: "var(--m-rest)", cost: rest });
  }
  return out;
}

// ---------- bucketing ----------

interface Bucket {
  t: number; end: number;
  /** Every conversation the backend named in this bucket — what the tooltip
      ranks. Kept separate from the band fold so a name is never lost. */
  by: Map<string, { cost: number; raw: number }>;
  /** The same costs folded onto the nine keys the ribbon can actually draw. */
  band: Map<string, number>;
  cost: number; raw: number;
}

/** The next bucket boundary after `t`, on local calendar lines. Never
    `t - t % ms`: epoch-modulo lands day, week and month edges on UTC midnight,
    which shows two buckets labelled with the same day in any offset timezone. */
function nextEdge(t: number, bucket: BucketId, weekAnchor: number): number {
  const d = new Date(t);
  const mins = bucket === "15m" ? 15 : bucket === "30m" ? 30 : 0;
  if (mins) { d.setSeconds(0, 0); d.setMinutes(Math.floor(d.getMinutes() / mins) * mins + mins); return d.getTime(); }
  if (bucket === "hour") { d.setMinutes(0, 0, 0); d.setHours(d.getHours() + 1); return d.getTime(); }
  if (bucket === "6h") { d.setMinutes(0, 0, 0); d.setHours(Math.floor(d.getHours() / 6) * 6 + 6); return d.getTime(); }
  if (bucket === "day") { d.setHours(0, 0, 0, 0); d.setDate(d.getDate() + 1); return d.getTime(); }
  if (bucket === "week") {
    // Anchored to the window's own reset weekday and hour, the way windows.rs does it.
    const a = new Date(weekAnchor);
    d.setHours(a.getHours(), a.getMinutes(), 0, 0);
    d.setDate(d.getDate() + ((a.getDay() - d.getDay() + 7) % 7 || 7));
    return d.getTime();
  }
  d.setHours(0, 0, 0, 0); d.setDate(1); d.setMonth(d.getMonth() + 1);
  return d.getTime();
}

/** Roll the emitted buckets up to `bucket`, folding every key outside `known`
    into `REST_KEY`. Gaps stay flat: an empty bucket is empty, never
    interpolated. */
function rollUp(series: Series, view: View, known: Set<string>): Bucket[] {
  const start = Date.parse(series.start);
  const now = Math.max(view.now, start);
  if (now <= start) return [];
  const edges = [start];
  for (let t = nextEdge(start, view.bucket, start); t < now && edges.length < 2000; t = nextEdge(t, view.bucket, start)) {
    edges.push(t);
  }
  const out: Bucket[] = edges.map((t, i) => ({ t, end: edges[i + 1] ?? now, by: new Map(), band: new Map(), cost: 0, raw: 0 }));

  let i = 0;
  for (const src of series.buckets) {
    const t = Date.parse(src.t);
    while (i < out.length - 1 && t >= out[i + 1].t) i++;
    const b = out[i];
    for (const [k, cost, raw] of view.mode === "by_model" ? src.by_model : src.by_session) {
      const key = known.has(k) ? k : REST_KEY;
      const e = b.by.get(key) ?? { cost: 0, raw: 0 };
      e.cost += cost; e.raw += raw;
      b.by.set(key, e);
      b.cost += cost; b.raw += raw;
    }
  }
  return out;
}

// ---------- geometry ----------

interface Box { x0: number; x1: number; yTop: number; yBase: number }

/** The plot inside a `w × h` chart: room for the axis labels on the left, the
    trend label above, and the tick and reset labels below. */
const boxOf = ([w, h]: [number, number]): Box =>
  ({ x0: 56, x1: Math.max(140, w - 32), yTop: 26, yBase: Math.max(80, h - 68) });

export const BAND = 3;
export const BAND_HOVER = 5;
const FADE = 46;

type Pt = [number, number];

/** Catmull-Rom through the anchors, as cubic béziers. A polyline kinks
    visibly at every split; a spline does not. */
function spline(pts: Pt[]): string {
  if (pts.length < 2) return "";
  let d = `M${pts[0][0].toFixed(1)},${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)], p1 = pts[i], p2 = pts[i + 1], p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1: Pt = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2: Pt = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    d += ` C${c1[0].toFixed(1)},${c1[1].toFixed(1)} ${c2[0].toFixed(1)},${c2[1].toFixed(1)} ${p2[0].toFixed(1)},${p2[1].toFixed(1)}`;
  }
  return d;
}

let gradSeq = 0;

/** One run of one key: a spline through its offset anchors, stroked with a
    gradient that fades in and out over `FADE` px so the band peels out of the
    single line and melts back into it. Each run carries the bucket range it
    covers, so hovering a bucket can thicken exactly the runs crossing it. */
function bands(P: Pt[], active: string[][], ks: Key[], hover: string | null): string {
  if (P.length < 2) return "";
  const seq = ++gradSeq;
  const defs: string[] = [];
  const paths: string[] = [];
  const lead = Math.min(FADE, (P[P.length - 1][0] - P[0][0]) / 8);
  // Where the centre line sits at an arbitrary x, for the fade anchors.
  const onTotal = (x: number): Pt => {
    for (let i = 1; i < P.length; i++) {
      if (x <= P[i][0]) {
        const t = Math.max(0, (x - P[i - 1][0]) / Math.max(1, P[i][0] - P[i - 1][0]));
        return [x, P[i - 1][1] + (P[i][1] - P[i - 1][1]) * t];
      }
    }
    return [x, P[P.length - 1][1]];
  };
  const offset = (i: number, key: string): number => {
    const on = active[i];
    return (on.indexOf(key) - (on.length - 1) / 2) * BAND;
  };

  ks.forEach((k, ki) => {
    let i = 0;
    while (i < active.length) {
      if (!active[i].includes(k.key)) { i++; continue; }
      let e = i;
      while (e + 1 < active.length && active[e + 1].includes(k.key)) e++;
      const pts: Pt[] = [];
      if (i > 0) pts.push(onTotal(Math.max(P[0][0], P[i][0] - lead)));
      for (let j = i; j <= e; j++) pts.push([P[j][0], P[j][1] + offset(j, k.key)]);
      const tail: Pt = [P[e + 1][0], P[e + 1][1] + offset(e, k.key)];
      pts.push(tail);
      const last = e + 1 === active.length;
      if (!last) pts.push(onTotal(Math.min(P[P.length - 1][0], tail[0] + lead)));

      const id = `tlb${seq}-${ki}-${i}`;
      const x0 = pts[0][0], x1 = pts[pts.length - 1][0], span = Math.max(1, x1 - x0);
      const stop = (o: string, op: number) => `<stop offset="${o}" style="stop-color:${k.color};stop-opacity:${op}"/>`;
      defs.push(`<linearGradient id="${id}" gradientUnits="userSpaceOnUse" x1="${x0.toFixed(1)}" x2="${x1.toFixed(1)}" y1="0" y2="0">`
        + stop("0", i > 0 ? 0 : 1)
        + stop((i > 0 ? lead / span : 0).toFixed(3), 1)
        + stop((1 - (last ? 0 : lead / span)).toFixed(3), 1)
        + stop("1", last ? 1 : 0)
        + `</linearGradient>`);
      paths.push(`<path class="band" data-k="${esc(k.key)}" data-b0="${i}" data-b1="${e}" d="${spline(pts)}"
        fill="none" stroke="url(#${id})" stroke-width="${BAND}" stroke-linejoin="round" stroke-linecap="round"
        opacity="${!hover || hover === k.key ? 1 : 0.14}"><title>${esc(k.name)}</title></path>`);
      i = e + 1;
    }
  });
  return `<defs>${defs.join("")}</defs>${paths.join("")}`;
}

// ---------- the panel ----------

const clock = (t: number): string => new Date(t).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

function tickLabel(t: number, bucket: BucketId): string {
  const d = new Date(t);
  if (bucket === "15m" || bucket === "30m") return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (bucket === "hour") return d.toLocaleTimeString([], { hour: "numeric" });
  if (bucket === "6h") return d.toLocaleString([], { weekday: "short", hour: "numeric" });
  if (bucket === "day") return `${d.toLocaleDateString([], { weekday: "short" })} ${d.getDate()}`;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}


/** How many conversations the tooltip names before the rest becomes a count. */
const TOOLTIP_ROWS = 3;

/** IBM Plex Mono's advance at the 9.5px axis size — every glyph is this wide,
    so a label's width is known without measuring it. */
const MONO_ADV = 5.72;
const TIP_NAME_FONT = '11px "IBM Plex Sans", system-ui, sans-serif';
const TIP_NUM_FONT = '10px "IBM Plex Mono", ui-monospace, monospace';

// Conversation titles are proportional text, so guessing an average advance
// puts the name under the figure on its right. Measure instead.
let measurer: CanvasRenderingContext2D | null | undefined;
function textWidth(s: string, font: string): number {
  if (measurer === undefined) measurer = document.createElement("canvas").getContext("2d");
  if (!measurer) return s.length * 6;
  measurer.font = font;
  return measurer.measureText(s).width;
}

// The font to measure in has to be the font the browser will actually paint,
// which is a question only the stylesheet can answer — a hard-coded guess here
// was wrong for a year's worth of CSS specificity. Read it off a live node once
// one exists; the constants above only cover the very first tooltip.
const fontCache = new Map<string, string>();
function fontOf(cls: string, fallback: string): string {
  const hit = fontCache.get(cls);
  if (hit) return hit;
  const el = document.querySelector(`.tlsvg .${cls}`);
  if (!el) return fallback;
  const cs = getComputedStyle(el);
  const font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
  fontCache.set(cls, font);
  return font;
}

/** `s` trimmed with an ellipsis until it actually fits `max` px. */
function fit(s: string, max: number, font: string): string {
  if (textWidth(s, font) <= max) return s;
  let lo = 0, hi = s.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (textWidth(s.slice(0, mid) + "…", font) <= max) lo = mid; else hi = mid - 1;
  }
  return lo > 0 ? s.slice(0, lo) + "…" : "";
}

const narrow = (): boolean =>
  typeof window !== "undefined" && window.matchMedia("(max-width:700px)").matches;

interface Plot {
  box: Box;
  ks: Key[];
  names: Map<string, { name: string; rank: number }>;
  buckets: Bucket[];
  /** Anchors on the centre line, one per bucket edge plus one at now. */
  P: Pt[];
  active: string[][];
  /** Cumulative axis value at each anchor — % of limit, or weighted dollars. */
  vals: number[];
  X: (t: number) => number;
  Y: (v: number) => number;
  yMax: number;
  pct: boolean;
  /** The axis value at now: the dial's percentage, or weighted dollars. */
  total: number;
}

function plot(w: WindowOut, view: View): Plot | null {
  if (!w.series) return null;
  const ks = keys(w, view.mode);
  const names = named(w, view.mode);
  const buckets = rollUp(w.series, view, new Set(names.keys()));
  if (buckets.length === 0) return null;

  // Only now collapse onto the nine keys the ribbon draws. `by` keeps the
  // detail so the tooltip can rank this bucket on its own terms.
  const bandKeys = new Set(ks.map((k) => k.key));
  for (const b of buckets) {
    for (const [k, v] of b.by) {
      const bk = bandKeys.has(k) ? k : REST_KEY;
      b.band.set(bk, (b.band.get(bk) ?? 0) + v.cost);
    }
  }

  const box = boxOf(view.size);
  const start = Date.parse(w.start), reset = Date.parse(w.reset);
  const span = Math.max(1, reset - start);
  const X = (t: number) => box.x0 + ((t - start) / span) * (box.x1 - box.x0);

  // Cumulative weighted cost per edge, then onto the axis. Calibrated, the axis
  // is % of the limit scaled so the ribbon's end is exactly the dial's number:
  // a live anchor starts the dial at Anthropic's reading, not at zero here.
  const cum = [0];
  let at = 0;
  for (const b of buckets) { at += b.cost; cum.push(at); }
  const pct = w.pct !== null;
  const toAxis = (c: number) => (pct ? (at > 0 ? (c / at) * w.pct! : 0) : c);
  const vals = cum.map(toAxis);
  const shown = vals[vals.length - 1];
  const yMax = pct
    ? Math.min(260, Math.max(100, Math.ceil(shown / 25) * 25))
    : Math.max(0.01, Math.max(shown, Math.min(shown / Math.max(0.02, w.elapsed_frac), shown * 3)) * 1.05);
  const Y = (v: number) => box.yBase - (v / yMax) * (box.yBase - box.yTop);

  const P: Pt[] = buckets.map((b, i) => [X(b.t), Y(vals[i])] as Pt);
  P.push([X(buckets[buckets.length - 1].end), Y(shown)]);
  const active = buckets.map((b) => ks.filter((k) => (b.band.get(k.key) ?? 0) > 0).map((k) => k.key));

  return { box, ks, names, buckets, P, active, vals, X, Y, yMax, pct, total: shown };
}

/** Which bucket a pointer at viewBox x `vx` is over, or null off the plot. */
export function hitBucket(w: WindowOut, view: View, vx: number): number | null {
  const p = plot(w, view);
  if (!p || vx < p.box.x0 || vx > p.box.x1) return null;
  for (let i = 0; i < p.buckets.length; i++) {
    if (vx >= p.X(p.buckets[i].t) && vx <= p.X(p.buckets[i].end)) return i;
  }
  return null;
}

export function bucketCount(w: WindowOut, view: View): number {
  const p = plot(w, view);
  return p ? p.buckets.length : 0;
}

/** The hover column, guide and tooltip — the contents of the chart's `#tlGuide`
    group, so a pointer move repaints only that and never the ribbon. */
export function guide(w: WindowOut, view: View): string {
  const p = plot(w, view);
  const i = view.hoverBucket;
  if (!p || i === null) return "";
  const b = p.buckets[i];
  if (!b) return "";
  const box = p.box;
  const gx = p.X((b.t + b.end) / 2);
  // The top 3 in *this* bucket. Under "Top" that means this bucket's own spend
  // — not the window-wide order, which buried whatever actually ran at a quiet
  // hour. Under "Recent" it means the list's own recency order, which is how
  // `by_session` already arrives, narrowed to what ran in this bucket.
  const rank = (k: string) => p.names.get(k)?.rank ?? Number.MAX_SAFE_INTEGER;
  const byRecency = view.sort === "recent" && view.mode === "by_session";
  const ranked = [...b.by.entries()].sort((a, c) =>
    byRecency ? rank(a[0]) - rank(c[0]) || c[1].cost - a[1].cost : c[1].cost - a[1].cost);
  const rows = ranked.slice(0, TOOLTIP_ROWS);
  const more = ranked.length - rows.length;
  const moreLabel = byRecency ? `+${ranked.length - rows.length} older` : `+${ranked.length - rows.length} more`;
  const moreCost = ranked.slice(TOOLTIP_ROWS).reduce((a, [, v]) => a + v.cost, 0);
  const tight = narrow();
  const tw = tight ? 200 : 300;
  const th = 46 + Math.max(1, rows.length) * 19 + (more > 0 ? 16 : 0);
  // Prefer the right of the guide; flip only when there is room on the left.
  const nowX = p.X(view.now);
  const covers = gx + 16 < nowX + 62 && gx + 16 + tw > nowX - 4;
  const flip = (gx + tw + 24 > box.x1 || covers) && gx - tw - 16 >= box.x0;
  const tx = Math.max(box.x0, Math.min(flip ? gx - tw - 16 : gx + 16, box.x1 - tw));
  const ty = Math.max(box.yTop, Math.min(box.yBase - th, p.Y(p.vals[i + 1]) - th / 2));
  const head = b.end - b.t <= 21.6e6
    ? `${clock(b.t)} – ${clock(b.end)}`
    : new Date(b.t).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
  const name = (k: string) => p.names.get(k)?.name ?? (k === REST_KEY ? REST_NAME : k);
  // A swatch means "this is the band you can see"; anything without one is
  // inside the "everything else" band, so it carries that band's grey.
  const color = (k: string) => p.ks.find((x) => x.key === k)?.color ?? "var(--m-rest)";
  const amount = (raw: number, cost: number) => (tight ? usd(cost) : `${tok(raw)} · ${usd(cost)}`);
  // The name gets whatever the row has left once its own figure is measured.
  const numFont = fontOf("ttr", TIP_NUM_FONT), nameFont = fontOf("ttn", TIP_NAME_FONT);
  const room = (amt: string) => Math.max(24, tw - 46 - textWidth(amt, numFont));

  return `<rect class="tlcol" x="${p.X(b.t).toFixed(1)}" width="${Math.max(1, p.X(b.end) - p.X(b.t)).toFixed(1)}" y="${box.yTop}" height="${box.yBase - box.yTop}"/>
    <line class="tlguide" x1="${gx.toFixed(1)}" x2="${gx.toFixed(1)}" y1="${box.yTop}" y2="${box.yBase}"/>
    <circle class="tldot" cx="${gx.toFixed(1)}" cy="${p.Y(p.vals[i + 1]).toFixed(1)}" r="3.5"/>
    <rect class="tltip" x="${tx.toFixed(1)}" y="${ty.toFixed(1)}" width="${tw}" height="${th}" rx="8"/>
    <text class="tth" x="${(tx + 12).toFixed(1)}" y="${(ty + 18).toFixed(1)}">${esc(head)}</text>
    <text class="ttv" x="${(tx + tw - 12).toFixed(1)}" y="${(ty + 18).toFixed(1)}" text-anchor="end">${esc(amount(b.raw, b.cost))}</text>
    <line class="tlrule" x1="${(tx + 10).toFixed(1)}" x2="${(tx + tw - 10).toFixed(1)}" y1="${(ty + 26).toFixed(1)}" y2="${(ty + 26).toFixed(1)}"/>
    ${rows.length === 0
      ? `<text class="tte" x="${(tx + 12).toFixed(1)}" y="${(ty + 45).toFixed(1)}">nothing in this bucket</text>`
      : rows.map(([k, v], j) => `<rect x="${(tx + 12).toFixed(1)}" y="${(ty + 37 + j * 19).toFixed(1)}" width="8" height="8" rx="2" fill="${color(k)}"/>
        <text class="ttn" x="${(tx + 26).toFixed(1)}" y="${(ty + 45 + j * 19).toFixed(1)}">${esc(fit(name(k), room(amount(v.raw, v.cost)), nameFont))}</text>
        <text class="ttr" x="${(tx + tw - 12).toFixed(1)}" y="${(ty + 45 + j * 19).toFixed(1)}" text-anchor="end">${esc(amount(v.raw, v.cost))}</text>`).join("")}
    ${more <= 0 ? "" : `<text class="ttm" x="${(tx + 26).toFixed(1)}" y="${(ty + 58 + (rows.length - 1) * 19).toFixed(1)}">${esc(moreLabel)}</text>
      <text class="ttm" x="${(tx + tw - 12).toFixed(1)}" y="${(ty + 58 + (rows.length - 1) * 19).toFixed(1)}" text-anchor="end">${esc(usd(moreCost))}</text>`}`;
}

const frame = (view: View, body: string): string =>
  `<svg class="tlsvg" viewBox="0 0 ${view.size[0]} ${view.size[1]}" width="${view.size[0]}" height="${view.size[1]}">${body}</svg>`;

/** Axes only, with a line of copy where the ribbon would be. */
function empty(view: View, note: string): string {
  const b = boxOf(view.size);
  return frame(view, `<line class="tlaxis" x1="${b.x0}" x2="${b.x1}" y1="${b.yBase}" y2="${b.yBase}"/>
    <line class="tlaxis" x1="${b.x0}" x2="${b.x0}" y1="${b.yTop}" y2="${b.yBase}"/>
    <text class="tte" x="${(b.x0 + b.x1) / 2}" y="${(b.yTop + b.yBase) / 2}" text-anchor="middle">${esc(note)}</text>`);
}

/**
 * The two dotted segments.
 *
 * Left of now: the average rate so far — window open to the live percentage.
 * Right of now: the rate that spends exactly what is left over the time that is
 * left, so it always arrives at 100% at the reset. One straight line through
 * both means you are spending exactly on budget; a down-kink means you spent
 * early and have less per hour from here; an up-kink means you have room.
 *
 * Uncalibrated there is no limit to divide up, so the right-hand segment falls
 * back to carrying the average rate onward.
 */
function trend(w: WindowOut, view: View, p: Plot): string {
  const box = p.box;
  const nowX = p.X(view.now), nowY = p.Y(p.total);
  const resetX = p.X(Date.parse(w.reset));
  const avg = `<line class="tltrend" x1="${box.x0}" x2="${nowX.toFixed(1)}" y1="${p.Y(0).toFixed(1)}" y2="${nowY.toFixed(1)}"/>`;
  if (!p.pct) {
    const projected = p.total / Math.max(0.02, w.elapsed_frac);
    return avg + `<line class="tltrend" x1="${nowX.toFixed(1)}" x2="${resetX.toFixed(1)}" y1="${nowY.toFixed(1)}" y2="${p.Y(Math.min(projected, p.yMax)).toFixed(1)}"/>
      <text class="ax" x="${box.x1}" y="14" text-anchor="end">at this rate ${esc(usd(projected))} by reset</text>`;
  }
  const left = Math.max(0, 100 - p.total);
  const endY = p.Y(Math.min(Math.max(p.total, 100), p.yMax));
  const label = left > 0
    ? `budget line · ${left.toFixed(0)}% left over ${esc(until(w.reset, view.now).replace(/^in /, ""))}`
    : `over the limit · no headroom left`;
  return avg + `<line class="tltrend ramp gauge-stroke" style="${paint(w.ramp)}" x1="${nowX.toFixed(1)}" x2="${resetX.toFixed(1)}" y1="${nowY.toFixed(1)}" y2="${endY.toFixed(1)}"/>
    <text class="rax gauge-fill" style="${paint(w.ramp)}" x="${box.x1}" y="14" text-anchor="end">${label}</text>`;
}

export function chart(w: WindowOut, view: View): string {
  if (w.idle) return empty(view, "no active block — the next message opens a 5 h block");
  const p = plot(w, view);
  if (!p || p.total <= 0) return empty(view, "nothing in this window yet");
  const box = p.box;

  const unit = (v: number) => (p.pct ? `${Math.round(v)}%` : usd(v));
  const step = p.pct ? (p.yMax > 150 ? 50 : 25) : p.yMax / 4;
  const grid: string[] = [];
  for (let v = 0; v <= p.yMax + 1e-9; v += step) {
    const hi = p.pct && Math.abs(v - 100) < 1e-9;
    grid.push(`<line class="${hi ? "tl100" : "tlgrid"}" x1="${box.x0}" x2="${box.x1}" y1="${p.Y(v).toFixed(1)}" y2="${p.Y(v).toFixed(1)}"/>
      <text class="ax${hi ? " hi" : ""}" x="${box.x0 - 10}" y="${(p.Y(v) + 3.5).toFixed(1)}" text-anchor="end">${esc(unit(v))}</text>`);
  }

  // Thin the tick labels to what fits. The buckets only span window-open to
  // now, so the room available is that stretch — not the whole plot, most of
  // which is the time still to come.
  const perLabel = tickLabel(p.buckets[0].t, view.bucket).length * MONO_ADV + 18;
  const used = Math.max(1, p.X(p.buckets[p.buckets.length - 1].end) - p.X(p.buckets[0].t));
  const every = Math.max(1, Math.ceil(p.buckets.length / Math.max(2, Math.floor(used / perLabel))));
  const ticks = p.buckets.filter((_, i) => i % every === 0).map((b) => {
    const x = p.X(b.t).toFixed(1);
    return `<line class="tlaxis" x1="${x}" x2="${x}" y1="${box.yBase}" y2="${box.yBase + 4}"/>
      <text class="ax" x="${x}" y="${box.yBase + 16}" text-anchor="middle">${esc(tickLabel(b.t, view.bucket))}</text>`;
  });

  // Late in a window the "now" stamp runs under the trend label, which is
  // right-aligned to the same line. Drop it to the second line of the margin.
  const nowX = p.X(view.now), nowY = p.Y(p.total);
  const nowLabel = `now ${clock(view.now)}`;
  const nowLabelY = nowX - 5 - nowLabel.length * MONO_ADV > box.x1 - 40 * MONO_ADV ? box.yTop - 2 : box.yTop - 12;
  const resetX = p.X(Date.parse(w.reset)).toFixed(1);

  return frame(view, grid.join("") + ticks.join("")
    + `<line class="tlaxis" x1="${box.x0}" x2="${box.x0}" y1="${box.yTop}" y2="${box.yBase}"/>`
    + trend(w, view, p)
    + bands(p.P, p.active, p.ks, view.hoverKey)
    + `<circle class="tlnow" cx="${nowX.toFixed(1)}" cy="${nowY.toFixed(1)}" r="3"/>
       <text class="tlnowv" x="${(nowX + 8).toFixed(1)}" y="${(nowY - 8).toFixed(1)}">${esc(p.pct ? p.total.toFixed(1) + "%" : usd(p.total))}</text>
       <line class="tlmark" x1="${nowX.toFixed(1)}" x2="${nowX.toFixed(1)}" y1="${box.yTop - 8}" y2="${box.yBase}"/>
       <text class="ax" x="${(nowX - 5).toFixed(1)}" y="${nowLabelY}" text-anchor="end">${esc(nowLabel)}</text>
       <line class="tlmark dash" x1="${resetX}" x2="${resetX}" y1="${box.yTop - 8}" y2="${box.yBase + 4}"/>
       <text class="ax" x="${resetX}" y="${box.yBase + 30}" text-anchor="end">reset ${esc(until(w.reset, view.now))}</text>`
    + `<g class="tlg" id="tlGuide"></g>`);
}

export function subline(w: WindowOut, view: View): string {
  const n = bucketCount(w, view);
  const unit = w.pct === null ? "cumulative weighted cost" : "cumulative % of this limit";
  const buckets = `${n} bucket${n === 1 ? "" : "s"}`;
  if (narrow()) return `${unit} · ${buckets}`;
  const per = view.mode === "by_session" ? "conversation" : "model";
  return `${unit} · one line, split into a touching band per ${per} running at that moment · ${buckets}`;
}
