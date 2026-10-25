use crate::model::Settings;
use crate::assets::app_icon;
use crate::store::Store;
use crate::ui::{provider_icon, tr};
use gpui::{
    div, img, prelude::*, px, rgb, rgba, size, AnyElement, App, Bounds, ClickEvent, Context, Entity,
    FontWeight, Rgba, SharedString, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowControlArea, WindowKind, WindowOptions,
};

const SURFACE: u32 = 0x0e0e10;
const SIDEBAR: u32 = 0x161618;
const CARD: u32 = 0x151517;
pub(crate) const MUTED: u32 = 0xa3a3a8;
pub(crate) const TEXT: u32 = 0xf5f5f5;
pub(crate) const ACCENT: u32 = 0x5c9bf5;
pub(crate) const DANGER: u32 = 0xff6b5a;

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Accounts,
    Language,
    Alerts,
}

pub(crate) type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Opens the settings window, or brings the existing one to the front.
pub fn open(store: Entity<Store>, cx: &mut App) {
    if let Some(handle) = store.read(cx).settings_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let french = store.read(cx).french();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(820.), px(620.)),
            cx,
        ))),
        // The header is drawn by the app, Cursor style; the title still names the window
        // in the taskbar and Alt+Tab.
        titlebar: Some(TitlebarOptions {
            title: Some(title(french).into()),
            appears_transparent: true,
            ..Default::default()
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_resizable: true,
        window_min_size: Some(size(px(640.), px(480.))),
        ..Default::default()
    };
    let handle: Option<WindowHandle<SettingsView>> = cx
        .open_window(options, |window, cx| {
            cx.new(|cx| SettingsView::new(store.clone(), window, cx))
        })
        .ok();
    store.update(cx, |s, _| s.settings_window = handle);
}

fn title(french: bool) -> String {
    tr(french, "CodeNotch — Réglages", "CodeNotch — Settings")
}

pub struct SettingsView {
    pub(crate) store: Entity<Store>,
    section: Section,
    /// Result of the last action: (success, text).
    pub(crate) message: Option<(bool, String)>,
    confirm_remove: Option<String>,
    /// The account being added or edited.
    pub(crate) editor: Option<crate::account_form::Editor>,
    /// A CLI sign-in in progress.
    pub(crate) login: Option<crate::account_form::Login>,
    pub(crate) channels: crate::channels::Channels,
}

impl SettingsView {
    fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A transparent titlebar drops the initial title, which the taskbar and Alt+Tab show.
        window.set_window_title(&title(store.read(cx).french()));
        cx.observe_in(&store, window, |_, store, window, cx| {
            window.set_window_title(&title(store.read(cx).french()));
            cx.notify();
        })
        .detach();
        let (french, chat) = {
            let s = store.read(cx);
            (s.french(), s.config.settings.telegram_chat_id.clone())
        };
        let channels = crate::channels::Channels::new(french, chat, cx);
        Self {
            store,
            section: Section::Accounts,
            message: None,
            confirm_remove: None,
            editor: None,
            login: None,
            channels,
        }
    }

    pub(crate) fn say(&mut self, ok: bool, text: String, cx: &mut Context<Self>) {
        self.message = Some((ok, text));
        cx.notify();
    }

    pub(crate) fn run(
        &mut self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Store) -> Result<Option<String>, String>,
    ) {
        let result = self.store.update(cx, |s, cx| {
            let result = action(s);
            cx.notify();
            result
        });
        self.message = match result {
            Ok(Some(text)) => Some((true, text)),
            Ok(None) => None,
            Err(error) => Some((false, error)),
        };
        cx.notify();
    }

    pub(crate) fn update_settings(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.store.read(cx).config.settings.clone();
        change(&mut settings);
        self.run(cx, move |s| s.save_settings(settings).map(|_| None));
    }

    /// A click handler that edits the settings and saves them.
    pub(crate) fn setting(
        &self,
        cx: &mut Context<Self>,
        change: impl Fn(&mut Settings) + 'static,
    ) -> Click {
        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.update_settings(cx, &change)
        }))
    }
}

pub(crate) fn toggle(id: impl Into<SharedString>, checked: bool, on_click: Click) -> AnyElement {
    div()
        .id(id.into())
        .w(px(38.))
        .h(px(22.))
        .flex_none()
        .p(px(3.))
        .rounded_full()
        .cursor_pointer()
        .bg(if checked { rgb(ACCENT) } else { rgb(0x3a3a3c) })
        .flex()
        .when(checked, |d| d.justify_end())
        .child(div().size(px(16.)).rounded_full().bg(rgb(0xffffff)))
        .on_click(on_click)
        .into_any_element()
}

pub(crate) fn button(id: impl Into<SharedString>, label: String, color: Rgba, on_click: Click) -> AnyElement {
    div()
        .id(id.into())
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgba(0xffffff1f))
        .cursor_pointer()
        .text_sm()
        .text_color(color)
        .hover(|d| d.bg(rgba(0xffffff12)))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

pub(crate) fn segmented(choices: Vec<(SharedString, String, bool, Click)>) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .p(px(3.))
        .gap(px(2.))
        .rounded_lg()
        .bg(rgb(0x1f1f22))
        .children(choices.into_iter().map(|(id, label, active, on_click)| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .when(active, |d| d.bg(rgb(0x355d8b)).text_color(rgb(TEXT)))
                .when(!active, |d| {
                    d.text_color(rgb(MUTED)).hover(|d| d.text_color(rgb(TEXT)))
                })
                .child(label)
                .on_click(on_click)
        }))
        .into_any_element()
}

fn stepper(id: &str, value: String, minus: Option<Click>, plus: Option<Click>) -> AnyElement {
    let side = |key: &str, glyph: &'static str, handler: Option<Click>| {
        let enabled = handler.is_some();
        let mut d = div()
            .id(SharedString::from(format!("{id}-{key}")))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .child(glyph);
        if let Some(handler) = handler {
            d = d
                .cursor_pointer()
                .hover(|d| d.bg(rgba(0xffffff14)))
                .on_click(handler);
        }
        d.text_color(if enabled { rgb(TEXT) } else { rgb(0x555555) })
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_1()
        .p(px(3.))
        .rounded_lg()
        .bg(rgb(0x1f1f22))
        .child(side("minus", "−", minus))
        .child(
            div()
                .w(px(56.))
                .flex()
                .justify_center()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(value),
        )
        .child(side("plus", "+", plus))
        .into_any_element()
}

/// A labelled setting: title and help on the left, its control on the right.
pub(crate) fn row(title: String, help: String, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .py_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(div().font_weight(FontWeight::BOLD).child(title))
                .child(div().text_sm().text_color(rgb(MUTED)).child(help)),
        )
        .child(control)
        .into_any_element()
}

pub(crate) fn card(title: String, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .px_5()
        .py_3()
        .rounded_xl()
        .bg(rgb(CARD))
        .border_1()
        .border_color(rgba(0xffffff12))
        .child(
            div()
                .pb_2()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(MUTED))
                .child(title.to_uppercase()),
        )
        .children(rows)
        .into_any_element()
}

pub(crate) fn note(text: String) -> AnyElement {
    div()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(rgba(0x5c9bf514))
        .border_1()
        .border_color(rgba(0x5c9bf533))
        .text_sm()
        .text_color(rgb(0xc0c6cf))
        .child(text)
        .into_any_element()
}

impl SettingsView {
    fn accounts(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.editor.is_some() {
            return self.account_editor(french, cx);
        }
        let accounts = self.store.read(cx).config.accounts.clone();
        let add = button(
            "add-account",
            tr(french, "+ Ajouter", "+ Add"),
            rgb(TEXT),
            Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.open_editor(None, window, cx)
            })),
        );
        let detect = button(
            "detect",
            tr(french, "Détecter", "Detect"),
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.run(cx, |s| {
                    s.add_discovered().map(|added| {
                        Some(match added {
                            0 => tr(
                                french,
                                "Aucun nouveau profil trouvé sur ce PC.",
                                "No new profile found on this PC.",
                            ),
                            n => tr(
                                french,
                                &format!("{n} profil(s) ajouté(s)."),
                                &format!("{n} profile(s) added."),
                            ),
                        })
                    })
                })
            })),
        );
        let mut rows: Vec<AnyElement> = vec![div()
            .flex()
            .items_center()
            .justify_between()
            .pb_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(tr(
                        french,
                        "Détecte les profils Codex, Claude, Gemini, Grok et OpenCode déjà connectés sur ce PC.",
                        "Finds the Codex, Claude, Gemini, Grok and OpenCode profiles already signed in on this PC.",
                    )),
            )
            .child(div().flex().flex_none().gap_2().child(detect).child(add))
            .into_any_element()];

        if accounts.is_empty() {
            rows.push(
                div()
                    .py_6()
                    .flex()
                    .justify_center()
                    .text_color(rgb(MUTED))
                    .child(tr(french, "Aucun compte configuré", "No account configured"))
                    .into_any_element(),
            );
        }
        for account in accounts {
            let id = account.id.clone();
            let confirming = self.confirm_remove.as_deref() == Some(id.as_str());
            let enabled = account.enabled;
            let toggle_id = id.clone();
            let actions = if confirming {
                let (remove_id, keep_id) = (id.clone(), id.clone());
                div()
                    .flex()
                    .gap_2()
                    .child(button(
                        format!("confirm-{keep_id}"),
                        tr(french, "Confirmer le retrait", "Confirm removal"),
                        rgb(DANGER),
                        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.confirm_remove = None;
                            let id = remove_id.clone();
                            this.run(cx, move |s| {
                                s.remove_account(&id).map(|_| {
                                    Some(tr(
                                        french,
                                        "Compte retiré de CodeNotch.",
                                        "Account removed from CodeNotch.",
                                    ))
                                })
                            })
                        })),
                    ))
                    .child(button(
                        format!("keep-{keep_id}"),
                        tr(french, "Garder", "Keep"),
                        rgb(TEXT),
                        Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.confirm_remove = None;
                            cx.notify();
                        })),
                    ))
                    .into_any_element()
            } else {
                let ask_id = id.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(button(
                        format!("remove-{id}"),
                        tr(french, "Retirer", "Remove"),
                        rgb(MUTED),
                        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.confirm_remove = Some(ask_id.clone());
                            cx.notify();
                        })),
                    ))
                    .child(toggle(
                        format!("enabled-{id}"),
                        enabled,
                        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let id = toggle_id.clone();
                            this.run(cx, move |s| s.set_enabled(&id, !enabled).map(|_| None))
                        })),
                    ))
                    .into_any_element()
            };
            let path = if account.profile_path.is_empty() {
                account.endpoint.clone()
            } else {
                account.profile_path.clone()
            };
            let edited = account.clone();
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_3()
                    .border_t_1()
                    .border_color(rgba(0xffffff0d))
                    .child(
                        div()
                            .id(SharedString::from(format!("edit-{id}")))
                            .flex()
                            .items_center()
                            .gap_3()
                            .flex_1()
                            .min_w_0()
                            .p_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|d| d.bg(rgba(0xffffff08)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.open_editor(Some(edited.clone()), window, cx)
                            }))
                            .child(
                        div()
                            .size(px(36.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(rgb(0x1f1f22))
                            .border_1()
                            .border_color(rgba(0xffffff14))
                            .child(provider_icon(&account.provider, 20.)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_weight(FontWeight::BOLD).child(account.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .truncate()
                                    .child(format!(
                                        "{} · {}",
                                        crate::account_form::name(&account.provider, french),
                                        path
                                    )),
                            ),
                    ))
                    .child(actions)
                    .into_any_element(),
            );
        }
        vec![
            card(tr(french, "Comptes configurés", "Configured accounts"), rows),
            note(tr(
                french,
                "Les sessions existantes restent dans leur outil d’origine. Les secrets ajoutés ici sont conservés dans le Gestionnaire d’identification Windows.",
                "Existing sessions stay in their own tool. Secrets added here are kept in Windows Credential Manager.",
            )),
        ]
    }

    fn language(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let choices = [("fr", "Français"), ("en", "English")]
            .into_iter()
            .map(|(code, label)| {
                (
                    SharedString::from(format!("lang-{code}")),
                    label.to_owned(),
                    (code == "fr") == french,
                    self.setting(cx, move |s| s.language = code.into()),
                )
            })
            .collect();
        vec![card(
            tr(french, "Langue", "Language"),
            vec![row(
                tr(french, "Langue de l’interface", "Interface language"),
                tr(
                    french,
                    "S’applique immédiatement à l’encoche, à ce panneau et au menu de l’icône.",
                    "Applies right away to the notch, this window and the tray menu.",
                ),
                segmented(choices),
            )],
        )]
    }

    fn alerts(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx).config.settings.clone();
        let percent_stepper = |this: &Self,
                               cx: &mut Context<Self>,
                               id: &str,
                               value: u8,
                               set: fn(&mut Settings, u8)| {
            let minus = (value > 50).then(|| this.setting(cx, move |s| set(s, value - 5)));
            let plus = (value < 100).then(|| this.setting(cx, move |s| set(s, value + 5)));
            stepper(id, format!("{value} %"), minus, plus)
        };
        let alert = percent_stepper(self, cx, "alert", s.alert_percent, |s, v| {
            s.alert_percent = v
        });
        let critical = percent_stepper(self, cx, "critical", s.critical_percent, |s, v| {
            s.critical_percent = v
        });
        let minutes = [15u16, 30, 60]
            .into_iter()
            .map(|n| {
                (
                    SharedString::from(format!("forecast-{n}")),
                    format!("{n} min"),
                    s.forecast_minutes == n,
                    self.setting(cx, move |s| s.forecast_minutes = n),
                )
            })
            .collect();
        vec![
            note(tr(
                french,
                "Prototype : les alertes ne sont pas encore envoyées. Ces réglages sont enregistrés et serviront dès leur portage.",
                "Prototype: alerts are not sent yet. These settings are saved and will apply once alerts are ported.",
            )),
            card(
                tr(french, "Seuils et notifications Windows", "Thresholds and Windows notifications"),
                vec![
                    row(
                        tr(french, "Notifications de quota", "Quota notifications"),
                        tr(
                            french,
                            "Une notification Windows à chaque niveau franchi, sans répétition tant qu’il reste dépassé.",
                            "One Windows notification per level crossed, not repeated while it stays exceeded.",
                        ),
                        toggle("notifications", s.notifications, self.setting(cx, |s| s.notifications = !s.notifications)),
                    ),
                    row(
                        tr(french, "Me prévenir à", "Warn me at"),
                        tr(french, "Pourcentage consommé d’une fenêtre de quota.", "Share of a quota window used."),
                        alert,
                    ),
                    row(
                        tr(french, "Seuil critique", "Critical level"),
                        tr(
                            french,
                            "Une seconde alerte, plus urgente. Le plafond atteint est toujours signalé.",
                            "A second, more urgent alert. Reaching the limit is always reported.",
                        ),
                        critical,
                    ),
                    row(
                        tr(french, "Alertes urgentes en plein écran", "Urgent alerts in full screen"),
                        tr(
                            french,
                            "La prévision, le seuil critique et le plafond s’affichent même pendant un jeu ou en mode jeu.",
                            "Forecast, critical and limit alerts show even during a game or in game mode.",
                        ),
                        toggle(
                            "critical-fullscreen",
                            s.critical_in_fullscreen,
                            self.setting(cx, |s| s.critical_in_fullscreen = !s.critical_in_fullscreen),
                        ),
                    ),
                ],
            ),
            card(
                tr(french, "Prévision du plafond", "Limit forecast"),
                vec![
                    row(
                        tr(french, "Prévenir avant le plafond", "Warn before the limit"),
                        tr(
                            french,
                            "Mesure le rythme de consommation et prévient quand le plafond arrive avant la réinitialisation.",
                            "Measures the pace of use and warns when the limit comes before the reset.",
                        ),
                        toggle("forecast", s.forecast_alerts, self.setting(cx, |s| s.forecast_alerts = !s.forecast_alerts)),
                    ),
                    row(
                        tr(french, "Anticipation", "Lead time"),
                        tr(
                            french,
                            "Délai avant le plafond estimé à partir duquel l’alerte part.",
                            "How long before the estimated limit the alert goes out.",
                        ),
                        segmented(minutes),
                    ),
                    row(
                        tr(french, "Prévenir quand le quota est rétabli", "Tell me when the quota is back"),
                        tr(
                            french,
                            "Après une réinitialisation, pour relancer les agents mis en pause.",
                            "After a reset, to restart paused agents.",
                        ),
                        toggle("reset", s.reset_alerts, self.setting(cx, |s| s.reset_alerts = !s.reset_alerts)),
                    ),
                ],
            ),
            self.discord_card(french, cx),
            self.telegram_card(french, cx),
        ]
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let french = self.store.read(cx).french();
        let (heading, intro, content) = match self.section {
            Section::Accounts => (
                tr(french, "Comptes", "Accounts"),
                tr(french, "Choisissez les comptes affichés dans l’encoche.", "Choose the accounts shown in the notch."),
                self.accounts(french, cx),
            ),
            Section::Language => (
                tr(french, "Langue", "Language"),
                tr(french, "Langue de l’encoche et des réglages.", "Language of the notch and settings."),
                self.language(french, cx),
            ),
            Section::Alerts => (
                tr(french, "Alertes", "Alerts"),
                tr(
                    french,
                    "Seuils et prévision du plafond, pour surveiller les agents qui tournent en fond.",
                    "Thresholds and limit forecast, to watch agents running in the background.",
                ),
                self.alerts(french, cx),
            ),
        };

        let nav = [
            (Section::Accounts, tr(french, "Comptes", "Accounts")),
            (Section::Language, tr(french, "Langue", "Language")),
            (Section::Alerts, tr(french, "Alertes", "Alerts")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (section, label))| {
            let active = self.section == section;
            div()
                .id(("nav", i))
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .when(active, |d| d.bg(rgba(0xffffff14)).text_color(rgb(TEXT)))
                .when(!active, |d| {
                    d.text_color(rgb(MUTED)).hover(|d| d.text_color(rgb(TEXT)))
                })
                .child(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.section = section;
                    this.message = None;
                    this.confirm_remove = None;
                    cx.notify();
                }))
        });

        let storage_error = self.store.read(cx).storage_error.clone();
        let message = self.message.clone();
        let section_label = heading.clone();

        let main = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .relative()
            .child(
                div()
                    .w(px(200.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .bg(rgb(SIDEBAR))
                    .border_r_1()
                    .border_color(rgba(0xffffff12))
                    .child(
                        div()
                            .px_3()
                            .pt_1()
                            .pb_2()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x6b6b70))
                            .child(tr(french, "RÉGLAGES", "SETTINGS")),
                    )
                    .children(nav)
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_3()
                            .text_xs()
                            .text_color(rgb(0x6b6b70))
                            .child(format!("GPUI · {}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .px_8()
                            .py_6()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_2xl().font_weight(FontWeight::BOLD).child(heading))
                                    .child(div().text_sm().text_color(rgb(MUTED)).child(intro)),
                            )
                            .when_some(storage_error, |d, error| {
                                d.child(div().text_sm().text_color(rgb(DANGER)).child(error))
                            })
                            .children(content)
                            // Room for the toast below the last card.
                            .child(div().h(px(48.))),
                    ),
            )
            // Results float at the bottom so they stay visible however far the page scrolls.
            .when_some(message, |d, (ok, text)| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(16.))
                        .left(px(224.))
                        .right(px(24.))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .id("toast")
                                .max_w(px(560.))
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .border_1()
                                .text_sm()
                                .cursor_pointer()
                                .bg(if ok { rgb(0x10261a) } else { rgb(0x2a1412) })
                                .border_color(if ok { rgba(0x28e07b55) } else { rgba(0xff6b5a55) })
                                .text_color(if ok { rgb(0x7ee2a8) } else { rgb(DANGER) })
                                .child(text)
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.message = None;
                                    cx.notify();
                                })),
                        ),
                )
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .child(titlebar(french, &section_label, window.is_maximized()))
            .child(main)
    }
}

/// A frameless, Cursor-like header: logo and name, a context pill in the middle, and flat
/// Windows caption buttons. The bar is a caption hit-test area, so Windows drags and snaps
/// the window from it.
fn titlebar(french: bool, section: &str, maximized: bool) -> AnyElement {
    // Plain buttons rather than caption hit-test areas: occluding the drag area under them
    // keeps them from starting a window drag, and a click always reaches them.
    let caption = |id: &'static str, glyph: &'static str, close: bool, action: fn(&mut Window)| {
        div()
            .id(id)
            .occlude()
            .w(px(46.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .font_family("Segoe Fluent Icons")
            .text_size(px(10.))
            .text_color(rgb(0xc8c8cc))
            .when(close, |d| d.hover(|d| d.bg(rgb(0xc42b1c)).text_color(rgb(0xffffff))))
            .when(!close, |d| d.hover(|d| d.bg(rgba(0xffffff12))))
            .child(glyph)
            .on_click(move |_, window, _| action(window))
    };
    div()
        .id("titlebar")
        .h(px(40.))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .bg(rgb(0x0a0a0c))
        .border_b_1()
        .border_color(rgba(0xffffff12))
        .window_control_area(WindowControlArea::Drag)
        .child(
            div()
                .w(px(200.))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .pl_3()
                .child(img(app_icon()).size(px(20.)).rounded_md())
                .child(div().text_sm().font_weight(FontWeight::BOLD).child("CodeNotch"))
                .child(div().text_xs().text_color(rgb(MUTED)).child("Windows")),
        )
        .child(
            div().flex_1().min_w_0().flex().justify_center().child(
                div()
                    .w(px(380.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .rounded_md()
                    .bg(rgb(0x18181b))
                    .border_1()
                    .border_color(rgba(0xffffff14))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(
                        div()
                            .font_family("Segoe Fluent Icons")
                            .text_size(px(10.))
                            .child("\u{E721}"),
                    )
                    .child(format!("{} › {}", tr(french, "Réglages", "Settings"), section)),
            ),
        )
        .child(
            div()
                .w(px(200.))
                .h_full()
                .flex_none()
                .flex()
                .justify_end()
                .child(caption("min", "\u{E921}", false, |w| w.minimize_window()))
                .child(caption(
                    "max",
                    if maximized { "\u{E923}" } else { "\u{E922}" },
                    false,
                    |w| w.zoom_window(),
                ))
                .child(caption("close", "\u{E8BB}", true, |w| w.remove_window())),
        )
        .into_any_element()
}
