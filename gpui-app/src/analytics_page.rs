//! The Analytics page of the settings: the quota history in detail (limits reached, time
//! blocked, pace), and the tokens of the local Claude Code and Codex logs (per day, hour,
//! model and project) with their cost at API list prices against the subscriptions.
use crate::analytics;
use crate::chart;
use crate::home::{card, group_label};
use crate::input::TextInput;
use crate::settings::{button, Click, Kind, SettingsView, ACCENT, MUTED};
use crate::token_logs::{Bucket, Cli, Tokens};
use crate::ui::{percentage, provider_icon, t, tokens as format_tokens};
use gpui::{
    canvas, div, fill, point, prelude::*, px, rgb, rgba, size, AnyElement, Bounds, ClickEvent, Context,
    FontWeight, PathBuilder, Rgba, SharedString,
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AnalyticsTab {
    Quotas,
    Tokens,
}

/// Colors of the token kinds, in stacking order (cache reads last, muted: they dwarf the rest).
const KINDS: [(&str, u32); 4] = [
    ("Sortie", 0x28e07b),
    ("Entrée", ACCENT),
    ("Écriture cache", 0xd9a53f),
    ("Lecture cache", 0x4a4a52),
];
const DAYS: [&str; 7] = ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"];

fn kinds(t: &Tokens) -> [u64; 4] {
    [t.output, t.input, t.cache_write, t.cache_read]
}

pub(crate) fn euros(value: f64, french: bool) -> String {
    let text = if value >= 100. { format!("{value:.0}") } else { format!("{value:.2}") };
    if french {
        format!("{} €", text.replace('.', ","))
    } else {
        format!("€{text}")
    }
}

/// Parses a price typed in either locale; empty is `None`.
fn amount(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().trim_end_matches('€').trim().replace(',', ".");
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>().map(Some).map_err(|_| ())
}

fn empty(text: String) -> AnyElement {
    div()
        .py(px(18.))
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text)
        .into_any_element()
}

/// The periods a window's past is counted in: "semaines" for a week.
fn periods_word(label: &str) -> &'static str {
    match analytics::period_length(label) {
        Some(l) if l == 86_400 => "journées",
        Some(l) if l == 7 * 86_400 => "semaines",
        Some(l) if l >= 28 * 86_400 => "mois",
        _ => "périodes",
    }
}

/// How an outlook was made, under its figure: "d'après tes 3 dernières semaines".
pub(crate) fn outlook_basis(o: &analytics::Outlook, label: &str, french: bool) -> String {
    if o.soon && o.exhausts_at.is_some() {
        return t(french, "au rythme de la dernière heure");
    }
    match o.basis {
        analytics::Basis::RecentPace => t(french, "au rythme de la dernière heure"),
        analytics::Basis::PastPeriods(n) => {
            let key = match periods_word(label) {
                "mois" => "d’après tes {n} derniers mois".to_owned(),
                word => format!("d’après tes {{n}} dernières {word}"),
            };
            t(french, &key).replace("{n}", &n.to_string())
        }
        analytics::Basis::ActiveHours => t(french, "au rythme moyen, sur tes heures d’usage habituelles"),
        analytics::Basis::AveragePace => t(french, "au rythme moyen de la période"),
    }
}

/// "entre 86 et 100 %": the lowest and highest the past periods lead to.
pub(crate) fn outlook_range(o: &analytics::Outlook, french: bool) -> Option<String> {
    let (low, high) = o.range?;
    let clamp = |v: f64| v.clamp(0., 100.).round();
    Some(
        t(french, "entre {low} et {high}")
            .replace("{low}", &format!("{}", clamp(low)))
            .replace("{high}", &percentage(clamp(high), french)),
    )
}

/// The line under a window in the notch popover: when its limit comes, or for a window of
/// a day or more, where it should stand at the reset. `true` for a limit.
pub(crate) fn popover_outlook(reading: &crate::model::Reading, label: &str, now: i64, french: bool) -> Option<(String, bool)> {
    let outlook = reading.outlooks.get(label);
    let limit = outlook.map_or_else(|| reading.exhausts_at.get(label).copied(), |o| o.exhausts_at);
    if let Some(at) = limit {
        let time = crate::ui::reset_time(Some(at), now, french);
        let key = match outlook.filter(|o| analytics::long_window(label) && !o.soon).map(|o| o.basis) {
            Some(analytics::Basis::PastPeriods(_)) => format!("Plafond {{time}}, selon tes {}", periods_word(label)),
            Some(analytics::Basis::ActiveHours) => "Plafond {time}, selon tes horaires".to_owned(),
            Some(analytics::Basis::AveragePace) => "Plafond {time} au rythme moyen".to_owned(),
            _ => "Plafond estimé {time} au rythme actuel".to_owned(),
        };
        return Some((t(french, &key).replace("{time}", &time), true));
    }
    let o = outlook.filter(|_| analytics::long_window(label))?;
    let value = percentage(o.expected.min(100.).round(), french);
    let text = match outlook_range(o, french) {
        Some(range) => t(french, "À la remise : ≈ {value}, {range}").replace("{range}", &range),
        None => t(french, "À la remise : ≈ {value}"),
    };
    Some((text.replace("{value}", &value), false))
}

/// One stacked bar per day.
fn day_bars(days: Vec<[u64; 4]>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let max = days.iter().map(|d| d.iter().sum::<u64>()).max().unwrap_or(0).max(1) as f32;
            let slot = bounds.size.width / days.len().max(1) as f32;
            let bar = (slot * 0.7).max(px(2.));
            for (i, day) in days.iter().enumerate() {
                let mut top = bounds.origin.y + bounds.size.height;
                for (value, (_, color)) in day.iter().zip(KINDS) {
                    let height = bounds.size.height * (*value as f32 / max);
                    if height <= px(0.) {
                        continue;
                    }
                    top -= height;
                    let origin = point(bounds.origin.x + slot * i as f32 + (slot - bar) / 2., top);
                    window.paint_quad(fill(Bounds { origin, size: size(bar, height) }, rgb(color)));
                }
            }
        },
    )
    .w_full()
    .h(px(110.))
}

impl SettingsView {
    pub(crate) fn analytics_heading(&self, french: bool) -> (String, String) {
        (
            t(french, "Analytics"),
            t(french, "Ton rythme de consommation et ce qu’il vaut au tarif API. Tout est calculé sur ce PC."),
        )
    }

    /// Asks for a scan of the logs when the Tokens tab has nothing yet, or a report older
    /// than ten minutes.
    fn ensure_tokens(&self, cx: &mut Context<Self>) {
        let s = self.store.read(cx);
        let now = chrono::Utc::now().timestamp();
        if !s.scanning && s.tokens.as_ref().is_none_or(|r| now - r.scanned_at > 600) {
            let store = self.store.clone();
            cx.spawn(async move |_, cx| crate::store::scan_tokens(&store, cx).await).detach();
        }
    }

    /// The two tabs of the page, underlined like a pivot; the filters sit at the end of the
    /// same line.
    fn analytics_tabs(&self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let tabs = [(AnalyticsTab::Quotas, "Quotas"), (AnalyticsTab::Tokens, "Tokens")];
        div()
            .flex()
            .gap(px(22.))
            .children(tabs.into_iter().map(|(tab, label)| {
                let active = self.analytics_tab == tab;
                div()
                    .id(SharedString::from(format!("analytics-tab-{label}")))
                    .pb(px(10.))
                    .border_b(px(2.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(active, |d| d.text_color(rgb(0xffffff)).border_color(rgb(0xffffff)))
                    .when(!active, |d| {
                        d.text_color(rgb(0xa1a1aa))
                            .border_color(rgba(0x00000000))
                            .hover(|d| d.text_color(rgb(0xffffff)))
                    })
                    .child(t(french, label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.analytics_tab = tab;
                        this.message = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    /// Small segmented control, as the Home page range.
    fn segments(&self, id: &str, options: Vec<(String, bool, Click)>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(28.))
            .p(px(2.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(0x414147))
            .bg(rgb(0x1c1c1f))
            .children(options.into_iter().enumerate().map(|(i, (label, on, click))| {
                div()
                    .id(SharedString::from(format!("{id}-{i}")))
                    .flex()
                    .items_center()
                    .h_full()
                    .px(px(8.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .when(on, |d| d.bg(rgb(0x3a3a3f)).text_color(rgb(0xffffff)))
                    .when(!on, |d| {
                        d.text_color(rgb(0x9a9aa3))
                            .cursor_pointer()
                            .hover(|d| d.text_color(rgb(0xffffff)))
                            .on_click(click)
                    })
                    .child(label)
            }))
            .into_any_element()
    }

    pub(crate) fn analytics_page(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let days = self.store.read(cx).config.settings.analytics_days;
        let range = self.segments(
            "analytics-range",
            [(7u16, "7 j"), (30, "30 j")]
                .into_iter()
                .map(|(value, label)| (t(french, label), value == days, self.setting(cx, move |s| s.analytics_days = value)))
                .collect(),
        );
        let mut toolbar = div().flex().items_center().gap(px(10.)).pb(px(8.)).child(range);
        if self.analytics_tab == AnalyticsTab::Tokens {
            let cli = self.analytics_cli;
            let filter = self.segments(
                "analytics-cli",
                [(None, "Tous"), (Some(Cli::Claude), "Claude Code"), (Some(Cli::Codex), "Codex")]
                    .into_iter()
                    .map(|(value, label)| -> (String, bool, Click) {
                        (
                            t(french, label),
                            value == cli,
                            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.analytics_cli = value;
                                cx.notify();
                            })),
                        )
                    })
                    .collect(),
            );
            let scanning = self.store.read(cx).scanning;
            toolbar = toolbar.child(filter).child(button(
                "analytics-rescan",
                Kind::Secondary,
                Some(if scanning { "loader-circle" } else { "refresh-cw" }),
                if scanning { t(french, "Analyse…") } else { t(french, "Actualiser") },
                (!scanning).then(|| -> Click {
                    let store = self.store.clone();
                    Box::new(move |_, _, cx| {
                        let store = store.clone();
                        cx.spawn(async move |cx| crate::store::scan_tokens(&store, cx).await).detach();
                    })
                }),
            ));
        }
        let header = div()
            .flex()
            .items_end()
            .justify_between()
            .gap(px(16.))
            .border_b_1()
            .border_color(rgba(0xffffff12))
            .child(self.analytics_tabs(french, cx))
            .child(toolbar);
        let mut out = vec![header.into_any_element()];
        match self.analytics_tab {
            AnalyticsTab::Quotas => out.extend(self.quotas_tab(french, days, cx)),
            AnalyticsTab::Tokens => {
                self.ensure_tokens(cx);
                out.extend(self.tokens_tab(french, days, cx));
            }
        }
        out
    }

    /// The account whose windows the Quotas tab details: the chosen one while it is still
    /// tracked, else the first.
    fn analytics_account(&self, cx: &Context<Self>) -> Option<crate::model::Account> {
        let accounts = self.store.read(cx).enabled();
        accounts
            .iter()
            .find(|a| self.analytics_account.as_deref() == Some(a.id.as_str()))
            .or(accounts.first())
            .cloned()
    }

    /// One chip per tracked account: its most used window now, lit when selected.
    fn account_picker(&self, selected: &str, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let chips: Vec<AnyElement> = s
            .enabled()
            .into_iter()
            .map(|account| {
                let active = account.id == selected;
                let headline = crate::ui::headline(s.readings.get(&account.id));
                let used = headline.map(|q| q.used_percent);
                let color = chart::tone(used);
                let id = account.id.clone();
                div()
                    .id(SharedString::from(format!("analytics-account-{}", account.id)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(12.))
                    .pr(px(14.))
                    .py(px(8.))
                    .rounded(px(8.))
                    .border_1()
                    .cursor_pointer()
                    .when(active, |d| d.bg(rgba(0xffffff0c)).border_color(rgba(0xffffff1f)))
                    .when(!active, |d| d.border_color(rgba(0xffffff0f)).hover(|d| d.bg(rgba(0xffffff06))))
                    .child(provider_icon(&account.provider, 16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if active { rgb(0xfafafa) } else { rgb(0xc3c3c8) })
                                    .child(account.name.clone()),
                            )
                            .child(div().truncate().text_size(px(11.)).text_color(rgb(MUTED)).child(match headline {
                                Some(q) => t(french, &q.label),
                                None => t(french, "Pas encore de relevé"),
                            })),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(used.map_or("—".to_owned(), |u| percentage(u, french).replace(" %", "%"))),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.analytics_account = Some(id.clone());
                        this.chart_hover = None;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        div().flex().flex_wrap().gap(px(6.)).children(chips).into_any_element()
    }

    fn quotas_tab(&self, french: bool, days: u16, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(account) = self.analytics_account(cx) else {
            return vec![empty(t(french, "Aucun compte suivi."))];
        };
        let picker = self.account_picker(&account.id, french, cx);
        let compact = self.viewport_width < px(1000.);
        let s = self.store.read(cx);
        let now = chrono::Utc::now().timestamp();
        let since = now - days as i64 * 86_400;
        let view = cx.entity().downgrade();

        let mut windows: Vec<_> = s.history.windows().filter(|(a, _, _)| *a == account.id).collect();
        windows.sort_by(|a, b| a.1.cmp(b.1));
        // The outlook of every window, the one of the last reading when there is one so the
        // notch and this page agree; it also names the nearest limit in the summary.
        let reading = s.readings.get(&account.id);
        let outlooks: Vec<(&str, Option<analytics::Outlook>)> = windows
            .iter()
            .map(|(_, label, samples)| {
                let saved = reading.and_then(|r| r.outlooks.get(*label).copied());
                (*label, saved.or_else(|| analytics::outlook(label, samples, now)))
            })
            .collect();
        let nearest_limit = outlooks
            .iter()
            .filter_map(|(label, o)| Some((*label, (*o)?.exhausts_at?)))
            .min_by_key(|(_, at)| *at);
        let limits: u32 = windows
            .iter()
            .map(|(_, label, _)| analytics::quota_stats(s.history.series(&account.id, label, since), since).limits)
            .sum();

        let summary = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.))
            .px(px(22.))
            .pt(px(20.))
            .pb(px(6.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(account.name.clone()))
                    .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(if windows.is_empty() {
                        t(french, "Pas encore de relevé : la courbe se dessine dès les premiers relevés réussis.")
                    } else {
                        match windows.len() {
                            1 => t(french, "1 fenêtre de quota suivie"),
                            n => t(french, "{count} fenêtres de quota suivies").replace("{count}", &n.to_string()),
                        }
                    })),
            )
            .children((!windows.is_empty()).then(|| {
                let (text, color) = match nearest_limit {
                    Some((label, at)) => (
                        t(french, "Plafond {window} prévu {when}")
                            .replace("{window}", &t(french, label))
                            .replace("{when}", &moment(at, now, french)),
                        0xf9a58f,
                    ),
                    None => (t(french, "Aucun plafond prévu avant les remises à zéro"), 0x6ee7b7),
                };
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(3.))
                    .text_size(px(12.))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(rgb(color)).child(text))
                    .child(div().text_size(px(11.)).text_color(rgb(0x8a8a93)).child(
                        match limits {
                            0 => t(french, "Aucune limite atteinte en {days} j"),
                            1 => t(french, "1 limite atteinte en {days} j"),
                            _ => t(french, "{count} limites atteintes en {days} j").replace("{count}", &limits.to_string()),
                        }
                        .replace("{days}", &days.to_string()),
                    ))
            }));

        let mut lanes: Vec<AnyElement> = vec![];
        for ((_, label, samples), (_, outlook)) in windows.iter().zip(&outlooks) {
            let stats = analytics::quota_stats(s.history.series(&account.id, label, since), since);
            let Some(outlook) = *outlook else {
                lanes.push(lane(compact,
                    window_identity(label, samples.last().map(|p| p.used), None, french),
                    empty(t(french, "Ce fournisseur ne donne pas la date de remise à zéro de cette fenêtre : pas de budget à comparer.")),
                    stats_column(&stats, days, french).unwrap_or_else(|| div().into_any_element()),
                ));
                continue;
            };
            let budget = outlook.budget;
            let points: Vec<chart::Point> = samples
                .iter()
                .filter(|p| p.at >= budget.start && p.at <= budget.at)
                .filter(|p| crate::alerts::same_period(p.resets_at, Some(budget.end)))
                .map(|p| chart::Point { at: p.at, value: p.used })
                .collect();
            let color = chart::tone(Some(budget.used));
            let key = format!("{}-{label}", account.id);
            let hovered = self.chart_hover.as_ref().filter(|(k, _)| *k == key).map(|(_, i)| *i);
            let on_hover: chart::OnHover = {
                let (view, key) = (view.clone(), key.clone());
                std::rc::Rc::new(move |index, _, cx| {
                    let key = key.clone();
                    let _ = view.update(cx, |this, cx| {
                        let next = index.map(|i| (key.clone(), i));
                        // Leaving one chart must not clear the hover of another.
                        if next.is_some() || this.chart_hover.as_ref().is_some_and(|(k, _)| *k == key) {
                            this.chart_hover = next;
                            cx.notify();
                        }
                    });
                })
            };
            let chart = chart::BudgetChart {
                id: SharedString::from(format!("chart-{key}")),
                budget,
                expected: outlook.expected,
                exhausts_at: outlook.exhausts_at,
                range: outlook.range,
                points,
                color,
                label: t(french, label),
                hovered,
                on_hover,
                value_text: std::rc::Rc::new(move |v| percentage(v, french)),
                time_text: std::rc::Rc::new(move |at| moment(at, now, french)),
                start_caption: t(french, "Début {when}").replace("{when}", &moment(budget.start, now, french)),
                end_caption: t(french, "Remise à zéro {when}").replace("{when}", &moment(budget.end, now, french)),
            }
            .render();
            lanes.push(lane(compact,
                window_identity(label, Some(budget.used), Some(&budget), french),
                chart,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(verdict(&outlook, label, now, french))
                    .children(stats_column(&stats, days, french))
                    .into_any_element(),
            ));
        }
        let lanes: Vec<AnyElement> = lanes
            .into_iter()
            .map(|l| div().border_t_1().border_color(rgba(0xffffff0f)).child(l).into_any_element())
            .collect();
        vec![
            picker,
            div()
                .rounded(px(12.))
                .border_1()
                .border_color(rgba(0xffffff12))
                .bg(rgba(0xffffff04))
                .flex()
                .flex_col()
                .child(summary)
                .children(lanes)
                .into_any_element(),
            legend(french),
        ]
    }

    fn tokens_tab(&mut self, french: bool, days: u16, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx);
        let compact = self.viewport_width < px(1000.);
        let Some(report) = s.tokens.clone() else {
            return vec![empty(t(french, "Analyse des logs de Claude Code et Codex…"))];
        };
        let now = chrono::Utc::now().timestamp();
        let since = now - days as i64 * 86_400;
        let filter = self.analytics_cli;
        let buckets: Vec<Bucket> = report
            .buckets
            .iter()
            .filter(|b| b.hour >= since && filter.is_none_or(|c| b.cli == c))
            .cloned()
            .collect();
        let sources: Vec<_> = report
            .sources
            .iter()
            .filter(|(src, _)| filter.is_none_or(|c| src.cli == c))
            .cloned()
            .collect();
        let missing: Vec<String> = sources
            .iter()
            .filter(|(_, found)| !found)
            .map(|(src, _)| t(french, "Aucun log local trouvé dans {path}.").replace("{path}", &src.dir.display().to_string()))
            .collect();
        if sources.is_empty() {
            return vec![empty(t(french, "Connecte un compte Claude Code ou Codex : leurs logs locaux alimentent cet onglet."))];
        }

        let mut total = Tokens::default();
        for b in &buckets {
            total += b.tokens;
        }
        let pricing = &s.pricing;
        let rate = s.config.settings.usd_to_eur;
        let (usd, unpriced) = analytics::cost(buckets.iter(), pricing);
        // One subscription per log folder: accounts sharing a profile are one subscription.
        let accounts = s.config.accounts.clone();
        let mut subscription = None::<f64>;
        for (src, _) in &sources {
            let Some(account) = src.accounts.first().and_then(|id| accounts.iter().find(|a| &a.id == id)) else {
                continue;
            };
            let price = s.config.settings.plan_prices_eur.get(&account.id).copied().or_else(|| {
                s.plan_suggestions_usd.get(&account.id).map(|usd| usd * rate)
            });
            if let Some(p) = price {
                *subscription.get_or_insert(0.) += p * days as f64 / 30.;
            }
        }
        let cost_eur = usd * rate;
        let verdict = subscription.filter(|p| *p > 0.).map(|p| {
            let ratio = cost_eur / p;
            if ratio >= 1. {
                t(french, "{ratio}× la valeur de tes abonnements sur la période")
                    .replace("{ratio}", &format!("{ratio:.1}").replace('.', if french { "," } else { "." }))
            } else {
                t(french, "Tes abonnements coûtent plus que cet usage au tarif API ({share} de leur prix)")
                    .replace("{share}", &percentage(ratio * 100., french))
            }
        });

        let summary = row(compact, vec![
            card()
                .gap(px(6.))
                .child(group_label(t(french, "Tokens")))
                .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(format_tokens(total.total(), true, french)))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                    t(french, "{output} en sortie · {input} en entrée · {cache} lus en cache")
                        .replace("{output}", &format_tokens(total.output, true, french))
                        .replace("{input}", &format_tokens(total.input + total.cache_write, true, french))
                        .replace("{cache}", &format_tokens(total.cache_read, true, french)),
                ))
                .into_any_element(),
            card()
                .gap(px(6.))
                .child(group_label(t(french, "Coût au tarif API")))
                .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(euros(cost_eur, french)))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(if unpriced > 0 {
                    t(french, "+ {count} tokens de modèles sans tarif, non estimés")
                        .replace("{count}", &format_tokens(unpriced, true, french))
                } else {
                    t(french, "Tous les modèles ont un tarif connu")
                }))
                .into_any_element(),
            card()
                .gap(px(6.))
                .child(group_label(t(french, "Abonnements")))
                .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(
                    subscription.map_or("—".to_owned(), |p| euros(p, french)),
                ))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(
                    verdict.unwrap_or_else(|| t(french, "Indique le prix de tes abonnements plus bas pour comparer")),
                ))
                .into_any_element(),
        ]);

        // Per day, stacked by kind.
        let per_day = analytics::daily(buckets.iter(), days, now);
        let first = per_day.first().map(|(d, _)| d.format("%d/%m").to_string()).unwrap_or_default();
        let legend = div().flex().flex_wrap().gap(px(14.)).children(KINDS.iter().zip(kinds(&total)).map(|((label, color), n)| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(rgb(0xc3c3c8))
                .child(div().size(px(8.)).rounded(px(2.)).bg(rgb(*color)))
                .child(format!("{} {}", t(french, label), format_tokens(n, true, french)))
        }));
        let daily_card = card()
            .gap(px(10.))
            .child(group_label(t(french, "Par jour")))
            .child(day_bars(per_day.iter().map(|(_, t)| kinds(t)).collect()))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(first)
                    .child(t(french, "aujourd’hui")),
            )
            .child(legend);

        // When: weekday × hour.
        let heat = analytics::heatmap(buckets.iter());
        let max = heat.iter().flatten().copied().max().unwrap_or(0).max(1);
        let hovered = self.heat_hover;
        let caption = match hovered {
            Some((d, h)) => t(french, "{day} {hour} h : {tokens} tokens")
                .replace("{day}", &t(french, DAYS[d]))
                .replace("{hour}", &h.to_string())
                .replace("{tokens}", &format_tokens(heat[d][h], true, french)),
            None => t(french, "Survole une case pour voir son total."),
        };
        let mut rows = vec![];
        for (d, row) in heat.iter().enumerate() {
            let cells: Vec<AnyElement> = row
                .iter()
                .enumerate()
                .map(|(h, value)| {
                    let share = *value as f32 / max as f32;
                    let alpha = if *value == 0 { 0x0a } else { (0x30 as f32 + share * 0xcf as f32) as u32 };
                    div()
                        .id(SharedString::from(format!("heat-{d}-{h}")))
                        .flex_1()
                        .h(px(16.))
                        .rounded(px(3.))
                        .bg(rgba(if *value == 0 { 0xffffff00 } else { 0x28e07b00 } | alpha))
                        .when(hovered == Some((d, h)), |c| c.border_1().border_color(rgb(0xffffff)))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.heat_hover = Some((d, h));
                            } else if this.heat_hover == Some((d, h)) {
                                this.heat_hover = None;
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                })
                .collect();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().w(px(30.)).flex_none().text_size(px(11.)).text_color(rgb(MUTED)).child(t(french, DAYS[d])))
                    .children(cells),
            );
        }
        let hours = div()
            .flex()
            .pl(px(33.))
            .text_size(px(11.))
            .text_color(rgb(MUTED))
            .children((0..24).step_by(6).map(|h| div().flex_1().child(format!("{h} h"))));
        let heat_card = card()
            .gap(px(5.))
            .child(group_label(t(french, "Quand (heure locale)")))
            .children(rows)
            .child(hours)
            .child(div().pt(px(4.)).text_size(px(11.)).text_color(rgb(0xc3c3c8)).child(caption));

        // Rankings.
        let ranking = |title: String, rows: Vec<(String, String, Tokens, Option<f64>)>| {
            let top = rows.first().map(|r| r.2.total()).unwrap_or(1).max(1);
            card()
                .gap(px(9.))
                .child(group_label(title))
                .children(rows.into_iter().take(8).map(|(name, detail, tokens, usd)| {
                    let share = tokens.total() as f32 / top as f32;
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(11.5))
                                .child(div().flex_1().min_w_0().truncate().child(name))
                                .child(div().text_color(rgb(0xc3c3c8)).child(format_tokens(tokens.total(), true, french)))
                                .child(
                                    div()
                                        .w(px(64.))
                                        .flex()
                                        .justify_end()
                                        .text_color(rgb(MUTED))
                                        .child(usd.map_or(t(french, "non estimé"), |u| euros(u * rate, french))),
                                ),
                        )
                        .child(
                            div()
                                .h(px(3.))
                                .w_full()
                                .rounded_full()
                                .bg(rgba(0xffffff0d))
                                .child(div().h_full().w(gpui::relative(share)).rounded_full().bg(rgb(ACCENT))),
                        )
                        .when(!detail.is_empty(), |d| {
                            d.child(div().text_size(px(11.)).text_color(rgb(0x8a8a93)).truncate().child(detail))
                        })
                }))
                .into_any_element()
        };
        let models = analytics::ranked(buckets.iter(), |b| b.model.clone())
            .into_iter()
            .map(|(model, tokens)| {
                let usd = pricing.cost_usd(&tokens, &model);
                (model, String::new(), tokens, usd)
            })
            .collect();
        let mut by_project: Vec<(String, Tokens, f64, bool)> = vec![];
        for (project, _) in analytics::ranked(buckets.iter(), |b| b.project.clone()) {
            let of_project = buckets.iter().filter(|b| b.project == project);
            let mut tokens = Tokens::default();
            for b in of_project.clone() {
                tokens += b.tokens;
            }
            let (usd, unpriced) = analytics::cost(of_project, pricing);
            by_project.push((project, tokens, usd, unpriced == 0));
        }
        let projects = by_project
            .into_iter()
            .map(|(path, tokens, usd, priced)| {
                let name = if path.is_empty() { t(french, "Dossier inconnu") } else { analytics::project_name(&path) };
                (name, path, tokens, priced.then_some(usd))
            })
            .collect();
        let rankings = row(compact, vec![ranking(t(french, "Par modèle"), models), ranking(t(french, "Par projet"), projects)]);

        let prices = self.prices_card(french, &sources, cx);
        let footer = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .text_size(px(11.))
            .text_color(rgb(MUTED))
            .child(
                t(french, "Coûts estimés aux tarifs API publics, convertis à 1 $ = {rate} €. Un modèle sans tarif n’est jamais compté à zéro : ajoute-le dans pricing.json, à côté de la configuration.")
                    .replace("{rate}", &format!("{rate:.2}").replace('.', if french { "," } else { "." })),
            )
            .children(missing)
            .when(report.skipped > 0, |d| {
                d.child(t(french, "{count} lignes de log illisibles ignorées.").replace("{count}", &report.skipped.to_string()))
            });

        if buckets.is_empty() {
            return vec![
                empty(t(french, "Aucune requête dans les logs locaux sur cette période.")),
                prices,
                footer.into_any_element(),
            ];
        }
        vec![
            summary,
            row(compact, vec![daily_card.into_any_element(), heat_card.into_any_element()]),
            rankings,
            prices,
            footer.into_any_element(),
        ]
    }

    /// The monthly price of each subscription, prefilled from the detected plan, and the
    /// dollar to euro rate.
    fn prices_card(&mut self, french: bool, sources: &[(crate::token_logs::Source, bool)], cx: &mut Context<Self>) -> AnyElement {
        let (accounts, saved, rate, suggestions) = {
            let s = self.store.read(cx);
            (
                s.config.accounts.clone(),
                s.config.settings.plan_prices_eur.clone(),
                s.config.settings.usd_to_eur,
                s.plan_suggestions_usd.clone(),
            )
        };
        let mut rows = vec![];
        for (src, _) in sources {
            let Some(account) = src.accounts.first().and_then(|id| accounts.iter().find(|a| &a.id == id)) else {
                continue;
            };
            let suggested = suggestions.get(&account.id).map(|u| u * rate);
            let input = self
                .price_inputs
                .entry(account.id.clone())
                .or_insert_with(|| {
                    let text = saved.get(&account.id).map(|p| format!("{p}")).unwrap_or_default();
                    cx.new(|cx| {
                        let mut input = TextInput::new("", cx);
                        input.set_text(text, cx);
                        input
                    })
                })
                .clone();
            let placeholder = suggested.map_or(t(french, "Prix mensuel en €"), |p| {
                t(french, "Proposé : {price}").replace("{price}", &euros(p, french))
            });
            input.update(cx, |i, cx| i.set_placeholder(placeholder, cx));
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(provider_icon(&account.provider, 15.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).child(account.name.clone()))
                    .child(div().w(px(180.)).child(input))
                    .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(t(french, "€ / mois"))),
            );
        }
        let rate_input = self
            .rate_input
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut input = TextInput::new("0,92", cx);
                    input.set_text(format!("{rate}"), cx);
                    input
                })
            })
            .clone();
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().text_size(px(12.)).child(t(french, "Taux de conversion : 1 $ =")))
                .child(div().w(px(180.)).child(rate_input))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child("€")),
        );
        card()
            .gap(px(10.))
            .child(group_label(t(french, "Prix des abonnements")))
            .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(t(
                french,
                "Laisse vide pour garder le prix proposé d’après ton forfait. Sert uniquement à la comparaison.",
            )))
            .children(rows)
            .child(div().flex().justify_end().child(button(
                "analytics-save-prices",
                Kind::Secondary,
                None,
                t(french, "Enregistrer"),
                Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| this.save_prices(french, cx)))),
            )))
            .into_any_element()
    }

    fn save_prices(&mut self, french: bool, cx: &mut Context<Self>) {
        let mut prices = std::collections::BTreeMap::new();
        for (id, input) in &self.price_inputs {
            match amount(&input.read(cx).text()) {
                Ok(Some(p)) if (0.0..=1000.0).contains(&p) => {
                    prices.insert(id.clone(), p);
                }
                Ok(None) => {}
                _ => return self.say(false, t(french, "Indique un prix entre 0 et 1000 €."), cx),
            }
        }
        let rate = match self.rate_input.as_ref().map(|i| amount(&i.read(cx).text())) {
            Some(Ok(Some(r))) if (0.5..=2.0).contains(&r) => r,
            Some(Ok(None)) | None => 0.92,
            _ => return self.say(false, t(french, "Indique un taux entre 0,5 et 2."), cx),
        };
        self.update_settings(cx, move |s| {
            s.plan_prices_eur = prices;
            s.usd_to_eur = rate;
        });
    }
}

/// A time said the way people say it: today, tomorrow or yesterday with the hour, and the
/// weekday with its date beyond, so the two ends of a week-long period never read alike.
fn moment(at: i64, now: i64, french: bool) -> String {
    use chrono::{Datelike, TimeZone};
    let (Some(when), Some(today)) = (
        chrono::Local.timestamp_opt(at, 0).single(),
        chrono::Local.timestamp_opt(now, 0).single(),
    ) else {
        return String::new();
    };
    let clock = when.format("%H:%M");
    match (when.date_naive() - today.date_naive()).num_days() {
        0 => format!("{} {clock}", t(french, "aujourd’hui")),
        1 => format!("{} {clock}", t(french, "demain")),
        -1 => format!("{} {clock}", t(french, "hier")),
        _ => format!(
            "{} {} {clock}",
            t(french, DAYS[when.weekday().num_days_from_monday() as usize]).to_lowercase(),
            when.format("%d/%m")
        ),
    }
}

/// One window: its usage on the left, its period in the middle, the outlook on the right.
/// In a narrow window the usage and the outlook share a row above the full-width chart.
fn lane(compact: bool, left: AnyElement, center: AnyElement, right: AnyElement) -> AnyElement {
    let base = div().px(px(22.)).py(px(20.));
    if compact {
        return base
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(left)
                    .child(div().w(px(176.)).flex_none().child(right)),
            )
            .child(center)
            .into_any_element();
    }
    base.flex()
        .items_start()
        .gap(px(28.))
        .child(div().w(px(168.)).flex_none().child(left))
        .child(div().flex_1().min_w_0().child(center))
        .child(div().w(px(176.)).flex_none().child(right))
        .into_any_element()
}

/// Side by side when there is room, stacked when the window is narrow.
fn row(compact: bool, items: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .when(compact, |d| d.flex_col())
        .gap(px(12.))
        .children(items)
        .into_any_element()
}

/// The window's name, its usage now and how it compares with an even pace.
fn window_identity(label: &str, used: Option<f64>, budget: Option<&analytics::Budget>, french: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(rgb(0xc3c3c8)).child(t(french, label)))
        .child(
            div()
                .text_size(px(28.))
                .line_height(px(30.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(used.map_or(rgb(0x8a8a93), |u| chart::tone(Some(u))))
                .child(used.map_or("—".to_owned(), |u| percentage(u, french))),
        )
        .children(budget.map(|b| pace_line(b, french)))
        .into_any_element()
}

/// How far usage is from the even pace, in points.
fn pace_line(budget: &analytics::Budget, french: bool) -> AnyElement {
    let delta = budget.used - budget.expected;
    let points = format!("{:.0}", delta.abs());
    let (text, color) = if delta > 2. {
        (t(french, "{n} pts au-dessus du rythme régulier").replace("{n}", &points), 0xf9a58f)
    } else if delta < -2. {
        (t(french, "{n} pts sous le rythme régulier").replace("{n}", &points), 0x6ee7b7)
    } else {
        (t(french, "Au rythme régulier"), MUTED)
    };
    div().text_size(px(11.5)).text_color(rgb(color)).child(text).into_any_element()
}

/// The outlook: when the limit comes if before the reset, else the usage at the reset and
/// the range the past periods give, with how it was estimated.
fn verdict(o: &analytics::Outlook, label: &str, now: i64, french: bool) -> AnyElement {
    let (caption, value, color, range) = match o.exhausts_at {
        Some(at) => (t(french, "Plafond prévu"), moment(at, now, french), rgb(0xf9735b), None),
        None => (
            t(french, "À la remise à zéro"),
            format!("≈ {}", percentage(o.expected.min(100.).round(), french)),
            chart::tone(Some(o.expected)),
            outlook_range(o, french),
        ),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(caption))
        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(value))
        .children(range.map(|r| div().text_size(px(11.)).text_color(rgb(0xc3c3c8)).child(r)))
        .child(div().text_size(px(11.)).text_color(rgb(0x8a8a93)).child(outlook_basis(o, label, french)))
        .into_any_element()
}

/// Limits reached and time blocked in the period, when there were any.
fn stats_column(stats: &analytics::QuotaStats, days: u16, french: bool) -> Option<AnyElement> {
    if stats.limits == 0 && stats.blocked == 0 {
        return None;
    }
    let blocked = if stats.blocked == 0 { "0 min".to_owned() } else { crate::alerts::duration(stats.blocked, french) };
    let line = |text: String| div().child(text);
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .text_size(px(11.))
        .text_color(rgb(0x8f8f96))
        .when(stats.limits > 0, |d| {
            d.child(line(
                match stats.limits {
                    1 => t(french, "1 limite atteinte en {days} j"),
                    _ => t(french, "{count} limites atteintes en {days} j").replace("{count}", &stats.limits.to_string()),
                }
                .replace("{days}", &days.to_string()),
            ))
        })
        .when(stats.blocked > 0, |d| d.child(line(t(french, "{time} bloqué à 100 %").replace("{time}", &blocked))))
        .into_any_element()
        .into()
}

/// What the lines of the charts mean.
fn legend(french: bool) -> AnyElement {
    let sample = |dash: Option<[f32; 2]>, color: Rgba| {
        canvas(|_, _, _| {}, move |bounds, _, window, _| {
            let mut b = PathBuilder::stroke(px(1.5));
            if let Some([on, off]) = dash {
                b = b.dash_array(&[px(on), px(off)]);
            }
            let y = bounds.origin.y + bounds.size.height / 2.;
            b.move_to(point(bounds.origin.x, y));
            b.line_to(point(bounds.origin.x + bounds.size.width, y));
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        })
        .w(px(22.))
        .h(px(8.))
    };
    // A short vertical bar with caps, as the chart draws the range at the reset.
    let range_sample = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let x = bounds.origin.x + bounds.size.width / 2.;
        let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
        let color = rgba(0x34d399b3);
        for (from, to) in [
            (point(x, top), point(x, bottom)),
            (point(x - px(3.), top), point(x + px(3.), top)),
            (point(x - px(3.), bottom), point(x + px(3.), bottom)),
        ] {
            let mut b = PathBuilder::stroke(px(1.5));
            b.move_to(from);
            b.line_to(to);
            if let Ok(path) = b.build() {
                window.paint_path(path, color);
            }
        }
    })
    .w(px(22.))
    .h(px(12.));
    let item = |line: AnyElement, text: String| div().flex().items_center().gap(px(7.)).child(line).child(text);
    div()
        .flex()
        .flex_wrap()
        .gap(px(18.))
        .text_size(px(11.))
        .text_color(rgb(0x8f8f96))
        .child(item(sample(None, rgb(0x34d399)).into_any_element(), t(french, "relevés")))
        .child(item(sample(Some([1.5, 3.5]), rgba(0x34d3998c)).into_any_element(), t(french, "avant le premier relevé")))
        .child(item(sample(Some([4., 4.]), rgba(0x34d399b3)).into_any_element(), t(french, "prévision jusqu’à la remise à zéro")))
        .child(item(range_sample.into_any_element(), t(french, "fourchette des périodes passées")))
        .child(item(sample(Some([5., 5.]), rgba(0xffffff4d)).into_any_element(), t(french, "rythme régulier jusqu’à la remise à zéro")))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_are_read_in_either_locale() {
        assert_eq!(amount(" 184,00 € "), Ok(Some(184.)));
        assert_eq!(amount("18.4"), Ok(Some(18.4)));
        assert_eq!(amount(""), Ok(None));
        assert!(amount("abc").is_err());
        assert_eq!(euros(184., true), "184 €");
        assert_eq!(euros(4.5, false), "€4.50");
    }
}
