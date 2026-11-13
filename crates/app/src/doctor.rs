//! `codenotch doctor`: what the app would see on this machine. Read-only (creates nothing),
//! never prints a secret: only presence ("set", "stored", "found") of tokens and files.

use codenotch_core::format::age;
use codenotch_core::logging::redact;
use codenotch_core::model::ReadingStatus;
use codenotch_core::secrets::{SecretKey, SecretStore};
use codenotch_core::state::PersistedState;
use codenotch_core::{i18n::tr, Account, Config, Env, Lang, Paths, ProviderKind};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What `doctor` needs from the outside (injectable for tests).
pub struct Probe<'a> {
    pub paths: &'a Paths,
    pub env: &'a Env,
    pub secrets: &'a dyn SecretStore,
    /// `codenotch.exe` (the hook helper is looked for next to it).
    pub exe: Option<PathBuf>,
    pub autostart: Option<bool>,
    pub data_dir_overridden: bool,
    pub now: i64,
}

fn yes(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn status_text(s: &ReadingStatus, now: i64) -> String {
    match s {
        ReadingStatus::Fresh => "fresh".into(),
        ReadingStatus::Stale => "stale".into(),
        ReadingStatus::SignedOut => "signed out".into(),
        ReadingStatus::NotInstalled => "not installed".into(),
        ReadingStatus::Empty => "nothing to report".into(),
        ReadingStatus::RateLimited { until: Some(u) } if *u > now => {
            format!("rate limited ({} s left)", u - now)
        }
        ReadingStatus::RateLimited { .. } => "rate limited".into(),
        ReadingStatus::Error => "error".into(),
    }
}

/// `kevin@gmail.com` → `k***@gmail.com`; a login without `@` keeps its first letter only.
pub(crate) fn mask_identity(s: &str) -> String {
    let s = s.trim();
    let (user, domain) = match s.split_once('@') {
        Some((u, d)) => (u, Some(d)),
        None => (s, None),
    };
    let first: String = user.chars().take(1).collect();
    match domain {
        Some(d) => format!("{first}***@{d}"),
        None => format!("{first}***"),
    }
}

/// Folder an account reads: its own `profile_path`, else the provider's default folder.
fn profile_folder(a: &Account, env: &Env) -> Option<PathBuf> {
    let own = a.profile_path.trim();
    if own.is_empty() {
        codenotch_providers::default_profile_dir(env, &a.provider)
    } else {
        Some(PathBuf::from(own))
    }
}

/// `plan Max 20x, signed in as k***@gmail.com` from the provider's local files (the last
/// reading's plan when the files say nothing). Masked; never a token.
fn plan_line(a: &Account, env: &Env, last_plan: Option<&str>) -> String {
    let registry = codenotch_engine::registry();
    let local = codenotch_engine::provider_for(&registry, a)
        .map(|p| p.local_identity(a, env))
        .unwrap_or_default();
    let plan = local
        .plan
        .as_deref()
        .or(last_plan)
        .unwrap_or("not detected");
    match local.identity.as_deref() {
        Some(who) => format!("plan {plan}, signed in as {}", mask_identity(who)),
        None => format!("plan {plan}"),
    }
}

/// Credential file of the file-based providers (existence only).
fn credential_files(kind: &ProviderKind, profile: &Path) -> Vec<PathBuf> {
    match kind {
        ProviderKind::Claude => vec![
            profile.join(".credentials.json"),
            profile.join("credentials.json"),
        ],
        ProviderKind::Codex => vec![profile.join("auth.json")],
        _ => Vec::new(),
    }
}

fn account_lines(out: &mut String, a: &Account, p: &Probe, state: &PersistedState) {
    let _ = writeln!(
        out,
        "  - {} [{}]{}",
        a.name,
        if a.uses_custom_endpoint() {
            "custom endpoint".to_string()
        } else {
            a.provider.id().to_string()
        },
        if a.enabled { "" } else { " (turned off)" }
    );
    let profile = profile_folder(a, p.env);
    if let Some(dir) = &profile {
        let _ = writeln!(
            out,
            "      profile folder: {} ({})",
            dir.display(),
            if dir.is_dir() { "found" } else { "missing" }
        );
        let files = credential_files(&a.provider, dir);
        if !files.is_empty() && !a.uses_custom_endpoint() {
            let found = files.iter().any(|f| f.is_file());
            let _ = writeln!(
                out,
                "      credential file: {}",
                if found { "found" } else { "missing" }
            );
        }
    }
    if a.uses_custom_endpoint() {
        // Origin only: user-info, path and query can hold credentials.
        let origin = codenotch_core::http::url_origin(&a.endpoint);
        let _ = writeln!(
            out,
            "      endpoint: {}",
            if origin.is_empty() {
                "?"
            } else {
                origin.as_str()
            }
        );
    }
    if !a.uses_custom_endpoint() {
        let restored = state.restored_readings();
        let last_plan = restored.get(&a.id).and_then(|r| r.plan.as_deref());
        let _ = writeln!(out, "      {}", plan_line(a, p.env, last_plan));
    }
    let env_set = !a.secret_env.trim().is_empty() && p.env.var(a.secret_env.trim()).is_some();
    let stored = p
        .secrets
        .get(&SecretKey::Account(a.id.clone()))
        .is_some_and(|v| !v.is_empty());
    if !a.secret_env.trim().is_empty() || stored {
        let _ = writeln!(
            out,
            "      token: {}{}",
            if a.secret_env.trim().is_empty() {
                String::new()
            } else {
                format!(
                    "env {} {}, ",
                    a.secret_env.trim(),
                    if env_set { "set" } else { "NOT SET" }
                )
            },
            if stored {
                "stored in Credential Manager"
            } else {
                "nothing stored in Credential Manager"
            }
        );
    }
    match state.restored_readings().get(&a.id) {
        Some(r) => {
            let mut line = format!(
                "      last reading: {} ({})",
                status_text(&r.status, p.now),
                age(Lang::En, r.fetched_at, p.now)
            );
            if let Some(n) = r
                .note
                .as_deref()
                .or(r.error.as_deref())
                .filter(|n| !n.trim().is_empty())
            {
                let _ = write!(line, " — {}", redact(tr(Lang::En, n)));
            }
            if let Some(until) = state.backoff_until.get(&a.id).filter(|u| **u > p.now) {
                let _ = write!(line, " — backing off for {} s", until - p.now);
            }
            let _ = writeln!(out, "{line}");
        }
        None => {
            let _ = writeln!(out, "      last reading: none yet");
        }
    }
}

/// `teams:` sync switch, server, pairing, token presence and queue file. Reads `teams.json`
/// only (no file is created); never prints the token, the team id or the device id.
fn teams_line(out: &mut String, config: &Config, p: &Probe) {
    let s = &config.settings;
    let server = s.teams_origin().unwrap_or("not set");
    let token = p
        .secrets
        .get(&SecretKey::TeamsDeviceToken)
        .is_some_and(|v| !v.is_empty());
    let pairing = match codenotch_teams::state::StateFile::load(&p.paths.teams_state()) {
        Ok(Some(state)) => format!(
            "paired with \"{}\" ({})",
            state.team_name, state.binding.origin
        ),
        Ok(None) => "not paired".to_string(),
        Err(_) => "pairing file UNREADABLE".to_string(),
    };
    let _ = writeln!(
        out,
        "teams: sync {}; server {server}; {pairing}; device token: {}; outbox: {}",
        if s.teams_sync_enabled { "on" } else { "off" },
        if token { "stored" } else { "absent" },
        if p.paths.teams_outbox().is_file() {
            "present"
        } else {
            "absent"
        }
    );
}

/// The whole report.
pub fn report(p: &Probe) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "codenotch {}", codenotch_core::VERSION);
    if let Some(exe) = &p.exe {
        let _ = writeln!(out, "executable: {}", exe.display());
    }
    let root = p.paths.root();
    let _ = writeln!(
        out,
        "data folder: {}{} ({})",
        root.display(),
        if p.data_dir_overridden {
            " (from CODENOTCH_DATA_DIR)"
        } else {
            ""
        },
        if root.is_dir() {
            "exists"
        } else {
            "not created yet"
        }
    );

    // Config.
    let cfg_path = p.paths.config();
    let config = if !cfg_path.exists() {
        let _ = writeln!(
            out,
            "config: {} — not found (defaults; first start shows the welcome)",
            cfg_path.display()
        );
        Config::default()
    } else {
        match Config::load(&cfg_path) {
            Ok(c) => {
                let s = &c.settings;
                let _ = writeln!(
                    out,
                    "config: {} — OK, {} account(s), {} enabled; language {}, refresh every {} s, onboarding {}",
                    cfg_path.display(),
                    c.accounts.len(),
                    c.enabled_accounts().count(),
                    s.language,
                    s.poll_interval(),
                    if c.needs_welcome() { "pending" } else { "done" }
                );
                c
            }
            Err(err) => {
                let _ = writeln!(
                    out,
                    "config: {} — UNREADABLE: {} (the app starts on defaults)",
                    cfg_path.display(),
                    redact(&err.to_string())
                );
                Config::default()
            }
        }
    };
    let state = PersistedState::load(&p.paths.state());
    let history = codenotch_core::History::new(p.paths.history()).read_all();
    let _ = writeln!(
        out,
        "state: {} — {}",
        p.paths.state().display(),
        if p.paths.state().exists() {
            "present"
        } else {
            "absent"
        }
    );
    let _ = writeln!(
        out,
        "history: {} — {} sample(s)",
        p.paths.history().display(),
        history.len()
    );
    let _ = writeln!(
        out,
        "log: {}",
        p.paths.logs_dir().join("codenotch.log").display()
    );

    // Providers.
    let _ = writeln!(out, "providers detected on this PC (file checks only):");
    for provider in codenotch_engine::registry() {
        let kind = provider.kind();
        if kind == ProviderKind::Custom {
            continue;
        }
        let detected = !provider.discover(p.env).is_empty();
        let configured = config
            .accounts
            .iter()
            .filter(|a| a.provider == kind && !a.uses_custom_endpoint())
            .count();
        let _ = writeln!(
            out,
            "  {:<14} detected: {:<3}  configured accounts: {configured}",
            kind.display_name(),
            yes(detected)
        );
    }
    // Every account discovery finds (what the app adds at launch), with its folder and plan.
    let found = codenotch_engine::discover_all(&codenotch_engine::registry(), p.env);
    let _ = writeln!(out, "accounts detected on this PC:");
    if found.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for a in &found {
        let folder = profile_folder(a, p.env)
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "-".into());
        let _ = writeln!(
            out,
            "  - {} [{}] {folder} — {}{}",
            a.name,
            a.provider.id(),
            plan_line(a, p.env, None),
            if codenotch_engine::is_configured(&config, a, p.env) {
                ""
            } else {
                " (not in the config yet)"
            }
        );
    }
    let _ = writeln!(out, "accounts:");
    if config.accounts.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    for a in config.ordered_accounts() {
        account_lines(&mut out, a, p, &state);
    }
    let telegram =
        codenotch_core::secrets::resolve_secret(&SecretKey::TelegramToken, p.env, p.secrets)
            .is_some();
    let discord =
        codenotch_core::secrets::resolve_secret(&SecretKey::DiscordWebhook, p.env, p.secrets)
            .is_some();
    let _ = writeln!(
        out,
        "telegram token: {}; discord webhook: {}",
        if telegram { "present" } else { "absent" },
        if discord { "present" } else { "absent" }
    );

    teams_line(&mut out, &config, p);

    // Hooks.
    let hook_exe = p
        .exe
        .as_ref()
        .map(|e| e.with_file_name("codenotch-hook.exe"));
    match &hook_exe {
        Some(h) => {
            let _ = writeln!(
                out,
                "hook helper: {} ({})",
                h.display(),
                if h.is_file() { "present" } else { "MISSING" }
            );
        }
        None => {
            let _ = writeln!(out, "hook helper: unknown (no executable path)");
        }
    }
    let profiles = codenotch_activity::claude_profile_dirs(p.env);
    let _ = writeln!(
        out,
        "hooks installed: {} (setting hooksEnabled: {}); Claude profiles: {}",
        yes(codenotch_activity::hooks_installed(p.env)),
        config.settings.hooks_enabled,
        if profiles.is_empty() {
            "none".to_string()
        } else {
            profiles
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    let _ = writeln!(
        out,
        "hook port: {} (127.0.0.1); hook token file: {}",
        config.settings.hook_port,
        if p.paths.hook_token().is_file() {
            "present"
        } else {
            "absent"
        }
    );
    let _ = writeln!(
        out,
        "open at login: {} (setting launchAtStartup: {})",
        match p.autostart {
            Some(b) => yes(b),
            None => "unknown",
        },
        config.settings.launch_at_startup
    );
    let _ = writeln!(
        out,
        "global shortcut: {}",
        config
            .settings
            .hotkey
            .as_deref()
            .filter(|h| !h.trim().is_empty())
            .unwrap_or("none")
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::secrets::MemorySecrets;
    use codenotch_core::Reading;

    #[test]
    fn report_shows_presence_but_never_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("data"));
        paths.ensure().unwrap();
        let env = Env::for_test(dir.path()).with_var("MY_TOKEN_VAR", "env-s3cret");
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        std::fs::write(
            env.home.join(".claude").join(".credentials.json"),
            "{\"accessToken\":\"file-s3cret\",\"subscriptionType\":\"max\",\"rateLimitTier\":\"default_claude_max_20x\"}",
        )
        .unwrap();
        std::fs::write(
            env.home.join(".claude.json"),
            "{\"oauthAccount\":{\"emailAddress\":\"kevin@mail.example\"}}",
        )
        .unwrap();
        std::fs::create_dir_all(env.home.join(".codex-work")).unwrap();
        std::fs::write(env.home.join(".codex-work").join("auth.json"), "{}").unwrap();
        let mut config = Config::default();
        let claude = Account::new(ProviderKind::Claude, "Claude");
        let mut custom = Account::new(ProviderKind::Custom, "Proxy");
        custom.endpoint = "https://quota.example.test/usage?key=query-s3cret".into();
        custom.secret_env = "MY_TOKEN_VAR".into();
        let mut local = Account::new(ProviderKind::Custom, "Local");
        local.endpoint =
            "http://alice:pw-s3cret@127.0.0.1:8080/k/path-s3cret/usage?apikey=q-s3cret".into();
        config.accounts = vec![claude.clone(), custom.clone(), local];
        config.save(&paths.config()).unwrap();
        let mut state = PersistedState::default();
        let mut r = Reading::signed_out(&claude, 100);
        r.note = Some("Sign in to Claude Code to see usage.".into());
        state.readings.insert(claude.id.clone(), r);
        state.save(&paths.state()).unwrap();
        let store = MemorySecrets::new();
        store
            .set(&SecretKey::Account(custom.id.clone()), "stored-s3cret")
            .unwrap();
        store.set(&SecretKey::TelegramToken, "tg-s3cret").unwrap();
        let exe = dir.path().join("bin").join("codenotch.exe");
        let text = report(&Probe {
            paths: &paths,
            env: &env,
            secrets: &store,
            exe: Some(exe),
            autostart: Some(false),
            data_dir_overridden: true,
            now: 160,
        });
        assert!(
            !text.contains("s3cret")
                && !text.contains("alice")
                && !text.contains("apikey")
                && !text.contains("kevin@"),
            "{text}"
        );
        assert_eq!(mask_identity("octocat"), "o***");
        for needle in [
            "config: ",
            "3 account(s)",
            "credential file: found",
            "env MY_TOKEN_VAR set",
            "stored in Credential Manager",
            "endpoint: https://quota.example.test\n",
            "endpoint: http://127.0.0.1:8080\n",
            "last reading: signed out",
            "telegram token: present",
            "teams: sync off; server not set; not paired; device token: absent; outbox: absent",
            "hook helper: ",
            "MISSING",
            "hooks installed: no",
            "hook port: 48666",
            "open at login: no",
            "Claude         detected: yes",
            "accounts detected on this PC:",
            "plan Max 20x, signed in as k***@mail.example",
            "Codex [codex] ",
            "plan not detected (not in the config yet)",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }
}
