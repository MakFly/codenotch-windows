//! Text formatting and colour grading. Pure functions; deterministic given `now` (and a time
//! zone for wall-clock output).
//!
//! * Unknown → "—" (U+2014). Waiting for a first reading → "…" (U+2026). Never "0%".

use crate::i18n::{fill, tr, Lang};
use crate::model::{AccountView, LimitWindow, ReadingStatus};
use chrono::{Datelike, TimeZone, Timelike};

pub const DASH: &str = "—";
pub const ELLIPSIS: &str = "…";

// ---------------------------------------------------------------------------------------------
// Percent
// ---------------------------------------------------------------------------------------------

/// "42%", "0.4%", "<0.1%", "99.6%", ">99.9%", or "—" when unknown.
pub fn percent(value: Option<f64>) -> String {
    let Some(v) = value.filter(|v| v.is_finite()) else {
        return DASH.to_string();
    };
    let v = v.clamp(0.0, 100.0);
    if v == 0.0 || v == 100.0 {
        return format!("{v:.0}%");
    }
    if v < 1.0 || v > 99.0 {
        let tenths = (v * 10.0).round() / 10.0;
        if tenths < 0.1 {
            return "<0.1%".into();
        }
        if tenths > 99.9 {
            return ">99.9%".into();
        }
        return format!("{tenths:.1}%");
    }
    format!("{:.0}%", v.round())
}

/// Figure for one window: "~17" (count), "~42%" (derived), "42%", or "—".
/// With `show_remaining` the figure is 100 − used.
pub fn window_figure(w: &LimitWindow, show_remaining: bool) -> String {
    if let Some(n) = w.count {
        return format!("~{n}");
    }
    let value = if show_remaining {
        w.remaining_percent()
    } else {
        w.used_percent
    };
    let text = percent(value);
    if w.derived && value.is_some() {
        format!("~{text}")
    } else {
        text
    }
}

/// Text under a notch ring: "…" (no reading / no windows yet), "—" (signed out, nothing to
/// report, or no headline window), else the headline figure.
pub fn cell_figure(view: &AccountView, show_remaining: bool) -> String {
    let Some(reading) = &view.reading else {
        return ELLIPSIS.to_string();
    };
    if matches!(
        reading.status,
        ReadingStatus::SignedOut | ReadingStatus::Empty | ReadingStatus::NotInstalled
    ) {
        return DASH.to_string();
    }
    if reading.windows.is_empty() {
        return if reading.status == ReadingStatus::Fresh {
            ELLIPSIS
        } else {
            DASH
        }
        .to_string();
    }
    match reading.headline() {
        Some(w) => window_figure(w, show_remaining),
        None => DASH.to_string(),
    }
}

/// "73% used · 27% left" (or remaining first). "—" when unknown.
pub fn used_line(lang: Lang, used: Option<f64>, remaining_first: bool) -> String {
    let Some(u) = used else {
        return DASH.to_string();
    };
    let used_s = percent(Some(u)).trim_end_matches('%').to_string();
    let left_s = percent(Some(100.0 - u)).trim_end_matches('%').to_string();
    let key = if remaining_first {
        "{left}% left · {used}% used"
    } else {
        "{used}% used · {left}% left"
    };
    fill(tr(lang, key), &[("used", &used_s), ("left", &left_s)])
}

// ---------------------------------------------------------------------------------------------
// Durations and dates
// ---------------------------------------------------------------------------------------------

/// Compact duration: "14 min", "2 h 14 min", "2 h", "3 d 4 h" / "3 j 4 h". Rounds to minutes.
pub fn duration(lang: Lang, secs: i64) -> String {
    let mins = ((secs.max(0) as f64) / 60.0).round() as i64;
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    let day = if lang == Lang::Fr { "j" } else { "d" };
    if d > 0 {
        if h > 0 {
            format!("{d} {day} {h} h")
        } else {
            format!("{d} {day}")
        }
    } else if h > 0 {
        if m > 0 {
            format!("{h} h {m} min")
        } else {
            format!("{h} h")
        }
    } else {
        format!("{} min", m.max(1))
    }
}

/// Countdown: "in 2 h 14 min" / "dans 2 h 14 min"; "Resetting…" when due; "on Nov 20" /
/// "le 20 nov." when 7 days or more away. `None` when the reset time is unknown.
pub fn reset_in(lang: Lang, resets_at: Option<i64>, now: i64) -> Option<String> {
    reset_in_tz(lang, resets_at, now, &chrono::Local)
}

/// [`reset_in`] with an explicit time zone (tests).
pub fn reset_in_tz<Tz: TimeZone>(
    lang: Lang,
    resets_at: Option<i64>,
    now: i64,
    tz: &Tz,
) -> Option<String> {
    let at = resets_at?;
    let left = at - now;
    if left <= 0 {
        return Some(tr(lang, "Resetting…").to_string());
    }
    if left >= 7 * 86_400 {
        let date = tz.timestamp_opt(at, 0).single()?;
        return Some(fill(
            tr(lang, "on {date}"),
            &[("date", &month_day(lang, date.month(), date.day()))],
        ));
    }
    Some(fill(tr(lang, "in {d}"), &[("d", &duration(lang, left))]))
}

/// Hover-card style: "Resetting…", "Resets in 14 min", "Resets at 4:59 AM", "Resets Thu 4:59 AM",
/// "Resets Nov 20". `clock24` picks 24 h time ("16:59").
pub fn reset_card<Tz: TimeZone>(
    lang: Lang,
    resets_at: Option<i64>,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> Option<String> {
    let at = resets_at?;
    let left = at - now;
    if left <= 0 {
        return Some(tr(lang, "Resetting…").to_string());
    }
    let mins = ((left as f64) / 60.0).round() as i64;
    if mins < 60 {
        return Some(fill(
            tr(lang, "Resets in {m} min"),
            &[("m", &mins.max(1).to_string())],
        ));
    }
    let when = tz.timestamp_opt(at, 0).single()?;
    let today = tz.timestamp_opt(now, 0).single()?;
    let t = clock(when.hour(), when.minute(), clock24);
    if left < 86_400 {
        return Some(fill(tr(lang, "Resets at {t}"), &[("t", &t)]));
    }
    let days = when
        .date_naive()
        .signed_duration_since(today.date_naive())
        .num_days();
    if days < 7 {
        let wd = weekday(lang, when.weekday().num_days_from_monday());
        return Some(fill(tr(lang, "Resets {d} {t}"), &[("d", wd), ("t", &t)]));
    }
    Some(fill(
        tr(lang, "Resets {date}"),
        &[("date", &month_day(lang, when.month(), when.day()))],
    ))
}

/// Relative age: "just now", "12m ago" / "il y a 12 min", "2h ago", "3d ago".
pub fn age(lang: Lang, fetched_at: i64, now: i64) -> String {
    let secs = (now - fetched_at).max(0);
    if secs < 60 {
        return tr(lang, "just now").to_string();
    }
    let short = match lang {
        Lang::En => {
            if secs < 3600 {
                format!("{}m", secs / 60)
            } else if secs < 48 * 3600 {
                format!("{}h", secs / 3600)
            } else {
                format!("{}d", secs / 86_400)
            }
        }
        Lang::Fr => {
            if secs < 3600 {
                format!("{} min", secs / 60)
            } else if secs < 48 * 3600 {
                format!("{} h", secs / 3600)
            } else {
                format!("{} j", secs / 86_400)
            }
        }
    };
    fill(tr(lang, "{d} ago"), &[("d", &short)])
}

/// "4:59 AM" / "16:59".
pub fn clock(hour: u32, minute: u32, clock24: bool) -> String {
    if clock24 {
        format!("{hour:02}:{minute:02}")
    } else {
        let (h, ampm) = match hour {
            0 => (12, "AM"),
            1..=11 => (hour, "AM"),
            12 => (12, "PM"),
            _ => (hour - 12, "PM"),
        };
        format!("{h}:{minute:02} {ampm}")
    }
}

/// Default clock style for a language (fr: 24 h, en: 12 h).
pub fn default_clock24(lang: Lang) -> bool {
    lang == Lang::Fr
}

fn weekday(lang: Lang, from_monday: u32) -> &'static str {
    const EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const FR: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
    let i = (from_monday as usize).min(6);
    if lang == Lang::Fr {
        FR[i]
    } else {
        EN[i]
    }
}

/// Short month name: "Nov" / "nov.". `month` is 1..=12 (clamped).
pub fn month_abbr(lang: Lang, month: u32) -> &'static str {
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const FR: [&str; 12] = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    let i = (month.clamp(1, 12) - 1) as usize;
    if lang == Lang::Fr {
        FR[i]
    } else {
        EN[i]
    }
}

/// "Nov 4" / "4 nov.".
pub fn month_day(lang: Lang, month: u32, day: u32) -> String {
    let m = month_abbr(lang, month);
    if lang == Lang::Fr {
        format!("{day} {m}")
    } else {
        format!("{m} {day}")
    }
}

// ---------------------------------------------------------------------------------------------
// Forecast
// ---------------------------------------------------------------------------------------------

/// How loud a forecast line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForecastTone {
    /// Fits before the reset.
    Muted,
    /// Runs out before the reset.
    Warn,
    /// Runs out within the hour.
    Critical,
}

/// A future instant, short: "in 25 min", "at 3:20 PM" (same day), "Thu 3:20 PM" (this week),
/// "on Nov 20"; "now" when already past. `approx` marks it as an estimate: "in ~25 min",
/// "around 3:20 PM", "~Thu 3:20 PM", "on ~Nov 20".
pub fn moment<Tz: TimeZone>(
    lang: Lang,
    at: i64,
    now: i64,
    tz: &Tz,
    clock24: bool,
    approx: bool,
) -> String {
    let left = at - now;
    if left <= 0 {
        return tr(lang, "now").to_string();
    }
    let tilde = if approx { "~" } else { "" };
    let in_d = || {
        fill(
            tr(lang, "in {d}"),
            &[("d", &format!("{tilde}{}", duration(lang, left)))],
        )
    };
    if left < 3600 {
        return in_d();
    }
    let (Some(when), Some(today)) = (
        tz.timestamp_opt(at, 0).single(),
        tz.timestamp_opt(now, 0).single(),
    ) else {
        return in_d();
    };
    let t = clock(when.hour(), when.minute(), clock24);
    let days = when
        .date_naive()
        .signed_duration_since(today.date_naive())
        .num_days();
    if days == 0 {
        let key = if approx { "around {t}" } else { "at {t}" };
        fill(tr(lang, key), &[("t", &t)])
    } else if days < 7 {
        format!(
            "{tilde}{} {t}",
            weekday(lang, when.weekday().num_days_from_monday())
        )
    } else {
        fill(
            tr(lang, "on {date}"),
            &[(
                "date",
                &format!("{tilde}{}", month_day(lang, when.month(), when.day())),
            )],
        )
    }
}

/// The window's forecast worth showing: known, and the window is not already full.
fn shown_forecast(w: &LimitWindow) -> Option<&crate::forecast::Forecast> {
    let f = w.forecast.as_ref()?;
    (w.count.is_none() && w.used_percent.is_some_and(|u| u < 100.0)).then_some(f)
}

/// Hover-card line: "At this pace: runs out Thu 3:20 PM" (shown only before the reset; Warn,
/// Critical within the hour) or "At this pace: ≈ 82% at reset" (Muted). Low confidence marks
/// the time ("~Thu 3:20 PM", "around 3:20 PM") and adds "(estimate)". `None` without a
/// forecast or for a full window.
pub fn forecast_card<Tz: TimeZone>(
    lang: Lang,
    w: &LimitWindow,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> Option<(String, ForecastTone)> {
    use crate::forecast::Confidence;
    let f = shown_forecast(w)?;
    let low = f.confidence == Confidence::Low;
    let (what, tone) = match f.exhaust_at.filter(|_| f.runs_out()) {
        Some(at) => {
            let when = moment(lang, at, now, tz, clock24, low);
            let tone = if at - now <= 3600 {
                ForecastTone::Critical
            } else {
                ForecastTone::Warn
            };
            (fill(tr(lang, "runs out {when}"), &[("when", &when)]), tone)
        }
        None => (at_reset(lang, f), ForecastTone::Muted),
    };
    let mut text = fill(tr(lang, "At this pace: {what}"), &[("what", &what)]);
    if low {
        text = fill(tr(lang, "{text} (estimate)"), &[("text", &text)]);
    }
    Some((text, tone))
}

/// Short form for status lines: "runs out ~Thu 3:20 PM" or "≈ 82% at reset".
pub fn forecast_short<Tz: TimeZone>(
    lang: Lang,
    w: &LimitWindow,
    now: i64,
    tz: &Tz,
    clock24: bool,
) -> Option<String> {
    let f = shown_forecast(w)?;
    Some(match f.exhaust_at.filter(|_| f.runs_out()) {
        Some(at) => fill(
            tr(lang, "runs out {when}"),
            &[("when", &moment(lang, at, now, tz, clock24, true))],
        ),
        None => at_reset(lang, f),
    })
}

fn at_reset(lang: Lang, f: &crate::forecast::Forecast) -> String {
    let p = format!("{:.0}", f.at_reset_display().round());
    fill(tr(lang, "≈ {p}% at reset"), &[("p", &p)])
}

// ---------------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------------

/// "950", "12.3k", "4.5M", "1.2B".
pub fn tokens(n: u64) -> String {
    let f = n as f64;
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", f / 1e3)
    } else if n < 1_000_000_000 {
        format!("{:.1}M", f / 1e6)
    } else {
        format!("{:.1}B", f / 1e9)
    }
}

/// "$1,155.95" / "€1.23" (en), "1 155,95 $" / "1,23 €" (fr): two decimals, thousands grouped
/// (comma in English, no-break space U+00A0 in French: Segoe UI draws U+202F
/// almost zero-width between digits, so "1 155" would read "1155"). `currency` is "USD" or "EUR".
pub fn money(lang: Lang, amount: f64, currency: &str) -> String {
    let symbol = if currency.eq_ignore_ascii_case("EUR") {
        "€"
    } else {
        "$"
    };
    let amount = if amount.is_finite() { amount } else { 0.0 };
    let fixed = format!("{:.2}", amount.abs());
    let (int, frac) = fixed.split_once('.').unwrap_or((fixed.as_str(), "00"));
    let sign = if amount < 0.0 && fixed != "0.00" {
        "-"
    } else {
        ""
    };
    match lang {
        Lang::En => format!("{sign}{symbol}{}.{frac}", group_thousands(int, ",")),
        Lang::Fr => format!("{sign}{},{frac} {symbol}", group_thousands(int, "\u{a0}")),
    }
}

/// "1234567" → "1,234,567" with `sep` between groups of three digits.
fn group_thousands(digits: &str, sep: &str) -> String {
    let n = digits.len();
    let mut out = String::with_capacity(n + n / 3 * sep.len());
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Colour grading
// ---------------------------------------------------------------------------------------------

/// Severity band of a usage figure. ui-kit maps it to a colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Grade {
    /// below the warning threshold ("ample", green)
    Ok,
    /// warning ≤ used < critical ("watch", yellow)
    Warn,
    /// used ≥ critical
    Critical,
    /// no figure
    Unknown,
}

/// Hard-step grade. `used`, `warn`, `critical` are percents (0–100).
/// Use `Settings::warn_percent()` / `critical_percent()` for the thresholds.
pub fn grade(used: Option<f64>, warn: f64, critical: f64) -> Grade {
    match used.filter(|u| u.is_finite()) {
        None => Grade::Unknown,
        Some(u) if u >= critical => Grade::Critical,
        Some(u) if u >= warn => Grade::Warn,
        Some(_) => Grade::Ok,
    }
}

/// Colour-ramp position: interpolate `from` → `to` by `t` (0–1). Below `warn`: Ok→Warn by
/// used/warn; above: Warn→Critical by (used−warn)/(100−warn). `None` when unknown.
pub fn ramp(used: Option<f64>, warn: f64) -> Option<(Grade, Grade, f32)> {
    let u = used.filter(|u| u.is_finite())?.clamp(0.0, 100.0);
    let warn = warn.clamp(1.0, 99.0);
    if u < warn {
        Some((Grade::Ok, Grade::Warn, (u / warn) as f32))
    } else {
        Some((
            Grade::Warn,
            Grade::Critical,
            ((u - warn) / (100.0 - warn)) as f32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Account;
    use crate::model::{ProviderKind, Reading, WindowKind};
    use chrono::FixedOffset;

    fn with_forecast(used: f64, f: crate::forecast::Forecast) -> LimitWindow {
        let mut w = LimitWindow::new("s", "Current session", WindowKind::Session).used(used);
        w.forecast = Some(f);
        w
    }

    #[test]
    fn forecast_lines() {
        use crate::forecast::{Basis, Confidence, Forecast};
        // Thursday 2027-01-07 10:00 UTC.
        let now = 1_799_316_000;
        let utc = FixedOffset::east_opt(0).unwrap();
        let f = Forecast {
            exhaust_at: Some(now + 5 * 3600 + 20 * 60),
            at_reset_percent: 130.0,
            resets_at: now + 2 * 86_400,
            confidence: Confidence::High,
            basis: Basis::Trajectory,
            computed_at: now,
        };
        let w = with_forecast(50.0, f);
        assert_eq!(
            forecast_card(Lang::Fr, &w, now, &utc, true),
            Some((
                "À ce rythme : épuisé à 15:20".to_string(),
                ForecastTone::Warn
            ))
        );
        assert_eq!(
            forecast_card(Lang::En, &w, now, &utc, false).unwrap().0,
            "At this pace: runs out at 3:20 PM"
        );
        assert_eq!(
            forecast_short(Lang::Fr, &w, now, &utc, true).unwrap(),
            "épuisé vers 15:20"
        );
        // Next day, low confidence.
        let mut g = f;
        g.exhaust_at = Some(now + 29 * 3600 + 20 * 60);
        g.confidence = Confidence::Low;
        let w = with_forecast(50.0, g);
        assert_eq!(
            forecast_card(Lang::Fr, &w, now, &utc, true).unwrap().0,
            "À ce rythme : épuisé ~ven. 15:20 (estimation)"
        );
        assert_eq!(
            forecast_short(Lang::En, &w, now, &utc, false).unwrap(),
            "runs out ~Fri 3:20 PM"
        );
        // Within the hour: critical.
        let mut c = f;
        c.exhaust_at = Some(now + 25 * 60);
        let (text, tone) =
            forecast_card(Lang::En, &with_forecast(90.0, c), now, &utc, false).unwrap();
        assert_eq!(text, "At this pace: runs out in 25 min");
        assert_eq!(tone, ForecastTone::Critical);
        // Fits: percent at reset, muted.
        let mut m = f;
        m.exhaust_at = None;
        m.at_reset_percent = 81.6;
        let w = with_forecast(40.0, m);
        assert_eq!(
            forecast_card(Lang::Fr, &w, now, &utc, true),
            Some((
                "À ce rythme : ≈ 82 % au reset".to_string(),
                ForecastTone::Muted
            ))
        );
        assert_eq!(
            forecast_short(Lang::En, &w, now, &utc, false).unwrap(),
            "≈ 82% at reset"
        );
        // Full window or no forecast: nothing.
        assert_eq!(
            forecast_card(Lang::En, &with_forecast(100.0, f), now, &utc, false),
            None
        );
        let plain = LimitWindow::new("s", "Current session", WindowKind::Session).used(5.0);
        assert_eq!(forecast_short(Lang::En, &plain, now, &utc, false), None);
    }

    #[test]
    fn percents() {
        assert_eq!(percent(None), "—");
        assert_eq!(percent(Some(f64::NAN)), "—");
        assert_eq!(percent(Some(0.0)), "0%");
        assert_eq!(percent(Some(42.4)), "42%");
        assert_eq!(percent(Some(0.44)), "0.4%");
        assert_eq!(percent(Some(0.04)), "<0.1%");
        assert_eq!(percent(Some(99.62)), "99.6%");
        assert_eq!(percent(Some(99.97)), ">99.9%");
        assert_eq!(percent(Some(100.0)), "100%");
    }

    #[test]
    fn figures_and_cells() {
        let w = LimitWindow::new("session", "Current session", WindowKind::Session).used(73.0);
        assert_eq!(window_figure(&w, false), "73%");
        assert_eq!(window_figure(&w, true), "27%");
        assert_eq!(window_figure(&w.clone().derived(), false), "~73%");
        let mut c = LimitWindow::new("c", "Requests", WindowKind::Other);
        c.count = Some(17);
        assert_eq!(window_figure(&c, false), "~17");

        let a = Account::new(ProviderKind::Claude, "c");
        let mut view = AccountView::new(a.clone());
        assert_eq!(cell_figure(&view, false), "…");
        view.reading = Some(Reading::signed_out(&a, 0));
        assert_eq!(cell_figure(&view, false), "—");
        view.reading = Some(Reading::fresh(
            &a,
            0,
            vec![LimitWindow::new("seven_day", "Weekly limit", WindowKind::Weekly).used(9.0)],
        ));
        assert_eq!(cell_figure(&view, false), "9%", "Claude: most-used weekly");
        view.reading = Some(Reading::fresh(
            &a,
            0,
            vec![LimitWindow::new(
                "seven_day",
                "Weekly limit",
                WindowKind::Weekly,
            )],
        ));
        assert_eq!(cell_figure(&view, false), "—", "missing headline is a dash");
        view.reading = Some(Reading::fresh(&a, 0, vec![w]));
        assert_eq!(cell_figure(&view, false), "73%");
    }

    #[test]
    fn used_lines() {
        assert_eq!(
            used_line(Lang::En, Some(73.0), false),
            "73% used · 27% left"
        );
        assert_eq!(
            used_line(Lang::Fr, Some(73.0), false),
            "73 % utilisés · 27 % restants"
        );
        assert_eq!(used_line(Lang::En, None, false), "—");
    }

    #[test]
    fn countdowns() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let now = 1_700_000_000;
        assert_eq!(reset_in_tz(Lang::En, None, now, &utc), None);
        assert_eq!(
            reset_in_tz(Lang::En, Some(now - 1), now, &utc).unwrap(),
            "Resetting…"
        );
        assert_eq!(
            reset_in_tz(Lang::En, Some(now + 2 * 3600 + 14 * 60), now, &utc).unwrap(),
            "in 2 h 14 min"
        );
        assert_eq!(
            reset_in_tz(Lang::Fr, Some(now + 2 * 3600 + 14 * 60), now, &utc).unwrap(),
            "dans 2 h 14 min"
        );
        assert_eq!(
            reset_in_tz(Lang::Fr, Some(now + 3 * 86_400 + 4 * 3600), now, &utc).unwrap(),
            "dans 3 j 4 h"
        );
        assert_eq!(
            reset_in_tz(Lang::En, Some(now + 30), now, &utc).unwrap(),
            "in 1 min"
        );
        // 1_700_000_000 = 2023-11-14 22:13:20 UTC; +10 days = Nov 24
        assert_eq!(
            reset_in_tz(Lang::En, Some(now + 10 * 86_400), now, &utc).unwrap(),
            "on Nov 24"
        );
        assert_eq!(
            reset_in_tz(Lang::Fr, Some(now + 10 * 86_400), now, &utc).unwrap(),
            "le 24 nov."
        );
    }

    #[test]
    fn card_resets() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let now = 1_700_000_000; // Tue 2023-11-14 22:13:20 UTC
        assert_eq!(
            reset_card(Lang::En, Some(now + 14 * 60), now, &utc, false).unwrap(),
            "Resets in 14 min"
        );
        assert_eq!(
            reset_card(Lang::En, Some(now + 3 * 3600), now, &utc, false).unwrap(),
            "Resets at 1:13 AM"
        );
        assert_eq!(
            reset_card(Lang::Fr, Some(now + 3 * 3600), now, &utc, true).unwrap(),
            "Réinit. à 01:13"
        );
        assert_eq!(
            reset_card(Lang::En, Some(now + 2 * 86_400), now, &utc, false).unwrap(),
            "Resets Thu 10:13 PM"
        );
        assert_eq!(
            reset_card(Lang::En, Some(now + 9 * 86_400), now, &utc, false).unwrap(),
            "Resets Nov 23"
        );
    }

    #[test]
    fn ages() {
        assert_eq!(age(Lang::En, 100, 120), "just now");
        assert_eq!(age(Lang::En, 0, 12 * 60), "12m ago");
        assert_eq!(age(Lang::Fr, 0, 12 * 60), "il y a 12 min");
        assert_eq!(age(Lang::En, 0, 2 * 3600), "2h ago");
        assert_eq!(age(Lang::Fr, 0, 3 * 86_400), "il y a 3 j");
    }

    #[test]
    fn grades_and_ramp() {
        assert_eq!(grade(None, 80.0, 95.0), Grade::Unknown);
        assert_eq!(grade(Some(10.0), 80.0, 95.0), Grade::Ok);
        assert_eq!(grade(Some(80.0), 80.0, 95.0), Grade::Warn);
        assert_eq!(grade(Some(95.0), 80.0, 95.0), Grade::Critical);
        assert_eq!(ramp(None, 50.0), None);
        let (a, b, t) = ramp(Some(25.0), 50.0).unwrap();
        assert_eq!((a, b), (Grade::Ok, Grade::Warn));
        assert!((t - 0.5).abs() < 1e-6);
        let (a, b, t) = ramp(Some(100.0), 50.0).unwrap();
        assert_eq!((a, b), (Grade::Warn, Grade::Critical));
        assert!((t - 1.0).abs() < 1e-6);
    }

    #[test]
    fn numbers() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(12_345), "12.3k");
        assert_eq!(tokens(4_500_000), "4.5M");
        assert_eq!(money(Lang::En, 1.234, "USD"), "$1.23");
        assert_eq!(money(Lang::Fr, 1.234, "EUR"), "1,23 €");
        assert_eq!(money(Lang::En, 1155.951, "USD"), "$1,155.95");
        assert_eq!(money(Lang::Fr, 1155.951, "USD"), "1\u{a0}155,95 $");
        assert_eq!(money(Lang::En, 1_234_567.0, "EUR"), "€1,234,567.00");
        assert_eq!(money(Lang::Fr, 999.999, "EUR"), "1\u{a0}000,00 €");
        assert_eq!(money(Lang::En, 999.0, "USD"), "$999.00");
        assert_eq!(money(Lang::En, 0.0, "USD"), "$0.00");
        assert_eq!(money(Lang::En, -1234.5, "USD"), "-$1,234.50");
        assert_eq!(money(Lang::Fr, -0.001, "EUR"), "0,00 €");
        assert_eq!(money(Lang::En, f64::NAN, "USD"), "$0.00");
        assert_eq!(duration(Lang::En, 3600), "1 h");
    }
}
