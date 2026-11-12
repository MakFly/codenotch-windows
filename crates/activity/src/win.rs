//! Win32 process view for "seen clears it": foreground window's process + the process table.

use crate::sessions::{ForegroundProbe, ProcessView};

/// The real desktop.
pub struct SystemProbe;

#[cfg(windows)]
impl ForegroundProbe for SystemProbe {
    fn snapshot(&self) -> Option<ProcessView> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        // SAFETY: plain Win32 queries with valid out-pointers.
        let foreground = unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            pid
        };
        if foreground == 0 {
            return None;
        }
        Some(ProcessView {
            foreground,
            procs: process_table(),
        })
    }
}

#[cfg(not(windows))]
impl ForegroundProbe for SystemProbe {
    fn snapshot(&self) -> Option<ProcessView> {
        None
    }
}

/// `pid → (parent pid, exe name)` for every process.
#[cfg(windows)]
pub fn process_table() -> std::collections::HashMap<u32, (u32, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = std::collections::HashMap::new();
    // SAFETY: snapshot handle checked and closed; entry struct sized as required.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            out.insert(entry.th32ProcessID, (entry.th32ParentProcessID, name));
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
pub fn process_table() -> std::collections::HashMap<u32, (u32, String)> {
    std::collections::HashMap::new()
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn process_table_contains_this_process() {
        let t = super::process_table();
        let me = std::process::id();
        let (_, name) = t.get(&me).expect("own pid listed");
        assert!(name.to_ascii_lowercase().ends_with(".exe"));
    }
}
