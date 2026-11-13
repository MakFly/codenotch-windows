//! Pure logic of the settings window: config edits (reducers), validation, the snapshot
//! re-sync rule and the content models of the pages. No gpui here: everything is unit tested.

use crate::i18n::{t, tf};
use codenotch_core::config::{
    Account, AntigravityLimit, AntigravityModel, ColorTransition, Config, Edge, NotchSize,
    RemovedAccount, Settings, ShowMode, TaskbarMode, ThemeMode, UpdateMode, WeeklyRing,
};
use codenotch_core::format;
use codenotch_core::model::{
    AccountView, AnalyticsSummary, ProviderKind, ReadingStatus, TeamsState, TeamsStatus,
    UpdateState,
};
use codenotch_core::Lang;
use serde_json::Value;
use std::collections::VecDeque;

/// Default warning / critical thresholds (fresh-install values of `Settings`).
pub const WARN_DEFAULT: u8 = 80;
pub const CRIT_DEFAULT: u8 = 95;

/// Preset choices for the segmented controls (the current value is added when it is not one).
pub const POLL_CHOICES: &[u64] = &[30, 60, 120, 300];
pub const HISTORY_CHOICES: &[u64] = &[24, 72, 168, 720];
pub const FORECAST_CHOICES: &[u64] = &[15, 30, 60, 120];
pub const ANALYTICS_DAY_CHOICES: &[u64] = &[7, 30, 90];

// ---------------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------------

/// One user change to the config. Applied by [`apply`]; kept while debounced so it can be
/// re-applied on top of a newer snapshot.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Language(Lang),
    Theme(ThemeMode),
    Edge(Edge),
    /// Monitor device name; empty = primary.
    Monitor(String),
    NotchSize(NotchSize),
    ShowMode(ShowMode),
    FoldForFullScreen(bool),
    AdaptivePill(bool),
    GameMode(bool),
    WeeklyRing(WeeklyRing),
    WeeklyRingDashed(bool),
    ShowRemaining(bool),
    ShowUsagePace(bool),
    ShowForecast(bool),
    ColorTransition(ColorTransition),
    TaskbarMode(TaskbarMode),
    WarnPercent(u8),
    CriticalPercent(u8),
    ResetLimits,
    Notifications(bool),
    CriticalInFullscreen(bool),
    ForecastAlerts(bool),
    ForecastMinutes(u64),
    ResetAlerts(bool),
    ResetCards(bool),
    ResetSound(bool),
    TelegramEnabled(bool),
    TelegramChatId(String),
    DiscordEnabled(bool),
    LaunchAtStartup(bool),
    TrayShowsUsage(bool),
    Hotkey(Option<String>),
    PollSeconds(u64),
    HistoryHours(u64),
    UpdateMode(UpdateMode),
    /// `updateManifestUrl` (already validated; empty = the default address).
    UpdateManifestUrl(String),
    AnalyticsDays(u64),
    UsdToEur(f64),
    PlanPrice {
        key: String,
        eur: Option<f64>,
    },
    AccountEnabled {
        id: String,
        on: bool,
    },
    /// "Show in notch" (`hiddenAccounts`).
    AccountVisible {
        id: String,
        on: bool,
    },
    RenameAccount {
        id: String,
        name: String,
    },
    /// Remove an account from CodeNotch (never its provider login or files). A discovered one
    /// gets a `removedAccounts` marker so discovery does not add it back.
    RemoveAccount(String),
    /// Undo of a removal: the account back at its place, hidden flag restored, marker dropped.
    RestoreAccount(Box<Removal>),
    /// "Add back" a removed discovered account: drop its marker (discovery finds it again).
    ForgetRemoved(usize),
    AddAccount(Box<Account>),
    /// New order of the Accounts page (every account id, accounts of one provider together):
    /// sets the provider order and the order of the accounts within each provider (both drive
    /// the notch cells). See [`drop_order`] / [`step_order`].
    ReorderAccounts(Vec<String>),
    AntigravityLimit(AntigravityLimit),
    AntigravityModel(AntigravityModel),
    MarkOnboarded,
    /// Settings window sidebar collapsed to its icon rail (`settingsSidebarCollapsed`).
    SidebarCollapsed(bool),
    /// `teamsSyncEnabled`.
    TeamsSyncEnabled(bool),
    /// `teamsEndpoint` (already validated; empty = the official server).
    TeamsEndpoint(String),
    /// Share an account with the team (`teamsExcludedAccounts` lists the ones not shared).
    TeamsShared {
        id: String,
        on: bool,
    },
}

/// Why an edit was not applied. `message()` is an English i18n key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    LastNotchAccount,
    UnknownAccount,
    EmptyName,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::LastNotchAccount => "At least one provider stays on the notch",
            Refusal::UnknownAccount => "This account no longer exists",
            Refusal::EmptyName => "Enter a name",
        }
    }
}

fn set_warn(s: &mut Settings, value: u8) {
    let w = value.clamp(1, 99);
    s.alert_percent = w;
    if s.critical_percent <= w {
        s.critical_percent = w + 1;
    }
}

fn set_critical(s: &mut Settings, value: u8) {
    let c = value.clamp(2, 100);
    s.critical_percent = c;
    if s.alert_percent >= c {
        s.alert_percent = c - 1;
    }
}

/// True when the edit would leave the notch with no account while it had at least one.
fn empties_notch(before: &Config, after: &Config) -> bool {
    !before.notch_accounts().is_empty() && after.notch_accounts().is_empty()
}

/// Apply one edit to `config`. On refusal `config` is left untouched.
pub fn apply(config: &mut Config, edit: &Edit) -> Result<(), Refusal> {
    let mut next = config.clone();
    {
        let s = &mut next.settings;
        match edit {
            Edit::Language(lang) => s.language = lang.code().to_string(),
            Edit::Theme(v) => s.theme = *v,
            Edit::Edge(v) => s.widget_position = *v,
            Edit::Monitor(name) => s.widget_placement.monitor = name.clone(),
            Edit::NotchSize(v) => s.notch_size = *v,
            Edit::ShowMode(v) => {
                s.show_mode = Some(*v);
                // Keep the 0.3 master switch meaningful for older builds reading the file.
                s.widget = *v != ShowMode::Hidden;
            }
            Edit::FoldForFullScreen(v) => s.fold_for_full_screen = *v,
            Edit::AdaptivePill(v) => s.adaptive_pill = *v,
            Edit::GameMode(v) => s.game_mode = *v,
            Edit::WeeklyRing(v) => s.weekly_ring = *v,
            Edit::WeeklyRingDashed(v) => s.weekly_ring_dashed = *v,
            Edit::ShowRemaining(v) => s.show_remaining = *v,
            Edit::ShowUsagePace(v) => s.show_usage_pace = *v,
            Edit::ShowForecast(v) => s.show_forecast = *v,
            Edit::ColorTransition(v) => s.color_transition = *v,
            Edit::TaskbarMode(v) => s.taskbar_mode = *v,
            Edit::WarnPercent(v) => set_warn(s, *v),
            Edit::CriticalPercent(v) => set_critical(s, *v),
            Edit::ResetLimits => {
                s.alert_percent = WARN_DEFAULT;
                s.critical_percent = CRIT_DEFAULT;
            }
            Edit::Notifications(v) => s.notifications = *v,
            Edit::CriticalInFullscreen(v) => s.critical_in_fullscreen = *v,
            Edit::ForecastAlerts(v) => s.forecast_alerts = *v,
            Edit::ForecastMinutes(v) => s.forecast_minutes = (*v).clamp(5, 24 * 60),
            Edit::ResetAlerts(v) => s.reset_alerts = *v,
            Edit::ResetCards(v) => s.reset_cards = *v,
            Edit::ResetSound(v) => s.reset_sound = *v,
            Edit::TelegramEnabled(v) => s.telegram_enabled = *v,
            Edit::TelegramChatId(v) => s.telegram_chat_id = v.trim().into(),
            Edit::DiscordEnabled(v) => s.discord_enabled = *v,
            Edit::LaunchAtStartup(v) => s.launch_at_startup = *v,
            Edit::TrayShowsUsage(v) => s.tray_shows_usage = *v,
            Edit::Hotkey(v) => s.hotkey = v.clone(),
            Edit::PollSeconds(v) => s.poll_seconds = (*v).clamp(30, 300),
            Edit::HistoryHours(v) => s.history_hours = (*v).clamp(1, 24 * 365),
            Edit::UpdateMode(v) => s.set_update_mode(*v),
            Edit::UpdateManifestUrl(v) => s.update_manifest_url = v.trim().to_string(),
            Edit::AnalyticsDays(v) => s.analytics_days = (*v).clamp(1, 365),
            Edit::UsdToEur(v) => {
                if v.is_finite() && *v > 0.0 {
                    s.usd_to_eur = *v;
                }
            }
            Edit::PlanPrice { key, eur } => {
                // One spelling per key: drop case variants (hand-edited files) first.
                s.plan_prices_eur
                    .retain(|k, _| k == key || !k.trim().eq_ignore_ascii_case(key.trim()));
                match eur {
                    Some(v) if v.is_finite() && *v >= 0.0 => {
                        s.plan_prices_eur.insert(key.clone(), Value::from(*v));
                    }
                    _ => {
                        s.plan_prices_eur.remove(key);
                    }
                }
            }
            Edit::AntigravityLimit(v) => s.antigravity_limit = *v,
            Edit::AntigravityModel(v) => s.antigravity_model = *v,
            Edit::MarkOnboarded => s.onboarding_done = Some(true),
            Edit::SidebarCollapsed(v) => s.settings_sidebar_collapsed = *v,
            Edit::TeamsSyncEnabled(v) => s.teams_sync_enabled = *v,
            Edit::TeamsEndpoint(v) => s.teams_endpoint = v.trim().to_string(),
            Edit::TeamsShared { id, on } => {
                s.teams_excluded_accounts.retain(|x| x != id);
                if !*on {
                    s.teams_excluded_accounts.push(id.clone());
                }
            }
            Edit::AccountVisible { id, on } => {
                if !next.accounts.iter().any(|a| &a.id == id) {
                    return Err(Refusal::UnknownAccount);
                }
                if *on {
                    s.hidden_accounts.retain(|h| h != id);
                } else if !s.hidden_accounts.contains(id) {
                    s.hidden_accounts.push(id.clone());
                }
            }
            Edit::ForgetRemoved(index) => {
                if *index < s.removed_accounts.len() {
                    s.removed_accounts.remove(*index);
                }
            }
            Edit::AccountEnabled { .. }
            | Edit::RenameAccount { .. }
            | Edit::RemoveAccount(_)
            | Edit::RestoreAccount(_)
            | Edit::AddAccount(_)
            | Edit::ReorderAccounts(_) => {}
        }
    }
    match edit {
        Edit::AccountEnabled { id, on } => {
            let account = next.account_mut(id).ok_or(Refusal::UnknownAccount)?;
            account.enabled = *on;
        }
        Edit::RenameAccount { id, name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(Refusal::EmptyName);
            }
            let account = next.account_mut(id).ok_or(Refusal::UnknownAccount)?;
            account.name = name.to_string();
        }
        Edit::RemoveAccount(id) => {
            let account = next.account(id).ok_or(Refusal::UnknownAccount)?.clone();
            next.accounts.retain(|a| &a.id != id);
            next.settings.hidden_accounts.retain(|h| h != id);
            if let Some(marker) = RemovedAccount::of(&account) {
                let s = &mut next.settings;
                s.removed_accounts.retain(|m| !m.same_profile(&account));
                s.removed_accounts.push(marker);
            }
        }
        Edit::RestoreAccount(removal) => {
            let r = &**removal;
            if next.account(&r.account.id).is_none() {
                let at = r.index.min(next.accounts.len());
                next.accounts.insert(at, r.account.clone());
            }
            let s = &mut next.settings;
            s.removed_accounts.retain(|m| !m.same_profile(&r.account));
            if r.hidden && !s.hidden_accounts.contains(&r.account.id) {
                s.hidden_accounts.push(r.account.id.clone());
            }
        }
        Edit::AddAccount(account) => {
            if account.name.trim().is_empty() {
                return Err(Refusal::EmptyName);
            }
            if next.account(&account.id).is_none() {
                next.accounts.push((**account).clone());
            }
        }
        Edit::ReorderAccounts(ids) => reorder_accounts(&mut next, ids),
        _ => {}
    }
    if matches!(
        edit,
        Edit::AccountEnabled { .. } | Edit::AccountVisible { .. } | Edit::RemoveAccount(_)
    ) && empties_notch(config, &next)
    {
        return Err(Refusal::LastNotchAccount);
    }
    *config = next;
    Ok(())
}

/// Apply an Accounts-page order: providers in the order they first appear in `ids` (the
/// others keep their place after them), accounts sorted by their position in `ids` (stable;
/// unknown ones last).
fn reorder_accounts(config: &mut Config, ids: &[String]) {
    let pos = |id: &str| ids.iter().position(|i| i == id).unwrap_or(usize::MAX);
    let mut order: Vec<ProviderKind> = Vec::new();
    for id in ids {
        if let Some(a) = config.account(id) {
            if !order.contains(&a.provider) {
                order.push(a.provider.clone());
            }
        }
    }
    for kind in config.settings.effective_provider_order() {
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    config.settings.provider_order = order;
    config.accounts.sort_by_key(|a| pos(&a.id));
}

/// Contiguous provider runs of a row list: `(start, end)` (end exclusive) per run.
fn provider_runs(rows: &[ProviderKind]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, k) in rows.iter().enumerate() {
        match runs.last_mut() {
            Some((start, end)) if rows[*start] == *k => *end = i + 1,
            _ => runs.push((i, i + 1)),
        }
    }
    runs
}

/// Accounts-page order after dropping row `from` at insertion `slot` (0..=n, a gap between
/// rows of the list as it is). Accounts of a provider stay together: within its own provider
/// the row moves alone; anywhere else its whole provider moves, to the provider boundary
/// nearest to `slot`. `None` when nothing changes.
pub fn drop_order(
    rows: &[(String, ProviderKind)],
    from: usize,
    slot: usize,
) -> Option<Vec<String>> {
    let n = rows.len();
    if from >= n || slot > n {
        return None;
    }
    let kinds: Vec<ProviderKind> = rows.iter().map(|(_, k)| k.clone()).collect();
    let runs = provider_runs(&kinds);
    let (gs, ge) = *runs.iter().find(|(s, e)| (*s..*e).contains(&from))?;
    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let out = if (gs..=ge).contains(&slot) {
        // Within its provider: move the row alone.
        let mut v = ids.clone();
        let id = v.remove(from);
        let at = if slot > from { slot - 1 } else { slot };
        v.insert(at, id);
        v
    } else {
        // Elsewhere: move the whole provider to the nearest boundary.
        let bounds: Vec<usize> = runs.iter().map(|(s, _)| *s).chain([n]).collect();
        let at = bounds
            .iter()
            .copied()
            .min_by_key(|b| (b.abs_diff(slot), *b))
            .unwrap_or(slot);
        let group: Vec<String> = ids[gs..ge].to_vec();
        let mut rest: Vec<String> = ids[..gs].iter().chain(&ids[ge..]).cloned().collect();
        let at = if at > gs { at - (ge - gs) } else { at };
        let at = at.min(rest.len());
        for (k, id) in group.into_iter().enumerate() {
            rest.insert(at + k, id);
        }
        rest
    };
    (out != ids).then_some(out)
}

/// Keyboard move ("Move up" / "Move down" in the "…" menu) of row `from`: past its neighbour
/// within its provider, else its whole provider past the neighbouring provider. `None` at the
/// ends.
pub fn step_order(rows: &[(String, ProviderKind)], from: usize, up: bool) -> Option<Vec<String>> {
    let n = rows.len();
    if from >= n {
        return None;
    }
    let kinds: Vec<ProviderKind> = rows.iter().map(|(_, k)| k.clone()).collect();
    let runs = provider_runs(&kinds);
    let g = runs.iter().position(|(s, e)| (*s..*e).contains(&from))?;
    let (gs, ge) = runs[g];
    if up && from > gs {
        return drop_order(rows, from, from - 1);
    }
    if !up && from + 1 < ge {
        return drop_order(rows, from, from + 2);
    }
    let ids: Vec<String> = rows.iter().map(|(id, _)| id.clone()).collect();
    let (a, b) = if up {
        (runs.get(g.checked_sub(1)?)?.0, ge)
    } else {
        let (_, ne) = *runs.get(g + 1)?;
        (gs, ne)
    };
    // Swap the two provider runs inside [a, b).
    let (first, second) = if up {
        (runs[g - 1], runs[g])
    } else {
        (runs[g], runs[g + 1])
    };
    let mut out: Vec<String> = ids[..a].to_vec();
    out.extend_from_slice(&ids[second.0..second.1]);
    out.extend_from_slice(&ids[first.0..first.1]);
    out.extend_from_slice(&ids[b..]);
    Some(out)
}

/// What a removal took away, to undo it exactly ([`Edit::RestoreAccount`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Removal {
    pub account: Account,
    /// Position in `accounts`.
    pub index: usize,
    /// It was hidden from the notch.
    pub hidden: bool,
}

/// The undo record of removing account `id` from `config` (before the removal).
pub fn removal_of(config: &Config, id: &str) -> Option<Removal> {
    let index = config.accounts.iter().position(|a| a.id == id)?;
    Some(Removal {
        account: config.accounts[index].clone(),
        index,
        hidden: config.settings.hidden_accounts.iter().any(|h| h == id),
    })
}

/// Why account `id` cannot be removed now, if it cannot: the last account on the notch stays
/// (every kind of account can be removed otherwise — it only leaves CodeNotch).
pub fn remove_blocked(config: &Config, id: &str) -> Option<Refusal> {
    let mut probe = config.clone();
    apply(&mut probe, &Edit::RemoveAccount(id.to_string())).err()
}

/// A new custom-endpoint account (bearer token, if any, goes through `SetSecret`).
pub fn new_custom_account(name: &str, url: &str, has_token: bool) -> Account {
    let mut account = Account::new(ProviderKind::Custom, name.trim());
    account.endpoint = url.trim().to_string();
    account.has_secret = has_token;
    account
}

// ---------------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------------

/// A field validation error. `message()` is an English i18n key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldError {
    NotANumber,
    OutOfRange,
    Url,
    HotkeyFormat,
    ChatId,
    TelegramToken,
    DiscordWebhook,
    EmptyName,
    HttpsOnly,
}

impl FieldError {
    pub fn message(self) -> &'static str {
        match self {
            FieldError::HttpsOnly => "Use an https:// address",
            FieldError::NotANumber => "Enter a number",
            FieldError::OutOfRange => "Value out of range",
            FieldError::Url => "Use an https:// address (http only for localhost)",
            FieldError::HotkeyFormat => "Use a format like Ctrl+Alt+N",
            FieldError::ChatId => "A chat id is a number (or @channel)",
            FieldError::TelegramToken => "A bot token looks like 123456:ABC…",
            FieldError::DiscordWebhook => "Paste the https:// webhook URL",
            FieldError::EmptyName => "Enter a name",
        }
    }
}

/// Parse a user number: "0,92", "0.92", " 1 234,5 € ", "21.6 $". `None` when not a finite number.
pub fn parse_number(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .trim_end_matches(['€', '$', '%'])
        .chars()
        .filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\''))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    let normalised = if cleaned.contains(',') && cleaned.contains('.') {
        // "1,234.5" (en) or "1.234,5" (fr): the last separator is the decimal one.
        let last_comma = cleaned.rfind(',').unwrap_or(0);
        let last_dot = cleaned.rfind('.').unwrap_or(0);
        if last_comma > last_dot {
            cleaned.replace('.', "").replace(',', ".")
        } else {
            cleaned.replace(',', "")
        }
    } else {
        cleaned.replace(',', ".")
    };
    normalised.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Number for an editable field: "0.92" / "0,92", "108" (no trailing zeros, ≤ 4 decimals).
pub fn format_number(lang: Lang, v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if lang == Lang::Fr {
        s = s.replace('.', ",");
    }
    s
}

/// Monthly plan price in EUR. Empty = no price.
pub fn parse_price(text: &str) -> Result<Option<f64>, FieldError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let v = parse_number(text).ok_or(FieldError::NotANumber)?;
    if !(0.0..=100_000.0).contains(&v) {
        return Err(FieldError::OutOfRange);
    }
    Ok(Some(v))
}

/// USD → EUR rate (0 < rate ≤ 10).
pub fn parse_rate(text: &str) -> Result<f64, FieldError> {
    let v = parse_number(text).ok_or(FieldError::NotANumber)?;
    if v <= 0.0 || v > 10.0 {
        return Err(FieldError::OutOfRange);
    }
    Ok(v)
}

/// Custom endpoint URL: https, or http on localhost only (`core::http::allowed_endpoint`).
pub fn validate_endpoint(text: &str) -> Result<String, FieldError> {
    let url = text.trim();
    if url.contains(char::is_whitespace) || !codenotch_core::http::allowed_endpoint(url) {
        return Err(FieldError::Url);
    }
    Ok(url.to_string())
}

/// Account / endpoint name.
pub fn validate_name(text: &str) -> Result<String, FieldError> {
    let name = text.trim();
    if name.is_empty() {
        return Err(FieldError::EmptyName);
    }
    Ok(name.chars().take(60).collect())
}

/// Telegram chat id: an integer (groups are negative) or an @channel name. Empty is allowed.
pub fn validate_chat_id(text: &str) -> Result<String, FieldError> {
    let id = text.trim();
    if id.is_empty() {
        return Ok(String::new());
    }
    let numeric = {
        let digits = id.strip_prefix('-').unwrap_or(id);
        !digits.is_empty() && digits.len() <= 20 && digits.chars().all(|c| c.is_ascii_digit())
    };
    let channel = id.len() > 1
        && id.starts_with('@')
        && id[1..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if numeric || channel {
        Ok(id.to_string())
    } else {
        Err(FieldError::ChatId)
    }
}

/// Telegram bot token: `<digits>:<at least 20 url-safe chars>`.
pub fn validate_telegram_token(text: &str) -> Result<(), FieldError> {
    let token = text.trim();
    let Some((bot, secret)) = token.split_once(':') else {
        return Err(FieldError::TelegramToken);
    };
    let ok = !bot.is_empty()
        && bot.chars().all(|c| c.is_ascii_digit())
        && secret.len() >= 20
        && secret
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(FieldError::TelegramToken)
    }
}

/// Discord webhook: an https URL without spaces.
pub fn validate_discord_webhook(text: &str) -> Result<(), FieldError> {
    let url = text.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://")
        && url.len() > "https://".len() + 3
        && !url.contains(char::is_whitespace)
    {
        Ok(())
    } else {
        Err(FieldError::DiscordWebhook)
    }
}

/// Normalise a global hotkey: "ctrl + alt + n" → "Ctrl+Alt+N". Empty → `Ok(None)` (no hotkey).
/// Needs Ctrl, Alt or Win (Shift alone would steal typing) and exactly one key: A–Z, 0–9,
/// F1–F24, Space, Insert, Delete, Home, End, PageUp, PageDown or an arrow.
pub fn normalize_hotkey(text: &str) -> Result<Option<String>, FieldError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (mut ctrl, mut alt, mut shift, mut win) = (false, false, false, false);
    let mut key: Option<String> = None;
    for part in text.split('+').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        let flag = match lower.as_str() {
            "ctrl" | "control" | "ctl" => &mut ctrl,
            "alt" => &mut alt,
            "shift" => &mut shift,
            "win" | "windows" | "super" | "meta" => &mut win,
            _ => {
                if key.is_some() {
                    return Err(FieldError::HotkeyFormat);
                }
                key = Some(hotkey_key(&lower).ok_or(FieldError::HotkeyFormat)?);
                continue;
            }
        };
        if *flag {
            return Err(FieldError::HotkeyFormat);
        }
        *flag = true;
    }
    let key = key.ok_or(FieldError::HotkeyFormat)?;
    if !(ctrl || alt || win) {
        return Err(FieldError::HotkeyFormat);
    }
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if win {
        parts.push("Win");
    }
    parts.push(&key);
    Ok(Some(parts.join("+")))
}

fn hotkey_key(lower: &str) -> Option<String> {
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase().to_string());
        }
        return None;
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) && !lower.starts_with("f0") {
            return Some(format!("F{n}"));
        }
        return None;
    }
    let named = match lower {
        "space" => "Space",
        "insert" | "ins" => "Insert",
        "delete" | "del" => "Delete",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        _ => return None,
    };
    Some(named.to_string())
}

// ---------------------------------------------------------------------------------------------
// Re-sync rule
// ---------------------------------------------------------------------------------------------

/// Keeps the local edited config consistent with the snapshots coming back from the engine.
///
/// * Every immediate save is recorded (`record_sent`). When the engine echoes one of them, the
///   echo is accepted only if nothing newer is still in flight (otherwise a stale echo would
///   undo a click the user just made).
/// * Debounced edits (`record_pending`) are re-applied on top of any adopted snapshot until sent.
/// * A config we did not send (engine-side change: new Claude account, hooks …) is adopted.
#[derive(Debug, Clone)]
pub struct Sync {
    sent: VecDeque<Config>,
    pending: Vec<Edit>,
    seen: Config,
}

impl Sync {
    const MAX_IN_FLIGHT: usize = 16;

    pub fn new(seen: Config) -> Self {
        Self {
            sent: VecDeque::new(),
            pending: Vec::new(),
            seen,
        }
    }

    pub fn record_pending(&mut self, edit: Edit) {
        self.pending.push(edit);
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// True when a pending (unsent) edit touches the thresholds (do not move the sliders).
    pub fn pending_thresholds(&self) -> bool {
        self.pending.iter().any(|e| {
            matches!(
                e,
                Edit::WarnPercent(_) | Edit::CriticalPercent(_) | Edit::ResetLimits
            )
        })
    }

    pub fn record_sent(&mut self, config: Config) {
        self.pending.clear();
        if self.sent.len() >= Self::MAX_IN_FLIGHT {
            self.sent.pop_front();
        }
        self.sent.push_back(config);
    }

    /// A snapshot arrived. Returns true when `edited` was replaced.
    pub fn on_snapshot(&mut self, edited: &mut Config, incoming: &Config) -> bool {
        if *incoming == self.seen {
            return false;
        }
        self.seen = incoming.clone();
        if let Some(pos) = self.sent.iter().position(|c| c == incoming) {
            self.sent.drain(..=pos);
            if !self.sent.is_empty() {
                return false;
            }
        } else {
            self.sent.clear();
        }
        let mut next = incoming.clone();
        for edit in &self.pending {
            let _ = apply(&mut next, edit);
        }
        if next == *edited {
            return false;
        }
        *edited = next;
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Choices
// ---------------------------------------------------------------------------------------------

/// Presets plus the current value when it is not one of them (sorted).
pub fn choices_with_current(presets: &[u64], current: u64) -> Vec<u64> {
    let mut v = presets.to_vec();
    if !v.contains(&current) {
        v.push(current);
        v.sort_unstable();
    }
    v
}

/// "30 s", "1 min", "2 min 30 s".
pub fn seconds_label(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

/// "15 min", "1 h", "1 h 30".
pub fn minutes_label(mins: u64) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

/// "24 h", "3 d" / "3 j".
pub fn hours_label(lang: Lang, hours: u64) -> String {
    if hours >= 48 && hours.is_multiple_of(24) {
        days_label(lang, hours / 24)
    } else {
        format!("{hours} h")
    }
}

/// "7 d" / "7 j".
pub fn days_label(lang: Lang, days: u64) -> String {
    format!("{days} {}", if lang == Lang::Fr { "j" } else { "d" })
}

/// Monitor choice list: labels "Screen 1 · Main", "Screen 2"…, the selected index and the value
/// to store for each entry (primary = empty string).
pub fn monitor_choices(
    lang: Lang,
    monitors: &[(String, bool)],
    current: &str,
) -> (Vec<String>, usize, Vec<String>) {
    let mut labels = Vec::new();
    let mut values = Vec::new();
    for (i, (name, primary)) in monitors.iter().enumerate() {
        let mut label = tf(lang, "Screen {n}", &[("n", &(i + 1).to_string())]);
        if *primary {
            label = format!("{label} · {}", t(lang, "Main"));
        }
        labels.push(label);
        values.push(if *primary {
            String::new()
        } else {
            name.clone()
        });
    }
    let selected = monitors
        .iter()
        .position(|(name, _)| !current.is_empty() && name == current)
        .or_else(|| monitors.iter().position(|(_, p)| *p))
        .unwrap_or(0);
    (labels, selected, values)
}

// ---------------------------------------------------------------------------------------------
// Accounts page model
// ---------------------------------------------------------------------------------------------

/// Colour family of a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    Warn,
    Error,
}

/// One row of the Accounts page.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountRow {
    pub id: String,
    pub provider: ProviderKind,
    pub name: String,
    pub letters: &'static str,
    /// "someone@example.com · Max".
    pub subtitle: Option<String>,
    pub status: String,
    pub tone: Tone,
    pub enabled: bool,
    pub visible: bool,
    pub removable: bool,
    pub signed_out: bool,
    pub refreshing: bool,
    pub can_move_up: bool,
    pub can_move_down: bool,
}

/// Status line for one account (reference wording).
pub fn status_line(
    lang: Lang,
    view: Option<&AccountView>,
    enabled: bool,
    show_remaining: bool,
    now: i64,
) -> (String, Tone) {
    if !enabled {
        return (t(lang, "Turned off").to_string(), Tone::Muted);
    }
    let Some(reading) = view.and_then(|v| v.reading.as_ref()) else {
        return (
            t(lang, "Waiting for the first reading…").to_string(),
            Tone::Muted,
        );
    };
    let tone_of_status = match &reading.status {
        ReadingStatus::SignedOut => Some(("not signed in", Tone::Warn)),
        ReadingStatus::NotInstalled => Some(("not installed", Tone::Muted)),
        ReadingStatus::Empty => Some(("nothing to report", Tone::Muted)),
        ReadingStatus::Error => Some(("not reachable", Tone::Error)),
        ReadingStatus::RateLimited { .. } => Some(("rate limited, trying again soon", Tone::Warn)),
        ReadingStatus::Fresh | ReadingStatus::Stale => None,
    };
    if let Some((key, tone)) = tone_of_status {
        let mut text = t(lang, key).to_string();
        if reading.status == ReadingStatus::SignedOut {
            text = format!("{text} · {}", t(lang, reading.provider.sign_in_hint()));
        }
        return (text, tone);
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(w) = reading.headline() {
        let used = format::used_line(lang, w.used_percent, show_remaining);
        parts.push(format!("{} · {used}", t(lang, &w.label_key)));
    }
    if reading.is_stale(now) {
        parts.push(t(lang, "last reading is old").to_string());
        return (parts.join(" · "), Tone::Warn);
    }
    (parts.join(" · "), Tone::Normal)
}

/// [`status_line`] plus the forecast of the most pressing window when `settings.show_forecast`
/// and the line is a normal one: " · runs out ~Thu 3:20 PM" (the earliest window that runs out
/// before its reset, labelled when it is not the headline) or " · ≈ 82% at reset" (headline).
/// Local time; 24 h clock in French.
pub fn status_line_full(
    lang: Lang,
    view: Option<&AccountView>,
    enabled: bool,
    settings: &Settings,
    now: i64,
) -> (String, Tone) {
    status_line_tz(
        lang,
        view,
        enabled,
        settings,
        now,
        &chrono::Local,
        format::default_clock24(lang),
    )
}

/// [`status_line_full`] with an explicit time zone and clock style (tests).
pub fn status_line_tz<Tz: chrono::TimeZone>(
    lang: Lang,
    view: Option<&AccountView>,
    enabled: bool,
    settings: &Settings,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> (String, Tone) {
    let (mut text, tone) = status_line(lang, view, enabled, settings.show_remaining, now);
    if tone == Tone::Normal && settings.show_forecast {
        if let Some(suffix) = view.and_then(|v| forecast_suffix(lang, v, now, tz, clock24)) {
            text.push_str(" · ");
            text.push_str(&suffix);
        }
    }
    (text, tone)
}

/// Forecast part of the status line (see [`status_line_full`]).
pub fn forecast_suffix<Tz: chrono::TimeZone>(
    lang: Lang,
    view: &AccountView,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> Option<String> {
    let reading = view.reading.as_ref().filter(|r| r.shows_figures())?;
    let headline = reading.headline();
    let first_out = reading
        .windows
        .iter()
        .filter(|w| {
            w.used_percent.is_some_and(|u| u < 100.0) && w.forecast.is_some_and(|f| f.runs_out())
        })
        .min_by_key(|w| w.forecast.and_then(|f| f.exhaust_at));
    if let Some(w) = first_out {
        let short = format::forecast_short(lang, w, now, tz, clock24)?;
        let is_headline = headline.is_some_and(|h| h.id == w.id);
        return Some(if is_headline {
            short
        } else {
            tf(
                lang,
                "{label}: {forecast}",
                &[
                    ("label", &codenotch_core::i18n::tr_label(lang, &w.label_key)),
                    ("forecast", &short),
                ],
            )
        });
    }
    format::forecast_short(lang, headline?, now, tz, clock24)
}

/// All accounts (enabled or not) in provider order, stable within a provider.
pub fn account_rows(
    lang: Lang,
    config: &Config,
    views: &[AccountView],
    now: i64,
) -> Vec<AccountRow> {
    let order = config.settings.effective_provider_order();
    let rank = |k: &ProviderKind| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
    let mut accounts: Vec<&Account> = config.accounts.iter().collect();
    accounts.sort_by_key(|a| rank(&a.provider));
    let n = accounts.len();
    accounts
        .into_iter()
        .enumerate()
        .map(|(index, a)| {
            let view = views.iter().find(|v| v.account.id == a.id);
            let reading = view.and_then(|v| v.reading.as_ref());
            let subtitle = {
                let parts: Vec<String> = reading
                    .map(|r| {
                        [r.identity.clone(), r.plan.clone()]
                            .into_iter()
                            .flatten()
                            .filter(|s| !s.trim().is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                (!parts.is_empty()).then(|| parts.join(" · "))
            };
            let (status, tone) = status_line_full(lang, view, a.enabled, &config.settings, now);
            AccountRow {
                id: a.id.clone(),
                provider: a.provider.clone(),
                name: a.name.clone(),
                letters: a.provider.glyph_letters(),
                subtitle,
                status,
                tone,
                enabled: a.enabled,
                visible: !config.settings.hidden_accounts.contains(&a.id),
                removable: remove_blocked(config, &a.id).is_none(),
                signed_out: reading.is_some_and(|r| r.status == ReadingStatus::SignedOut),
                refreshing: view.is_some_and(|v| v.refreshing),
                // Every row moves (alone or with its provider) unless it is at an end.
                can_move_up: index > 0,
                can_move_down: index + 1 < n,
            }
        })
        .collect()
}

/// Accounts worth showing in onboarding: everything except "not installed".
pub fn discovered_accounts(views: &[AccountView]) -> Vec<&AccountView> {
    views
        .iter()
        .filter(|v| {
            !matches!(
                v.reading.as_ref().map(|r| &r.status),
                Some(ReadingStatus::NotInstalled)
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Update line
// ---------------------------------------------------------------------------------------------

/// What the update button does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAction {
    Check,
    /// Download (if needed) and install the version on offer.
    Install,
    Busy,
}

/// Status text and button for the update row. Never claims "up to date" before a completed
/// check. `checked_at`: last completed check (any outcome), added when the state lacks it.
pub fn update_line(
    lang: Lang,
    state: &UpdateState,
    checked_at: Option<i64>,
    now: i64,
) -> (String, UpdateAction) {
    let checked = |at: i64| {
        tf(
            lang,
            "checked {age}",
            &[("age", &format::age(lang, at, now))],
        )
    };
    let with_check = |text: String| match checked_at {
        Some(at) => format!("{text} · {}", checked(at)),
        None => text,
    };
    match state {
        UpdateState::Idle => (t(lang, "Not checked yet").to_string(), UpdateAction::Check),
        UpdateState::Checking { .. } => (t(lang, "Checking…").to_string(), UpdateAction::Busy),
        UpdateState::UpToDate { checked_at, .. } => (
            format!("{} · {}", t(lang, "Up to date"), checked(*checked_at)),
            UpdateAction::Check,
        ),
        UpdateState::Available {
            version, forced, ..
        } => {
            let key = if *forced {
                "Version {v} is required"
            } else {
                "Version {v} is available"
            };
            (
                with_check(tf(lang, key, &[("v", version)])),
                UpdateAction::Install,
            )
        }
        UpdateState::Downloading {
            version, progress, ..
        } => (
            tf(
                lang,
                "Downloading version {v}… {n}%",
                &[("v", version), ("n", &progress.to_string())],
            ),
            UpdateAction::Busy,
        ),
        UpdateState::Ready { version, .. } => (
            tf(lang, "Version {v} is ready to install", &[("v", version)]),
            UpdateAction::Install,
        ),
        UpdateState::Installing { .. } => (t(lang, "Installing…").to_string(), UpdateAction::Busy),
        UpdateState::Failed { message, .. } => (
            with_check(tf(
                lang,
                "Update failed: {msg}",
                &[("msg", codenotch_core::tr(lang, message))],
            )),
            UpdateAction::Check,
        ),
    }
}

/// Validate the Teams server address: empty (the official server), `https://host[:port]`, or
/// `http://` to localhost only. An origin: no user info, path, query or fragment. The engine
/// checks it again before any request.
pub fn validate_teams_endpoint(text: &str) -> Result<String, FieldError> {
    let url = text.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(String::new());
    }
    let lower = url.to_ascii_lowercase();
    let (host, local_only) = match (
        lower.strip_prefix("https://"),
        lower.strip_prefix("http://"),
    ) {
        (Some(h), _) => (h, false),
        (None, Some(h)) => (h, true),
        _ => return Err(FieldError::Url),
    };
    let name = if host.starts_with('[') {
        host.split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        host.split(':').next().unwrap_or("").to_string()
    };
    let bad = host.is_empty()
        || name.is_empty()
        || host.contains(['/', '?', '#', '@', '\\'])
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
        || (local_only && !matches!(name.as_str(), "localhost" | "127.0.0.1" | "[::1]"));
    if bad {
        return Err(FieldError::Url);
    }
    Ok(url.to_string())
}

/// Accounts that can be shared with a team: enabled Claude and Codex accounts (others never
/// leave the PC). `(account, shared)` in config order.
pub fn teams_accounts(config: &Config) -> Vec<(&Account, bool)> {
    config
        .ordered_accounts()
        .into_iter()
        .filter(|a| {
            a.enabled
                && !a.uses_custom_endpoint()
                && matches!(a.provider, ProviderKind::Claude | ProviderKind::Codex)
        })
        .map(|a| (a, !config.settings.teams_excludes(&a.id)))
        .collect()
}

/// Status line of the Teams page.
pub fn teams_status_line(lang: Lang, status: &TeamsStatus) -> (String, Tone) {
    let team = status.team_name.as_deref().unwrap_or("");
    let error =
        |default: &str| t(lang, status.last_error.as_deref().unwrap_or(default)).to_string();
    match status.state {
        TeamsState::Off => (t(lang, "Turned off").to_string(), Tone::Muted),
        TeamsState::NotPaired => (t(lang, "Not connected to a team").to_string(), Tone::Normal),
        TeamsState::Active => {
            let mut line = tf(lang, "Connected to {team}", &[("team", team)]);
            if status.queued > 0 {
                line.push_str(" · ");
                line.push_str(&tf(
                    lang,
                    "{n} events waiting",
                    &[("n", &status.queued.to_string())],
                ));
            }
            (line, Tone::Normal)
        }
        TeamsState::Paused => (error("Sync paused, retrying"), Tone::Warn),
        TeamsState::Revoked => (error("Device revoked by the team"), Tone::Error),
        TeamsState::ConsentNeeded => (
            error("The team changed what it sees. Review and accept to resume."),
            Tone::Error,
        ),
        TeamsState::Error => (error("Cannot save Teams sync data"), Tone::Error),
    }
}

/// The device is paired (whatever the sync state).
pub fn teams_paired(status: &TeamsStatus) -> bool {
    status.team_name.is_some() && !matches!(status.state, TeamsState::Off | TeamsState::NotPaired)
}

/// Validate the update manifest address: empty (the official one) or `https://…`.
pub fn validate_update_url(text: &str) -> Result<String, FieldError> {
    let url = text.trim();
    if url.is_empty() {
        return Ok(String::new());
    }
    let lower = url.to_ascii_lowercase();
    let host = lower
        .strip_prefix("https://")
        .map(|r| r.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if host.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(FieldError::HttpsOnly);
    }
    Ok(url.to_string())
}

// ---------------------------------------------------------------------------------------------
// Analytics model
// ---------------------------------------------------------------------------------------------

/// A cost for display: the full figure when every token is priced; "≥ figure" when some tokens
/// have no price and something is priced; "—" when nothing is priced. `fmt` formats the number.
pub fn cost_text(cost_usd: f64, unpriced_tokens: u64, fmt: impl Fn(f64) -> String) -> String {
    if unpriced_tokens == 0 {
        fmt(cost_usd)
    } else if cost_usd > 0.0 {
        format!("≥ {}", fmt(cost_usd))
    } else {
        codenotch_core::format::DASH.to_string()
    }
}

/// Key of an account's plan in `planPricesEur`: "Claude Max", or the provider name when the
/// plan is unknown. The one documented format, `codenotch_core::config::plan_price_key`
/// (analytics reads the same keys).
pub fn plan_key(provider: &ProviderKind, plan: Option<&str>) -> String {
    codenotch_core::config::plan_price_key(provider, plan)
}

/// Price stored for a plan key (number or numeric string), EUR per month. Exact key first,
/// then a case-insensitive match (hand-edited files).
pub fn plan_price(settings: &Settings, key: &str) -> Option<f64> {
    let value = settings.plan_prices_eur.get(key).or_else(|| {
        settings
            .plan_prices_eur
            .iter()
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(key.trim()))
            .map(|(_, v)| v)
    })?;
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_number(s),
        _ => None,
    }
    .filter(|v| v.is_finite() && *v >= 0.0)
}

/// Price of one plan line for the comparison: its own key (`"Claude Max"`), else the older /
/// hand-written forms analytics also accepts — the bare plan name, the provider display name,
/// the provider id (`"claude"`).
pub fn plan_price_any(
    settings: &Settings,
    provider: &ProviderKind,
    plan: Option<&str>,
) -> Option<f64> {
    plan_price(settings, &plan_key(provider, plan))
        .or_else(|| plan.and_then(|p| plan_price(settings, p)))
        .or_else(|| plan_price(settings, &plan_key(provider, None)))
        .or_else(|| plan_price(settings, provider.id()))
}

/// USD → EUR rate of the settings (0.92 when the stored one is unusable).
pub fn fx_rate(settings: &Settings) -> f64 {
    if settings.usd_to_eur.is_finite() && settings.usd_to_eur > 0.0 {
        settings.usd_to_eur
    } else {
        0.92
    }
}

/// Built-in list price of a plan in EUR per month (`codenotch_core::config::LIST_PRICES_USD`
/// converted with the `usdToEur` setting), `None` for a plan without one.
pub fn list_price_eur(
    settings: &Settings,
    provider: &ProviderKind,
    plan: Option<&str>,
) -> Option<f64> {
    codenotch_core::config::list_price_usd(provider, plan).map(|usd| usd * fx_rate(settings))
}

/// The monthly price used for one plan in the comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanPrice {
    /// Typed by the user (EUR / month).
    User(f64),
    /// No price typed: the built-in list price (EUR / month).
    List(f64),
    /// The user typed 0: leave this plan out of the comparison.
    Off,
    /// No price typed and no list price known.
    Unknown,
}

impl PlanPrice {
    /// EUR per month counted in the comparison.
    pub fn eur(self) -> Option<f64> {
        match self {
            PlanPrice::User(v) | PlanPrice::List(v) => Some(v),
            PlanPrice::Off | PlanPrice::Unknown => None,
        }
    }
}

/// Price of a plan: the user's (any accepted key form) always wins — a positive value is used,
/// `0` means "do not compare"; with nothing typed, the built-in list price when there is one.
pub fn resolve_plan_price(
    settings: &Settings,
    provider: &ProviderKind,
    plan: Option<&str>,
) -> PlanPrice {
    match plan_price_any(settings, provider, plan) {
        Some(v) if v > 0.0 => PlanPrice::User(v),
        Some(_) => PlanPrice::Off,
        None => {
            list_price_eur(settings, provider, plan).map_or(PlanPrice::Unknown, PlanPrice::List)
        }
    }
}

/// One plan line of the analytics page (unique key per provider).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLine {
    pub key: String,
    pub provider: ProviderKind,
    /// How many enabled accounts use this plan.
    pub accounts: usize,
    /// Plan name from the latest reading (`None` = unknown).
    pub plan: Option<String>,
}

/// Plans of the enabled accounts, in provider order, de-duplicated by key.
pub fn plan_lines(config: &Config, views: &[AccountView]) -> Vec<PlanLine> {
    let mut lines: Vec<PlanLine> = Vec::new();
    for account in config.ordered_accounts() {
        if matches!(
            account.provider,
            ProviderKind::Custom | ProviderKind::Other(_)
        ) {
            continue;
        }
        let plan = views
            .iter()
            .find(|v| v.account.id == account.id)
            .and_then(|v| v.reading.as_ref())
            .and_then(|r| r.plan.as_deref());
        let key = plan_key(&account.provider, plan);
        match lines.iter_mut().find(|l| l.key == key) {
            Some(l) => l.accounts += 1,
            None => lines.push(PlanLine {
                key,
                provider: account.provider.clone(),
                accounts: 1,
                plan: plan
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_string),
            }),
        }
    }
    lines
}

/// One provider line: API-equivalent cost vs plan cost over the period, both in EUR.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub provider: ProviderKind,
    pub tokens: u64,
    pub sessions: u32,
    pub api_usd: f64,
    pub api_eur: f64,
    /// Plan cost pro rata over the period; `None` when no price is set for this provider.
    pub plan_eur: Option<f64>,
    /// Some of `plan_eur` comes from built-in list prices (an estimate).
    pub list_price: bool,
    /// Tokens without a known price: `api_*` (and the ratio) are then lower bounds.
    pub unpriced_tokens: u64,
}

/// Per-provider comparison. `days` = analytics period.
pub fn comparisons(
    summary: &AnalyticsSummary,
    config: &Config,
    views: &[AccountView],
) -> Vec<Comparison> {
    let fx = fx_rate(&config.settings);
    let days = f64::from(summary.days.max(1));
    let plans = plan_lines(config, views);
    summary
        .per_provider
        .iter()
        .map(|p| {
            // Only a provider with usage data (Claude Code / Codex transcripts) has an
            // API-equivalent cost: the plans of the others (Cursor, Grok, Copilot …) are never
            // weighed against it — each line only takes the plans of its own provider.
            let has_usage = p.tokens.total() > 0 || p.cost_usd > 0.0;
            let prices: Vec<PlanPrice> = plans
                .iter()
                .filter(|l| has_usage && l.provider == p.provider)
                .map(|l| {
                    match resolve_plan_price(&config.settings, &l.provider, l.plan.as_deref()) {
                        PlanPrice::User(v) => PlanPrice::User(v * l.accounts as f64),
                        PlanPrice::List(v) => PlanPrice::List(v * l.accounts as f64),
                        other => other,
                    }
                })
                .filter(|price| price.eur().is_some())
                .collect();
            let list_price = prices
                .iter()
                .any(|price| matches!(price, PlanPrice::List(_)));
            let plan_eur = (!prices.is_empty()).then(|| {
                prices.iter().filter_map(|price| price.eur()).sum::<f64>() * days
                    / codenotch_core::DAYS_PER_MONTH
            });
            Comparison {
                provider: p.provider.clone(),
                tokens: p.tokens.total(),
                sessions: p.sessions,
                api_usd: p.cost_usd,
                api_eur: p.cost_usd * fx,
                plan_eur,
                list_price,
                unpriced_tokens: p.unpriced_tokens,
            }
        })
        .collect()
}

/// Analytics page state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsState {
    /// Nothing computed yet and no request made.
    NotComputed,
    /// A refresh was requested and no new summary arrived yet.
    Loading,
    /// A summary with no tokens at all.
    Empty,
    Ready,
}

/// `requested_at`: `generated_at` of the summary shown when the user pressed Refresh
/// (`Some(0)` when there was none).
pub fn analytics_state(
    summary: Option<&AnalyticsSummary>,
    requested_at: Option<i64>,
) -> AnalyticsState {
    let generated = summary.map(|s| s.generated_at).unwrap_or(0);
    if requested_at.is_some_and(|r| r == generated) {
        return AnalyticsState::Loading;
    }
    match summary {
        None => AnalyticsState::NotComputed,
        Some(s) if s.total_tokens.total() == 0 && s.per_day.is_empty() => AnalyticsState::Empty,
        Some(_) => AnalyticsState::Ready,
    }
}

/// How an analytics refresh ended, once the summary changed: `Ok(message key)` or
/// `Err(engine message)`. `None` while the summary is still the one shown when Refresh was
/// pressed (`requested_at`).
pub fn analytics_outcome(
    summary: Option<&AnalyticsSummary>,
    requested_at: i64,
) -> Option<Result<&'static str, String>> {
    let s = summary?;
    if s.generated_at == requested_at {
        return None;
    }
    Some(match &s.error {
        Some(err) => Err(err.clone()),
        None => Ok("Statistics updated"),
    })
}

/// Toast kind of an engine notice (`Event::Notice`, already translated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Success,
    Info,
    Error,
}

/// Notices are plain strings: the few confirmations and the "busy" note are recognised (in
/// either language, with or without the final period); anything else is a failure (sign-in
/// failed, secret not stored, setting refused, shortcut not registered …).
pub fn notice_kind(text: &str) -> NoticeKind {
    // The engine's own wording (crates/engine/src/i18n.rs), both languages.
    const SUCCESS: &[&str] = &[
        "Test message sent",
        "Message de test envoyé",
        "Hooks installed",
        "Hooks installés",
        "Hooks removed",
        "Hooks retirés",
        "Saved",
        "Enregistré",
        "The notch is back in the middle",
    ];
    const INFO: &[&str] = &[
        "Hooks are already being updated; try again in a moment",
        "Les hooks sont déjà en cours de mise à jour ; réessayez dans un instant",
    ];
    let norm = |s: &str| s.trim().trim_end_matches('.').to_lowercase();
    let text = norm(text);
    let matches = |keys: &[&str]| keys.iter().any(|k| norm(k) == text);
    if matches(SUCCESS) || text == norm(t(Lang::Fr, "The notch is back in the middle")) {
        NoticeKind::Success
    } else if matches(INFO) {
        NoticeKind::Info
    } else {
        NoticeKind::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::{
        DailyUsage, ModelUsage, ProviderUsage, Reading, Snapshot, TokenCounts,
    };

    fn config_with(kinds: &[ProviderKind]) -> Config {
        let mut c = Config::default();
        for (i, k) in kinds.iter().enumerate() {
            let mut a = Account::new(k.clone(), format!("acc{i}"));
            a.id = format!("id{i}");
            c.accounts.push(a);
        }
        c
    }

    #[test]
    fn thresholds_keep_warn_below_critical() {
        let mut c = Config::default();
        apply(&mut c, &Edit::WarnPercent(97)).unwrap();
        assert_eq!(
            (c.settings.alert_percent, c.settings.critical_percent),
            (97, 98)
        );
        apply(&mut c, &Edit::CriticalPercent(50)).unwrap();
        assert_eq!(
            (c.settings.alert_percent, c.settings.critical_percent),
            (49, 50)
        );
        apply(&mut c, &Edit::WarnPercent(0)).unwrap();
        assert_eq!(c.settings.alert_percent, 1);
        apply(&mut c, &Edit::WarnPercent(255)).unwrap();
        assert_eq!(
            (c.settings.alert_percent, c.settings.critical_percent),
            (99, 100)
        );
        apply(&mut c, &Edit::CriticalPercent(1)).unwrap();
        assert_eq!(
            (c.settings.alert_percent, c.settings.critical_percent),
            (1, 2)
        );
        apply(&mut c, &Edit::ResetLimits).unwrap();
        assert_eq!(
            (c.settings.alert_percent, c.settings.critical_percent),
            (WARN_DEFAULT, CRIT_DEFAULT)
        );
    }

    #[test]
    fn simple_edits_touch_their_key_only() {
        let mut c = Config::default();
        c.settings.extra.insert("futureFlag".into(), Value::from(3));
        c.extra.insert("top".into(), Value::from("keep"));
        let before = c.clone();
        apply(&mut c, &Edit::Theme(ThemeMode::Dark)).unwrap();
        apply(&mut c, &Edit::Language(Lang::Fr)).unwrap();
        apply(&mut c, &Edit::PollSeconds(10)).unwrap();
        assert_eq!(c.settings.theme, ThemeMode::Dark);
        assert_eq!(c.settings.language, "fr");
        assert_eq!(c.settings.poll_seconds, 30, "clamped to the engine's range");
        assert_eq!(c.settings.extra, before.settings.extra);
        assert_eq!(c.extra, before.extra);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["settings"]["futureFlag"], 3);
        assert_eq!(json["top"], "keep");
    }

    #[test]
    fn show_mode_keeps_legacy_widget_flag() {
        let mut c = Config::default();
        apply(&mut c, &Edit::ShowMode(ShowMode::Hidden)).unwrap();
        assert_eq!(c.settings.show_mode(), ShowMode::Hidden);
        assert!(!c.settings.widget);
        apply(&mut c, &Edit::ShowMode(ShowMode::Always)).unwrap();
        assert!(c.settings.widget);
    }

    #[test]
    fn last_notch_account_cannot_be_turned_off_or_hidden() {
        let mut c = config_with(&[ProviderKind::Claude, ProviderKind::Codex]);
        apply(
            &mut c,
            &Edit::AccountVisible {
                id: "id0".into(),
                on: false,
            },
        )
        .unwrap();
        assert_eq!(c.settings.hidden_accounts, vec!["id0".to_string()]);
        let before = c.clone();
        assert_eq!(
            apply(
                &mut c,
                &Edit::AccountEnabled {
                    id: "id1".into(),
                    on: false
                }
            ),
            Err(Refusal::LastNotchAccount)
        );
        assert_eq!(
            apply(
                &mut c,
                &Edit::AccountVisible {
                    id: "id1".into(),
                    on: false
                }
            ),
            Err(Refusal::LastNotchAccount)
        );
        assert_eq!(c, before, "refused edits leave the config untouched");
        apply(
            &mut c,
            &Edit::AccountVisible {
                id: "id0".into(),
                on: true,
            },
        )
        .unwrap();
        assert!(c.settings.hidden_accounts.is_empty());
        apply(
            &mut c,
            &Edit::AccountEnabled {
                id: "id1".into(),
                on: false,
            },
        )
        .unwrap();
        assert!(!c.accounts[1].enabled);
        assert_eq!(
            apply(
                &mut c,
                &Edit::AccountEnabled {
                    id: "nope".into(),
                    on: false
                }
            ),
            Err(Refusal::UnknownAccount)
        );
    }

    #[test]
    fn empty_config_allows_enabling() {
        let mut c = config_with(&[ProviderKind::Claude]);
        c.accounts[0].enabled = false;
        apply(
            &mut c,
            &Edit::AccountEnabled {
                id: "id0".into(),
                on: true,
            },
        )
        .unwrap();
        assert!(c.accounts[0].enabled);
    }

    #[test]
    fn rename_add_remove_accounts() {
        let mut c = config_with(&[ProviderKind::Claude, ProviderKind::Codex]);
        assert_eq!(
            apply(
                &mut c,
                &Edit::RenameAccount {
                    id: "id0".into(),
                    name: "  ".into()
                }
            ),
            Err(Refusal::EmptyName)
        );
        apply(
            &mut c,
            &Edit::RenameAccount {
                id: "id0".into(),
                name: " Claude · work ".into(),
            },
        )
        .unwrap();
        assert_eq!(c.accounts[0].name, "Claude · work");

        let custom = new_custom_account(" Team ", " https://x.example/usage ", true);
        let id = custom.id.clone();
        assert_eq!(custom.endpoint, "https://x.example/usage");
        assert!(custom.has_secret && custom.secret_env.is_empty());
        apply(&mut c, &Edit::AddAccount(Box::new(custom))).unwrap();
        assert_eq!(c.accounts.len(), 3);
        apply(
            &mut c,
            &Edit::AccountVisible {
                id: id.clone(),
                on: false,
            },
        )
        .unwrap();
        apply(&mut c, &Edit::RemoveAccount(id.clone())).unwrap();
        assert_eq!(c.accounts.len(), 2);
        assert!(!c.settings.hidden_accounts.contains(&id));
    }

    #[test]
    fn remove_marks_discovered_accounts_and_undo_restores_exactly() {
        let mut c = config_with(&[
            ProviderKind::Claude,
            ProviderKind::Claude,
            ProviderKind::Codex,
        ]);
        c.accounts[1].profile_path = "C:\\Users\\x\\.claude-2".into();
        c.accounts[1].name = "Claude (2)".into();
        apply(
            &mut c,
            &Edit::AccountVisible {
                id: "id1".into(),
                on: false,
            },
        )
        .unwrap();
        let before = c.clone();

        let removal = removal_of(&c, "id1").unwrap();
        assert_eq!((removal.index, removal.hidden), (1, true));
        assert_eq!(remove_blocked(&c, "id1"), None);
        apply(&mut c, &Edit::RemoveAccount("id1".into())).unwrap();
        assert!(c.account("id1").is_none());
        assert!(!c.settings.hidden_accounts.contains(&"id1".to_string()));
        // A marker so discovery does not bring it back (provider + folder + name).
        assert_eq!(c.settings.removed_accounts.len(), 1);
        let m = &c.settings.removed_accounts[0];
        assert_eq!(m.provider, ProviderKind::Claude);
        assert_eq!(m.name, "Claude (2)");
        assert!(m.same_profile(&removal.account));
        // Removing it again (same profile, new id) does not stack markers.
        let mut again = removal.account.clone();
        again.id = "id9".into();
        apply(&mut c, &Edit::AddAccount(Box::new(again))).unwrap();
        apply(&mut c, &Edit::RemoveAccount("id9".into())).unwrap();
        assert_eq!(c.settings.removed_accounts.len(), 1);

        // Undo: same place, same hidden flag, marker gone — the config as it was.
        apply(&mut c, &Edit::RestoreAccount(Box::new(removal.clone()))).unwrap();
        assert_eq!(c, before);
        // Restoring twice adds nothing.
        apply(&mut c, &Edit::RestoreAccount(Box::new(removal))).unwrap();
        assert_eq!(c, before);
    }

    #[test]
    fn custom_endpoints_leave_no_marker_and_add_back_drops_one() {
        let mut c = config_with(&[ProviderKind::Claude]);
        let custom = new_custom_account("Team", "https://x.example/usage", false);
        let id = custom.id.clone();
        apply(&mut c, &Edit::AddAccount(Box::new(custom))).unwrap();
        apply(&mut c, &Edit::RemoveAccount(id)).unwrap();
        assert!(c.settings.removed_accounts.is_empty());

        apply(
            &mut c,
            &Edit::AddAccount(Box::new(Account::new(ProviderKind::Codex, "Codex"))),
        )
        .unwrap();
        let codex = c.accounts.last().unwrap().id.clone();
        apply(&mut c, &Edit::RemoveAccount(codex)).unwrap();
        assert_eq!(c.settings.removed_accounts.len(), 1);
        apply(&mut c, &Edit::ForgetRemoved(5)).unwrap(); // out of range: no-op
        assert_eq!(c.settings.removed_accounts.len(), 1);
        apply(&mut c, &Edit::ForgetRemoved(0)).unwrap();
        assert!(c.settings.removed_accounts.is_empty());
    }

    #[test]
    fn the_last_notch_account_cannot_be_removed() {
        let c = config_with(&[ProviderKind::Claude]);
        assert_eq!(remove_blocked(&c, "id0"), Some(Refusal::LastNotchAccount));
        assert_eq!(remove_blocked(&c, "nope"), Some(Refusal::UnknownAccount));
        let mut c2 = c.clone();
        assert_eq!(
            apply(&mut c2, &Edit::RemoveAccount("id0".into())),
            Err(Refusal::LastNotchAccount)
        );
        assert_eq!(c2, c);
    }

    #[test]
    fn plan_prices_round_trip_and_remove() {
        let mut c = Config::default();
        apply(
            &mut c,
            &Edit::PlanPrice {
                key: "Claude Max".into(),
                eur: Some(108.0),
            },
        )
        .unwrap();
        assert_eq!(plan_price(&c.settings, "Claude Max"), Some(108.0));
        c.settings
            .plan_prices_eur
            .insert("Codex Plus".into(), Value::from("21,6"));
        assert_eq!(plan_price(&c.settings, "Codex Plus"), Some(21.6));
        apply(
            &mut c,
            &Edit::PlanPrice {
                key: "Claude Max".into(),
                eur: None,
            },
        )
        .unwrap();
        assert_eq!(plan_price(&c.settings, "Claude Max"), None);
        apply(&mut c, &Edit::UsdToEur(f64::NAN)).unwrap();
        assert_eq!(c.settings.usd_to_eur, 0.92);
        apply(&mut c, &Edit::UsdToEur(0.95)).unwrap();
        assert_eq!(c.settings.usd_to_eur, 0.95);
    }

    #[test]
    fn numbers_accept_french_and_english_decimals() {
        assert_eq!(parse_number("0,92"), Some(0.92));
        assert_eq!(parse_number("0.92"), Some(0.92));
        assert_eq!(parse_number(" 1 234,5 € "), Some(1234.5));
        assert_eq!(parse_number("1\u{202f}234,5"), Some(1234.5));
        assert_eq!(parse_number("1,234.5"), Some(1234.5));
        assert_eq!(parse_number("1.234,5"), Some(1234.5));
        assert_eq!(parse_number("21.6 $"), Some(21.6));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_price(""), Ok(None));
        assert_eq!(parse_price("-3"), Err(FieldError::OutOfRange));
        assert_eq!(parse_price("x"), Err(FieldError::NotANumber));
        assert_eq!(parse_rate("0"), Err(FieldError::OutOfRange));
        assert_eq!(parse_rate("0,9"), Ok(0.9));
        assert_eq!(format_number(Lang::Fr, 0.92), "0,92");
        assert_eq!(format_number(Lang::En, 108.0), "108");
        assert_eq!(format_number(Lang::En, 21.599999), "21.6");
        assert_eq!(parse_number(&format_number(Lang::Fr, 1234.5)), Some(1234.5));
    }

    #[test]
    fn endpoint_and_names() {
        assert_eq!(
            validate_endpoint(" https://usage.example.com/q "),
            Ok("https://usage.example.com/q".into())
        );
        assert!(validate_endpoint("http://127.0.0.1:9/u").is_ok());
        assert_eq!(
            validate_endpoint("http://example.com/u"),
            Err(FieldError::Url)
        );
        assert_eq!(
            validate_endpoint("https://exa mple.com"),
            Err(FieldError::Url)
        );
        assert_eq!(validate_endpoint(""), Err(FieldError::Url));
        assert_eq!(validate_name("  "), Err(FieldError::EmptyName));
        assert_eq!(validate_name(&"x".repeat(100)).unwrap().len(), 60);
    }

    #[test]
    fn hotkeys_normalise_or_fail() {
        assert_eq!(
            normalize_hotkey("ctrl + alt + n"),
            Ok(Some("Ctrl+Alt+N".into()))
        );
        assert_eq!(
            normalize_hotkey("Shift+Win+F12"),
            Ok(Some("Shift+Win+F12".into()))
        );
        assert_eq!(
            normalize_hotkey("Win+Shift+F12"),
            Ok(Some("Shift+Win+F12".into()))
        );
        assert_eq!(
            normalize_hotkey("Alt+Control+pgup"),
            Ok(Some("Ctrl+Alt+PageUp".into()))
        );
        assert_eq!(normalize_hotkey(""), Ok(None));
        assert_eq!(normalize_hotkey("N"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Shift+N"), Err(FieldError::HotkeyFormat));
        assert_eq!(
            normalize_hotkey("Ctrl+Ctrl+N"),
            Err(FieldError::HotkeyFormat)
        );
        assert_eq!(normalize_hotkey("Ctrl+N+M"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Ctrl+"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Ctrl+F25"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Ctrl+F01"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Ctrl+Alt"), Err(FieldError::HotkeyFormat));
        assert_eq!(normalize_hotkey("Ctrl+é"), Err(FieldError::HotkeyFormat));
    }

    #[test]
    fn notification_fields() {
        assert_eq!(validate_chat_id(" -100123 "), Ok("-100123".into()));
        assert_eq!(validate_chat_id("@my_channel"), Ok("@my_channel".into()));
        assert_eq!(validate_chat_id(""), Ok(String::new()));
        assert_eq!(validate_chat_id("12a"), Err(FieldError::ChatId));
        assert_eq!(validate_chat_id("@"), Err(FieldError::ChatId));
        assert!(validate_telegram_token("123456:ABCdefGhIJKlmNoPQRsTUVwxyZ_12").is_ok());
        assert_eq!(
            validate_telegram_token("123456"),
            Err(FieldError::TelegramToken)
        );
        assert_eq!(
            validate_telegram_token("abc:ABCdefGhIJKlmNoPQRsTUVwxyZ"),
            Err(FieldError::TelegramToken)
        );
        assert!(validate_discord_webhook("https://discord.com/api/webhooks/1/x").is_ok());
        assert_eq!(
            validate_discord_webhook("http://discord.com/api/webhooks/1/x"),
            Err(FieldError::DiscordWebhook)
        );
    }

    #[test]
    fn sync_adopts_external_changes_and_keeps_pending_edits() {
        let base = Config::default();
        let mut edited = base.clone();
        let mut sync = Sync::new(base.clone());
        // Same snapshot again: nothing.
        assert!(!sync.on_snapshot(&mut edited, &base));
        // Debounced slider edit, then an engine-side change (new account).
        apply(&mut edited, &Edit::WarnPercent(60)).unwrap();
        sync.record_pending(Edit::WarnPercent(60));
        assert!(sync.pending_thresholds());
        let mut external = base.clone();
        external
            .accounts
            .push(Account::new(ProviderKind::Claude, "new"));
        assert!(sync.on_snapshot(&mut edited, &external));
        assert_eq!(edited.accounts.len(), 1, "external change adopted");
        assert_eq!(edited.settings.alert_percent, 60, "pending edit survives");
        // Sending clears pending.
        sync.record_sent(edited.clone());
        assert!(!sync.has_pending());
    }

    #[test]
    fn sync_ignores_stale_echoes_while_newer_saves_are_in_flight() {
        let base = Config::default();
        let mut edited = base.clone();
        let mut sync = Sync::new(base.clone());
        apply(&mut edited, &Edit::Theme(ThemeMode::Dark)).unwrap();
        let first = edited.clone();
        sync.record_sent(first.clone());
        apply(&mut edited, &Edit::Edge(Edge::Left)).unwrap();
        let second = edited.clone();
        sync.record_sent(second.clone());
        // Echo of the first save: must not undo the edge change.
        assert!(!sync.on_snapshot(&mut edited, &first));
        assert_eq!(edited, second);
        // Echo of the second: consistent, nothing to change.
        assert!(!sync.on_snapshot(&mut edited, &second));
        assert_eq!(edited, second);
    }

    #[test]
    fn choices_and_labels() {
        assert_eq!(
            choices_with_current(POLL_CHOICES, 60),
            vec![30, 60, 120, 300]
        );
        assert_eq!(
            choices_with_current(POLL_CHOICES, 45),
            vec![30, 45, 60, 120, 300]
        );
        assert_eq!(seconds_label(30), "30 s");
        assert_eq!(seconds_label(120), "2 min");
        assert_eq!(seconds_label(90), "1 min 30 s");
        assert_eq!(minutes_label(90), "1 h 30");
        assert_eq!(minutes_label(60), "1 h");
        assert_eq!(hours_label(Lang::Fr, 168), "7 j");
        assert_eq!(hours_label(Lang::En, 24), "24 h");
        assert_eq!(hours_label(Lang::En, 36), "36 h");
    }

    #[test]
    fn monitor_list_marks_primary_and_stores_empty_for_it() {
        let mons = vec![
            (r"\\.\DISPLAY1".to_string(), true),
            (r"\\.\DISPLAY2".to_string(), false),
        ];
        let (labels, sel, values) = monitor_choices(Lang::En, &mons, "");
        assert_eq!(labels, ["Screen 1 · Main", "Screen 2"]);
        assert_eq!(sel, 0);
        assert_eq!(values, ["", r"\\.\DISPLAY2"]);
        assert_eq!(monitor_choices(Lang::En, &mons, r"\\.\DISPLAY2").1, 1);
        assert_eq!(monitor_choices(Lang::En, &mons, r"\\.\GONE").1, 0);
    }

    #[test]
    fn account_rows_from_demo() {
        let snap = Snapshot::demo_at(1_800_000_000);
        let rows = account_rows(Lang::En, &snap.config, &snap.accounts, snap.now);
        assert_eq!(rows.len(), snap.config.accounts.len());
        assert_eq!(rows[0].provider, ProviderKind::Claude);
        assert!(!rows[0].can_move_up && rows.last().is_some_and(|r| !r.can_move_down));
        let grok = rows
            .iter()
            .find(|r| r.provider == ProviderKind::Grok)
            .unwrap();
        assert!(grok.signed_out);
        assert!(grok.status.starts_with("not signed in"));
        assert_eq!(grok.tone, Tone::Warn);
        let custom = rows
            .iter()
            .find(|r| r.provider == ProviderKind::Custom)
            .unwrap();
        assert!(custom.removable);
        let claude = &rows[0];
        assert!(claude
            .subtitle
            .as_deref()
            .is_some_and(|s| s.contains("Max")));
        let fr = account_rows(Lang::Fr, &snap.config, &snap.accounts, snap.now);
        let grok_fr = fr
            .iter()
            .find(|r| r.provider == ProviderKind::Grok)
            .unwrap();
        assert!(grok_fr.status.starts_with("non connecté"));
    }

    #[test]
    fn status_lines_cover_every_state() {
        let a = Account::new(ProviderKind::Codex, "x");
        let mut v = AccountView::new(a.clone());
        assert_eq!(
            status_line(Lang::En, Some(&v), false, false, 0).0,
            "Turned off"
        );
        assert_eq!(
            status_line(Lang::En, Some(&v), true, false, 0).0,
            "Waiting for the first reading…"
        );
        v.reading = Some(Reading::error(&a, 0, "boom"));
        assert_eq!(
            status_line(Lang::En, Some(&v), true, false, 0),
            ("not reachable".into(), Tone::Error)
        );
        v.reading = Some(Reading::rate_limited(&a, 0, None));
        assert_eq!(
            status_line(Lang::En, Some(&v), true, false, 0).1,
            Tone::Warn
        );
        v.reading = Some(Reading::fresh(
            &a,
            1,
            vec![codenotch_core::LimitWindow::new(
                "primary",
                "Current session",
                codenotch_core::WindowKind::Session,
            )
            .used(42.0)],
        ));
        assert_eq!(
            status_line(Lang::En, Some(&v), true, false, 10).0,
            "Current session · 42% used · 58% left"
        );
        let (stale, tone) = status_line(Lang::En, Some(&v), true, false, 100_000);
        assert!(stale.ends_with("last reading is old") && tone == Tone::Warn);
    }

    #[test]
    fn status_line_appends_the_forecast() {
        use codenotch_core::demo::ids;
        let now = 1_800_000_000; // 08:00 UTC
        let utc = chrono::FixedOffset::east_opt(0).unwrap();
        let mut s = codenotch_core::Snapshot::demo_at(now);
        let settings = s.config.settings.clone();
        let line = |s: &codenotch_core::Snapshot, lang, settings: &Settings| {
            status_line_tz(
                lang,
                s.account(ids::CLAUDE),
                true,
                settings,
                now,
                &utc,
                true,
            )
            .0
        };
        // The session (headline) runs out first: no label.
        let fr = line(&s, Lang::Fr, &settings);
        assert!(
            fr.starts_with("Session en cours · 61 % utilisés · 39 % restants · épuisé vers 09:"),
            "{fr}"
        );
        // Only the weekly window runs out: labelled.
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            let mut f = r.windows[1].forecast.unwrap();
            f.exhaust_at = Some(now + 26 * 3600);
            r.windows[1].forecast = Some(f);
            let mut g = r.windows[0].forecast.unwrap();
            g.exhaust_at = None;
            g.at_reset_percent = 81.6;
            r.windows[0].forecast = Some(g);
        }
        let en = line(&s, Lang::En, &settings);
        assert!(en.ends_with(" · Weekly limit: runs out ~Sat 10:00"), "{en}");
        // Nothing runs out: the headline's percent at reset.
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            r.windows[1].forecast = None;
        }
        assert!(line(&s, Lang::Fr, &settings).ends_with(" · ≈ 82 % au reset"));
        // Setting off: plain line.
        let mut off = settings.clone();
        off.show_forecast = false;
        assert_eq!(
            line(&s, Lang::En, &off),
            "Current session · 61% used · 39% left"
        );
    }

    #[test]
    fn update_line_never_claims_up_to_date_before_a_check() {
        let (text, action) = update_line(Lang::En, &UpdateState::Idle, None, 0);
        assert!(!text.contains("Up to date"));
        assert_eq!(action, UpdateAction::Check);
        assert_eq!(
            update_line(Lang::En, &UpdateState::Checking { manual: true }, None, 0).1,
            UpdateAction::Busy
        );
        let (text, _) = update_line(
            Lang::En,
            &UpdateState::UpToDate {
                checked_at: 0,
                manual: false,
            },
            Some(0),
            30,
        );
        assert_eq!(text, "Up to date · checked just now");
        let available = UpdateState::Available {
            version: "1.1.0".into(),
            notes: String::new(),
            notes_url: None,
            forced: false,
            manual: false,
        };
        let (text, action) = update_line(Lang::Fr, &available, None, 0);
        assert_eq!(text, "La version 1.1.0 est disponible");
        assert_eq!(action, UpdateAction::Install);
        let (text, _) = update_line(Lang::En, &available, Some(0), 30);
        assert_eq!(text, "Version 1.1.0 is available · checked just now");
        let (text, action) = update_line(
            Lang::Fr,
            &UpdateState::Downloading {
                version: "1.1.0".into(),
                progress: 42,
                forced: false,
            },
            None,
            0,
        );
        assert_eq!(text, "Téléchargement de la version 1.1.0… 42 %");
        assert_eq!(action, UpdateAction::Busy);
        assert_eq!(
            update_line(
                Lang::En,
                &UpdateState::Ready {
                    version: "1.1.0".into(),
                    forced: false
                },
                None,
                0
            ),
            (
                "Version 1.1.0 is ready to install".to_string(),
                UpdateAction::Install
            )
        );
        assert_eq!(
            update_line(
                Lang::En,
                &UpdateState::Installing {
                    version: "1.1.0".into()
                },
                None,
                0
            )
            .1,
            UpdateAction::Busy
        );
        let (text, action) = update_line(
            Lang::En,
            &UpdateState::Failed {
                message: "offline".into(),
                manual: true,
            },
            None,
            0,
        );
        assert!(text.contains("offline"));
        assert_eq!(action, UpdateAction::Check);
    }

    #[test]
    fn teams_endpoint_edits_and_shared_accounts() {
        assert_eq!(validate_teams_endpoint("  "), Ok(String::new()));
        assert_eq!(
            validate_teams_endpoint(" https://teams.example.test/ "),
            Ok("https://teams.example.test".into())
        );
        assert_eq!(
            validate_teams_endpoint("http://127.0.0.1:8080"),
            Ok("http://127.0.0.1:8080".into())
        );
        assert!(validate_teams_endpoint("http://[::1]:9").is_ok());
        for bad in [
            "http://teams.example.test",
            "ftp://x",
            "https://user@x.test",
            "https://x.test/path",
            "https://x.test?q=1",
            "https://",
            "https://a b",
        ] {
            assert_eq!(validate_teams_endpoint(bad), Err(FieldError::Url), "{bad}");
        }
        let mut c = Config::default();
        let claude = Account::new(ProviderKind::Claude, "C");
        let cursor = Account::new(ProviderKind::Cursor, "Cu");
        c.accounts = vec![claude.clone(), cursor];
        assert!(!c.settings.teams_sync_enabled);
        apply(&mut c, &Edit::TeamsSyncEnabled(true)).unwrap();
        apply(&mut c, &Edit::TeamsEndpoint(" https://t.test ".into())).unwrap();
        assert!(c.settings.teams_sync_enabled);
        assert_eq!(c.settings.teams_endpoint, "https://t.test");
        let only = teams_accounts(&c);
        assert_eq!(only.len(), 1, "only Claude and Codex accounts");
        assert!(only[0].1);
        let off = Edit::TeamsShared {
            id: claude.id.clone(),
            on: false,
        };
        apply(&mut c, &off).unwrap();
        apply(&mut c, &off).unwrap();
        assert_eq!(c.settings.teams_excluded_accounts, vec![claude.id.clone()]);
        assert!(!teams_accounts(&c)[0].1);
        apply(
            &mut c,
            &Edit::TeamsShared {
                id: claude.id.clone(),
                on: true,
            },
        )
        .unwrap();
        assert!(c.settings.teams_excluded_accounts.is_empty());
    }

    #[test]
    fn teams_status_lines() {
        let mut st = TeamsStatus::default();
        assert_eq!(teams_status_line(Lang::En, &st).1, Tone::Muted);
        assert!(!teams_paired(&st));
        st.state = TeamsState::NotPaired;
        assert_eq!(
            teams_status_line(Lang::En, &st).0,
            "Not connected to a team"
        );
        st.state = TeamsState::Active;
        st.team_name = Some("Acme".into());
        st.queued = 3;
        assert!(teams_paired(&st));
        assert_eq!(
            teams_status_line(Lang::En, &st).0,
            "Connected to Acme · 3 events waiting"
        );
        assert_eq!(
            teams_status_line(Lang::Fr, &st).0,
            "Connecté à Acme · 3 événements en attente"
        );
        st.state = TeamsState::Revoked;
        st.last_error = Some("Device revoked by the team".into());
        let (line, tone) = teams_status_line(Lang::Fr, &st);
        assert_eq!(tone, Tone::Error);
        assert_eq!(line, "Appareil révoqué par l'équipe");
    }

    #[test]
    fn update_url_and_mode_edits() {
        assert_eq!(validate_update_url("  "), Ok(String::new()));
        assert_eq!(
            validate_update_url(" https://example.com/update.json "),
            Ok("https://example.com/update.json".into())
        );
        for bad in [
            "http://example.com/u.json",
            "https://",
            "https:///x",
            "file:///C:/u.json",
            "example.com/u.json",
            "https://exa mple.com/u.json",
        ] {
            assert_eq!(
                validate_update_url(bad),
                Err(FieldError::HttpsOnly),
                "{bad}"
            );
        }
        let mut c = Config::default();
        apply(&mut c, &Edit::UpdateMode(UpdateMode::Off)).unwrap();
        assert_eq!(c.settings.update_mode(), UpdateMode::Off);
        assert!(!c.settings.update_checks, "legacy switch kept in sync");
        apply(&mut c, &Edit::UpdateMode(UpdateMode::Auto)).unwrap();
        assert!(c.settings.update_checks);
        apply(
            &mut c,
            &Edit::UpdateManifestUrl(" https://e.com/u.json ".into()),
        )
        .unwrap();
        assert_eq!(c.settings.update_manifest_url, "https://e.com/u.json");
    }

    fn summary() -> AnalyticsSummary {
        let tc = |n| TokenCounts {
            input: n,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        AnalyticsSummary {
            days: 30,
            generated_at: 5,
            total_tokens: tc(600),
            total_cost_usd: 10.0,
            per_provider: vec![ProviderUsage {
                provider: ProviderKind::Claude,
                tokens: tc(600),
                cost_usd: 10.0,
                sessions: 3,
                unpriced_tokens: 100,
            }],
            per_day: vec![
                DailyUsage {
                    date: "2026-11-02".into(),
                    provider: ProviderKind::Claude,
                    tokens: tc(100),
                    cost_usd: 1.0,
                    unpriced_tokens: 100,
                },
                DailyUsage {
                    date: "2026-11-01".into(),
                    provider: ProviderKind::Claude,
                    tokens: tc(200),
                    cost_usd: 2.0,
                    ..Default::default()
                },
                DailyUsage {
                    date: "2026-11-02".into(),
                    provider: ProviderKind::Codex,
                    tokens: tc(300),
                    cost_usd: 3.0,
                    ..Default::default()
                },
            ],
            per_model: vec![
                ModelUsage {
                    model: "opus".into(),
                    provider: ProviderKind::Claude,
                    tokens: tc(500),
                    cost_usd: 10.0,
                    ..Default::default()
                },
                ModelUsage {
                    model: "mystery".into(),
                    provider: ProviderKind::Claude,
                    tokens: tc(100),
                    cost_usd: 0.0,
                    unpriced_tokens: 100,
                },
            ],
            unpriced_tokens: 100,
            error: None,
            detail: Default::default(),
        }
    }

    #[test]
    fn unpriced_and_comparison() {
        let s = summary();
        let mut config = config_with(&[ProviderKind::Claude, ProviderKind::Claude]);
        config.settings.usd_to_eur = 0.9;
        let views: Vec<AccountView> = config
            .accounts
            .iter()
            .cloned()
            .map(AccountView::new)
            .collect();
        // No plan known → key is the provider name; two accounts share it.
        let plans = plan_lines(&config, &views);
        assert_eq!(
            plans,
            vec![PlanLine {
                key: "Claude".into(),
                provider: ProviderKind::Claude,
                accounts: 2,
                plan: None,
            }]
        );
        let c = comparisons(&s, &config, &views);
        assert_eq!(c[0].plan_eur, None);
        assert!((c[0].api_eur - 9.0).abs() < 1e-9);
        apply(
            &mut config,
            &Edit::PlanPrice {
                key: "Claude".into(),
                eur: Some(20.0),
            },
        )
        .unwrap();
        let c = comparisons(&s, &config, &views);
        // 2 accounts × 20 € × 30 days / 30.44 (same month length as the analytics report).
        let plan = 40.0 * 30.0 / codenotch_core::DAYS_PER_MONTH;
        assert!((c[0].plan_eur.unwrap() - plan).abs() < 1e-9);
        assert!((c[0].api_eur / c[0].plan_eur.unwrap() - 9.0 / plan).abs() < 1e-9);
        assert_eq!(plan_key(&ProviderKind::Codex, Some(" Plus ")), "Codex Plus");
        assert_eq!(c[0].unpriced_tokens, 100);
    }

    #[test]
    fn plan_prices_resolve_user_first_then_list_price() {
        let mut st = Settings {
            usd_to_eur: 0.5,
            ..Settings::default()
        };
        let claude = ProviderKind::Claude;
        // Nothing typed: the list price, in euros.
        assert_eq!(
            resolve_plan_price(&st, &claude, Some("Max 20x")),
            PlanPrice::List(100.0)
        );
        assert_eq!(
            resolve_plan_price(&st, &claude, Some("Team")),
            PlanPrice::Unknown
        );
        assert_eq!(resolve_plan_price(&st, &claude, None), PlanPrice::Unknown);
        // A typed price wins, even below the list price.
        apply_settings_price(&mut st, "Claude Max 20x", Some(150.0));
        assert_eq!(
            resolve_plan_price(&st, &claude, Some("Max 20x")),
            PlanPrice::User(150.0)
        );
        // 0 = leave the plan out; clearing the field goes back to the list price.
        apply_settings_price(&mut st, "Claude Max 20x", Some(0.0));
        assert_eq!(
            resolve_plan_price(&st, &claude, Some("Max 20x")),
            PlanPrice::Off
        );
        assert_eq!(PlanPrice::Off.eur(), None);
        apply_settings_price(&mut st, "Claude Max 20x", None);
        assert_eq!(
            resolve_plan_price(&st, &claude, Some("Max 20x")),
            PlanPrice::List(100.0)
        );
        assert_eq!(
            list_price_eur(&st, &ProviderKind::Cursor, Some("Ultra")),
            Some(100.0)
        );
    }

    fn apply_settings_price(st: &mut Settings, key: &str, eur: Option<f64>) {
        let mut c = Config {
            settings: st.clone(),
            ..Config::default()
        };
        apply(
            &mut c,
            &Edit::PlanPrice {
                key: key.into(),
                eur,
            },
        )
        .unwrap();
        *st = c.settings;
    }

    #[test]
    fn comparison_uses_list_prices_only_for_providers_with_usage() {
        let mut s = summary();
        // Cursor appears in the summary without usage: its plan is never compared.
        s.per_provider.push(codenotch_core::model::ProviderUsage {
            provider: ProviderKind::Cursor,
            ..Default::default()
        });
        let mut config = config_with(&[ProviderKind::Claude, ProviderKind::Cursor]);
        config.settings.usd_to_eur = 0.9;
        let views: Vec<AccountView> = config
            .accounts
            .iter()
            .cloned()
            .map(|a| {
                let plan = match a.provider {
                    ProviderKind::Claude => "Max 20x",
                    _ => "Ultra",
                };
                let mut r = codenotch_core::model::Reading::fresh(&a, 1, vec![]);
                r.plan = Some(plan.into());
                let mut v = AccountView::new(a);
                v.reading = Some(r);
                v
            })
            .collect();
        let month = 30.0 / codenotch_core::DAYS_PER_MONTH;
        let c = comparisons(&s, &config, &views);
        assert_eq!(c.len(), 2);
        assert!((c[0].plan_eur.unwrap() - 200.0 * 0.9 * month).abs() < 1e-9);
        assert!(c[0].list_price);
        assert_eq!(c[1].provider, ProviderKind::Cursor);
        assert_eq!(c[1].plan_eur, None, "no usage data, no comparison");
        // The user's price replaces the list price.
        apply(
            &mut config,
            &Edit::PlanPrice {
                key: "Claude Max 20x".into(),
                eur: Some(90.0),
            },
        )
        .unwrap();
        let c = comparisons(&s, &config, &views);
        assert!((c[0].plan_eur.unwrap() - 90.0 * month).abs() < 1e-9);
        assert!(!c[0].list_price);
        // 0 turns the comparison off.
        apply(
            &mut config,
            &Edit::PlanPrice {
                key: "Claude Max 20x".into(),
                eur: Some(0.0),
            },
        )
        .unwrap();
        assert_eq!(comparisons(&s, &config, &views)[0].plan_eur, None);
    }

    fn rows_of(spec: &[(&str, ProviderKind)]) -> Vec<(String, ProviderKind)> {
        spec.iter()
            .map(|(id, k)| (id.to_string(), k.clone()))
            .collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn dropping_moves_a_row_within_its_provider_or_the_whole_provider() {
        use ProviderKind::{Claude, Codex, Cursor};
        // c1 c2 | x | u1 u2
        let rows = rows_of(&[
            ("c1", Claude),
            ("c2", Claude),
            ("x", Codex),
            ("u1", Cursor),
            ("u2", Cursor),
        ]);
        // Same place (before or after itself): nothing.
        assert_eq!(drop_order(&rows, 0, 0), None);
        assert_eq!(drop_order(&rows, 0, 1), None);
        // Within Claude: c1 after c2.
        assert_eq!(
            drop_order(&rows, 0, 2),
            Some(ids(&["c2", "c1", "x", "u1", "u2"]))
        );
        assert_eq!(
            drop_order(&rows, 4, 3),
            Some(ids(&["c1", "c2", "x", "u2", "u1"]))
        );
        // Codex to the top / the end.
        assert_eq!(
            drop_order(&rows, 2, 0),
            Some(ids(&["x", "c1", "c2", "u1", "u2"]))
        );
        assert_eq!(
            drop_order(&rows, 2, 5),
            Some(ids(&["c1", "c2", "u1", "u2", "x"]))
        );
        // A Claude row dropped inside Cursor: all of Claude moves to the nearest provider
        // boundary (before / after Cursor are as near: the first one wins).
        assert_eq!(
            drop_order(&rows, 1, 4),
            Some(ids(&["x", "c1", "c2", "u1", "u2"]))
        );
        // Cursor row dropped between c1 and c2: nearest boundary is the top (slot 0 vs 2 tie
        // goes to the lower one) — Cursor moves above Claude.
        assert_eq!(
            drop_order(&rows, 3, 1),
            Some(ids(&["u1", "u2", "c1", "c2", "x"]))
        );
        // Out of range.
        assert_eq!(drop_order(&rows, 9, 0), None);
        assert_eq!(drop_order(&rows, 0, 9), None);
    }

    #[test]
    fn keyboard_moves_step_past_the_neighbour() {
        use ProviderKind::{Claude, Codex, Cursor};
        let rows = rows_of(&[("c1", Claude), ("c2", Claude), ("x", Codex), ("u1", Cursor)]);
        assert_eq!(step_order(&rows, 0, true), None);
        assert_eq!(step_order(&rows, 3, false), None);
        assert_eq!(
            step_order(&rows, 0, false),
            Some(ids(&["c2", "c1", "x", "u1"]))
        );
        assert_eq!(
            step_order(&rows, 1, true),
            Some(ids(&["c2", "c1", "x", "u1"]))
        );
        // Last Claude row down: Claude swaps with Codex.
        assert_eq!(
            step_order(&rows, 1, false),
            Some(ids(&["x", "c1", "c2", "u1"]))
        );
        // Codex up: past the whole of Claude.
        assert_eq!(
            step_order(&rows, 2, true),
            Some(ids(&["x", "c1", "c2", "u1"]))
        );
        assert_eq!(
            step_order(&rows, 2, false),
            Some(ids(&["c1", "c2", "u1", "x"]))
        );
    }

    #[test]
    fn reorder_sets_provider_and_account_order() {
        let mut c = config_with(&[
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Claude,
        ]);
        let order = |c: &Config| -> Vec<String> {
            account_rows(Lang::En, c, &[], 0)
                .into_iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(order(&c), ids(&["id0", "id2", "id1"]));
        let rows: Vec<(String, ProviderKind)> = account_rows(Lang::En, &c, &[], 0)
            .into_iter()
            .map(|r| (r.id, r.provider))
            .collect();
        // Second Claude account first, then Codex above Claude.
        let next = drop_order(&rows, 1, 0).unwrap();
        apply(&mut c, &Edit::ReorderAccounts(next)).unwrap();
        assert_eq!(order(&c), ids(&["id2", "id0", "id1"]));
        let rows: Vec<(String, ProviderKind)> = account_rows(Lang::En, &c, &[], 0)
            .into_iter()
            .map(|r| (r.id, r.provider))
            .collect();
        let next = drop_order(&rows, 2, 0).unwrap();
        apply(&mut c, &Edit::ReorderAccounts(next)).unwrap();
        assert_eq!(order(&c), ids(&["id1", "id2", "id0"]));
        // The notch follows (enabled accounts, same order).
        let notch: Vec<&str> = c.notch_accounts().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(notch, vec!["id1", "id2", "id0"]);
        let full = c.settings.effective_provider_order();
        assert_eq!(&full[..2], &[ProviderKind::Codex, ProviderKind::Claude]);
        assert!(ProviderKind::CANONICAL_ORDER
            .iter()
            .all(|k| full.contains(k)));
    }

    #[test]
    fn costs_with_unpriced_tokens_are_lower_bounds() {
        let usd = |v: f64| format!("${v:.2}");
        assert_eq!(cost_text(10.0, 0, usd), "$10.00");
        assert_eq!(cost_text(10.0, 5, usd), "≥ $10.00");
        assert_eq!(cost_text(0.0, 5, usd), "—");
        assert_eq!(cost_text(0.0, 0, usd), "$0.00");
    }

    #[test]
    fn plan_prices_accept_older_key_forms() {
        let mut s = Settings::default();
        s.plan_prices_eur.insert("claude".into(), Value::from(90));
        assert_eq!(
            plan_price_any(&s, &ProviderKind::Claude, Some("Max")),
            Some(90.0)
        );
        s.plan_prices_eur.insert("Max".into(), Value::from(100));
        assert_eq!(
            plan_price_any(&s, &ProviderKind::Claude, Some("Max")),
            Some(100.0)
        );
        s.plan_prices_eur
            .insert("claude max".into(), Value::from(180));
        assert_eq!(
            plan_price_any(&s, &ProviderKind::Claude, Some("Max")),
            Some(180.0)
        );
        // Saving through the page writes the canonical key and drops the case variant.
        let mut c = Config {
            settings: s,
            ..Default::default()
        };
        apply(
            &mut c,
            &Edit::PlanPrice {
                key: plan_key(&ProviderKind::Claude, Some("Max")),
                eur: Some(200.0),
            },
        )
        .unwrap();
        assert!(c.settings.plan_prices_eur.contains_key("Claude Max"));
        assert!(!c.settings.plan_prices_eur.contains_key("claude max"));
        assert_eq!(plan_price(&c.settings, "Claude Max"), Some(200.0));
    }

    #[test]
    fn notices_are_classified() {
        assert_eq!(notice_kind("Test message sent."), NoticeKind::Success);
        assert_eq!(notice_kind("Message de test envoyé."), NoticeKind::Success);
        assert_eq!(notice_kind("Hooks installés."), NoticeKind::Success);
        assert_eq!(notice_kind("Hooks removed."), NoticeKind::Success);
        assert_eq!(
            notice_kind("Hooks are already being updated; try again in a moment."),
            NoticeKind::Info
        );
        assert_eq!(notice_kind("Sign-in failed: timeout"), NoticeKind::Error);
        assert_eq!(notice_kind("Unknown account."), NoticeKind::Error);
    }

    #[test]
    fn analytics_outcomes() {
        let mut s = summary();
        let shown = s.generated_at;
        assert_eq!(analytics_outcome(None, 0), None);
        assert_eq!(analytics_outcome(Some(&s), shown), None);
        assert_eq!(
            analytics_outcome(Some(&s), shown - 1),
            Some(Ok("Statistics updated"))
        );
        s.error = Some("disk".into());
        assert_eq!(
            analytics_outcome(Some(&s), shown - 1),
            Some(Err("disk".into()))
        );
    }

    #[test]
    fn analytics_states() {
        let s = summary();
        assert_eq!(analytics_state(None, None), AnalyticsState::NotComputed);
        assert_eq!(analytics_state(None, Some(0)), AnalyticsState::Loading);
        assert_eq!(analytics_state(Some(&s), Some(5)), AnalyticsState::Loading);
        assert_eq!(analytics_state(Some(&s), Some(1)), AnalyticsState::Ready);
        assert_eq!(
            analytics_state(Some(&AnalyticsSummary::default()), None),
            AnalyticsState::Empty
        );
    }

    #[test]
    fn onboarding_lists_discovered_accounts_only() {
        let snap = Snapshot::demo_at(1_800_000_000);
        let found = discovered_accounts(&snap.accounts);
        assert!(found.len() < snap.accounts.len());
        assert!(found
            .iter()
            .all(|v| v.account.provider != ProviderKind::Qoder));
        let mut c = snap.config.clone();
        c.settings.onboarding_done = None;
        apply(&mut c, &Edit::MarkOnboarded).unwrap();
        assert!(!c.needs_welcome());
    }
}
