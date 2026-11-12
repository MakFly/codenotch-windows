//! Per-account fetch scheduling as pure functions (no clock, no I/O).
//!
//! The engine keeps one [`AccountSched`] per enabled account and asks [`next_action`] on every
//! tick. Rules (agreed with the providers' authors, data-spec §2.4, §3.6, §4.4):
//!
//! | kind | automatic period | manual floor | 429 handling |
//! |---|---|---|---|
//! | Claude | 300 s (also while sessions are active) | 300 s, every attempt counts | exponential, persisted; one early attempt when a reset falls between the last attempt and now; skipped while the sign-in gate is busy |
//! | Codex | `max(300, poll)` | none; allowed during a back-off (reads its local snapshot) | `until` (automatic polls only) |
//! | Grok | `max(300, poll)` | none | `until` when set |
//! | Custom endpoint | `max(60, poll)` | none | `until` when set |
//! | providers-extra | `max(min_interval, poll)` | 60 s | `until` when set |

use codenotch_core::model::{ProviderKind, Reading, ReadingStatus};
use codenotch_core::{Account, Settings};
use std::time::Duration;

/// Claude: minimum gap between two attempts on one account (data-spec §2.4 `MIN_FETCH_SECS`).
pub const CLAUDE_FLOOR_SECS: i64 = 300;
/// Claude loop period while a Claude session is active (data-spec §2.4 `POLL_ACTIVE_SECS`).
/// Never below [`CLAUDE_FLOOR_SECS`] per account, so it cannot fetch more often.
pub const CLAUDE_ACTIVE_SECS: i64 = 60;
/// providers-extra: minimum gap before a manual refresh.
pub const EXTRA_MANUAL_FLOOR_SECS: i64 = 60;

/// How one account is paced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Automatic period in seconds.
    pub interval: i64,
    /// Minimum gap between the last attempt and a manual one.
    pub manual_floor: i64,
    /// Claude: exponential 429 back-off (`claude_backoff_secs`).
    pub exponential_backoff: bool,
    /// Claude: one early attempt when a window's reset falls in `(last attempt, now]`.
    pub reset_exception: bool,
    /// Codex: a manual refresh may run during a back-off.
    pub manual_during_backoff: bool,
    /// Claude: wait while the sign-in / renewal gate is busy.
    pub auth_gate: bool,
    /// Claude: after a restart the last attempt is `max(last attempt, fetched_at)`.
    pub restore_last_attempt: bool,
}

impl Policy {
    /// Policy for an account. `min_interval` = the provider's `min_interval()`;
    /// `claude_active` = a Claude session is running (faster loop, same per-account floor).
    pub fn for_account(
        account: &Account,
        min_interval: Duration,
        settings: &Settings,
        claude_active: bool,
    ) -> Policy {
        let kind = if account.uses_custom_endpoint() {
            ProviderKind::Custom
        } else {
            account.provider.clone()
        };
        Policy::for_kind(&kind, min_interval, settings, claude_active)
    }

    pub fn for_kind(
        kind: &ProviderKind,
        min_interval: Duration,
        settings: &Settings,
        claude_active: bool,
    ) -> Policy {
        let min = i64::try_from(min_interval.as_secs())
            .unwrap_or(i64::MAX)
            .max(1);
        let poll = i64::try_from(settings.poll_interval()).unwrap_or(300);
        let regular = min.max(poll);
        let base = Policy {
            interval: regular,
            manual_floor: 0,
            exponential_backoff: false,
            reset_exception: false,
            manual_during_backoff: false,
            auth_gate: false,
            restore_last_attempt: false,
        };
        match kind {
            ProviderKind::Claude => {
                let interval = if claude_active {
                    CLAUDE_ACTIVE_SECS
                } else {
                    regular
                };
                Policy {
                    interval: interval.max(CLAUDE_FLOOR_SECS),
                    manual_floor: CLAUDE_FLOOR_SECS,
                    exponential_backoff: true,
                    reset_exception: true,
                    auth_gate: true,
                    restore_last_attempt: true,
                    ..base
                }
            }
            ProviderKind::Codex => Policy {
                manual_during_backoff: true,
                ..base
            },
            ProviderKind::Grok | ProviderKind::Custom => base,
            _ => Policy {
                manual_floor: EXTRA_MANUAL_FLOOR_SECS,
                ..base
            },
        }
    }
}

/// Scheduling state of one account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSched {
    /// When the last attempt started (unix secs).
    pub last_attempt: Option<i64>,
    /// No automatic attempt before this (start-up stagger).
    pub not_before: i64,
    /// Rate-limit deadline.
    pub backoff_until: Option<i64>,
    /// Consecutive 429s (Claude).
    pub consecutive_429: u32,
    /// The user asked for a refresh that has not run yet.
    pub manual_pending: bool,
    /// A fetch is running.
    pub in_flight: bool,
    /// Reset times of the latest reading's windows.
    pub resets: Vec<i64>,
}

/// What to do with an account now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Start a fetch (`manual` is passed to the provider).
    Fetch { manual: bool },
    /// Nothing before `until`.
    Wait { until: i64 },
    /// A fetch is already running.
    InFlight,
    /// Claude sign-in / renewal holds the gate; try again on the next tick.
    AuthBusy,
}

impl Decision {
    /// For `AccountView::next_attempt_at`.
    pub fn next_attempt_at(self, now: i64) -> Option<i64> {
        match self {
            Decision::Fetch { .. } => Some(now),
            Decision::Wait { until } => Some(until),
            Decision::InFlight | Decision::AuthBusy => None,
        }
    }
}

/// The scheduling decision for one account at `now`.
pub fn next_action(s: &AccountSched, p: &Policy, now: i64, auth_busy: bool) -> Decision {
    if s.in_flight {
        return Decision::InFlight;
    }
    if p.auth_gate && auth_busy {
        return Decision::AuthBusy;
    }
    let manual_at = s
        .last_attempt
        .map_or(i64::MIN, |t| t.saturating_add(p.manual_floor));
    if let Some(until) = s.backoff_until.filter(|u| *u > now) {
        if s.manual_pending && p.manual_during_backoff && now >= manual_at {
            return Decision::Fetch { manual: true };
        }
        return Decision::Wait { until };
    }
    if s.manual_pending && now >= manual_at {
        return Decision::Fetch { manual: true };
    }
    let auto_at = match s.last_attempt {
        Some(t) => t.saturating_add(p.interval).max(s.not_before),
        None => s.not_before,
    };
    if now >= auto_at {
        return Decision::Fetch { manual: false };
    }
    let mut until = auto_at;
    if s.manual_pending {
        until = until.min(manual_at);
    }
    if p.reset_exception {
        if let Some(last) = s.last_attempt {
            if s.resets.iter().any(|r| last < *r && *r <= now) {
                return Decision::Fetch { manual: false };
            }
            if let Some(next_reset) = s
                .resets
                .iter()
                .copied()
                .filter(|r| *r > now && *r > last)
                .min()
            {
                until = until.min(next_reset);
            }
        }
    }
    Decision::Wait { until }
}

/// A fetch starts now.
pub fn on_start(s: &mut AccountSched, now: i64) {
    s.in_flight = true;
    s.manual_pending = false;
    s.last_attempt = Some(now);
}

/// A fetch finished with `reading` at `now`: back-off bookkeeping.
pub fn on_result(s: &mut AccountSched, p: &Policy, reading: &Reading, now: i64) {
    s.in_flight = false;
    s.resets = reading.windows.iter().filter_map(|w| w.resets_at).collect();
    match &reading.status {
        ReadingStatus::RateLimited { until } => {
            if p.exponential_backoff {
                s.consecutive_429 = s.consecutive_429.saturating_add(1);
                let retry_after = until.map(|u| (u - now).max(0));
                let own =
                    now + codenotch_providers::claude_backoff_secs(s.consecutive_429, retry_after);
                s.backoff_until = Some(until.map_or(own, |u| own.max(u)));
            } else {
                s.backoff_until = until.filter(|u| *u > now);
            }
        }
        ReadingStatus::Fresh => {
            s.consecutive_429 = 0;
            s.backoff_until = None;
        }
        _ => {
            if !p.exponential_backoff {
                s.backoff_until = None;
            }
        }
    }
}

/// Persisted pieces of an account's schedule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Restored<'a> {
    pub reading: Option<&'a Reading>,
    pub backoff_until: Option<i64>,
    pub last_attempt: Option<i64>,
    pub consecutive_429: u32,
}

/// Schedule state after a restart. Claude keeps its floor (`max(last attempt, fetched_at)`);
/// other providers start fresh (their restored readings are stale until refreshed). A deadline
/// is kept only while it is in the future.
pub fn restore(p: &Policy, saved: Restored<'_>, now: i64, not_before: i64) -> AccountSched {
    let last_attempt = if p.restore_last_attempt {
        let fetched = saved.reading.map(|r| r.fetched_at).filter(|t| *t > 0);
        match (saved.last_attempt, fetched) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
        .filter(|t| *t <= now)
    } else {
        None
    };
    AccountSched {
        last_attempt,
        not_before,
        backoff_until: saved.backoff_until.filter(|u| *u > now),
        consecutive_429: if p.exponential_backoff {
            saved.consecutive_429
        } else {
            0
        },
        manual_pending: false,
        in_flight: false,
        resets: saved
            .reading
            .map(|r| r.windows.iter().filter_map(|w| w.resets_at).collect())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_core::model::{LimitWindow, WindowKind};

    fn settings(poll: u64) -> Settings {
        Settings {
            poll_seconds: poll,
            ..Settings::default()
        }
    }
    fn claude() -> Policy {
        Policy::for_kind(
            &ProviderKind::Claude,
            Duration::from_secs(300),
            &settings(60),
            false,
        )
    }
    fn codex() -> Policy {
        Policy::for_kind(
            &ProviderKind::Codex,
            Duration::from_secs(300),
            &settings(60),
            false,
        )
    }
    fn grok() -> Policy {
        Policy::for_kind(
            &ProviderKind::Grok,
            Duration::from_secs(300),
            &settings(60),
            false,
        )
    }
    fn custom(poll: u64) -> Policy {
        Policy::for_kind(
            &ProviderKind::Custom,
            Duration::from_secs(60),
            &settings(poll),
            false,
        )
    }
    fn extra() -> Policy {
        Policy::for_kind(
            &ProviderKind::Cursor,
            Duration::from_secs(300),
            &settings(60),
            false,
        )
    }
    fn after(last: i64) -> AccountSched {
        AccountSched {
            last_attempt: Some(last),
            ..AccountSched::default()
        }
    }
    fn reading(
        kind: ProviderKind,
        status: ReadingStatus,
        fetched_at: i64,
        resets: &[i64],
    ) -> Reading {
        let a = Account::new(kind, "t");
        let mut r = Reading::fresh(&a, fetched_at, Vec::new());
        r.status = status;
        for (i, at) in resets.iter().enumerate() {
            r.windows.push(
                LimitWindow::new(format!("w{i}"), "W", WindowKind::Session)
                    .used(10.0)
                    .resets_at(*at),
            );
        }
        r
    }
    fn rl(until: Option<i64>) -> Reading {
        reading(
            ProviderKind::Claude,
            ReadingStatus::RateLimited { until },
            0,
            &[],
        )
    }
    use Decision::*;

    #[test]
    fn intervals_per_kind() {
        assert_eq!(claude().interval, 300);
        assert_eq!(
            Policy::for_kind(
                &ProviderKind::Claude,
                Duration::from_secs(300),
                &settings(30),
                true
            )
            .interval,
            300
        );
        assert_eq!(claude().manual_floor, 300);
        assert_eq!((codex().interval, codex().manual_floor), (300, 0));
        assert_eq!((grok().interval, grok().manual_floor), (300, 0));
        assert_eq!((custom(60).interval, custom(60).manual_floor), (60, 0));
        assert_eq!(custom(120).interval, 120);
        assert_eq!(
            custom(5).interval,
            60,
            "poll clamps to 30, the provider minimum wins"
        );
        assert_eq!((extra().interval, extra().manual_floor), (300, 60));
        let fast_extra = Policy::for_kind(
            &ProviderKind::Qoder,
            Duration::from_secs(10),
            &settings(45),
            false,
        );
        assert_eq!(fast_extra.interval, 45, "max(min_interval, poll)");
    }

    #[test]
    fn custom_endpoint_accounts_use_the_custom_policy() {
        let mut a = Account::new(ProviderKind::Claude, "x");
        a.endpoint = "https://example.com/u".into();
        let p = Policy::for_account(&a, Duration::from_secs(60), &settings(60), false);
        assert_eq!(p, custom(60));
        a.endpoint.clear();
        assert_eq!(
            Policy::for_account(&a, Duration::from_secs(300), &settings(60), false),
            claude()
        );
    }

    #[test]
    fn first_attempt_waits_for_the_stagger() {
        let s = AccountSched {
            not_before: 110,
            ..AccountSched::default()
        };
        assert_eq!(next_action(&s, &claude(), 100, false), Wait { until: 110 });
        assert_eq!(
            next_action(&s, &claude(), 110, false),
            Fetch { manual: false }
        );
        // A manual refresh is not held back by the stagger.
        let s = AccountSched {
            manual_pending: true,
            ..s
        };
        assert_eq!(
            next_action(&s, &claude(), 100, false),
            Fetch { manual: true }
        );
    }

    #[test]
    fn claude_floor_applies_to_automatic_and_manual() {
        let p = claude();
        let mut s = after(1_000);
        assert_eq!(next_action(&s, &p, 1_100, false), Wait { until: 1_300 });
        assert_eq!(next_action(&s, &p, 1_300, false), Fetch { manual: false });
        s.manual_pending = true;
        assert_eq!(
            next_action(&s, &p, 1_299, false),
            Wait { until: 1_300 },
            "manual within the floor waits"
        );
        assert_eq!(next_action(&s, &p, 1_300, false), Fetch { manual: true });
    }

    #[test]
    fn claude_failures_count_as_attempts() {
        let p = claude();
        let mut s = AccountSched::default();
        on_start(&mut s, 500);
        assert_eq!(next_action(&s, &p, 501, false), InFlight);
        on_result(
            &mut s,
            &p,
            &reading(ProviderKind::Claude, ReadingStatus::Error, 0, &[]),
            510,
        );
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 600, false), Wait { until: 800 });
        assert_eq!(next_action(&s, &p, 800, false), Fetch { manual: true });
    }

    #[test]
    fn claude_single_early_attempt_after_a_reset() {
        let p = claude();
        let mut s = after(1_000);
        s.resets = vec![1_100, 5_000];
        assert_eq!(
            next_action(&s, &p, 1_050, false),
            Wait { until: 1_100 },
            "wakes at the reset"
        );
        assert_eq!(next_action(&s, &p, 1_100, false), Fetch { manual: false });
        on_start(&mut s, 1_100);
        // The provider still reports the old reset (lag): no second early attempt.
        on_result(
            &mut s,
            &p,
            &reading(ProviderKind::Claude, ReadingStatus::Fresh, 1_100, &[1_100]),
            1_101,
        );
        assert_eq!(next_action(&s, &p, 1_150, false), Wait { until: 1_400 });
        // A reset before the last attempt never triggers anything.
        let s = AccountSched {
            resets: vec![900],
            ..after(1_000)
        };
        assert_eq!(next_action(&s, &p, 1_050, false), Wait { until: 1_300 });
    }

    #[test]
    fn reset_exception_is_claude_only_and_never_during_backoff() {
        let s = AccountSched {
            resets: vec![1_100],
            ..after(1_000)
        };
        assert_eq!(
            next_action(&s, &codex(), 1_150, false),
            Wait { until: 1_300 }
        );
        assert_eq!(
            next_action(&s, &extra(), 1_150, false),
            Wait { until: 1_300 }
        );
        let s = AccountSched {
            backoff_until: Some(2_000),
            ..s
        };
        assert_eq!(
            next_action(&s, &claude(), 1_150, false),
            Wait { until: 2_000 }
        );
    }

    #[test]
    fn claude_exponential_backoff() {
        let p = claude();
        let mut s = after(0);
        // 1st 429, provider until = now + 60 → 60 s.
        on_result(&mut s, &p, &rl(Some(1_060)), 1_000);
        assert_eq!((s.consecutive_429, s.backoff_until), (1, Some(1_060)));
        on_result(&mut s, &p, &rl(Some(2_060)), 2_000);
        assert_eq!(s.backoff_until, Some(2_120), "2nd: 120 s");
        on_result(&mut s, &p, &rl(Some(3_060)), 3_000);
        assert_eq!(s.backoff_until, Some(3_240), "3rd: 240 s");
        on_result(&mut s, &p, &rl(Some(4_060)), 4_000);
        on_result(&mut s, &p, &rl(Some(5_060)), 5_000);
        assert_eq!(s.backoff_until, Some(5_900), "capped at 900 s");
        on_result(&mut s, &p, &rl(Some(6_060)), 6_000);
        assert_eq!(s.backoff_until, Some(6_900));
        // Retry-After beyond the cap wins.
        on_result(&mut s, &p, &rl(Some(9_000)), 7_000);
        assert_eq!(s.backoff_until, Some(9_000));
        // Unknown until: own back-off.
        on_result(&mut s, &p, &rl(None), 10_000);
        assert_eq!(s.backoff_until, Some(10_900));
        // Not fetched before the deadline, even manually.
        s.manual_pending = true;
        s.last_attempt = Some(10_000);
        assert_eq!(next_action(&s, &p, 10_500, false), Wait { until: 10_900 });
        assert_eq!(next_action(&s, &p, 10_900, false), Fetch { manual: true });
        // An error keeps the count; a fresh reading resets it.
        on_result(
            &mut s,
            &p,
            &reading(ProviderKind::Claude, ReadingStatus::Error, 0, &[]),
            11_000,
        );
        assert_eq!(s.consecutive_429, 8);
        on_result(
            &mut s,
            &p,
            &reading(ProviderKind::Claude, ReadingStatus::Fresh, 11_000, &[]),
            11_000,
        );
        assert_eq!((s.consecutive_429, s.backoff_until), (0, None));
        on_result(&mut s, &p, &rl(Some(12_060)), 12_000);
        assert_eq!(s.backoff_until, Some(12_060), "count restarted");
    }

    #[test]
    fn claude_waits_while_the_auth_gate_is_busy() {
        let s = AccountSched {
            manual_pending: true,
            ..AccountSched::default()
        };
        assert_eq!(next_action(&s, &claude(), 100, true), AuthBusy);
        assert_eq!(
            next_action(&s, &claude(), 100, false),
            Fetch { manual: true }
        );
        assert_eq!(
            next_action(&s, &codex(), 100, true),
            Fetch { manual: true },
            "only Claude uses the gate"
        );
        assert_eq!(AuthBusy.next_attempt_at(5), None);
    }

    #[test]
    fn restart_restores_floor_and_deadlines() {
        let p = claude();
        let r = reading(ProviderKind::Claude, ReadingStatus::Stale, 900, &[5_000]);
        let saved = Restored {
            reading: Some(&r),
            backoff_until: Some(1_500),
            last_attempt: Some(950),
            consecutive_429: 2,
        };
        let s = restore(&p, saved.clone(), 1_000, 1_002);
        assert_eq!(s.last_attempt, Some(950), "max(last attempt, fetched_at)");
        assert_eq!(s.backoff_until, Some(1_500));
        assert_eq!(s.consecutive_429, 2);
        assert_eq!(s.resets, vec![5_000]);
        assert_eq!(next_action(&s, &p, 1_000, false), Wait { until: 1_500 });
        // Without a saved attempt, fetched_at is the last fetch.
        let s = restore(
            &p,
            Restored {
                last_attempt: None,
                backoff_until: None,
                ..saved.clone()
            },
            1_000,
            1_002,
        );
        assert_eq!(next_action(&s, &p, 1_000, false), Wait { until: 1_200 });
        // Expired deadline dropped; very old reading → due at the stagger.
        let old = reading(ProviderKind::Claude, ReadingStatus::Stale, 10, &[]);
        let s = restore(
            &p,
            Restored {
                reading: Some(&old),
                backoff_until: Some(999),
                last_attempt: None,
                consecutive_429: 1,
            },
            1_000,
            1_002,
        );
        assert_eq!(s.backoff_until, None);
        assert_eq!(next_action(&s, &p, 1_000, false), Wait { until: 1_002 });
        // Other providers start fresh (restored readings are stale until refreshed) but keep deadlines.
        let s = restore(&codex(), saved, 1_000, 1_004);
        assert_eq!((s.last_attempt, s.consecutive_429), (None, 0));
        assert_eq!(
            next_action(&s, &codex(), 1_004, false),
            Wait { until: 1_500 }
        );
        // A last attempt in the future (clock change) is ignored.
        let s = restore(
            &p,
            Restored {
                reading: None,
                backoff_until: None,
                last_attempt: Some(5_000),
                consecutive_429: 0,
            },
            1_000,
            1_000,
        );
        assert_eq!(s.last_attempt, None);
    }

    #[test]
    fn codex_manual_is_not_floored_and_may_run_during_backoff() {
        let p = codex();
        let mut s = after(1_000);
        assert_eq!(next_action(&s, &p, 1_005, false), Wait { until: 1_300 });
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 1_005, false), Fetch { manual: true });
        on_start(&mut s, 1_005);
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Codex,
                ReadingStatus::RateLimited { until: Some(2_000) },
                1_005,
                &[],
            ),
            1_006,
        );
        assert_eq!(s.backoff_until, Some(2_000));
        assert_eq!(
            next_action(&s, &p, 1_400, false),
            Wait { until: 2_000 },
            "automatic polls honour the deadline"
        );
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 1_400, false), Fetch { manual: true });
    }

    #[test]
    fn grok_honours_until_without_exponential_backoff() {
        let p = grok();
        let mut s = after(1_000);
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Grok,
                ReadingStatus::RateLimited { until: Some(1_100) },
                1_000,
                &[],
            ),
            1_000,
        );
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Grok,
                ReadingStatus::RateLimited { until: Some(1_100) },
                1_000,
                &[],
            ),
            1_001,
        );
        assert_eq!((s.backoff_until, s.consecutive_429), (Some(1_100), 0));
        s.manual_pending = true;
        assert_eq!(
            next_action(&s, &p, 1_050, false),
            Wait { until: 1_100 },
            "manual does not bypass a deadline"
        );
        assert_eq!(next_action(&s, &p, 1_100, false), Fetch { manual: true });
        // No floor on manual refresh.
        let mut s = after(1_000);
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 1_001, false), Fetch { manual: true });
        // Unknown until: no deadline at all.
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Grok,
                ReadingStatus::RateLimited { until: None },
                1_000,
                &[],
            ),
            1_001,
        );
        assert_eq!(s.backoff_until, None);
    }

    #[test]
    fn custom_polls_every_minute_and_honours_until() {
        let p = custom(60);
        let mut s = after(1_000);
        assert_eq!(next_action(&s, &p, 1_030, false), Wait { until: 1_060 });
        assert_eq!(next_action(&s, &p, 1_060, false), Fetch { manual: false });
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Custom,
                ReadingStatus::RateLimited { until: Some(1_500) },
                1_000,
                &[],
            ),
            1_000,
        );
        assert_eq!(next_action(&s, &p, 1_060, false), Wait { until: 1_500 });
        // A later non-429 reading clears it.
        on_result(
            &mut s,
            &p,
            &reading(ProviderKind::Custom, ReadingStatus::Error, 1_000, &[]),
            1_000,
        );
        assert_eq!(s.backoff_until, None);
    }

    #[test]
    fn extra_providers_manual_floor_and_until() {
        let p = extra();
        let mut s = after(1_000);
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 1_030, false), Wait { until: 1_060 });
        assert_eq!(next_action(&s, &p, 1_060, false), Fetch { manual: true });
        s.manual_pending = false;
        assert_eq!(next_action(&s, &p, 1_060, false), Wait { until: 1_300 });
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Cursor,
                ReadingStatus::RateLimited { until: Some(2_000) },
                1_000,
                &[],
            ),
            1_000,
        );
        s.manual_pending = true;
        assert_eq!(next_action(&s, &p, 1_500, false), Wait { until: 2_000 });
        // A deadline in the past is not kept.
        on_result(
            &mut s,
            &p,
            &reading(
                ProviderKind::Cursor,
                ReadingStatus::RateLimited { until: Some(900) },
                1_000,
                &[],
            ),
            1_000,
        );
        assert_eq!(s.backoff_until, None);
    }

    #[test]
    fn in_flight_fetches_are_never_doubled() {
        for p in [claude(), codex(), grok(), custom(60), extra()] {
            let mut s = AccountSched {
                manual_pending: true,
                ..AccountSched::default()
            };
            assert_eq!(next_action(&s, &p, 10_000, false), Fetch { manual: true });
            on_start(&mut s, 10_000);
            assert!(!s.manual_pending);
            s.manual_pending = true; // another click while running
            assert_eq!(next_action(&s, &p, 10_001, false), InFlight);
            assert_eq!(InFlight.next_attempt_at(1), None);
        }
    }

    #[test]
    fn manual_vs_automatic_flag() {
        let p = extra();
        let s = after(0);
        assert_eq!(next_action(&s, &p, 300, false), Fetch { manual: false });
        let s = AccountSched {
            manual_pending: true,
            ..s
        };
        assert_eq!(next_action(&s, &p, 300, false), Fetch { manual: true });
        assert_eq!(Fetch { manual: true }.next_attempt_at(7), Some(7));
        assert_eq!(Wait { until: 9 }.next_attempt_at(7), Some(9));
    }
}
