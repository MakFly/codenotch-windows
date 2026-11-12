//! Overlay windows and the Win32 bits gpui does not expose.
//!
//! * [`overlay_options`] / [`open_overlay`]: transparent, borderless, non-focused `PopUp`
//!   window (what the 0.3 notch used).
//! * [`apply_overlay_styles`]: `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, topmost, no taskbar /
//!   Alt-Tab entry. Call once after the window exists (e.g. first render).
//! * [`move_window`]: `SetWindowPos(HWND_TOPMOST, …, SWP_NOACTIVATE)` in logical px of the
//!   window's current monitor. Exact device-pixel variants: [`move_window_device`].
//! * [`monitors`]: every monitor with device name, bounds, work area and scale (for placement
//!   and the settings "Screen" list). Lives here, not in `codenotch-platform`, because UI crates
//!   may not depend on platform.
//!
//! All functions are UI-thread only and are no-ops (or `None`) off Windows.

use gpui::{
    point, px, size, App, Bounds, Entity, Pixels, Render, Window, WindowBackgroundAppearance,
    WindowBounds, WindowHandle, WindowKind, WindowOptions,
};

/// Windows "Animation effects" is off (Settings › Accessibility › Visual effects): UI
/// transitions should then be instant. `false` off Windows.
pub fn reduced_motion() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION,
        };
        let mut on: i32 = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into `on`.
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                &mut on as *mut i32 as *mut core::ffi::c_void,
                0,
            )
        };
        ok != 0 && on == 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Rectangle in physical (device) pixels, virtual-screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl DeviceRect {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    /// Convert to logical px with `scale` (device px per logical px).
    pub fn to_logical(&self, scale: f32) -> Bounds<Pixels> {
        let s = if scale > 0.0 { scale } else { 1.0 };
        Bounds {
            origin: point(px(self.x as f32 / s), px(self.y as f32 / s)),
            size: size(px(self.w as f32 / s), px(self.h as f32 / s)),
        }
    }
    /// From logical px with `scale`.
    pub fn from_logical(b: Bounds<Pixels>, scale: f32) -> Self {
        let f = |p: Pixels| (f32::from(p) * scale).round() as i32;
        Self {
            x: f(b.origin.x),
            y: f(b.origin.y),
            w: f(b.size.width).max(1),
            h: f(b.size.height).max(1),
        }
    }
}

/// One monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// Device name, e.g. `\\.\DISPLAY2` (what `widgetPlacement.monitor` stores).
    pub name: String,
    pub bounds: DeviceRect,
    /// Bounds minus the taskbar.
    pub work: DeviceRect,
    /// Device px per logical px (DPI / 96).
    pub scale: f32,
    pub primary: bool,
}

/// Options for a transparent, borderless, non-focused PopUp window at `bounds` (logical px).
pub fn overlay_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    }
}

/// Open an overlay window and apply [`apply_overlay_styles`]. Logs and returns `None` on failure.
pub fn open_overlay<V: Render + 'static>(
    cx: &mut App,
    bounds: Bounds<Pixels>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Option<WindowHandle<V>> {
    match cx.open_window(overlay_options(bounds), |window, cx| {
        apply_overlay_styles(window);
        build(window, cx)
    }) {
        Ok(handle) => Some(handle),
        Err(err) => {
            log::error!("cannot open overlay window: {err}");
            None
        }
    }
}

/// Raw HWND of a gpui window (as an integer), Windows only.
pub fn hwnd(window: &Window) -> Option<isize> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Maximise the window, or restore it when it is maximised (caption button, title-bar
/// double-click). gpui 0.2.2's `zoom_window` only maximises on Windows.
pub fn toggle_maximize(window: &Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            IsZoomed, ShowWindowAsync, SW_MAXIMIZE, SW_RESTORE,
        };
        let h = h as windows_sys::Win32::Foundation::HWND;
        // SAFETY: `h` is this window's live HWND; both calls only read/post to it.
        unsafe {
            let cmd = if IsZoomed(h) != 0 {
                SW_RESTORE
            } else {
                SW_MAXIMIZE
            };
            ShowWindowAsync(h, cmd);
        }
        return;
    }
    window.zoom_window();
}

/// [`bring_to_front`] for a gpui window.
pub fn bring_window_to_front(window: &Window) -> bool {
    match hwnd(window) {
        Some(h) => bring_to_front(h),
        None => {
            window.activate_window();
            false
        }
    }
}

/// Restore (if minimised), raise and activate a top-level window of this process, working
/// around the Windows foreground lock.
///
/// `SetForegroundWindow` is refused unless this process is allowed to change the foreground
/// (it received the last input event, owns the foreground window, …). Our notch is
/// `WS_EX_NOACTIVATE`: clicking it leaves another app in the foreground, so a plain
/// `SetForegroundWindow` (or gpui's `activate_window`, which also runs it later on a task)
/// often only flashes the taskbar button. Steps, each logged at debug level:
/// 1. `AllowSetForegroundWindow(ASFW_ANY)`; `SW_RESTORE` when minimised (`SW_SHOW` when hidden);
/// 2. attach our input queue to the foreground window's thread (`AttachThreadInput`), then
///    `BringWindowToTop` + `SetForegroundWindow` + `SetActiveWindow` + `SetFocus`, detach;
/// 3. only if that failed: the Alt-key unlock (`keybd_event(VK_MENU)` down, `SetForegroundWindow`,
///    up — the release reaches our window, so the previous app does not enter menu mode);
/// 4. last resort: `HWND_TOPMOST` then `HWND_NOTOPMOST` so it is at least visible on top.
///
/// Call it synchronously on the UI thread while handling the user's action. Returns true when
/// the window ends up as the foreground window. No-op (`false`) off Windows.
pub fn bring_to_front(hwnd: isize) -> bool {
    #[cfg(windows)]
    {
        front::bring_to_front(hwnd)
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        false
    }
}

#[cfg(windows)]
mod front {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        keybd_event, SetActiveWindow, SetFocus, KEYEVENTF_KEYUP, VK_MENU,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId,
        IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow, SetWindowPos, ShowWindow,
        ASFW_ANY, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_RESTORE,
        SW_SHOW,
    };

    pub fn bring_to_front(raw: isize) -> bool {
        let h = raw as HWND;
        // SAFETY: plain user32 calls on a window handle we own (checked with IsWindow first);
        // AttachThreadInput is always paired with a detach below.
        unsafe {
            if IsWindow(h) == 0 {
                log::debug!("bring_to_front: {raw:#x} is not a window");
                return false;
            }
            let allowed = AllowSetForegroundWindow(ASFW_ANY) != 0;
            log::debug!("bring_to_front: AllowSetForegroundWindow(ASFW_ANY) = {allowed}");
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
                log::debug!("bring_to_front: restored from minimised");
            } else if IsWindowVisible(h) == 0 {
                ShowWindow(h, SW_SHOW);
                log::debug!("bring_to_front: shown");
            }
            let fg = GetForegroundWindow();
            if fg == h {
                BringWindowToTop(h);
                log::debug!("bring_to_front: already the foreground window");
                return true;
            }

            // Step 2: share the foreground thread's input state while we switch.
            let ours = GetCurrentThreadId();
            let theirs = if fg.is_null() {
                0
            } else {
                GetWindowThreadProcessId(fg, std::ptr::null_mut())
            };
            let attached = theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, 1) != 0;
            let top = BringWindowToTop(h) != 0;
            let set = SetForegroundWindow(h) != 0;
            SetActiveWindow(h);
            SetFocus(h);
            if attached {
                AttachThreadInput(ours, theirs, 0);
            }
            let ok = GetForegroundWindow() == h;
            log::debug!(
                "bring_to_front: attach={attached} BringWindowToTop={top} \
                 SetForegroundWindow={set} -> foreground={ok}"
            );
            if ok {
                return true;
            }

            // Step 3: the Alt-key unlock (synthetic input makes us the last input's owner).
            keybd_event(VK_MENU as u8, 0, 0, 0);
            let set = SetForegroundWindow(h) != 0;
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            SetActiveWindow(h);
            SetFocus(h);
            let ok = GetForegroundWindow() == h;
            log::debug!("bring_to_front: alt unlock SetForegroundWindow={set} -> foreground={ok}");
            if ok {
                return true;
            }

            // Step 4: at least on top of the z-order.
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW;
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, flags);
            SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, flags);
            let ok = GetForegroundWindow() == h;
            log::debug!("bring_to_front: topmost toggle -> foreground={ok}");
            ok
        }
    }
}

/// Tool window, never activated, topmost, hidden from the taskbar and Alt-Tab.
pub fn apply_overlay_styles(window: &Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let h = h as windows_sys::Win32::Foundation::HWND;
        unsafe {
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let ex = (ex | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST) & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);
            SetWindowPos(
                h,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Let mouse events pass through the window (`WS_EX_TRANSPARENT | WS_EX_LAYERED`) or not.
/// Untested with gpui's DirectX swap chain: verify on the desktop before relying on it.
pub fn set_click_through(window: &Window, through: bool) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let h = h as windows_sys::Win32::Foundation::HWND;
        unsafe {
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let ex = if through {
                ex | WS_EX_TRANSPARENT | WS_EX_LAYERED
            } else {
                ex & !WS_EX_TRANSPARENT
            };
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex as isize);
        }
    }
    #[cfg(not(windows))]
    let _ = (window, through);
}

/// Re-assert topmost without moving (after another app grabbed the top band).
pub fn keep_topmost(window: &Window) {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            SetWindowPos(
                h as windows_sys::Win32::Foundation::HWND,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Move + resize in logical px of the window's current monitor (uses `window.scale_factor()`).
pub fn move_window(window: &mut Window, bounds: Bounds<Pixels>) {
    let rect = DeviceRect::from_logical(bounds, window.scale_factor());
    if !move_window_device(window, rect) {
        window.resize(bounds.size);
    }
}

/// Move + resize in device px. Returns false when not supported (non-Windows).
pub fn move_window_device(window: &Window, rect: DeviceRect) -> bool {
    #[cfg(windows)]
    if let Some(h) = hwnd(window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            SetWindowPos(
                h as windows_sys::Win32::Foundation::HWND,
                HWND_TOPMOST,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                SWP_NOACTIVATE,
            );
        }
        return true;
    }
    let _ = (window, rect);
    false
}

/// Monitor the window is on.
pub fn window_monitor(window: &Window) -> Option<MonitorInfo> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
        let h = hwnd(window)?;
        let m = unsafe {
            MonitorFromWindow(
                h as windows_sys::Win32::Foundation::HWND,
                MONITOR_DEFAULTTONEAREST,
            )
        };
        win::monitor_info(m)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// Work area of the window's monitor in logical px (same space as [`move_window`]).
pub fn work_area(window: &Window) -> Option<Bounds<Pixels>> {
    let m = window_monitor(window)?;
    Some(m.work.to_logical(window.scale_factor()))
}

/// All monitors (primary first).
pub fn monitors() -> Vec<MonitorInfo> {
    #[cfg(windows)]
    {
        let mut v = win::enumerate();
        v.sort_by_key(|m| !m.primary);
        v
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// Monitor by device name, else the primary one.
pub fn monitor_or_primary(name: &str) -> Option<MonitorInfo> {
    let all = monitors();
    all.iter()
        .find(|m| !name.is_empty() && m.name == name)
        .or_else(|| all.iter().find(|m| m.primary))
        .or(all.first())
        .cloned()
}

#[cfg(windows)]
mod win {
    use super::{DeviceRect, MonitorInfo};
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    fn rect(r: RECT) -> DeviceRect {
        DeviceRect {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
        }
    }

    pub fn monitor_info(m: HMONITOR) -> Option<MonitorInfo> {
        if m.is_null() {
            return None;
        }
        unsafe {
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
                return None;
            }
            let len = info
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(info.szDevice.len());
            let name = String::from_utf16_lossy(&info.szDevice[..len]);
            let (mut dx, mut dy) = (96u32, 96u32);
            let scale = if GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) == 0 {
                dx as f32 / 96.0
            } else {
                1.0
            };
            Some(MonitorInfo {
                name,
                bounds: rect(info.monitorInfo.rcMonitor),
                work: rect(info.monitorInfo.rcWork),
                scale,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            })
        }
    }

    unsafe extern "system" fn collect(
        m: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> windows_sys::core::BOOL {
        let list = &mut *(data as *mut Vec<MonitorInfo>);
        if let Some(info) = monitor_info(m) {
            list.push(info);
        }
        1
    }

    pub fn enumerate() -> Vec<MonitorInfo> {
        let mut list: Vec<MonitorInfo> = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect),
                &mut list as *mut _ as LPARAM,
            );
        }
        list
    }
}

/// A normal window's default rectangle (logical px): `want`, shrunk to fit `work` (a work area,
/// device px, with `scale` device px per logical px) minus `margin` on every side, never below
/// `min`, and centered in the work area.
pub fn fit_centered(
    work: DeviceRect,
    scale: f32,
    want: (f32, f32),
    min: (f32, f32),
    margin: f32,
) -> Bounds<Pixels> {
    let area = work.to_logical(scale);
    let (aw, ah) = (f32::from(area.size.width), f32::from(area.size.height));
    let w = want.0.min(aw - 2.0 * margin).max(min.0).round();
    let h = want.1.min(ah - 2.0 * margin).max(min.1).round();
    let x = f32::from(area.origin.x) + ((aw - w) / 2.0).max(0.0);
    let y = f32::from(area.origin.y) + ((ah - h) / 2.0).max(0.0);
    Bounds {
        origin: point(px(x.round()), px(y.round())),
        size: size(px(w), px(h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centered_keeps_size_when_it_fits() {
        // 2560 x 1392 work area at 100 %: 1120 x 900 fits, centered.
        let work = DeviceRect {
            x: 0,
            y: 0,
            w: 2560,
            h: 1392,
        };
        let b = fit_centered(work, 1.0, (1120.0, 900.0), (800.0, 560.0), 24.0);
        assert_eq!(b.size, size(px(1120.0), px(900.0)));
        assert_eq!(b.origin, point(px(720.0), px(246.0)));
    }

    #[test]
    fn fit_centered_shrinks_to_the_work_area() {
        // 1920 x 1032 device px at 150 % = 1280 x 688 logical: height clamps to 688 - 48.
        let work = DeviceRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1032,
        };
        let b = fit_centered(work, 1.5, (1120.0, 900.0), (800.0, 560.0), 24.0);
        assert_eq!(b.size, size(px(1120.0), px(640.0)));
        assert_eq!(b.origin, point(px(80.0), px(24.0)));
        // Never below the minimum, even on a tiny work area.
        let tiny = DeviceRect {
            x: 1920,
            y: 0,
            w: 800,
            h: 600,
        };
        let b = fit_centered(tiny, 1.0, (1120.0, 900.0), (800.0, 560.0), 24.0);
        assert_eq!(b.size, size(px(800.0), px(560.0)));
        assert_eq!(b.origin, point(px(1920.0), px(20.0)));
    }

    #[test]
    fn device_rect_round_trip() {
        let r = DeviceRect {
            x: 3000,
            y: 100,
            w: 105,
            h: 300,
        };
        let l = r.to_logical(1.5);
        assert_eq!(DeviceRect::from_logical(l, 1.5), r);
        assert!(r.contains(3000, 100) && !r.contains(3105, 100));
    }

    #[test]
    fn monitors_enumerate_headless() {
        // Reads monitor info only; works without a window.
        let all = monitors();
        if let Some(first) = all.first() {
            assert!(first.scale > 0.0);
            assert!(first.work.w <= first.bounds.w);
        }
    }
}
