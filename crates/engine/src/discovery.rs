//! Merging discovered accounts into the config (add only; never remove or rename).

use codenotch_core::config::RemovedAccount;
use codenotch_core::provider::Provider;
use codenotch_core::{Account, Config, Env};
use std::path::PathBuf;

/// Run every provider's `discover()` (file checks only; may still touch the disk: worker thread).
pub fn discover_all(registry: &[Box<dyn Provider>], env: &Env) -> Vec<Account> {
    let mut found = Vec::new();
    for p in registry {
        let kind = p.kind();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.discover(env))) {
            Ok(list) => found.extend(list),
            Err(_) => log::error!("discovery for {kind} panicked"),
        }
    }
    found
}

/// Folder identity of an account: its profile folder, or the provider's default folder when it
/// has none (so a discovered default profile matches a configured explicit `~\.claude`).
pub fn profile_key(account: &Account, env: &Env) -> String {
    let dir = if account.profile_path.trim().is_empty() {
        codenotch_providers::default_profile_dir(env, &account.provider)
    } else {
        Some(PathBuf::from(account.profile_path.trim()))
    };
    dir.map(|d| {
        d.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    })
    .unwrap_or_default()
}

/// Is `candidate` already configured (same provider and same profile folder)?
pub fn is_configured(config: &Config, candidate: &Account, env: &Env) -> bool {
    let key = profile_key(candidate, env);
    config.accounts.iter().any(|a| {
        a.provider == candidate.provider && !a.uses_custom_endpoint() && profile_key(a, env) == key
    })
}

/// Did the user remove `candidate` (a `removedAccounts` marker with the same provider and
/// profile folder, an empty folder meaning the provider's default one)?
pub fn is_removed(config: &Config, candidate: &Account, env: &Env) -> bool {
    if candidate.uses_custom_endpoint() {
        return false;
    }
    config
        .settings
        .removed_accounts
        .iter()
        .any(|m| marker_matches(m, candidate, env))
}

/// `marker` names `candidate`'s profile (an empty folder = the provider's default one).
pub fn marker_matches(marker: &RemovedAccount, candidate: &Account, env: &Env) -> bool {
    if marker.provider != candidate.provider || candidate.uses_custom_endpoint() {
        return false;
    }
    let mut probe = Account::new(marker.provider.clone(), marker.name.clone());
    probe.profile_path = marker.profile_path.clone();
    profile_key(&probe, env) == profile_key(candidate, env)
}

/// Append the accounts of `found` that are not configured yet (enabled, at the end of the
/// order). Returns the accounts added.
pub fn merge(config: &mut Config, found: Vec<Account>, env: &Env) -> Vec<Account> {
    let mut added = Vec::new();
    for mut account in found {
        if is_configured(config, &account, env) || is_removed(config, &account, env) {
            continue;
        }
        account.enabled = true;
        while config.account(&account.id).is_some() {
            account.id.push('x');
        }
        config.accounts.push(account.clone());
        added.push(account);
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::ProviderKind;

    #[test]
    fn merge_adds_only_new_accounts_and_keeps_existing_ones() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let mut config = Config::default();
        let mut mine = Account::new(ProviderKind::Claude, "My Claude");
        mine.profile_path = env.home.join(".claude").to_string_lossy().into_owned() + "\\";
        mine.enabled = false;
        config.accounts.push(mine.clone());
        let mut codex_custom = Account::new(ProviderKind::Codex, "proxy");
        codex_custom.endpoint = "https://example.com/u".into();
        config.accounts.push(codex_custom);

        let default_claude = Account::new(ProviderKind::Claude, "Claude");
        let mut second = Account::new(ProviderKind::Claude, "Claude (2)");
        second.profile_path = env.home.join(".CLAUDE-2").to_string_lossy().into_owned();
        let codex = Account::new(ProviderKind::Codex, "Codex");
        let added = merge(
            &mut config,
            vec![
                default_claude,
                second.clone(),
                codex.clone(),
                second.clone(),
            ],
            &env,
        );
        assert_eq!(
            added.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Claude (2)", "Codex"]
        );
        assert_eq!(config.accounts.len(), 4);
        assert_eq!(
            config.accounts[0], mine,
            "existing account untouched (still disabled, same name)"
        );
        assert!(config.accounts[2..].iter().all(|a| a.enabled));
        // Idempotent.
        let again = merge(
            &mut config,
            vec![Account::new(ProviderKind::Claude, "Claude"), second, codex],
            &env,
        );
        assert!(again.is_empty());
        assert_eq!(config.accounts.len(), 4);
    }

    #[test]
    fn removed_accounts_are_not_rediscovered() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let mut config = Config::default();
        // Removed while its profile folder was written in full; rediscovered with an empty one
        // (the default folder): still the same profile.
        let mut gone = Account::new(ProviderKind::Claude, "Claude");
        gone.profile_path = env.home.join(".claude").to_string_lossy().into_owned();
        config
            .settings
            .removed_accounts
            .push(codenotch_core::config::RemovedAccount::of(&gone).unwrap());
        let mut second = Account::new(ProviderKind::Claude, "Claude (2)");
        second.profile_path = env.home.join(".claude-2").to_string_lossy().into_owned();
        let added = merge(
            &mut config,
            vec![Account::new(ProviderKind::Claude, "Claude"), second],
            &env,
        );
        assert_eq!(
            added.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Claude (2)"]
        );
        // Marker dropped ("Add back"): found again.
        config.settings.removed_accounts.clear();
        let added = merge(
            &mut config,
            vec![Account::new(ProviderKind::Claude, "Claude")],
            &env,
        );
        assert_eq!(added.len(), 1);
    }

    #[test]
    fn colliding_ids_are_made_unique() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let mut config = Config::default();
        let mut a = Account::new(ProviderKind::Cursor, "c");
        a.profile_path = "C:/one".into();
        let mut b = a.clone();
        b.profile_path = "C:/two".into();
        config.accounts.push(a);
        let added = merge(&mut config, vec![b], &env);
        assert_eq!(added.len(), 1);
        assert_ne!(config.accounts[0].id, config.accounts[1].id);
    }
}
