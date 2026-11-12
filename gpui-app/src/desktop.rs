/// True when the foreground window covers its monitor.
/// Our own notch is too small to count.
pub fn fullscreen_active() -> Option<bool> {
    #[cfg(windows)]
    {
        windows_fullscreen()
    }
    #[cfg(not(windows))]
    {
        Some(false)
    }
}

#[cfg(windows)]
fn windows_fullscreen() -> Option<bool> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITORINFO, MONITOR_DEFAULTTONEAREST, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return Some(false);
        }
        let mut window_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut window_rect) == 0 {
            return None;
        }
        let width = window_rect.right - window_rect.left;
        let height = window_rect.bottom - window_rect.top;
        // The notch itself is a few hundred pixels. Ignore it.
        if width < 800 || height < 600 {
            return Some(false);
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return None;
        }
        let monitor = info.rcMonitor;
        Some(
            window_rect.left <= monitor.left
                && window_rect.top <= monitor.top
                && window_rect.right >= monitor.right
                && window_rect.bottom >= monitor.bottom,
        )
    }
}
