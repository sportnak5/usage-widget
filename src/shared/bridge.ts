// The only file that talks to Rust. Everything else is plain DOM.
//
// Outside Tauri (plain `vite dev` in a browser) there is no backend, so the
// same functions serve `public/dev/snapshot.json` — produced by
// `ledger-cli --json` — and turn every command into a no-op. That keeps the
// UI developable and screenshot-able without the native shell.
import type { CalibrationInput, Settings, Snapshot } from "./types";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T>(event: string, cb: (e: { payload: T }) => void) => Promise<() => void>;

let invoke: Invoke;
let listen: Listen;

if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const ev = await import("@tauri-apps/api/event");
  invoke = core.invoke as Invoke;
  listen = ev.listen as Listen;
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
    claude_dir: null, plan: "Max (20x)", boost: null, refresh_secs: 60,
    calibration: { session_reset_at: null, auto_fetched_at: null, manual_at: null, weekly_reset: null, session: null, weekly: null, fable: null },
    widget: null, show_widget: true, theme: null,
  };
  invoke = (async (cmd: string) => {
    switch (cmd) {
      case "get_snapshot": case "refresh_now": case "calibrate": case "update_settings": return devSnapshot();
      case "get_settings": return devSettings;
      case "get_home": return "/Users/me";
      case "get_autostart": return false;
      default: return undefined;
    }
  }) as Invoke;
  listen = (async () => () => {}) as Listen;
}

export const isNative = inTauri;
export const getSnapshot = () => invoke<Snapshot | null>("get_snapshot");
export const refreshNow = () => invoke<Snapshot>("refresh_now");
export const getSettings = () => invoke<Settings>("get_settings");
export const getHome = () => invoke<string>("get_home");
export const calibrate = (input: CalibrationInput) => invoke<Snapshot>("calibrate", { input });
export const updateSettings = (patch: Partial<Pick<Settings, "claude_dir" | "refresh_secs" | "show_widget" | "plan" | "boost">>) =>
  invoke<Snapshot>("update_settings", { patch });
export const openMain = () => invoke<void>("open_main");
export const openSettings = () => invoke<void>("open_settings");
export const setWidgetExpanded = (expanded: boolean, width: number, height: number) =>
  invoke<void>("set_widget_expanded", { expanded, width, height });
export const hideWidget = () => invoke<void>("hide_widget");
export const getAutostart = () => invoke<boolean>("get_autostart");
export const setAutostart = (on: boolean) => invoke<void>("set_autostart", { on });

export const onSnapshot = (cb: (s: Snapshot) => void) => listen<Snapshot>("snapshot", (e) => cb(e.payload));
export const onOpenSettings = (cb: () => void) => listen<void>("open-settings", () => cb());

/** Window drag has to come from the window API; a no-op in the browser. */
export async function startDragging(): Promise<void> {
  if (!inTauri) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().startDragging();
}
