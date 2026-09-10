// The only file that talks to Rust. Everything else is plain DOM.
//
// Outside Tauri (plain `vite dev` in a browser) there is no backend, so the
// same functions serve `public/dev/snapshot.json` — produced by
// `ledger-cli --json` — and turn every command into a no-op. That keeps the
// UI developable and screenshot-able without the native shell.
import type { CalibrationInput, Settings, Snapshot, ThreadSort } from "./types";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (e: { payload: T }) => void) => Promise<() => void>;
type Emit = (event: string, payload?: unknown) => Promise<void>;

let invoke: Invoke;
let listen: Listen;
let emit: Emit;

if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const ev = await import("@tauri-apps/api/event");
  invoke = core.invoke as Invoke;
  listen = ev.listen as Listen;
  emit = ev.emit as Emit;
} else {
  let cached: Snapshot | null = null;
  const devSnapshot = async (): Promise<Snapshot | null> => {
    if (cached) return cached;
    try {
      const r = await fetch("/dev/snapshot.json");
      if (r.ok) cached = (await r.json()) as Snapshot;
    } catch { /* no fixture */ }
    return cached;
  };
  const devSettings: Settings = {
    claude_dir: null, plan: "Max (20x)", boost: null, refresh_secs: 15,
    calibration: { session_reset_at: null, auto_fetched_at: null, manual_at: null, weekly_reset: null, session: null, weekly: null, fable: null },
    widget: null, show_widget: true, widget_on_top: false, theme: null,
    thread_sort: "usage", list_rows: 25, widget_rows: 5, live_readings: false,
  };
  // The dev fixture has no backend to re-rank it, so do it here: the toggle
  // is worth exercising without the native shell.
  const devSortedSnapshot = async (sort: ThreadSort): Promise<Snapshot | null> => {
    const s = await devSnapshot();
    if (!s) return s;
    s.thread_sort = sort;
    for (const w of s.windows) {
      w.by_session.sort((a, b) => sort === "recent"
        ? Date.parse(b.last) - Date.parse(a.last) || b.cost - a.cost
        : b.cost - a.cost);
    }
    return s;
  };
  invoke = (async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "get_snapshot": case "refresh_now": case "calibrate": case "update_settings":
      case "set_live_readings": case "mark_thread_read": return devSnapshot();
      case "set_thread_sort": return devSortedSnapshot(args?.sort as ThreadSort);
      case "check_live_readings": return "Dev fixture — no credential is read here.";
      case "get_settings": return devSettings;
      case "get_home": return "/Users/me";
      case "get_autostart": return false;
      default: return undefined;
    }
  }) as Invoke;
  listen = (async () => () => {}) as Listen;
  emit = (async () => {}) as Emit;
}

export const isNative = inTauri;
export const getSnapshot = () => invoke<Snapshot | null>("get_snapshot");
export const refreshNow = () => invoke<Snapshot>("refresh_now");
export const getSettings = () => invoke<Settings>("get_settings");
export const getHome = () => invoke<string>("get_home");
export const calibrate = (input: CalibrationInput) => invoke<Snapshot>("calibrate", { input });
/** Switching on is refused unless a real fetch works first, so the toggle can
 *  never sit on while quietly doing nothing. */
export const setLiveReadings = (enabled: boolean) => invoke<Snapshot>("set_live_readings", { enabled });
/** Exercise the whole path without changing any setting. */
export const checkLiveReadings = () => invoke<string>("check_live_readings");
export const updateSettings = (patch: Partial<Pick<Settings, "claude_dir" | "refresh_secs" | "show_widget" | "widget_on_top" | "plan" | "boost" | "list_rows" | "widget_rows">>) =>
  invoke<Snapshot>("update_settings", { patch });
/** Rank both windows' conversation lists. The snapshot that comes back — and
 *  the one every other window is sent — carries the new order and the new
 *  toggle state, so the two can't drift apart. */
export const setThreadSort = (sort: ThreadSort) => invoke<Snapshot>("set_thread_sort", { sort });
/** Opening a conversation spends its unread badge. */
export const markThreadRead = (session: string) => invoke<Snapshot>("mark_thread_read", { session });
export const openMain = () => invoke<void>("open_main");
export const openSettings = () => invoke<void>("open_settings");
export const setWidgetExpanded = (expanded: boolean, width: number, height: number, scale: number) =>
  invoke<void>("set_widget_expanded", { expanded, width, height, scale });
export const hideWidget = () => invoke<void>("hide_widget");
export const getAutostart = () => invoke<boolean>("get_autostart");
export const setAutostart = (on: boolean) => invoke<void>("set_autostart", { on });

export const onSnapshot = (cb: (s: Snapshot) => void) => listen<Snapshot>("snapshot", (e) => cb(e.payload));
export const onOpenSettings = (cb: () => void) => listen<void>("open-settings", () => cb());

// Appearance lives in localStorage, which the two webviews share but do not get
// storage events across, so a change is announced explicitly.
export const broadcastScheme = () => emit("scheme-changed");
export const onSchemeChanged = (cb: () => void) => listen<void>("scheme-changed", () => cb());

/** Window drag has to come from the window API; a no-op in the browser. */
export async function startDragging(): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}

/** Hand the drag to the OS resize loop, from the widget's corner grip. */
export async function startResizing(): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startResizeDragging("SouthEast");
}
