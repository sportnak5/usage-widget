export const esc = (s: unknown): string =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c] as string);

export const short = (m: string): string => m.replace("claude-", "").replace(/-\d{8}$/, "");

export const tok = (n: number): string =>
  n >= 1e9 ? (n / 1e9).toFixed(2) + "B"
  : n >= 1e6 ? (n / 1e6).toFixed(1) + "M"
  : n >= 1e3 ? Math.round(n / 1e3) + "K"
  : String(n);

export const usd = (n: number): string => "$" + n.toFixed(2);

export const hhmm = (s: string): string =>
  new Date(s).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

export const when = (s: string): string =>
  new Date(s).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" });

/** "in 3 hr 46 min" / "in 2 d 4 hr" / "now" */
export function until(iso: string, now: number = Date.now()): string {
  const ms = new Date(iso).getTime() - now;
  if (ms <= 0) return "now";
  const m = Math.round(ms / 60000);
  if (m < 60) return `in ${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `in ${h} hr ${m % 60} min`;
  const d = Math.floor(h / 24);
  return `in ${d} d ${h % 24} hr`;
}

let HOME = "";
export const setHome = (h: string) => { HOME = h.replace(/[\\/]+$/, ""); };

/** `/Users/me/proj` → `~/proj`; Windows paths too. */
export function tidy(p: string | null | undefined): string {
  if (!p) return "(unknown)";
  if (HOME && (p === HOME)) return "~";
  if (HOME && (p.startsWith(HOME + "/") || p.startsWith(HOME + "\\"))) return "~" + p.slice(HOME.length);
  return p;
}

export const basename = (p: string): string => p.split(/[\\/]/).filter(Boolean).pop() || p;

export const entryName = (e: { key: string | [string, string]; title: string | null }, g: string): string => {
  if (g === "by_model") return short(e.key as string);
  if (g === "by_project") return tidy(e.key as string);
  return e.title || "untitled session";
};
