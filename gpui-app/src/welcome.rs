//! First-run screen: connect the AI subscriptions before reaching the settings, as in the
//! Tauri app. It replaces the settings while `onboarding_done` is `Some(false)`.
use crate::account_form::name;
use crate::login::{self, ProviderStatus};
use crate::model::Account;
use crate::settings::{button, segmented, SettingsView, MUTED, TEXT};
use crate::ui::{provider_icon, tr};
use gpui::{
    div, prelude::*, px, rgb, rgba, AnyElement, ClickEvent, Context, FontWeight, SharedString,
};

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

fn primary(id: &'static str, label: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px_4()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .bg(rgb(0x355d8b))
        .hover(|d| d.bg(rgb(0x3f6c9f)))
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .child(label)
}

impl SettingsView {
    pub(crate) fn load_statuses(&mut self) {
        self.statuses = statuses();
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.onboarding_done = Some(true));
    }

    fn quick_row(&mut self, status: &ProviderStatus, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let accounts = self.store.read(cx).config.accounts.clone();
        let p = status.provider.clone();
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let busy = self.login.is_some();
        let is_tracked = tracked(&accounts, status);
        let state = if !status.installed && !status.signed_in {
            tr(french, "CLI non installé", "CLI not installed")
        } else if status.signed_in {
            if is_tracked {
                tr(french, "Connecté · suivi dans l’encoche", "Signed in · shown in the notch")
            } else {
                tr(french, "Connecté sur ce PC · pas encore ajouté", "Signed in on this PC · not added yet")
            }
        } else {
            tr(french, "Installé · pas encore connecté", "Installed · not signed in yet")
        };
        let id = SharedString::from(format!("quick-{p}"));
        let action: AnyElement = if waiting {
            button(
                id,
                tr(french, "Annuler", "Cancel"),
                rgb(TEXT),
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.login = None;
                    cx.notify();
                })),
            )
        } else if status.signed_in && !is_tracked {
            let provider = p.clone();
            div()
                .id(id)
                .flex_none()
                .px_4()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .bg(rgb(0x355d8b))
                .hover(|d| d.bg(rgb(0x3f6c9f)))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .child(tr(french, "+ Ajouter", "+ Add"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let provider = provider.clone();
                    this.run(cx, move |s| {
                        s.finish_login(&provider).map(|_| {
                            Some(tr(
                                french,
                                "Compte enregistré. Le suivi démarrera automatiquement.",
                                "Account saved. Tracking starts automatically.",
                            ))
                        })
                    });
                    this.load_statuses();
                    this.refresh_now(cx);
                }))
                .into_any_element()
        } else if !status.installed || busy {
            div()
                .flex_none()
                .px_4()
                .py_1p5()
                .rounded_md()
                .border_1()
                .border_color(rgba(0xffffff12))
                .text_sm()
                .text_color(rgb(0x5a5a60))
                .child(tr(french, "Se connecter", "Sign in"))
                .into_any_element()
        } else {
            let provider = p.clone();
            let label = if status.signed_in {
                tr(french, "Reconnecter", "Sign in again")
            } else {
                tr(french, "Se connecter", "Sign in")
            };
            let el = if status.signed_in {
                div()
                    .id(id)
                    .flex_none()
                    .px_4()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .hover(|d| d.bg(rgba(0xffffff12)))
                    .text_sm()
                    .child(label)
            } else {
                div()
                    .id(id)
                    .flex_none()
                    .px_4()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(0x355d8b))
                    .hover(|d| d.bg(rgb(0x3f6c9f)))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(label)
            };
            el.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.sign_in(provider.clone(), String::new(), cx)
            }))
            .into_any_element()
        };
        let desktop_app = status.cli_path.as_deref().is_some_and(|path| {
            let lower = path.to_lowercase();
            lower.contains("claude-data") || lower.contains("openai\\codex\\bin\\")
        });
        div()
            .flex()
            .items_center()
            .gap_3()
            .py_3()
            .border_t_1()
            .border_color(rgba(0xffffff0d))
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
                    .child(provider_icon(&p, 20.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_weight(FontWeight::BOLD).child(name(&p, french)))
                    .child(div().text_sm().text_color(rgb(MUTED)).child(state))
                    .when_some(status.cli_path.clone(), |d, path| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x6b6b70))
                                .truncate()
                                .child(format!(
                                    "{} {path}",
                                    if desktop_app {
                                        tr(french, "Détecté dans l’application de bureau :", "Found in the desktop app:")
                                    } else {
                                        tr(french, "Détecté :", "Found:")
                                    }
                                )),
                        )
                    })
                    .when(!status.installed, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x6b6b70))
                                .child(format!("{} {}", tr(french, "Installer :", "Install:"), install_hint(&p))),
                        )
                    }),
            )
            .child(action)
            .into_any_element()
    }

    pub(crate) fn welcome(&mut self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self
            .store
            .read(cx)
            .config
            .accounts
            .iter()
            .filter(|a| a.enabled)
            .count();
        let languages = [("en", "English"), ("fr", "Français")]
            .into_iter()
            .map(|(code, label)| {
                (
                    SharedString::from(format!("welcome-lang-{code}")),
                    label.to_owned(),
                    (code == "fr") == french,
                    self.setting(cx, move |s| s.language = code.into()),
                )
            })
            .collect();
        let statuses = self.statuses.clone_rows();
        let rows: Vec<AnyElement> = statuses.iter().map(|s| self.quick_row(s, french, cx)).collect();
        let pending = self.login.as_ref().map(|_| {
            div()
                .pt_2()
                .text_sm()
                .text_color(rgb(0x8dbbff))
                .child(tr(
                    french,
                    "Connexion en cours… Termine-la dans le navigateur ou la console ouverte.",
                    "Signing in… Finish in the browser or the console that opened.",
                ))
        });
        let steps = [
            (
                tr(french, "Connecte", "Connect"),
                tr(french, "Clique « Se connecter » et termine dans le navigateur.", "Click “Sign in” and finish in the browser."),
            ),
            (
                tr(french, "Survole", "Hover"),
                tr(
                    french,
                    "Passe la souris sur la languette au bord de l’écran (à droite par défaut) pour voir tes quotas.",
                    "Move the pointer onto the tab at the screen edge (right by default) to see your quotas.",
                ),
            ),
            (
                tr(french, "Ajuste", "Adjust"),
                tr(
                    french,
                    "Copilot, Ollama, la position et les alertes se règlent ensuite dans les réglages.",
                    "Copilot, Ollama, the position and the alerts are set up later in the settings.",
                ),
            ),
        ];
        let message = self.message.clone();

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
                    .max_w(px(680.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .px_8()
                    .py_8()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .bg(rgba(0x28e07b1f))
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0x7ee2a8))
                                    .child(tr(french, "Bienvenue", "Welcome")),
                            )
                            .child(segmented(languages)),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_3xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child(tr(french, "Connecte tes abonnements IA", "Connect your AI subscriptions")),
                            )
                            .child(div().w_full().text_color(rgb(MUTED)).child(tr(
                                french,
                                "CodeNotch affiche les quotas de ChatGPT, Claude, Gemini, Grok et OpenCode dans une petite encoche au bord de l’écran. Connecte au moins un compte : la connexion se fait chez le fournisseur, CodeNotch ne voit jamais ton mot de passe.",
                                "CodeNotch shows your ChatGPT, Claude, Gemini, Grok and OpenCode quotas in a small notch at the screen edge. Connect at least one account: you sign in with the provider, CodeNotch never sees your password.",
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px_5()
                            .py_3()
                            .rounded_xl()
                            .bg(rgb(0x151517))
                            .border_1()
                            .border_color(rgba(0xffffff12))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .pb_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(MUTED))
                                            .child(tr(french, "TES ABONNEMENTS", "YOUR SUBSCRIPTIONS")),
                                    )
                                    .child(button(
                                        "welcome-check",
                                        tr(french, "↻ Vérifier", "↻ Check"),
                                        rgb(TEXT),
                                        Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                            this.load_statuses();
                                            cx.notify();
                                        })),
                                    )),
                            )
                            .children(rows)
                            .children(pending),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .children(steps.into_iter().enumerate().map(|(i, (title, text))| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .p_4()
                                    .rounded_xl()
                                    .bg(rgb(0x121214))
                                    .border_1()
                                    .border_color(rgba(0xffffff0d))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .size(px(20.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded_full()
                                                    .bg(rgb(0x1f1f22))
                                                    .text_xs()
                                                    .child(format!("{}", i + 1)),
                                            )
                                            .child(div().font_weight(FontWeight::BOLD).child(title)),
                                    )
                                    .child(div().text_sm().text_color(rgb(MUTED)).child(text))
                            })),
                    ),
            );

        let finish = if count > 0 {
            primary("welcome-finish", tr(french, "✓ Terminer", "✓ Done"))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx)))
                .into_any_element()
        } else {
            div()
                .px_4()
                .py_1p5()
                .rounded_md()
                .bg(rgb(0x1f2a38))
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0x5a6a80))
                .child(tr(french, "✓ Terminer", "✓ Done"))
                .into_any_element()
        };
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .px_8()
            .py_3()
            .border_t_1()
            .border_color(rgba(0xffffff12))
            .bg(rgb(0x0b0b0d))
            .child(div().flex_1().text_sm().text_color(rgb(MUTED)).child(match count {
                0 => tr(french, "Aucun compte connecté pour l’instant", "No account connected yet"),
                1 => tr(french, "1 compte suivi", "1 account tracked"),
                n => tr(french, &format!("{n} comptes suivis"), &format!("{n} accounts tracked")),
            }))
            .child(button(
                "welcome-later",
                tr(french, "Plus tard", "Later"),
                rgb(TEXT),
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx))),
            ))
            .child(finish);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .child(body)
            .child(footer)
            .when_some(message, |d, (ok, text)| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(72.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .max_w(px(560.))
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .border_1()
                                .text_sm()
                                .bg(if ok { rgb(0x10261a) } else { rgb(0x2a1412) })
                                .border_color(if ok { rgba(0x28e07b55) } else { rgba(0xff6b5a55) })
                                .text_color(if ok { rgb(0x7ee2a8) } else { rgb(0xff6b5a) })
                                .child(text),
                        ),
                )
            })
            .into_any_element()
    }
}

trait CloneRows {
    fn clone_rows(&self) -> Vec<ProviderStatus>;
}

/// `ProviderStatus` is shared with the Tauri backend, which does not derive `Clone`.
impl CloneRows for Vec<ProviderStatus> {
    fn clone_rows(&self) -> Vec<ProviderStatus> {
        self.iter()
            .map(|s| ProviderStatus {
                provider: s.provider.clone(),
                installed: s.installed,
                cli_path: s.cli_path.clone(),
                signed_in: s.signed_in,
                signed_in_at: s.signed_in_at,
                profile_path: s.profile_path.clone(),
            })
            .collect()
    }
}
