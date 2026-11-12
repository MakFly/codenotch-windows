//! Small rotating file logger for the `log` facade, writing `<data dir>\logs\codenotch.log`.
//!
//! * Rotation: when the file passes 1 MiB it becomes `codenotch.1.log` (older ones shift up to
//!   `codenotch.3.log`, the oldest is deleted).
//! * Every message goes through [`redact`] (best effort). The rule stays: **never log a secret**.
//! * Call [`init`] once from `main`; libraries only use `log::info!` etc.
//! * The stderr mirror can be coloured (ANSI, see [`stderr_color`]); the log file never is.
//! * Records without a target (gpui's `log_err`, whose crate lives outside a `crates/` folder)
//!   show `crate (file:line)` instead, and a few known-harmless dependency messages are
//!   demoted (see [`effective_level`]).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const LOG_FILE: &str = "codenotch.log";
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;
pub const KEEP_ROTATED: usize = 3;

struct FileLogger {
    dir: PathBuf,
    level: log::LevelFilter,
    file: Mutex<Option<File>>,
    also_stderr: bool,
    /// ANSI colours on the stderr mirror.
    color: bool,
}

impl FileLogger {
    fn path(&self) -> PathBuf {
        self.dir.join(LOG_FILE)
    }

    fn open(&self) -> Option<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())
            .ok()
    }

    fn rotate_if_needed(&self, slot: &mut Option<File>) {
        let size = std::fs::metadata(self.path()).map(|m| m.len()).unwrap_or(0);
        if size < MAX_LOG_BYTES {
            return;
        }
        *slot = None; // close before renaming (Windows)
        rotate(&self.dir);
        *slot = self.open();
    }
}

/// Shift `codenotch.N.log` up by one and move the live file to `.1`.
fn rotate(dir: &Path) {
    let name = |i: usize| dir.join(format!("codenotch.{i}.log"));
    let _ = std::fs::remove_file(name(KEEP_ROTATED));
    for i in (1..KEEP_ROTATED).rev() {
        let _ = std::fs::rename(name(i), name(i + 1));
    }
    let _ = std::fs::rename(dir.join(LOG_FILE), name(1));
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = effective_level(record);
        if level > self.level {
            return;
        }
        // Quiet noisy dependencies below warn.
        let target = display_target(record);
        let ours = target.starts_with("codenotch");
        if !ours && level > log::Level::Warn {
            return;
        }
        let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let message = redact(&record.args().to_string());
        if self.also_stderr {
            eprint!(
                "{}",
                format_line(&time, level, &target, &message, self.color)
            );
        }
        let line = format_line(&time, level, &target, &message, false);
        if let Ok(mut slot) = self.file.lock() {
            self.rotate_if_needed(&mut slot);
            if slot.is_none() {
                *slot = self.open();
            }
            if let Some(file) = slot.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut slot) = self.file.lock() {
            if let Some(f) = slot.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// One log line (with its `\n`). `color` adds ANSI styles: dim time, coloured level, dim cyan
/// target. Never pass `color` for the log file.
fn format_line(time: &str, level: log::Level, target: &str, message: &str, color: bool) -> String {
    if !color {
        return format!("{time} {level:<5} {target}: {message}\n");
    }
    let style = match level {
        log::Level::Error => "1;31",
        log::Level::Warn => "33",
        log::Level::Info => "32",
        log::Level::Debug => "36",
        log::Level::Trace => "2",
    };
    format!(
        "\x1b[2m{time}\x1b[0m \x1b[{style}m{level:<5}\x1b[0m \x1b[2;36m{target}\x1b[0m: {message}\n"
    )
}

/// The record's target, or `crate (path:line)` when it has none. gpui's `log_err` derives the
/// target from a `crates/<name>/src/` path, which the registry copy does not have, so its
/// errors would otherwise print as `ERROR : ...` with no hint of where they come from.
fn display_target(record: &log::Record) -> String {
    let target = record.target();
    if !target.is_empty() {
        return target.to_string();
    }
    fallback_target(record.file(), record.line())
}

fn fallback_target(file: Option<&str>, line: Option<u32>) -> String {
    let Some(file) = file else {
        return "?".to_string();
    };
    let file = file.replace('\\', "/");
    let (krate, rel) = match file.rfind("/src/") {
        Some(i) => {
            let dir = file[..i].rsplit('/').next().unwrap_or("");
            // "gpui-0.2.2" -> "gpui"
            let name = dir
                .char_indices()
                .find(|&(j, c)| c == '-' && dir[j + 1..].starts_with(|d: char| d.is_ascii_digit()))
                .map_or(dir, |(j, _)| &dir[..j]);
            (name.to_string(), file[i + 5..].to_string())
        }
        None => (
            String::new(),
            file.rsplit('/').next().unwrap_or(&file).to_string(),
        ),
    };
    let at = match line {
        Some(l) => format!("{rel}:{l}"),
        None => rel,
    };
    if krate.is_empty() {
        format!("({at})")
    } else {
        format!("{krate} ({at})")
    }
}

/// Level after demoting known-harmless dependency noise to `Debug` (which the filter on
/// non-codenotch targets then drops):
/// * debug builds of gpui ask for the DXGI debug layer, only present when the optional Windows
///   "Graphics Tools" feature is installed. Without it `DXGIGetDebugInterface1` fails with
///   `0x887A002D` (`DXGI_ERROR_SDK_COMPONENT_MISSING`, logged as an error by
///   `directx_devices.rs`) and gpui warns that DirectX debugging is disabled. Rendering is
///   unaffected (release builds never ask).
fn effective_level(record: &log::Record) -> log::Level {
    let level = record.level();
    if record.target().starts_with("codenotch") {
        return level;
    }
    let file = record.file().unwrap_or("").replace('\\', "/");
    if file.ends_with("platform/windows/directx_devices.rs")
        || record.target() == "gpui::platform::windows::directx_devices"
    {
        let text = record.args().to_string();
        if text.contains("887A002D") || text.contains("Failed to get DXGI debug interface") {
            return log::Level::Debug;
        }
    }
    level
}

/// Whether the stderr mirror should use ANSI colours. `NO_COLOR` (set, non-empty) turns them
/// off; `CLICOLOR_FORCE` (set, not `0`) forces them on; otherwise they are used when stderr is
/// a terminal (a console, or an MSYS/mintty pty such as Git Bash) and `enable_vt` (called only
/// then) reports that it accepts escape codes (Windows: `ENABLE_VIRTUAL_TERMINAL_PROCESSING`).
pub fn stderr_color(enable_vt: impl FnOnce() -> bool) -> bool {
    use std::io::IsTerminal;
    let var = |k: &str| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned());
    decide_color(
        var("NO_COLOR").as_deref(),
        var("CLICOLOR_FORCE").as_deref(),
        std::io::stderr().is_terminal(),
        enable_vt,
    )
}

fn decide_color(
    no_color: Option<&str>,
    force: Option<&str>,
    terminal: bool,
    enable_vt: impl FnOnce() -> bool,
) -> bool {
    if no_color.is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if force.is_some_and(|v| !v.is_empty() && v != "0") {
        // A real console still needs VT processing to render them.
        if terminal {
            enable_vt();
        }
        return true;
    }
    terminal && enable_vt()
}

/// Install the file logger (once; later calls return an error from `log`).
/// `also_stderr` mirrors lines to stderr (CLI modes, debug builds), with ANSI colours when
/// `color` (see [`stderr_color`]). The log file is always plain text.
pub fn init(
    logs_dir: &Path,
    level: log::LevelFilter,
    also_stderr: bool,
    color: bool,
) -> crate::Result<()> {
    std::fs::create_dir_all(logs_dir)
        .map_err(|e| crate::Error::io(logs_dir.display().to_string(), e))?;
    let logger = FileLogger {
        dir: logs_dir.to_path_buf(),
        level,
        file: Mutex::new(None),
        also_stderr,
        color: also_stderr && color,
    };
    log::set_boxed_logger(Box::new(logger)).map_err(|e| crate::Error::other(e.to_string()))?;
    log::set_max_level(level);
    Ok(())
}

/// Best-effort removal of secrets from a log line:
/// * `Bearer <x>` → `Bearer ***`
/// * `token=`, `key=`, `secret=`, `password=`, `code=`, `access_token=`, `refresh_token=` values → `***`
/// * `?name=` / `&name=` values when `name` contains key, token, secret, pass, pwd, auth, sig,
///   code, cred or session (`?apikey=x` → `?apikey=***`)
/// * URL user-info: `scheme://user:pass@host` → `scheme://***@host`
/// * Telegram `bot<id>:<token>` → `bot***`
/// * any run of ≥ 24 `[A-Za-z0-9_+=-]` characters mixing letters and digits → `***`
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let is_tok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=');
    let lower: String = text.to_ascii_lowercase();
    let lower_chars: Vec<char> = lower.chars().collect();
    let starts = |at: usize, pat: &str| -> bool {
        let p: Vec<char> = pat.chars().collect();
        at + p.len() <= lower_chars.len() && lower_chars[at..at + p.len()] == p[..]
    };
    const KEYS: [&str; 7] = [
        "access_token=",
        "refresh_token=",
        "token=",
        "key=",
        "secret=",
        "password=",
        "code=",
    ];
    // Query/form parameter names whose value is hidden when the name contains one of these.
    const SENSITIVE_PARAMS: [&str; 10] = [
        "key", "token", "secret", "pass", "pwd", "auth", "sig", "code", "cred", "session",
    ];
    let ends_authority = |c: char| {
        c.is_whitespace() || matches!(c, '/' | '?' | '#' | '\\' | '"' | '\'' | '<' | '>' | ')')
    };
    while i < chars.len() {
        // "scheme://user:pass@host" → "scheme://***@host"
        if starts(i, "://") {
            out.push_str("://");
            i += 3;
            let mut j = i;
            while j < chars.len() && !ends_authority(chars[j]) {
                j += 1;
            }
            if let Some(at) = chars[i..j].iter().rposition(|c| *c == '@') {
                out.push_str("***");
                i += at; // continue at the '@'
            }
            continue;
        }
        // "?name=value" / "&name=value" with a sensitive name → "?name=***"
        if matches!(chars[i], '?' | '&') {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '_' | '-' | '.'))
            {
                j += 1;
            }
            if j > i + 1 && j < chars.len() && chars[j] == '=' {
                let name: String = lower_chars[i + 1..j].iter().collect();
                if SENSITIVE_PARAMS.iter().any(|s| name.contains(s)) {
                    out.extend(&chars[i..=j]);
                    i = j + 1;
                    while i < chars.len()
                        && !chars[i].is_whitespace()
                        && !matches!(chars[i], '&' | '"' | ',' | ';' | '#' | '\'')
                    {
                        i += 1;
                    }
                    out.push_str("***");
                    continue;
                }
            }
        }
        // "bearer <x>"
        if starts(i, "bearer ") {
            out.extend(&chars[i..i + 7]);
            i += 7;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '"' {
                i += 1;
            }
            out.push_str("***");
            continue;
        }
        // "bot<digits>:" (Telegram)
        if starts(i, "bot") && i + 3 < chars.len() && chars[i + 3].is_ascii_digit() {
            let mut j = i + 3;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j < chars.len() && chars[j] == ':' {
                out.push_str("bot***");
                i = j + 1;
                while i < chars.len() && (is_tok(chars[i]) || chars[i] == ':') {
                    i += 1;
                }
                continue;
            }
        }
        if let Some(key) = KEYS
            .iter()
            .find(|k| starts(i, k) && (i == 0 || !chars[i - 1].is_ascii_alphanumeric()))
        {
            let n = key.chars().count();
            out.extend(&chars[i..i + n]);
            i += n;
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !matches!(chars[i], '&' | '"' | ',' | ';')
            {
                i += 1;
            }
            out.push_str("***");
            continue;
        }
        if is_tok(chars[i]) {
            let start = i;
            while i < chars.len() && is_tok(chars[i]) {
                i += 1;
            }
            let word = &chars[start..i];
            let has_digit = word.iter().any(|c| c.is_ascii_digit());
            let has_alpha = word.iter().any(|c| c.is_ascii_alphabetic());
            if word.len() >= 24 && has_digit && has_alpha {
                out.push_str("***");
            } else {
                out.extend(word);
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_secret_shapes() {
        assert_eq!(
            redact("Authorization: Bearer abc.def"),
            "Authorization: Bearer ***"
        );
        assert_eq!(redact("GET /x?token=abc&y=1"), "GET /x?token=***&y=1");
        assert_eq!(
            redact("https://api.telegram.org/bot12345:AAH-xyz_123/sendMessage"),
            "https://api.telegram.org/bot***/sendMessage"
        );
        assert_eq!(
            redact("key sk-ant-oat01-AbCdEf0123456789AbCdEf0123 end"),
            "key *** end"
        );
        assert_eq!(redact("refresh_token=zz, ok"), "refresh_token=***, ok");
    }

    #[test]
    fn redacts_query_parameters_and_user_info() {
        assert_eq!(
            redact("GET https://h/u?apikey=abc1&x=1"),
            "GET https://h/u?apikey=***&x=1"
        );
        assert_eq!(
            redact("https://h/u?x=1&api_key=abc&ApiKey=def"),
            "https://h/u?x=1&api_key=***&ApiKey=***"
        );
        assert_eq!(
            redact("https://h/u?sig=s1&auth=a1&pwd=p1&session_id=z"),
            "https://h/u?sig=***&auth=***&pwd=***&session_id=***"
        );
        assert_eq!(
            redact("https://user:pa55@host.example/x"),
            "https://***@host.example/x"
        );
        assert_eq!(
            redact("http://u:p@127.0.0.1:8080/x: Connection Failed"),
            "http://***@127.0.0.1:8080/x: Connection Failed"
        );
        assert_eq!(redact("\"https://a:b@c\""), "\"https://***@c\"");
        assert_eq!(
            redact("https://host/x?page=2&lang=fr"),
            "https://host/x?page=2&lang=fr"
        );
        assert_eq!(redact("mail me at a@b.c"), "mail me at a@b.c");
        for s in [
            "http://alice:pw@h/p?apikey=s3cret",
            "https://h/?client_secret=s3cret",
            "https://h/?accessToken=s3cret",
        ] {
            let r = redact(s);
            assert!(!r.contains("s3cret") && !r.contains("alice"), "{r}");
        }
    }

    #[test]
    fn leaves_normal_text_alone() {
        let s = "fetched https://api.anthropic.com/api/oauth/usage in 120 ms, status 200, acc-1a2b";
        assert_eq!(redact(s), s);
        assert_eq!(redact("Réinit. dans 5 min"), "Réinit. dans 5 min");
    }

    #[test]
    fn plain_and_colored_lines() {
        let plain = format_line("T", log::Level::Error, "gpui", "boom", false);
        assert_eq!(plain, "T ERROR gpui: boom\n");
        let c = format_line("T", log::Level::Error, "gpui", "boom", true);
        assert!(c.contains("\x1b[1;31mERROR\x1b[0m"), "{c:?}");
        assert!(c.starts_with("\x1b[2mT\x1b[0m") && c.ends_with(": boom\n"));
        let w = format_line("T", log::Level::Warn, "x", "m", true);
        assert!(w.contains("\x1b[33mWARN \x1b[0m"), "{w:?}");
    }

    #[test]
    fn fallback_target_names_the_crate_and_place() {
        let f = r"C:\Users\u\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-0.2.2\src\window.rs";
        assert_eq!(
            fallback_target(Some(f), Some(1121)),
            "gpui (window.rs:1121)"
        );
        let f = "/home/u/.cargo/registry/src/idx/gpui-0.2.2/src/platform/windows/events.rs";
        assert_eq!(
            fallback_target(Some(f), None),
            "gpui (platform/windows/events.rs)"
        );
        assert_eq!(fallback_target(Some("a/b/c.rs"), Some(3)), "(c.rs:3)");
        assert_eq!(fallback_target(None, None), "?");
    }

    #[test]
    fn dxgi_debug_layer_noise_is_demoted() {
        let level = |target: &str, file: &str, level: log::Level, text: &str| {
            effective_level(
                &log::Record::builder()
                    .target(target)
                    .file(Some(file))
                    .level(level)
                    .args(format_args!("{text}"))
                    .build(),
            )
        };
        let dx = r"C:\r\gpui-0.2.2\src\platform\windows\directx_devices.rs";
        let e = log::Level::Error;
        assert_eq!(
            level(
                "",
                dx,
                e,
                "Error { code: HRESULT(0x887A002D), message: \"x\" }"
            ),
            log::Level::Debug
        );
        assert_eq!(
            level(
                "gpui::platform::windows::directx_devices",
                dx,
                log::Level::Warn,
                "Failed to get DXGI debug interface. DirectX debugging features will be disabled."
            ),
            log::Level::Debug
        );
        assert_eq!(level("", dx, e, "Creating Direct3D device"), e);
        assert_eq!(
            level("", "C:/r/gpui-0.2.2/src/window.rs", e, "window not found"),
            e
        );
        assert_eq!(level("codenotch::gui", dx, e, "0x887A002D"), e);
    }

    #[test]
    fn color_choice() {
        let yes = || true;
        let no = || false;
        assert!(decide_color(None, None, true, yes));
        assert!(!decide_color(None, None, true, no), "console without VT");
        assert!(!decide_color(None, None, false, yes), "redirected");
        assert!(
            !decide_color(Some("1"), Some("1"), true, yes),
            "NO_COLOR wins"
        );
        assert!(
            decide_color(Some(""), None, true, yes),
            "empty NO_COLOR is ignored"
        );
        assert!(decide_color(None, Some("1"), false, no), "forced");
        assert!(!decide_color(None, Some("0"), false, yes));
    }

    #[test]
    fn rotation_shifts_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join(LOG_FILE), "live").unwrap();
        std::fs::write(d.join("codenotch.1.log"), "one").unwrap();
        rotate(d);
        assert_eq!(
            std::fs::read_to_string(d.join("codenotch.1.log")).unwrap(),
            "live"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("codenotch.2.log")).unwrap(),
            "one"
        );
        assert!(!d.join(LOG_FILE).exists());
    }
}
