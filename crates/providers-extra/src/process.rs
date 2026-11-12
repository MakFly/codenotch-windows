//! Hidden, bounded child processes.
//!
//! * [`run_hidden`]: plain pipes, `CREATE_NO_WINDOW`, stdout capped, hard timeout, and the whole
//!   process tree in a kill-on-close job object so nothing outlives the call (`gh auth token`,
//!   the Qoder Node sidecar and the daemon it starts, `powershell`, `netstat`).
//! * [`run_pseudo_console`] (Windows): the same guarantees for programs that print nothing to a
//!   redirected pipe (Antigravity's `agy`), run inside a ConPTY pseudo-console.
//!
//! Both refuse to start anything when `CODENOTCH_NO_SPAWN=1` (checked by callers through
//! [`crate::util::spawn_disabled`], and again here).

use codenotch_core::provider::Env;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

/// Result of a finished child.
#[derive(Debug)]
pub(crate) struct Output {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
}

/// Kill-on-close job object (process-tree cleanup).
#[cfg(windows)]
pub(crate) struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    pub(crate) fn new() -> Option<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: plain Win32 calls with valid (null) attributes; the handle is owned by `Job`.
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let job = Job(h);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (ok != 0).then_some(job)
        }
    }

    /// Put a process (raw handle) in the job.
    pub(crate) fn assign(&self, process: windows_sys::Win32::Foundation::HANDLE) -> bool {
        // SAFETY: both handles are valid for the duration of the call.
        unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, process) != 0
        }
    }

    /// Kill every process in the job now.
    pub(crate) fn terminate(&self) {
        // SAFETY: valid job handle.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: we own the handle; closing the last handle kills the remaining processes.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Spawn `program args…` hidden (`CREATE_NO_WINDOW`, stdin/stderr null, stdout piped), wait at
/// most `timeout`, keep at most `max_stdout` bytes. The child and its descendants are killed on
/// timeout and when the call returns.
pub(crate) fn run_hidden(
    env: &Env,
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
    max_stdout: usize,
) -> Result<Output, String> {
    use std::process::{Command, Stdio};
    if crate::util::spawn_disabled(env) {
        return Err("process launch disabled (CODENOTCH_NO_SPAWN=1)".into());
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let name = program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {name}: {}", e.kind()))?;
    #[cfg(windows)]
    let job = Job::new().filter(|job| {
        use std::os::windows::io::AsRawHandle;
        job.assign(child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE)
    });
    let mut stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let room = (max_stdout + 1).saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..n.min(room)]);
        }
        kept
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Ok(None) => break None,
            Err(_) => break None,
        }
    };
    if status.is_none() {
        #[cfg(windows)]
        if let Some(job) = &job {
            job.terminate();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    // Closing the job kills any descendant still holding the pipe, so the reader always ends.
    #[cfg(windows)]
    drop(job);
    let stdout = reader
        .join()
        .map_err(|_| "output reader failed".to_string())?;
    let Some(status) = status else {
        return Err(format!("{name} timed out after {} s", timeout.as_secs()));
    };
    if stdout.len() > max_stdout {
        return Err(format!("{name} printed too much output"));
    }
    Ok(Output {
        success: status.success(),
        code: status.code(),
        stdout,
    })
}

/// Windows `CommandLineToArgvW` quoting of one argument.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn quote_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".into();
    }
    if !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut slashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// Sorted, double-NUL-terminated UTF-16 environment block: `vars` plus `extra` (which replaces a
/// same-named variable, case-insensitively).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn environment_block(
    vars: impl IntoIterator<Item = (String, String)>,
    extra: &[(&str, &str)],
) -> Vec<u16> {
    let mut all: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| {
            !k.is_empty()
                && !k.starts_with('=')
                && !extra.iter().any(|(e, _)| e.eq_ignore_ascii_case(k))
        })
        .chain(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    all.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in &all {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    if all.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Run `program args…` attached to a hidden ConPTY pseudo-console inside a kill-on-close job.
/// Returns the raw console output (≤ `max_output` bytes). The child is created suspended, put in
/// the job, then resumed, so no descendant can escape the clean-up.
///
/// No `CREATE_NO_WINDOW` here on purpose: with it the child gets its own hidden console instead of
/// the pseudo-console and nothing it prints is captured. A pseudo-console never shows a window.
#[cfg(windows)]
pub(crate) fn run_pseudo_console(
    env: &Env,
    program: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    extra_env: &[(&str, &str)],
    timeout: Duration,
    max_output: usize,
) -> Result<Vec<u8>, String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Console::{
        ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES,
        STARTUPINFOEXW,
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: owned handle, closed once.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }
    struct Pty(HPCON);
    impl Drop for Pty {
        fn drop(&mut self) {
            // SAFETY: owned pseudo-console, closed once.
            unsafe { ClosePseudoConsole(self.0) }
        }
    }
    struct AttrList(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttrList {
        fn drop(&mut self) {
            // SAFETY: the list was initialised by InitializeProcThreadAttributeList.
            unsafe { DeleteProcThreadAttributeList(self.0) }
        }
    }

    if crate::util::spawn_disabled(env) {
        return Err("process launch disabled (CODENOTCH_NO_SPAWN=1)".into());
    }
    if !program.is_file() {
        return Err("program not found".into());
    }
    let job = Job::new().ok_or("cannot create a job object")?;

    // Pipes: we write `in_write` (never used, kept open), the console writes `out_write`.
    let (mut in_read, mut in_write, mut out_read, mut out_write): (HANDLE, HANDLE, HANDLE, HANDLE) = (
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    // SAFETY: out-pointers to locals; handles are wrapped right after.
    unsafe {
        if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
            return Err("cannot create the input pipe".into());
        }
    }
    let (in_read, _in_write) = (Handle(in_read), Handle(in_write));
    // SAFETY: as above.
    unsafe {
        if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
            return Err("cannot create the output pipe".into());
        }
    }
    let out_write = Handle(out_write);
    // SAFETY: `out_read` is a fresh pipe handle; `File` takes ownership and closes it.
    let mut out_file = unsafe { std::fs::File::from_raw_handle(out_read as _) };

    let mut hpc: HPCON = 0;
    // SAFETY: valid pipe handles; `hpc` receives the console.
    let hr = unsafe {
        CreatePseudoConsole(COORD { X: 160, Y: 60 }, in_read.0, out_write.0, 0, &mut hpc)
    };
    // The console holds its own duplicates now.
    drop(in_read);
    drop(out_write);
    if hr < 0 {
        return Err(format!("cannot create a pseudo-console (0x{hr:08X})"));
    }
    let pty = Pty(hpc);

    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = out_file.read(&mut chunk) {
            if n == 0 {
                break;
            }
            let room = (max_output + 1).saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..n.min(room)]);
        }
        kept
    });

    let mut size = 0usize;
    // SAFETY: size query with a null list is the documented first call (it "fails" by design).
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
    }
    let mut storage = vec![0u8; size.max(64)];
    let list: LPPROC_THREAD_ATTRIBUTE_LIST = storage.as_mut_ptr() as _;
    // SAFETY: `storage` is large enough (size from the query) and outlives the list.
    let list = unsafe {
        if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
            return Err("cannot initialise the attribute list".into());
        }
        AttrList(list)
    };
    // SAFETY: the attribute value is the HPCON itself (by value), as documented for this attribute.
    unsafe {
        if UpdateProcThreadAttribute(
            list.0,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            pty.0 as *const core::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) == 0
        {
            return Err("cannot attach the pseudo-console".into());
        }
    }

    let mut cmdline = quote_arg(&program.to_string_lossy());
    for a in args {
        cmdline.push(' ');
        cmdline.push_str(&quote_arg(a));
    }
    let mut cmdline_w: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let program_w: Vec<u16> = program
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let cwd_w: Option<Vec<u16>> = cwd.map(|d| {
        d.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    });
    let env_block = environment_block(
        std::env::vars_os().map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        }),
        extra_env,
    );

    let mut si: STARTUPINFOEXW = STARTUPINFOEXW::default();
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    // Null std handles + USESTDHANDLES: nothing inherited can bypass the pseudo-console.
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.lpAttributeList = list.0;
    let mut pi: PROCESS_INFORMATION = PROCESS_INFORMATION::default();
    // SAFETY: every pointer refers to a live, NUL-terminated buffer owned by this frame.
    let created = unsafe {
        CreateProcessW(
            program_w.as_ptr(),
            cmdline_w.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_ptr() as *const core::ffi::c_void,
            cwd_w.as_ref().map_or(std::ptr::null(), |w| w.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        )
    };
    if created == 0 {
        drop(pty);
        let _ = reader.join();
        return Err("cannot start the program".into());
    }
    let process = Handle(pi.hProcess);
    let thread = Handle(pi.hThread);
    if !job.assign(process.0) {
        // SAFETY: valid process handle.
        unsafe {
            TerminateProcess(process.0, 1);
        }
        drop(pty);
        let _ = reader.join();
        return Err("cannot put the program in a job object".into());
    }
    // SAFETY: valid thread handle of the suspended main thread.
    let resumed = unsafe { ResumeThread(thread.0) } != u32::MAX;
    drop(thread);
    if !resumed {
        job.terminate();
        drop(pty);
        let _ = reader.join();
        return Err("cannot resume the program".into());
    }

    let started = Instant::now();
    let mut exit_code = 0u32;
    let mut timed_out = false;
    loop {
        // SAFETY: valid process handle.
        if unsafe { WaitForSingleObject(process.0, 100) } == WAIT_OBJECT_0 {
            // SAFETY: valid process handle, out-pointer to a local.
            unsafe {
                GetExitCodeProcess(process.0, &mut exit_code);
            }
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            job.terminate();
            break;
        }
    }
    // Kill what is left of the tree, then close the console so the reader sees end-of-file.
    drop(job);
    drop(process);
    drop(pty);
    let output = reader
        .join()
        .map_err(|_| "output reader failed".to_string())?;
    if timed_out {
        return Err(format!("timed out after {} s", timeout.as_secs()));
    }
    if exit_code != 0 {
        return Err(format!("exited with code {exit_code}"));
    }
    if output.len() > max_output {
        return Err("output is too large".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cmd_exe() -> Option<PathBuf> {
        let p = std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
        p.is_file().then_some(p)
    }

    #[test]
    fn quoting_follows_argv_rules() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(
            quote_arg(r"C:\Program Files\agy.exe"),
            r#""C:\Program Files\agy.exe""#
        );
        assert_eq!(quote_arg("ends with\\"), "\"ends with\\\\\"");
        assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn environment_block_is_sorted_and_overrides() {
        let vars = [
            ("Path", "C:\\bin"),
            ("agy_cli_disable_auto_update", "0"),
            ("APPDATA", "C:\\a"),
            ("=C:", "x"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));
        let block = environment_block(vars, &[("AGY_CLI_DISABLE_AUTO_UPDATE", "true")]);
        assert_eq!(block[block.len() - 2..], [0, 0]);
        let text = String::from_utf16(&block[..block.len() - 2]).unwrap();
        assert_eq!(
            text.split('\0').collect::<Vec<_>>(),
            [
                "AGY_CLI_DISABLE_AUTO_UPDATE=true",
                "APPDATA=C:\\a",
                "Path=C:\\bin"
            ]
        );
        assert_eq!(environment_block(Vec::new(), &[]), vec![0, 0]);
    }

    #[test]
    fn no_spawn_is_honoured() {
        let env = Env::for_test(Path::new("Z:/x")).with_var("CODENOTCH_NO_SPAWN", "1");
        let err = run_hidden(
            &env,
            Path::new("C:/Windows/System32/cmd.exe"),
            &["/c", "echo"],
            None,
            Duration::from_secs(1),
            10,
        )
        .unwrap_err();
        assert!(err.contains("disabled"));
    }

    #[test]
    fn hidden_run_captures_and_times_out() {
        let Some(cmd) = cmd_exe() else { return };
        let env = Env::for_test(Path::new("Z:/x"));
        let out = run_hidden(
            &env,
            &cmd,
            &["/c", "echo hello"],
            None,
            Duration::from_secs(10),
            1024,
        )
        .unwrap();
        assert!(out.success);
        assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
        let big = run_hidden(
            &env,
            &cmd,
            &["/c", "echo 0123456789012345678901234567890123456789"],
            None,
            Duration::from_secs(10),
            8,
        );
        assert!(big.unwrap_err().contains("too much"));
        let started = Instant::now();
        let slow = run_hidden(
            &env,
            &cmd,
            &["/c", "ping -n 10 127.0.0.1 >nul"],
            None,
            Duration::from_millis(300),
            1024,
        );
        assert!(slow.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[test]
    fn pseudo_console_captures_and_times_out() {
        let Some(cmd) = cmd_exe() else { return };
        let env = Env::for_test(Path::new("Z:/x"));
        let out = run_pseudo_console(
            &env,
            &cmd,
            &["/c", "echo hello from conpty"],
            None,
            &[],
            Duration::from_secs(15),
            65536,
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&out).contains("hello from conpty"));
        let started = Instant::now();
        let slow = run_pseudo_console(
            &env,
            &cmd,
            &["/c", "ping -n 10 127.0.0.1 >nul"],
            None,
            &[],
            Duration::from_millis(400),
            65536,
        );
        assert!(slow.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(run_pseudo_console(
            &env,
            Path::new(r"C:\no\such\agy.exe"),
            &[],
            None,
            &[],
            Duration::from_secs(1),
            10
        )
        .is_err());
    }
}
