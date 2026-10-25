//! Quick connect (`.quick-connect`), shared by the Accounts page and the first-run welcome
//! screen (`.welcome`), which replaces the settings while `onboarding_done` is `Some(false)`.
use crate::account_form::name;
use crate::login::{self, ProviderStatus};
use crate::model::Account;
use crate::settings::{button, icon, message, Kind, SettingsView, MUTED};
use crate::ui::{provider_icon, t};
use gpui::{
    div, prelude::*, px, rgb, rgba, Animation, AnimationExt, AnyElement, ClickEvent, ClipboardItem, Context,
    FontWeight, SharedString, Transformation,
};
use std::time::Duration;

pub(crate) fn statuses() -> Vec<ProviderStatus> {
    login::PROVIDERS
        .iter()
        .map(|p| login::status(p, None))
        .collect()
}

fn install_hint(provider: &str) -> &'static str {
    match provider {
        "codex" => "npm i -g @openai/codex",
        "claude" => "irm https://claude.ai/install.ps1 | iex",
        "gemini" => "npm i -g @google/gemini-cli",
        "grok" => "x.ai/cli",
        _ => "npm i -g opencode-ai",
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let key = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    key(a) == key(b)
}

/// The profile this status describes is already in the notch.
fn tracked(accounts: &[Account], status: &ProviderStatus) -> bool {
    accounts.iter().any(|a| {
        a.provider == status.provider
            && (a.profile_path.is_empty() || same_path(&a.profile_path, &status.profile_path))
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ok,
    Warn,
    Info,
    Idle,
}

/// A status pill with its dot, as in the quick-connect mock.
fn pill(tone: Tone, text: String) -> AnyElement {
    let (fg, bg, dot) = match tone {
        Tone::Ok => (0x7fd69b, rgba(0x7fd69b1a), 0x7fd69b),
        Tone::Warn => (0xefa44f, rgba(0xefa44f1a), 0xefa44f),
        Tone::Info => (0xb8cbe6, rgba(0x24303fff), 0xb8cbe6),
        Tone::Idle => (0x9a9aa3, rgba(0xffffff0d), 0x6b6b72),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .child(div().size(px(6.)).rounded_full().bg(rgb(dot)))
        .child(text)
        .into_any_element()
}

fn group_label(text: String) -> AnyElement {
    div()
        .mb(px(8.))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0x77777f))
        .child(text.to_uppercase())
        .into_any_element()
}

/// Rows inside one rounded, divided card.
fn row_card(rows: Vec<AnyElement>, dashed: bool) -> AnyElement {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(if dashed { rgba(0xffffff1a) } else { rgba(0xffffff12) })
        .bg(rgba(0xffffff05))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(move |(i, row)| {
            div()
                .when(i + 1 < count, |d| d.border_b_1().border_color(rgba(0xffffff0f)))
                .child(row)
        }))
        .into_any_element()
}

/// The CLI path as "codex.exe · application Codex", the full path left for the tooltip.
fn short_source(path: &str, french: bool) -> (String, String) {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path).to_owned();
    let lower = path.to_lowercase();
    let place = if lower.contains("openai\\codex\\bin\\") || lower.contains("programs\\openai\\codex") {
        t(french, "application Codex")
    } else if lower.contains("claude-data") {
        t(french, "application Claude")
    } else {
        let dir = path[..path.len() - file.len()].trim_end_matches(['\\', '/']);
        match dirs::home_dir().map(|h| h.to_string_lossy().to_string()) {
            Some(home) if dir.to_lowercase().starts_with(&home.to_lowercase()) => {
                format!("~{}", &dir[home.len()..])
            }
            _ => dir.to_owned(),
        }
    };
    (file, place)
}

impl SettingsView {
    pub(crate) fn load_statuses(&mut self) {
        self.statuses = statuses();
    }

    /// Opens the provider's official sign-in from a quick-connect row.
    fn quick_sign_in(&self, provider: String, cx: &mut Context<Self>) -> crate::settings::Click {
        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| this.sign_in(provider.clone(), String::new(), cx)))
    }

    /// Signed in on this PC: the session is tracked, or only needs adding.
    fn signed_in_row(&mut self, index: usize, tracked_now: bool, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.statuses[index];
        let p = status.provider.clone();
        let path = status.cli_path.clone();
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let (status_pill, action) = if waiting {
            (pill(Tone::Info, t(french, "Connexion en cours")), self.waiting_action(french, cx))
        } else if tracked_now {
            let reconnect = div()
                .id(SharedString::from(format!("reconnect-{p}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(32.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgba(0x00000000))
                .text_size(px(12.))
                .text_color(rgb(0xc3c3c8))
                .opacity(0.7)
                .cursor_pointer()
                .hover(|d| d.opacity(1.).bg(rgba(0xffffff0f)).border_color(rgb(0x3b3b3f)).text_color(rgb(0xffffff)))
                .tooltip(crate::settings::tip(t(french, "Relancer la connexion officielle")))
                .child(icon("refresh-cw", 14., 0xc3c3c8))
                .child(t(french, "Reconnecter"))
                .when(self.login.is_none(), |d| d.on_click(self.quick_sign_in(p.clone(), cx)))
                .into_any_element();
            (pill(Tone::Ok, t(french, "Suivi dans l’encoche")), reconnect)
        } else {
            let provider = p.clone();
            let add = button(
                SharedString::from(format!("add-{p}")),
                Kind::Primary,
                Some("plus"),
                t(french, "Ajouter"),
                Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let provider = provider.clone();
                    this.run(cx, move |s| {
                        s.finish_login(&provider)
                            .map(|_| Some(t(french, "Compte enregistré. Le suivi démarrera automatiquement.")))
                    });
                    this.load_statuses();
                    this.refresh_now(cx);
                }))),
            );
            (pill(Tone::Warn, t(french, "Connecté sur ce PC")), add)
        };
        let source = path.map(|path| {
            let (file, place) = short_source(&path, french);
            div()
                .id(SharedString::from(format!("source-{p}")))
                .mt(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(rgb(0x7e7e88))
                .tooltip(crate::settings::tip(path))
                .child(icon("terminal", 12., 0x7e7e88))
                .child(div().font_family("Consolas").child(file))
                .child(div().text_color(rgb(0x5c5c64)).child("·"))
                .child(place)
        });
        let hint = (!tracked_now && !waiting)
            .then(|| t(french, "Session trouvée sur ce PC, pas encore dans l’encoche."))
            .or(waiting.then(|| t(french, "Termine dans la fenêtre ouverte, l’ajout est automatique.")));
        self.row(&p, french, status_pill, hint, source.map(|s| s.into_any_element()), action)
    }

    /// Installed but signed out, or a sign-in in progress.
    fn signed_out_row(&mut self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.statuses[index].provider.clone();
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let (status_pill, hint, action) = if waiting {
            (
                pill(Tone::Info, t(french, "Connexion en cours")),
                t(french, "Termine dans la fenêtre ouverte, l’ajout est automatique."),
                self.waiting_action(french, cx),
            )
        } else {
            let click = self.login.is_none().then(|| self.quick_sign_in(p.clone(), cx));
            (
                pill(Tone::Info, t(french, "Installé")),
                t(french, "CLI trouvé, pas encore connecté."),
                button(SharedString::from(format!("sign-in-{p}")), Kind::Primary, Some("log-in"), t(french, "Se connecter"), click),
            )
        };
        self.row(&p, french, status_pill, Some(hint), None, action)
    }

    fn waiting_action(&self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(34.))
                    .px(px(12.))
                    .rounded(px(6.))
                    .bg(rgb(0x24303f))
                    .text_size(px(12.))
                    .text_color(rgb(0xb8cbe6))
                    .child(
                        icon("loader-circle", 14., 0xb8cbe6).with_animation(
                            "quick-spinner",
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, delta| icon.with_transformation(Transformation::rotate(gpui::percentage(delta))),
                        ),
                    )
                    .child(t(french, "En attente du navigateur…")),
            )
            .child(button(
                "quick-cancel",
                Kind::Ghost,
                None,
                t(french, "Annuler"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.login = None;
                    cx.notify();
                }))),
            ))
            .into_any_element()
    }

    fn row(
        &self,
        provider: &str,
        french: bool,
        status_pill: AnyElement,
        hint: Option<String>,
        source: Option<AnyElement>,
        action: AnyElement,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(16.))
            .py(px(12.))
            .hover(|d| d.bg(rgba(0xffffff08)))
            .child(provider_icon(provider, 22.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(t(french, &name(provider, true))))
                            .child(status_pill),
                    )
                    .when_some(hint, |d, hint| d.child(div().mt(px(4.)).text_size(px(11.)).text_color(rgb(0x9999a2)).child(hint)))
                    .children(source),
            )
            .child(action)
            .into_any_element()
    }

    /// CLI not installed: the install command is the action, copyable in one click.
    fn install_card(&mut self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.statuses[index].provider.clone();
        let command = install_hint(&p).to_owned();
        let copied = self.copied.as_deref() == Some(command.as_str());
        let to_copy = command.clone();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_color(rgba(0xffffff12))
            .bg(rgba(0xffffff05))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(provider_icon(&p, 20.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(t(french, &name(&p, true))))
                            .child(div().mt(px(2.)).text_size(px(11.)).text_color(rgb(0x9999a2)).child(crate::account_form::description(&p, french))),
                    )
                    .child(pill(Tone::Idle, t(french, "CLI absent"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(12.))
                    .pr(px(4.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(0x2c2c30))
                    .bg(rgb(0x17171a))
                    .child(div().font_family("Consolas").text_size(px(11.)).text_color(rgb(0x5c9bf5)).child("$"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Consolas")
                            .text_size(px(11.5))
                            .text_color(rgb(0xd4d4db))
                            .child(command.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("copy-{p}")))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .h(px(28.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(if copied { rgb(0x7fd69b) } else { rgb(0xc3c3c8) })
                            .hover(|d| d.bg(rgba(0xffffff0f)).text_color(rgb(0xffffff)))
                            .child(icon(if copied { "check" } else { "copy" }, 13., if copied { 0x7fd69b } else { 0xc3c3c8 }))
                            .child(if copied { t(french, "Copié") } else { t(french, "Copier") })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(to_copy.clone()));
                                this.copied = Some(to_copy.clone());
                                cx.notify();
                                let done = to_copy.clone();
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(Duration::from_millis(1200)).await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.copied.as_deref() == Some(done.as_str()) {
                                            this.copied = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(0x7e7e88))
                    .child(t(french, "Puis reviens ici et clique sur « Vérifier »."))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_color(rgb(0x6b6b72))
                            .child(icon("log-in", 12., 0x6b6b72))
                            .child(t(french, "Se connecter")),
                    ),
            )
            .into_any_element()
    }

    /// Quick connect: a summary, the sessions in the notch, those to connect, and the CLIs
    /// still to install.
    pub(crate) fn quick_connect(&mut self, french: bool, heading: &str, intro: bool, cx: &mut Context<Self>) -> AnyElement {
        let accounts = self.store.read(cx).config.accounts.clone();
        let (mut in_notch, mut to_connect, mut to_install) = (vec![], vec![], vec![]);
        for i in 0..self.statuses.len() {
            let s = &self.statuses[i];
            let waiting = self.login.as_ref().is_some_and(|l| l.provider == s.provider);
            if s.signed_in && tracked(&accounts, s) && !waiting {
                in_notch.push(i);
            } else if s.signed_in || s.installed || waiting {
                to_connect.push(i);
            } else {
                to_install.push(i);
            }
        }
        let summary = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .when(!in_notch.is_empty(), |d| {
                d.child(pill(Tone::Ok, t(french, "{count} suivis").replace("{count}", &in_notch.len().to_string())))
            })
            .when(!to_install.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(10.))
                        .py(px(3.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(0x414147))
                        .text_size(px(11.))
                        .text_color(rgb(0x9a9aa3))
                        .child(t(french, "{count} à installer").replace("{count}", &to_install.len().to_string())),
                )
            })
            .child(
                div()
                    .id("quick-check")
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff0d)))
                    .tooltip(crate::settings::tip(t(french, "Vérifier à nouveau")))
                    .child(icon("refresh-cw", 15., 0xb8cbe6))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.load_statuses();
                        cx.notify();
                    })),
            );

        let in_notch_rows: Vec<AnyElement> = in_notch.iter().map(|&i| self.signed_in_row(i, true, french, cx)).collect();
        let to_connect_rows: Vec<AnyElement> = to_connect
            .iter()
            .map(|&i| {
                if self.statuses[i].signed_in {
                    self.signed_in_row(i, false, french, cx)
                } else {
                    self.signed_out_row(i, french, cx)
                }
            })
            .collect();
        let install_cards: Vec<AnyElement> = to_install.iter().map(|&i| self.install_card(i, french, cx)).collect();
        // Two cards per line, like the mock's grid.
        let mut install_grid: Vec<AnyElement> = vec![];
        let mut cards = install_cards.into_iter().peekable();
        while cards.peek().is_some() {
            let first = cards.next();
            let second = cards.next();
            install_grid.push(
                div()
                    .flex()
                    .gap(px(12.))
                    .children(first)
                    .child(second.unwrap_or_else(|| div().flex_1().into_any_element()))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(12.))
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!heading.is_empty(), |d| {
                                d.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(t(french, heading)))
                            })
                            .when(intro, |d| {
                                d.child(
                                    div()
                                        .mt(px(4.))
                                        .max_w(px(440.))
                                        .text_size(px(11.))
                                        .line_height(px(17.))
                                        .text_color(rgb(0x9999a2))
                                        .child(t(french, "Connexion officielle du fournisseur. CodeNotch ajoute le compte dès que la session est détectée — il ne voit jamais ton mot de passe.")),
                                )
                            }),
                    )
                    .child(summary),
            )
            .when(!in_notch_rows.is_empty(), |d| {
                d.child(group_label(t(french, "Dans l’encoche"))).child(row_card(in_notch_rows, false))
            })
            .when(!to_connect_rows.is_empty(), |d| {
                d.child(div().mt(px(20.)).child(group_label(t(french, "À connecter"))))
                    .child(row_card(to_connect_rows, false))
            })
            .when(!install_grid.is_empty(), |d| {
                d.child(div().mt(px(20.)).child(group_label(t(french, "Disponibles"))))
                    .child(div().flex().flex_col().gap(px(12.)).children(install_grid))
            })
            .into_any_element()
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.onboarding_done = Some(true));
    }

    pub(crate) fn welcome(&mut self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self.store.read(cx).enabled().len();
        let language = self.select(
            "welcome-language",
            if french { "Français".into() } else { "English".into() },
            [("en", "English"), ("fr", "Français")]
                .into_iter()
                .map(|(code, label)| (label.to_owned(), (code == "fr") == french, self.setting(cx, move |s| s.language = code.into())))
                .collect(),
            140.,
            cx,
        );
        let subscriptions = self.quick_connect(french, "Tes abonnements", false, cx);
        let steps = [
            ("Connecte", "Clique « Se connecter » et termine dans le navigateur."),
            ("Survole", "Passe la souris sur la languette au bord de l’écran (à droite par défaut) pour voir tes quotas."),
            ("Ajuste", "Copilot, Ollama, la position et les alertes se règlent ensuite dans les réglages."),
        ];
        let banner = self.message.clone().map(|(ok, text)| message(ok, text));

        let body = div()
            .id("welcome-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(700.))
                    .min_w_0()
                    .px(px(30.))
                    .pt(px(26.))
                    .pb(px(24.))
                    .flex()
                    .flex_col()
                    .children(banner)
                    .child(
                        div()
                            .mb(px(22.))
                            .w_full()
                            .child(
                                div()
                                    .mb(px(16.))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(12.))
                                    .child(
                                        div()
                                            .px(px(10.))
                                            .py(px(4.))
                                            .rounded_full()
                                            .bg(rgb(0x24303f))
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(0xb8cbe6))
                                            .child(t(french, "Bienvenue")),
                                    )
                                    .child(language),
                            )
                            .child(
                                div()
                                    .mb(px(8.))
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t(french, "Connecte tes abonnements IA")),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(560.))
                                    .text_size(px(12.5))
                                    .line_height(px(20.))
                                    .text_color(rgb(MUTED))
                                    .child(t(french, "CodeNotch affiche les quotas de ChatGPT, Claude, Gemini, Grok et OpenCode dans une petite encoche au bord de l’écran. Connecte au moins un compte : la connexion se fait chez le fournisseur, CodeNotch ne voit jamais ton mot de passe.")),
                            ),
                    )
                    .child(subscriptions)
                    .child(
                        div()
                            .mt(px(22.))
                            .flex()
                            .gap(px(12.))
                            .children(steps.into_iter().enumerate().map(|(i, (title, text))| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .p(px(12.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgba(0xffffff12))
                                    .child(
                                        div()
                                            .flex()
                                            .text_size(px(12.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(div().text_color(rgb(0x8f8f96)).child(format!("{}.\u{a0}", i + 1)))
                                            .child(t(french, title)),
                                    )
                                    .child(div().text_size(px(11.)).line_height(px(16.5)).text_color(rgb(0x9999a2)).child(t(french, text)))
                            })),
                    ),
            );

        let finish = button(
            "welcome-finish",
            Kind::Primary,
            Some("check"),
            t(french, "Terminer"),
            (count > 0).then(|| -> crate::settings::Click {
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx)))
            }),
        );
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(30.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgba(0xffffff12))
            .bg(rgb(0x121214))
            .child(div().flex_1().text_size(px(11.)).text_color(rgb(0x9999a2)).child(match count {
                0 => t(french, "Aucun compte connecté pour l’instant"),
                1 => t(french, "{count} compte suivi").replace("{count}", "1"),
                n => t(french, "{count} comptes suivis").replace("{count}", &n.to_string()),
            }))
            .child(button(
                "welcome-later",
                Kind::Secondary,
                None,
                t(french, "Plus tard"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx)))),
            ))
            .child(finish);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .child(footer)
            .into_any_element()
    }
}
