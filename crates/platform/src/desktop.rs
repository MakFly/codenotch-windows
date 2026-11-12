//! Full-screen and game detection (data-spec §11 "Full-screen detection"), polled every 400 ms
//! from a UI timer.
//!
//! **Full screen** — the foreground window is classified by its class name, rectangle and style:
//! * this app's own windows and transient shell surfaces (`XamlExplorerHostIslandWindow`,
//!   `MultitaskingViewFrame`, `TaskSwitcherWnd`, `ForegroundStaging`) → *unchanged* (the previous
//!   answer is kept, so Alt+Tab or hovering the notch does not unfold it);
//! * the desktop and taskbars (`Progman`, `WorkerW`, `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`)
//!   → windowed;
//! * not maximised-with-caption and the window rectangle equals its monitor rectangle within
//!   ±4 px on all four sides → full screen on that monitor.
//!
//! **Game** ([`game_in_front`]) — heuristic, see its doc.

use crate::win::{from_wide, OwnedHandle};
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUERY_USER_NOTIFICATION_STATE, QUNS_BUSY,
    QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    IsZoomed, GWL_STYLE, WS_CAPTION,
};

/// Slack when comparing a window with its monitor, in device px.
pub const SLACK_PX: i32 = 4;

/// Classes that never change the current answer.
const TRANSIENT_CLASSES: &[&str] = &[
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "TaskSwitcherWnd",
    "ForegroundStaging",
];
/// Shell surfaces that mean "no full-screen app".
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// A rectangle in device px (left, top, right, bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

/// Outcome of one look at the foreground window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Keep the previous answer.
    Unchanged,
    Windowed,
    /// Full screen on the window's monitor.
    FullScreen,
}

/// What we know about the foreground window (pure input of [`classify`]).
#[derive(Debug, Clone)]
pub struct Foreground {
    pub class: String,
    pub own: bool,
    pub rect: Rect,
    pub monitor: Rect,
    /// `IsZoomed` and the window has `WS_CAPTION` (a normal maximised window).
    pub maximised: bool,
}

/// The full-screen decision (pure).
pub fn classify(fg: &Foreground) -> Verdict {
    if fg.own || TRANSIENT_CLASSES.contains(&fg.class.as_str()) {
        return Verdict::Unchanged;
    }
    if SHELL_CLASSES.contains(&fg.class.as_str()) {
        return Verdict::Windowed;
    }
    let r = fg.rect;
    let m = fg.monitor;
    let close = |a: i32, b: i32| (a - b).abs() <= SLACK_PX;
    let covers = close(r.left, m.left)
        && close(r.top, m.top)
        && close(r.right, m.right)
        && close(r.bottom, m.bottom);
    if covers && !fg.maximised {
        Verdict::FullScreen
    } else {
        Verdict::Windowed
    }
}

static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Foreground window covers its whole monitor (±4 px), excluding shell/notch windows.
/// Returns the monitor device name (`\\.\DISPLAY1`) when full screen, `None` otherwise.
/// `own_windows` = HWNDs of our windows to ignore (the answer is then the previous one).
pub fn fullscreen_monitor(own_windows: &[isize]) -> Option<String> {
    let verdict = probe_foreground(own_windows);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    match verdict {
        Some((Verdict::Unchanged, _)) => {}
        Some((Verdict::Windowed, _)) | None => *last = None,
        Some((Verdict::FullScreen, name)) => *last = name,
    }
    last.clone()
}

/// Look at the foreground window. `None` when there is none (e.g. the secure desktop).
fn probe_foreground(own_windows: &[isize]) -> Option<(Verdict, Option<String>)> {
    // SAFETY: no preconditions; may return null.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let class = window_class(hwnd);
    let own = own_windows.contains(&(hwnd as isize));
    let mut wr = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `hwnd` came from GetForegroundWindow (a stale handle just makes the call fail).
    if unsafe { GetWindowRect(hwnd, &mut wr) } == 0 {
        return None;
    }
    let (monitor, name) = monitor_of(hwnd)?;
    // SAFETY: plain queries on a window handle; failures return 0.
    let (zoomed, style) = unsafe { (IsZoomed(hwnd) != 0, GetWindowLongW(hwnd, GWL_STYLE) as u32) };
    let fg = Foreground {
        class,
        own,
        rect: wr.into(),
        monitor,
        maximised: zoomed && style & WS_CAPTION == WS_CAPTION,
    };
    let verdict = classify(&fg);
    Some((verdict, (verdict == Verdict::FullScreen).then_some(name)))
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: buffer of 128 u16; the call writes at most nmaxcount chars including NUL.
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    from_wide(&buf[..n as usize])
}

/// Monitor rectangle and device name of the monitor nearest to `hwnd`.
fn monitor_of(hwnd: HWND) -> Option<(Rect, String)> {
    // SAFETY: MonitorFromWindow with DEFAULTTONEAREST always returns a monitor for a valid hwnd.
    let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if hmon.is_null() {
        return None;
    }
    // SAFETY: MONITORINFOEXW is plain data; zero is a valid bit pattern.
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: cbSize announces the EX layout, so the API may fill szDevice.
    let ok = unsafe {
        GetMonitorInfoW(
            hmon,
            (&mut info as *mut MONITORINFOEXW).cast::<MONITORINFO>(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some((info.monitorInfo.rcMonitor.into(), from_wide(&info.szDevice)))
}

// ---------------------------------------------------------------------------------------------
// Game detection
// ---------------------------------------------------------------------------------------------

/// Executables that go full screen without being games (browsers, video players, slide shows,
/// remote desktops). Lower-case file names.
const NOT_GAMES: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
    "opera.exe",
    "vivaldi.exe",
    "arc.exe",
    "vlc.exe",
    "mpc-hc.exe",
    "mpc-hc64.exe",
    "mpc-be64.exe",
    "mpv.exe",
    "potplayermini64.exe",
    "wmplayer.exe",
    "video.ui.exe",
    "microsoft.media.player.exe",
    "powerpnt.exe",
    "mstsc.exe",
    "msrdc.exe",
    "explorer.exe",
    "applicationframehost.exe",
    "code.exe",
    "windowsterminal.exe",
];

/// The shell's "user notification state" (`SHQueryUserNotificationState`), simplified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationState {
    /// A Direct3D exclusive full-screen app is running (`QUNS_RUNNING_D3D_FULL_SCREEN`).
    D3dFullScreen,
    /// A full-screen app or presentation settings are active (`QUNS_BUSY`).
    Busy,
    Other,
}

/// The game decision (pure):
/// * exclusive Direct3D full screen → game;
/// * otherwise the foreground window must be borderless full screen (see [`classify`]) **and**
///   the shell must report "busy" **and** the process must not be a known non-game
///   (browser, video player, PowerPoint, remote desktop, Explorer…).
pub fn is_game(state: NotificationState, fullscreen_fg: bool, exe_name: Option<&str>) -> bool {
    match state {
        NotificationState::D3dFullScreen => true,
        NotificationState::Busy if fullscreen_fg => {
            let exe = exe_name.unwrap_or("").to_ascii_lowercase();
            !exe.is_empty() && !NOT_GAMES.contains(&exe.as_str())
        }
        _ => false,
    }
}

/// A game / exclusive full-screen app is in front (for automatic game mode).
///
/// Heuristic, not a game database: Windows itself reports `QUNS_RUNNING_D3D_FULL_SCREEN` for
/// exclusive full-screen Direct3D apps (most older games). Modern games often run borderless
/// ("full-screen optimisations"); those are caught when the shell says `QUNS_BUSY` and the
/// foreground window fills its monitor, unless the process is a known non-game. Limits: a
/// windowed game is never detected; a full-screen non-game not in the exclusion list (an
/// unusual video player, a kiosk app) counts as a game; `QUNS_BUSY` is not raised by every
/// full-screen app on every Windows build. Never touches the app's own windows.
pub fn game_in_front() -> bool {
    let state = notification_state();
    if state == NotificationState::D3dFullScreen {
        return true;
    }
    if state != NotificationState::Busy {
        return false;
    }
    // SAFETY: no preconditions.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return false;
    }
    let fullscreen = matches!(probe_foreground(&[]), Some((Verdict::FullScreen, _)));
    if !fullscreen || is_own_process(hwnd) {
        return false;
    }
    let exe = process_exe_name(hwnd);
    is_game(state, fullscreen, exe.as_deref())
}

fn notification_state() -> NotificationState {
    let mut q: QUERY_USER_NOTIFICATION_STATE = 0;
    // SAFETY: `q` is a valid out pointer.
    let hr = unsafe { SHQueryUserNotificationState(&mut q) };
    if hr < 0 {
        return NotificationState::Other;
    }
    match q {
        QUNS_RUNNING_D3D_FULL_SCREEN => NotificationState::D3dFullScreen,
        QUNS_BUSY => NotificationState::Busy,
        _ => NotificationState::Other,
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: valid out pointer; a stale hwnd leaves pid at 0.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid
}

fn is_own_process(hwnd: HWND) -> bool {
    window_pid(hwnd) == std::process::id()
}

/// File name of the process owning `hwnd` ("game.exe").
fn process_exe_name(hwnd: HWND) -> Option<String> {
    let pid = window_pid(hwnd);
    if pid == 0 {
        return None;
    }
    // SAFETY: limited query right works across integrity levels for most processes.
    let process =
        OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` u16; the call updates `len` with the chars written.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process.raw(),
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    let path = from_wide(&buf[..len as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect {
        left: 0,
        top: 0,
        right: 2560,
        bottom: 1440,
    };
    const MON2: Rect = Rect {
        left: 2560,
        top: -200,
        right: 4480,
        bottom: 880,
    };

    fn fg(class: &str, rect: Rect, monitor: Rect) -> Foreground {
        Foreground {
            class: class.into(),
            own: false,
            rect,
            monitor,
            maximised: false,
        }
    }

    #[test]
    fn exact_cover_is_full_screen() {
        assert_eq!(classify(&fg("UnrealWindow", MON, MON)), Verdict::FullScreen);
        assert_eq!(
            classify(&fg("Chrome_WidgetWin_1", MON2, MON2)),
            Verdict::FullScreen
        );
    }

    #[test]
    fn four_px_slack_each_side() {
        let r = Rect {
            left: -4,
            top: 3,
            right: 2564,
            bottom: 1436,
        };
        assert_eq!(classify(&fg("X", r, MON)), Verdict::FullScreen);
        let r = Rect {
            left: -5,
            top: 0,
            right: 2560,
            bottom: 1440,
        };
        assert_eq!(classify(&fg("X", r, MON)), Verdict::Windowed);
        // A window that only fills the work area (taskbar visible) is not full screen.
        let r = Rect {
            left: 0,
            top: 0,
            right: 2560,
            bottom: 1392,
        };
        assert_eq!(classify(&fg("X", r, MON)), Verdict::Windowed);
    }

    #[test]
    fn maximised_window_with_caption_is_windowed() {
        // Maximised windows overhang the monitor by their frame (≈ 8 px) anyway, but a
        // captioned maximised window that happens to match must still be windowed.
        let mut f = fg("Notepad", MON, MON);
        f.maximised = true;
        assert_eq!(classify(&f), Verdict::Windowed);
    }

    #[test]
    fn shell_and_desktop_are_windowed() {
        for class in SHELL_CLASSES {
            assert_eq!(classify(&fg(class, MON, MON)), Verdict::Windowed, "{class}");
        }
    }

    #[test]
    fn own_and_transient_windows_keep_the_previous_answer() {
        for class in TRANSIENT_CLASSES {
            assert_eq!(
                classify(&fg(class, MON, MON)),
                Verdict::Unchanged,
                "{class}"
            );
        }
        let mut f = fg("Zed::Window", MON, MON);
        f.own = true;
        assert_eq!(classify(&f), Verdict::Unchanged);
    }

    #[test]
    fn small_window_is_windowed() {
        let r = Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 700,
        };
        assert_eq!(classify(&fg("X", r, MON)), Verdict::Windowed);
    }

    #[test]
    fn game_heuristic() {
        use NotificationState::*;
        assert!(is_game(D3dFullScreen, false, None));
        assert!(is_game(Busy, true, Some("Cyberpunk2077.exe")));
        assert!(!is_game(Busy, true, Some("chrome.exe")));
        assert!(!is_game(Busy, true, Some("MSEDGE.EXE")));
        assert!(!is_game(Busy, true, Some("POWERPNT.EXE")));
        assert!(!is_game(Busy, false, Some("game.exe")));
        assert!(!is_game(Busy, true, None));
        assert!(!is_game(Other, true, Some("game.exe")));
    }

    #[test]
    fn live_probes_do_not_panic() {
        // Read-only queries of the current desktop; the result depends on the machine.
        let _ = fullscreen_monitor(&[]);
        let _ = game_in_front();
        let _ = notification_state();
    }
}
