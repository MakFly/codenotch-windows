//! The Home page of the settings: every tracked account with each of its quota windows,
//! where an even pace would stand, and shortcuts to the settings people change most.
use crate::account_form::name;
use crate::analytics::period_length;
use crate::chart::tone;
use crate::model::{Account, Window as Quota};
use crate::settings::{
    button, icon, list_header, position_label, text_button, AccountsTab, Kind, Section, SettingsView, MUTED,
};
use crate::ui::{freshness, is_fresh, percentage, provider_icon, reset_time, status_label, t};
use gpui::{div, prelude::*, px, relative, rgb, rgba, AnyElement, ClickEvent, Context, FontWeight, SharedString};

/// The white mark of the even pace on a quota bar, and in the legend.
const PACE_MARK: u32 = 0xffffffb3;

/// A quiet title over a group of content, the same voice as `list_header`.
pub(crate) fn group_label(text: String) -> AnyElement {
    div()
        .mb(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(0xa8a8b0))
        .child(text)
        .into_any_element()
}

pub(crate) fn card() -> gpui::Div {
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

/// One bordered surface holding rows separated by hairlines.
pub(crate) fn list(rows: Vec<AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(rgba(0xffffff12))
        .bg(rgba(0xffffff04))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(|(i, row)| {
            div().when(i > 0, |d| d.border_t_1().border_color(rgba(0xffffff0f))).child(row)
        }))
}

/// Where an even pace from the start of the window's period would stand now, in percent:
/// needs the period length from the label and a reset still to come.
fn even_pace(label: &str, resets_at: Option<i64>, now: i64) -> Option<f64> {
    let end = resets_at?;
    let length = period_length(label)?;
    let start = end - length;
    (start < now && now < end).then(|| 100. * (now - start) as f64 / length as f64)
}

/// A quota window as a bar: its name, the share used, the even pace as a white mark, the
/// percentage and the time to its reset.
fn window_line(q: &Quota, now: i64, french: bool) -> AnyElement {
    let color = tone(Some(q.used_percent));
    let fill = (q.used_percent / 100.).clamp(0., 1.) as f32;
    let pace = even_pace(&q.label, q.resets_at, now);
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(72.))
                .flex_none()
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(0xc3c3c8))
                .child(t(french, &q.label)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .relative()
                .h(px(6.))
                .rounded_full()
                .bg(rgba(0xffffff14))
                .child(div().h_full().w(relative(fill)).rounded_full().bg(color))
                .children(pace.map(|p| {
                    div()
                        .absolute()
                        .top(px(-3.))
                        .left(relative((p / 100.) as f32))
                        .ml(px(-1.))
                        .w(px(2.))
                        .h(px(12.))
                        .rounded(px(1.))
                        .bg(rgba(PACE_MARK))
                })),
        )
        .child(
            div()
                .w(px(52.))
                .flex_none()
                .flex()
                .justify_end()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(percentage(q.used_percent, french)),
        )
        .child(
            div()
                .w(px(112.))
                .flex_none()
                .truncate()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(reset_time(q.resets_at, now, french)),
        )
        .into_any_element()
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
        let intro = match latest {
            Some(_) => {
                // « Il y a 19 s » follows a comma: lower its first letter.
                let text = freshness(latest, now, french);
                let mut when = text.chars();
                let when = when.next().map(|c| c.to_lowercase().chain(when).collect::<String>()).unwrap_or_default();
                t(french, "{count}, relevés {when}").replace("{count}", &count).replace("{when}", &when)
            }
            None => t(french, "{count}, aucun relevé encore").replace("{count}", &count),
        };
        let refresh = button(
            "home-refresh",
            Kind::Secondary,
            Some("refresh-cw"),
            t(french, "Actualiser"),
            Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_now(cx)))),
        );
        (heading, intro, refresh)
    }

    /// A tracked account and each of its quota windows; clicking it opens its page in
    /// Analytics.
    fn account_row(&self, account: &Account, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let now = chrono::Utc::now().timestamp();
        let interval = s.config.settings.poll_seconds;
        let reading = s.readings.get(&account.id);
        let title = if account.name.is_empty() { name(&account.provider, french) } else { account.name.clone() };
        let plan = reading.and_then(|r| r.plan.clone());
        // What stands in the way of a current reading, if anything.
        let note: Option<(String, u32)> = match reading {
            None => Some((freshness(None, now, french), MUTED)),
            Some(r) if r.status != "ok" => {
                Some((t(french, status_label(&r.status)), if r.status == "needsAuth" { 0xf5b83d } else { MUTED }))
            }
            Some(_) if !is_fresh(reading, interval, now) => Some((t(french, "Ancien relevé"), MUTED)),
            Some(_) => None,
        };
        let windows = reading.map(|r| r.windows.as_slice()).unwrap_or_default();
        let lines: Vec<AnyElement> = if windows.is_empty() {
            vec![div().text_size(px(12.)).text_color(rgb(MUTED)).child(t(french, "Pas encore de quota lu")).into_any_element()]
        } else {
            windows.iter().map(|q| window_line(q, now, french)).collect()
        };
        let id = account.id.clone();
        div()
            .id(SharedString::from(format!("home-quota-{}", account.id)))
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(18.))
            .py(px(14.))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(0xffffff06)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.section = Section::Analytics;
                this.analytics_tab = crate::analytics_page::AnalyticsTab::Quotas;
                this.analytics_account = Some(id.clone());
                this.message = None;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(provider_icon(&account.provider, 16.))
                    .child(div().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(title))
                    .children(plan.map(|p| div().flex_none().text_size(px(11.)).text_color(rgb(MUTED)).child(p)))
                    .child(div().flex_1())
                    .children(note.map(|(text, color)| div().flex_none().text_size(px(11.)).text_color(rgb(color)).child(text)))
                    .child(icon("chevron-right", 14., 0x6b6b72)),
            )
            .child(div().pl(px(26.)).flex().flex_col().gap(px(8.)).children(lines))
            .into_any_element()
    }

    /// Writes the whole history to a CSV file in Downloads and shows it in Explorer.
    fn export_history(&mut self, french: bool, cx: &mut Context<Self>) {
        let csv = {
            let s = self.store.read(cx);
            let accounts = &s.config.accounts;
            s.history.csv(|id| match accounts.iter().find(|a| a.id == id) {
                Some(a) => (a.name.clone(), a.provider.clone()),
                None => (id.to_owned(), String::new()),
            })
        };
        let Some(dir) = dirs::download_dir().or_else(dirs::document_dir) else {
            return self.say(false, t(french, "Dossier Téléchargements introuvable."), cx);
        };
        let file = dir.join(format!("codenotch-history-{}.csv", chrono::Local::now().format("%Y%m%d")));
        // A BOM so Excel reads the accents as UTF-8.
        match std::fs::write(&file, format!("\u{feff}{csv}")) {
            Ok(()) => {
                let _ = std::process::Command::new("explorer")
                    .arg(format!("/select,{}", file.display()))
                    .spawn();
                self.say(true, t(french, "Historique exporté : {path}").replace("{path}", &file.display().to_string()), cx);
            }
            Err(_) => self.say(false, t(french, "Impossible d’écrire le fichier CSV."), cx),
        }
    }

    /// A setting people change often: its name, its current value, and the page it lives on.
    fn shortcut(
        &self,
        id: &'static str,
        leading: &str,
        title: String,
        detail: String,
        go: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(SharedString::from(id))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(10.))
            .min_h(px(44.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|d| d.bg(rgba(0xffffff08)))
            .child(icon(leading, 15., 0xc3c3c8))
            .child(div().flex_none().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_end()
                    .text_size(px(11.5))
                    .text_color(rgb(MUTED))
                    .child(div().truncate().child(detail)),
            )
            .child(icon("chevron-right", 14., 0x6b6b72))
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

        let quotas: AnyElement = if accounts.is_empty() {
            card()
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
                .into_any_element()
        } else {
            let rows = accounts.iter().map(|a| self.account_row(a, french, cx)).collect();
            let legend = div()
                .flex()
                .items_center()
                .gap(px(16.))
                .px(px(4.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(div().flex_none().w(px(2.)).h(px(12.)).rounded(px(1.)).bg(rgba(PACE_MARK)))
                        .child(div().truncate().child(t(
                            french,
                            "Rythme régulier : où en serait un quota dépensé à vitesse constante jusqu’à la remise à zéro.",
                        ))),
                )
                .child(text_button(
                    "home-export",
                    "download",
                    t(french, "Exporter l’historique (CSV)"),
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| this.export_history(french, cx))),
                ));
            div().flex().flex_col().gap(px(8.)).child(list(rows)).child(legend).into_any_element()
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
        let shortcuts = [
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
        let last = shortcuts.len() - 1;
        let shortcuts = div().flex().flex_col().children(shortcuts.into_iter().enumerate().map(|(i, row)| {
            div().when(i < last, |d| d.border_b_1().border_color(rgba(0xffffff0f))).child(row)
        }));

        vec![
            div().pt(px(4.)).child(quotas).into_any_element(),
            div().child(list_header(t(french, "Raccourcis"), None)).child(shortcuts).into_any_element(),
        ]
    }
}
