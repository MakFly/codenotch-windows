//! One-instance lock (named mutex) and "second launch → open settings in the first one".
//!
//! Objects live in the per-session `Local\` namespace:
//! * `Local\CodeNotch.<suffix>.Activate` — auto-reset event, created **before** the mutex, so a
//!   second instance that sees the mutex can always open it;
//! * `Local\CodeNotch.<suffix>.Instance` — the mutex; `ERROR_ALREADY_EXISTS` = another instance.
//!
//! The suffix is `Main`, or a hash of `CODENOTCH_DATA_DIR` when that variable redirects the app
//! (so a dev/test copy with its own data folder can run beside the installed one).
//!
//! The primary instance runs a small thread that waits for the event and calls the
//! [`InstanceGuard::on_activate`] callback. Dropping the guard stops the thread and releases
//! the mutex.

use crate::win::{wide, OwnedHandle};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, SetEvent, WaitForMultipleObjects, INFINITE,
};

pub enum Instance {
    /// We are the first instance; keep the guard alive for the app's lifetime.
    Primary(InstanceGuard),
    /// Another instance runs and was asked to open its settings.
    AlreadyRunning,
}

type Callback = Box<dyn Fn() + Send>;

#[derive(Default)]
struct Shared {
    callback: Option<Callback>,
    /// Activations that arrived before a callback was installed.
    pending: u32,
}

pub struct InstanceGuard {
    shared: Arc<Mutex<Shared>>,
    stop: Option<Arc<OwnedHandle>>,
    thread: Option<JoinHandle<()>>,
    // Field order: the thread is stopped (Drop) before the handles below are closed.
    _event: Option<Arc<OwnedHandle>>,
    _mutex: Option<OwnedHandle>,
}

impl InstanceGuard {
    /// Called on a private thread each time a second instance asks us to show settings.
    /// An activation that arrived before this call fires `f` once, right away (on this thread).
    pub fn on_activate(&self, f: Box<dyn Fn() + Send>) {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if shared.pending > 0 {
            shared.pending = 0;
            f();
        }
        shared.callback = Some(f);
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Some(stop) = &self.stop {
            // SAFETY: valid event handle owned by us.
            unsafe { SetEvent(stop.raw()) };
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Take the lock, or signal the running instance. Never fails: if the lock cannot be created
/// at all, we run as primary without single-instance protection (logged).
pub fn acquire() -> Instance {
    acquire_named(&name_suffix())
}

fn name_suffix() -> String {
    match std::env::var_os(codenotch_core::paths::DATA_DIR_ENV) {
        Some(dir) if !dir.is_empty() => {
            // FNV-1a over the path, case-folded: stable across runs (unlike RandomState).
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in dir.to_string_lossy().to_lowercase().bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            format!("Data{h:016x}")
        }
        _ => "Main".to_string(),
    }
}

pub(crate) fn acquire_named(suffix: &str) -> Instance {
    match try_acquire(suffix) {
        Ok(instance) => instance,
        Err(msg) => {
            log::warn!("single instance: {msg}; continuing without the lock");
            Instance::Primary(InstanceGuard::detached())
        }
    }
}

fn try_acquire(suffix: &str) -> Result<Instance, String> {
    let event_name = wide(&format!("Local\\CodeNotch.{suffix}.Activate"));
    let mutex_name = wide(&format!("Local\\CodeNotch.{suffix}.Instance"));
    // SAFETY: default security; auto-reset, initially non-signalled; NUL-terminated name.
    let event =
        OwnedHandle::new(unsafe { CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr()) })
            .ok_or("cannot create the activation event")?;
    // SAFETY: default security, not initially owned (ownership is irrelevant: existence is the lock).
    let raw_mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    let already = crate::win::last_error() == ERROR_ALREADY_EXISTS;
    let mutex = OwnedHandle::new(raw_mutex).ok_or("cannot create the instance mutex")?;
    if already {
        // We were just launched by the user, so we may set the foreground window: hand that
        // right to the running instance (we do not know its pid, hence ASFW_ANY) so its
        // settings window comes to the front instead of flashing in the taskbar.
        // SAFETY: no pointers; failure only means the primary falls back to its own unlock.
        let granted = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
            )
        } != 0;
        log::debug!("second launch: AllowSetForegroundWindow(ASFW_ANY) = {granted}");
        // SAFETY: valid event handle (shared with the primary instance).
        unsafe { SetEvent(event.raw()) };
        log::info!("another instance is running; asked it to open settings");
        return Ok(Instance::AlreadyRunning);
    }
    // SAFETY: unnamed manual-reset event used to stop our wait thread.
    let stop = OwnedHandle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
        .ok_or("cannot create the stop event")?;
    let event = Arc::new(event);
    let stop = Arc::new(stop);
    let shared = Arc::new(Mutex::new(Shared::default()));
    let thread = {
        let (event, stop, shared) = (event.clone(), stop.clone(), shared.clone());
        std::thread::Builder::new()
            .name("codenotch-instance".into())
            .spawn(move || wait_loop(&event, &stop, &shared))
            .map_err(|_| "cannot start the instance thread")?
    };
    Ok(Instance::Primary(InstanceGuard {
        shared,
        stop: Some(stop),
        thread: Some(thread),
        _event: Some(event),
        _mutex: Some(mutex),
    }))
}

fn wait_loop(event: &OwnedHandle, stop: &OwnedHandle, shared: &Mutex<Shared>) {
    let handles = [stop.raw(), event.raw()];
    loop {
        // SAFETY: two valid handles kept alive by the Arcs for the whole loop.
        let r = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
        if r == WAIT_OBJECT_0 + 1 {
            log::info!("second launch detected");
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            match &s.callback {
                Some(f) => {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                        log::error!("activation callback panicked");
                    }
                }
                None => s.pending += 1,
            }
        } else {
            // Stop requested, or the wait failed.
            return;
        }
    }
}

impl InstanceGuard {
    /// A guard that holds nothing (lock unavailable).
    fn detached() -> InstanceGuard {
        InstanceGuard {
            shared: Arc::new(Mutex::new(Shared::default())),
            stop: None,
            thread: None,
            _event: None,
            _mutex: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::test_util::random_token;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Random object names: never touches a running CodeNotch.
    #[test]
    fn second_acquire_signals_the_first() {
        let suffix = format!("Test{}", random_token());
        let Instance::Primary(guard) = acquire_named(&suffix) else {
            panic!("first acquire must be primary");
        };
        let (tx, rx) = mpsc::channel();
        guard.on_activate(Box::new(move || {
            let _ = tx.send(());
        }));
        assert!(matches!(acquire_named(&suffix), Instance::AlreadyRunning));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("activation callback");
        assert!(matches!(acquire_named(&suffix), Instance::AlreadyRunning));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("second activation");

        drop(guard);
        // Lock released: a new acquire is primary again.
        let again = acquire_named(&suffix);
        assert!(matches!(again, Instance::Primary(_)));
    }

    #[test]
    fn activation_before_callback_is_delivered_once() {
        let suffix = format!("Test{}", random_token());
        let Instance::Primary(guard) = acquire_named(&suffix) else {
            panic!("first acquire must be primary");
        };
        assert!(matches!(acquire_named(&suffix), Instance::AlreadyRunning));
        // Let the wait thread record the pending activation.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while guard.shared.lock().unwrap().pending == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (tx, rx) = mpsc::channel();
        guard.on_activate(Box::new(move || {
            let _ = tx.send(());
        }));
        rx.recv_timeout(Duration::from_secs(1))
            .expect("pending activation");
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn suffix_is_stable() {
        let a = name_suffix();
        assert_eq!(a, name_suffix());
        assert!(a == "Main" || a.starts_with("Data"));
    }
}
