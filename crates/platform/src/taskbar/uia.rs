//! UI Automation reading of the taskbar's occupied spans (the only part of the crate using the
//! `windows` crate). Read-only: element rectangles and control types, nothing else.

use super::Rect;

/// Control types that occupy taskbar space: Start, search, task view, widgets, app buttons,
/// tray icons, the clock text.
#[cfg(windows)]
const OCCUPYING: [i32; 6] = [
    50000, // Button
    50007, // ListItem
    50011, // MenuItem
    50020, // Text
    50006, // Image
    50005, // Hyperlink
];

/// Walk limit for the raw-view fallback.
#[cfg(windows)]
const MAX_NODES: usize = 1600;

/// Keep only rectangles that look like taskbar items: non-empty, crossing the bar, narrower than
/// half of it (wider ones are containers).
pub(super) fn keep(r: Rect, bar: Rect) -> bool {
    r.right > r.left
        && r.bottom > r.top
        && r.bottom > bar.top
        && r.top < bar.bottom
        && r.right > bar.left
        && r.left < bar.right
        && (r.right - r.left) < (bar.right - bar.left) / 2
}

#[cfg(windows)]
pub(super) use imp::{Apartment, Uia};

#[cfg(windows)]
mod imp {
    use super::{keep, Rect, MAX_NODES, OCCUPYING};
    use windows::core::Interface;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest,
        IUIAutomationCondition, IUIAutomationElement, TreeScope_Descendants,
        UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsOffscreenPropertyId,
    };

    /// COM initialised (multithreaded) for the life of the value on the creating thread;
    /// `CoUninitialize` on drop only when the matching init succeeded. Not `Send`.
    pub struct Apartment {
        initialised: bool,
        _not_send: std::marker::PhantomData<*const ()>,
    }

    impl Apartment {
        pub fn enter() -> Self {
            // SAFETY: plain COM initialisation of the current thread; S_OK and S_FALSE both
            // require a matching CoUninitialize, RPC_E_CHANGED_MODE does not.
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Self {
                initialised: hr.is_ok(),
                _not_send: std::marker::PhantomData,
            }
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.initialised {
                // SAFETY: balances the successful CoInitializeEx of this thread.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// One UI Automation client. Every interface is released when dropped (the `windows` crate's
    /// interface types are RAII). Must be dropped before its [`Apartment`].
    pub struct Uia {
        automation: IUIAutomation,
        cache: IUIAutomationCacheRequest,
        all: IUIAutomationCondition,
    }

    fn rect(r: RECT) -> Rect {
        Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }

    impl Uia {
        /// Create the client (COM must be initialised on this thread). Sets 400 ms connection and
        /// transaction timeouts when the system supports them.
        pub fn new(_apartment: &Apartment) -> Option<Self> {
            // SAFETY: COM calls on an initialised thread; returned interfaces are owned.
            unsafe {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
                if let Ok(two) = automation.cast::<IUIAutomation2>() {
                    let _ = two.SetConnectionTimeout(400);
                    let _ = two.SetTransactionTimeout(400);
                }
                let cache = automation.CreateCacheRequest().ok()?;
                cache.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
                cache.AddProperty(UIA_ControlTypePropertyId).ok()?;
                cache.AddProperty(UIA_IsOffscreenPropertyId).ok()?;
                let all = automation.CreateTrueCondition().ok()?;
                Some(Self {
                    automation,
                    cache,
                    all,
                })
            }
        }

        /// Occupied rectangles under the taskbar window `bar` (device px). `None` when UI
        /// Automation gave nothing usable.
        pub fn occupied(&self, bar: isize, bar_rect: Rect) -> Option<Vec<Rect>> {
            let hwnd = HWND(bar as *mut core::ffi::c_void);
            // SAFETY: read-only UIA queries; a stale HWND makes ElementFromHandle fail.
            let root = unsafe { self.automation.ElementFromHandle(hwnd) }.ok()?;
            let found = self.cached_scan(&root, bar_rect).filter(|v| !v.is_empty());
            found
                .or_else(|| self.walk(&root, bar_rect))
                .filter(|v| !v.is_empty())
        }

        /// One cross-process round trip: every control-view descendant with its rectangle,
        /// control type and off-screen flag cached.
        fn cached_scan(&self, root: &IUIAutomationElement, bar: Rect) -> Option<Vec<Rect>> {
            // SAFETY: read-only UIA queries on owned interfaces.
            unsafe {
                let list = root
                    .FindAllBuildCache(TreeScope_Descendants, &self.all, &self.cache)
                    .ok()?;
                let n = list.Length().ok()?.clamp(0, MAX_NODES as i32);
                let mut out = Vec::new();
                for i in 0..n {
                    let Ok(e) = list.GetElement(i) else { continue };
                    if e.CachedIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) {
                        continue;
                    }
                    let Ok(kind) = e.CachedControlType() else {
                        continue;
                    };
                    if !OCCUPYING.contains(&kind.0) {
                        continue;
                    }
                    if let Ok(r) = e.CachedBoundingRectangle() {
                        let r = rect(r);
                        if keep(r, bar) {
                            out.push(r);
                        }
                    }
                }
                Some(out)
            }
        }

        /// Fallback: raw-view walk with live property reads (slower), capped at 1600 nodes.
        fn walk(&self, root: &IUIAutomationElement, bar: Rect) -> Option<Vec<Rect>> {
            // SAFETY: read-only UIA queries on owned interfaces.
            unsafe {
                let walker = self.automation.RawViewWalker().ok()?;
                let mut pending = vec![walker.GetFirstChildElement(root).ok()?];
                let mut visited = 0usize;
                let mut out = Vec::new();
                while let Some(e) = pending.pop() {
                    visited += 1;
                    if visited > MAX_NODES {
                        break;
                    }
                    if let Ok(next) = walker.GetNextSiblingElement(&e) {
                        pending.push(next);
                    }
                    if e.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) {
                        continue;
                    }
                    if let Ok(child) = walker.GetFirstChildElement(&e) {
                        pending.push(child);
                    }
                    let Ok(kind) = e.CurrentControlType() else {
                        continue;
                    };
                    if !OCCUPYING.contains(&kind.0) {
                        continue;
                    }
                    if let Ok(r) = e.CurrentBoundingRectangle() {
                        let r = rect(r);
                        if keep(r, bar) {
                            out.push(r);
                        }
                    }
                }
                Some(out)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_filters_containers_and_strays() {
        let bar = Rect {
            left: 0,
            top: 1032,
            right: 1920,
            bottom: 1080,
        };
        assert!(keep(
            Rect {
                left: 10,
                top: 1036,
                right: 50,
                bottom: 1076
            },
            bar
        ));
        assert!(!keep(
            Rect {
                left: 0,
                top: 1032,
                right: 1200,
                bottom: 1080
            },
            bar
        ));
        assert!(!keep(
            Rect {
                left: 10,
                top: 100,
                right: 50,
                bottom: 140
            },
            bar
        ));
        assert!(!keep(
            Rect {
                left: 10,
                top: 1036,
                right: 10,
                bottom: 1076
            },
            bar
        ));
    }
}
