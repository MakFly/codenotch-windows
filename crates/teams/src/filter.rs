//! The privacy filter: the only code that turns local data into contract values.
//!
//! It reads exactly these fields and nothing else:
//! * `Account`: `id`, `provider`, `enabled` (and the exclusion list of the settings);
//! * `Reading`: `account_id`, `provider`, `status`, `fetched_at`, `plan`, `headline_window`,
//!   `weekly_window`, and per window `id`, `kind`, `used_percent`, `resets_at`,
//!   `window_seconds`, `group` (only to skip grouped windows);
//! * `HourlyUsage`: `hour`, `source` (compared with the root list, never copied), `provider`,
//!   `model`, token counts.
//!
//! Every string that leaves is translated into a closed set ([`Provider`], [`PlanCode`],
//! [`WindowId`], [`ModelId`]) or is an id CodeNotch generated (account reference, uuid). A
//! window that does not translate is left out; a plan or model that does not translate is
//! `unknown`. Accounts are shared only when enabled, of a provider of the contract and not
//! excluded; usage only from a scan root that belongs to exactly one such account.

use std::collections::{BTreeMap, HashMap};

use codenotch_core::analytics::{HourlyUsage, RootOwners};
use codenotch_core::model::{ReadingStatus, WindowKind as CoreKind};
use codenotch_core::{Config, Reading};
use codenotch_teams_schema::{
    ModelId, PlanCode, Provider, QuotaWindow, Tokens, WindowId, WindowKind, WindowRole,
};
use sha2::{Digest, Sha256};

/// Most windows in one snapshot (contract limit).
pub const MAX_WINDOWS: usize = 8;

/// Account reference sent instead of any account data: the first 16 hex characters of
/// `sha256(device_id + account_id)`. Differs per device, so it cannot be joined across teams.
pub fn account_ref(device_id: &str, account_id: &str) -> String {
    let digest = Sha256::digest(format!("{device_id}{account_id}").as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// One account Teams may share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedAccount {
    pub account_ref: String,
    pub provider: Provider,
    /// Usage can be attributed (it owns at least one scan root alone).
    pub usage: bool,
}

/// What may leave the device right now. Built from the current config, the current scan
/// roots and the pairing; the outbox is filtered again against it before every send.
#[derive(Clone, Default, PartialEq)]
pub struct Policy {
    /// Account id → shared account. Missing = never shared.
    accounts: HashMap<String, SharedAccount>,
    /// Scan root → the one account its usage belongs to.
    roots: HashMap<String, String>,
    /// Hours before this (unix secs) are never sent.
    floor: i64,
}

/// Counts only: account ids and scan roots (folder paths) never reach a log.
impl std::fmt::Debug for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Policy")
            .field("accounts", &self.accounts.len())
            .field("roots", &self.roots.len())
            .field("floor", &self.floor)
            .finish()
    }
}

impl Policy {
    /// `device_id` derives the account references, `floor` is the first hour that may be
    /// sent (pairing time, or erase time, or 45 days earlier with the member's agreement).
    pub fn new(config: &Config, owners: &[RootOwners], device_id: &str, floor: i64) -> Policy {
        let mut accounts = HashMap::new();
        for a in config.enabled_accounts() {
            let Some(provider) = Provider::parse(a.provider.id()) else {
                continue;
            };
            if config.settings.teams_excludes(&a.id) {
                continue;
            }
            accounts.insert(
                a.id.clone(),
                SharedAccount {
                    account_ref: account_ref(device_id, &a.id),
                    provider,
                    usage: false,
                },
            );
        }
        let mut roots = HashMap::new();
        for root in owners {
            // Exactly one enabled account; it must be shared and of the root's provider.
            let [id] = root.accounts.as_slice() else {
                continue;
            };
            let Some(shared) = accounts.get_mut(id) else {
                continue;
            };
            if Provider::parse(root.provider.id()) != Some(shared.provider) {
                continue;
            }
            shared.usage = true;
            roots.insert(root.source.clone(), id.clone());
        }
        Policy {
            accounts,
            roots,
            floor,
        }
    }

    pub fn floor(&self) -> i64 {
        self.floor
    }

    /// Move the floor (backfill agreement given or withdrawn, data erased).
    pub fn set_floor(&mut self, floor: i64) {
        self.floor = floor;
    }

    /// The shared account of `account_id`.
    pub fn account(&self, account_id: &str) -> Option<&SharedAccount> {
        self.accounts.get(account_id)
    }

    /// The shared account with this reference.
    pub fn by_ref(&self, account_ref: &str) -> Option<&SharedAccount> {
        self.accounts
            .values()
            .find(|a| a.account_ref == account_ref)
    }

    /// Quota windows and plan of a FRESH reading of a shared account, translated to the
    /// contract. `None` = nothing may be sent for this reading.
    pub fn quota(&self, reading: &Reading) -> Option<QuotaDraft> {
        if reading.status != ReadingStatus::Fresh {
            return None;
        }
        let shared = self.account(&reading.account_id)?;
        if Provider::parse(reading.provider.id()) != Some(shared.provider) {
            return None;
        }
        let provider = shared.provider;
        let headline = reading.headline().map(|w| w.id.as_str());
        let weekly = reading.weekly().map(|w| w.id.as_str());
        let mut windows: Vec<QuotaWindow> = Vec::new();
        for w in &reading.windows {
            if w.group.is_some() {
                continue;
            }
            let Some(id) = WindowId::parse(provider, &w.id) else {
                continue;
            };
            if windows.iter().any(|x| x.id == id) || windows.len() == MAX_WINDOWS {
                continue;
            }
            let role = if headline == Some(w.id.as_str()) {
                Some(WindowRole::Headline)
            } else if weekly == Some(w.id.as_str()) {
                Some(WindowRole::Weekly)
            } else {
                None
            };
            windows.push(QuotaWindow {
                id,
                kind: match w.kind {
                    CoreKind::Session => WindowKind::Session,
                    CoreKind::Weekly => WindowKind::Weekly,
                    CoreKind::Monthly => WindowKind::Monthly,
                    CoreKind::Other => WindowKind::Other,
                },
                role,
                used_percent: w
                    .used_percent
                    .filter(|p| p.is_finite())
                    .map(|p| p.clamp(0.0, 100.0)),
                resets_at: w.resets_at,
                window_seconds: w.window_seconds.filter(|s| *s >= 1),
            });
        }
        Some(QuotaDraft {
            account_ref: shared.account_ref.clone(),
            provider,
            plan: PlanCode::parse(provider, reading.plan.as_deref().unwrap_or("")),
            observed_at: reading.fetched_at,
            windows,
        })
    }

    /// Absolute token totals per natural key from the hourly export: rows of roots owned by
    /// one shared account, hours at or after the floor; several roots and models of a key
    /// add up.
    pub fn usage(&self, rows: &[HourlyUsage]) -> BTreeMap<UsageKey, Tokens> {
        let mut out: BTreeMap<UsageKey, Tokens> = BTreeMap::new();
        for row in rows {
            if row.hour < self.floor || row.hour.rem_euclid(3600) != 0 {
                continue;
            }
            let Some(shared) = self.roots.get(&row.source).and_then(|id| self.account(id)) else {
                continue;
            };
            if Provider::parse(row.provider.id()) != Some(shared.provider) {
                continue;
            }
            let key = UsageKey {
                account_ref: shared.account_ref.clone(),
                provider: shared.provider,
                model: ModelId::parse(shared.provider, &row.model),
                hour_start: row.hour,
            };
            let t = out.entry(key).or_default();
            t.input += row.tokens.input;
            t.output += row.tokens.output;
            t.cache_read += row.tokens.cache_read;
            t.cache_write += row.tokens.cache_write;
            t.cache_write_1h += row.cache_write_1h.min(row.tokens.cache_write);
        }
        out
    }
}

/// A snapshot before its sequence number is assigned.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaDraft {
    pub account_ref: String,
    pub provider: Provider,
    pub plan: PlanCode,
    pub observed_at: i64,
    pub windows: Vec<QuotaWindow>,
}

/// Natural key of a `usage.hour` (the device is implied).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UsageKey {
    pub account_ref: String,
    pub provider: Provider,
    pub model: ModelId,
    pub hour_start: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::{Account, LimitWindow, ProviderKind, TokenCounts};

    const DEVICE: &str = "6f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11";

    fn account(kind: ProviderKind, id: &str) -> Account {
        let mut a = Account::new(kind, "name");
        a.id = id.into();
        a
    }

    fn row(source: &str, kind: ProviderKind, model: &str, hour: i64, input: u64) -> HourlyUsage {
        HourlyUsage {
            hour,
            source: source.into(),
            provider: kind,
            model: model.into(),
            tokens: TokenCounts {
                input,
                ..Default::default()
            },
            cache_write_1h: 0,
        }
    }

    fn owners(list: &[(&str, ProviderKind, &[&str])]) -> Vec<RootOwners> {
        list.iter()
            .map(|(s, p, a)| RootOwners {
                source: s.to_string(),
                provider: p.clone(),
                accounts: a.iter().map(|x| x.to_string()).collect(),
            })
            .collect()
    }

    #[test]
    fn account_ref_is_16_hex_and_device_bound() {
        let r = account_ref(DEVICE, "claude-1");
        assert!(codenotch_teams_schema::validate::is_account_ref(&r));
        assert_ne!(
            r,
            account_ref("7f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11", "claude-1")
        );
        assert_eq!(r, account_ref(DEVICE, "claude-1"));
    }

    #[test]
    fn only_roots_with_exactly_one_shared_account_export_usage() {
        let mut config = Config {
            accounts: vec![
                account(ProviderKind::Claude, "in"),
                account(ProviderKind::Claude, "out"),
                account(ProviderKind::Codex, "cx"),
                account(ProviderKind::Cursor, "cursor"),
            ],
            ..Default::default()
        };
        config.settings.teams_excluded_accounts = vec!["out".into()];
        let roots = owners(&[
            ("default", ProviderKind::Claude, &[]),
            ("shared", ProviderKind::Claude, &["in", "out"]),
            ("cx-s", ProviderKind::Codex, &["cx"]),
            ("cx-a", ProviderKind::Codex, &["cx"]),
            ("excluded", ProviderKind::Claude, &["out"]),
            ("wrong-provider", ProviderKind::Claude, &["cx"]),
        ]);
        let p = Policy::new(&config, &roots, DEVICE, 0);
        assert!(p.account("in").is_some_and(|a| !a.usage));
        assert!(p.account("out").is_none() && p.account("cursor").is_none());
        let h = 7200;
        let rows = [
            row("default", ProviderKind::Claude, "claude-opus-5-5", h, 1),
            row("shared", ProviderKind::Claude, "claude-opus-5-5", h, 2),
            row("excluded", ProviderKind::Claude, "claude-opus-5-5", h, 4),
            row(
                "wrong-provider",
                ProviderKind::Claude,
                "claude-opus-5-5",
                h,
                8,
            ),
            row("unknown-root", ProviderKind::Codex, "gpt-5", h, 16),
            row("cx-s", ProviderKind::Codex, "gpt-5", h, 32),
            row("cx-a", ProviderKind::Codex, "gpt-5", h, 64),
            row("cx-a", ProviderKind::Codex, "C_Users_Kev_client", h, 128),
            row("cx-a", ProviderKind::Codex, "gpt-5", h + 1, 256),
        ];
        let usage = p.usage(&rows);
        let got: Vec<(ModelId, u64)> = usage.iter().map(|(k, t)| (k.model, t.input)).collect();
        assert_eq!(got, [(ModelId::Gpt5, 96), (ModelId::Unknown, 128)]);
        assert!(usage
            .keys()
            .all(|k| k.account_ref == account_ref(DEVICE, "cx")));
        // Floor: earlier hours are not exported.
        let p = Policy::new(&config, &roots, DEVICE, h + 3600);
        assert!(p.usage(&rows).is_empty());
    }

    #[test]
    fn quota_translates_into_closed_sets_only() {
        let mut config = Config::default();
        let acc = account(ProviderKind::Claude, "c1");
        config.accounts = vec![acc.clone()];
        let p = Policy::new(&config, &[], DEVICE, 0);
        let mut r = Reading::fresh(
            &acc,
            100,
            vec![
                LimitWindow::new("session", "Current session", CoreKind::Session).used(f64::NAN),
                LimitWindow::new("seven_day", "x", CoreKind::Weekly).used(140.0),
                LimitWindow::new("sk-secret123", "custom", CoreKind::Other).used(1.0),
                LimitWindow::new("spend", "Spend", CoreKind::Monthly).used(1.0),
            ],
        );
        r.plan = Some("sk-secret123".into());
        let d = p.quota(&r).expect("draft");
        assert_eq!(d.plan, PlanCode::Unknown);
        assert_eq!(
            d.windows.iter().map(|w| w.id).collect::<Vec<_>>(),
            [WindowId::Session, WindowId::SevenDay]
        );
        assert_eq!(d.windows[0].used_percent, None);
        assert_eq!(d.windows[0].role, Some(WindowRole::Headline));
        assert_eq!(d.windows[1].used_percent, Some(100.0));
        assert_eq!(d.windows[1].role, Some(WindowRole::Weekly));
        r.plan = Some("Max 20x".into());
        assert_eq!(p.quota(&r).unwrap().plan, PlanCode::Max20x);
        r.status = ReadingStatus::Stale;
        assert!(p.quota(&r).is_none());
        r.status = ReadingStatus::Fresh;
        config.settings.teams_excluded_accounts = vec!["c1".into()];
        assert!(Policy::new(&config, &[], DEVICE, 0).quota(&r).is_none());
    }
}
