//! # codenotch-core
//!
//! Shared types and logic of CodeNotch for Windows. Every other crate depends on this one.
//! It holds plain data and pure logic only: no gpui, no Win32, no threads of its own.
//!
//! | module | what |
//! |---|---|
//! | [`paths`] | data folder (`%APPDATA%\dev.kev.smashtoken` or `CODENOTCH_DATA_DIR`) and file names |
//! | [`config`] | `config.gpui.json`: [`Config`], [`Account`], [`Settings`]; atomic save; unknown keys survive |
//! | [`model`] | [`ProviderKind`], [`LimitWindow`], [`Reading`], [`Snapshot`], [`Command`], [`Event`] … |
//! | [`provider`] | the [`Provider`] trait, [`Env`], [`FetchContext`] |
//! | [`http`] | the [`HttpClient`] trait, the real [`UreqHttp`] client and the [`FakeHttp`] test double |
//! | [`history`] | `history.jsonl` (`{"a","l","t","u","r"}` per line) |
//! | [`state`] | `state.json` (last readings and alert bookkeeping, owned by the engine) |
//! | [`i18n`] | `tr(lang, "English key")` with a French table |
//! | [`forecast`] | when a window runs out / percent at reset, from its history and the activity profile |
//! | [`format`] | percent, reset countdown, age, colour [`Grade`] |
//! | [`logging`] | small rotating file logger in `<data dir>\logs\`, [`logging::redact`] |
//! | [`demo`] | [`Snapshot::demo`] with every state, [`Snapshot::demo_for`] the detected providers |
//!
//! Conventions (binding for every crate):
//! * Missing data is never zero: an unknown percent is `None` and is shown as "—".
//! * Times are unix **seconds** (`i64`). Percentages are **0–100** (`f64`).
//! * Secrets (tokens, chat ids, webhook URLs) never go to logs, `Debug` output, the config file or the UI.

pub mod analytics;
pub mod config;
pub mod demo;
pub mod error;
pub mod forecast;
pub mod format;
pub mod history;
pub mod hooks;
pub mod http;
pub mod i18n;
pub mod logging;
pub mod model;
pub mod paths;
pub mod provider;
pub mod secrets;
pub mod state;
pub mod system;

pub use analytics::{
    AnalyticsDetail, HourCell, HourlyUsage, RootOwners, Series, SeriesBy, SessionActivity,
    SessionFact, SharedDetail, UsageFact, UsageFilter, UsageTotals, DAYS_PER_MONTH, HEATMAP_DAYS,
    HOURLY_EXPORT_DAYS,
};
pub use config::{
    teams_available_with, Account, ColorTransition, Config, Edge, NotchSize, Settings, ShowMode,
    TaskbarMode, ThemeMode, UpdateMode, WeeklyRing, WidgetPlacement, DEFAULT_UPDATE_MANIFEST_URL,
    TEAMS_OFFICIAL_ORIGIN,
};
pub use error::{Error, Result};
pub use format::Grade;
pub use history::{History, HistoryPoint};
pub use http::{FakeHttp, HttpClient, HttpError, HttpRequest, HttpResponse, Method, UreqHttp};
pub use i18n::{tr, trf, Lang};
pub use model::{
    AccountView, ActivityState, Alert, AlertKind, AnalyticsSummary, Command, DailyUsage, Event,
    EventSink, LimitWindow, ModelUsage, PlanCode, ProviderKind, ProviderUsage, Reading,
    ReadingStatus, SessionInfo, Snapshot, TeamsInvitePreview, TeamsPayer, TeamsState, TeamsStatus,
    TeamsVisibility, TokenCounts, UpdateState, WindowKind,
};
pub use paths::Paths;
pub use provider::{Env, FetchContext, LocalIdentity, Provider};
pub use secrets::{MemorySecrets, NoSecrets, SecretKey, SecretStore, SecretValue, SecretsPresent};
pub use state::PersistedState;
pub use system::{NoopSystem, SystemActions};

/// Version of the whole app (workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Current time in unix seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
