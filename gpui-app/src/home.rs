//! The Home page of the settings: the tracked quotas at a glance, shortcuts to the settings
//! people change most.
use crate::account_form::name;
use crate::model::Account;
use crate::settings::{button, icon, position_label, AccountsTab, Kind, Section, SettingsView, MUTED};
use crate::ui::{
    freshness, headline, is_fresh, percentage, provider_icon, reset_time, status_label, t, usage_color,
};
use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, ClickEvent, Context, FontWeight, SharedString};

fn group_label(text: String) -> AnyElement {
    div()
        .mb(px(8.))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0x77777f))
        .child(text.to_uppercase())
        .into_any_element()
}

/// Cards three to a line, the last line padded so every card keeps the same width.
fn grid(cards: Vec<AnyElement>) -> AnyElement {
    let mut rows = vec![];
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut line: Vec<AnyElement> = cards.by_ref().take(3).collect();
        while line.len() < 3 {
            line.push(div().flex_1().into_any_element());
        }
        rows.push(div().flex().gap(px(12.)).children(line));
    }
    div().flex().flex_col().gap(px(12.)).children(rows).into_any_element()
}

fn card() -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .p(px(14.))
        .rounded(px(12.))
        .border_1()
        .border_color(rgba(0xffffff12))
        .bg(rgba(0xffffff05))
}

impl SettingsView {
    /// The page heading: the state of the tracked accounts, and a refresh.
    pub(crate) fn home_heading(&self, french: bool, cx: &mut Context<Self>) -> (String, String, AnyElement) {
        let s = self.store.read(cx);
        let now = chrono::Utc::now().timestamp();
        let accounts = s.enabled();
        let interval = s.config.settings.poll_seconds;
        let to_check = accounts
            .iter()
            .filter(|a| !is_fresh(s.readings.get(&a.id), interval, now))
            .count();
        let latest = accounts.iter().filter_map(|a| s.readings.get(&a.id)?.updated_at).max();
        let heading = match (accounts.len(), to_check) {
            (0, _) => t(french, "Aucun compte suivi"),
            (_, 0) => t(french, "Tout est suivi"),
            (_, 1) => t(french, "1 compte à vérifier"),
            (_, n) => t(french, "{count} comptes à vérifier").replace("{count}", &n.to_string()),
        };
        let count = match accounts.len() {
            1 => t(french, "1 compte dans l’encoche"),
            n => t(french, "{count} comptes dans l’encoche").replace("{count}", &n.to_string()),
        };
        let intro = format!("{count} · {}", freshness(latest, now, french));
        let refresh = button(
            "home-refresh",
            Kind::Secondary,
            Some("refresh-cw"),
            t(french, "Actualiser"),
            Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_now(cx)))),
        );
        (heading, intro, refresh)
    }

    fn quota_card(&self, account: &Account, french: bool, cx: &Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let now = chrono::Utc::now().timestamp();
        let reading = s.readings.get(&account.id);
        let quota = headline(reading);
        let used = quota.map(|q| q.used_percent);
        let title = if account.name.is_empty() { name(&account.provider, french) } else { account.name.clone() };
        let plan = reading.and_then(|r| r.plan.clone());
        let value = match (quota, reading) {
            (Some(q), _) => div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(usage_color(used))
                .child(percentage(q.used_percent, french)),
            (None, Some(r)) => div().text_size(px(13.)).text_color(rgb(0x9a9aa3)).child(t(french, status_label(&r.status))),
            (None, None) => div().text_size(px(13.)).text_color(rgb(0x9a9aa3)).child(freshness(None, now, french)),
        };
        let fill = used.unwrap_or(0.).clamp(0., 100.) as f32 / 100.;
        card()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(provider_icon(&account.provider, 16.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(title))
                    .children(plan.map(|p| div().flex_none().text_size(px(10.5)).text_color(rgb(0x8f8f96)).child(p))),
            )
            .child(value)
            .child(
                div()
                    .h(px(4.))
                    .w_full()
                    .rounded_full()
                    .bg(rgba(0xffffff14))
                    .child(div().h_full().w(gpui::relative(fill)).rounded_full().bg(usage_color(used))),
            )
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(0x8f8f96))
                    .truncate()
                    .child(match quota {
                        Some(q) => format!("{} · {}", t(french, &q.label), reset_time(q.resets_at, now, french)),
                        None => t(french, "Pas encore de quota lu"),
                    }),
            )
            .into_any_element()
    }

    fn shortcut(
        &self,
        id: &'static str,
        leading: &str,
        title: String,
        detail: String,
        go: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        card()
            .id(SharedString::from(id))
            .gap(px(4.))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(0xffffff0a)).border_color(rgba(0xffffff1f)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(icon(leading, 15., 0xc3c3c8))
                    .child(title)
                    .child(div().flex_1())
                    .child(icon("chevron-right", 14., 0x6b6b72)),
            )
            .child(div().pl(px(23.)).text_size(px(11.)).text_color(rgb(0x8f8f96)).child(detail))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                go(this);
                this.message = None;
                cx.notify();
            }))
            .into_any_element()
    }

    pub(crate) fn home_page(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (accounts, settings) = {
            let s = self.store.read(cx);
            (s.enabled(), s.config.settings.clone())
        };

        let cards: Vec<AnyElement> = if accounts.is_empty() {
            vec![card()
                .id("home-empty")
                .items_center()
                .gap(px(6.))
                .py(px(22.))
                .border_dashed()
                .cursor_pointer()
                .hover(|d| d.bg(rgba(0xffffff0a)))
                .child(icon("plus", 18., 0xc3c3c8))
                .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(t(french, "Connecte un premier compte")))
                .child(div().text_size(px(11.)).text_color(rgb(MUTED)).child(t(french, "Ses quotas apparaîtront ici et dans l’encoche.")))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.section = Section::Accounts;
                    this.accounts_tab = AccountsTab::Quick;
                    cx.notify();
                }))
                .into_any_element()]
        } else {
            accounts.iter().map(|a| self.quota_card(a, french, cx)).collect()
        };

        // Providers CodeNotch can sign in to that are not tracked yet.
        let missing: Vec<String> = self
            .statuses
            .iter()
            .filter(|st| !accounts.iter().any(|a| a.provider == st.provider))
            .map(|st| crate::ui::provider_title(&st.provider).to_owned())
            .collect();
        let connect_detail = if missing.is_empty() {
            t(french, "Tous tes abonnements sont connectés")
        } else {
            t(french, "{list} disponibles").replace("{list}", &missing.join(", "))
        };
        let alerts_detail = if settings.notifications {
            t(french, "Activées à {percent} %").replace("{percent}", &settings.alert_percent.to_string())
        } else {
            t(french, "Désactivées")
        };
        let shortcuts = vec![
            self.shortcut(
                "home-connect",
                "plus",
                t(french, "Connecter un compte"),
                connect_detail,
                |this| {
                    this.section = Section::Accounts;
                    this.accounts_tab = AccountsTab::Quick;
                },
                cx,
            ),
            self.shortcut(
                "home-position",
                "monitor",
                t(french, "Position de l’encoche"),
                t(french, position_label(&settings.widget_position)),
                |this| this.section = Section::Appearance,
                cx,
            ),
            self.shortcut("home-alerts", "bell", t(french, "Alertes"), alerts_detail, |this| this.section = Section::Alerts, cx),
        ];

        vec![
            div().pt(px(8.)).child(grid(cards)).into_any_element(),
            div().child(group_label(t(french, "Raccourcis"))).child(grid(shortcuts)).into_any_element(),
        ]
    }
}
