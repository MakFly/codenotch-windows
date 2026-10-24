use crate::model::WidgetPlacement;

/// Keep WebView2's backing surface and screen origin stable across hover modes.
/// Only the visible/input region changes; the transparent remainder is not a window hit target.
pub fn surface_size(edge: &str, count: usize, screen: (f64, f64)) -> (f64, f64) {
    let cells = count.clamp(1, 6) as f64;
    if matches!(edge, "left" | "right") {
        (
            420.0,
            (cells * 94.0 + 136.0).max(440.0).min(screen.1 - 60.0),
        )
    } else {
        (
            (cells * 73.0 + 122.0).max(360.0).min(screen.0),
            530.0_f64.min(screen.1),
        )
    }
}

pub fn visible_region(
    edge: &str,
    surface: (i32, i32),
    visible: (i32, i32),
) -> (i32, i32, i32, i32) {
    let (width, height) = (visible.0.min(surface.0), visible.1.min(surface.1));
    let (x, y) = match edge {
        "left" => (0, (surface.1 - height) / 2),
        "right" => (surface.0 - width, (surface.1 - height) / 2),
        "bottom" => ((surface.0 - width) / 2, surface.1 - height),
        _ => ((surface.0 - width) / 2, 0),
    };
    (x, y, x + width, y + height)
}

#[cfg(windows)]
pub fn set_input_region(
    window: &tauri::WebviewWindow,
    rect: (i32, i32, i32, i32),
) -> Result<(), String> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateRectRgn, DeleteObject, EqualRgn, GetWindowRgn, SetWindowRgn,
    };
    let hwnd = window.hwnd().map_err(|_| "Fenêtre indisponible.")?.0;
    unsafe {
        let next = CreateRectRgn(rect.0, rect.1, rect.2, rect.3);
        if next.is_null() {
            return Err("Zone d’encoche indisponible.".into());
        }
        let current = CreateRectRgn(0, 0, 0, 0);
        let same =
            !current.is_null() && GetWindowRgn(hwnd, current) != 0 && EqualRgn(current, next) != 0;
        if !current.is_null() {
            DeleteObject(current);
        }
        if same {
            DeleteObject(next);
            return Ok(());
        }
        // On success Windows owns the region. Never free it after assignment.
        if SetWindowRgn(hwnd, next, 1) == 0 {
            DeleteObject(next);
            return Err("Impossible de définir la zone de l’encoche.".into());
        }
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn set_input_region(_: &tauri::WebviewWindow, _: (i32, i32, i32, i32)) -> Result<(), String> {
    Ok(())
}

/// Commit size and position together: a separate resize paints the new surface
/// at the old origin for one frame. Identical bounds must not invalidate WebView2.
#[cfg(windows)]
pub fn set_bounds(
    window: &tauri::WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::RECT,
        UI::WindowsAndMessaging::{GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER},
    };
    let hwnd = window.hwnd().map_err(|_| "Fenêtre indisponible.")?.0;
    unsafe {
        let mut current: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut current) != 0
            && (
                current.left,
                current.top,
                current.right - current.left,
                current.bottom - current.top,
            ) == (x, y, width, height)
        {
            return Ok(());
        }
        if SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        ) == 0
        {
            return Err("Impossible de positionner l’encoche.".into());
        }
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn set_bounds(
    window: &tauri::WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    window
        .set_size(tauri::PhysicalSize::new(width as u32, height as u32))
        .map_err(|_| "Dimensions indisponibles.")?;
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|_| "Position indisponible.".into())
}

/// Store the edge's anchor rather than the top-left of a resizable surface.
/// Ratios survive DPI/resolution changes; monitor identity survives rearrangement.
pub fn capture(
    edge: &str,
    position: (i32, i32),
    size: (u32, u32),
    origin: (i32, i32),
    screen: (u32, u32),
    monitor: Option<String>,
) -> WidgetPlacement {
    let (dx, dy) = offset(edge, size);
    WidgetPlacement {
        monitor,
        x: ((position.0 as f64 + dx - origin.0 as f64) / screen.0.max(1) as f64).clamp(0.0, 1.0),
        y: ((position.1 as f64 + dy - origin.1 as f64) / screen.1.max(1) as f64).clamp(0.0, 1.0),
    }
}

fn offset(edge: &str, size: (u32, u32)) -> (f64, f64) {
    match edge {
        "right" => (size.0 as f64, size.1 as f64 / 2.0),
        "left" => (0.0, size.1 as f64 / 2.0),
        "bottom" => (size.0 as f64 / 2.0, size.1 as f64),
        _ => (size.0 as f64 / 2.0, 0.0),
    }
}

pub fn restore(
    edge: &str,
    saved: &WidgetPlacement,
    size: (u32, u32),
    origin: (i32, i32),
    screen: (u32, u32),
) -> (i32, i32) {
    let (dx, dy) = offset(edge, size);
    let ratio = |v: f64| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.5
        }
    };
    let x = origin.0 as f64 + ratio(saved.x) * screen.0 as f64 - dx;
    let y = origin.1 as f64 + ratio(saved.y) * screen.1 as f64 - dy;
    // A disconnected/smaller monitor must never make the widget unreachable.
    (
        x.round().clamp(
            origin.0 as f64,
            origin.0 as f64 + screen.0.saturating_sub(size.0) as f64,
        ) as i32,
        y.round().clamp(
            origin.1 as f64,
            origin.1 as f64 + screen.1.saturating_sub(size.1) as f64,
        ) as i32,
    )
}

pub fn covers_monitor(client: (i32, i32, i32, i32), monitor: (i32, i32, i32, i32)) -> bool {
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

/// Complete only when Windows exits its modal move loop, not when the move
/// request is merely queued. This avoids saving the pre-drag position.
#[cfg(windows)]
pub async fn drag(window: &tauri::WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{HWND, POINT},
        UI::{
            Input::KeyboardAndMouse::ReleaseCapture,
            WindowsAndMessaging::{GetCursorPos, SendMessageW, HTCAPTION, WM_NCLBUTTONDOWN},
        },
    };
    let hwnd = window.hwnd().map_err(|_| "Fenêtre indisponible.")?.0 as usize;
    let (done, completed) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || unsafe {
            let mut point = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut point) == 0 {
                let _ = done.send(Err("Position de souris indisponible.".to_owned()));
                return;
            }
            ReleaseCapture();
            let coordinates =
                ((point.x as u32 & 0xffff) | ((point.y as u32 & 0xffff) << 16)) as isize;
            SendMessageW(
                hwnd as HWND,
                WM_NCLBUTTONDOWN,
                HTCAPTION as usize,
                coordinates,
            );
            let _ = done.send(Ok(()));
        })
        .map_err(|_| "Impossible de déplacer l’encoche.")?;
    completed
        .await
        .map_err(|_| "Déplacement interrompu.".to_owned())?
}
#[cfg(not(windows))]
pub async fn drag(window: &tauri::WebviewWindow) -> Result<(), String> {
    window
        .start_dragging()
        .map_err(|_| "Déplacement indisponible.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_regions_share_one_surface_and_anchor() {
        for edge in ["right", "left", "top", "bottom"] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let size = surface_size(edge, 2, (1920.0, 1080.0));
                let surface = ((size.0 * scale) as i32, (size.1 * scale) as i32);
                let vertical = matches!(edge, "left" | "right");
                let peek = if vertical { (24.0, 96.0) } else { (96.0, 24.0) };
                let rail = if vertical {
                    (74.0, 324.0)
                } else {
                    (236.0, 100.0)
                };
                let center = offset(edge, (surface.0 as u32, surface.1 as u32));
                for visible in [peek, rail, size] {
                    let rect = visible_region(
                        edge,
                        surface,
                        ((visible.0 * scale) as i32, (visible.1 * scale) as i32),
                    );
                    assert!(
                        rect.0 >= 0 && rect.1 >= 0 && rect.2 <= surface.0 && rect.3 <= surface.1
                    );
                    let local = offset(edge, ((rect.2 - rect.0) as u32, (rect.3 - rect.1) as u32));
                    assert!((rect.0 as f64 + local.0 - center.0).abs() <= 0.5);
                    assert!((rect.1 as f64 + local.1 - center.1).abs() <= 0.5);
                }
            }
        }
    }
    #[test]
    fn anchor_survives_collapse_expansion_and_serialization() {
        for edge in ["right", "left", "top", "bottom"] {
            let saved = capture(
                edge,
                (-1200, 300),
                (420, 440),
                (-1920, 0),
                (1920, 1080),
                Some("second".into()),
            );
            let saved: WidgetPlacement =
                serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
            let small = restore(edge, &saved, (24, 96), (-1920, 0), (1920, 1080));
            let again = capture(
                edge,
                small,
                (24, 96),
                (-1920, 0),
                (1920, 1080),
                Some("second".into()),
            );
            assert_eq!(
                restore(edge, &again, (420, 440), (-1920, 0), (1920, 1080)),
                (-1200, 300)
            );
        }
    }
    #[test]
    fn changed_dpi_and_missing_monitor_keep_widget_visible() {
        let saved = WidgetPlacement {
            monitor: Some("disconnected".into()),
            x: 1.0,
            y: 1.0,
        };
        assert_eq!(
            restore("right", &saved, (630, 660), (0, 0), (1280, 720)),
            (650, 60)
        );
        let broken = WidgetPlacement {
            monitor: None,
            x: f64::NAN,
            y: -4.0,
        };
        assert_eq!(
            restore("left", &broken, (74, 324), (0, 0), (1920, 1080)),
            (960, 0)
        );
    }
    #[test]
    fn fullscreen_client_not_maximized_titlebar_or_windowed_game() {
        assert!(covers_monitor((0, 0, 1920, 1080), (0, 0, 1920, 1080)));
        assert!(covers_monitor((-1920, 0, 0, 1080), (-1920, 0, 0, 1080)));
        assert!(!covers_monitor((0, 31, 1920, 1040), (0, 0, 1920, 1080)));
        assert!(!covers_monitor((100, 100, 1500, 900), (0, 0, 1920, 1080)));
    }
}
