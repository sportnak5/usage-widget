//! Per-OS window plumbing that Tauri does not expose.
//!
//! Only the widget needs it: "pinned to the desktop" means something different
//! on each system, and neither spelling is reachable through the cross-platform
//! API. Everything here degrades to a no-op if the OS declines.

/// Make the widget part of the desktop rather than another app window: it stays
/// where it is when the desktop is revealed, and follows you between desktops.
#[cfg(target_os = "macos")]
pub fn pin_to_desktop(w: &tauri::WebviewWindow, pinned: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const MOVE_TO_ACTIVE_SPACE: usize = 1 << 1;
    const STATIONARY: usize = 1 << 4;
    const IGNORES_CYCLE: usize = 1 << 6;

    let Ok(ptr) = w.ns_window() else { return };
    if ptr.is_null() {
        return;
    }
    // `stationary` is the flag Mission Control and Show Desktop honour: a
    // window carrying it is left alone while the app windows slide aside.
    let behavior = if pinned {
        CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE
    } else {
        MOVE_TO_ACTIVE_SPACE
    };
    // Safety: `ns_window` hands back this window's live NSWindow, and
    // -setCollectionBehavior: takes one NSUInteger.
    unsafe {
        let ns = ptr as *mut AnyObject;
        let _: () = msg_send![ns, setCollectionBehavior: behavior];
    }
}

#[cfg(target_os = "windows")]
pub fn pin_to_desktop(w: &tauri::WebviewWindow, pinned: bool) {
    let Ok(hwnd) = w.hwnd() else { return };
    win::pin(hwnd.0 as _, pinned);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pin_to_desktop(_w: &tauri::WebviewWindow, _pinned: bool) {}

/// Offset between screen coordinates and the coordinates the widget window
/// actually takes, in logical points. Zero everywhere except a Windows widget
/// parented to the desktop, whose position is relative to the virtual screen.
#[cfg(target_os = "windows")]
pub fn desktop_offset(scale_factor: f64) -> (f64, f64) {
    let (x, y) = win::virtual_origin();
    (x as f64 / scale_factor, y as f64 / scale_factor)
}

#[cfg(not(target_os = "windows"))]
pub fn desktop_offset(_scale_factor: f64) -> (f64, f64) {
    (0.0, 0.0)
}

#[cfg(target_os = "windows")]
mod win {
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowExW, FindWindowW, GetSystemMetrics, SendMessageTimeoutW, SetParent,
        SMTO_NORMAL, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    /// Whether the widget is currently a child of the desktop's WorkerW, which
    /// is what makes its position parent-relative.
    static PARENTED: AtomicBool = AtomicBool::new(false);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Explorer keeps the wallpaper in a WorkerW window that sits behind the
    /// desktop icons. It only exists once Progman has been nudged with the
    /// undocumented 0x052C message; after that it is the WorkerW *following*
    /// the window that owns SHELLDLL_DefView.
    unsafe fn worker_w() -> HWND {
        let progman = FindWindowW(wide("Progman").as_ptr(), null());
        if progman.is_null() {
            return null_mut();
        }
        let mut ignored: usize = 0;
        // Two spellings of the same nudge: the second is what Windows 10 1903
        // and later answer to.
        SendMessageTimeoutW(progman, 0x052C, 0, 0, SMTO_NORMAL, 1000, &mut ignored);
        SendMessageTimeoutW(progman, 0x052C, 0x0D, 0x01, SMTO_NORMAL, 1000, &mut ignored);

        let mut found: HWND = null_mut();
        EnumWindows(Some(find_worker), &mut found as *mut HWND as LPARAM);
        found
    }

    unsafe extern "system" fn find_worker(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let defview = FindWindowExW(hwnd, null_mut(), wide("SHELLDLL_DefView").as_ptr(), null());
        if !defview.is_null() {
            let worker = FindWindowExW(null_mut(), hwnd, wide("WorkerW").as_ptr(), null());
            if !worker.is_null() {
                *(lparam as *mut HWND) = worker;
                return 0; // stop enumerating
            }
        }
        1
    }

    pub fn pin(hwnd: HWND, pinned: bool) {
        if hwnd.is_null() {
            return;
        }
        unsafe {
            if pinned {
                let worker = worker_w();
                // No WorkerW (a locked-down shell, or a future Explorer): leave
                // the window top-level. always_on_bottom still applies.
                // SetParent returns the *previous* parent, which is null for a
                // top-level window, so its return says nothing about success.
                if !worker.is_null() {
                    SetParent(hwnd, worker);
                    PARENTED.store(true, Ordering::Relaxed);
                }
            } else if PARENTED.swap(false, Ordering::Relaxed) {
                SetParent(hwnd, null_mut());
            }
        }
    }

    /// Top-left of the virtual screen in physical pixels — the origin the
    /// widget's coordinates are relative to once it is parented.
    pub fn virtual_origin() -> (i32, i32) {
        if !PARENTED.load(Ordering::Relaxed) {
            return (0, 0);
        }
        unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            )
        }
    }
}
