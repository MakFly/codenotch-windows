//! Read-only observation of the actual widget while Computer Use exercises it.
//! No input injection, focus changes, or modifications to another window.
#[cfg(windows)]
fn main() {
    use std::{
        ptr::null,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::{CreateRectRgn, DeleteObject, GetRgnBox, GetWindowRgn},
        UI::WindowsAndMessaging::*,
    };
    let title: Vec<u16> = "CodeNotch".encode_utf16().chain(Some(0)).collect();
    unsafe {
        SetProcessDPIAware();
        let hwnd = FindWindowW(null(), title.as_ptr());
        assert!(
            !hwnd.is_null(),
            "Launch the local CodeNotch executable first"
        );
        let start = Instant::now();
        let seconds: u64 = std::env::args()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(90);
        let mut previous = String::new();
        while start.elapsed().as_secs() < seconds && IsWindow(hwnd) != 0 {
            let mut bounds: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut bounds);
            let region = CreateRectRgn(0, 0, 0, 0);
            let mut clip: RECT = std::mem::zeroed();
            let region_type = GetWindowRgn(hwnd, region);
            GetRgnBox(region, &mut clip);
            DeleteObject(region);
            // The top-left is outside the collapsed/rail region for right-edge mode.
            let hit = WindowFromPoint(POINT {
                x: bounds.left + 2,
                y: bounds.top + 2,
            });
            let corner_blocked = GetAncestor(hit, GA_ROOT) == hwnd;
            let state = format!(
                "bounds=({},{},{},{}) region=({},{},{},{}) type={} visible={} corner_blocked={}",
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                clip.left,
                clip.top,
                clip.right,
                clip.bottom,
                region_type,
                IsWindowVisible(hwnd),
                corner_blocked
            );
            if previous != state {
                println!("{}ms {state}", start.elapsed().as_millis());
                previous = state;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}
#[cfg(not(windows))]
fn main() {}
