use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

/// Providers whose official CLI owns an OAuth session that CodeNotch can read.
pub const PROVIDERS: [&str; 5] = ["codex", "claude", "gemini", "grok", "opencode"];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub provider: String,
    pub installed: bool,
    /// Resolved CLI, shown so the user can see which installation will be used.
    pub cli_path: Option<String>,
    pub signed_in: bool,
    /// Last write of the credential file, in milliseconds, so a new sign-in can be detected.
    pub signed_in_at: Option<i64>,
    pub profile_path: String,
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Saved user and machine PATH. A Start-menu launch keeps the PATH from logon, so a CLI
/// installed afterwards is only visible here.
#[cfg(windows)]
fn registry_path() -> Vec<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
    };
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let name = wide("Path");
    let mut found = vec![];
    for (root, key) in [
        (HKEY_CURRENT_USER, "Environment"),
        (
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        ),
    ] {
        let key = wide(key);
        let mut buffer = vec![0u16; 8192];
        for _ in 0..4 {
            let mut size = (buffer.len() * 2) as u32;
            // REG_EXPAND_SZ values are expanded because RRF_NOEXPAND is not passed.
            let status = unsafe {
                RegGetValueW(
                    root,
                    key.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if status == 234 {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if status == 0 {
                let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                found.extend(std::env::split_paths(&OsString::from_wide(&buffer[..end])));
            }
            break;
        }
    }
    found
}
#[cfg(not(windows))]
fn registry_path() -> Vec<PathBuf> {
    vec![]
}

/// Global bin folders of the Node and package managers that install these CLIs.
fn package_manager_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let local = env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData/Local"));
    let roaming = env_dir("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming"));
    // npm puts Windows shims directly in its prefix, not in <prefix>\bin.
    let npm_prefix = env_dir("NPM_CONFIG_PREFIX").or_else(|| {
        std::fs::read_to_string(home.join(".npmrc"))
            .ok()?
            .lines()
            .filter_map(|l| l.trim().strip_prefix("prefix"))
            .filter_map(|l| l.trim_start().strip_prefix('='))
            .map(|l| PathBuf::from(l.trim()))
            .find(|p| p.is_absolute())
    });
    let mut dirs: Vec<PathBuf> = npm_prefix.into_iter().collect();
    dirs.extend([
        roaming.join("npm"),
        env_dir("PNPM_HOME").unwrap_or_else(|| local.join("pnpm")),
        env_dir("BUN_INSTALL")
            .unwrap_or_else(|| home.join(".bun"))
            .join("bin"),
        env_dir("VOLTA_HOME")
            .unwrap_or_else(|| local.join("Volta"))
            .join("bin"),
        env_dir("SCOOP")
            .unwrap_or_else(|| home.join("scoop"))
            .join("shims"),
        local.join("Microsoft/WinGet/Links"),
    ]);
    dirs.extend(env_dir("NVM_SYMLINK"));
    dirs.extend(env_dir("ProgramFiles").map(|d| d.join("WinGet/Links")));
    dirs.extend(env_dir("ProgramData").map(|d| d.join("scoop/shims")));
    dirs.extend(env_dir("ChocolateyInstall").map(|d| d.join("bin")));
    dirs
}

/// Children of `dir`, newest first by numeric version when names are versions, else by date.
fn newest_children(dir: &Path) -> Vec<PathBuf> {
    let key = |p: &PathBuf| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        let version: Vec<u64> = name.split('.').map_while(|n| n.parse().ok()).collect();
        let modified = std::fs::metadata(p).and_then(|m| m.modified()).ok();
        (version, modified)
    };
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort_by_key(|p| std::cmp::Reverse(key(p)));
    children
}

/// Every place an installation of the CLI may live, most authoritative first.
fn candidates(provider: &str) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let local = env_dir("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData/Local"));
    let name = match provider {
        "codex" | "claude" | "gemini" | "grok" | "opencode" => provider,
        _ => return vec![],
    };
    let mut found = vec![];
    match provider {
        "codex" => {
            // Official standalone install.ps1, including CODEX_INSTALL_DIR and its junction target.
            found.extend(env_dir("CODEX_INSTALL_DIR").map(|d| d.join("codex.exe")));
            found.push(local.join("Programs/OpenAI/Codex/bin/codex.exe"));
            found.push(default_profile("codex").join("packages/standalone/current/bin/codex.exe"));
        }
        "claude" => found.push(home.join(".local/bin/claude.exe")),
        // Grok Build's installer keeps its binary inside the Grok home.
        "grok" => found.push(default_profile("grok").join("bin/grok.exe")),
        _ => {}
    }
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    folders.extend(registry_path());
    folders.extend(package_manager_dirs());
    for folder in folders {
        found.push(folder.join(format!("{name}.exe")));
        found.push(folder.join(format!("{name}.cmd")));
    }
    match provider {
        // CLI copy extracted by the Codex desktop app (MSIX), outside its protected package.
        "codex" => found.extend(
            newest_children(&local.join("OpenAI/Codex/bin"))
                .into_iter()
                .map(|d| d.join("codex.exe")),
        ),
        "claude" => {
            for package in newest_children(&local.join("Microsoft/WinGet/Packages")) {
                let name = package.file_name().unwrap_or_default().to_string_lossy();
                if name.starts_with("Anthropic.ClaudeCode") {
                    found.push(package.join("claude.exe"));
                }
            }
            // Claude Code bundled with the Claude desktop app:
            // Claude-Data\claude_<id>\claude-code\<version>\<hash>\claude.exe
            for app in newest_children(&local.join("Claude-Data")) {
                for version in newest_children(&app.join("claude-code")) {
                    let mut builds = newest_children(&version);
                    builds.sort_by_key(|b| !b.join(".verified").is_file());
                    found.extend(builds.into_iter().map(|b| b.join("claude.exe")));
                }
            }
        }
        _ => {}
    }
    found
}

/// Desktop apps ship executables with the same name that are not the CLI.
fn is_cli(path: &Path) -> bool {
    let text = path.to_string_lossy().to_lowercase();
    path.is_file() && !text.contains("windowsapps") && !text.contains("anthropicclaude")
}

/// Resolves the official CLI from native installers, PATH (live and saved), package managers
/// and the copies bundled with the Codex and Claude desktop apps.
pub fn executable(provider: &str) -> Option<PathBuf> {
    candidates(provider).into_iter().find(|p| is_cli(p))
}

/// Finds any CLI by its command names, in PATH (live and saved), the package managers'
/// global folders and ~/.local/bin. Used for the CLI catalog in data/cli-agents.json.
#[allow(dead_code)]
pub fn find_command<S: AsRef<str>>(names: &[S]) -> Option<PathBuf> {
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    folders.extend(registry_path());
    folders.extend(package_manager_dirs());
    folders.extend(dirs::home_dir().map(|h| h.join(".local/bin")));
    folders.into_iter().find_map(|folder| {
        names.iter().find_map(|name| {
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| folder.join(format!("{}.{ext}", name.as_ref())))
                .find(|path| is_cli(path))
        })
    })
}

/// Profile folder used by the CLI when no explicit path is configured.
pub fn default_profile(provider: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    match provider {
        "codex" => env_dir("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        "claude" => env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
        "grok" => env_dir("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
        // OpenCode follows XDG on every platform, Windows included.
        "opencode" => env_dir("XDG_DATA_HOME")
            .unwrap_or_else(|| home.join(".local/share"))
            .join("opencode"),
        _ => env_dir("GEMINI_CLI_HOME").unwrap_or(home).join(".gemini"),
    }
}

pub fn credential_file(provider: &str) -> &'static str {
    match provider {
        "codex" | "grok" | "opencode" => "auth.json",
        "claude" => ".credentials.json",
        _ => "oauth_creds.json",
    }
}

pub fn status(provider: &str, profile: Option<&Path>) -> ProviderStatus {
    let profile = profile
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_profile(provider));
    let modified = std::fs::metadata(profile.join(credential_file(provider)))
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.modified().ok());
    let cli = executable(provider);
    ProviderStatus {
        provider: provider.into(),
        installed: cli.is_some(),
        cli_path: cli.map(|p| p.to_string_lossy().into()),
        signed_in: modified.is_some(),
        signed_in_at: modified
            .flatten()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64),
        profile_path: profile.to_string_lossy().into(),
    }
}

/// Opens a provider sign-in page in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("Adresse de connexion refusée.".into());
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL,
        };
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (verb, target) = (wide("open"), wide(url));
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW reports success with a value greater than 32.
        if result as isize <= 32 {
            return Err("Impossible d’ouvrir le navigateur. Ouvre le lien affiché.".into());
        }
    }
    Ok(())
}

fn not_installed(provider: &str) -> String {
    match provider {
        "codex" => "Codex CLI introuvable. Installe l’application Codex ou « npm i -g @openai/codex », puis réessaie.",
        "claude" => "Claude Code introuvable. Installe-le depuis claude.com/download ou avec « npm i -g @anthropic-ai/claude-code », puis réessaie.",
        "grok" => "Grok CLI introuvable. Installe Grok Build depuis x.ai/cli, puis réessaie.",
        "opencode" => "OpenCode introuvable. Installe-le avec « npm i -g opencode-ai », puis réessaie.",
        _ => "Gemini CLI introuvable. Installe-le avec « npm i -g @google/gemini-cli », puis réessaie.",
    }
    .into()
}

/// Opens the provider's own sign-in in a visible console window. The CLI performs the OAuth
/// flow in the browser and writes its credentials; CodeNotch never sees the authorization code.
pub fn start(provider: &str, profile: Option<&Path>) -> Result<(), String> {
    if !PROVIDERS.contains(&provider) {
        return Err("Connecteur inconnu.".into());
    }
    let cli = executable(provider).ok_or_else(|| not_installed(provider))?;
    let cli = cli.to_string_lossy();
    // The path comes from our own lookup; still refuse characters cmd.exe would interpret.
    if cli.contains(['"', '%', '^', '!']) {
        return Err(not_installed(provider));
    }
    let args = match provider {
        "codex" | "grok" => " login",
        "claude" | "opencode" => " auth login",
        _ => "",
    };
    // `start` gives the CLI its own console even though CodeNotch is a GUI process.
    let mut cmd = std::process::Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("/d /c start \"CodeNotch\" \"{cli}\"{args}"))
            .creation_flags(0x08000000);
    }
    if let Some(dir) = profile {
        match provider {
            "codex" => {
                cmd.env("CODEX_HOME", dir);
            }
            "claude" => {
                cmd.env("CLAUDE_CONFIG_DIR", dir);
            }
            _ => {}
        }
    }
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name == "CLAUDECODE"
            || name.starts_with("CLAUDE_CODE_")
            || matches!(name.as_ref(), "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN")
        {
            cmd.env_remove(key);
        }
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|_| "Impossible d’ouvrir la fenêtre de connexion.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_providers_are_refused() {
        assert!(start("copilot", None).is_err());
        assert!(executable("custom").is_none());
    }
    #[test]
    fn bundled_versions_sort_newest_first() {
        let dir = std::env::temp_dir().join(format!("codenotch-{}", uuid::Uuid::new_v4()));
        for v in ["2.1.304", "2.1.1000", "2.1.99"] {
            std::fs::create_dir_all(dir.join(v)).unwrap();
        }
        let names: Vec<_> = newest_children(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["2.1.1000", "2.1.304", "2.1.99"]);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn desktop_app_binaries_are_not_the_cli() {
        assert!(!is_cli(Path::new(
            r"C:\Program Files\WindowsApps\Claude_1_x64__x\app\claude.exe"
        )));
    }
    #[test]
    #[ignore = "prints what this machine resolves"]
    fn print_local_detection() {
        for p in PROVIDERS {
            let s = status(p, None);
            println!(
                "{p}: cli={:?} signed_in={} profile={}",
                s.cli_path, s.signed_in, s.profile_path
            );
        }
    }
    #[test]
    fn missing_credentials_are_not_signed_in() {
        let dir = std::env::temp_dir().join(format!("codenotch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!status("gemini", Some(&dir)).signed_in);
        std::fs::write(dir.join("oauth_creds.json"), "{}").unwrap();
        let found = status("gemini", Some(&dir));
        assert!(found.signed_in && found.signed_in_at.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
