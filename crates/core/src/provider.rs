//! The provider contract.
//!
//! A provider turns local credentials/files + HTTP into a [`Reading`]. Providers are called by
//! the engine on **worker threads** (never the UI thread), may block, and must:
//! * read files only through [`Env`] paths (tests inject a temp home),
//! * do HTTP only through [`FetchContext::http`] (tests inject [`crate::FakeHttp`]),
//! * never panic, never return a secret in any field, never log a token,
//! * return `None` for anything unknown (never 0).

use crate::config::{Account, Settings};
use crate::http::HttpClient;
use crate::model::{ProviderKind, Reading};
use crate::secrets::{resolve_secret_with, SecretKey, SecretStore};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Machine environment seen by providers. Injectable: tests build one with [`Env::for_test`].
#[derive(Debug, Clone)]
pub struct Env {
    /// `%USERPROFILE%`
    pub home: PathBuf,
    /// `%APPDATA%` (Roaming)
    pub appdata: PathBuf,
    /// `%LOCALAPPDATA%`
    pub local_appdata: PathBuf,
    /// Variables set explicitly (they win over the process environment).
    vars: HashMap<String, String>,
    /// Fall back to the real process environment for variables.
    inherit_vars: bool,
}

impl Env {
    /// The real machine.
    pub fn system() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            appdata: dirs::config_dir().unwrap_or_else(|| home.join("AppData").join("Roaming")),
            local_appdata: dirs::data_local_dir()
                .unwrap_or_else(|| home.join("AppData").join("Local")),
            home,
            vars: HashMap::new(),
            inherit_vars: true,
        }
    }

    /// Isolated environment under `root` (`root\home`, `root\AppData\Roaming`,
    /// `root\AppData\Local`), with **no** inherited variables. Folders are not created.
    pub fn for_test(root: &Path) -> Self {
        let home = root.join("home");
        Self {
            appdata: root.join("AppData").join("Roaming"),
            local_appdata: root.join("AppData").join("Local"),
            home,
            vars: HashMap::new(),
            inherit_vars: false,
        }
    }

    /// Builder: set a variable.
    pub fn with_var(mut self, name: &str, value: &str) -> Self {
        self.vars.insert(name.to_string(), value.to_string());
        self
    }

    /// Look up an environment variable (explicit first, then the process if inheriting).
    /// Empty values count as unset.
    pub fn var(&self, name: &str) -> Option<String> {
        if let Some(v) = self.vars.get(name) {
            return (!v.is_empty()).then(|| v.clone());
        }
        if self.inherit_vars {
            return std::env::var(name).ok().filter(|v| !v.is_empty());
        }
        None
    }

    /// `home\<rel>`
    pub fn home_path(&self, rel: &str) -> PathBuf {
        self.home.join(rel)
    }

    /// Profile folder of `account`, or `home\<default_dir>` when the account has none.
    pub fn profile_dir(&self, account: &Account, default_dir: &str) -> PathBuf {
        if account.profile_path.is_empty() {
            self.home.join(default_dir)
        } else {
            PathBuf::from(&account.profile_path)
        }
    }
}

/// Everything a single fetch may use besides the account.
#[derive(Clone)]
pub struct FetchContext {
    /// Clock (unix secs). Use this, not the system clock, so tests are deterministic.
    pub now: i64,
    /// The last reading for this account, if any (for carry-over and back-off decisions).
    pub previous: Option<Reading>,
    pub http: Arc<dyn HttpClient>,
    pub env: Env,
    /// Current settings (language, Antigravity lane choice …). Read-only.
    pub settings: Settings,
    /// `true` when the user asked for this refresh (click / menu), `false` for the scheduler.
    pub manual: bool,
    /// Secret store (Credential Manager in the app) holding account tokens saved from the
    /// settings window (`SecretKey::Account(id)`). `None` = environment variables only.
    pub secrets: Option<Arc<dyn SecretStore>>,
}

impl FetchContext {
    pub fn new(now: i64, http: Arc<dyn HttpClient>, env: Env) -> Self {
        Self {
            now,
            previous: None,
            http,
            env,
            settings: Settings::default(),
            manual: false,
            secrets: None,
        }
    }

    /// Token of `account`: the environment variable named by `account.secret_env` first, then
    /// `SecretKey::Account(account.id)` in [`FetchContext::secrets`] (the order of
    /// [`resolve_secret_with`]). Trimmed; empty = `None`. Never log the result.
    pub fn account_secret(&self, account: &Account) -> Option<String> {
        let store: &dyn SecretStore = match &self.secrets {
            Some(s) => s.as_ref(),
            None => &crate::secrets::NoSecrets,
        };
        let config = crate::config::Config {
            accounts: vec![account.clone()],
            ..Default::default()
        };
        resolve_secret_with(
            &SecretKey::Account(account.id.clone()),
            Some(&config),
            &self.env,
            store,
        )
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    }
}

impl std::fmt::Debug for FetchContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchContext")
            .field("now", &self.now)
            .field("manual", &self.manual)
            .field("secrets", &self.secrets.is_some())
            .finish()
    }
}

/// Who is signed in to an account and on which plan, as far as the provider's local files say
/// (no HTTP, no child process). Neither field is a secret; `None` = the files do not say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalIdentity {
    /// E-mail or login.
    pub identity: Option<String>,
    /// Plan label, in the same form the provider's readings use ("Max 20x", "Pro" …).
    pub plan: Option<String>,
}

impl LocalIdentity {
    /// Fill the reading's missing identity / plan (a value the fetch found always wins).
    pub fn fill(&self, reading: &mut Reading) {
        if reading.identity.is_none() {
            reading.identity.clone_from(&self.identity);
        }
        if reading.plan.is_none() {
            reading.plan.clone_from(&self.plan);
        }
    }
}

/// One provider implementation (one per [`ProviderKind`]).
pub trait Provider: Send + Sync {
    /// Which kind this implementation serves.
    fn kind(&self) -> ProviderKind;

    /// Accounts found on this machine that are not configured yet (onboarding / "Add").
    /// Must be cheap (file checks only, no HTTP). Ids must be fresh (`Account::new`).
    fn discover(&self, env: &Env) -> Vec<Account>;

    /// Identity and plan of `account` from local files only (no HTTP, no child process), so
    /// they show even when the usage read fails or before it ran. Default: nothing known.
    fn local_identity(&self, account: &Account, env: &Env) -> LocalIdentity {
        let _ = (account, env);
        LocalIdentity::default()
    }

    /// Read the quota of one account. Blocking; never panics; errors become a `Reading`
    /// with the matching [`crate::ReadingStatus`].
    fn fetch(&self, account: &Account, ctx: &FetchContext) -> Reading;

    /// Minimum delay between two automatic fetches of one account (the engine uses
    /// `max(min_interval, settings.poll_interval())`).
    fn min_interval(&self) -> Duration {
        Duration::from_secs(60)
    }

    /// Start an interactive sign-in (opens a console/browser). Default: unsupported.
    fn sign_in(&self, account: &Account, env: &Env) -> crate::Result<()> {
        let _ = (account, env);
        Err(crate::Error::unsupported(format!(
            "sign-in for {}",
            self.kind().display_name()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_is_isolated() {
        let env = Env::for_test(Path::new("Z:/t"));
        assert_eq!(env.home, PathBuf::from("Z:/t/home"));
        assert_eq!(env.var("PATH"), None);
        let env = env.with_var("MY_TOKEN_ENV", "v").with_var("EMPTY", "");
        assert_eq!(env.var("MY_TOKEN_ENV").as_deref(), Some("v"));
        assert_eq!(env.var("EMPTY"), None);
    }

    #[test]
    fn account_secret_env_first_then_store() {
        use crate::secrets::MemorySecrets;
        let mut a = Account::new(ProviderKind::Custom, "c");
        let env = Env::for_test(Path::new("Z:/t"));
        let http: Arc<dyn HttpClient> = Arc::new(crate::http::FakeHttp::new());
        let mut ctx = FetchContext::new(0, http, env.clone());
        assert_eq!(ctx.account_secret(&a), None);
        let store = Arc::new(MemorySecrets::new());
        store
            .set(&SecretKey::Account(a.id.clone()), " stored ")
            .unwrap();
        ctx.secrets = Some(store.clone());
        assert_eq!(ctx.account_secret(&a).as_deref(), Some("stored"));
        a.secret_env = "MY_TOKEN".into();
        assert_eq!(
            ctx.account_secret(&a).as_deref(),
            Some("stored"),
            "unset variable falls back to the store"
        );
        ctx.env = env.with_var("MY_TOKEN", "from-env");
        assert_eq!(ctx.account_secret(&a).as_deref(), Some("from-env"));
        assert!(!format!("{ctx:?}").contains("stored"));
    }

    #[test]
    fn local_identity_fills_only_gaps() {
        let a = Account::new(ProviderKind::Cursor, "c");
        let local = LocalIdentity {
            identity: Some("me@example.test".into()),
            plan: Some("Ultra".into()),
        };
        let mut r = Reading::signed_out(&a, 1);
        local.fill(&mut r);
        assert_eq!(r.identity.as_deref(), Some("me@example.test"));
        assert_eq!(r.plan.as_deref(), Some("Ultra"));
        let mut r = Reading::new(&a, 1);
        r.plan = Some("Pro".into());
        local.fill(&mut r);
        assert_eq!(r.plan.as_deref(), Some("Pro"), "the fetched plan wins");
    }

    #[test]
    fn profile_dir_defaults_to_home() {
        let env = Env::for_test(Path::new("Z:/t"));
        let mut a = Account::new(ProviderKind::Claude, "c");
        assert_eq!(
            env.profile_dir(&a, ".claude"),
            PathBuf::from("Z:/t/home/.claude")
        );
        a.profile_path = "Q:/p".into();
        assert_eq!(env.profile_dir(&a, ".claude"), PathBuf::from("Q:/p"));
    }
}
