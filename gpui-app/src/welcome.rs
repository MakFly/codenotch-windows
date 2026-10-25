//! Quick connect (`.quick-connect`), shared by the Accounts page and the first-run welcome
//! screen (`.welcome`), which replaces the settings while `onboarding_done` is `Some(false)`.
use crate::account_form::name;
use crate::login::{self, ProviderStatus};
use crate::model::Account;
use crate::settings::{button, list_header, message, text_button, Kind, SettingsView, MUTED};
use crate::ui::{provider_icon, t};
use gpui::{div, prelude::*, px, rgb, rgba, AnyElement, ClickEvent, Context, FontWeight, SharedString};

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

fn code(text: String) -> impl IntoElement {
    div().font_family("Consolas").child(text)
}

impl SettingsView {
    pub(crate) fn load_statuses(&mut self) {
        self.statuses = statuses();
    }

    fn quick_row(&mut self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.statuses[index];
        let accounts = self.store.read(cx).config.accounts.clone();
        let p = status.provider.clone();
        let (installed, signed_in, cli_path) = (status.installed, status.signed_in, status.cli_path.clone());
        let is_tracked = tracked(&accounts, status);
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let busy = self.login.is_some();
        let state = if !installed && !signed_in {
            t(french, "CLI non installé")
        } else if signed_in {
            if is_tracked {
                t(french, "Connecté · suivi dans l’encoche")
            } else {
                t(french, "Connecté sur ce PC · pas encore ajouté")
            }
        } else {
            t(french, "Installé · pas encore connecté")
        };
        let id = SharedString::from(format!("quick-{p}"));
        let action = if waiting {
            button(
                id,
                Kind::Secondary,
                Some("x"),
                t(french, "Annuler"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.login = None;
                    cx.notify();
                }))),
            )
        } else if signed_in && !is_tracked {
            let provider = p.clone();
            button(
                id,
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
            )
        } else {
            let provider = p.clone();
            button(
                id,
                if signed_in { Kind::Secondary } else { Kind::Primary },
                Some("log-in"),
                if signed_in { t(french, "Reconnecter") } else { t(french, "Se connecter") },
                (installed && !busy).then(|| -> crate::settings::Click {
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.sign_in(provider.clone(), String::new(), cx)
                    }))
                }),
            )
        };
        let desktop_app = cli_path.as_deref().is_some_and(|path| {
            let lower = path.to_lowercase();
            lower.contains("claude-data") || lower.contains("openai\\codex\\bin\\")
        });
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(11.))
            .border_b_1()
            .border_color(rgba(0xffffff12))
            .child(div().mx(px(4.)).child(provider_icon(&p, 23.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .flex_1()
                    .min_w_0()
                    .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(t(french, &name(&p, true))))
                    .child(div().text_size(px(11.)).line_height(px(16.)).text_color(rgb(0x9999a2)).child(state))
                    .when_some(cli_path, |d, path| {
                        d.child(
                            div()
                                .flex()
                                .gap_1()
                                .text_size(px(10.5))
                                .text_color(rgb(0x7e7e88))
                                .child(if desktop_app {
                                    t(french, "Détecté dans l’application de bureau :")
                                } else {
                                    t(french, "Détecté :")
                                })
                                .child(code(path)),
                        )
                    })
                    .when(!installed, |d| {
                        d.child(
                            div()
                                .flex()
                                .gap_1()
                                .text_size(px(11.))
                                .text_color(rgb(0x9999a2))
                                .child(t(french, "Installer :"))
                                .child(code(install_hint(&p).into())),
                        )
                    }),
            )
            .child(action)
            .into_any_element()
    }

    /// `.quick-connect`: sign in to each CLI provider, or add a session found on this PC.
    pub(crate) fn quick_connect(&mut self, french: bool, heading: &str, intro: bool, cx: &mut Context<Self>) -> AnyElement {
        let check = text_button(
            "quick-check",
            "refresh-cw",
            t(french, "Vérifier"),
            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.load_statuses();
                cx.notify();
            })),
        );
        let rows: Vec<AnyElement> = (0..self.statuses.len()).map(|i| self.quick_row(i, french, cx)).collect();
        let pending = self.login.as_ref().map(|_| {
            div()
                .mt(px(12.))
                .p(px(12.))
                .rounded(px(6.))
                .bg(rgb(0x24303f))
                .text_size(px(12.))
                .text_color(rgb(0xc4d5ee))
                .child(t(french, "Termine la connexion dans le navigateur ou la console ouverte. CodeNotch ajoute le compte dès que la session est détectée."))
        });
        div()
            .flex()
            .flex_col()
            .child(list_header(t(french, heading), Some(check)))
            .when(intro, |d| {
                d.child(
                    div()
                        .mb(px(4.))
                        .text_size(px(11.))
                        .line_height(px(16.5))
                        .text_color(rgb(0x9999a2))
                        .child(t(french, "Connecte ChatGPT, Claude, Gemini, Grok ou OpenCode avec leur connexion officielle. CodeNotch ouvre la fenêtre de connexion, puis ajoute le compte dès que la session est détectée.")),
                )
            })
            .children(rows)
            .children(pending)
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
