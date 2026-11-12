//! Waiting for another process to end (the previous version, after a self-update restart).

use std::time::Duration;

/// Wait until process `pid` has exited, at most `timeout`. True when it is gone (or never
/// existed / cannot be opened, which for our purpose is the same), false on timeout.
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    #[cfg(windows)]
    {
        use crate::win::OwnedHandle;
        use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        if pid == 0 || pid == std::process::id() {
            return true;
        }
        // SAFETY: plain query for a SYNCHRONIZE handle; null when the process is gone.
        let Some(handle) = OwnedHandle::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) })
        else {
            return true;
        };
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid process handle owned by `handle` for the whole call.
        let r = unsafe { WaitForSingleObject(handle.raw(), ms) };
        r != WAIT_TIMEOUT
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, timeout);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gone_and_self_return_at_once() {
        assert!(wait_for_exit(0, Duration::from_secs(5)));
        assert!(wait_for_exit(std::process::id(), Duration::from_secs(5)));
        // Process ids are multiples of 4: this one does not exist.
        assert!(wait_for_exit(0xFFFF_FFF1, Duration::from_secs(5)));
    }

    #[cfg(windows)]
    #[test]
    fn waits_for_a_child_to_exit() {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(wait_for_exit(child.id(), Duration::from_secs(10)));
        assert!(child.wait().unwrap().success());
    }
}
