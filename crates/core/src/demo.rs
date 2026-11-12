//! Realistic fake data covering every state, for building screens without providers.
//! All identities are fictional.
//!
//! * [`Snapshot::demo_at`]: the fixed fixture with every state (tests and UI work).
//! * [`Snapshot::demo_for`]: what `codenotch.exe --demo` shows: fake figures, but only for the
//!   providers discovered on this PC.

use crate::config::{Account, Config, Settings, WeeklyRing};
use crate::model::*;

/// Fixed ids so UI code and tests can refer to demo accounts.
pub mod ids {
    pub const CLAUDE: &str = "demo-claude";
    pub const CLAUDE_WORK: &str = "demo-claude-work";
    pub const CODEX: &str = "demo-codex";
    pub const GROK: &str = "demo-grok";
    pub const CURSOR: &str = "demo-cursor";
    pub const COPILOT: &str = "demo-copilot";
    pub const GLM: &str = "demo-glm";
    pub const OPENCODE: &str = "demo-opencode";
    pub const CUSTOM: &str = "demo-custom";
    pub const QODER: &str = "demo-qoder";
}

fn account(id: &str, kind: ProviderKind, name: &str) -> Account {
    let mut a = Account::new(kind, name);
    a.id = id.to_string();
    a
}

fn session(id: &str, title: &str, state: ActivityState, since: i64, detail: &str) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        title: title.into(),
        cwd: Some(format!(
            "C:\\dev\\{}",
            title.split(' ').next().unwrap_or("proj")
        )),
        model: Some("claude-sonnet-4-5".into()),
        state,
        since,
        cost: Some(0.42),
        detail: Some(detail.into()),
    }
}

impl Snapshot {
    /// The full demo fixture at the current time.
    pub fn demo() -> Snapshot {
        Snapshot::demo_at(crate::now_secs())
    }

    /// Demo snapshot limited to the `detected` providers (one entry per discovered account, in
    /// discovery order), at `now`. Pure: discovery is the caller's job.
    ///
    /// Each detected account takes the fixture's fictional reading for its kind (the first Claude
    /// the personal one, the second the work one); a kind the fixture lacks, or a third Claude,
    /// gets a plausible made-up reading. Undetected providers are absent, as is the custom
    /// endpoint (never discovered). Nothing detected = no account (the empty state). Analytics
    /// keep only the detected kinds.
    pub fn demo_for(detected: &[ProviderKind], now: i64) -> Snapshot {
        let full = Snapshot::demo_at(now);
        let mut snap = full.clone();
        snap.config.accounts.clear();
        snap.accounts.clear();
        let mut claudes = 0;
        for kind in detected {
            let template = match kind {
                ProviderKind::Claude => {
                    claudes += 1;
                    match claudes {
                        1 => Some(ids::CLAUDE),
                        2 => Some(ids::CLAUDE_WORK),
                        _ => None,
                    }
                }
                ProviderKind::Codex => Some(ids::CODEX),
                ProviderKind::Grok => Some(ids::GROK),
                ProviderKind::Cursor => Some(ids::CURSOR),
                ProviderKind::Copilot => Some(ids::COPILOT),
                ProviderKind::Glm => Some(ids::GLM),
                ProviderKind::OpenCode => Some(ids::OPENCODE),
                ProviderKind::Custom => continue,
                // The fixture's Qoder is "not installed"; a detected one is installed.
                ProviderKind::Qoder
                | ProviderKind::CommandCode
                | ProviderKind::Antigravity
                | ProviderKind::Other(_) => None,
            }
            .filter(|id| snap.account(id).is_none());
            let view = match template.and_then(|id| full.account(id)) {
                Some(v) => v.clone(),
                None => {
                    let mut id = format!("demo-{}", kind.id());
                    let mut n = 1;
                    while snap.account(&id).is_some() || full.account(&id).is_some() {
                        n += 1;
                        id = format!("demo-{}-{n}", kind.id());
                    }
                    made_up(&id, kind, now)
                }
            };
            snap.config.accounts.push(view.account.clone());
            snap.accounts.push(view);
        }
        snap.analytics = full
            .analytics
            .and_then(|a| analytics_for(a, |k| detected.contains(k)));
        demo_forecasts(&mut snap, now);
        snap
    }

    /// Demo snapshot at `now` (deterministic for tests).
    pub fn demo_at(now: i64) -> Snapshot {
        use WindowKind::*;
        let claude = account(ids::CLAUDE, ProviderKind::Claude, "Claude · perso");
        let claude_work = account(ids::CLAUDE_WORK, ProviderKind::Claude, "Claude · work");
        let codex = account(ids::CODEX, ProviderKind::Codex, "Codex");
        let grok = account(ids::GROK, ProviderKind::Grok, "Grok");
        let cursor = account(ids::CURSOR, ProviderKind::Cursor, "Cursor");
        let copilot = account(ids::COPILOT, ProviderKind::Copilot, "GitHub Copilot");
        let glm = account(ids::GLM, ProviderKind::Glm, "z.ai");
        let opencode = account(ids::OPENCODE, ProviderKind::OpenCode, "OpenCode");
        let mut custom = account(ids::CUSTOM, ProviderKind::Custom, "Team proxy");
        custom.endpoint = "https://usage.example.com/v1/quota".into();
        custom.secret_env = "TEAM_PROXY_TOKEN".into();
        let qoder = account(ids::QODER, ProviderKind::Qoder, "Qoder");

        let settings = Settings {
            language: "en".into(),
            weekly_ring: WeeklyRing::Outside,
            onboarding_done: Some(true),
            ..Settings::default()
        };
        let config = Config {
            accounts: vec![
                claude.clone(),
                claude_work.clone(),
                codex.clone(),
                grok.clone(),
                cursor.clone(),
                copilot.clone(),
                glm.clone(),
                opencode.clone(),
                custom.clone(),
                qoder.clone(),
            ],
            settings,
            extra: Default::default(),
        };
        let mut snap = Snapshot::from_config(config, now);

        // 1. Claude, fresh, multi-window, working.
        let mut r = Reading::fresh(
            &claude,
            now - 45,
            vec![
                LimitWindow::new("session", "Current session", Session)
                    .used(61.0)
                    .resets_at(now + 2 * 3600 + 14 * 60)
                    .window_seconds(5 * 3600),
                LimitWindow::new("seven_day", "Weekly limit", Weekly)
                    .used(34.0)
                    .resets_at(now + 3 * 86_400 + 4 * 3600)
                    .window_seconds(7 * 86_400),
                LimitWindow::new("seven_day_opus", "Weekly limit (Opus)", Weekly)
                    .used(12.0)
                    .resets_at(now + 3 * 86_400 + 4 * 3600),
            ],
        );
        r.identity = Some("someone@example.com".into());
        r.plan = Some("Max".into());
        set(
            &mut snap,
            ids::CLAUDE,
            Some(r),
            ActivityState::Working,
            vec![
                session(
                    "s-1",
                    "codenotch · 3f2a",
                    ActivityState::Working,
                    now - 95,
                    "cargo test --workspace",
                ),
                session(
                    "s-2",
                    "website · 9c1d",
                    ActivityState::Done,
                    now - 600,
                    "Update the pricing page",
                ),
            ],
        );

        // 2. Second Claude account, warning level, waiting on the user.
        let mut r = Reading::fresh(
            &claude_work,
            now - 30,
            vec![
                LimitWindow::new("session", "Current session", Session)
                    .used(88.0)
                    .resets_at(now + 40 * 60),
                LimitWindow::new("seven_day", "Weekly limit", Weekly)
                    .used(71.0)
                    .resets_at(now + 5 * 86_400),
            ],
        );
        r.identity = Some("dev@company.example".into());
        r.plan = Some("Team".into());
        set(
            &mut snap,
            ids::CLAUDE_WORK,
            Some(r),
            ActivityState::Waiting,
            vec![session(
                "s-3",
                "backend · 77ab",
                ActivityState::Waiting,
                now - 20,
                "Allow Bash: npm run migrate?",
            )],
        );

        // 3. Codex, stale (40 min old).
        let mut r = Reading::fresh(
            &codex,
            now - 40 * 60,
            vec![
                LimitWindow::new("primary", "Current session", Session)
                    .used(23.0)
                    .resets_at(now + 3 * 3600),
                LimitWindow::new("secondary", "Weekly limit", Weekly)
                    .used(57.0)
                    .resets_at(now + 6 * 86_400),
            ],
        );
        r.status = ReadingStatus::Stale;
        r.plan = Some("Plus".into());
        set(&mut snap, ids::CODEX, Some(r), ActivityState::Idle, vec![]);

        // 4. Grok, signed out.
        set(
            &mut snap,
            ids::GROK,
            Some(Reading::signed_out(&grok, now - 10)),
            ActivityState::Idle,
            vec![],
        );

        // 5. Cursor, rate limited, last numbers critical.
        let mut r = Reading::rate_limited(&cursor, now - 5 * 60, Some(now + 600));
        r.windows.push(
            LimitWindow::new("included", "Included usage", Monthly)
                .used(97.0)
                .resets_at(now + 12 * 86_400)
                .detail("$19.40 of $20.00"),
        );
        r.error = Some("HTTP 429".into());
        set(&mut snap, ids::CURSOR, Some(r), ActivityState::Idle, vec![]);
        if let Some(v) = snap.account_mut(ids::CURSOR) {
            v.next_attempt_at = Some(now + 600);
        }

        // 6. Copilot, monthly premium requests.
        let r = Reading::fresh(
            &copilot,
            now - 120,
            vec![
                LimitWindow::new("premium_interactions", "Premium requests", Monthly)
                    .used(45.0)
                    .resets_at(now + 18 * 86_400)
                    .detail("135 / 300 requests"),
            ],
        );
        set(
            &mut snap,
            ids::COPILOT,
            Some(r),
            ActivityState::Idle,
            vec![],
        );

        // 7. GLM, error.
        set(
            &mut snap,
            ids::GLM,
            Some(Reading::error(
                &glm,
                now - 60,
                "not reachable: connection refused",
            )),
            ActivityState::Idle,
            vec![],
        );

        // 8. OpenCode, first reading in flight.
        if let Some(v) = snap.account_mut(ids::OPENCODE) {
            v.refreshing = true;
        }

        // 9. Custom endpoint: derived figure + count-only window.
        let mut count = LimitWindow::new("requests", "Requests today", Other);
        count.count = Some(17);
        let r = Reading::fresh(
            &custom,
            now - 90,
            vec![
                LimitWindow::new("quota", "Quota", Monthly)
                    .used(8.5)
                    .derived(),
                count,
            ],
        );
        set(&mut snap, ids::CUSTOM, Some(r), ActivityState::Idle, vec![]);

        // 10. Qoder, not installed (no notch cell).
        set(
            &mut snap,
            ids::QODER,
            Some(Reading::not_installed(&qoder, now)),
            ActivityState::Idle,
            vec![],
        );

        snap.update = UpdateState::Available {
            version: "0.1.1".into(),
            notes: "Fixes and improvements.".into(),
            notes_url: Some("https://github.com/MakFly/codenotch-windows/releases".into()),
            forced: false,
            manual: false,
        };
        snap.update_checked_at = Some(now - 3600);
        snap.analytics = Some(demo_analytics(now));
        demo_forecasts(&mut snap, now);
        snap
    }
}

/// Forecasts for the demo readings, computed by the real estimator from a made-up steady
/// history (one integer sample every 10 minutes since each period started). The fixture's
/// Claude session runs out before its reset; its weekly window does not.
fn demo_forecasts(snap: &mut Snapshot, now: i64) {
    let utc = chrono::FixedOffset::east_opt(0).expect("UTC offset");
    for view in &mut snap.accounts {
        let Some(r) = view.reading.as_mut() else {
            continue;
        };
        if !r.shows_figures() {
            continue;
        }
        let observed = r.fetched_at;
        let id = r.account_id.as_str();
        let history: Vec<crate::history::HistoryPoint> = r
            .windows
            .iter()
            .filter_map(|w| {
                let (used, reset, len) = (w.used_percent?, w.resets_at?, w.window_seconds?);
                let start = reset - len;
                let span = (observed - start).max(1) as f64;
                Some(
                    (start..observed)
                        .step_by(600)
                        .map(move |at| crate::history::HistoryPoint {
                            account_id: id.to_string(),
                            label: w.label_key.clone(),
                            at,
                            used_percent: (used * (at - start) as f64 / span).floor(),
                            resets_at: Some(reset),
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();
        for w in &mut r.windows {
            if w.forecast.is_none() {
                w.forecast = crate::forecast::forecast(w, observed, &history, now, None, &utc);
            }
        }
    }
}

fn set(
    snap: &mut Snapshot,
    id: &str,
    reading: Option<Reading>,
    activity: ActivityState,
    sessions: Vec<SessionInfo>,
) {
    if let Some(v) = snap.account_mut(id) {
        v.reading = reading;
        v.activity = activity;
        v.sessions = sessions;
    }
}

/// A plausible fresh reading for a detected account the fixture has no entry for.
fn made_up(id: &str, kind: &ProviderKind, now: i64) -> AccountView {
    use WindowKind::*;
    let account = account(id, kind.clone(), kind.display_name());
    let r = match kind {
        ProviderKind::Claude => Reading::fresh(
            &account,
            now - 75,
            vec![
                LimitWindow::new("session", "Current session", Session)
                    .used(14.0)
                    .resets_at(now + 4 * 3600)
                    .window_seconds(5 * 3600),
                LimitWindow::new("seven_day", "Weekly limit", Weekly)
                    .used(9.0)
                    .resets_at(now + 6 * 86_400)
                    .window_seconds(7 * 86_400),
            ],
        ),
        ProviderKind::Qoder => {
            let mut r = Reading::fresh(
                &account,
                now - 150,
                vec![LimitWindow::new("quota", "Plan usage", Monthly)
                    .used(42.0)
                    .resets_at(now + 20 * 86_400)
                    .detail("840 / 2000 credits")],
            );
            r.plan = Some("Pro".into());
            r
        }
        // Derived figure + count-only window (the states the fixture's custom endpoint shows).
        ProviderKind::CommandCode => {
            let mut count = LimitWindow::new("requests", "Requests today", Other);
            count.count = Some(17);
            Reading::fresh(
                &account,
                now - 90,
                vec![
                    LimitWindow::new("monthly", "Monthly credits", Monthly)
                        .used(8.5)
                        .derived(),
                    count,
                ],
            )
        }
        ProviderKind::Antigravity => {
            let mut r = Reading::fresh(
                &account,
                now - 200,
                vec![
                    LimitWindow::new("gemini-5h", "5-hour Limit", Session)
                        .used(27.0)
                        .resets_at(now + 3 * 3600)
                        .window_seconds(5 * 3600),
                    LimitWindow::new("gemini-weekly", "Weekly Limit", Weekly)
                        .used(48.0)
                        .resets_at(now + 4 * 86_400)
                        .window_seconds(7 * 86_400),
                ],
            );
            r.headline_window = Some("gemini-5h".into());
            r.weekly_window = Some("gemini-weekly".into());
            r
        }
        _ => Reading::fresh(
            &account,
            now - 120,
            vec![LimitWindow::new("usage", "Usage", Monthly)
                .used(30.0)
                .resets_at(now + 15 * 86_400)],
        ),
    };
    let mut view = AccountView::new(account);
    view.reading = Some(r);
    view
}

/// `summary` limited to the kinds `keep` accepts, totals recomputed. `None` when nothing is left.
fn analytics_for(
    mut summary: AnalyticsSummary,
    keep: impl Fn(&ProviderKind) -> bool,
) -> Option<AnalyticsSummary> {
    summary.per_provider.retain(|p| keep(&p.provider));
    summary.per_day.retain(|d| keep(&d.provider));
    summary.per_model.retain(|m| keep(&m.provider));
    let mut detail = (*summary.detail).clone();
    detail.facts.retain(|f| keep(&f.provider));
    detail.hours.retain(|h| keep(&h.provider));
    detail.sessions.retain(|s| keep(&s.provider));
    summary.detail = crate::analytics::SharedDetail::new(detail);
    if summary.per_provider.is_empty() {
        return None;
    }
    summary.total_tokens = TokenCounts::default();
    summary.total_cost_usd = 0.0;
    for p in &summary.per_provider {
        summary.total_tokens.add(&p.tokens);
        summary.total_cost_usd += p.cost_usd;
    }
    Some(summary)
}

fn demo_analytics(now: i64) -> AnalyticsSummary {
    let day = |i: i64| {
        chrono::DateTime::from_timestamp(now - i * 86_400, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default()
    };
    let tc = |i: u64, o: u64, cr: u64, cw: u64| TokenCounts {
        input: i,
        output: o,
        cache_read: cr,
        cache_write: cw,
    };
    let mut per_day = Vec::new();
    for i in (0..7).rev() {
        let k = (7 - i) as u64;
        per_day.push(DailyUsage {
            date: day(i),
            provider: ProviderKind::Claude,
            tokens: tc(120_000 * k, 40_000 * k, 900_000 * k, 60_000 * k),
            cost_usd: 2.1 * k as f64,
            ..Default::default()
        });
        per_day.push(DailyUsage {
            date: day(i),
            provider: ProviderKind::Codex,
            tokens: tc(50_000 * k, 15_000 * k, 0, 0),
            cost_usd: 0.6 * k as f64,
            ..Default::default()
        });
    }
    let mut total = TokenCounts::default();
    let mut cost = 0.0;
    for d in &per_day {
        total.add(&d.tokens);
        cost += d.cost_usd;
    }
    let claude_cost: f64 = per_day
        .iter()
        .filter(|d| d.provider == ProviderKind::Claude)
        .map(|d| d.cost_usd)
        .sum();
    let mut claude_tokens = TokenCounts::default();
    let mut codex_tokens = TokenCounts::default();
    for d in &per_day {
        if d.provider == ProviderKind::Claude {
            claude_tokens.add(&d.tokens)
        } else {
            codex_tokens.add(&d.tokens)
        }
    }
    AnalyticsSummary {
        days: 7,
        generated_at: now,
        total_tokens: total,
        total_cost_usd: cost,
        per_provider: vec![
            ProviderUsage {
                provider: ProviderKind::Claude,
                tokens: claude_tokens,
                cost_usd: claude_cost,
                sessions: 23,
                ..Default::default()
            },
            ProviderUsage {
                provider: ProviderKind::Codex,
                tokens: codex_tokens,
                cost_usd: cost - claude_cost,
                sessions: 9,
                ..Default::default()
            },
        ],
        per_day,
        per_model: vec![
            ModelUsage {
                model: "claude-opus-4-1".into(),
                provider: ProviderKind::Claude,
                tokens: tc(900_000, 200_000, 4_000_000, 300_000),
                cost_usd: claude_cost * 0.7,
                ..Default::default()
            },
            ModelUsage {
                model: "claude-sonnet-4-5".into(),
                provider: ProviderKind::Claude,
                tokens: tc(2_460_000, 920_000, 21_200_000, 1_380_000),
                cost_usd: claude_cost * 0.3,
                ..Default::default()
            },
            ModelUsage {
                model: "gpt-5-codex".into(),
                provider: ProviderKind::Codex,
                tokens: codex_tokens,
                cost_usd: cost - claude_cost,
                ..Default::default()
            },
        ],
        detail: crate::analytics::SharedDetail::new(demo_detail(now, 7)),
        ..Default::default()
    }
}

/// Plausible statistics tables for the demo: a year of weekday-heavy usage on three models.
/// Not tied to the summary totals above.
fn demo_detail(now: i64, days: u32) -> crate::analytics::AnalyticsDetail {
    use crate::analytics::*;
    use chrono::{Datelike, Duration};
    let today = chrono::DateTime::from_timestamp(now, 0)
        .map(|d| d.date_naive())
        .unwrap_or_default();
    let fmt = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
    let back = |n: u32| today - Duration::days(i64::from(n.saturating_sub(1)));
    let fdays = fact_days(days);
    let models: [(ProviderKind, &str, f64); 3] = [
        (ProviderKind::Claude, "claude-opus-4-1", 0.000_018),
        (ProviderKind::Claude, "claude-sonnet-4-5", 0.000_004),
        (ProviderKind::Codex, "gpt-5-codex", 0.000_003),
    ];
    let mut facts = Vec::new();
    let mut sessions = Vec::new();
    let mut hours = Vec::new();
    for i in 0..i64::from(fdays) {
        let date = today - Duration::days(i);
        let wd = date.weekday().num_days_from_monday() as u64;
        // Cheap deterministic noise.
        let noise = (i as u64).wrapping_mul(2_654_435_761) % 100;
        if wd >= 5 && noise < 70 || noise < 8 {
            continue;
        }
        for (k, (provider, model, per_token)) in models.iter().enumerate() {
            let base = [300_000u64, 900_000, 250_000][k];
            let tokens = TokenCounts {
                input: base / 10 * (1 + noise % 7),
                output: base / 30 * (1 + noise % 5),
                cache_read: base * (2 + noise % 9),
                cache_write: base / 8 * (1 + noise % 3),
            };
            let n = 1 + (noise % 4) as u32;
            facts.push(UsageFact {
                date: fmt(date),
                provider: provider.clone(),
                model: (*model).into(),
                tokens,
                cost_usd: tokens.total() as f64 * per_token,
                unpriced_tokens: 0,
                sessions: n,
            });
            for _ in 0..n {
                sessions.push(SessionFact {
                    provider: provider.clone(),
                    activity: vec![SessionActivity {
                        date: fmt(date),
                        model: (*model).into(),
                    }],
                });
            }
        }
    }
    for (provider, model, per_token) in &models {
        for weekday in 0..7u8 {
            for hour in 8..20u8 {
                let w = if weekday >= 5 { 1 } else { 4 };
                let peak = 12 - (i64::from(hour) - 14).unsigned_abs().min(6);
                let tokens = 40_000 * w * peak;
                hours.push(HourCell {
                    provider: provider.clone(),
                    model: (*model).into(),
                    weekday,
                    hour,
                    tokens,
                    cost_usd: tokens as f64 * per_token,
                    unpriced_tokens: 0,
                });
            }
        }
    }
    facts.sort_by(|a, b| a.date.cmp(&b.date));
    let previous_to = back(days) - Duration::days(1);
    AnalyticsDetail {
        today: fmt(today),
        period_from: fmt(back(days)),
        previous_from: fmt(previous_to - Duration::days(i64::from(days - 1))),
        previous_to: fmt(previous_to),
        heatmap_from: fmt(back(HEATMAP_DAYS)),
        facts_from: fmt(back(fdays)),
        facts,
        hours,
        sessions,
        unpriced_models: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::cell_figure;

    #[test]
    fn demo_covers_every_state() {
        let now = 1_800_000_000;
        let s = Snapshot::demo_at(now);
        let status = |id: &str| {
            s.account(id)
                .unwrap()
                .reading
                .as_ref()
                .map(|r| r.status.clone())
        };
        assert_eq!(status(ids::CLAUDE), Some(ReadingStatus::Fresh));
        assert_eq!(status(ids::CODEX), Some(ReadingStatus::Stale));
        assert_eq!(status(ids::GROK), Some(ReadingStatus::SignedOut));
        assert!(matches!(
            status(ids::CURSOR),
            Some(ReadingStatus::RateLimited { .. })
        ));
        assert_eq!(status(ids::GLM), Some(ReadingStatus::Error));
        assert_eq!(status(ids::OPENCODE), None);
        assert_eq!(status(ids::QODER), Some(ReadingStatus::NotInstalled));
        assert_eq!(
            s.account(ids::CLAUDE).unwrap().activity,
            ActivityState::Working
        );
        assert_eq!(s.activity(), ActivityState::Waiting);
        assert_eq!(s.worst_percent(), Some(97.0));
        // Qoder has no cell.
        assert_eq!(s.notch_accounts().len(), 9);
        assert!(s
            .notch_accounts()
            .iter()
            .all(|v| v.account.id != ids::QODER));
        let fig = |id: &str| cell_figure(s.account(id).unwrap(), false);
        assert_eq!(fig(ids::CLAUDE), "61%");
        assert_eq!(fig(ids::GROK), "—");
        assert_eq!(fig(ids::OPENCODE), "…");
        assert_eq!(fig(ids::CUSTOM), "~9%");
        assert!(s.account(ids::CLAUDE).unwrap().weekly().is_some());
        assert!(s.account(ids::CODEX).unwrap().is_stale(now));
        assert!(s.analytics.as_ref().unwrap().total_cost_usd > 0.0);
        // Forecasts: the Claude session runs out before its reset, the weekly window fits.
        let claude = s.account(ids::CLAUDE).unwrap().reading.as_ref().unwrap();
        let session = claude.windows[0].forecast.unwrap();
        assert!(session.runs_out(), "{session:?}");
        let weekly = claude.windows[1].forecast.unwrap();
        assert!(!weekly.runs_out() && (55.0..70.0).contains(&weekly.at_reset_percent));
        assert_eq!(claude.windows[2].forecast, None, "no window length");
    }

    fn ids_of(s: &Snapshot) -> Vec<&str> {
        s.accounts.iter().map(|v| v.account.id.as_str()).collect()
    }

    /// Every discoverable provider detected (two Claude profiles): every state a detected
    /// provider can be in, and no custom endpoint.
    #[test]
    fn demo_for_all_providers_covers_every_state() {
        use ProviderKind::*;
        let now = 1_800_000_000;
        let detected = [
            Claude,
            Claude,
            Codex,
            Grok,
            Cursor,
            Copilot,
            Glm,
            OpenCode,
            CommandCode,
            Antigravity,
            Qoder,
        ];
        let s = Snapshot::demo_for(&detected, now);
        assert_eq!(s.accounts.len(), detected.len());
        assert_eq!(s.config.accounts.len(), detected.len());
        assert!(
            s.account(ids::CUSTOM).is_none(),
            "custom is never discovered"
        );
        assert!(s
            .accounts
            .iter()
            .all(|v| v.account.provider != ProviderKind::Custom));
        let full = Snapshot::demo_at(now);
        for id in [
            ids::CLAUDE,
            ids::CLAUDE_WORK,
            ids::CODEX,
            ids::GROK,
            ids::CURSOR,
            ids::COPILOT,
            ids::GLM,
            ids::OPENCODE,
        ] {
            assert_eq!(s.account(id), full.account(id), "{id} keeps the fixture");
        }
        let status = |p: ProviderKind| {
            s.accounts
                .iter()
                .find(|v| v.account.provider == p)
                .unwrap()
                .reading
                .as_ref()
                .map(|r| r.status.clone())
        };
        assert_eq!(status(Codex), Some(ReadingStatus::Stale));
        assert_eq!(status(Grok), Some(ReadingStatus::SignedOut));
        assert!(matches!(
            status(Cursor),
            Some(ReadingStatus::RateLimited { .. })
        ));
        assert_eq!(status(Glm), Some(ReadingStatus::Error));
        assert_eq!(status(OpenCode), None);
        // Detected = installed: every account has a notch cell.
        assert_eq!(status(Qoder), Some(ReadingStatus::Fresh));
        assert_eq!(s.notch_accounts().len(), detected.len());
        assert_eq!(s.activity(), ActivityState::Waiting);
        assert_eq!(s.worst_percent(), Some(97.0));
        let fig = |p: ProviderKind| {
            let v = s.accounts.iter().find(|v| v.account.provider == p).unwrap();
            cell_figure(v, false)
        };
        assert_eq!(fig(Claude), "61%");
        assert_eq!(fig(CommandCode), "~9%", "derived figure");
        assert_eq!(fig(Antigravity), "27%");
        assert_eq!(fig(Qoder), "42%");
        let ids = ids_of(&s);
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "ids are unique");
        let a = s.analytics.as_ref().unwrap();
        assert_eq!(a.per_provider.len(), 2);
    }

    #[test]
    fn demo_for_shows_only_detected_providers() {
        let now = 1_800_000_000;
        let s = Snapshot::demo_for(&[ProviderKind::Claude, ProviderKind::Codex], now);
        assert_eq!(ids_of(&s), [ids::CLAUDE, ids::CODEX]);
        assert!(s.account(ids::CLAUDE_WORK).is_none(), "one Claude profile");
        assert_eq!(s.notch_accounts().len(), 2);

        // Analytics follow the detected kinds.
        let s = Snapshot::demo_for(&[ProviderKind::Codex, ProviderKind::Cursor], now);
        let a = s.analytics.as_ref().unwrap();
        assert!(a
            .per_provider
            .iter()
            .all(|p| p.provider == ProviderKind::Codex));
        assert!(a.per_day.iter().all(|d| d.provider == ProviderKind::Codex));
        assert!(a
            .per_model
            .iter()
            .all(|m| m.provider == ProviderKind::Codex));
        assert!((a.total_cost_usd - a.per_provider[0].cost_usd).abs() < 1e-9);
        let s = Snapshot::demo_for(&[ProviderKind::Cursor], now);
        assert!(s.analytics.is_none());
    }

    #[test]
    fn demo_for_nothing_detected_is_the_empty_state() {
        let s = Snapshot::demo_for(&[], 1_800_000_000);
        assert!(s.accounts.is_empty());
        assert!(s.config.accounts.is_empty());
        assert!(s.notch_accounts().is_empty());
        assert!(s.analytics.is_none());
        let s = Snapshot::demo_for(&[ProviderKind::Custom], 1_800_000_000);
        assert!(
            s.accounts.is_empty(),
            "custom endpoints are not discoverable"
        );
    }

    #[test]
    fn demo_for_extra_claude_profiles_get_their_own_reading() {
        let s = Snapshot::demo_for(&vec![ProviderKind::Claude; 3], 1_800_000_000);
        assert_eq!(
            ids_of(&s),
            [ids::CLAUDE, ids::CLAUDE_WORK, "demo-claude-2"],
            "unique ids"
        );
        assert_eq!(
            s.accounts[2].reading.as_ref().map(|r| r.status.clone()),
            Some(ReadingStatus::Fresh)
        );
        assert!(s.accounts[2].headline().is_some());
    }
}
