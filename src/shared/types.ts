// Mirrors src-tauri/src/ledger/snapshot.rs. Keep the two in step.

export type WindowId = "session" | "weekly" | "fable";

export interface ModelPart {
  model: string;
  cost: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
}

export interface Entry {
  key: string | [string, string];
  title: string | null;
  title_kind: "custom" | "prompt" | null;
  cost: number;
  raw: number;
  n: number;
  first: string;
  last: string;
  share: number;
  pct: number | null;
  models: ModelPart[];
}

export interface Kinds {
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
}

export interface WindowOut {
  id: WindowId;
  label: string;
  sub: string;
  start: string;
  reset: string;
  boundary_known: boolean;
  idle: boolean;
  calibrated: boolean;
  pct: number | null;
  pace: number | null;
  ramp: number | null;
  elapsed_frac: number;
  total_cost: number;
  total_raw: number;
  messages: number;
  limit: number | null;
  kinds: Kinds;
  by_model: Entry[];
  by_project: Entry[];
  by_session: Entry[];
}

export interface ScanStats {
  files_total: number;
  files_in_window: number;
  files_read: number;
  bytes_read: number;
  lines_seen: number;
  records_added: number;
  duplicates_skipped: number;
  records_retained: number;
  duration_ms: number;
}

export interface Reading { percent: number; resets_at: string | null }
export interface UsageCache {
  fetched_at: string;
  session: Reading | null;
  weekly: Reading | null;
  fable: Reading | null;
}

/** Why Claude Code's cached Usage reading did or didn't calibrate us. */
export type CacheReason =
  | "ok" | "no_dir" | "no_file" | "unreadable" | "no_key" | "no_readings" | "too_low" | "no_usage";

export interface CacheDiag {
  path: string;
  projects_dir: string;
  reason: CacheReason;
  detail: string | null;
  fetched_at: string | null;
  best_pct: number | null;
}

export interface Snapshot {
  generated_at: string;
  plan: string;
  boost: string | null;
  calibrated: boolean;
  windows: WindowOut[];
  scan: ScanStats;
  index_records: number;
  usage_cache: UsageCache | null;
  calibration_source: "auto" | "manual" | "none";
  cache_diag: CacheDiag;
}

export interface WeeklyReset {
  weekday: "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun";
  hour: number;
  minute: number;
}

export interface Anchor {
  pct: number;
  captured_at: string;
  implied_limit: number;
}

export interface Calibration {
  session_reset_at: string | null;
  auto_fetched_at: string | null;
  manual_at: string | null;
  weekly_reset: WeeklyReset | null;
  session: Anchor | null;
  weekly: Anchor | null;
  fable: Anchor | null;
}

export interface Settings {
  claude_dir: string | null;
  plan: string;
  boost: string | null;
  refresh_secs: number;
  calibration: Calibration;
  widget: { x: number; y: number; expanded: boolean; scale: number } | null;
  show_widget: boolean;
  widget_on_top: boolean;
  theme: string | null;
}

export interface CalibrationInput {
  session_pct?: number;
  weekly_pct?: number;
  fable_pct?: number;
  session_resets_in_minutes?: number;
  weekly_reset?: WeeklyReset;
  plan?: string;
  boost?: string;
}

export type GroupKey = "by_session" | "by_project" | "by_model";
