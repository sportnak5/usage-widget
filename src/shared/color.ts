// Model colors and the pace ramp. Hues live here; saturation and lightness
// come from theme tokens so the same hue holds on both grounds.

const MODEL_VAR: Record<string, string> = {
  "claude-fable-5-1": "--m-fable51",
  "claude-fable-5": "--m-fable5",
  "claude-opus-5": "--m-opus5",
  "claude-opus-4-8": "--m-opus48",
  "claude-opus-4-7": "--m-opus48",
  "claude-opus-4-6": "--m-opus48",
  "claude-sonnet-5": "--m-sonnet5",
  "claude-sonnet-4-6": "--m-sonnet5",
  "claude-haiku-4-5": "--m-haiku",
};

export function modelColor(model: string): string {
  const exact = MODEL_VAR[model];
  if (exact) return `var(${exact})`;
  const family = Object.keys(MODEL_VAR).filter((k) => model.startsWith(k)).sort((a, b) => b.length - a.length)[0];
  return family ? `var(${MODEL_VAR[family]})` : "var(--m-rest)";
}

export const convColor = (i: number): string => `var(--c${i % 8})`;

// Non-uniform stops: a linear 142°→0° sweep parks its middle in washed-out
// yellow-green where green, yellow and orange are hard to tell apart.
const STOPS: [number, number][] = [[0, 148], [0.3, 126], [0.5, 70], [0.7, 38], [0.85, 18], [1, 2]];

export function hueAt(t: number): number {
  t = Math.min(1, Math.max(0, t));
  for (let i = 1; i < STOPS.length; i++) {
    const [t1, h1] = STOPS[i], [t0, h0] = STOPS[i - 1];
    if (t <= t1) return h0 + (h1 - h0) * (t - t0) / (t1 - t0);
  }
  return 2;
}

// Yellows read pale at the lightness that suits greens and reds; darken them.
const lx = (h: number): string => (1 - 0.16 * Math.max(0, 1 - Math.abs(h - 68) / 46)).toFixed(3);

/** Inline style setting --h/--lx for anything colored by the ramp. */
export function paint(t: number | null): string {
  if (t === null) return "--h:0;--lx:1;--gauge-muted:1";
  const h = hueAt(t).toFixed(1);
  return `--h:${h};--lx:${lx(Number(h))}`;
}

export const paceNote = (pace: number | null): string =>
  pace === null ? "not calibrated" : Math.abs(pace - 1) < 0.05 ? "on pace" : `${pace.toFixed(1)}× pace`;
