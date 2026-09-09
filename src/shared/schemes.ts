// Color schemes. tokens.css holds the base ("Ledger") palette; a scheme is a
// partial override of those custom properties, stamped into a <style> element
// that is appended after the stylesheets so it wins on document order.
//
// A scheme carries a light map and a dark map, because the app has three theme
// states (explicit light, explicit dark, follow the system) and the override
// has to mirror all three the same way tokens.css does.

export interface TokenDef { v: string; label: string }
export interface TokenGroup { name: string; tokens: TokenDef[] }
export type Vars = Record<string, string>;
export interface Scheme { id: string; name: string; note: string; light: Vars; dark: Vars }

/** Every color a user may edit, grouped the way the settings dialog shows them. */
export const GROUPS: TokenGroup[] = [
  { name: "Surfaces", tokens: [
    { v: "--ground", label: "Page" }, { v: "--surface", label: "Card" }, { v: "--sunk", label: "Recessed" },
    { v: "--line", label: "Border" }, { v: "--line-soft", label: "Hairline" }, { v: "--ring", label: "Dial track" },
  ] },
  { name: "Text", tokens: [
    { v: "--ink", label: "Primary" }, { v: "--ink-2", label: "Secondary" },
    { v: "--muted", label: "Muted" }, { v: "--faint", label: "Faint" },
  ] },
  { name: "Accent & status", tokens: [
    { v: "--accent", label: "Accent" }, { v: "--accent-soft", label: "Accent wash" },
    { v: "--warn", label: "Warning" }, { v: "--crit", label: "Error" },
  ] },
  { name: "Models", tokens: [
    { v: "--m-fable51", label: "Fable 5.1" }, { v: "--m-fable5", label: "Fable 5" },
    { v: "--m-opus5", label: "Opus 5" }, { v: "--m-opus48", label: "Opus 4.x" },
    { v: "--m-sonnet5", label: "Sonnet" }, { v: "--m-haiku", label: "Haiku" },
    { v: "--m-rest", label: "Everything else" },
  ] },
  { name: "Conversations", tokens: [
    { v: "--c0", label: "1st" }, { v: "--c1", label: "2nd" }, { v: "--c2", label: "3rd" }, { v: "--c3", label: "4th" },
    { v: "--c4", label: "5th" }, { v: "--c5", label: "6th" }, { v: "--c6", label: "7th" }, { v: "--c7", label: "8th" },
  ] },
];

export const ALL_TOKENS: string[] = GROUPS.flatMap((g) => g.tokens.map((t) => t.v));

export const SCHEMES: Scheme[] = [
  {
    id: "ledger", name: "Ledger", note: "the original indigo",
    light: {}, dark: {}, // tokens.css is this scheme
  },
  {
    id: "slate", name: "Slate", note: "cool grey, teal accent",
    light: {
      "--ground": "#EEF1F3", "--surface": "#FFFFFF", "--sunk": "#E4E8EB", "--line": "#D2D8DD", "--line-soft": "#E6EAEE",
      "--ring": "#DCE1E6", "--ink": "#12181C", "--ink-2": "#38434B", "--muted": "#5C6873", "--faint": "#87929C",
      "--accent": "#0F7B84", "--accent-soft": "#DCEEF0", "--warn": "#A8801E", "--crit": "#A63A32",
      "--m-fable51": "#3F6E9E", "--m-fable5": "#6E9BC4", "--m-opus5": "#0F7B84", "--m-opus48": "#3EA0A6",
      "--m-sonnet5": "#4C7A5A", "--m-haiku": "#7C8A94", "--m-rest": "#BAC3CA",
      "--c0": "#0F7B84", "--c1": "#3F6E9E", "--c2": "#4C7A5A", "--c3": "#A8801E",
      "--c4": "#96566E", "--c5": "#5E8FA8", "--c6": "#6B8F4E", "--c7": "#8A6A55",
    },
    dark: {
      "--ground": "#0E1316", "--surface": "#161D22", "--sunk": "#0F1519", "--line": "#293338", "--line-soft": "#212A2F",
      "--ring": "#242E33", "--ink": "#E4EAEE", "--ink-2": "#B6C2CA", "--muted": "#8A97A1", "--faint": "#68757E",
      "--accent": "#43BDC4", "--accent-soft": "#16333A", "--warn": "#D6A544", "--crit": "#DE6A60",
      "--m-fable51": "#79A8D6", "--m-fable5": "#A3C6E4", "--m-opus5": "#43BDC4", "--m-opus48": "#77D3D6",
      "--m-sonnet5": "#7FC08F", "--m-haiku": "#8C99A3", "--m-rest": "#374249",
      "--c0": "#43BDC4", "--c1": "#79A8D6", "--c2": "#7FC08F", "--c3": "#D6A544",
      "--c4": "#DB8CA5", "--c5": "#8FB6CC", "--c6": "#A5C878", "--c7": "#C79C7F",
    },
  },
  {
    id: "ember", name: "Ember", note: "warm paper, amber accent",
    light: {
      "--ground": "#F4EFE8", "--surface": "#FFFCF8", "--sunk": "#EBE4DA", "--line": "#DED3C5", "--line-soft": "#EDE6DB",
      "--ring": "#E3DACE", "--ink": "#241C14", "--ink-2": "#4A3E32", "--muted": "#6E6154", "--faint": "#968878",
      "--accent": "#B5561D", "--accent-soft": "#F7E3D5", "--warn": "#A9761A", "--crit": "#A62F2A",
      "--m-fable51": "#8A4B8C", "--m-fable5": "#B07CB2", "--m-opus5": "#B5561D", "--m-opus48": "#D07C43",
      "--m-sonnet5": "#4E7F5C", "--m-haiku": "#93857A", "--m-rest": "#CFC3B4",
      "--c0": "#B5561D", "--c1": "#8A4B8C", "--c2": "#4E7F5C", "--c3": "#A9761A",
      "--c4": "#A34158", "--c5": "#5A7796", "--c6": "#6F8438", "--c7": "#8A6248",
    },
    dark: {
      "--ground": "#16110C", "--surface": "#1F1913", "--sunk": "#15100B", "--line": "#352C22", "--line-soft": "#2A231B",
      "--ring": "#2E251C", "--ink": "#F0E7DC", "--ink-2": "#C9BCAC", "--muted": "#9B8D7D", "--faint": "#786B5D",
      "--accent": "#E8834A", "--accent-soft": "#3A2416", "--warn": "#DFAE4E", "--crit": "#E36B5F",
      "--m-fable51": "#C48FC6", "--m-fable5": "#DBB4DC", "--m-opus5": "#E8834A", "--m-opus48": "#F0A87C",
      "--m-sonnet5": "#79BE8B", "--m-haiku": "#9A8C7E", "--m-rest": "#3E342A",
      "--c0": "#E8834A", "--c1": "#C48FC6", "--c2": "#79BE8B", "--c3": "#DFAE4E",
      "--c4": "#DE8098", "--c5": "#8DA9CC", "--c6": "#B0C56A", "--c7": "#C89A79",
    },
  },
  {
    id: "forest", name: "Forest", note: "green ground, moss accent",
    light: {
      "--ground": "#ECF1EA", "--surface": "#FFFFFF", "--sunk": "#E2E9DF", "--line": "#D0DACC", "--line-soft": "#E4EBE1",
      "--ring": "#DBE3D7", "--ink": "#141A15", "--ink-2": "#3A463A", "--muted": "#5C6A5C", "--faint": "#87947F",
      "--accent": "#2F6B45", "--accent-soft": "#DEECE1", "--warn": "#9E7C1C", "--crit": "#A3392E",
      "--m-fable51": "#5B5AA6", "--m-fable5": "#8B87C6", "--m-opus5": "#2F6B45", "--m-opus48": "#5A9068",
      "--m-sonnet5": "#1F7C79", "--m-haiku": "#7E8B7C", "--m-rest": "#BCC7B8",
      "--c0": "#2F6B45", "--c1": "#5B5AA6", "--c2": "#1F7C79", "--c3": "#9E7C1C",
      "--c4": "#9B4560", "--c5": "#4C7BA0", "--c6": "#6D8B34", "--c7": "#87664A",
    },
    dark: {
      "--ground": "#0D120E", "--surface": "#151C16", "--sunk": "#0E140F", "--line": "#263127", "--line-soft": "#1F291F",
      "--ring": "#212B22", "--ink": "#E4EBE2", "--ink-2": "#B7C4B6", "--muted": "#8B9889", "--faint": "#6A776A",
      "--accent": "#5FBB80", "--accent-soft": "#1B3324", "--warn": "#D3A748", "--crit": "#DE6A5C",
      "--m-fable51": "#9A97E0", "--m-fable5": "#BDBAEE", "--m-opus5": "#5FBB80", "--m-opus48": "#8AD3A2",
      "--m-sonnet5": "#3FB0AD", "--m-haiku": "#8B978A", "--m-rest": "#333E34",
      "--c0": "#5FBB80", "--c1": "#9A97E0", "--c2": "#3FB0AD", "--c3": "#D3A748",
      "--c4": "#DA7F9C", "--c5": "#7FA8D2", "--c6": "#A8C468", "--c7": "#C09877",
    },
  },
  {
    id: "graphite", name: "Graphite", note: "near-monochrome, one blue",
    light: {
      "--ground": "#F0F0F1", "--surface": "#FFFFFF", "--sunk": "#E7E7E9", "--line": "#D6D6D9", "--line-soft": "#E9E9EC",
      "--ring": "#DFDFE2", "--ink": "#141416", "--ink-2": "#3D3D42", "--muted": "#616167", "--faint": "#8C8C93",
      "--accent": "#2F5FD0", "--accent-soft": "#E3E9FA", "--warn": "#8E7A2A", "--crit": "#9C3A34",
      "--m-fable51": "#2B2B30", "--m-fable5": "#57575E", "--m-opus5": "#2F5FD0", "--m-opus48": "#6D88CE",
      "--m-sonnet5": "#7A7A82", "--m-haiku": "#9C9CA4", "--m-rest": "#C6C6CB",
      "--c0": "#2F5FD0", "--c1": "#2B2B30", "--c2": "#6D88CE", "--c3": "#57575E",
      "--c4": "#8C8C93", "--c5": "#4A6FA8", "--c6": "#71717A", "--c7": "#A4A4AC",
    },
    dark: {
      "--ground": "#101012", "--surface": "#191A1D", "--sunk": "#111113", "--line": "#2C2D31", "--line-soft": "#232427",
      "--ring": "#26272B", "--ink": "#E9E9EC", "--ink-2": "#BEBEC5", "--muted": "#8E8E97", "--faint": "#6C6C75",
      "--accent": "#7C9BF2", "--accent-soft": "#20263C", "--warn": "#CFAE55", "--crit": "#DC6A61",
      "--m-fable51": "#E4E4EA", "--m-fable5": "#B4B4BE", "--m-opus5": "#7C9BF2", "--m-opus48": "#A5B8F6",
      "--m-sonnet5": "#8A8A94", "--m-haiku": "#6C6C76", "--m-rest": "#33343A",
      "--c0": "#7C9BF2", "--c1": "#E4E4EA", "--c2": "#A5B8F6", "--c3": "#B4B4BE",
      "--c4": "#8A8A94", "--c5": "#5D7BC4", "--c6": "#9C9CA6", "--c7": "#6C6C76",
    },
  },
];

export const CUSTOM_ID = "custom";
const KEY_ID = "tl-scheme";
const KEY_CUSTOM = "tl-custom";
const STYLE_ID = "tl-scheme-style";

const read = (k: string): string | null => { try { return localStorage.getItem(k); } catch { return null; } };
const write = (k: string, v: string): void => { try { localStorage.setItem(k, v); } catch { /* storage may be unavailable */ } };

export const schemeById = (id: string): Scheme | undefined => SCHEMES.find((s) => s.id === id);

/** The chosen scheme id — a built-in, or "custom". */
export function currentSchemeId(): string {
  const id = read(KEY_ID);
  return id === CUSTOM_ID || schemeById(id ?? "") ? (id as string) : "ledger";
}

/** The user's own palette: a partial override per mode, possibly empty. */
export function customScheme(): { light: Vars; dark: Vars } {
  try {
    const raw = read(KEY_CUSTOM);
    if (raw) {
      const p = JSON.parse(raw) as { light?: Vars; dark?: Vars };
      return { light: p.light ?? {}, dark: p.dark ?? {} };
    }
  } catch { /* corrupt or unavailable; fall through to empty */ }
  return { light: {}, dark: {} };
}

export function saveCustom(c: { light: Vars; dark: Vars }): void {
  write(KEY_CUSTOM, JSON.stringify(c));
}

const activeVars = (): { light: Vars; dark: Vars } => {
  const id = currentSchemeId();
  if (id === CUSTOM_ID) return customScheme();
  const s = schemeById(id)!;
  return { light: s.light, dark: s.dark };
};

const block = (vars: Vars): string => Object.entries(vars).map(([k, v]) => `${k}:${v}`).join(";");

/** Stamp the active scheme into a style element that outranks tokens.css. */
export function applyScheme(): void {
  const { light, dark } = activeVars();
  let el = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!el) {
    el = document.createElement("style");
    el.id = STYLE_ID;
    document.head.appendChild(el);
  }
  // All three theme states, mirroring how tokens.css declares them: an explicit
  // choice has to beat the system preference in either direction.
  el.textContent =
    `:root{${block(light)}}\n` +
    `@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){${block(dark)}}}\n` +
    `:root[data-theme="dark"]{${block(dark)}}`;
}

export function setScheme(id: string): void {
  write(KEY_ID, id);
  applyScheme();
}

/** Read what a token actually resolves to right now, as `#rrggbb`. */
export function computedToken(v: string): string {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(v).trim();
  return toHex(raw);
}

export function toHex(c: string): string {
  const s = c.trim();
  if (/^#[0-9a-f]{6}$/i.test(s)) return s.toLowerCase();
  if (/^#[0-9a-f]{3}$/i.test(s)) return "#" + s.slice(1).split("").map((d) => d + d).join("").toLowerCase();
  if (/^#[0-9a-f]{8}$/i.test(s)) return s.slice(0, 7).toLowerCase();
  const m = s.match(/^rgba?\(([^)]+)\)$/i);
  if (m) {
    const [r, g, b] = m[1].split(/[,\s/]+/).filter(Boolean).slice(0, 3)
      .map((n) => Math.round(n.endsWith("%") ? Number(n.slice(0, -1)) * 2.55 : Number(n)));
    return "#" + [r, g, b].map((n) => Math.max(0, Math.min(255, n | 0)).toString(16).padStart(2, "0")).join("");
  }
  return "#000000";
}

/** The palette a scheme shows in a given mode, filled in from the base tokens. */
export function paletteOf(id: string, mode: "light" | "dark"): Vars {
  const own = id === CUSTOM_ID ? customScheme()[mode] : (schemeById(id) ?? SCHEMES[0])[mode];
  const out: Vars = {};
  // Ledger is tokens.css itself, so anything a scheme leaves out comes from the sheet.
  for (const v of ALL_TOKENS) out[v] = toHex(own[v] ?? baseToken(v, mode));
  return out;
}

// tokens.css is the source of truth for the Ledger palette. Rather than
// duplicating it here, ask the document — with the scheme style temporarily
// silenced so its overrides don't answer for the base.
let baseCache: { light: Vars; dark: Vars } | null = null;
function baseToken(v: string, mode: "light" | "dark"): string {
  if (!baseCache) baseCache = readBase();
  return baseCache[mode][v] ?? "#000000";
}

function readBase(): { light: Vars; dark: Vars } {
  const probe = document.createElement("div");
  probe.style.display = "none";
  document.body.appendChild(probe);
  const style = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  const saved = style?.textContent ?? null;
  if (style) style.textContent = "";
  const root = document.documentElement;
  const had = root.dataset.theme;
  const grab = (mode: "light" | "dark"): Vars => {
    root.dataset.theme = mode;
    const cs = getComputedStyle(probe);
    const out: Vars = {};
    for (const v of ALL_TOKENS) out[v] = toHex(cs.getPropertyValue(v));
    return out;
  };
  const light = grab("light");
  const dark = grab("dark");
  if (had) root.dataset.theme = had; else delete root.dataset.theme;
  if (style && saved !== null) style.textContent = saved;
  probe.remove();
  return { light, dark };
}
