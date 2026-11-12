//! `config.gpui.json`: accounts and settings.
//!
//! Compatibility rules (binding):
//! * Every key written by the 0.2/0.3 apps keeps its camelCase name and meaning.
//! * Unknown keys survive a load → save round trip (`extra` maps, `#[serde(flatten)]`).
//! * A bad value for an enum setting falls back to its default instead of failing the whole file.
//! * Secrets are never stored here: only the *name* of an environment variable (`secretEnv`)
//!   and a `hasSecret` flag. The Telegram chat id is stored (as in 0.3) but wrapped in
//!   [`Private`] so it never shows in `Debug` output or logs.
//! * Saving is atomic (temp file + rename).

use crate::error::{Error, Result};
use crate::model::ProviderKind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

/// Deserialize a value, falling back to `T::default()` when it has the wrong shape.
fn lenient<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// A string that must not appear in logs or `Debug` output (e.g. a Telegram chat id).
/// Serialises as a plain JSON string.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Private(pub String);

impl Private {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Private {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            f.write_str("Private(empty)")
        } else {
            f.write_str("Private(***)")
        }
    }
}

impl From<&str> for Private {
    fn from(s: &str) -> Self {
        Private(s.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------------------------

/// One configured account (one notch cell). Field names match the 0.2 file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Stable id, unique in the file. Used as key everywhere (history `a`, state, commands).
    pub id: String,
    /// User-visible name ("Claude · perso").
    pub name: String,
    pub provider: ProviderKind,
    /// Disabled accounts are neither fetched nor shown.
    pub enabled: bool,
    /// Provider profile folder (`C:\Users\x\.claude`). Empty = the provider's default folder.
    #[serde(default)]
    pub profile_path: String,
    /// Custom JSON endpoint URL (custom provider, or override). Empty = none.
    #[serde(default)]
    pub endpoint: String,
    /// NAME of the environment variable that holds the endpoint's bearer token. Never the token.
    #[serde(default)]
    pub secret_env: String,
    /// True when the user said a secret exists for this account (informational).
    #[serde(default)]
    pub has_secret: bool,
    /// Unknown keys, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Account {
    /// New enabled account with a fresh unique id (`<provider>-<hex time><counter>`).
    pub fn new(provider: ProviderKind, name: impl Into<String>) -> Self {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) & 0xfff;
        Self {
            id: format!("{}-{:x}{:03x}", provider.id(), nanos & 0xffff_ffff_ffff, n),
            name: name.into(),
            provider,
            enabled: true,
            profile_path: String::new(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
            extra: Map::new(),
        }
    }

    /// True when this account reads a custom JSON endpoint (provider `custom`, or any
    /// account with a non-empty `endpoint`, as in 0.3).
    pub fn uses_custom_endpoint(&self) -> bool {
        self.provider == ProviderKind::Custom || !self.endpoint.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// Settings enums
// ---------------------------------------------------------------------------------------------

/// Screen edge the notch is pinned to. Key `widgetPosition` (0.3 name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// Left and right edges hold an upright (vertical) pill.
    pub fn is_upright(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

/// `theme`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// `notchSize`: Small 0.8, Medium 1.0, Large 1.25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotchSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl NotchSize {
    pub fn scale(self) -> f32 {
        match self {
            NotchSize::Small => 0.8,
            NotchSize::Medium => 1.0,
            NotchSize::Large => 1.25,
        }
    }
}

/// `showMode`: when the open pill is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShowMode {
    /// Open pill always shown.
    Always,
    /// Folded rest pill until hovered (fresh-install default).
    #[default]
    Hover,
    /// No pill at all (tray icon forced on; reset cards still appear).
    Hidden,
}

/// `weeklyRing`: second ring placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeeklyRing {
    #[default]
    Off,
    Inside,
    Outside,
}

/// `colorTransition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorTransition {
    #[default]
    HardStep,
    Ramp,
}

/// `taskbarMode`: rings in the taskbar instead of the notch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskbarMode {
    #[default]
    Off,
    Left,
    Right,
}

/// `updateMode`: what CodeNotch does about new versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Check automatically and propose the update with a toast (fresh-install default).
    #[default]
    Notify,
    /// Check automatically, download in the background and install when idle.
    Auto,
    /// No automatic check (the settings button still checks on demand).
    Off,
}

/// Update manifest read when `updateManifestUrl` is empty.
pub const DEFAULT_UPDATE_MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/MakFly/codenotch-windows/main/update.json";

/// `antigravityLimit`: which lane the Antigravity ring reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AntigravityLimit {
    #[default]
    #[serde(rename = "automatic")]
    Automatic,
    #[serde(rename = "5h")]
    FiveHour,
    #[serde(rename = "weekly")]
    Weekly,
}

/// `antigravityModel`: which model family the Antigravity ring reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AntigravityModel {
    #[default]
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "3p")]
    ThirdParty,
}

// ---------------------------------------------------------------------------------------------
// WidgetPlacement
// ---------------------------------------------------------------------------------------------

/// `widgetPlacement`: monitor device name and position along the edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WidgetPlacement {
    /// Monitor device name (`\\.\DISPLAY2`). Empty = primary.
    #[serde(default)]
    pub monitor: String,
    /// Legacy (0.3 wrote 1.0). Not used for placement.
    #[serde(default = "default_x")]
    pub x: f64,
    /// Position of the notch centre along its edge, 0..1 (0.5 = centred).
    #[serde(default = "default_y")]
    pub y: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for WidgetPlacement {
    fn default() -> Self {
        Self {
            monitor: String::new(),
            x: default_x(),
            y: default_y(),
            extra: Map::new(),
        }
    }
}

impl WidgetPlacement {
    /// `y` clamped to 0..1.
    pub fn along(&self) -> f64 {
        if self.y.is_finite() {
            self.y.clamp(0.0, 1.0)
        } else {
            0.5
        }
    }
}

fn default_x() -> f64 {
    1.0
}
fn default_y() -> f64 {
    0.5
}
fn default_lang() -> String {
    "en".into()
}
fn default_poll() -> u64 {
    60
}
fn default_true() -> bool {
    true
}
fn default_alert() -> u8 {
    80
}
fn default_critical() -> u8 {
    95
}
fn default_forecast_min() -> u64 {
    30
}
fn default_history() -> u64 {
    24
}
fn default_analytics() -> u64 {
    7
}
fn default_fx() -> f64 {
    0.92
}
fn default_hook_port() -> u16 {
    48666
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

/// All user settings. Keys are camelCase in the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // ----- keys from 0.2/0.3 (names frozen) -----
    /// UI language code: "en", "fr" (anything else falls back to English; see [`Settings::lang`]).
    #[serde(default = "default_lang")]
    pub language: String,
    #[serde(default, deserialize_with = "lenient")]
    pub theme: ThemeMode,
    /// Automatic refresh period in seconds (see [`Settings::poll_interval`] for the clamp).
    #[serde(default = "default_poll")]
    pub poll_seconds: u64,
    /// Master switch for desktop notifications (threshold / forecast / reset alerts).
    #[serde(default = "default_true")]
    pub notifications: bool,
    /// Warning threshold in percent used (also the "watch" colour limit).
    #[serde(default = "default_alert")]
    pub alert_percent: u8,
    /// Legacy master "show the notch" switch. `false` = [`ShowMode::Hidden`] when `showMode` is absent.
    #[serde(default = "default_true")]
    pub widget: bool,
    /// Edge the notch is pinned to.
    #[serde(default, deserialize_with = "lenient")]
    pub widget_position: Edge,
    /// Legacy 0.3 "expand on hover". Kept for round trip; the UI uses [`Settings::show_mode`].
    #[serde(default = "default_true")]
    pub auto_hide: bool,
    /// Legacy 0.3 "hide in full screen". Kept for round trip; the UI uses `foldForFullScreen`.
    #[serde(default = "default_true")]
    pub hide_in_fullscreen: bool,
    /// Game mode: hide everything and stop non-critical alerts while on.
    #[serde(default)]
    pub game_mode: bool,
    #[serde(default)]
    pub widget_placement: WidgetPlacement,
    /// `None` = key absent: existing accounts then count as already onboarded.
    #[serde(default)]
    pub onboarding_done: Option<bool>,
    /// Critical threshold in percent used.
    #[serde(default = "default_critical")]
    pub critical_percent: u8,
    #[serde(default = "default_true")]
    pub forecast_alerts: bool,
    /// Forecast alert lead time: warn when the quota is projected to run out within N minutes.
    #[serde(default = "default_forecast_min")]
    pub forecast_minutes: u64,
    /// Desktop notification when a window resets.
    #[serde(default = "default_true")]
    pub reset_alerts: bool,
    /// Let critical alerts through while a full-screen app is in front.
    #[serde(default = "default_true")]
    pub critical_in_fullscreen: bool,
    #[serde(default)]
    pub telegram_enabled: bool,
    /// Telegram chat id. The bot token is NOT here (env var / credential store, see notify).
    #[serde(default)]
    pub telegram_chat_id: Private,
    #[serde(default)]
    pub discord_enabled: bool,
    #[serde(default)]
    pub launch_at_startup: bool,
    /// Tray tooltip shows the percentages.
    #[serde(default = "default_true")]
    pub tray_shows_usage: bool,
    /// Global hotkey, e.g. "Ctrl+Alt+N". `None` = no hotkey (serialised as `null`).
    #[serde(default)]
    pub hotkey: Option<String>,
    /// How many hours of history to keep in `history.jsonl`.
    #[serde(default = "default_history")]
    pub history_hours: u64,
    /// Analytics period in days.
    #[serde(default = "default_analytics")]
    pub analytics_days: u64,
    #[serde(default = "default_fx")]
    pub usd_to_eur: f64,
    /// Plan prices, EUR per month. Keys are written in ONE format, [`plan_price_key`]:
    /// `"<Provider display name> <plan>"` (`"Claude Max"`, `"Codex Plus"`) or the provider display
    /// name alone when the plan is unknown (`"Claude"`). Values: a number or a numeric string.
    /// Readers also accept (case-insensitive) an account id, an account name, a bare plan name
    /// and a provider id (`"claude"`), for hand-edited and older files. Preserved verbatim.
    #[serde(default)]
    pub plan_prices_eur: Map<String, Value>,

    // ----- keys added in 1.0 (all optional in the file) -----
    #[serde(default, deserialize_with = "lenient")]
    pub notch_size: NotchSize,
    /// `None` = key absent (config from 0.3): see [`Settings::show_mode`].
    #[serde(default, deserialize_with = "lenient")]
    pub show_mode: Option<ShowMode>,
    /// Fold the pill while a full-screen app is in front of its monitor.
    #[serde(default = "default_true")]
    pub fold_for_full_screen: bool,
    /// Rest pill flips dark/light from the backdrop luminance.
    #[serde(default)]
    pub adaptive_pill: bool,
    /// Legacy "notch always dark". Kept for round trip; ignored: the notch, its cards and the
    /// settings window all follow [`Settings::theme`].
    #[serde(default = "default_true")]
    pub notch_always_dark: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub weekly_ring: WeeklyRing,
    #[serde(default)]
    pub weekly_ring_dashed: bool,
    /// Show remaining instead of used (figure and sweep; colour still grades *used*).
    #[serde(default)]
    pub show_remaining: bool,
    /// Show the usage pace (deficit) figure.
    #[serde(default)]
    pub show_usage_pace: bool,
    /// Show the exhaustion forecast ("At this pace: runs out Thu 3:20 PM").
    #[serde(default = "default_true")]
    pub show_forecast: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub color_transition: ColorTransition,
    #[serde(default, deserialize_with = "lenient")]
    pub taskbar_mode: TaskbarMode,
    /// On-screen reset card when a window renews.
    #[serde(default = "default_true")]
    pub reset_cards: bool,
    /// Play the system sound with the reset card.
    #[serde(default = "default_true")]
    pub reset_sound: bool,
    /// Provider order on the notch/tray. Empty = [`ProviderKind::CANONICAL_ORDER`].
    #[serde(default)]
    pub provider_order: Vec<ProviderKind>,
    /// Account ids that are fetched but not drawn on the notch.
    #[serde(default)]
    pub hidden_accounts: Vec<String>,
    /// Discovered accounts the user removed: discovery does not add them back (until the
    /// marker is dropped from the settings window's "Removed accounts").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_accounts: Vec<RemovedAccount>,
    /// Claude Code hooks installed / wanted.
    #[serde(default)]
    pub hooks_enabled: bool,
    /// Local hook server port (127.0.0.1).
    #[serde(default = "default_hook_port")]
    pub hook_port: u16,
    /// Legacy (≤ 1.0) "check for updates" switch. Superseded by `updateMode` (see
    /// [`Settings::update_mode`]); kept in sync for older builds reading the file.
    #[serde(default = "default_true")]
    pub update_checks: bool,
    /// `None` = key absent (older file): derived from `updateChecks` (`false` → off).
    #[serde(default, deserialize_with = "lenient")]
    pub update_mode: Option<UpdateMode>,
    /// Update manifest URL (https). Empty = [`DEFAULT_UPDATE_MANIFEST_URL`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub update_manifest_url: String,
    #[serde(default, deserialize_with = "lenient")]
    pub antigravity_limit: AntigravityLimit,
    #[serde(default, deserialize_with = "lenient")]
    pub antigravity_model: AntigravityModel,
    /// Settings window: navigation sidebar collapsed to its icon rail.
    #[serde(default)]
    pub settings_sidebar_collapsed: bool,

    /// Unknown keys, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    /// Fresh-install defaults.
    fn default() -> Self {
        Self {
            language: default_lang(),
            theme: ThemeMode::default(),
            poll_seconds: default_poll(),
            notifications: true,
            alert_percent: default_alert(),
            widget: true,
            widget_position: Edge::default(),
            auto_hide: true,
            hide_in_fullscreen: true,
            game_mode: false,
            widget_placement: WidgetPlacement::default(),
            onboarding_done: None,
            critical_percent: default_critical(),
            forecast_alerts: true,
            forecast_minutes: default_forecast_min(),
            reset_alerts: true,
            critical_in_fullscreen: true,
            telegram_enabled: false,
            telegram_chat_id: Private::default(),
            discord_enabled: false,
            launch_at_startup: false,
            tray_shows_usage: true,
            hotkey: None,
            history_hours: default_history(),
            analytics_days: default_analytics(),
            usd_to_eur: default_fx(),
            plan_prices_eur: Map::new(),
            notch_size: NotchSize::default(),
            show_mode: Some(ShowMode::Hover),
            fold_for_full_screen: true,
            adaptive_pill: false,
            notch_always_dark: true,
            weekly_ring: WeeklyRing::default(),
            weekly_ring_dashed: false,
            show_remaining: false,
            show_usage_pace: false,
            show_forecast: true,
            color_transition: ColorTransition::default(),
            taskbar_mode: TaskbarMode::default(),
            reset_cards: true,
            reset_sound: true,
            provider_order: Vec::new(),
            hidden_accounts: Vec::new(),
            removed_accounts: Vec::new(),
            hooks_enabled: false,
            hook_port: default_hook_port(),
            update_checks: true,
            update_mode: Some(UpdateMode::Notify),
            update_manifest_url: String::new(),
            antigravity_limit: AntigravityLimit::default(),
            antigravity_model: AntigravityModel::default(),
            settings_sidebar_collapsed: false,
            extra: Map::new(),
        }
    }
}

/// A removed discovered account (`removedAccounts`): provider + profile folder (empty = the
/// provider's default folder), and the name it had, for the settings window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedAccount {
    pub provider: ProviderKind,
    #[serde(default)]
    pub profile_path: String,
    #[serde(default)]
    pub name: String,
}

impl RemovedAccount {
    /// The marker for `account` (`None` for a custom-endpoint account: never discovered).
    pub fn of(account: &Account) -> Option<Self> {
        (!account.uses_custom_endpoint()).then(|| Self {
            provider: account.provider.clone(),
            profile_path: account.profile_path.clone(),
            name: account.name.clone(),
        })
    }

    /// Same provider and same profile folder text (case, slashes and a trailing separator
    /// ignored). The engine also resolves an empty folder to the default one.
    pub fn same_profile(&self, account: &Account) -> bool {
        let norm = |p: &str| {
            p.trim()
                .replace('/', "\\")
                .trim_end_matches('\\')
                .to_lowercase()
        };
        self.provider == account.provider
            && !account.uses_custom_endpoint()
            && norm(&self.profile_path) == norm(&account.profile_path)
    }
}

/// The one key format of `planPricesEur` (see [`Settings::plan_prices_eur`]):
/// `"Claude Max"` for a known plan, `"Claude"` when the plan is unknown.
pub fn plan_price_key(provider: &ProviderKind, plan: Option<&str>) -> String {
    match plan.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!("{} {p}", provider.display_name()),
        None => provider.display_name().to_string(),
    }
}

/// Date of the public list prices in [`LIST_PRICES_USD`] (check them again after it).
pub const LIST_PRICES_AS_OF: &str = "2026-06";

/// Built-in monthly list prices (USD, individual monthly billing, before tax) of well-known
/// plans, used to compare with the API-equivalent cost when the user has not entered a price
/// for that plan in `planPricesEur` (a price the user typed always wins; `0` turns the
/// comparison off for that plan). Converted to euros with `usdToEur`.
///
/// `(provider, plan names as the providers report them (see [`normalise_plan`]), USD / month)`.
/// Only plans with a single, public, per-seat price; team / business / enterprise plans and
/// plans whose tier is unknown ("Max" without 5x / 20x) have none.
pub const LIST_PRICES_USD: &[(ProviderKind, &[&str], f64)] = &[
    (ProviderKind::Claude, &["pro"], 20.0),
    (ProviderKind::Claude, &["max 5x"], 100.0),
    (ProviderKind::Claude, &["max 20x"], 200.0),
    // Codex rides on the ChatGPT plan.
    (ProviderKind::Codex, &["plus"], 20.0),
    (ProviderKind::Codex, &["pro"], 200.0),
    (ProviderKind::Cursor, &["pro"], 20.0),
    (ProviderKind::Cursor, &["pro+", "pro plus"], 60.0),
    (ProviderKind::Cursor, &["ultra"], 200.0),
    // GitHub's API still calls Copilot Pro "individual".
    (ProviderKind::Copilot, &["pro", "individual"], 10.0),
    (ProviderKind::Copilot, &["pro+", "pro plus"], 39.0),
    (ProviderKind::Grok, &["supergrok"], 30.0),
    (ProviderKind::Grok, &["supergrok heavy"], 300.0),
];

/// Plan name for matching: lower case, `_` / `-` / runs of spaces → one space, a leading
/// provider name dropped ("Claude Max 20x" → "max 20x", "Pro_Plus" → "pro plus").
pub fn normalise_plan(provider: &ProviderKind, plan: &str) -> String {
    let lower = plan.to_lowercase().replace(['_', '-'], " ");
    let mut words: Vec<&str> = lower.split_whitespace().collect();
    let name = provider.display_name().to_lowercase();
    let prefix: Vec<&str> = name.split_whitespace().collect();
    if words.len() > prefix.len() && words[..prefix.len()] == prefix[..] {
        words.drain(..prefix.len());
    }
    words.join(" ")
}

/// Built-in monthly list price (USD) of a plan, `None` when unknown (see [`LIST_PRICES_USD`]).
pub fn list_price_usd(provider: &ProviderKind, plan: Option<&str>) -> Option<f64> {
    let plan = normalise_plan(provider, plan?);
    LIST_PRICES_USD
        .iter()
        .find(|(p, names, _)| p == provider && names.contains(&plan.as_str()))
        .map(|(_, _, usd)| *usd)
}

impl Settings {
    /// UI language. `"auto"`, `"system"` or empty → [`crate::i18n::system_lang`] (English
    /// unless the app called [`crate::i18n::set_system_lang`] with the OS language).
    pub fn lang(&self) -> crate::i18n::Lang {
        match self.language.trim().to_ascii_lowercase().as_str() {
            "" | "auto" | "system" => crate::i18n::system_lang(),
            code => crate::i18n::Lang::from_code(code),
        }
    }

    /// Effective show mode. When `showMode` is absent (0.3 file): `widget == false` → Hidden,
    /// otherwise Hover (the fresh-install default).
    pub fn show_mode(&self) -> ShowMode {
        match self.show_mode {
            Some(mode) => mode,
            None if !self.widget => ShowMode::Hidden,
            None => ShowMode::Hover,
        }
    }

    /// Effective update mode. When `updateMode` is absent (file from ≤ 1.0): `updateChecks ==
    /// false` → Off, otherwise Notify.
    pub fn update_mode(&self) -> UpdateMode {
        match self.update_mode {
            Some(mode) => mode,
            None if !self.update_checks => UpdateMode::Off,
            None => UpdateMode::Notify,
        }
    }

    /// Set the update mode (and the legacy `updateChecks` switch to match).
    pub fn set_update_mode(&mut self, mode: UpdateMode) {
        self.update_mode = Some(mode);
        self.update_checks = mode != UpdateMode::Off;
    }

    /// Manifest URL to read: `updateManifestUrl` when set, else the default.
    pub fn update_manifest_url(&self) -> &str {
        match self.update_manifest_url.trim() {
            "" => DEFAULT_UPDATE_MANIFEST_URL,
            url => url,
        }
    }

    /// Automatic refresh period, clamped to 30..=300 s.
    pub fn poll_interval(&self) -> u64 {
        self.poll_seconds.clamp(30, 300)
    }

    /// Warning ("watch") threshold in percent, clamped to 1..=99.
    pub fn warn_percent(&self) -> f64 {
        f64::from(self.alert_percent).clamp(1.0, 99.0)
    }

    /// Critical threshold in percent, clamped to (warn+1)..=100.
    pub fn critical_percent(&self) -> f64 {
        f64::from(self.critical_percent).clamp(self.warn_percent() + 1.0, 100.0)
    }

    /// Provider order to use (the configured one, completed with the canonical order).
    pub fn effective_provider_order(&self) -> Vec<ProviderKind> {
        let mut order: Vec<ProviderKind> = Vec::new();
        for kind in self
            .provider_order
            .iter()
            .chain(ProviderKind::CANONICAL_ORDER.iter())
        {
            if !order.contains(kind) {
                order.push(kind.clone());
            }
        }
        order
    }
}

// ---------------------------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------------------------

/// The whole `config.gpui.json` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub settings: Settings,
    /// Unknown top-level keys, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Config {
    /// Parse JSON text (a UTF-8 BOM is ignored).
    pub fn parse(text: &str) -> Result<Config> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        Ok(serde_json::from_str(text)?)
    }

    /// Serialise to pretty JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Load from `path`. A missing file gives `Ok(Config::default())`.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(err) => Err(Error::io(path.display().to_string(), err)),
        }
    }

    /// Load, never failing: on error returns defaults plus a user-facing message. When the file
    /// exists but cannot be parsed it is first **copied** to `<name>.unreadable.json` (the
    /// original stays in place until the next save).
    pub fn load_or_default(path: &Path) -> (Config, Option<String>) {
        match Config::load(path) {
            Ok(config) => (config, None),
            Err(err) => {
                if matches!(err, Error::Json(_)) {
                    let backup = path.with_extension("unreadable.json");
                    let _ = std::fs::copy(path, backup);
                }
                (
                    Config::default(),
                    Some(format!("{}: {err}", path.display())),
                )
            }
        }
    }

    /// Save atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = self.to_json()?;
        crate::paths::write_atomic(path, text.as_bytes())
            .map_err(|err| Error::io(path.display().to_string(), err))
    }

    /// Welcome screen only when nothing is connected and onboarding was never finished.
    pub fn needs_welcome(&self) -> bool {
        !self
            .settings
            .onboarding_done
            .unwrap_or(!self.accounts.is_empty())
    }

    /// Resolve `onboardingDone` (call before the first save).
    pub fn mark_onboarded(&mut self) {
        self.settings.onboarding_done = Some(true);
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub fn account_mut(&mut self, id: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// Enabled accounts in file order.
    pub fn enabled_accounts(&self) -> impl Iterator<Item = &Account> {
        self.accounts.iter().filter(|a| a.enabled)
    }

    /// Enabled accounts sorted by the effective provider order (stable within a provider).
    pub fn ordered_accounts(&self) -> Vec<&Account> {
        let order = self.settings.effective_provider_order();
        let rank = |kind: &ProviderKind| order.iter().position(|k| k == kind).unwrap_or(usize::MAX);
        let mut list: Vec<&Account> = self.enabled_accounts().collect();
        list.sort_by_key(|a| rank(&a.provider));
        list
    }

    /// Enabled accounts drawn on the notch: ordered, minus `hiddenAccounts`.
    pub fn notch_accounts(&self) -> Vec<&Account> {
        self.ordered_accounts()
            .into_iter()
            .filter(|a| !self.settings.hidden_accounts.contains(&a.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_prices_match_the_plan_names_providers_report() {
        let usd = |p: ProviderKind, plan: &str| list_price_usd(&p, Some(plan));
        assert_eq!(usd(ProviderKind::Claude, "Max 20x"), Some(200.0));
        assert_eq!(usd(ProviderKind::Claude, "Max 5x"), Some(100.0));
        assert_eq!(usd(ProviderKind::Claude, "Pro"), Some(20.0));
        assert_eq!(usd(ProviderKind::Claude, "Max"), None, "unknown tier");
        assert_eq!(usd(ProviderKind::Claude, "Team"), None);
        assert_eq!(usd(ProviderKind::Codex, "Plus"), Some(20.0));
        assert_eq!(usd(ProviderKind::Codex, "pro"), Some(200.0));
        assert_eq!(usd(ProviderKind::Cursor, "Ultra"), Some(200.0));
        assert_eq!(usd(ProviderKind::Cursor, "Pro plus"), Some(60.0));
        assert_eq!(usd(ProviderKind::Cursor, "pro_plus"), Some(60.0));
        assert_eq!(usd(ProviderKind::Copilot, "Pro"), Some(10.0));
        assert_eq!(usd(ProviderKind::Copilot, "Pro+"), Some(39.0));
        assert_eq!(usd(ProviderKind::Grok, "SuperGrok Heavy"), Some(300.0));
        assert_eq!(usd(ProviderKind::Grok, "SuperGrok"), Some(30.0));
        // Provider prefix (a plan key) and spacing do not matter; another provider's plan does.
        assert_eq!(usd(ProviderKind::Claude, "Claude  Max_20x"), Some(200.0));
        assert_eq!(usd(ProviderKind::Copilot, "GitHub Copilot Pro"), Some(10.0));
        assert_eq!(usd(ProviderKind::Glm, "Pro"), None);
        assert_eq!(list_price_usd(&ProviderKind::Claude, None), None);
    }

    #[test]
    fn plan_price_key_format_and_auto_language() {
        assert_eq!(
            plan_price_key(&ProviderKind::Claude, Some("Max")),
            "Claude Max"
        );
        assert_eq!(plan_price_key(&ProviderKind::Codex, Some("  ")), "Codex");
        assert_eq!(plan_price_key(&ProviderKind::Codex, None), "Codex");
        let mut s = Settings {
            language: "fr-FR".into(),
            ..Default::default()
        };
        assert_eq!(s.lang(), crate::i18n::Lang::Fr);
        s.language = "auto".into();
        assert_eq!(s.lang(), crate::i18n::system_lang());
    }

    /// Synthetic fixture with exactly the key set of the owner's real 0.3 file (values invented).
    pub(crate) const OWNER_SHAPE: &str = r#"{
      "accounts": [
        {"id": "acc-1", "name": "Claude · perso", "provider": "claude", "enabled": true,
         "profilePath": "C:\\Users\\someone\\.claude", "endpoint": "", "secretEnv": "", "hasSecret": false},
        {"id": "acc-2", "name": "Codex", "provider": "codex", "enabled": true,
         "profilePath": "", "endpoint": "", "secretEnv": "", "hasSecret": false},
        {"id": "acc-3", "name": "Grok", "provider": "grok", "enabled": false,
         "profilePath": "", "endpoint": "", "secretEnv": "", "hasSecret": false}
      ],
      "settings": {
        "language": "fr",
        "theme": "dark",
        "pollSeconds": 60,
        "notifications": true,
        "alertPercent": 80,
        "widget": true,
        "widgetPosition": "right",
        "autoHide": true,
        "hideInFullscreen": true,
        "gameMode": false,
        "widgetPlacement": { "monitor": "\\\\.\\DISPLAY1", "x": 1.0, "y": 0.44 },
        "onboardingDone": true,
        "criticalPercent": 95,
        "forecastAlerts": true,
        "forecastMinutes": 30,
        "resetAlerts": true,
        "criticalInFullscreen": true,
        "telegramEnabled": false,
        "telegramChatId": "123456",
        "discordEnabled": false,
        "launchAtStartup": false,
        "trayShowsUsage": true,
        "hotkey": null,
        "historyHours": 24,
        "analyticsDays": 7,
        "usdToEur": 0.92,
        "planPricesEur": {}
      }
    }"#;

    #[test]
    fn owner_shape_round_trips_every_key_unchanged() {
        let config = Config::parse(OWNER_SHAPE).unwrap();
        assert_eq!(config.settings.lang(), crate::i18n::Lang::Fr);
        assert_eq!(config.settings.theme, ThemeMode::Dark);
        assert_eq!(config.settings.widget_position, Edge::Right);
        assert_eq!(config.settings.show_mode(), ShowMode::Hover);
        assert_eq!(config.accounts.len(), 3);
        assert_eq!(config.accounts[0].provider, ProviderKind::Claude);
        assert_eq!(config.accounts[0].profile_path, r"C:\Users\someone\.claude");
        assert!(!config.needs_welcome());

        let original: Value = serde_json::from_str(OWNER_SHAPE).unwrap();
        let again = serde_json::to_value(&config).unwrap();
        // Every original key is present with the same value.
        for (key, value) in original["settings"].as_object().unwrap() {
            assert_eq!(&again["settings"][key], value, "settings.{key}");
        }
        for (i, account) in original["accounts"].as_array().unwrap().iter().enumerate() {
            for (key, value) in account.as_object().unwrap() {
                assert_eq!(&again["accounts"][i][key], value, "accounts[{i}].{key}");
            }
        }
        assert!(again["settings"].get("hotkey").unwrap().is_null());
        // Saved → reloaded is identical.
        let reloaded = Config::parse(&config.to_json().unwrap()).unwrap();
        assert_eq!(reloaded, config);
    }

    #[test]
    fn unknown_keys_survive_round_trip() {
        let raw = r#"{"accounts":[{"id":"a","name":"A","provider":"claude","enabled":true,"futureAcc":1}],
                      "settings":{"futureFlag":{"deep":[1,2]},"widgetPlacement":{"y":0.3,"z":9}},
                      "topLevelThing":"keep"}"#;
        let config = Config::parse(raw).unwrap();
        let out = serde_json::to_value(&config).unwrap();
        assert_eq!(out["topLevelThing"], "keep");
        assert_eq!(out["settings"]["futureFlag"]["deep"][1], 2);
        assert_eq!(out["settings"]["widgetPlacement"]["z"], 9);
        assert_eq!(out["accounts"][0]["futureAcc"], 1);
    }

    #[test]
    fn unknown_provider_string_is_preserved() {
        let raw = r#"{"accounts":[{"id":"a","name":"D","provider":"deepseek","enabled":true}]}"#;
        let config = Config::parse(raw).unwrap();
        assert_eq!(
            config.accounts[0].provider,
            ProviderKind::Other("deepseek".into())
        );
        let out = serde_json::to_value(&config).unwrap();
        assert_eq!(out["accounts"][0]["provider"], "deepseek");
    }

    #[test]
    fn bad_enum_values_fall_back_instead_of_failing() {
        let raw = r#"{"settings":{"theme":"purple","widgetPosition":42,"weeklyRing":"diagonal","notchSize":"large"}}"#;
        let config = Config::parse(raw).unwrap();
        assert_eq!(config.settings.theme, ThemeMode::System);
        assert_eq!(config.settings.widget_position, Edge::Right);
        assert_eq!(config.settings.weekly_ring, WeeklyRing::Off);
        assert_eq!(config.settings.notch_size, NotchSize::Large);
    }

    #[test]
    fn new_settings_have_defaults_and_serialise_camel_case() {
        let config = Config::parse(r#"{"settings":{}}"#).unwrap();
        assert!(config.needs_welcome());
        assert_eq!(config.settings.hook_port, 48666);
        assert!(config.settings.fold_for_full_screen);
        assert_eq!(config.settings.show_mode, None);
        let out = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(out["settings"]["showMode"], "hover");
        assert_eq!(out["settings"]["colorTransition"], "hard_step");
        assert_eq!(out["settings"]["antigravityLimit"], "automatic");
        assert_eq!(out["settings"]["notchSize"], "medium");
    }

    #[test]
    fn legacy_widget_false_means_hidden() {
        let config = Config::parse(r#"{"settings":{"widget":false}}"#).unwrap();
        assert_eq!(config.settings.show_mode(), ShowMode::Hidden);
    }

    #[test]
    fn legacy_file_without_show_mode_means_hover() {
        let config = Config::parse(r#"{"settings":{"widget":true}}"#).unwrap();
        assert_eq!(config.settings.show_mode, None);
        assert_eq!(config.settings.show_mode(), ShowMode::Hover);
    }

    #[test]
    fn update_mode_migrates_from_update_checks() {
        let s = |raw: &str| Config::parse(raw).unwrap().settings;
        // Older files: no updateMode key.
        assert_eq!(s(r#"{"settings":{}}"#).update_mode(), UpdateMode::Notify);
        assert_eq!(
            s(r#"{"settings":{"updateChecks":true}}"#).update_mode(),
            UpdateMode::Notify
        );
        assert_eq!(
            s(r#"{"settings":{"updateChecks":false}}"#).update_mode(),
            UpdateMode::Off
        );
        // The new key wins; a bad value falls back to the legacy switch.
        assert_eq!(
            s(r#"{"settings":{"updateChecks":false,"updateMode":"auto"}}"#).update_mode(),
            UpdateMode::Auto
        );
        assert_eq!(
            s(r#"{"settings":{"updateChecks":false,"updateMode":"sometimes"}}"#).update_mode(),
            UpdateMode::Off
        );
        // Fresh install, and setting a mode keeps the legacy switch in sync.
        let mut fresh = Settings::default();
        assert_eq!(fresh.update_mode(), UpdateMode::Notify);
        fresh.set_update_mode(UpdateMode::Off);
        assert!(!fresh.update_checks);
        fresh.set_update_mode(UpdateMode::Auto);
        assert!(fresh.update_checks);
        let out = serde_json::to_value(&fresh).unwrap();
        assert_eq!(out["updateMode"], "auto");
        assert!(out.get("updateManifestUrl").is_none(), "empty URL not written");
        // Manifest URL: default unless set.
        assert_eq!(fresh.update_manifest_url(), DEFAULT_UPDATE_MANIFEST_URL);
        fresh.update_manifest_url = " https://example.com/u.json ".into();
        assert_eq!(fresh.update_manifest_url(), "https://example.com/u.json");
    }

    #[test]
    fn thresholds_are_clamped() {
        let mut s = Settings {
            alert_percent: 0,
            critical_percent: 0,
            ..Settings::default()
        };
        assert_eq!(s.warn_percent(), 1.0);
        assert_eq!(s.critical_percent(), 2.0);
        s.alert_percent = 100;
        s.critical_percent = 250;
        assert_eq!(s.warn_percent(), 99.0);
        assert_eq!(s.critical_percent(), 100.0);
    }

    #[test]
    fn chat_id_is_not_in_debug_output() {
        let config = Config::parse(OWNER_SHAPE).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("123456"));
        assert_eq!(config.settings.telegram_chat_id.as_str(), "123456");
    }

    #[test]
    fn save_and_load_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.gpui.json");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let mut config = Config::parse(OWNER_SHAPE).unwrap();
        config.settings.weekly_ring = WeeklyRing::Outside;
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[test]
    fn bom_is_ignored_and_garbage_is_reported() {
        assert!(Config::parse("\u{feff}{}").is_ok());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.gpui.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (config, err) = Config::load_or_default(&path);
        assert_eq!(config, Config::default());
        assert!(err.is_some());
        assert!(dir.path().join("config.gpui.unreadable.json").exists());
    }

    #[test]
    fn ordered_accounts_follow_provider_order() {
        let mut config = Config::default();
        config.accounts.push(Account::new(ProviderKind::Grok, "g"));
        config
            .accounts
            .push(Account::new(ProviderKind::Claude, "c"));
        config.accounts.push(Account::new(ProviderKind::Codex, "x"));
        let names: Vec<_> = config
            .ordered_accounts()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        assert_eq!(names, ["c", "x", "g"]);
        config.settings.provider_order = vec![ProviderKind::Grok];
        let names: Vec<_> = config
            .ordered_accounts()
            .iter()
            .map(|a| a.name.clone())
            .collect();
        assert_eq!(names, ["g", "c", "x"]);
        let hidden = config.accounts[0].id.clone();
        config.settings.hidden_accounts.push(hidden);
        assert_eq!(config.notch_accounts().len(), 2);
    }

    #[test]
    fn new_account_ids_are_unique() {
        let ids: std::collections::HashSet<_> = (0..200)
            .map(|_| Account::new(ProviderKind::Claude, "c").id)
            .collect();
        assert_eq!(ids.len(), 200);
    }

    /// Read-only check against the owner's real file, when present. Run explicitly with
    /// `cargo test -p codenotch-core -- --ignored real_config`. Prints no values.
    #[test]
    #[ignore]
    fn real_config_round_trips_if_present() {
        let path = crate::paths::Paths::resolve().config();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let original: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        let config = Config::parse(&text).expect("real config must parse");
        let again = serde_json::to_value(&config).unwrap();
        fn check(orig: &Value, new: &Value, at: &str) {
            match orig {
                Value::Object(map) => {
                    for (k, v) in map {
                        let n = new.get(k).unwrap_or_else(|| panic!("key lost: {at}.{k}"));
                        check(v, n, &format!("{at}.{k}"));
                    }
                }
                Value::Array(items) => {
                    for (i, v) in items.iter().enumerate() {
                        check(v, &new[i], &format!("{at}[{i}]"));
                    }
                }
                other => assert!(other == new, "value changed at {at}"),
            }
        }
        check(&original, &again, "");
    }
}
