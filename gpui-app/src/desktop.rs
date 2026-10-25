//! Win32 pieces GPUI does not expose: topmost placement, show/hide without
//! activation, and full screen detection (same rules as `src-tauri/src/widget.rs`).
use gpui::{App, Bounds, Pixels, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    // Window has an inherent `window_handle()` (GPUI's id); this is the raw-window-handle one.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as _),
        _ => None,
    }
}

/// Moves the window to `bounds` (logical pixels) above other windows, or hides it,
/// without focusing it. Windows sends `WM_SIZE` synchronously, so like GPUI's own
/// `resize` this runs on the next turn of the executor, once the window is no longer
/// being updated; otherwise GPUI drops the resize and draws nothing at the new size.
#[cfg(windows)]
pub fn place(window: &Window, bounds: Bounds<Pixels>, visible: bool, cx: &App) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE,
    };
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let scale = window.scale_factor();
    let physical = |v: Pixels| (f32::from(v) * scale).round() as i32;
    let (x, y) = (physical(bounds.origin.x), physical(bounds.origin.y));
    let (width, height) = (physical(bounds.size.width), physical(bounds.size.height));
    cx.foreground_executor()
        .spawn(async move {
            unsafe {
                if visible {
                    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE);
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        })
        .detach();
}

#[cfg(not(windows))]
pub fn place(_: &Window, _: Bounds<Pixels>, _: bool, _: &App) {}

/// The primary display minus the taskbar, in logical pixels.
#[cfg(windows)]
pub fn work_area(scale: f32) -> Option<Bounds<Pixels>> {
    use gpui::{point, px, size};
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA},
    };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as _, 0) };
    (ok != 0).then(|| {
        let logical = |v: i32| px(v as f32 / scale);
        Bounds {
            origin: point(logical(r.left), logical(r.top)),
            size: size(logical(r.right - r.left), logical(r.bottom - r.top)),
        }
    })
}

#[cfg(not(windows))]
pub fn work_area(_: f32) -> Option<Bounds<Pixels>> {
    None
}

fn covers_monitor(client: (i32, i32, i32, i32), monitor: (i32, i32, i32, i32)) -> bool {
    client.0 <= monitor.0 + 1
        && client.1 <= monitor.1 + 1
        && client.2 >= monitor.2 - 1
        && client.3 >= monitor.3 - 1
}

/// No game hooks, process scanning, titles or keystroke recording.
/// None means a transient foreground switch: retain the previous state.
#[cfg(windows)]
pub fn fullscreen_active() -> Option<bool> {
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::{
            ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        UI::{
            Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN},
            WindowsAndMessaging::*,
        },
    };
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(foreground, &mut pid);
        if pid == std::process::id()
            || foreground == GetShellWindow()
            || foreground == GetDesktopWindow()
        {
            return Some(false);
        }
        let mut class = [0u16; 128];
        let length = GetClassNameW(foreground, class.as_mut_ptr(), class.len() as i32);
        if matches!(
            String::from_utf16_lossy(&class[..length.max(0) as usize]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return Some(false);
        }
        let mut state = 0;
        if SHQueryUserNotificationState(&mut state) >= 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN {
            return Some(true);
        }
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut client: RECT = std::mem::zeroed();
        let mut point = POINT { x: 0, y: 0 };
        if GetMonitorInfoW(
            MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        ) == 0
            || GetClientRect(foreground, &mut client) == 0
            || ClientToScreen(foreground, &mut point) == 0
        {
            return None;
        }
        let screen = monitor.rcMonitor;
        Some(covers_monitor(
            (
                point.x,
                point.y,
                point.x + client.right,
                point.y + client.bottom,
            ),
            (screen.left, screen.top, screen.right, screen.bottom),
        ))
    }
}

#[cfg(not(windows))]
pub fn fullscreen_active() -> Option<bool> {
    Some(false)
}
