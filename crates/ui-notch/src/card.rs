//! Hover card content model (visual-spec §5.3, data-spec §2.5/§6). Pure: built from one
//! `AccountView`, rendered by `notch.rs`.

use crate::cell::{is_current, sweep};
use crate::text::{datetime, t, tf};
use chrono::{Datelike, TimeZone, Timelike};
use codenotch_core::config::Settings;
use codenotch_core::format::{age, forecast_card, reset_card, used_line, ForecastTone};
use codenotch_core::i18n::{tr, tr_label, Lang};
use codenotch_core::model::{AccountView, ActivityState, LimitWindow, ProviderKind, ReadingStatus};

pub const MAX_SESSIONS: usize = codenotch_ui_kit::metrics::CARD_MAX_SESSIONS;

#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    /// Filled share of the track (remaining mode fills by what is left).
    pub fraction: f32,
    /// Colour source (always *used*).
    pub used: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// Countdown, or "Last reported reset: …" for a historical Claude window.
    pub reset: Option<String>,
    /// `None` for count-only windows.
    pub bar: Option<Bar>,
    /// "73% used · 27% left · 12% deficit" / "~17 requests today".
    pub line: String,
    /// Provider detail ("$19.40 of $20.00").
    pub detail: Option<String>,
    /// "At this pace: runs out Thu 3:20 PM" / "At this pace: ≈ 82% at reset".
    pub forecast: Option<(String, ForecastTone)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Row(Row),
    /// Windows sharing a `group` heading (Antigravity families, Codex Spark …).
    Group {
        heading: String,
        rows: Vec<Row>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub state: ActivityState,
    pub left: String,
    pub right: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignIn {
    pub label: String,
    pub busy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardModel {
    pub account_id: String,
    pub provider: ProviderKind,
    /// "Claude Usage".
    pub title: String,
    /// "someone@example.com · Max" when several accounts of this provider are on the notch.
    pub account_line: Option<String>,
    /// "Updated 12m ago".
    pub stale_line: Option<String>,
    /// "Last known usage — not current."
    pub not_current: Option<String>,
    pub blocks: Vec<Block>,
    pub notes: Vec<String>,
    pub sign_in: Option<SignIn>,
    pub sessions: Vec<SessionRow>,
    /// "and 3 more".
    pub more: Option<String>,
}

/// Inputs besides the view.
pub struct CardCtx<'a, Tz: TimeZone> {
    pub lang: Lang,
    pub settings: &'a Settings,
    pub now: i64,
    pub tz: &'a Tz,
    pub clock24: bool,
    /// Several notch accounts share this provider.
    pub multi: bool,
    /// A sign-in was started from the card and has not finished.
    pub signing_in: bool,
}

pub fn card_model<Tz: TimeZone>(view: &AccountView, cx: &CardCtx<'_, Tz>) -> CardModel {
    let lang = cx.lang;
    let provider = view.account.provider.clone();
    let title = tf(lang, "{name} Usage", &[("name", provider.display_name())]);
    let reading = view.reading.as_ref();

    let account_line = cx.multi.then(|| {
        let who = reading
            .and_then(|r| r.identity.clone())
            .unwrap_or_else(|| view.account.name.clone());
        match reading.and_then(|r| r.plan.as_deref()) {
            Some(plan) if !plan.is_empty() => format!("{who} · {plan}"),
            _ => who,
        }
    });
    let stale_line = reading
        .filter(|r| r.is_stale(cx.now) && r.fetched_at > 0)
        .map(|r| {
            tf(
                lang,
                "Updated {age}",
                &[("age", &age(lang, r.fetched_at, cx.now))],
            )
        });

    let mut blocks: Vec<Block> = Vec::new();
    let mut not_current = false;
    if let Some(r) = reading.filter(|r| r.status != ReadingStatus::NotInstalled) {
        for w in &r.windows {
            let current = is_current(r, w, cx.now) && r.shows_figures();
            not_current |= !current && r.provider == ProviderKind::Claude;
            let row = row_for(w, current, cx);
            match &w.group {
                Some(g) => match blocks
                    .iter_mut()
                    .find(|b| matches!(b, Block::Group { heading, .. } if heading == g))
                {
                    Some(Block::Group { rows, .. }) => rows.push(row),
                    _ => blocks.push(Block::Group {
                        heading: tr(lang, g).to_string(),
                        rows: vec![row],
                    }),
                },
                None => blocks.push(Block::Row(row)),
            }
        }
    }

    let mut notes = Vec::new();
    match reading {
        None => notes.push(tr(lang, "Waiting for first reading…").to_string()),
        Some(r) => {
            if r.status == ReadingStatus::SignedOut {
                let hint = tr(lang, provider.sign_in_hint()).to_string();
                notes.push(hint.clone());
                if let Some(n) = r
                    .note
                    .as_deref()
                    .filter(|n| !n.is_empty() && tr(lang, n) != hint)
                {
                    notes.push(tr(lang, n).to_string());
                }
            } else {
                if let Some(n) = r.note.as_deref().filter(|n| !n.is_empty()) {
                    notes.push(tr(lang, n).to_string());
                }
                if let ReadingStatus::RateLimited { until: Some(until) } = r.status {
                    if let Some(when) = codenotch_core::format::reset_in(lang, Some(until), cx.now)
                    {
                        notes.push(tf(lang, "Retrying {when}", &[("when", &when)]));
                    }
                }
                if let Some(e) = r.error.as_deref().filter(|e| !e.is_empty()) {
                    notes.push(tr(lang, e).to_string());
                }
                if notes.is_empty() && r.windows.is_empty() {
                    notes.push(match r.status {
                        ReadingStatus::Empty => t(lang, "Nothing to report").to_string(),
                        _ => tr(lang, "Waiting for first reading…").to_string(),
                    });
                }
            }
        }
    }

    let sign_in = (provider == ProviderKind::Claude
        && reading.is_some_and(|r| r.status == ReadingStatus::SignedOut))
    .then(|| SignIn {
        label: tr(
            lang,
            if cx.signing_in {
                "Signing in..."
            } else {
                "Sign in"
            },
        )
        .to_string(),
        busy: cx.signing_in,
    });

    let (sessions, more) = session_rows(view, lang);
    CardModel {
        account_id: view.account.id.clone(),
        provider,
        title,
        account_line,
        stale_line,
        not_current: (not_current && !blocks.is_empty())
            .then(|| tr(lang, "Last known usage — not current.").to_string()),
        blocks,
        notes,
        sign_in,
        sessions,
        more,
    }
}

fn row_for<Tz: TimeZone>(w: &LimitWindow, current: bool, cx: &CardCtx<'_, Tz>) -> Row {
    let lang = cx.lang;
    let reset = if current {
        reset_card(lang, w.resets_at, cx.now, cx.tz, cx.clock24)
    } else {
        w.resets_at
            .and_then(|at| cx.tz.timestamp_opt(at, 0).single())
            .map(|d| {
                let when = datetime(lang, d.month(), d.day(), d.hour(), d.minute(), cx.clock24);
                tf(lang, "Last reported reset: {t}", &[("t", &when)])
            })
    };
    let label = tr_label(lang, &w.label_key).into_owned();
    if let Some(n) = w.count {
        let line = match n {
            0 => tr(lang, "no requests today").to_string(),
            1 => tr(lang, "~1 request today").to_string(),
            n => tf(lang, "~{n} requests today", &[("n", &n.to_string())]),
        };
        return Row {
            label,
            reset,
            bar: None,
            line,
            detail: w.detail.clone(),
            forecast: None,
        };
    }
    let remaining = cx.settings.show_remaining;
    let mut line = used_line(lang, w.used_percent, remaining);
    if cx.settings.show_usage_pace && current {
        if let Some(d) = w.pace_deficit(cx.now).map(f64::round).filter(|d| *d >= 1.0) {
            line.push_str(" · ");
            line.push_str(&tf(lang, "{v}% deficit", &[("v", &format!("{d:.0}"))]));
        }
    }
    let bar = Bar {
        fraction: w.used_percent.map_or(0.0, |u| sweep(u, remaining)),
        used: w.used_percent,
    };
    let forecast = (cx.settings.show_forecast && current)
        .then(|| forecast_card(lang, w, cx.now, cx.tz, cx.clock24))
        .flatten();
    Row {
        label,
        reset,
        bar: Some(bar),
        line,
        detail: w.detail.clone(),
        forecast,
    }
}

fn state_word(lang: Lang, s: ActivityState) -> Option<String> {
    match s {
        ActivityState::Working => Some(tr(lang, "Working").to_string()),
        ActivityState::Waiting => Some(tr(lang, "needs your input").to_string()),
        ActivityState::Done => Some(tr(lang, "Done").to_string()),
        ActivityState::Idle => None,
    }
}

/// Session rows: waiting first, then busy, then the rest; newest first; at most 5.
fn session_rows(view: &AccountView, lang: Lang) -> (Vec<SessionRow>, Option<String>) {
    let mut list: Vec<&codenotch_core::model::SessionInfo> = view.sessions.iter().collect();
    list.sort_by(|a, b| {
        b.state
            .rank()
            .cmp(&a.state.rank())
            .then(b.since.cmp(&a.since))
    });
    let mut rows: Vec<SessionRow> = list
        .iter()
        .take(MAX_SESSIONS)
        .map(|s| SessionRow {
            state: s.state,
            left: if s.title.is_empty() {
                s.id.clone()
            } else {
                s.title.clone()
            },
            right: state_word(lang, s.state),
        })
        .collect();
    if rows.is_empty() && view.activity != ActivityState::Idle {
        rows.push(SessionRow {
            state: view.activity,
            left: view.account.provider.display_name().to_string(),
            right: state_word(lang, view.activity),
        });
    }
    let more = (list.len() > MAX_SESSIONS).then(|| {
        tf(
            lang,
            "and {n} more",
            &[("n", &(list.len() - MAX_SESSIONS).to_string())],
        )
    });
    (rows, more)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use codenotch_core::demo::ids;
    use codenotch_core::model::{SessionInfo, Snapshot};

    const NOW: i64 = 1_800_000_000;

    fn model(s: &Snapshot, id: &str, lang: Lang, multi: bool) -> CardModel {
        let utc = FixedOffset::east_opt(0).unwrap();
        let cx = CardCtx {
            lang,
            settings: s.settings(),
            now: NOW,
            tz: &utc,
            clock24: false,
            multi,
            signing_in: false,
        };
        card_model(s.account(id).unwrap(), &cx)
    }

    fn rows(m: &CardModel) -> Vec<&Row> {
        m.blocks
            .iter()
            .flat_map(|b| match b {
                Block::Row(r) => vec![r],
                Block::Group { rows, .. } => rows.iter().collect(),
            })
            .collect()
    }

    #[test]
    fn claude_card() {
        let s = Snapshot::demo_at(NOW);
        let m = model(&s, ids::CLAUDE, Lang::En, true);
        assert_eq!(m.title, "Claude Usage");
        assert_eq!(m.account_line.as_deref(), Some("someone@example.com · Max"));
        assert!(m.stale_line.is_none() && m.not_current.is_none());
        let r = rows(&m);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].label, "Current session");
        assert_eq!(r[0].reset.as_deref(), Some("Resets at 10:14 AM")); // 08:00 UTC + 2 h 14
        assert_eq!(r[0].line, "61% used · 39% left");
        assert!((r[0].bar.as_ref().unwrap().fraction - 0.61).abs() < 1e-6);
        // sessions: working before done
        assert_eq!(m.sessions.len(), 2);
        assert_eq!(m.sessions[0].state, ActivityState::Working);
        assert_eq!(m.sessions[0].right.as_deref(), Some("Working"));
        assert!(m.sign_in.is_none());
        assert!(m.notes.is_empty());
    }

    #[test]
    fn french_card() {
        let s = Snapshot::demo_at(NOW);
        let m = model(&s, ids::CLAUDE, Lang::Fr, false);
        assert_eq!(m.title, "Consommation Claude");
        assert!(m.account_line.is_none());
        let r = rows(&m);
        assert_eq!(r[0].label, "Session en cours");
        assert_eq!(r[0].line, "61 % utilisés · 39 % restants");
    }

    #[test]
    fn model_scoped_weekly_label_is_translated() {
        let mut s = Snapshot::demo_at(NOW);
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            r.windows[1].label_key = "Weekly limit (Fable)".into();
        }
        let m = model(&s, ids::CLAUDE, Lang::Fr, false);
        assert_eq!(rows(&m)[1].label, "Limite hebdomadaire (Fable)");
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        assert_eq!(rows(&m)[1].label, "Weekly limit (Fable)");
    }

    #[test]
    fn stale_signed_out_rate_limited_error_waiting() {
        let s = Snapshot::demo_at(NOW);
        let codex = model(&s, ids::CODEX, Lang::En, false);
        assert_eq!(codex.stale_line.as_deref(), Some("Updated 40m ago"));
        assert!(
            codex.not_current.is_none(),
            "only Claude is labelled historical"
        );

        let grok = model(&s, ids::GROK, Lang::En, false);
        assert_eq!(grok.notes, vec!["Run grok login to see usage.".to_string()]);
        assert!(grok.sign_in.is_none(), "sign-in button is Claude only");

        let cursor = model(&s, ids::CURSOR, Lang::En, false);
        assert!(cursor
            .notes
            .iter()
            .any(|n| n == "Rate limited, trying again soon"));
        assert!(cursor.notes.iter().any(|n| n == "HTTP 429"));
        assert!(cursor
            .notes
            .iter()
            .any(|n| n.starts_with("Retrying in 10 min")));
        assert_eq!(rows(&cursor)[0].detail.as_deref(), Some("$19.40 of $20.00"));

        let glm = model(&s, ids::GLM, Lang::En, false);
        assert_eq!(
            glm.notes,
            vec!["not reachable: connection refused".to_string()]
        );

        let oc = model(&s, ids::OPENCODE, Lang::En, false);
        assert_eq!(oc.notes, vec!["Waiting for first reading…".to_string()]);
        assert!(oc.blocks.is_empty());
    }

    #[test]
    fn count_window_and_pace() {
        let mut s = Snapshot::demo_at(NOW);
        let custom = model(&s, ids::CUSTOM, Lang::En, false);
        let r = rows(&custom);
        assert_eq!(r[1].line, "~17 requests today");
        assert!(r[1].bar.is_none());
        s.config.settings.show_usage_pace = true;
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        // 5 h window, 2 h 14 left → 2 h 46 elapsed (55.3 %), 61 % used → 6 % deficit
        assert_eq!(rows(&m)[0].line, "61% used · 39% left · 6% deficit");
        s.config.settings.show_remaining = true;
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        assert!(rows(&m)[0].line.starts_with("39% left · 61% used"));
        assert!((rows(&m)[0].bar.as_ref().unwrap().fraction - 0.39).abs() < 1e-6);
    }

    #[test]
    fn forecast_lines_follow_the_setting() {
        let mut s = Snapshot::demo_at(NOW);
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        let r = rows(&m);
        // Session: 61 % after 2 h 46 at a steady pace → runs out ~1 h 46 later, before the
        // reset in 2 h 14 (NOW is 08:00 UTC).
        let (text, tone) = r[0].forecast.clone().expect("session forecast");
        assert!(
            text.starts_with("At this pace: runs out at 9:") && !text.contains("estimate"),
            "{text}"
        );
        assert_eq!(tone, ForecastTone::Warn);
        // Weekly: 34 % after 3 d 20 h of 7 d → about 62 % at reset.
        let (text, tone) = r[1].forecast.clone().expect("weekly forecast");
        assert!(
            text == "At this pace: ≈ 62% at reset" || text == "At this pace: ≈ 61% at reset",
            "{text}"
        );
        assert_eq!(tone, ForecastTone::Muted);
        // No window length → no forecast.
        assert_eq!(r[2].forecast, None);
        let fr = model(&s, ids::CLAUDE, Lang::Fr, false);
        let (text, _) = rows(&fr)[0].forecast.clone().unwrap();
        assert!(text.starts_with("À ce rythme : épuisé à 9:"), "{text}");
        s.config.settings.show_forecast = false;
        let off = model(&s, ids::CLAUDE, Lang::En, false);
        assert!(rows(&off).iter().all(|r| r.forecast.is_none()));
    }

    #[test]
    fn signed_out_claude_shows_history_and_sign_in() {
        let mut s = Snapshot::demo_at(NOW);
        if let Some(r) = s.account_mut(ids::CLAUDE).and_then(|v| v.reading.as_mut()) {
            r.status = ReadingStatus::SignedOut;
            r.note = Some("Sign in to Claude Code to see usage.".into());
        }
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        assert_eq!(
            m.not_current.as_deref(),
            Some("Last known usage — not current.")
        );
        assert!(rows(&m)[0]
            .reset
            .as_deref()
            .unwrap()
            .starts_with("Last reported reset: "));
        assert_eq!(m.notes.len(), 1, "hint not repeated");
        assert_eq!(m.sign_in.as_ref().unwrap().label, "Sign in");
    }

    #[test]
    fn groups_and_session_overflow() {
        let mut s = Snapshot::demo_at(NOW);
        if let Some(v) = s.account_mut(ids::CLAUDE) {
            if let Some(r) = v.reading.as_mut() {
                r.windows[1].group = Some("Opus".into());
                r.windows[2].group = Some("Opus".into());
            }
            v.sessions = (0..8)
                .map(|i| SessionInfo {
                    id: format!("s{i}"),
                    title: format!("p{i}"),
                    state: if i == 3 {
                        ActivityState::Waiting
                    } else {
                        ActivityState::Idle
                    },
                    since: i,
                    ..Default::default()
                })
                .collect();
        }
        let m = model(&s, ids::CLAUDE, Lang::En, false);
        assert_eq!(m.blocks.len(), 2);
        assert!(
            matches!(&m.blocks[1], Block::Group { heading, rows } if heading == "Opus" && rows.len() == 2)
        );
        assert_eq!(m.sessions.len(), 5);
        assert_eq!(m.sessions[0].left, "p3");
        assert_eq!(m.sessions[1].left, "p7");
        assert_eq!(m.more.as_deref(), Some("and 3 more"));
    }
}
