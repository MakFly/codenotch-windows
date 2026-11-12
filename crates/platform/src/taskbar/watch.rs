//! Window-state reads, the background measuring thread and the cache behind [`probe`].

use super::{layout, Hidden, Placement, Rect, Side};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Everything [`super::layout`] needs, from one look at the desktop (device px).
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// `Shell_TrayWnd` rectangle; `None` when there is no taskbar window.
    pub bar: Option<Rect>,
    /// The taskbar's monitor.
    pub monitor: Rect,
    /// That monitor is the primary one.
    pub primary: bool,
    pub visible: bool,
    /// `SHAppBarMessage(ABM_GETSTATE)` reports auto-hide.
    pub auto_hide: bool,
    /// Monitor DPI / 96.
    pub dpi: f32,
    /// A full-screen application is in front on the taskbar's monitor.
    pub fullscreen: bool,
    /// Occupied spans (UI Automation items + the tray); `None` = could not be read.
    pub occupied: Option<Vec<Rect>>,
}

/// Cheap state every 250 ms; UI Automation at most this often.
const TICK: Duration = Duration::from_millis(250);
const FULL_EVERY: Duration = Duration::from_secs(1);
/// The thread idles when nobody probed for this long.
const IDLE_AFTER: Duration = Duration::from_secs(3);
/// A failed UI Automation read keeps the last good one this long.
const KEEP_GOOD: Duration = Duration::from_secs(6);

struct Shared {
    started: bool,
    last_request: Option<Instant>,
    measurement: Option<Measurement>,
    previous: Option<(Side, Rect)>,
    last_outcome: Option<Result<(), Hidden>>,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    started: false,
    last_request: None,
    measurement: None,
    previous: None,
    last_outcome: None,
});
static WAKE: Condvar = Condvar::new();

fn lock() -> std::sync::MutexGuard<'static, Shared> {
    SHARED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where a strip `wanted` device px wide goes on the primary taskbar, from the cached
/// measurement. Never blocks on Explorer (see the module docs); call it from the UI thread on a
/// timer. The first call starts the measuring thread and returns `Err(Hidden::NotMeasured)`
/// until the first measurement is in (normally well under a second).
pub fn probe(side: Side, wanted: i32) -> Result<Placement, Hidden> {
    let mut s = lock();
    let now = Instant::now();
    let was_idle = s
        .last_request
        .is_none_or(|t| now.duration_since(t) >= IDLE_AFTER);
    s.last_request = Some(now);
    if !s.started {
        s.started = true;
        let spawned = std::thread::Builder::new()
            .name("codenotch-taskbar".into())
            .spawn(run);
        if let Err(err) = spawned {
            log::error!("taskbar: cannot start the measuring thread: {err}");
            s.started = false;
            return Err(Hidden::NotMeasured);
        }
    } else if was_idle {
        WAKE.notify_all();
    }
    let result = match &s.measurement {
        None => Err(Hidden::NotMeasured),
        Some(m) => {
            let previous = s.previous.filter(|(p, _)| *p == side).map(|(_, r)| r);
            layout(m, side, wanted, previous)
        }
    };
    s.previous = result.as_ref().ok().map(|p| (side, p.rect));
    let outcome = result.as_ref().map(|_| ()).map_err(|h| *h);
    if s.last_outcome != Some(outcome) {
        s.last_outcome = Some(outcome);
        match outcome {
            Ok(()) => log::info!("taskbar strip: shown"),
            Err(why) => log::info!("taskbar strip: hidden ({why:?})"),
        }
    }
    result
}

/// One complete synchronous measurement on the calling thread (COM and the thread's DPI
/// awareness are set up and restored around it). Diagnostics only: this talks to Explorer and
/// may take tens of milliseconds — never call it on the UI thread.
pub fn measure() -> Measurement {
    let _dpi = imp::DpiScope::per_monitor();
    let apartment = imp::Apartment::enter();
    let uia = imp::Uia::new(&apartment);
    let state = imp::read_bar();
    let fullscreen = imp::fullscreen(&state, false);
    let auto_hide = state.hwnd.is_some_and(|_| imp::auto_hide());
    let occupied = state
        .hwnd
        .and_then(|h| imp::occupied(uia.as_ref(), h, state.rect?));
    let m = state.measurement(auto_hide, fullscreen, occupied);
    drop(uia);
    drop(apartment);
    m
}

/// The measuring thread (see the module docs for its schedule).
fn run() {
    let _dpi = imp::DpiScope::per_monitor();
    let apartment = imp::Apartment::enter();
    let uia = imp::Uia::new(&apartment);
    if uia.is_none() {
        log::warn!("taskbar: UI Automation is unavailable; the strip stays hidden");
    }
    let mut key: Option<(isize, Rect, u32)> = None;
    let mut next_full = Instant::now();
    let mut good: Option<(Instant, Vec<Rect>)> = None;
    let mut auto_hide = false;
    let mut fullscreen = false;
    loop {
        {
            let mut s = lock();
            while s.last_request.is_none_or(|t| t.elapsed() >= IDLE_AFTER) {
                s.measurement = None;
                let (guard, _) = WAKE
                    .wait_timeout(s, Duration::from_secs(5))
                    .unwrap_or_else(|e| e.into_inner());
                s = guard;
                // Back from idle: everything is re-read at once.
                next_full = Instant::now();
            }
        }
        let now = Instant::now();
        let state = imp::read_bar();
        let k = state
            .hwnd
            .zip(state.rect)
            .map(|(h, r)| (h, r, state.dpi.to_bits()));
        if k != key {
            if k.map(|k| k.0) != key.map(|k| k.0) {
                good = None;
            }
            key = k;
            next_full = now;
        }
        fullscreen = imp::fullscreen(&state, fullscreen);
        if now >= next_full {
            next_full = now + FULL_EVERY;
            auto_hide = state.hwnd.is_some_and(|_| imp::auto_hide());
            let read = state
                .hwnd
                .zip(state.rect)
                .and_then(|(h, r)| imp::occupied(uia.as_ref(), h, r));
            match read {
                Some(v) => good = Some((now, v)),
                None => {
                    if good
                        .as_ref()
                        .is_some_and(|(t, _)| now.duration_since(*t) >= KEEP_GOOD)
                    {
                        good = None;
                    }
                }
            }
        }
        let m = state.measurement(auto_hide, fullscreen, good.as_ref().map(|(_, v)| v.clone()));
        lock().measurement = Some(m);
        std::thread::sleep(TICK);
    }
}

mod imp {
    use super::{Measurement, Rect};
    use crate::desktop::{classify, Foreground, Verdict};
    use crate::win::{from_wide, wide};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::HiDpi::{
        GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
    };
    use windows_sys::Win32::UI::Shell::{SHAppBarMessage, ABM_GETSTATE, ABS_AUTOHIDE, APPBARDATA};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindowLongW,
        GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, IsZoomed, GWL_STYLE,
        MONITORINFOF_PRIMARY, WS_CAPTION,
    };

    pub(super) use super::super::uia::{Apartment, Uia};

    /// Per-monitor-v2 DPI awareness for the current thread (physical coordinates from
    /// `GetWindowRect` and UI Automation); the previous context is restored on drop.
    pub struct DpiScope(DPI_AWARENESS_CONTEXT);

    impl DpiScope {
        pub fn per_monitor() -> Self {
            // SAFETY: changes only the calling thread's DPI context; returns the old one or null.
            Self(unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            })
        }
    }

    impl Drop for DpiScope {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: restores the context this thread had before.
                unsafe { SetThreadDpiAwarenessContext(self.0) };
            }
        }
    }

    fn rect(r: RECT) -> Rect {
        Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }

    fn window_rect(hwnd: HWND) -> Option<Rect> {
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: valid out pointer; a stale handle makes the call fail.
        (unsafe { GetWindowRect(hwnd, &mut r) } != 0).then(|| rect(r))
    }

    /// The cheap part of a measurement.
    pub struct BarState {
        pub hwnd: Option<isize>,
        pub rect: Option<Rect>,
        pub visible: bool,
        pub monitor: Rect,
        pub hmonitor: isize,
        pub primary: bool,
        pub dpi: f32,
    }

    impl BarState {
        pub fn measurement(
            &self,
            auto_hide: bool,
            fullscreen: bool,
            occupied: Option<Vec<Rect>>,
        ) -> Measurement {
            Measurement {
                bar: self.hwnd.and(self.rect),
                monitor: self.monitor,
                primary: self.primary,
                visible: self.visible,
                auto_hide,
                dpi: self.dpi,
                fullscreen,
                occupied,
            }
        }
    }

    fn monitor_of(hwnd: HWND) -> Option<(HMONITOR, Rect, bool, f32)> {
        // SAFETY: MonitorFromWindow with DEFAULTTONEAREST has no preconditions.
        let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if hmon.is_null() {
            return None;
        }
        // SAFETY: MONITORINFO is plain data; cbSize set below.
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        // SAFETY: valid monitor handle and out pointer.
        if unsafe { GetMonitorInfoW(hmon, &mut info) } == 0 {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        // SAFETY: valid monitor handle and out pointers.
        let dpi = if unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) } == 0 {
            dx.max(48) as f32 / 96.0
        } else {
            1.0
        };
        Some((
            hmon,
            rect(info.rcMonitor),
            info.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi,
        ))
    }

    /// Taskbar window, rectangle, visibility, monitor and DPI. No cross-process messages.
    pub fn read_bar() -> BarState {
        let class = wide("Shell_TrayWnd");
        // SAFETY: NUL-terminated class name; a null result means no taskbar.
        let bar = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        let empty = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if bar.is_null() {
            return BarState {
                hwnd: None,
                rect: None,
                visible: false,
                monitor: empty,
                hmonitor: 0,
                primary: false,
                dpi: 1.0,
            };
        }
        let (hmon, monitor, primary, dpi) =
            monitor_of(bar).unwrap_or((std::ptr::null_mut(), empty, false, 1.0));
        BarState {
            hwnd: Some(bar as isize),
            rect: window_rect(bar),
            // SAFETY: plain query; a stale handle reads as not visible.
            visible: unsafe { IsWindowVisible(bar) } != 0,
            monitor,
            hmonitor: hmon as isize,
            primary,
            dpi,
        }
    }

    /// `ABM_GETSTATE` (a message to Explorer: measuring thread only).
    pub fn auto_hide() -> bool {
        // SAFETY: APPBARDATA is plain data; cbSize announces the layout.
        let mut data: APPBARDATA = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        // SAFETY: valid pointer to an initialised APPBARDATA.
        let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) } as u32;
        state & ABS_AUTOHIDE != 0
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 128];
        // SAFETY: buffer of 128 u16; at most that many chars are written.
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        if n <= 0 {
            return String::new();
        }
        from_wide(&buf[..n as usize])
    }

    /// A full-screen app is in front on the taskbar's monitor. Our own windows and transient
    /// shell surfaces keep `previous` (same rules as `desktop::fullscreen_monitor`).
    pub fn fullscreen(bar: &BarState, previous: bool) -> bool {
        // SAFETY: no preconditions; may return null.
        let fg = unsafe { GetForegroundWindow() };
        if fg.is_null() || bar.hmonitor == 0 {
            return false;
        }
        let mut pid = 0u32;
        // SAFETY: valid out pointer.
        unsafe { GetWindowThreadProcessId(fg, &mut pid) };
        let Some(r) = window_rect(fg) else {
            return previous;
        };
        let Some((hmon, monitor, _, _)) = monitor_of(fg) else {
            return previous;
        };
        // SAFETY: plain queries on a window handle; failures return 0.
        let (zoomed, style) = unsafe { (IsZoomed(fg) != 0, GetWindowLongW(fg, GWL_STYLE) as u32) };
        let f = Foreground {
            class: class_name(fg),
            own: pid == std::process::id(),
            rect: r,
            monitor,
            maximised: zoomed && style & WS_CAPTION == WS_CAPTION,
        };
        match classify(&f) {
            Verdict::Unchanged => previous,
            Verdict::Windowed => false,
            Verdict::FullScreen => hmon as isize == bar.hmonitor,
        }
    }

    /// UI Automation items plus the notification area (`TrayNotifyWnd`, stretched to the bar's
    /// right end). `None` when UI Automation is unavailable or found nothing.
    pub fn occupied(uia: Option<&Uia>, bar: isize, bar_rect: Rect) -> Option<Vec<Rect>> {
        let mut found = uia?.occupied(bar, bar_rect)?;
        let class = wide("TrayNotifyWnd");
        // SAFETY: child lookup by class name on the taskbar window; null when absent.
        let tray = unsafe {
            FindWindowExW(
                bar as HWND,
                std::ptr::null_mut(),
                class.as_ptr(),
                std::ptr::null(),
            )
        };
        // SAFETY: plain query.
        if !tray.is_null() && unsafe { IsWindowVisible(tray) } != 0 {
            if let Some(mut r) = window_rect(tray) {
                r.right = bar_rect.right;
                if r.right > r.left && r.bottom > r.top {
                    found.push(r);
                }
            }
        }
        Some(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only probe of the real taskbar: prints rectangles and the computed strips only.
    /// Run with `cargo test -p codenotch-platform -- --ignored --nocapture live_taskbar`.
    #[test]
    #[ignore = "reads the live desktop"]
    fn live_taskbar_probe() {
        let started = Instant::now();
        let m = measure();
        let took = started.elapsed();
        println!("measured in {took:?}");
        println!("bar      {:?}", m.bar);
        println!(
            "monitor  {:?} primary={} dpi={}",
            m.monitor, m.primary, m.dpi
        );
        println!(
            "visible={} auto_hide={} fullscreen={}",
            m.visible, m.auto_hide, m.fullscreen
        );
        match &m.occupied {
            None => println!("occupied: none"),
            Some(v) => {
                let mut v = v.clone();
                v.sort_by_key(|r| (r.left, r.top));
                println!("occupied: {} rects", v.len());
                for r in v {
                    println!(
                        "  {:>6} {:>6} {:>6} {:>6}  ({}x{})",
                        r.left,
                        r.top,
                        r.right,
                        r.bottom,
                        r.right - r.left,
                        r.bottom - r.top
                    );
                }
            }
        }
        for wanted in [200, 400] {
            for side in [Side::Left, Side::BeforeTray] {
                println!(
                    "{side:?} wanted {wanted}: {:?}",
                    layout(&m, side, wanted, None)
                );
            }
        }
    }
}
