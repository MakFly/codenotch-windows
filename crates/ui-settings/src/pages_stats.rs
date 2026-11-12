//! The statistics page ("Statistiques") in the shadcn/ui `dashboard-01` look: filters row
//! (period toggle group, provider, model), `SectionCards`-style KPI cards, a compact card of
//! secondary figures, the "Évolution" area chart (`ChartAreaInteractive`), the activity
//! heatmap, by-model shares, the hours grid and the plans. Refresh lives in the header bar
//! ([`SettingsView::stats_refresh_button`]). Every figure comes from `crate::stats` over
//! `AnalyticsSummary::detail`, filtered client-side by the page's local
//! [`StatsUi`](crate::stats::StatsUi).

use crate::i18n::{t, tf};
use crate::logic::{self, AnalyticsState, Edit};
use crate::stats::{self, HeatCell, Kpis, Metric, SeriesKey, StatsUi, Trend};
use crate::view::SettingsView;
use crate::widgets::{
    caption, card, glyph, note, Btn, BtnKind, ComboBox, Segmented, CAPTION, DESC, FONT,
};
use codenotch_core::analytics::AnalyticsDetail;
use codenotch_core::format;
use codenotch_core::{AnalyticsSummary, Command, Lang, Snapshot};
use codenotch_ui_kit::metrics::SETTINGS_STATS_MAX_W;
use codenotch_ui_kit::{AreaChart, ChartSeries, HoverGrid, Theme, TipContent, TipRow};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, linear_color_stop, linear_gradient, px, relative, AnyElement, Context, Div, FontWeight,
    IntoElement, ParentElement, SharedString, Styled, Window,
};

/// Gap between cards of a row / grid (shadcn `gap-4`).
const GAP: f32 = 16.0;
/// Padding inside a card (shadcn `px-6` / `py-6`).
const CARD_PAD: f32 = 24.0;
/// Width of the weekday labels left of the heat grids.
const ROW_LABEL_W: f32 = 30.0;
/// Row height of the hours grid (one caption line).
const ROW_H: f32 = 18.0;
/// Weekday / month labels of the heat grids.
const HEAT_LABEL: f32 = 11.0;
/// Models listed in "By model" (the rest is summed up in a caption).
const MODEL_ROWS: usize = 8;
/// Card title (`CardTitle`, 16 px semibold) and description (14 px muted).
const CARD_TITLE: f32 = 16.0;

fn s(text: impl Into<String>) -> SharedString {
    SharedString::from(text.into())
}

/// Card header (shadcn `CardHeader`): title (16 px semibold) and an optional muted
/// description, with an optional control (`CardAction`) on the right.
fn card_header(
    t_: &Theme,
    title: String,
    desc: Option<String>,
    control: Option<AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(16.))
        .px(px(CARD_PAD))
        .pt(px(CARD_PAD))
        .pb(px(16.))
        .child(
            div()
                .flex_grow()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(CARD_TITLE))
                        .line_height(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t_.foreground)
                        .child(title),
                )
                .when_some(desc, |d, desc| {
                    d.child(
                        div()
                            .text_size(px(FONT))
                            .line_height(px(20.))
                            .text_color(t_.muted_foreground)
                            .child(desc),
                    )
                }),
        )
        .children(control.map(|c| div().flex_none().child(c)))
}

/// One primary KPI card (shadcn `SectionCards`).
struct Kpi {
    label: String,
    value: String,
    /// Value colour (default: foreground).
    color: Option<gpui::Hsla>,
    /// Change vs the previous period (badge + trend line); only when reliable
    /// (`stats::delta_reliable`).
    delta: Option<f64>,
    /// First footer line when there is no delta (medium weight), e.g. the figure in euros.
    lead: Option<(String, Option<Trend>)>,
    /// Second footer line (muted): what the figure is (left out when the delta's comparison
    /// takes its place).
    context: Vec<String>,
    /// Always on the second footer line (e.g. tokens without a price).
    note: Option<String>,
}

impl Kpi {
    fn new(label: &str, value: String) -> Self {
        Kpi {
            label: label.to_string(),
            value,
            color: None,
            delta: None,
            lead: None,
            context: Vec::new(),
            note: None,
        }
    }
    fn delta(mut self, d: Option<f64>) -> Self {
        self.delta = d;
        self
    }
    fn color(mut self, c: Option<gpui::Hsla>) -> Self {
        self.color = c;
        self
    }
    fn lead(mut self, text: String, trend: Option<Trend>) -> Self {
        self.lead = Some((text, trend));
        self
    }
    fn context(mut self, text: String) -> Self {
        if !text.is_empty() {
            self.context.push(text);
        }
        self
    }
    fn note(mut self, text: Option<String>) -> Self {
        self.note = text;
        self
    }
}

/// Trend arrow next to a footer line or in a badge.
fn arrow(trend: Option<Trend>) -> Option<Div> {
    trend
        .and_then(Trend::arrow)
        .map(|a| div().flex_none().child(a))
}

/// Outline badge (shadcn `Badge variant="outline"`): `rounded-md`, 1 px border, 12 px medium,
/// muted, with the trend arrow.
fn trend_badge(t_: &Theme, lang: Lang, delta: f64) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .h(px(22.))
        .px(px(8.))
        .rounded(px(6.))
        .border_1()
        .border_color(t_.border)
        .text_size(px(CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t_.muted_foreground)
        .whitespace_nowrap()
        .children(arrow(Some(Trend::of(delta))))
        .child(stats::signed_pct(lang, delta))
}

/// A KPI card: label (14 px muted) and value (30 px semibold, 24 px when narrow) with the
/// trend badge on the right; footer: an interpretation line (medium) and the context (muted).
/// Light: the card fades from `card` to 5 % `primary` at the bottom, as in the block.
fn kpi_card(t_: &Theme, lang: Lang, k: Kpi, value_px: f32, days: u32) -> Div {
    let (lead, lead_trend, context) = match k.delta {
        Some(d) => {
            let trend = Trend::of(d);
            let text = match trend {
                Trend::Up => t(lang, "Trending up this period"),
                Trend::Down => t(lang, "Trending down this period"),
                Trend::Flat => t(lang, "Stable this period"),
            };
            // The lead (euros, input / output) moves into the context line.
            let mut ctx: Vec<String> = k.lead.iter().map(|(l, _)| l.clone()).collect();
            ctx.push(tf(
                lang,
                "vs the previous {n} days",
                &[("n", &days.to_string())],
            ));
            ctx.extend(k.note);
            (Some(text.to_string()), Some(trend), ctx)
        }
        None => {
            let (lead, trend) = k.lead.map_or((None, None), |(l, tr)| (Some(l), tr));
            let mut ctx = k.context;
            ctx.extend(k.note);
            (lead, trend, ctx)
        }
    };
    // Label and badge share the first line (the badge wraps under the label in a narrow
    // card); the value has the full width below them.
    let header = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(8.))
                .gap_y(px(4.))
                .child(
                    div()
                        .text_size(px(FONT))
                        .line_height(px(22.))
                        .text_color(t_.muted_foreground)
                        .whitespace_nowrap()
                        .child(k.label),
                )
                .children(k.delta.map(|d| trend_badge(t_, lang, d).ml_auto())),
        )
        .child(
            div()
                .text_size(px(value_px))
                .line_height(px(value_px + 6.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(k.color.unwrap_or(t_.foreground))
                .truncate()
                .child(k.value),
        );
    let footer = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .text_size(px(FONT))
        .line_height(px(20.))
        .when_some(lead, |d, lead| {
            // One text run (the arrow after a no-break space), so it wraps like a sentence.
            let text = match lead_trend.and_then(Trend::arrow) {
                Some(a) => format!("{lead}\u{a0}\u{a0}{a}"),
                None => lead,
            };
            d.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t_.foreground)
                    .child(text),
            )
        })
        .when(!context.is_empty(), |d| {
            d.child(
                div()
                    .text_color(t_.muted_foreground)
                    .child(context.join(" · ")),
            )
        });
    // min-width 0: a grid track never grows to a text's unwrapped width.
    let base = card(t_)
        .min_w(px(0.))
        .gap(px(20.))
        .p(px(CARD_PAD))
        .child(header)
        .child(footer);
    if t_.dark {
        base
    } else {
        let tint = codenotch_ui_kit::theme::lerp(t_.card, t_.primary, 0.05);
        base.bg(linear_gradient(
            180.0,
            linear_color_stop(t_.card, 0.0),
            linear_color_stop(tint, 1.0),
        ))
    }
}

/// One secondary figure (the compact card under the KPI cards).
struct Habit {
    label: String,
    value: String,
    subs: Vec<String>,
}

impl Habit {
    fn new(label: &str, value: String, subs: Vec<String>) -> Self {
        Habit {
            label: label.to_string(),
            value,
            subs,
        }
    }
}

/// Label, value (20 px semibold) and one-line muted sub-lines of a secondary figure.
fn habit_cell(t_: &Theme, h: Habit) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .min_w(px(0.))
        .px(px(20.))
        .py(px(16.))
        .child(
            div()
                .text_size(px(FONT))
                .line_height(px(20.))
                .text_color(t_.muted_foreground)
                .truncate()
                .child(h.label),
        )
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t_.foreground)
                .truncate()
                .child(h.value),
        )
        .children(h.subs.into_iter().map(|text| {
            div()
                .text_size(px(CAPTION))
                .line_height(px(16.))
                .text_color(t_.muted_foreground)
                .truncate()
                .child(text)
        }))
}

fn days_text(lang: Lang, n: u32) -> String {
    if n == 1 {
        t(lang, "1 day").to_string()
    } else {
        tf(lang, "{n} days", &[("n", &n.to_string())])
    }
}

/// Colour of a chart series / model: the provider's hue, a shade of it per model (by its rank
/// in the period), neutral grey for "Others".
fn series_color(
    t_: &Theme,
    key: &SeriesKey,
    ranks: &[((codenotch_core::ProviderKind, String), usize)],
) -> gpui::Hsla {
    match key {
        SeriesKey::Provider(p) => t_.provider_color(p),
        SeriesKey::Model(p, m) => t_.model_color(p, stats::rank_of(ranks, p, m)),
        SeriesKey::Others => t_.chart_others,
        SeriesKey::Total => t_.chart_neutral,
    }
}

/// Legend swatch + name.
fn legend_item(t_: &Theme, color: gpui::Hsla, name: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().size(px(10.)).rounded(px(2.)).bg(color))
        .child(
            div()
                .text_size(px(CAPTION))
                .text_color(t_.muted_foreground)
                .child(name),
        )
}

impl SettingsView {
    pub(crate) fn page_analytics(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let summary = snap.analytics.as_ref();
        let state = logic::analytics_state(summary, self.analytics_requested);
        if state != AnalyticsState::Loading {
            self.analytics_requested = None;
        }
        let models = summary
            .map(|s| s.detail.models_in(&s.detail.period_from, &s.detail.today))
            .unwrap_or_default();
        self.stats.reconcile(&models);

        let mut page = div()
            .flex()
            .flex_col()
            .child(self.stats_filters(&models, t_, lang, cx));

        let failed = summary
            .and_then(|s| s.error.as_deref())
            .filter(|_| state != AnalyticsState::Loading);
        if let Some(err) = failed {
            page = page.child(
                note(
                    t_.destructive,
                    tf(
                        lang,
                        "Analytics could not be computed: {msg}",
                        &[("msg", err)],
                    ),
                )
                .pt(px(10.)),
            );
        }
        match (state, summary) {
            (AnalyticsState::Loading, _) => {
                page = page.child(caption(
                    t_,
                    t(lang, "Reading the local transcripts…").to_string(),
                ));
            }
            (AnalyticsState::Empty, Some(sum)) if sum.error.is_some() => {}
            (AnalyticsState::NotComputed, _) | (_, None) => {
                page = page
                    .child(caption(t_, t(lang, "No data yet").to_string()))
                    .child(caption(t_, t(lang, "Press Refresh to read the Claude Code and Codex transcripts on this PC. Nothing leaves your computer.").to_string()));
            }
            (AnalyticsState::Empty, Some(sum)) => {
                page = page.child(caption(
                    t_,
                    tf(
                        lang,
                        "No usage in the last {n} days.",
                        &[("n", &sum.days.to_string())],
                    ),
                ));
            }
            (AnalyticsState::Ready, Some(sum)) if sum.detail.facts.is_empty() => {
                page = page.child(caption(
                    t_,
                    t(lang, "Press Refresh to load the detailed statistics.").to_string(),
                ));
            }
            (AnalyticsState::Ready, Some(sum)) => {
                page = self.analytics_body(page, sum, snap, t_, lang, window, cx);
            }
        }

        page.child(crate::widgets::section_title(
            t_,
            t(lang, "Plans").to_string(),
            false,
        ))
        .child(self.plans_group(snap, t_, lang, window, cx))
    }

    /// Refresh, as a ghost icon button of the header bar.
    pub(crate) fn stats_refresh_button(
        &self,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Btn {
        let view = cx.entity().downgrade();
        let model = self.model.clone();
        Btn::new("analytics-refresh", t(lang, "Refresh").to_string(), *t_)
            .icon(glyph::REFRESH)
            .kind(BtnKind::Ghost)
            .small()
            .on_click(move |_, cx| {
                let generated = model
                    .read(cx)
                    .snapshot
                    .analytics
                    .as_ref()
                    .map(|a| a.generated_at)
                    .unwrap_or(0);
                let _ = view.update(cx, |this, cx| {
                    this.analytics_requested = Some(generated);
                    this.start_analytics_toast(generated, cx);
                    cx.notify();
                });
                model.read(cx).send(Command::RefreshAnalytics);
            })
    }

    /// Filters row: the period as a toggle group (longest first, like the block's "Last 3
    /// months / 30 days / 7 days"; a select in a narrow window), then the provider and model
    /// selects. A new period is saved and recomputed.
    fn stats_filters(
        &mut self,
        models: &[(codenotch_core::ProviderKind, String)],
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        let st = &self.edited.settings;
        let period: AnyElement = {
            let view = cx.entity().downgrade();
            let model = self.model.clone();
            let mut values: Vec<u64> =
                logic::choices_with_current(logic::ANALYTICS_DAY_CHOICES, st.analytics_days);
            values.reverse();
            let selected = values.iter().position(|d| *d == st.analytics_days);
            let labels: Vec<String> = values.iter().map(|d| logic::days_label(lang, *d)).collect();
            let pick = move |i: usize, _: &mut Window, cx: &mut gpui::App| {
                let Some(days) = values.get(i).copied() else {
                    return;
                };
                let _ = view.update(cx, |this, cx| {
                    this.commit(Edit::AnalyticsDays(days), cx);
                    let generated = this
                        .model
                        .read(cx)
                        .snapshot
                        .analytics
                        .as_ref()
                        .map(|a| a.generated_at)
                        .unwrap_or(0);
                    this.analytics_requested = Some(generated);
                    this.start_analytics_toast(generated, cx);
                });
                model.read(cx).send(Command::RefreshAnalytics);
            };
            if self.narrow {
                ComboBox::new("period", labels, selected, *t_)
                    .on_change(pick)
                    .into_any_element()
            } else {
                Segmented::new("period", labels, selected, *t_)
                    .on_change(pick)
                    .into_any_element()
            }
        };

        let providers = stats::providers_of(models);
        let provider = {
            let view = cx.entity().downgrade();
            let mut labels = vec![t(lang, "All providers").to_string()];
            labels.extend(providers.iter().map(|p| p.display_name().to_string()));
            let selected = match &self.stats.provider {
                None => Some(0),
                Some(p) => providers.iter().position(|q| q == p).map(|i| i + 1),
            };
            let values = providers.clone();
            ComboBox::new("stats-provider", labels, selected, *t_)
                .disabled(providers.is_empty())
                .on_change(move |i, _, cx| {
                    let choice = i.checked_sub(1).and_then(|i| values.get(i).cloned());
                    let _ = view.update(cx, |this, cx| {
                        if this.stats.provider != choice {
                            this.stats.provider = choice;
                            this.stats.model = None;
                            cx.notify();
                        }
                    });
                })
        };
        let model = {
            let view = cx.entity().downgrade();
            let pairs = stats::models_of(models, self.stats.provider.as_ref());
            let mut labels = vec![t(lang, "All models").to_string()];
            labels.extend(pairs.iter().map(|(_, m)| m.clone()));
            let selected = match (&self.stats.provider, &self.stats.model) {
                (Some(p), Some(m)) => pairs
                    .iter()
                    .position(|(q, n)| q == p && n == m)
                    .map(|i| i + 1),
                _ => Some(0),
            };
            let empty = pairs.is_empty();
            ComboBox::new("stats-model", labels, selected, *t_)
                .disabled(empty)
                .on_change(move |i, _, cx| {
                    let choice = i.checked_sub(1).and_then(|i| pairs.get(i).cloned());
                    let _ = view.update(cx, |this, cx| {
                        match choice {
                            Some((p, m)) => {
                                this.stats.provider = Some(p);
                                this.stats.model = Some(m);
                            }
                            None => this.stats.model = None,
                        }
                        cx.notify();
                    });
                })
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(period)
            .child(provider)
            .child(model)
    }

    #[allow(clippy::too_many_arguments)]
    fn analytics_body(
        &self,
        mut page: Div,
        sum: &AnalyticsSummary,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let d: &AnalyticsDetail = &sum.detail;
        let ui = self.stats.clone();
        let filter = ui.filter();
        let k = stats::kpis(d, &filter);
        let width = self.content_width(window, SETTINGS_STATS_MAX_W);

        if u64::from(sum.days) != self.edited.settings.analytics_days {
            page = page.child(caption(
                t_,
                tf(
                    lang,
                    "These figures cover the last {n} days. Press Refresh to recompute.",
                    &[("n", &sum.days.to_string())],
                ),
            ));
        }

        let comparisons = logic::comparisons(sum, &self.edited, &snap.accounts);
        let ratio = stats::plan_ratio(&comparisons, &ui);
        page = page.child(
            div()
                .mt(px(GAP))
                .child(self.kpi_grid(&k, ratio, &ui, t_, lang, width)),
        );
        if ratio.is_some() {
            page = page.child(caption(t_, t(lang, "×N compares the API-equivalent cost with your plan price over the same period (above ×1, the plan pays off).").to_string()));
        }
        if ratio.is_some_and(|r| r.list_price) {
            page = page.child(caption(
                t_,
                tf(
                    lang,
                    "≈: plans without a price of yours use their public list price ({date}). Enter 0 to leave a plan out.",
                    &[("date", codenotch_core::config::LIST_PRICES_AS_OF)],
                ),
            ));
        }

        let ranks = stats::model_ranks(d);
        page = page
            .child(
                div()
                    .mt(px(GAP))
                    .child(self.evolution_card(d, &ui, &ranks, t_, lang, width, cx)),
            )
            .child(div().mt(px(GAP)).child(heatmap_card(
                d,
                &filter,
                t_,
                lang,
                width,
                self.dev.heat,
            )));

        let shares = models_card(d, &filter, &ranks, t_, lang);
        let hours = hours_card(d, &filter, &k, t_, lang, width, self.dev.hour);
        let row = if width >= 760.0 {
            div()
                .flex()
                .items_start()
                .gap(px(GAP))
                .child(shares.flex_1().flex_basis(px(0.)).min_w(px(0.)))
                .child(hours.flex_1().flex_basis(px(0.)).min_w(px(0.)))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .child(shares)
                .child(hours)
        };
        page.child(div().mt(px(GAP)).child(row))
    }

    /// The four primary KPI cards (4 across / 2 × 2 / stacked), then the secondary figures in
    /// one compact card split by hairlines (3 × 2, 2 × 3 when narrow).
    fn kpi_grid(
        &self,
        k: &Kpis,
        ratio: Option<stats::PlanRatio>,
        ui: &StatsUi,
        t_: &Theme,
        lang: Lang,
        width: f32,
    ) -> Div {
        let fx = self.edited.settings.usd_to_eur;
        let usd = |v: f64| format::money(lang, v, "USD");
        let eur = |v: f64| format::money(lang, v * fx, "EUR");
        let unpriced = k.totals.unpriced_tokens;
        let tk = k.totals.tokens;
        let dash = || format::DASH.to_string();
        let days = k.period_days;

        let unpriced_note = (unpriced > 0).then(|| {
            tf(
                lang,
                "≥: {n} tokens have no price",
                &[("n", &stats::tok(lang, unpriced))],
            )
        });
        let plan_sub = if ratio.is_some_and(|r| r.list_price) {
            t(lang, "API-equivalent cost vs list price (≈)")
        } else if ratio.is_some() {
            t(lang, "API-equivalent cost vs plan price")
        } else if ui.model.is_some() {
            t(lang, "Plans are per provider")
        } else {
            t(lang, "Set a plan price below")
        };
        let plan_lead = match ratio {
            Some(r) if r.ratio >= 1.0 => (t(lang, "The plan pays off"), Some(Trend::Up)),
            Some(_) => (t(lang, "Below the plan price"), Some(Trend::Down)),
            None => (t(lang, "No plan price yet"), None),
        };
        let primary = vec![
            Kpi::new(
                t(lang, "API-equivalent cost"),
                logic::cost_text(k.totals.cost_usd, unpriced, usd),
            )
            .delta(k.cost_delta)
            .lead(logic::cost_text(k.totals.cost_usd, unpriced, eur), None)
            .context(if unpriced > 0 {
                String::new()
            } else {
                t(lang, "At public API prices").to_string()
            })
            .note(unpriced_note),
            Kpi::new(t(lang, "Tokens"), stats::tok(lang, tk.total()))
                .delta(k.tokens_delta)
                .lead(
                    format!(
                        "{} {} · {} {}",
                        t(lang, "Input"),
                        stats::tok(lang, tk.input),
                        t(lang, "Output"),
                        stats::tok(lang, tk.output)
                    ),
                    None,
                )
                .context(t(lang, "Input, output and cache").to_string()),
            Kpi::new(
                t(lang, "Monthly projection"),
                logic::cost_text(k.monthly_projection, unpriced, usd),
            )
            .lead(
                tf(
                    lang,
                    "{eur} per month",
                    &[(
                        "eur",
                        &logic::cost_text(k.monthly_projection, unpriced, eur),
                    )],
                ),
                None,
            )
            .context(tf(
                lang,
                "At the pace of the last {n} days",
                &[("n", &days.to_string())],
            )),
            Kpi::new(
                t(lang, "Plan value"),
                ratio.map_or_else(dash, |r| {
                    let text = stats::ratio_text(lang, r.ratio, r.lower_bound);
                    // An estimate from list prices: "≈ ×3,2" (a lower bound keeps its "≥").
                    if r.list_price && !r.lower_bound {
                        format!("≈ {text}")
                    } else {
                        text
                    }
                }),
            )
            .color(ratio.filter(|r| r.ratio >= 1.0).map(|_| t_.success))
            .lead(plan_lead.0.to_string(), plan_lead.1)
            .context(plan_sub.to_string()),
        ];
        // The top model is in "By model"; the busiest day keeps its volume and peak hour.
        let mut busiest_sub = Vec::new();
        if let Some((_, tokens)) = &k.busiest_day {
            let mut line = tf(
                lang,
                "{tokens} tokens",
                &[("tokens", &stats::tok(lang, *tokens))],
            );
            if let Some(h) = k.peak_hour {
                line = format!(
                    "{line} · {}",
                    tf(lang, "peak around {h}:00", &[("h", &h.to_string())])
                );
            }
            busiest_sub.push(line);
        }
        let habits = vec![
            Habit::new(
                t(lang, "Sessions"),
                k.totals.sessions.to_string(),
                vec![k.tokens_per_session.map_or_else(dash, |n| {
                    tf(
                        lang,
                        "{tokens} tokens / session",
                        &[("tokens", &stats::tok(lang, n))],
                    )
                })],
            ),
            Habit::new(
                t(lang, "Cache hit rate"),
                k.cache_rate.map_or_else(dash, |r| stats::pct(lang, r)),
                vec![format!(
                    "{} {} · {} {}",
                    t(lang, "Cache read"),
                    stats::tok(lang, tk.cache_read),
                    t(lang, "Cache write"),
                    stats::tok(lang, tk.cache_write)
                )],
            ),
            Habit::new(
                t(lang, "Cost per active day"),
                k.cost_per_active_day
                    .map_or_else(dash, |c| logic::cost_text(c, unpriced, usd)),
                vec![tf(
                    lang,
                    "{n} active days out of {total}",
                    &[
                        ("n", &k.active_days.to_string()),
                        ("total", &k.period_days.to_string()),
                    ],
                )],
            ),
            Habit::new(
                t(lang, "Pace"),
                k.tokens_per_active_hour.map_or_else(dash, |n| {
                    tf(lang, "{tokens} / h", &[("tokens", &stats::tok(lang, n))])
                }),
                vec![tf(
                    lang,
                    "{n} active time slots",
                    &[("n", &k.active_hours.to_string())],
                )],
            ),
            Habit::new(
                t(lang, "Streak"),
                days_text(lang, k.streak),
                vec![tf(
                    lang,
                    "Record: {n}",
                    &[("n", &days_text(lang, k.best_streak))],
                )],
            ),
            Habit::new(
                t(lang, "Busiest day"),
                k.busiest_day
                    .as_ref()
                    .map_or_else(dash, |(date, _)| stats::short_date(lang, date)),
                busiest_sub,
            ),
        ];

        let value_px = if self.narrow { 24. } else { 30. };
        let cols = stats::kpi_columns(width, GAP);
        let cards = div().grid().grid_cols(cols as u16).gap(px(GAP)).children(
            primary
                .into_iter()
                .map(|st| kpi_card(t_, lang, st, value_px, days)),
        );
        let strip_cols = stats::habit_columns(width);
        let strip =
            card(t_)
                .overflow_hidden()
                .child(div().grid().grid_cols(strip_cols as u16).children(
                    habits.into_iter().enumerate().map(|(i, h)| {
                        let (col, row) = (i % strip_cols, i / strip_cols);
                        habit_cell(t_, h)
                            .when(col > 0, |d| d.border_l_1().border_color(t_.border))
                            .when(row > 0, |d| d.border_t_1().border_color(t_.border))
                    }),
                ));
        div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(cards)
            .child(strip)
    }

    /// "Évolution" (shadcn `ChartAreaInteractive`): title + description, the metric as a
    /// toggle group at the top right, the stacked area chart full width (250 px).
    #[allow(clippy::too_many_arguments)]
    fn evolution_card(
        &self,
        d: &AnalyticsDetail,
        ui: &StatsUi,
        ranks: &[((codenotch_core::ProviderKind, String), usize)],
        t_: &Theme,
        lang: Lang,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let data = stats::chart_data(d, ui, t(lang, "Sessions"), t(lang, "Others"));
        let metric = {
            let view = cx.entity().downgrade();
            let labels: Vec<String> = Metric::ALL
                .iter()
                .map(|m| t(lang, m.label()).to_string())
                .collect();
            let selected = Metric::ALL.iter().position(|m| *m == ui.metric);
            Segmented::new("stats-metric", labels, selected, *t_)
                .small()
                .on_change(move |i, _, cx| {
                    let Some(m) = Metric::ALL.get(i).copied() else {
                        return;
                    };
                    let _ = view.update(cx, |this, cx| {
                        this.stats.metric = m;
                        cx.notify();
                    });
                })
        };
        let desc = match (ui.metric, ui.provider.is_some()) {
            (Metric::Sessions, _) => t(lang, "Sessions per day"),
            (Metric::Cost, false) => t(lang, "API-equivalent cost per day, by provider"),
            (Metric::Cost, true) => t(lang, "API-equivalent cost per day, by model"),
            (Metric::Tokens, false) => t(lang, "Tokens per day, by provider"),
            (Metric::Tokens, true) => t(lang, "Tokens per day, by model"),
        };
        let labels: Vec<SharedString> = data
            .dates
            .iter()
            .map(|date| s(stats::short_date(lang, date)))
            .collect();
        let series: Vec<ChartSeries> = data
            .series
            .iter()
            .zip(&data.keys)
            .map(|((name, values), key)| ChartSeries {
                name: s(name.clone()),
                color: series_color(t_, key, ranks),
                values: values.clone(),
            })
            .collect();
        let legend = (series.len() > 1).then(|| {
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap_x(px(16.))
                .gap_y(px(4.))
                .pt(px(12.))
                .children(
                    series
                        .iter()
                        .map(|s| legend_item(t_, s.color, s.name.to_string())),
                )
        });
        let format: Box<dyn Fn(f64) -> String> = match ui.metric {
            Metric::Cost => Box::new(move |v| format::money(lang, v, "USD")),
            Metric::Tokens => Box::new(move |v| stats::tok(lang, v.max(0.0).round() as u64)),
            Metric::Sessions => Box::new(|v| format!("{:.0}", v.max(0.0))),
        };
        let axis: Box<dyn Fn(f64) -> String> = match ui.metric {
            Metric::Cost => Box::new(move |v| stats::money_axis(lang, v)),
            Metric::Tokens => Box::new(move |v| stats::tok_axis(lang, v)),
            Metric::Sessions => Box::new(|v| {
                if (v - v.round()).abs() < 1e-6 {
                    format!("{:.0}", v.max(0.0))
                } else {
                    format!("{:.1}", v.max(0.0))
                }
            }),
        };
        // Today is still accumulating: its segment is drawn lighter and dashed.
        let chart = AreaChart::new("stats-chart", *t_, labels, series)
            .format(format)
            .axis_format(axis)
            .partial_last(true)
            .total_label(t(lang, "Total").to_string())
            .width_hint(width - 2.0 * CARD_PAD - 2.0);
        card(t_)
            .child(card_header(
                t_,
                t(lang, "Evolution").to_string(),
                Some(desc.to_string()),
                Some(metric.into_any_element()),
            ))
            .child(
                div()
                    .px(px(CARD_PAD))
                    .pt(px(8.))
                    .pb(px(CARD_PAD))
                    .child(chart)
                    .children(legend),
            )
    }
}

/// GitHub-style heatmap of the last 53 weeks.
/// `forced`: cell index (`col * 7 + row`) whose tooltip is shown without a pointer (dev).
fn heatmap_card(
    d: &AnalyticsDetail,
    filter: &codenotch_core::analytics::UsageFilter,
    t_: &Theme,
    lang: Lang,
    width: f32,
    forced: Option<usize>,
) -> Div {
    let days = d.heatmap(filter);
    // As many weeks as fit with legible cells (≥ 10 px), the most recent ones.
    let avail = width - 2.0 * CARD_PAD - 2.0 - ROW_LABEL_W;
    let weeks = stats::heat_weeks(avail);
    let cells = stats::heat_grid(&days, weeks);
    let active = days.iter().filter(|(_, t)| t.tokens.total() > 0).count();
    let (cell, gap) = stats::grid_cell(avail, weeks, 18.0);
    let pitch = cell + gap;
    // A month label is ~30 px wide: skip one that would overlap the previous.
    let min_cols = (32.0 / pitch).ceil() as usize;
    let usd = move |v: f64| format::money(lang, v, "USD");

    // Month labels.
    let mut months = div().relative().h(px(16.)).ml(px(ROW_LABEL_W)).mb(px(4.));
    for (col, m) in stats::month_labels(&cells, min_cols) {
        months = months.child(
            div()
                .absolute()
                .left(px(col as f32 * pitch))
                .top_0()
                .text_size(px(HEAT_LABEL))
                .text_color(t_.muted_foreground)
                .whitespace_nowrap()
                .child(format::month_abbr(lang, m)),
        );
    }

    // Columns of 7 cells; days after today are left empty.
    let mut by_col: Vec<[Option<&HeatCell>; 7]> = vec![[None; 7]; weeks];
    for c in &cells {
        if let Some(col) = by_col.get_mut(c.col) {
            col[c.row.min(6)] = Some(c);
        }
    }
    let labels = div()
        .flex_none()
        .w(px(ROW_LABEL_W))
        .flex()
        .flex_col()
        .gap(px(gap))
        .children((0..7).map(|r| {
            div()
                .h(px(cell))
                .flex()
                .items_center()
                .text_size(px(HEAT_LABEL))
                .text_color(t_.muted_foreground)
                .child(if r % 2 == 0 && r < 6 {
                    stats::weekday_short(lang, r)
                } else {
                    ""
                })
        }));
    // Plain cells; one hover grid over them gives the instant tooltip of the hovered day.
    let mut grid = div().flex().gap(px(gap));
    for col in &by_col {
        let mut column = div().flex().flex_col().gap(px(gap));
        for c in col {
            let base = div().size(px(cell)).rounded(px(2.));
            column = column.child(match c {
                None => base,
                Some(c) => base
                    .bg(t_.heat_color(c.level))
                    .border_1()
                    .border_color(t_.heat_border),
            });
        }
        grid = grid.child(column);
    }
    let tips: Vec<[Option<TipContent>; 7]> = by_col
        .iter()
        .map(|col| {
            col.map(|c| {
                c.map(|c| {
                    let title = stats::day_title(lang, &c.date);
                    if c.totals.tokens.total() == 0 {
                        return TipContent::new(
                            title,
                            vec![TipRow::new(t(lang, "No activity").to_string(), "")],
                        );
                    }
                    TipContent::new(
                        title,
                        vec![
                            TipRow::new(
                                t(lang, "Tokens").to_string(),
                                stats::tok(lang, c.totals.tokens.total()),
                            )
                            .swatch(t_.heat_color(c.level.max(1))),
                            TipRow::new(
                                t(lang, "Cost").to_string(),
                                logic::cost_text(c.totals.cost_usd, c.totals.unpriced_tokens, usd),
                            ),
                            TipRow::new(
                                t(lang, "Sessions").to_string(),
                                c.totals.sessions.to_string(),
                            ),
                        ],
                    )
                })
            })
        })
        .collect();
    let grid = HoverGrid::new("heat-grid", *t_, weeks, 7, cell, gap, grid, move |c, r| {
        tips.get(c).and_then(|col| col[r].clone())
    })
    .forced(forced.map(|i| (i / 7, i % 7)));

    let legend = div()
        .flex()
        .items_center()
        .gap(px(3.))
        .text_size(px(CAPTION))
        .text_color(t_.muted_foreground)
        .child(div().mr(px(4.)).child(t(lang, "Less").to_string()))
        .children((0..5u8).map(|l| {
            div()
                .size(px(cell.clamp(10.0, 11.0)))
                .rounded(px(2.))
                .bg(t_.heat_color(l))
                .border_1()
                .border_color(t_.heat_border)
        }))
        .child(div().ml(px(4.)).child(t(lang, "More").to_string()));

    card(t_)
        .child(card_header(
            t_,
            t(lang, "Activity").to_string(),
            Some(if weeks >= stats::HEAT_COLS {
                t(lang, "Tokens per day over the last 12 months").to_string()
            } else {
                tf(
                    lang,
                    "Tokens per day over the last {n} weeks",
                    &[("n", &weeks.to_string())],
                )
            }),
            None,
        ))
        .child(
            div()
                .px(px(CARD_PAD))
                .pb(px(CARD_PAD))
                .child(months)
                .child(div().flex().child(labels).child(grid))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(16.))
                        .pt(px(12.))
                        .child(
                            div()
                                .text_size(px(CAPTION))
                                .text_color(t_.muted_foreground)
                                .child(tf(
                                    lang,
                                    "{n} active days in the last year",
                                    &[("n", &active.to_string())],
                                )),
                        )
                        .child(legend),
                ),
        )
}

/// Token share of each model of the period.
fn models_card(
    d: &AnalyticsDetail,
    filter: &codenotch_core::analytics::UsageFilter,
    ranks: &[((codenotch_core::ProviderKind, String), usize)],
    t_: &Theme,
    lang: Lang,
) -> Div {
    let usd = move |v: f64| format::money(lang, v, "USD");
    let shares = stats::model_shares(d, filter);
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .px(px(CARD_PAD))
        .pb(px(CARD_PAD));
    for m in shares.iter().take(MODEL_ROWS) {
        let color = t_.model_color(&m.provider, stats::rank_of(ranks, &m.provider, &m.model));
        let cost = if m.unpriced_tokens > 0 && m.cost_usd <= 0.0 {
            t(lang, "no price").to_string()
        } else {
            logic::cost_text(m.cost_usd, m.unpriced_tokens, usd)
        };
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_grow()
                                .min_w(px(0.))
                                .flex()
                                .items_baseline()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(DESC))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t_.foreground)
                                        .truncate()
                                        .child(m.model.clone()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(CAPTION))
                                        .text_color(t_.muted_foreground)
                                        .child(m.provider.display_name().to_string()),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(CAPTION))
                                .text_color(t_.muted_foreground)
                                .child(format!(
                                    "{} · {} · {}",
                                    tf(
                                        lang,
                                        "{tokens} tokens",
                                        &[("tokens", &stats::tok(lang, m.tokens))]
                                    ),
                                    cost,
                                    stats::pct(lang, m.share)
                                )),
                        ),
                )
                .child(
                    div().h(px(6.)).w_full().rounded_full().bg(t_.muted).child(
                        div()
                            .h_full()
                            .rounded_full()
                            .w(relative(m.share.clamp(0.0, 1.0) as f32))
                            .min_w(px(if m.share > 0.0 { 3. } else { 0. }))
                            .bg(color),
                    ),
                ),
        );
    }
    if shares.len() > MODEL_ROWS {
        let rest = &shares[MODEL_ROWS..];
        let tokens: u64 = rest.iter().map(|m| m.tokens).sum();
        list = list.child(
            div()
                .text_size(px(CAPTION))
                .text_color(t_.muted_foreground)
                .child(tf(
                    lang,
                    "{n} other models · {tokens} tokens",
                    &[
                        ("n", &rest.len().to_string()),
                        ("tokens", &stats::tok(lang, tokens)),
                    ],
                )),
        );
    }
    if shares.is_empty() {
        list = list.child(
            div()
                .text_size(px(DESC))
                .text_color(t_.muted_foreground)
                .child(t(lang, "No usage for this filter.").to_string()),
        );
    }
    card(t_)
        .child(card_header(
            t_,
            t(lang, "By model").to_string(),
            Some(t(lang, "Share of the tokens of the period").to_string()),
            None,
        ))
        .child(list)
}

/// Weekday × hour activity grid of the period.
/// `forced`: cell index (`weekday * 24 + hour`) whose tooltip is shown without a pointer (dev).
#[allow(clippy::too_many_arguments)]
fn hours_card(
    d: &AnalyticsDetail,
    filter: &codenotch_core::analytics::UsageFilter,
    k: &Kpis,
    t_: &Theme,
    lang: Lang,
    width: f32,
    forced: Option<usize>,
) -> Div {
    let grid = d.week_hour_tokens(filter);
    let max = grid.iter().flatten().copied().max().unwrap_or(0);
    let card_w = if width >= 760.0 {
        (width - GAP) / 2.0
    } else {
        width
    };
    let avail = card_w - 2.0 * CARD_PAD - 2.0 - ROW_LABEL_W;
    let (cell, gap) = stats::grid_cell(avail, 24, 22.0);
    let pitch = cell + gap;
    // Weekday labels, then the plain cells under one hover grid (instant tooltip).
    // Rows as tall as a label line (the cells centred in them), as before the hover grid.
    let row_h = cell.max(ROW_H);
    let row_gap = gap + (row_h - cell);
    let labels = div()
        .flex_none()
        .w(px(ROW_LABEL_W))
        .flex()
        .flex_col()
        .gap(px(gap))
        .children((0..7).map(|wd| {
            div()
                .h(px(row_h))
                .flex()
                .items_center()
                .text_size(px(CAPTION))
                .text_color(t_.muted_foreground)
                .child(stats::weekday_short(lang, wd))
        }));
    let mut cells = div().flex().flex_col().gap(px(row_gap));
    for hours in grid.iter() {
        let mut row = div().flex().gap(px(gap));
        for v in hours.iter() {
            row = row.child(
                div()
                    .flex_none()
                    .size(px(cell))
                    .rounded(px(2.))
                    .bg(t_.heat_color(stats::heat_level(*v, max)))
                    .border_1()
                    .border_color(t_.heat_border),
            );
        }
        cells = cells.child(row);
    }
    let tokens_label = t(lang, "Tokens").to_string();
    let theme = *t_;
    let hover = HoverGrid::new("hour-grid", *t_, 24, 7, cell, gap, cells, move |h, wd| {
        let v = *grid.get(wd)?.get(h)?;
        Some(TipContent::new(
            format!("{} · {h} h", stats::weekday_short(lang, wd)),
            vec![TipRow::new(tokens_label.clone(), stats::tok(lang, v))
                .swatch(theme.heat_color(stats::heat_level(v, max).max(1)))],
        ))
    })
    .row_gap(row_gap)
    .forced(forced.map(|i| (i % 24, i / 24)));
    let rows = div()
        .flex()
        .child(labels)
        .child(div().pt(px((row_h - cell) / 2.0)).child(hover));
    let mut axis = div().relative().h(px(16.)).mt(px(6.)).ml(px(ROW_LABEL_W));
    for h in [0usize, 6, 12, 18] {
        axis = axis.child(
            div()
                .absolute()
                .top_0()
                .left(px(h as f32 * pitch))
                .text_size(px(CAPTION))
                .text_color(t_.muted_foreground)
                .child(format!("{h} h")),
        );
    }
    let desc = match (k.peak_weekday, k.peak_hour) {
        (Some(wd), Some(h)) => tf(
            lang,
            "Busiest on {day} · peak around {h}:00",
            &[
                ("day", stats::weekday_short(lang, wd)),
                ("h", &h.to_string()),
            ],
        ),
        _ => t(lang, "Tokens by weekday and hour over the period").to_string(),
    };
    card(t_)
        .child(card_header(
            t_,
            t(lang, "Activity hours").to_string(),
            Some(desc),
            None,
        ))
        .child(
            div()
                .px(px(CARD_PAD))
                .pb(px(CARD_PAD))
                .child(rows)
                .child(axis),
        )
}
