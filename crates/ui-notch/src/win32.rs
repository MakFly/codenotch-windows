//! Win32 bits the notch needs that ui-kit does not have (candidates to move into
//! `codenotch_ui_kit::window`). All UI-thread only, no-ops off Windows.
//!
//! **Re-entrancy:** every call that makes Windows send messages to a gpui window synchronously
//! (move, resize, region, show, menus) must run *outside* gpui's `App` borrow — i.e. from a
//! spawned task, never from `render` or an event handler — or gpui misses the resulting
//! `WM_SIZE`/`WM_MOVE` (its window callbacks use `try_borrow_mut`). See [`WinOps`].

use codenotch_ui_kit::window::DeviceRect;

/// Pending window operations, applied together from a spawned task.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WinOps {
    pub place: Option<DeviceRect>,
    /// `Some(rects)` sets the window region to the union of `rects` (window coords; empty =
    /// nothing hit, nothing drawn). `None` leaves it alone.
    pub region: Option<Vec<DeviceRect>>,
    pub show: Option<bool>,
    pub styles: bool,
    pub topmost: bool,
}

impl WinOps {
    pub fn is_empty(&self) -> bool {
        *self == WinOps::default()
    }

    /// Apply to `hwnd` (from `codenotch_ui_kit::window::hwnd`).
    pub fn apply(&self, hwnd: isize) {
        if self.styles {
            apply_styles(hwnd);
        }
        if let Some(r) = self.place {
            place(hwnd, r);
        } else if self.topmost {
            topmost(hwnd);
        }
        if let Some(region) = &self.region {
            set_region(hwnd, region);
        }
        if let Some(show) = self.show {
            set_visible(hwnd, show);
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::{DeviceRect, Frame};
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, CreateRectRgn, SetWindowRgn};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_LBUTTON, VK_MENU, VK_RBUTTON,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn h(hwnd: isize) -> HWND {
        hwnd as HWND
    }

    pub fn apply_styles(hwnd: isize) {
        unsafe {
            let ex = GetWindowLongPtrW(h(hwnd), GWL_EXSTYLE) as u32;
            let ex = (ex | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST) & !WS_EX_APPWINDOW;
            SetWindowLongPtrW(h(hwnd), GWL_EXSTYLE, ex as isize);
            SetWindowPos(
                h(hwnd),
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }

    pub fn window_rect(hwnd: isize) -> Option<DeviceRect> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        (unsafe { GetWindowRect(h(hwnd), &mut r) } != 0).then(|| DeviceRect {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
        })
    }

    /// Client area in screen coordinates (what gpui draws into).
    pub fn client_rect(hwnd: isize) -> Option<DeviceRect> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut p = POINT { x: 0, y: 0 };
        (unsafe { GetClientRect(h(hwnd), &mut r) != 0 && ClientToScreen(h(hwnd), &mut p) != 0 })
            .then(|| DeviceRect {
                x: p.x,
                y: p.y,
                w: r.right - r.left,
                h: r.bottom - r.top,
            })
    }

    /// gpui's invisible frame around the client area (zero when it cannot be measured).
    pub fn frame(hwnd: isize) -> Frame {
        match (window_rect(hwnd), client_rect(hwnd)) {
            (Some(w), Some(c)) => Frame::between(w, c),
            _ => Frame::default(),
        }
    }

    /// Move + resize so the **client area** (gpui's drawing surface) is `r`, topmost, no
    /// activation. Crossing to a monitor with another DPI makes gpui apply Windows' suggested
    /// rect (`WM_DPICHANGED`) and changes the frame; we then place again once.
    pub fn place(hwnd: isize, r: DeviceRect) {
        for _ in 0..2 {
            let o = frame(hwnd).outer(r);
            unsafe {
                SetWindowPos(h(hwnd), HWND_TOPMOST, o.x, o.y, o.w, o.h, SWP_NOACTIVATE);
            }
            if client_rect(hwnd) == Some(r) {
                return;
            }
        }
    }

    pub fn topmost(hwnd: isize) {
        unsafe {
            SetWindowPos(
                h(hwnd),
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Clip hit-testing (and drawing) to the union of `rects` (client coordinates, shifted
    /// here into the window coordinates `SetWindowRgn` expects). Points outside go to the
    /// windows below, which is how empty parts of our windows stay click-through.
    pub fn set_region(hwnd: isize, rects: &[DeviceRect]) {
        use windows_sys::Win32::Graphics::Gdi::{CombineRgn, DeleteObject, RGN_OR};
        let f = frame(hwnd);
        unsafe {
            let rgn = CreateRectRgn(0, 0, 0, 0);
            if rgn.is_null() {
                return;
            }
            for r in rects.iter().map(|r| f.to_window(*r)) {
                let part = CreateRectRgn(r.x, r.y, r.right(), r.bottom());
                if !part.is_null() {
                    CombineRgn(rgn, rgn, part, RGN_OR);
                    DeleteObject(part);
                }
            }
            // The system owns the region after a successful call.
            if SetWindowRgn(h(hwnd), rgn, 1) == 0 {
                DeleteObject(rgn);
            }
        }
    }

    pub fn set_visible(hwnd: isize, show: bool) {
        unsafe {
            let visible = IsWindowVisible(h(hwnd)) != 0;
            if show && !visible {
                ShowWindow(h(hwnd), SW_SHOWNOACTIVATE);
                SetWindowPos(
                    h(hwnd),
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else if !show && visible {
                ShowWindow(h(hwnd), SW_HIDE);
            }
        }
    }

    pub fn cursor_pos() -> Option<(i32, i32)> {
        let mut p = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
    }

    /// Is the pointer inside `hwnd`'s visible shape (window rect clipped by its window region)?
    /// Geometry only (another window on top does not count). `None` when the cursor position
    /// cannot be read (secure desktop…).
    pub fn cursor_in_window(hwnd: isize) -> Option<bool> {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateRectRgn, DeleteObject, GetWindowRgn, PtInRegion, RGN_ERROR,
        };
        let (x, y) = cursor_pos()?;
        let r = window_rect(hwnd)?;
        unsafe {
            if IsWindowVisible(h(hwnd)) == 0 {
                return Some(false);
            }
            if x < r.x || y < r.y || x >= r.right() || y >= r.bottom() {
                return Some(false);
            }
            let rgn = CreateRectRgn(0, 0, 0, 0);
            if rgn.is_null() {
                return Some(true);
            }
            let inside = if GetWindowRgn(h(hwnd), rgn) == RGN_ERROR {
                true // no window region: the whole rect
            } else {
                PtInRegion(rgn, x - r.x, y - r.y) != 0
            };
            DeleteObject(rgn);
            Some(inside)
        }
    }

    /// The physical primary button is down (honours swapped buttons).
    pub fn primary_button_down() -> bool {
        unsafe {
            let swapped = GetSystemMetrics(SM_SWAPBUTTON) != 0;
            let vk = if swapped { VK_RBUTTON } else { VK_LBUTTON };
            (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0
        }
    }

    pub fn alt_down() -> bool {
        unsafe { (GetAsyncKeyState(VK_MENU as i32) as u16 & 0x8000) != 0 }
    }

    /// Windows "Animation effects" off.
    pub fn reduced_motion() -> bool {
        let mut on: i32 = 1;
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

    /// Short time pattern of the user's locale ("HH:mm", "h:mm tt").
    pub fn short_time_pattern() -> Option<String> {
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
        };
        let key: Vec<u16> = "Control Panel\\International\0".encode_utf16().collect();
        let val: Vec<u16> = "sShortTime\0".encode_utf16().collect();
        let mut buf = [0u16; 64];
        let mut len = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                val.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }

    pub fn beep() {
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONASTERISK);
        }
    }

    /// Ask uxtheme for dark (or light) native menus. Undocumented ordinals 135
    /// (`SetPreferredAppMode`) and 136 (`FlushMenuThemes`), as most Win32 apps do; silently
    /// skipped when missing.
    fn menu_theme(dark: bool) {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        unsafe {
            let name: Vec<u16> = "uxtheme.dll\0".encode_utf16().collect();
            let lib = LoadLibraryW(name.as_ptr());
            if lib.is_null() {
                return;
            }
            if let Some(f) = GetProcAddress(lib, 135usize as *const u8) {
                let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                // 2 = ForceDark, 3 = ForceLight
                set_mode(if dark { 2 } else { 3 });
            }
            if let Some(f) = GetProcAddress(lib, 136usize as *const u8) {
                let flush: unsafe extern "system" fn() = std::mem::transmute(f);
                flush();
            }
        }
    }

    /// Native popup menu at screen point `(x, y)`; returns the chosen id. Modal: call it from a
    /// spawned task, never inside a gpui update.
    pub fn popup_menu(
        hwnd: isize,
        items: &[super::MenuItem],
        x: i32,
        y: i32,
        dark: bool,
    ) -> Option<u32> {
        menu_theme(dark);
        unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return None;
            }
            for item in items {
                match item {
                    super::MenuItem::Separator => {
                        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                    }
                    super::MenuItem::Item {
                        id,
                        label,
                        checked,
                        enabled,
                    } => {
                        let text: Vec<u16> =
                            label.encode_utf16().chain(std::iter::once(0)).collect();
                        let mut flags = MF_STRING;
                        if *checked {
                            flags |= MF_CHECKED;
                        }
                        if !*enabled {
                            flags |= MF_GRAYED;
                        }
                        AppendMenuW(menu, flags, *id as usize, text.as_ptr());
                    }
                }
            }
            let previous = GetForegroundWindow();
            SetForegroundWindow(h(hwnd));
            let chosen = TrackPopupMenuEx(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
                x,
                y,
                h(hwnd),
                std::ptr::null(),
            );
            PostMessageW(h(hwnd), WM_NULL, 0, 0);
            DestroyMenu(menu);
            if !previous.is_null() && previous != h(hwnd) {
                SetForegroundWindow(previous);
            }
            (chosen > 0).then_some(chosen as u32)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DeviceRect;
    pub fn apply_styles(_: isize) {}
    pub fn window_rect(_: isize) -> Option<DeviceRect> {
        None
    }
    pub fn client_rect(_: isize) -> Option<DeviceRect> {
        None
    }
    pub fn frame(_: isize) -> super::Frame {
        super::Frame::default()
    }
    pub fn place(_: isize, _: DeviceRect) {}
    pub fn topmost(_: isize) {}
    pub fn set_region(_: isize, _: &[DeviceRect]) {}
    pub fn set_visible(_: isize, _: bool) {}
    pub fn cursor_pos() -> Option<(i32, i32)> {
        None
    }
    pub fn cursor_in_window(_: isize) -> Option<bool> {
        None
    }
    pub fn primary_button_down() -> bool {
        false
    }
    pub fn alt_down() -> bool {
        false
    }
    pub fn reduced_motion() -> bool {
        false
    }
    pub fn short_time_pattern() -> Option<String> {
        None
    }
    pub fn beep() {}
    pub fn popup_menu(_: isize, _: &[super::MenuItem], _: i32, _: i32, _: bool) -> Option<u32> {
        None
    }
}

pub use imp::*;

/// Device px between a window's outer rect and its client area (left, top, right, bottom).
/// gpui keeps an invisible resize frame around the client area of its title-bar-less windows
/// (8 px on the sides and bottom, 1 px on top at 96 dpi on Windows 11), while every rect we
/// compute (placement, region, hit tests) is in client = gpui coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Frame {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Frame {
    /// Frame measured from the window rect and the client rect (both in screen coordinates).
    pub fn between(window: DeviceRect, client: DeviceRect) -> Self {
        Self {
            left: client.x - window.x,
            top: client.y - window.y,
            right: window.right() - client.right(),
            bottom: window.bottom() - client.bottom(),
        }
    }

    /// Outer window rect whose client area is exactly `client`.
    pub fn outer(self, client: DeviceRect) -> DeviceRect {
        DeviceRect {
            x: client.x - self.left,
            y: client.y - self.top,
            w: client.w + self.left + self.right,
            h: client.h + self.top + self.bottom,
        }
    }

    /// A client-coordinates rect in window coordinates (what `SetWindowRgn` expects).
    pub fn to_window(self, r: DeviceRect) -> DeviceRect {
        DeviceRect {
            x: r.x + self.left,
            y: r.y + self.top,
            ..r
        }
    }
}

/// One native menu entry.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    Item {
        id: u32,
        label: String,
        checked: bool,
        enabled: bool,
    },
    Separator,
}

/// 24-hour clock per the Windows short time format (falls back to the language default).
pub fn clock24(lang: codenotch_core::i18n::Lang) -> bool {
    static CACHE: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| short_time_pattern().map(|p| crate::text::clock24_from_pattern(&p)))
        .unwrap_or_else(|| codenotch_core::format::default_clock24(lang))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> DeviceRect {
        DeviceRect { x, y, w, h }
    }

    /// Measured on Windows 11 at 96 dpi (TMA 2026-11-12): window rect 360 × 1020 at
    /// (2200, 186), gpui client area 344 × 1011 at (2208, 187).
    const WIN11_96DPI: Frame = Frame {
        left: 8,
        top: 1,
        right: 8,
        bottom: 8,
    };

    #[test]
    fn frame_is_measured_from_window_and_client_rects() {
        let f = Frame::between(r(2200, 186, 360, 1020), r(2208, 187, 344, 1011));
        assert_eq!(f, WIN11_96DPI);
    }

    /// Regression: the notch placed its *window* rect flush with the screen edge, so gpui's
    /// client area (where the pill is drawn) ended 8 px short of it and the 10 px rest pill at
    /// the edge fell outside the drawing surface — the notch was invisible.
    #[test]
    fn placing_puts_the_client_area_on_the_target() {
        let target = r(2200, 186, 360, 1020); // right edge of a 2560 px wide monitor
        let outer = WIN11_96DPI.outer(target);
        assert_eq!(outer, r(2192, 185, 376, 1029));
        let client = r(
            outer.x + WIN11_96DPI.left,
            outer.y + WIN11_96DPI.top,
            outer.w - WIN11_96DPI.left - WIN11_96DPI.right,
            outer.h - WIN11_96DPI.top - WIN11_96DPI.bottom,
        );
        assert_eq!(client, target);
        assert_eq!(client.right(), 2560);
    }

    #[test]
    fn region_rects_are_shifted_into_window_coordinates() {
        // Rest-pill wake zone of a right-edge notch, client coordinates.
        let wake = r(316, 470, 44, 80);
        assert_eq!(WIN11_96DPI.to_window(wake), r(324, 471, 44, 80));
    }

    #[test]
    fn frameless_window_is_unchanged() {
        let f = Frame::default();
        let a = r(10, 20, 30, 40);
        assert_eq!(f.outer(a), a);
        assert_eq!(f.to_window(a), a);
    }
}
