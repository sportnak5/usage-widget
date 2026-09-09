//! The Tauri shell: two windows, a tray icon, a refresh loop, and the
//! commands the pages call. All logic lives in `engine`; this file only
//! wires it to the OS.

use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

use crate::engine::{CalibrationInput, Engine};
use crate::ledger::Snapshot;
use crate::settings::{Settings, WidgetGeometry};

pub struct AppState {
    engine: Mutex<Engine>,
    snapshot: Mutex<Option<Snapshot>>,
}

const WIDGET_MIN_W: f64 = 380.0;
const WIDGET_MAX_W: f64 = 640.0;
const WIDGET_H: f64 = 84.0;

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
    let geom = lock(&state.engine).settings.widget.clone();
    if let Some(g) = geom {
        let _ = w.set_position(tauri::Position::Physical(PhysicalPosition { x: g.x, y: g.y }));
    }
    let _ = w.show();
}

fn toggle_widget(app: &AppHandle) {
    let Some(w) = app.get_webview_window("widget") else { return };
    let visible = w.is_visible().unwrap_or(false);
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

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SettingsPatch {
    pub claude_dir: Option<String>,
    pub refresh_secs: Option<u64>,
    pub show_widget: Option<bool>,
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
        let want = lock(&state.engine).settings.show_widget;
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
        let _ = w.hide();
    }
}

/// The widget page measured itself; resize the OS window to match.
#[tauri::command]
fn set_widget_expanded(app: AppHandle, expanded: bool, width: f64, height: f64) -> Result<(), String> {
    let w = app.get_webview_window("widget").ok_or("no widget window")?;
    let h = height.clamp(WIDGET_H, 600.0);
    let wd = width.clamp(WIDGET_MIN_W, WIDGET_MAX_W);
    w.set_size(tauri::Size::Logical(LogicalSize { width: wd, height: h }))
        .map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    let mut eng = lock(&state.engine);
    let pos = w.outer_position().ok();
    let cur = eng.settings.widget.clone();
    eng.settings.widget = Some(WidgetGeometry {
        x: pos.map(|p| p.x).or(cur.as_ref().map(|c| c.x)).unwrap_or(0),
        y: pos.map(|p| p.y).or(cur.as_ref().map(|c| c.y)).unwrap_or(0),
        expanded,
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

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--autostart"])))
        .manage(AppState { engine: Mutex::new(engine), snapshot: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            get_settings,
            get_home,
            calibrate,
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
                    let state = app.state::<AppState>();
                    let mut eng = lock(&state.engine);
                    eng.settings.show_widget = false;
                    eng.save();
                }
            }
            WindowEvent::Moved(pos) if window.label() == "widget" => {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                let mut eng = lock(&state.engine);
                let expanded = eng.settings.widget.as_ref().map(|g| g.expanded).unwrap_or(false);
                eng.settings.widget = Some(WidgetGeometry { x: pos.x, y: pos.y, expanded });
                let _ = eng.settings.save(&eng.settings_path);
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Token Ledger");
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Token Ledger", true, None::<&str>)?;
    let widget = MenuItem::with_id(app, "widget", "Show / Hide Widget", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh Now", true, None::<&str>)?;
    let calibrate = MenuItem::with_id(app, "calibrate", "Calibrate…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Token Ledger", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &widget, &refresh, &calibrate, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    let mut tray = TrayIconBuilder::with_id("tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Token Ledger")
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => show_main(app),
            "widget" => toggle_widget(app),
            "refresh" => {
                let _ = do_refresh(app);
            }
            "calibrate" => {
                show_main(app);
                let _ = app.emit("open-settings", ());
            }
            "quit" => app.exit(0),
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
