//! The Tauri shell: two windows, a tray icon, a refresh loop, and the
//! commands the pages call. All logic lives in `engine`; this file only
//! wires it to the OS.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Deserialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use crate::engine::{CalibrationInput, Engine};
use crate::ledger::Snapshot;
use crate::settings::{Settings, WidgetGeometry};

pub struct AppState {
    engine: Mutex<Engine>,
    snapshot: Mutex<Option<Snapshot>>,
    /// Creating the widget window emits a Moved event of its own, before the
    /// remembered position has been applied. Until the position *we* chose is
    /// on the window, a move is the window server talking, not the user.
    widget_placed: AtomicBool,
    /// A move the window server reported that has not been written down yet.
    widget_move: Mutex<Option<PendingMove>>,
    /// The display layout as last seen, and when it last changed.
    displays: Mutex<Displays>,
}

/// Where the widget was last reported, and when. Held back rather than saved,
/// because a display appearing or disappearing relocates the widget exactly
/// the way a drag does and only the timing tells them apart.
struct PendingMove {
    pos: LogicalPosition<f64>,
    at: Instant,
}

/// Position, size and scale of one display: x, y, width, height, scale bits.
type DisplayShape = (i32, i32, u32, u32, u64);

struct Displays {
    /// Every display, sorted. `None` until the first poll has run.
    fingerprint: Option<Vec<DisplayShape>>,
    changed_at: Instant,
}

/// How often the display layout is checked. A KVM switch takes the monitor
/// away without warning, and nothing notifies an app that it happened.
const DISPLAY_POLL: Duration = Duration::from_millis(400);
/// How long after a display change a reported move is still the window server
/// rearranging windows rather than the user dragging one.
const DISPLAY_SETTLE: Duration = Duration::from_secs(2);
/// How long a move has to stand before it is taken as the user's choice.
const MOVE_SETTLE: Duration = Duration::from_millis(700);

const WIDGET_MIN_W: f64 = 380.0;
const WIDGET_MAX_W: f64 = 640.0;
const WIDGET_H: f64 = 84.0;
const WIDGET_MIN_SCALE: f64 = 0.6;
const WIDGET_MAX_SCALE: f64 = 3.0;

fn lock<'a, T>(m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Rescan, rebuild the snapshot, tell every window. Persist the index only
/// when something was read — a warm refresh with nothing new costs nothing.
fn do_refresh(app: &AppHandle) -> Result<Snapshot, String> {
    let state = app.state::<AppState>();
    let mut eng = lock(&state.engine);
    let now = Utc::now();
    let result = eng.refresh(now);
    if matches!(&result, Ok(s) if s.files_read > 0) {
        eng.save();
    }
    let snap = eng.snapshot(now);
    drop(eng);
    *lock(&state.snapshot) = Some(snap.clone());
    let _ = app.emit("snapshot", &snap);
    result.map(|_| snap)
}

fn republish(app: &AppHandle) -> Snapshot {
    let state = app.state::<AppState>();
    let mut eng = lock(&state.engine);
    eng.save();
    let snap = eng.snapshot(Utc::now());
    drop(eng);
    *lock(&state.snapshot) = Some(snap.clone());
    let _ = app.emit("snapshot", &snap);
    snap
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn show_widget(app: &AppHandle) {
    let Some(w) = app.get_webview_window("widget") else { return };
    let state = app.state::<AppState>();
    let on_top = lock(&state.engine).settings.widget_on_top;
    place_widget(app, &w);
    state.widget_placed.store(true, Ordering::Relaxed);
    apply_widget_layer(&w, on_top);
    let _ = w.show();
}

/// Put the widget back on its remembered position. Runs at first show and
/// again every time the set of displays changes.
fn place_widget(app: &AppHandle, w: &tauri::WebviewWindow) {
    let state = app.state::<AppState>();
    let geom = lock(&state.engine).settings.widget.clone();
    let pos = geom
        .filter(|g| on_a_monitor(w, g.x, g.y))
        .map(|g| LogicalPosition { x: g.x as f64, y: g.y as f64 })
        // Saved on a display that is gone, or asleep: park it where it can be
        // seen rather than showing it off the edge of every screen.
        .or_else(|| default_widget_position(w));
    if let Some(p) = pos {
        let (ox, oy) = crate::platform::desktop_offset(w.scale_factor().unwrap_or(1.0));
        let _ = w.set_position(tauri::Position::Logical(LogicalPosition { x: p.x - ox, y: p.y - oy }));
    }
}

/// Everything about the current display layout that moves windows around when
/// it changes: where each display sits, how big it is, and how dense it is.
fn monitor_fingerprint(w: &tauri::WebviewWindow) -> Option<Vec<DisplayShape>> {
    let monitors = w.available_monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    let mut v: Vec<_> = monitors
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            (p.x, p.y, s.width, s.height, m.scale_factor().to_bits())
        })
        .collect();
    v.sort();
    Some(v)
}

/// One pass of the display watcher, on the main thread because that is the
/// only place the window server answers questions about screens.
fn poll_displays(app: &AppHandle) {
    let Some(w) = app.get_webview_window("widget") else { return };
    let state = app.state::<AppState>();
    let now = Instant::now();

    let Some(fingerprint) = monitor_fingerprint(&w) else { return };
    let changed = {
        let mut d = lock(&state.displays);
        let first = d.fingerprint.is_none();
        let changed = d.fingerprint.as_ref() != Some(&fingerprint);
        if changed {
            d.fingerprint = Some(fingerprint);
            d.changed_at = now;
        }
        changed && !first
    };
    if changed {
        // A monitor came or went — a KVM switch, a lid, a sleeping display.
        // Whatever the window server did with the widget in response is not a
        // move the user asked for, so drop it and go back to the saved spot.
        *lock(&state.widget_move) = None;
        if state.widget_placed.load(Ordering::Relaxed) {
            place_widget(app, &w);
        }
        return;
    }

    let settled = {
        let mut pending = lock(&state.widget_move);
        let changed_at = lock(&state.displays).changed_at;
        if now.duration_since(changed_at) < DISPLAY_SETTLE {
            // Still settling: anything reported now — including the move our
            // own re-placement caused — is the window server, so throw it out
            // rather than hold it and write it down once the layout is quiet.
            *pending = None;
            None
        } else {
            match pending.as_ref() {
                Some(m) if now.duration_since(m.at) >= MOVE_SETTLE => pending.take(),
                _ => None,
            }
        }
    };
    if let Some(m) = settled {
        commit_widget_move(app, m.pos);
    }
}

/// Write down a move that has not settled yet, because the widget is about to
/// be hidden or the app to quit and the pending position would go with it.
fn flush_widget_move(app: &AppHandle) {
    let state = app.state::<AppState>();
    let pending = {
        let mut p = lock(&state.widget_move);
        let changed_at = lock(&state.displays).changed_at;
        if Instant::now().duration_since(changed_at) < DISPLAY_SETTLE {
            *p = None;
            None
        } else {
            p.take()
        }
    };
    if let Some(m) = pending {
        commit_widget_move(app, m.pos);
    }
}

/// Write a settled move into the saved geometry.
fn commit_widget_move(app: &AppHandle, pos: LogicalPosition<f64>) {
    let state = app.state::<AppState>();
    let mut eng = lock(&state.engine);
    let cur = eng.settings.widget.clone();
    let expanded = cur.as_ref().map(|g| g.expanded).unwrap_or(false);
    let scale = cur.as_ref().map(|g| g.scale).unwrap_or(1.0);
    eng.settings.widget = Some(WidgetGeometry { x: pos.x as i32, y: pos.y as i32, expanded, scale });
    let _ = eng.settings.save(&eng.settings_path);
}

/// The display holding the menu bar: on macOS the global coordinate space is
/// anchored to it, so it is the one at the origin. `primary_monitor` does not
/// agree when a second display sits above and to the left of it.
fn main_monitor(w: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let monitors = w.available_monitors().ok()?;
    monitors
        .iter()
        .find(|m| m.position().x == 0 && m.position().y == 0)
        .cloned()
        .or_else(|| w.primary_monitor().ok().flatten())
        .or_else(|| w.current_monitor().ok().flatten())
}

/// Is the widget's top-left corner inside a display that exists right now?
/// Everything here is in logical points: that is the space the saved geometry
/// is in, and the one the OS lays displays out in.
fn on_a_monitor(w: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    let Ok(monitors) = w.available_monitors() else { return true };
    monitors.iter().any(|m| {
        let p: LogicalPosition<f64> = m.position().to_logical(m.scale_factor());
        let s: tauri::LogicalSize<f64> = m.size().to_logical(m.scale_factor());
        let (x, y) = (x as f64, y as f64);
        // A whole corner of the card, not just the pixel, has to be on screen.
        x >= p.x && y >= p.y && x + 60.0 <= p.x + s.width && y + 40.0 <= p.y + s.height
    })
}

/// Top-right of the main display, inset far enough to clear the menu bar.
fn default_widget_position(w: &tauri::WebviewWindow) -> Option<LogicalPosition<f64>> {
    let m = main_monitor(w)?;
    let p: LogicalPosition<f64> = m.position().to_logical(m.scale_factor());
    let s: tauri::LogicalSize<f64> = m.size().to_logical(m.scale_factor());
    let size: tauri::LogicalSize<f64> = w
        .outer_size()
        .map(|z| z.to_logical(w.scale_factor().unwrap_or(1.0)))
        .unwrap_or(tauri::LogicalSize { width: 466.0, height: 87.0 });
    Some(LogicalPosition { x: p.x + (s.width - size.width - 40.0).max(0.0), y: p.y + 60.0 })
}

/// Put the widget back on the main display and show it — the escape hatch when
/// it is parked on a display that is no longer there.
fn reset_widget_position(app: &AppHandle) {
    let Some(w) = app.get_webview_window("widget") else { return };
    let state = app.state::<AppState>();
    // Anything the window server was in the middle of reporting is stale now.
    *lock(&state.widget_move) = None;
    if let Some(p) = default_widget_position(&w) {
        let _ = w.set_position(tauri::Position::Logical(p));
        let state = app.state::<AppState>();
        let mut eng = lock(&state.engine);
        let cur = eng.settings.widget.clone();
        eng.settings.widget = Some(WidgetGeometry {
            x: p.x as i32,
            y: p.y as i32,
            expanded: cur.as_ref().map(|g| g.expanded).unwrap_or(false),
            scale: cur.as_ref().map(|g| g.scale).unwrap_or(1.0),
        });
        eng.settings.show_widget = true;
        eng.save();
    }
    let _ = w.show();
}

/// Where the widget sits in the window stack: floating above everything, or
/// parked below normal windows so it reads as part of the desktop.
fn apply_widget_layer(w: &tauri::WebviewWindow, on_top: bool) {
    let _ = w.set_always_on_top(on_top);
    let _ = w.set_always_on_bottom(!on_top);
    crate::platform::pin_to_desktop(w, !on_top);
}

fn toggle_widget(app: &AppHandle) {
    let Some(w) = app.get_webview_window("widget") else { return };
    let visible = w.is_visible().unwrap_or(false);
    if visible {
        flush_widget_move(app);
    }
    let state = app.state::<AppState>();
    {
        let mut eng = lock(&state.engine);
        eng.settings.show_widget = !visible;
        eng.save();
    }
    if visible {
        let _ = w.hide();
    } else {
        show_widget(app);
    }
}

// ---------- commands ----------

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    lock(&state.snapshot).clone()
}

#[tauri::command]
fn refresh_now(app: AppHandle) -> Result<Snapshot, String> {
    do_refresh(&app)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.engine).settings.clone()
}

#[tauri::command]
fn get_home() -> String {
    dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default()
}

#[tauri::command]
fn calibrate(app: AppHandle, input: CalibrationInput) -> Result<Snapshot, String> {
    {
        let state = app.state::<AppState>();
        let mut eng = lock(&state.engine);
        let now = Utc::now();
        // Make sure the anchor is computed against fresh totals.
        let _ = eng.refresh(now);
        eng.calibrate(input, now)?;
    }
    Ok(republish(&app))
}

/// Turn live readings on or off. Switching on is refused unless a fetch with
/// Claude Code's credential actually works, so the toggle can never end up on
/// while silently doing nothing.
#[tauri::command]
fn set_live_readings(app: AppHandle, enabled: bool) -> Result<Snapshot, String> {
    {
        let state = app.state::<AppState>();
        let mut eng = lock(&state.engine);
        eng.set_live_readings(enabled)?;
        let _ = eng.refresh(Utc::now());
        eng.save();
    }
    Ok(republish(&app))
}

/// Try the whole path without committing to it — what the "Check" button in
/// settings and the first-run step both call.
#[tauri::command]
fn check_live_readings(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let eng = lock(&state.engine);
    eng.probe_live_readings()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    pub claude_dir: Option<String>,
    pub refresh_secs: Option<u64>,
    pub show_widget: Option<bool>,
    pub widget_on_top: Option<bool>,
    pub plan: Option<String>,
    pub boost: Option<String>,
    pub theme: Option<String>,
}

#[tauri::command]
fn update_settings(app: AppHandle, patch: SettingsPatch) -> Result<Snapshot, String> {
    let mut rescan = false;
    {
        let state = app.state::<AppState>();
        let mut eng = lock(&state.engine);
        if let Some(d) = patch.claude_dir {
            let d = d.trim().to_string();
            let new = if d.is_empty() { None } else { Some(std::path::PathBuf::from(d)) };
            if new != eng.settings.claude_dir {
                eng.settings.claude_dir = new;
                // Different transcripts: the index is meaningless now.
                eng.index = crate::ledger::Index::default();
                rescan = true;
            }
        }
        if let Some(s) = patch.refresh_secs {
            eng.settings.refresh_secs = s.clamp(15, 3600);
        }
        if let Some(v) = patch.show_widget {
            eng.settings.show_widget = v;
        }
        if let Some(v) = patch.widget_on_top {
            eng.settings.widget_on_top = v;
        }
        if let Some(p) = patch.plan {
            eng.settings.plan = p;
        }
        if let Some(b) = patch.boost {
            eng.settings.boost = if b.trim().is_empty() { None } else { Some(b) };
        }
        if let Some(t) = patch.theme {
            eng.settings.theme = if t.is_empty() { None } else { Some(t) };
        }
        eng.save();
    }
    if let Some(w) = app.get_webview_window("widget") {
        let state = app.state::<AppState>();
        let (want, on_top) = {
            let eng = lock(&state.engine);
            (eng.settings.show_widget, eng.settings.widget_on_top)
        };
        apply_widget_layer(&w, on_top);
        if want {
            show_widget(&app);
        } else {
            let _ = w.hide();
        }
    }
    if rescan {
        do_refresh(&app)
    } else {
        Ok(republish(&app))
    }
}

#[tauri::command]
fn open_main(app: AppHandle) {
    show_main(&app);
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_main(&app);
    let _ = app.emit("open-settings", ());
}

#[tauri::command]
fn hide_widget(app: AppHandle) {
    if let Some(w) = app.get_webview_window("widget") {
        flush_widget_move(&app);
        let _ = w.hide();
    }
}

/// The widget page measured itself; resize the OS window to match.
#[tauri::command]
fn set_widget_expanded(
    app: AppHandle,
    expanded: bool,
    width: f64,
    height: f64,
    scale: f64,
) -> Result<(), String> {
    let w = app.get_webview_window("widget").ok_or("no widget window")?;
    // The card is zoomed, so the bounds it may occupy scale with it.
    let k = scale.clamp(WIDGET_MIN_SCALE, WIDGET_MAX_SCALE);
    let h = height.clamp(WIDGET_H * k, 600.0 * k);
    let wd = width.clamp(WIDGET_MIN_W * k, WIDGET_MAX_W * k);
    w.set_size(tauri::Size::Logical(LogicalSize { width: wd, height: h }))
        .map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let mut eng = lock(&state.engine);
    let sf = w.scale_factor().unwrap_or(1.0);
    let (ox, oy) = crate::platform::desktop_offset(sf);
    let pos = w.outer_position().ok().map(|p| {
        let l: LogicalPosition<f64> = p.to_logical(sf);
        ((l.x + ox) as i32, (l.y + oy) as i32)
    });
    let cur = eng.settings.widget.clone();
    eng.settings.widget = Some(WidgetGeometry {
        x: pos.map(|p| p.0).or(cur.as_ref().map(|c| c.x)).unwrap_or(0),
        y: pos.map(|p| p.1).or(cur.as_ref().map(|c| c.y)).unwrap_or(0),
        expanded,
        scale: k,
    });
    let _ = eng.settings.save(&eng.settings_path);
    Ok(())
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, on: bool) -> Result<(), String> {
    let al = app.autolaunch();
    if on { al.enable() } else { al.disable() }.map_err(|e| e.to_string())
}

// ---------- app ----------

pub fn run() {
    let engine = Engine::open();
    let show_widget_at_start = engine.settings.show_widget;
    let calibrated = engine.settings.calibration.session.is_some();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // macOS reopens the running app; every other platform starts a second copy,
    // tray icon and all, unless the first one is told to come forward instead.
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }));
    }
    builder
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .manage(AppState {
            engine: Mutex::new(engine),
            snapshot: Mutex::new(None),
            widget_placed: AtomicBool::new(false),
            widget_move: Mutex::new(None),
            displays: Mutex::new(Displays { fingerprint: None, changed_at: Instant::now() }),
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            get_settings,
            get_home,
            calibrate,
            set_live_readings,
            check_live_readings,
            update_settings,
            open_main,
            open_settings,
            hide_widget,
            set_widget_expanded,
            get_autostart,
            set_autostart,
        ])
        .setup(move |app| {
            build_tray(app.handle())?;

            if show_widget_at_start {
                show_widget(app.handle());
            }
            // Opening the app from Spotlight or the Dock should put something
            // on screen. At login it should not: the tray owns the process.
            if !std::env::args().any(|a| a == "--autostart") {
                show_main(app.handle());
            }
            // First run: nothing is calibrated, so the widget can't show
            // percentages yet. Open the ledger with the calibration pane.
            if !calibrated {
                show_main(app.handle());
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(600));
                    let _ = h.emit("open-settings", ());
                });
            }

            // Screens can only be asked about from the main thread, so the
            // timer lives here and the work is dispatched back.
            let watcher = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(DISPLAY_POLL);
                let h = watcher.clone();
                if watcher.run_on_main_thread(move || poll_displays(&h)).is_err() {
                    return;
                }
            });

            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                let _ = do_refresh(&handle);
                let secs = {
                    let state = handle.state::<AppState>();
                    let s = lock(&state.engine).settings.refresh_secs;
                    s.clamp(15, 3600)
                };
                std::thread::sleep(Duration::from_secs(secs));
            });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                // Both windows hide; the tray owns the process lifetime.
                api.prevent_close();
                let _ = window.hide();
                if window.label() == "widget" {
                    let app = window.app_handle();
                    flush_widget_move(app);
                    let state = app.state::<AppState>();
                    let mut eng = lock(&state.engine);
                    eng.settings.show_widget = false;
                    eng.save();
                }
            }
            WindowEvent::Moved(pos) if window.label() == "widget" => {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                if !state.widget_placed.load(Ordering::Relaxed) {
                    return;
                }
                // Moved carries physical pixels; set_position takes points, so
                // storing raw pixels would double the position on every launch.
                let sf = window.scale_factor().unwrap_or(1.0);
                let p: LogicalPosition<f64> = pos.to_logical(sf);
                let (ox, oy) = crate::platform::desktop_offset(sf);
                // Held, not saved: the display watcher decides whether this was
                // the user or the window server shuffling windows about.
                *lock(&state.widget_move) = Some(PendingMove {
                    pos: LogicalPosition { x: p.x + ox, y: p.y + oy },
                    at: Instant::now(),
                });
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("error while running Token Ledger")
        .run(|_app, _event| {
            // Dock or Spotlight click while the app is already running: macOS
            // sends Reopen, and with every window hidden nothing would happen.
            // The variant only exists on macOS, so the arm has to as well.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_main(_app);
            }
        });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Token Ledger", true, None::<&str>)?;
    let widget = MenuItem::with_id(app, "widget", "Show / Hide Widget", true, None::<&str>)?;
    let recenter = MenuItem::with_id(app, "recenter", "Reset Widget Position", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh Now", true, None::<&str>)?;
    let calibrate = MenuItem::with_id(app, "calibrate", "Calibrate…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Token Ledger", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &widget, &recenter, &refresh, &calibrate, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    let mut tray = TrayIconBuilder::with_id("tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Token Ledger")
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "widget" => toggle_widget(app),
            "recenter" => reset_widget_position(app),
            "refresh" => {
                let _ = do_refresh(app);
            }
            "calibrate" => {
                show_main(app);
                let _ = app.emit("open-settings", ());
            }
            "quit" => {
                flush_widget_move(app);
                app.exit(0)
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_widget(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
