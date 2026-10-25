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
                // Windows 11 outlines and rounds every top-level window, transparent or not;
                // the notch draws its own shape.
                use windows_sys::Win32::Graphics::Dwm::{
                    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
                    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
                };
                let corner = DWMWCP_DONOTROUND;
                let border = DWMWA_COLOR_NONE;
                DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &corner as *const _ as _, 4);
                DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR as u32, &border as *const _ as _, 4);
                // GPUI creates popups as overlapped windows, which keep a 1 px top frame line.
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, SWP_FRAMECHANGED, WS_POPUP,
                };
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                let popup = WS_POPUP as isize;
                let flags = if style & popup == 0 {
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style | popup);
                    SWP_NOACTIVATE | SWP_FRAMECHANGED
                } else {
                    SWP_NOACTIVATE
                };
                if visible {
                    // GPUI keeps an invisible frame around its windows on Windows 11 (a strip
                    // at the top where Windows paints a light line, and resize borders on the
                    // other sides), so the drawn area sits inside the window rect. Measure that
                    // frame, grow the window by it so the client area lands exactly on the
                    // requested bounds (flush against the screen edge), and clip the frame off.
                    use windows_sys::Win32::{
                        Foundation::{POINT, RECT},
                        Graphics::Gdi::{ClientToScreen, CreateRectRgn, SetWindowRgn},
                        UI::WindowsAndMessaging::{GetClientRect, GetWindowRect},
                    };
                    SetWindowPos(hwnd, HWND_TOPMOST, x, y, width, height, flags);
                    let mut outer = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    let mut origin = POINT { x: 0, y: 0 };
                    if GetWindowRect(hwnd, &mut outer) != 0
                        && GetClientRect(hwnd, &mut client) != 0
                        && ClientToScreen(hwnd, &mut origin) != 0
                    {
                        let left = origin.x - outer.left;
                        let top = origin.y - outer.top;
                        let right = outer.right - origin.x - client.right;
                        let bottom = outer.bottom - origin.y - client.bottom;
                        SetWindowPos(
                            hwnd,
                            HWND_TOPMOST,
                            x - left,
                            y - top,
                            width + left + right,
                            height + top + bottom,
                            SWP_NOACTIVATE,
                        );
                        SetWindowRgn(hwnd, CreateRectRgn(left, top, left + width, top + height), 1);
                    }
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

/// Whether the pointer is over the window's drawn area. Resizing a window under a still
/// pointer makes Windows report a mouse leave although the pointer never moved; this asks
/// for the real position instead.
#[cfg(windows)]
pub fn pointer_over(window: &Window) -> bool {
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, IsWindowVisible},
    };
    let Some(hwnd) = hwnd(window) else {
        return false;
    };
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut origin = POINT { x: 0, y: 0 };
        IsWindowVisible(hwnd) != 0
            && GetCursorPos(&mut cursor) != 0
            && GetClientRect(hwnd, &mut client) != 0
            && ClientToScreen(hwnd, &mut origin) != 0
            && cursor.x >= origin.x
            && cursor.x < origin.x + client.right
            && cursor.y >= origin.y
            && cursor.y < origin.y + client.bottom
    }
}

#[cfg(not(windows))]
pub fn pointer_over(window: &Window) -> bool {
    window.is_window_hovered()
}

#[cfg(windows)]
fn logical(r: windows_sys::Win32::Foundation::RECT, scale: f32) -> Bounds<Pixels> {
    use gpui::{point, px, size};
    let l = |v: i32| px(v as f32 / scale);
    Bounds { origin: point(l(r.left), l(r.top)), size: size(l(r.right - r.left), l(r.bottom - r.top)) }
}

/// A display's name (`\\.\DISPLAY2`, as Tauri reports it) and its area minus the taskbar.
#[cfg(windows)]
fn monitor_work(monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> Option<(String, windows_sys::Win32::Foundation::RECT)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFOEXW};
    unsafe {
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as _) == 0 {
            return None;
        }
        let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        Some((String::from_utf16_lossy(&info.szDevice[..end]), info.monitorInfo.rcWork))
    }
}

/// The named display minus the taskbar, in logical pixels; the primary display when it is
/// not connected (or none is named), so the notch can never end up out of reach.
#[cfg(windows)]
pub fn work_area(scale: f32, monitor: Option<&str>) -> Option<Bounds<Pixels>> {
    use windows_sys::Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR},
        UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA},
    };
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, found: LPARAM) -> i32 {
        unsafe { (*(found as *mut Vec<HMONITOR>)).push(monitor) };
        1
    }
    if let Some(name) = monitor {
        let mut monitors: Vec<HMONITOR> = vec![];
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut monitors as *mut _ as LPARAM) };
        if let Some((_, r)) = monitors.into_iter().filter_map(monitor_work).find(|(n, _)| n == name) {
            return Some(logical(r, scale));
        }
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as _, 0) };
    (ok != 0).then(|| logical(r, scale))
}

#[cfg(not(windows))]
pub fn work_area(_: f32, _: Option<&str>) -> Option<Bounds<Pixels>> {
    None
}

/// The mouse pointer, the work area of the display it is on and that display's name, in
/// logical pixels.
pub struct Pointer {
    pub at: gpui::Point<Pixels>,
    pub area: Bounds<Pixels>,
    pub monitor: String,
}

#[cfg(windows)]
pub fn pointer(scale: f32) -> Option<Pointer> {
    use gpui::{point, px};
    use windows_sys::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST},
        UI::WindowsAndMessaging::GetCursorPos,
    };
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return None;
        }
        let (monitor, work) = monitor_work(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST))?;
        Some(Pointer {
            at: point(px(cursor.x as f32 / scale), px(cursor.y as f32 / scale)),
            area: logical(work, scale),
            monitor,
        })
    }
}

#[cfg(not(windows))]
pub fn pointer(_: f32) -> Option<Pointer> {
    None
}

/// Whether the left mouse button is held, wherever the pointer is.
#[cfg(windows)]
pub fn left_button_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 }
}

#[cfg(not(windows))]
pub fn left_button_down() -> bool {
    false
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

/// The identifier the installer's shortcuts carry, which Windows needs to show a toast.
#[cfg(windows)]
const APP_ID: &str = "dev.kev.smashtoken";

/// A Windows notification. Installed builds have a Start menu shortcut carrying `APP_ID`;
/// a build run from elsewhere borrows PowerShell's so the toast still shows.
#[cfg(windows)]
pub fn toast(title: &str, body: &str) {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("uninstall.exe").exists()))
        .unwrap_or(false);
    let id = if installed { APP_ID } else { tauri_winrt_notification::Toast::POWERSHELL_APP_ID };
    let _ = tauri_winrt_notification::Toast::new(id).title(title).text1(body).show();
}

#[cfg(not(windows))]
pub fn toast(_: &str, _: &str) {}
