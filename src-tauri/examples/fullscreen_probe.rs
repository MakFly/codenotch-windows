//! Local integration probe: requires CodeNotch running with automatic fullscreen hiding enabled.
//! Creates only its own temporary window; never controls a game or other application.
#[cfg(windows)]
fn main() {
    use std::{
        ptr::null,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        UI::WindowsAndMessaging::*,
    };
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
    unsafe fn settle() {
        let until = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < until {
            let mut message = std::mem::zeroed();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    struct Probe(HWND);
    impl Drop for Probe {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }
    unsafe {
        SetProcessDPIAware();
        let notch = FindWindowW(null(), wide("CodeNotch").as_ptr());
        assert!(!notch.is_null(), "Launch CodeNotch first");
        let window = Probe(CreateWindowExW(
            0,
            wide("STATIC").as_ptr(),
            wide("CodeNotch : test plein écran temporaire — fermeture automatique").as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            200,
            200,
            640,
            480,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            null(),
        ));
        assert!(!window.0.is_null());
        SetForegroundWindow(window.0);
        // Windows may refuse foreground acquisition by a background shell.
        // Leave time to activate this visible test window normally.
        let activation_deadline = Instant::now() + Duration::from_secs(60);
        while GetForegroundWindow() != window.0 && Instant::now() < activation_deadline {
            settle();
        }
        settle();
        assert_eq!(
            GetForegroundWindow(),
            window.0,
            "Probe could not acquire foreground; test inconclusive"
        );
        assert_ne!(
            IsWindowVisible(notch),
            0,
            "Notch should be visible with a normal window"
        );
        println!("PASS: ordinary foreground window keeps notch visible");
        ShowWindow(window.0, SW_MAXIMIZE);
        settle();
        assert_ne!(
            IsWindowVisible(notch),
            0,
            "A maximized window with title bar is not fullscreen"
        );
        println!("PASS: maximized window is not misclassified as fullscreen");
        let mut monitor: MONITORINFO = std::mem::zeroed();
        monitor.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        assert_ne!(
            GetMonitorInfoW(
                MonitorFromWindow(window.0, MONITOR_DEFAULTTONEAREST),
                &mut monitor
            ),
            0
        );
        ShowWindow(window.0, SW_RESTORE);
        SetWindowLongPtrW(window.0, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
        let rect = monitor.rcMonitor;
        SetWindowPos(
            window.0,
            HWND_TOP,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_FRAMECHANGED | SWP_SHOWWINDOW,
        );
        SetForegroundWindow(window.0);
        settle();
        assert_eq!(
            GetForegroundWindow(),
            window.0,
            "Foreground changed; test inconclusive"
        );
        assert_eq!(
            IsWindowVisible(notch),
            0,
            "Notch should hide in borderless fullscreen"
        );
        println!("PASS: borderless fullscreen hides the real CodeNotch window");
        ShowWindow(window.0, SW_MINIMIZE);
        settle();
        assert_ne!(
            IsWindowVisible(notch),
            0,
            "Notch should return after leaving fullscreen"
        );
        println!("PASS: leaving fullscreen restores the real CodeNotch window");
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
