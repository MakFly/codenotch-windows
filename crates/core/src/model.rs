//! Plain data shared by engine, providers and UI.
//!
//! Units: percentages are **0–100 used** (`f64`), times are **unix seconds** (`i64`).
//! An unknown value is `None` and is never replaced by 0.

use crate::config::{Account, Config, Settings};
use crate::secrets::{SecretKey, SecretValue, SecretsPresent};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;

/// A reading older than this is drawn as stale (15 min).
pub const STALE_AFTER_SECS: i64 = 15 * 60;

// ---------------------------------------------------------------------------------------------
// ProviderKind
// ---------------------------------------------------------------------------------------------

/// Which service an account belongs to. Serialised as its lowercase id; unknown ids are kept
/// in [`ProviderKind::Other`] and written back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum ProviderKind {
    #[default]
    Claude,
    Codex,
    Glm,
    OpenCode,
    CommandCode,
    Cursor,
    Grok,
    Copilot,
    Antigravity,
    Qoder,
    /// User-defined JSON endpoint (see `codenotch-providers` custom provider).
    Custom,
    /// Unknown id from a newer/older file. Never fetched.
    Other(String),
}

impl ProviderKind {
    /// Notch, tray and settings order (minus DeepSeek).
    pub const CANONICAL_ORDER: &'static [ProviderKind] = &[
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Glm,
        ProviderKind::OpenCode,
        ProviderKind::CommandCode,
        ProviderKind::Cursor,
        ProviderKind::Grok,
        ProviderKind::Copilot,
        ProviderKind::Antigravity,
        ProviderKind::Qoder,
        ProviderKind::Custom,
    ];

    /// Parse an id (case-insensitive). `gemini` is accepted for Antigravity, `zai`/`z.ai` for GLM.
    pub fn from_id(id: &str) -> Self {
        match id.trim().to_ascii_lowercase().as_str() {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "glm" | "zai" | "z.ai" => Self::Glm,
            "opencode" => Self::OpenCode,
            "commandcode" => Self::CommandCode,
            "cursor" => Self::Cursor,
            "grok" => Self::Grok,
            "copilot" => Self::Copilot,
            "antigravity" | "gemini" => Self::Antigravity,
            "qoder" => Self::Qoder,
            "custom" => Self::Custom,
            _ => Self::Other(id.to_string()),
        }
    }

    /// Stable lowercase id (what the config file stores).
    pub fn id(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Glm => "glm",
            Self::OpenCode => "opencode",
            Self::CommandCode => "commandcode",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Copilot => "copilot",
            Self::Antigravity => "antigravity",
            Self::Qoder => "qoder",
            Self::Custom => "custom",
            Self::Other(id) => id,
        }
    }

    /// Product name (not translated).
    pub fn display_name(&self) -> &str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Glm => "z.ai",
            Self::OpenCode => "OpenCode",
            Self::CommandCode => "Command Code",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
            Self::Copilot => "GitHub Copilot",
            Self::Antigravity => "Antigravity",
            Self::Qoder => "Qoder",
            Self::Custom => "Custom",
            Self::Other(id) => id,
        }
    }

    /// Web page with the provider's own usage view ("Open …" menu item).
    pub fn usage_url(&self) -> Option<&'static str> {
        Some(match self {
            Self::Claude => "https://claude.ai/settings/usage",
            Self::Codex => "https://chatgpt.com/#settings/Account",
            Self::Glm => "https://z.ai/manage-apikey/apikey-list",
            Self::OpenCode => "https://opencode.ai",
            Self::CommandCode => "https://commandcode.ai",
            Self::Cursor => "https://cursor.com/dashboard",
            Self::Grok => "https://grok.com/?_s=usage",
            Self::Copilot => "https://github.com/settings/copilot",
            Self::Antigravity => "https://antigravity.google",
            Self::Qoder => "https://qoder.com/account/integrations",
            Self::Custom | Self::Other(_) => return None,
        })
    }

    /// Glyph asset id (`<id>.svg` in ui-kit assets or the user glyph folder).
    pub fn glyph_id(&self) -> &str {
        match self {
            Self::Other(_) => "custom",
            other => other.id(),
        }
    }

    /// Fallback letters drawn in the ring when no glyph is available.
    pub fn glyph_letters(&self) -> &'static str {
        match self {
            Self::Claude => "C",
            Self::Codex => "Cx",
            Self::Glm => "Z.ai",
            Self::OpenCode => "OC",
            Self::CommandCode => "CC",
            Self::Cursor => "Cu",
            Self::Grok => "Gk",
            Self::Copilot => "Cp",
            Self::Antigravity => "Ag",
            Self::Qoder => "Q",
            Self::Custom | Self::Other(_) => "•",
        }
    }

    /// Declared headline window ids, in preference order.
    pub fn headline_candidates(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["session", "spend"],
            Self::Codex => &["primary"],
            Self::Cursor => &["included", "api"],
            Self::Grok => &["credits"],
            Self::Copilot => &["premium_interactions"],
            Self::Glm => &["session"],
            Self::OpenCode => &["rolling"],
            Self::CommandCode => &["monthly"],
            Self::Qoder => &["quota"],
            // Antigravity: the provider sets `Reading::headline_window` itself (lane picker).
            Self::Antigravity | Self::Custom | Self::Other(_) => &[],
        }
    }

    /// When no declared headline id is present, may the first window be used?
    /// (Grok, Copilot and custom endpoints: yes. Others: no figure.)
    pub fn headline_falls_back_to_first(&self) -> bool {
        matches!(
            self,
            Self::Grok | Self::Copilot | Self::Custom | Self::Other(_) | Self::Antigravity
        )
    }

    /// When no declared headline id is present, may the most-used weekly window be the
    /// headline? (Claude: yes, so a reading without a 5 h session still shows a figure; the
    /// weekly ring then stands down.)
    pub fn headline_falls_back_to_worst_weekly(&self) -> bool {
        matches!(self, Self::Claude)
    }

    /// Declared weekly window ids, in preference order.
    pub fn weekly_candidates(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["seven_day", "weekly_all", "weekly"],
            Self::Codex => &["secondary"],
            Self::Glm | Self::OpenCode | Self::CommandCode => &["weekly"],
            _ => &[],
        }
    }

    /// English i18n key of the "how to sign in" hint shown when signed out.
    pub fn sign_in_hint(&self) -> &'static str {
        match self {
            Self::Claude => "Sign in to Claude Code to see usage.",
            Self::Codex => "Sign in to Codex to see usage.",
            Self::Cursor => "Sign in to Cursor to see usage.",
            Self::Grok => "Run grok login to see usage.",
            Self::Copilot => "Run gh auth login to see usage.",
            Self::OpenCode => "Run opencode auth login to see usage.",
            Self::CommandCode => "Sign in with the Command Code app to read your usage.",
            Self::Antigravity => "Sign in to Antigravity to see usage.",
            Self::Qoder => "Sign in to qodercli to see usage.",
            Self::Glm => "Add your z.ai API key to see usage.",
            Self::Custom | Self::Other(_) => "Check the endpoint settings.",
        }
    }
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

impl Serialize for ProviderKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for ProviderKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let id = String::deserialize(d)?;
        Ok(ProviderKind::from_id(&id))
    }
}

// ---------------------------------------------------------------------------------------------
// LimitWindow
// ---------------------------------------------------------------------------------------------

/// What a window measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    /// Rolling short window (Claude/Codex 5 h session).
    Session,
    Weekly,
    Monthly,
    #[default]
    Other,
}

/// One quota window reported by a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    /// Stable id per provider (`session`, `primary`, `seven_day`, `credits` …).
    pub id: String,
    /// Canonical **English** label (an i18n key, e.g. "Current session"); localised at display.
    pub label_key: String,
    #[serde(default)]
    pub kind: WindowKind,
    /// Percent **used**, 0–100. `None` = unknown (never substitute 0).
    #[serde(default)]
    pub used_percent: Option<f64>,
    /// Reset time, unix seconds. `None` = unknown.
    #[serde(default)]
    pub resets_at: Option<i64>,
    /// Full window length in seconds (for the pace figure).
    #[serde(default)]
    pub window_seconds: Option<i64>,
    /// Extra line for the card ("$12.40 of $50", "312 / 500 requests").
    #[serde(default)]
    pub detail: Option<String>,
    /// Count-only window (no denominator): the ring draws only its track, the cell shows `~N`.
    #[serde(default)]
    pub count: Option<i64>,
    /// Figure computed by CodeNotch rather than reported by the vendor: shown with a `~` prefix.
    #[serde(default)]
    pub derived: bool,
    /// Heading on the card (model family, Codex "Spark", Claude account …). Grouped windows
    /// are ignored by reset detection.
    #[serde(default)]
    pub group: Option<String>,
    /// Exhaustion forecast for the current period, filled by the engine (never by providers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forecast: Option<crate::forecast::Forecast>,
}

impl LimitWindow {
    pub fn new(id: impl Into<String>, label_key: impl Into<String>, kind: WindowKind) -> Self {
        Self {
            id: id.into(),
            label_key: label_key.into(),
            kind,
            ..Default::default()
        }
    }
    /// Builder: set percent used (clamped 0–100; NaN → None).
    pub fn used(mut self, percent: f64) -> Self {
        self.used_percent = clamp_percent(percent);
        self
    }
    pub fn resets_at(mut self, at: i64) -> Self {
        self.resets_at = Some(at);
        self
    }
    pub fn window_seconds(mut self, secs: i64) -> Self {
        self.window_seconds = Some(secs);
        self
    }
    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = Some(text.into());
        self
    }
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
    pub fn derived(mut self) -> Self {
        self.derived = true;
        self
    }

    /// Used as a 0–1 fraction.
    pub fn fraction(&self) -> Option<f64> {
        self.used_percent.map(|p| (p / 100.0).clamp(0.0, 1.0))
    }
    /// 100 − used.
    pub fn remaining_percent(&self) -> Option<f64> {
        self.used_percent.map(|p| (100.0 - p).clamp(0.0, 100.0))
    }
    /// The reset time is known and in the past.
    pub fn reset_passed(&self, now: i64) -> bool {
        matches!(self.resets_at, Some(at) if at <= now)
    }
    /// Usage pace deficit in points: used − elapsed share of the window, when positive.
    pub fn pace_deficit(&self, now: i64) -> Option<f64> {
        let used = self.used_percent?;
        let (reset, len) = (self.resets_at?, self.window_seconds?);
        if len <= 0 {
            return None;
        }
        let elapsed = (len - (reset - now)).clamp(0, len) as f64 / len as f64 * 100.0;
        let deficit = used - elapsed;
        (deficit > 0.0).then_some(deficit)
    }
}

/// Clamp a percentage to 0–100; non-finite → None.
pub fn clamp_percent(p: f64) -> Option<f64> {
    p.is_finite().then(|| p.clamp(0.0, 100.0))
}

/// Accept either a 0–1 fraction or a 0–100 percentage (values ≤ 1 are fractions).
pub fn percent_from_fraction_or_percent(v: f64) -> Option<f64> {
    if !v.is_finite() {
        return None;
    }
    clamp_percent(if (0.0..=1.0).contains(&v) {
        v * 100.0
    } else {
        v
    })
}

// ---------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------

/// Outcome of one fetch.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReadingStatus {
    /// Live numbers.
    #[default]
    Fresh,
    /// Numbers kept from an earlier fetch (persisted, or the source is lagging).
    Stale,
    /// Provider installed but not signed in / token expired.
    SignedOut,
    /// Provider not found on this machine (no cell is shown).
    NotInstalled,
    /// Signed in, but nothing metered to report.
    Empty,
    /// HTTP 429; `until` = when to try again (unix secs) if known.
    RateLimited { until: Option<i64> },
    /// Anything else; see `Reading::error`.
    Error,
}

/// One provider reading for one account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub account_id: String,
    pub provider: ProviderKind,
    /// When the numbers shown were taken (unix secs). For a carried-over reading this is the
    /// original time, not the time of the failed attempt.
    pub fetched_at: i64,
    #[serde(default)]
    pub status: ReadingStatus,
    #[serde(default)]
    pub windows: Vec<LimitWindow>,
    /// Signed-in identity (e-mail / login). Shown on the card. Not a secret.
    #[serde(default)]
    pub identity: Option<String>,
    /// Plan name ("Max", "Pro", "Plus" …).
    #[serde(default)]
    pub plan: Option<String>,
    /// One line for the user (source, plan, hint). English or already localised.
    #[serde(default)]
    pub note: Option<String>,
    /// Short technical error (no secrets) when `status` is Error/RateLimited.
    #[serde(default)]
    pub error: Option<String>,
    /// Explicit headline window id. `None` = use [`ProviderKind::headline_candidates`].
    #[serde(default)]
    pub headline_window: Option<String>,
    /// Explicit weekly window id. `None` = use [`ProviderKind::weekly_candidates`].
    #[serde(default)]
    pub weekly_window: Option<String>,
}

impl Reading {
    /// Empty Fresh reading for `account` at `now`.
    pub fn new(account: &Account, now: i64) -> Self {
        Self {
            account_id: account.id.clone(),
            provider: account.provider.clone(),
            fetched_at: now,
            status: ReadingStatus::Fresh,
            windows: Vec::new(),
            identity: None,
            plan: None,
            note: None,
            error: None,
            headline_window: None,
            weekly_window: None,
        }
    }
    /// Fresh reading with windows.
    pub fn fresh(account: &Account, now: i64, windows: Vec<LimitWindow>) -> Self {
        Self {
            windows,
            ..Self::new(account, now)
        }
    }
    /// Reading with a status and a note, no windows.
    pub fn with_status(
        account: &Account,
        now: i64,
        status: ReadingStatus,
        note: impl Into<String>,
    ) -> Self {
        Self {
            status,
            note: Some(note.into()),
            ..Self::new(account, now)
        }
    }
    /// Error reading. `message` must be short and secret-free.
    pub fn error(account: &Account, now: i64, message: impl Into<String>) -> Self {
        Self {
            status: ReadingStatus::Error,
            error: Some(message.into()),
            ..Self::new(account, now)
        }
    }
    pub fn signed_out(account: &Account, now: i64) -> Self {
        Self::with_status(
            account,
            now,
            ReadingStatus::SignedOut,
            account.provider.sign_in_hint(),
        )
    }
    pub fn not_installed(account: &Account, now: i64) -> Self {
        Self::with_status(account, now, ReadingStatus::NotInstalled, "Not installed")
    }
    pub fn rate_limited(account: &Account, now: i64, until: Option<i64>) -> Self {
        Self {
            status: ReadingStatus::RateLimited { until },
            note: Some("Rate limited, trying again soon".into()),
            ..Self::new(account, now)
        }
    }

    pub fn window(&self, id: &str) -> Option<&LimitWindow> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// The headline (main ring) window: explicit `headline_window`, else the provider's declared
    /// ids, else (only for providers that allow it) the first window, or the most-used weekly
    /// window. Otherwise a missing declared id gives `None`: no other window is promoted.
    pub fn headline(&self) -> Option<&LimitWindow> {
        self.headline_pick().map(|(w, _)| w)
    }

    /// The headline and whether it is the worst-weekly fallback.
    fn headline_pick(&self) -> Option<(&LimitWindow, bool)> {
        if let Some(id) = &self.headline_window {
            return self.window(id).map(|w| (w, false));
        }
        let declared = self.provider.headline_candidates();
        if let Some(w) = declared.iter().find_map(|id| self.window(id)) {
            return Some((w, false));
        }
        if self.provider.headline_falls_back_to_first() {
            return self
                .windows
                .iter()
                .find(|w| w.count.is_none())
                .or(self.windows.first())
                .map(|w| (w, false));
        }
        if self.provider.headline_falls_back_to_worst_weekly() {
            return self
                .windows
                .iter()
                .filter(|w| w.kind == WindowKind::Weekly && w.count.is_none())
                .filter_map(|w| w.used_percent.filter(|p| p.is_finite()).map(|p| (w, p)))
                .fold(
                    None,
                    |best: Option<(&LimitWindow, f64)>, (w, p)| match best {
                        Some((_, b)) if b >= p => best,
                        _ => Some((w, p)),
                    },
                )
                .map(|(w, _)| (w, true));
        }
        None
    }

    /// The weekly (second ring) window, never the same as the headline. `None` when the headline
    /// is itself the worst-weekly fallback (no second ring repeating the same kind of figure).
    pub fn weekly(&self) -> Option<&LimitWindow> {
        let pick = self.headline_pick();
        if pick.is_some_and(|(_, fallback)| fallback) {
            return None;
        }
        let head = pick.map(|(w, _)| w.id.as_str());
        let found = match &self.weekly_window {
            Some(id) => self.window(id),
            None => self
                .provider
                .weekly_candidates()
                .iter()
                .find_map(|id| self.window(id)),
        };
        found.filter(|w| Some(w.id.as_str()) != head)
    }

    /// Headline percent used (None = no figure).
    pub fn headline_percent(&self) -> Option<f64> {
        if !self.shows_figures() {
            return None;
        }
        self.headline().and_then(|w| w.used_percent)
    }

    /// Highest known percent across all windows (None when nothing is known).
    pub fn worst_percent(&self) -> Option<f64> {
        if !self.shows_figures() {
            return None;
        }
        self.windows
            .iter()
            .filter_map(|w| w.used_percent)
            .fold(None, |acc: Option<f64>, p| {
                Some(acc.map_or(p, |a| a.max(p)))
            })
    }

    /// Signed out / not installed / empty readings show no figure even if windows linger.
    pub fn shows_figures(&self) -> bool {
        !matches!(
            self.status,
            ReadingStatus::SignedOut | ReadingStatus::NotInstalled | ReadingStatus::Empty
        )
    }

    /// Age in seconds at `now` (never negative).
    pub fn age(&self, now: i64) -> i64 {
        (now - self.fetched_at).max(0)
    }

    /// Drawn dimmed: status Stale, or older than [`STALE_AFTER_SECS`].
    pub fn is_stale(&self, now: i64) -> bool {
        self.status == ReadingStatus::Stale
            || (self.fetched_at > 0 && self.age(now) > STALE_AFTER_SECS)
    }
}

// ---------------------------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------------------------

/// What an agent session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ActivityState {
    #[default]
    Idle,
    /// Agent busy (spinning arc).
    Working,
    /// Agent needs the user (pulsing amber ring).
    Waiting,
    /// Finished, not yet seen by the user.
    Done,
}

impl ActivityState {
    /// Attention rank: Waiting > Working > Done > Idle.
    pub fn rank(self) -> u8 {
        match self {
            ActivityState::Waiting => 3,
            ActivityState::Working => 2,
            ActivityState::Done => 1,
            ActivityState::Idle => 0,
        }
    }
    /// Most attention-worthy state of a set (Idle when empty).
    pub fn aggregate<I: IntoIterator<Item = ActivityState>>(states: I) -> ActivityState {
        states
            .into_iter()
            .max_by_key(|s| s.rank())
            .unwrap_or_default()
    }
}

/// One agent session (Claude Code session, Codex thread, Cursor composer …).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    /// "<cwd leaf> · <id prefix>" or the tool's own name.
    pub title: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    pub state: ActivityState,
    /// When the current state started (unix secs).
    pub since: i64,
    /// Session cost in USD when known.
    #[serde(default)]
    pub cost: Option<f64>,
    /// Short detail: last prompt (≤120 chars), waiting message or current tool. No secrets.
    #[serde(default)]
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Analytics summary (filled by codenotch-analytics)
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenCounts {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
    pub fn add(&mut self, other: &TokenCounts) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub provider: ProviderKind,
    pub tokens: TokenCounts,
    /// USD cost of the priced tokens only (a lower bound when `unpriced_tokens > 0`).
    pub cost_usd: f64,
    pub sessions: u32,
    /// Tokens of models without a known price, left out of `cost_usd`.
    #[serde(default)]
    pub unpriced_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    /// Local date `YYYY-MM-DD`.
    pub date: String,
    pub provider: ProviderKind,
    pub tokens: TokenCounts,
    /// USD cost of the priced tokens only (a lower bound when `unpriced_tokens > 0`).
    pub cost_usd: f64,
    /// Tokens of models without a known price, left out of `cost_usd`.
    #[serde(default)]
    pub unpriced_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub provider: ProviderKind,
    pub tokens: TokenCounts,
    /// USD cost; 0 with `unpriced_tokens > 0` means "no price known" (unknown, not free).
    pub cost_usd: f64,
    /// Tokens without a known price (all of them for an unpriced model).
    #[serde(default)]
    pub unpriced_tokens: u64,
}

/// Token/cost analytics over the last `days` days from local transcripts.
///
/// Costs cover **priced** tokens only: when `unpriced_tokens > 0` every cost here is a lower
/// bound and must be shown as such ("≥ …") or as "—", never as a complete figure.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsSummary {
    pub days: u32,
    pub generated_at: i64,
    pub total_tokens: TokenCounts,
    pub total_cost_usd: f64,
    pub per_provider: Vec<ProviderUsage>,
    /// Oldest first.
    pub per_day: Vec<DailyUsage>,
    /// Most expensive first.
    pub per_model: Vec<ModelUsage>,
    /// Tokens of models without a known price, left out of `total_cost_usd`.
    #[serde(default)]
    pub unpriced_tokens: u64,
    /// Why the last analytics run failed (short, English, no paths or secrets). `None` when it
    /// worked. On failure the previous figures are kept and `generated_at` is the failed run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Fact tables for the statistics page (per day × provider × model over the period, the
    /// previous period and the 53-week heatmap; weekday × hour; sessions). Shared behind an
    /// `Arc`. Empty for summaries from older versions or failed first runs.
    #[serde(default)]
    pub detail: crate::analytics::SharedDetail,
}

impl AnalyticsSummary {
    /// Every token has a price (costs are complete figures).
    pub fn fully_priced(&self) -> bool {
        self.unpriced_tokens == 0
    }
}

// ---------------------------------------------------------------------------------------------
// Update state
// ---------------------------------------------------------------------------------------------

/// Where the self-updater stands (see `codenotch_engine::update`). `manual`: the check was
/// asked from the settings window (the UI then reports every outcome, not only news).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdateState {
    #[default]
    Idle,
    /// Reading the update manifest.
    Checking {
        manual: bool,
    },
    UpToDate {
        checked_at: i64,
        manual: bool,
    },
    /// A newer version is published. `forced`: the running version is below the manifest's
    /// `min_version` (installed without asking).
    Available {
        version: String,
        /// Release notes (plain text, may be empty).
        notes: String,
        /// Release page (https only).
        notes_url: Option<String>,
        forced: bool,
        manual: bool,
    },
    /// Downloading `version`: `progress` 0..=100.
    Downloading {
        version: String,
        progress: u8,
        forced: bool,
    },
    /// Downloaded, verified and unpacked; waiting to be installed.
    Ready {
        version: String,
        forced: bool,
    },
    /// Files swapped; the new version is starting and this process is about to quit.
    Installing {
        version: String,
    },
    /// `message`: English i18n key or short technical reason.
    Failed {
        message: String,
        manual: bool,
    },
}

impl UpdateState {
    /// A check, download or install is running.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            UpdateState::Checking { .. }
                | UpdateState::Downloading { .. }
                | UpdateState::Installing { .. }
        )
    }
}

// ---------------------------------------------------------------------------------------------
// Teams sync status
// ---------------------------------------------------------------------------------------------

/// State of Teams sync, as the settings window shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TeamsState {
    /// `teamsSyncEnabled` is off (default): no request, no file, no thread.
    #[default]
    Off,
    /// On, but this device is not paired with a team.
    NotPaired,
    /// Paired and sending.
    Active,
    /// Paired; sending is paused (network error, server busy …) and will be retried.
    Paused,
    /// The team revoked this device (its token is refused).
    Revoked,
    /// The team changed what it sees: the member must review and accept again.
    ConsentNeeded,
    /// Sync cannot run (storage error, worker failed to start …): see `last_error`.
    Error,
}

/// Teams sync status in the [`Snapshot`]. Holds no secret: the device token's presence is
/// implied by the state, never its value.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamsStatus {
    pub state: TeamsState,
    /// Name of the paired team. `None` = not paired.
    pub team_name: Option<String>,
    /// Events waiting to be sent.
    pub queued: u32,
    /// Last successful send (unix secs). `None` = never.
    pub last_sent_at: Option<i64>,
    /// Last error, English i18n key or short technical reason (no secret).
    pub last_error: Option<String>,
}

/// Who pays for an account's plan, as the member declares it to the team.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamsPayer {
    /// The company pays (or reimburses) the plan.
    Company,
    /// The member pays and is not reimbursed.
    Member,
}

// ---------------------------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------------------------

/// One configured account with everything the UI needs about it.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountView {
    pub account: Account,
    /// Latest reading (`None` = waiting for the first reading: "…").
    pub reading: Option<Reading>,
    /// Aggregated activity of `sessions` (plus non-session busy signals).
    pub activity: ActivityState,
    pub sessions: Vec<SessionInfo>,
    /// A fetch is in flight (refresh spinner).
    pub refreshing: bool,
    /// Next scheduled fetch (unix secs), e.g. end of a back-off.
    pub next_attempt_at: Option<i64>,
}

impl AccountView {
    pub fn new(account: Account) -> Self {
        Self {
            account,
            reading: None,
            activity: ActivityState::Idle,
            sessions: Vec::new(),
            refreshing: false,
            next_attempt_at: None,
        }
    }
    pub fn headline(&self) -> Option<&LimitWindow> {
        self.reading.as_ref().and_then(|r| r.headline())
    }
    pub fn headline_percent(&self) -> Option<f64> {
        self.reading.as_ref().and_then(|r| r.headline_percent())
    }
    pub fn weekly(&self) -> Option<&LimitWindow> {
        self.reading.as_ref().and_then(|r| r.weekly())
    }
    pub fn is_stale(&self, now: i64) -> bool {
        self.reading.as_ref().is_some_and(|r| r.is_stale(now))
    }
}

/// Everything a UI needs to draw, as plain cloneable data. Produced by the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub config: Config,
    /// One view per configured account, in `config.accounts` order (use
    /// [`Snapshot::notch_accounts`] for display order).
    pub accounts: Vec<AccountView>,
    /// A full-screen app is in front of the notch's monitor.
    pub fullscreen: bool,
    /// Game mode active (config switch or detected game).
    pub game_mode: bool,
    pub update: UpdateState,
    /// Last completed update check (unix secs), whatever its outcome.
    pub update_checked_at: Option<i64>,
    pub analytics: Option<AnalyticsSummary>,
    /// Engine clock when the snapshot was built (unix secs).
    pub now: i64,
    /// Last config load/save error, for the settings error strip.
    pub storage_error: Option<String>,
    /// Which app-wide secrets exist (presence only, never values).
    pub secrets: SecretsPresent,
    /// Notch hidden for this session by `Command::ToggleNotch` (global hotkey). Not persisted.
    pub notch_hidden: bool,
    /// Teams sync status ([`TeamsState::Off`] unless the user turned it on).
    pub teams: TeamsStatus,
}

impl Snapshot {
    /// Snapshot with one empty view per account.
    pub fn from_config(config: Config, now: i64) -> Self {
        let accounts = config
            .accounts
            .iter()
            .cloned()
            .map(AccountView::new)
            .collect();
        Self {
            config,
            accounts,
            fullscreen: false,
            game_mode: false,
            update: UpdateState::Idle,
            update_checked_at: None,
            analytics: None,
            now,
            storage_error: None,
            secrets: SecretsPresent::default(),
            notch_hidden: false,
            teams: TeamsStatus::default(),
        }
    }
    pub fn settings(&self) -> &Settings {
        &self.config.settings
    }
    pub fn lang(&self) -> crate::i18n::Lang {
        self.config.settings.lang()
    }
    pub fn account(&self, id: &str) -> Option<&AccountView> {
        self.accounts.iter().find(|v| v.account.id == id)
    }
    pub fn account_mut(&mut self, id: &str) -> Option<&mut AccountView> {
        self.accounts.iter_mut().find(|v| v.account.id == id)
    }
    /// Enabled, not hidden, not "not installed", in provider order: one notch cell each.
    pub fn notch_accounts(&self) -> Vec<&AccountView> {
        self.config
            .notch_accounts()
            .into_iter()
            .filter_map(|a| self.account(&a.id))
            .filter(|v| {
                !matches!(
                    v.reading.as_ref().map(|r| &r.status),
                    Some(ReadingStatus::NotInstalled)
                )
            })
            .collect()
    }
    /// Highest headline percent across enabled accounts.
    pub fn worst_percent(&self) -> Option<f64> {
        self.accounts
            .iter()
            .filter(|v| v.account.enabled)
            .filter_map(|v| v.headline_percent())
            .fold(None, |acc: Option<f64>, p| {
                Some(acc.map_or(p, |a| a.max(p)))
            })
    }
    /// Most attention-worthy activity across accounts.
    pub fn activity(&self) -> ActivityState {
        ActivityState::aggregate(self.accounts.iter().map(|v| v.activity))
    }
}

// ---------------------------------------------------------------------------------------------
// Commands and events
// ---------------------------------------------------------------------------------------------

/// UI / tray / hotkey → engine. Sent on a `std::sync::mpsc::Sender<Command>`.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Refresh one account (`Some(id)`) or all (`None`). Manual: bypasses the poll interval but
    /// not a rate-limit deadline.
    Refresh(Option<String>),
    /// Replace and persist the whole config.
    SaveConfig(Box<Config>),
    /// Ask the UI to open the settings window (engine echoes [`Event::OpenSettings`]).
    OpenSettings,
    /// Open a URL in the default browser.
    OpenUrl(String),
    /// Start the provider's sign-in flow for this account id.
    SignIn(String),
    /// Add another Claude profile (new config dir + sign-in).
    AddClaudeAccount,
    InstallHooks,
    RemoveHooks,
    TestTelegram,
    TestDiscord,
    /// Show a sample reset card.
    PreviewResetCard,
    /// Check the update manifest now (settings button: every outcome is reported).
    CheckUpdate,
    /// Download (if needed) and install the update on offer now, then restart.
    InstallUpdate,
    /// App → engine: the user is busy in a CodeNotch window (settings open and focused…): an
    /// automatic update waits until this is false.
    UiBusy(bool),
    /// Put the notch back in the middle of its edge (and on the primary monitor if its own is gone).
    Recentre,
    /// Open the data folder in Explorer.
    OpenDataFolder,
    /// Recompute analytics now.
    RefreshAnalytics,
    /// App → engine: desktop state polled by `codenotch-platform` (full-screen app in front of
    /// the notch's monitor, game detected). The engine copies it into the next snapshot.
    DesktopState {
        fullscreen: bool,
        game: bool,
    },
    /// Store a secret (settings window). The engine writes it to the `SecretStore` and
    /// republishes the snapshot with updated `secrets` presence. `Debug` hides the value.
    SetSecret {
        key: SecretKey,
        value: SecretValue,
    },
    /// Delete a secret from the store (env vars are untouched).
    ClearSecret(SecretKey),
    /// Show/hide the notch for this session (global hotkey); flips `Snapshot::notch_hidden`.
    ToggleNotch,
    /// Teams: read the invite (team, visibility, server) before pairing. The value is the
    /// invite link (secret; `Debug` hides it).
    TeamsPreviewInvite(SecretValue),
    /// Teams: pair this device with the invite link (after the user confirmed the preview).
    TeamsPair(SecretValue),
    /// Teams: detach this device (token, pairing state and queue are deleted).
    TeamsDetach,
    /// Teams: send the queued events now.
    TeamsFlush,
    /// Teams: the member's separate agreement to also send the usage of the days before
    /// pairing (`true`) or its withdrawal (`false`).
    TeamsAllowBackfill(bool),
    /// Teams: declare who pays for a shared account's plan and, when the member pays, the
    /// monthly price they declare (`None` = not declared).
    TeamsDeclareAccount {
        account_id: String,
        payer: TeamsPayer,
        price_eur_month: Option<f64>,
    },
    /// Teams: ask the server to erase this member's data.
    TeamsEraseServerData,
    Quit,
}

/// Kind of a user alert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AlertKind {
    /// Crossed the warning threshold.
    Warn,
    /// Crossed the critical threshold.
    Critical,
    /// Projected to run out within `minutes_left`.
    Forecast { minutes_left: i64 },
    /// The window renewed.
    Reset,
}

/// One alert, ready to show (title/body already localised).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub kind: AlertKind,
    pub account_id: String,
    pub window_id: String,
    pub used_percent: Option<f64>,
    pub at: i64,
    pub title: String,
    pub body: String,
}

/// Engine → UI / notify.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// New state to draw. The UI replaces its whole snapshot.
    SnapshotChanged(Box<Snapshot>),
    /// A window renewed: show the reset card.
    ResetDetected {
        account_id: String,
        window_id: String,
        used_percent: Option<f64>,
        next_reset_at: Option<i64>,
    },
    /// An alert was raised (already delivered to toast/Telegram/Discord by notify).
    Alert(Alert),
    /// Open (or focus) the settings window.
    OpenSettings,
    /// Short message for a toast/strip ("Test message sent", "Hooks installed", an error …).
    Notice(String),
    /// The app is quitting: close windows.
    Quit,
}

/// Callback the engine uses to publish [`Event`]s. Cheap to clone, callable from any thread.
/// The UI side creates it with `codenotch_ui_kit::UiModel::spawn_pump`.
#[derive(Clone)]
pub struct EventSink(Arc<dyn Fn(Event) + Send + Sync>);

impl EventSink {
    pub fn new(f: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
    /// Sink that drops everything.
    pub fn noop() -> Self {
        Self::new(|_| {})
    }
    /// Sink backed by a std channel (CLI, tests).
    pub fn channel() -> (Self, std::sync::mpsc::Receiver<Event>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        (
            Self::new(move |e| {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(e);
                }
            }),
            rx,
        )
    }
    pub fn emit(&self, event: Event) {
        (self.0)(event)
    }
}

impl std::fmt::Debug for EventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventSink")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(kind: ProviderKind) -> Account {
        Account::new(kind, "t")
    }

    #[test]
    fn provider_ids_round_trip() {
        for kind in ProviderKind::CANONICAL_ORDER {
            assert_eq!(&ProviderKind::from_id(kind.id()), kind);
            let json = serde_json::to_string(kind).unwrap();
            assert_eq!(&serde_json::from_str::<ProviderKind>(&json).unwrap(), kind);
        }
        assert_eq!(ProviderKind::from_id("Gemini"), ProviderKind::Antigravity);
        assert_eq!(ProviderKind::from_id("weird").id(), "weird");
        assert!(ProviderKind::Claude
            .usage_url()
            .unwrap()
            .starts_with("https://"));
    }

    #[test]
    fn headline_uses_declared_ids_and_never_promotes() {
        let a = acc(ProviderKind::Claude);
        let mut r = Reading::fresh(
            &a,
            0,
            vec![
                LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly).used(30.0),
                LimitWindow::new("session", "Current session", WindowKind::Session).used(61.0),
            ],
        );
        assert_eq!(r.headline().unwrap().id, "session");
        assert_eq!(r.weekly().unwrap().id, "seven_day");
        assert_eq!(r.worst_percent(), Some(61.0));
        // Codex declares `primary`: a missing one promotes nothing.
        let codex = Reading::fresh(
            &acc(ProviderKind::Codex),
            0,
            vec![LimitWindow::new("secondary", "Weekly limit", WindowKind::Weekly).used(30.0)],
        );
        assert!(
            codex.headline().is_none(),
            "weekly must not be promoted to headline"
        );
        assert_eq!(codex.headline_percent(), None);
        r.windows.remove(1);
        assert_eq!(
            r.headline().unwrap().id,
            "seven_day",
            "Claude: worst weekly"
        );
    }

    #[test]
    fn claude_without_session_heads_with_the_most_used_weekly() {
        let a = acc(ProviderKind::Claude);
        let mut r = Reading::fresh(
            &a,
            0,
            vec![
                LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly).used(71.0),
                LimitWindow::new("weekly_scoped", "Weekly limit (Fable)", WindowKind::Weekly)
                    .used(73.0),
                LimitWindow::new("extra_usage", "Extra usage", WindowKind::Monthly).used(90.0),
            ],
        );
        assert_eq!(r.headline().unwrap().id, "weekly_scoped");
        assert_eq!(r.headline_percent(), Some(73.0));
        assert!(r.weekly().is_none(), "no second ring in the fallback");
        // a session comes back: unchanged behaviour
        r.windows
            .push(LimitWindow::new("session", "Current session", WindowKind::Session).used(12.0));
        assert_eq!(r.headline().unwrap().id, "session");
        assert_eq!(r.weekly().unwrap().id, "seven_day");
        // extra usage alone (or an unknown weekly figure) never heads
        let only_extra = Reading::fresh(
            &a,
            0,
            vec![
                LimitWindow::new("extra_usage", "Extra usage", WindowKind::Monthly).used(90.0),
                LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly),
            ],
        );
        assert!(only_extra.headline().is_none());
    }

    #[test]
    fn grok_falls_back_to_first_window() {
        let a = acc(ProviderKind::Grok);
        let r = Reading::fresh(
            &a,
            0,
            vec![LimitWindow::new("x", "Credits", WindowKind::Other).used(5.0)],
        );
        assert_eq!(r.headline_percent(), Some(5.0));
    }

    #[test]
    fn explicit_headline_and_weekly_never_equal() {
        let a = acc(ProviderKind::Antigravity);
        let mut r = Reading::fresh(
            &a,
            0,
            vec![LimitWindow::new("w", "Weekly limit", WindowKind::Weekly).used(5.0)],
        );
        r.headline_window = Some("w".into());
        r.weekly_window = Some("w".into());
        assert!(r.weekly().is_none());
    }

    #[test]
    fn unknown_is_never_zero() {
        let w = LimitWindow::new("a", "A", WindowKind::Other);
        assert_eq!(w.used_percent, None);
        assert_eq!(w.fraction(), None);
        assert_eq!(
            LimitWindow::new("a", "A", WindowKind::Other)
                .used(f64::NAN)
                .used_percent,
            None
        );
        let a = acc(ProviderKind::Codex);
        let r = Reading::fresh(&a, 0, vec![w]);
        assert_eq!(r.worst_percent(), None);
        let signed_out = Reading::signed_out(&a, 0);
        assert_eq!(signed_out.headline_percent(), None);
    }

    #[test]
    fn staleness_and_pace() {
        let a = acc(ProviderKind::Codex);
        let r = Reading::fresh(&a, 1_000, vec![]);
        assert!(!r.is_stale(1_000 + STALE_AFTER_SECS));
        assert!(r.is_stale(1_001 + STALE_AFTER_SECS));
        // 5 h window, 1 h elapsed (20 %), 50 % used → 30 points deficit.
        let w = LimitWindow::new("primary", "Current session", WindowKind::Session)
            .used(50.0)
            .window_seconds(5 * 3600)
            .resets_at(4 * 3600);
        assert!((w.pace_deficit(0).unwrap() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn fraction_or_percent() {
        assert_eq!(percent_from_fraction_or_percent(0.42), Some(42.0));
        assert_eq!(percent_from_fraction_or_percent(42.0), Some(42.0));
        assert_eq!(percent_from_fraction_or_percent(420.0), Some(100.0));
        assert_eq!(percent_from_fraction_or_percent(f64::INFINITY), None);
    }

    #[test]
    fn reading_serde_round_trip() {
        let a = acc(ProviderKind::Claude);
        let mut r = Reading::rate_limited(&a, 5, Some(99));
        r.windows
            .push(LimitWindow::new("session", "Current session", WindowKind::Session).used(1.5));
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"rateLimited\""));
        assert_eq!(serde_json::from_str::<Reading>(&json).unwrap(), r);
    }

    #[test]
    fn activity_aggregate() {
        use ActivityState::*;
        assert_eq!(ActivityState::aggregate([Idle, Done, Working]), Working);
        assert_eq!(ActivityState::aggregate([Working, Waiting]), Waiting);
        assert_eq!(ActivityState::aggregate([]), Idle);
    }

    #[test]
    fn teams_status_is_off_by_default_and_commands_hide_invites() {
        let snap = Snapshot::from_config(Config::default(), 0);
        assert_eq!(snap.teams, TeamsStatus::default());
        assert_eq!(snap.teams.state, TeamsState::Off);
        assert_eq!(snap.teams.queued, 0);
        assert_eq!(snap.teams.last_sent_at, None);
        let invite = SecretValue::new("https://teams.example.test/join/s3cr3t-code");
        for cmd in [
            Command::TeamsPreviewInvite(invite.clone()),
            Command::TeamsPair(invite),
        ] {
            assert!(!format!("{cmd:?}").contains("s3cr3t"));
        }
    }

    #[test]
    fn event_sink_channel_delivers() {
        let (sink, rx) = EventSink::channel();
        let s2 = sink.clone();
        std::thread::spawn(move || s2.emit(Event::Notice("hi".into())))
            .join()
            .unwrap();
        assert_eq!(rx.recv().unwrap(), Event::Notice("hi".into()));
    }
}
