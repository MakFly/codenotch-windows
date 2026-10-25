//! Adding and editing an account, and signing in through the provider's official CLI,
//! as the Tauri settings do.
use crate::input::TextInput;
use crate::model::Account;
use crate::settings::{button, icon, message, toggle, Click, Kind, SettingsView, MUTED};
use crate::store::{self, RUNTIME};
use crate::ui::{provider_icon, t, tr};
use crate::{codex, login};
use gpui::{
    div, prelude::*, px, rgb, rgba, AnyElement, ClickEvent, Context, Entity, Focusable,
    FontWeight, Task, Window,
};
use std::{path::PathBuf, time::Duration};

pub(crate) const PROVIDERS: [&str; 8] = [
    "codex", "claude", "gemini", "grok", "opencode", "copilot", "ollama", "custom",
];

pub(crate) fn name(provider: &str, french: bool) -> String {
    match provider {
        "codex" => "Codex / ChatGPT".into(),
        "claude" => "Claude".into(),
        "gemini" => "Gemini".into(),
        "grok" => "Grok".into(),
        "opencode" => "OpenCode".into(),
        "copilot" => "GitHub Copilot".into(),
        "ollama" => "Ollama".into(),
        _ => tr(french, "Endpoint personnalisé", "Custom endpoint"),
    }
}

pub(crate) fn description(provider: &str, french: bool) -> String {
    match provider {
        "codex" => tr(french, "Quotas d’abonnement et activité en tokens via Codex CLI.", "Subscription quotas and token activity through Codex CLI."),
        "claude" => tr(french, "Quotas Claude Code via sa session OAuth Windows.", "Claude Code quotas through its Windows OAuth session."),
        "gemini" => tr(french, "Quotas Gemini CLI via sa session Google.", "Gemini CLI quotas through its Google session."),
        "grok" => tr(french, "Crédits d’abonnement Grok via la session Grok CLI.", "Grok subscription credits through the Grok CLI session."),
        "opencode" => tr(french, "Quotas de l’abonnement OpenCode Go via sa clé.", "OpenCode Go subscription quotas through its key."),
        "copilot" => tr(french, "Requêtes premium et quotas publiés par GitHub.", "Premium requests and quotas published by GitHub."),
        "ollama" => tr(french, "Relevé JSON d’un serveur Ollama, local ou distant.", "JSON reading from an Ollama server, local or remote."),
        _ => tr(french, "Un relevé JSON exposé par ton propre service.", "A JSON reading served by your own service."),
    }
}

fn is_login(provider: &str) -> bool {
    login::PROVIDERS.contains(&provider)
}
fn has_profile(provider: &str) -> bool {
    ["codex", "claude"].contains(&provider)
}
fn has_endpoint(provider: &str) -> bool {
    ["custom", "ollama"].contains(&provider)
}
fn has_secret(provider: &str) -> bool {
    !["codex", "gemini"].contains(&provider)
}

fn profile_placeholder(provider: &str, french: bool) -> String {
    let dir = login::default_profile(provider);
    tr(
        french,
        &format!("Par défaut : {}", dir.display()),
        &format!("Default: {}", dir.display()),
    )
}

pub(crate) struct Editor {
    id: String,
    provider: String,
    enabled: bool,
    has_secret: bool,
    clear_secret: bool,
    confirm_remove: bool,
    name: Entity<TextInput>,
    profile: Entity<TextInput>,
    endpoint: Entity<TextInput>,
    secret: Entity<TextInput>,
    secret_env: Entity<TextInput>,
}

pub(crate) struct Login {
    pub(crate) provider: String,
    url: Option<String>,
    user_code: Option<String>,
    /// Keeps Codex's in-app sign-in (and its callback server) alive.
    task: Option<tokio::task::JoinHandle<()>>,
    _watch: Task<()>,
}

impl Drop for Login {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

type Started = (Option<String>, Option<String>, Option<codex::LoginOutcome>);

/// Codex signs in inside the app through its App Server; the other CLIs open their own
/// sign-in in a console window. Either way the CLI writes the credentials.
async fn launch(provider: String, profile: String) -> Result<Started, String> {
    let path = (!profile.is_empty()).then(|| PathBuf::from(&profile));
    if path.as_ref().is_some_and(|p| !p.is_absolute()) {
        return Err("Le dossier du profil doit être un chemin absolu.".into());
    }
    if provider == "codex" && login::executable("codex").is_some() {
        if let Some((start, outcome)) = codex::login(&profile).await? {
            let _ = login::open_url(&start.url);
            return Ok((Some(start.url), start.user_code, Some(outcome)));
        }
    }
    login::start(&provider, path.as_deref())?;
    Ok((None, None, None))
}


impl SettingsView {
    pub(crate) fn open_editor(
        &mut self,
        account: Option<Account>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let french = self.store.read(cx).french();
        let a = account.unwrap_or(Account {
            id: String::new(),
            name: String::new(),
            provider: "codex".into(),
            enabled: true,
            profile_path: String::new(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
        });
        let mut input = |placeholder: String, text: &str, masked: bool| {
            let text = text.to_owned();
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                if masked {
                    input = input.masked();
                }
                input.set_text(text, cx);
                input
            })
        };
        let name = input(tr(french, "Ex. Personnel, travail, projet…", "E.g. Personal, work, project…"), &a.name, false);
        let profile = input(profile_placeholder(&a.provider, french), &a.profile_path, false);
        let endpoint = input("https://…/usage.json".into(), &a.endpoint, false);
        let secret = input(
            if a.has_secret {
                tr(french, "Laisser vide pour conserver le secret", "Leave empty to keep the secret")
            } else {
                tr(french, "Bearer token / jeton OAuth", "Bearer token / OAuth token")
            },
            "",
            true,
        );
        let secret_env = input(
            if a.provider == "copilot" { "GH_TOKEN".into() } else { tr(french, "MON_TOKEN_IA", "MY_AI_TOKEN") },
            &a.secret_env,
            false,
        );
        window.focus(&name.focus_handle(cx));
        self.login = None;
        self.message = None;
        self.editor = Some(Editor {
            id: a.id,
            provider: a.provider,
            enabled: a.enabled,
            has_secret: a.has_secret,
            clear_secret: false,
            confirm_remove: false,
            name,
            profile,
            endpoint,
            secret,
            secret_env,
        });
        cx.notify();
    }

    fn close_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.login = None;
        cx.notify();
    }

    fn pick_provider(&mut self, provider: &'static str, cx: &mut Context<Self>) {
        let french = self.store.read(cx).french();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.provider = provider.into();
        let (profile, env) = (editor.profile.clone(), editor.secret_env.clone());
        profile.update(cx, |i, cx| i.set_placeholder(profile_placeholder(provider, french), cx));
        env.update(cx, |i, cx| {
            i.set_placeholder(
                if provider == "copilot" { "GH_TOKEN".into() } else { tr(french, "MON_TOKEN_IA", "MY_AI_TOKEN") },
                cx,
            )
        });
        self.login = None;
        cx.notify();
    }

    /// Reads the providers again right away, so a saved account shows up in the notch.
    pub(crate) fn refresh_now(&self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |_, cx| store::poll(&store, true, cx).await)
            .detach();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.editor.as_ref() else {
            return;
        };
        let french = self.store.read(cx).french();
        let read = |i: &Entity<TextInput>| i.read(cx).text().trim().to_owned();
        let p = e.provider.as_str();
        let account = Account {
            id: e.id.clone(),
            name: read(&e.name),
            provider: e.provider.clone(),
            enabled: e.enabled,
            profile_path: if has_profile(p) { read(&e.profile) } else { String::new() },
            endpoint: if has_endpoint(p) { read(&e.endpoint) } else { String::new() },
            secret_env: if has_secret(p) { read(&e.secret_env) } else { String::new() },
            has_secret: e.has_secret,
        };
        let secret = has_secret(p).then(|| read(&e.secret));
        let clear = e.clear_secret;
        self.run(cx, move |s| {
            s.save_account(account, secret, clear).map(|_| {
                Some(tr(
                    french,
                    "Compte enregistré. Le suivi démarrera automatiquement.",
                    "Account saved. Tracking starts automatically.",
                ))
            })
        });
        if self.message.as_ref().is_some_and(|(ok, _)| *ok) {
            self.close_editor(cx);
            self.refresh_now(cx);
        }
    }

    fn start_login(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.editor.as_ref() else {
            return;
        };
        let provider = e.provider.clone();
        let profile = if has_profile(&provider) {
            e.profile.read(cx).text().trim().to_owned()
        } else {
            String::new()
        };
        self.sign_in(provider, profile, cx);
    }

    /// Starts the provider's sign-in and adds the profile once its session appears.
    pub(crate) fn sign_in(&mut self, provider: String, profile: String, cx: &mut Context<Self>) {
        let french = self.store.read(cx).french();
        self.login = None;
        self.message = None;
        let started_ms = chrono::Utc::now().timestamp_millis();
        let runtime = RUNTIME.get().expect("tokio runtime").clone();
        let launched = runtime.spawn(launch(provider.clone(), profile.clone()));
        let watched = provider.clone();
        let watch = cx.spawn(async move |this, cx| {
            let fail = |error: String, this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let _ = this.update(cx, |this, cx| {
                    this.login = None;
                    this.say(false, error, cx);
                });
            };
            let (url, user_code, outcome) = match launched.await {
                Ok(Ok(started)) => started,
                Ok(Err(error)) => return fail(error, &this, cx),
                Err(_) => return fail("Connexion interrompue.".into(), &this, cx),
            };
            let task = outcome.map(|outcome| {
                runtime.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(900), outcome).await;
                })
            });
            if this
                .update(cx, |this, cx| {
                    if let Some(login) = this.login.as_mut() {
                        login.url = url;
                        login.user_code = user_code;
                        login.task = task;
                    }
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
            // The CLI writes its credential file once the sign-in succeeds.
            let path = (!profile.is_empty()).then(|| PathBuf::from(&profile));
            for _ in 0..450 {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let status = login::status(&watched, path.as_deref());
                if status.signed_in_at.is_some_and(|at| at >= started_ms) {
                    let provider = watched.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.run(cx, move |s| {
                            s.finish_login(&provider).map(|_| {
                                Some(tr(
                                    french,
                                    &format!("Connexion {} réussie. Les quotas apparaîtront dans l’encoche.", name(&provider, true)),
                                    &format!("{} sign-in done. Quotas will show in the notch.", name(&provider, false)),
                                ))
                            })
                        });
                        this.editor = None;
                        this.load_statuses();
                        this.refresh_now(cx);
                        this.login = None;
                    });
                    return;
                }
            }
            fail(tr(french, "Connexion expirée. Réessaie.", "Sign-in timed out. Try again."), &this, cx);
        });
        self.login = Some(Login {
            provider,
            url: None,
            user_code: None,
            task: None,
            _watch: watch,
        });
        cx.notify();
    }

}

/// `.account-fields > label` and its input.
fn field(label: String, hint: Option<String>, input: &Entity<TextInput>, help: Option<String>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .mt(px(20.))
                .mb(px(8.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .text_size(px(13.))
                .child(label)
                .when_some(hint, |d, hint| d.child(div().text_size(px(11.)).text_color(rgb(0xa8a8b0)).child(hint))),
        )
        .child(input.clone())
        .when_some(help, |d, help| {
            d.child(div().mt(px(7.)).text_size(px(12.)).line_height(px(18.)).text_color(rgb(MUTED)).child(help))
        })
        .into_any_element()
}

impl SettingsView {
    /// The account form, as the Tauri `.dialog-content` over a dimmed window.
    pub(crate) fn account_dialog(&mut self, french: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let e = self.editor.as_ref()?;
        let provider = e.provider.clone();
        let p = provider.as_str();
        let editing = !e.id.is_empty();
        let mut fields: Vec<AnyElement> = vec![];

        if !editing {
            let options = PROVIDERS
                .into_iter()
                .map(|choice| {
                    let click: Click = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| this.pick_provider(choice, cx)));
                    (t(french, &name(choice, true)), choice == p, click)
                })
                .collect();
            fields.push(
                div()
                    .mt(px(20.))
                    .mb(px(8.))
                    .text_size(px(13.))
                    .child(t(french, "Fournisseur"))
                    .into_any_element(),
            );
            fields.push(self.select("provider", t(french, &name(p, true)), options, 472., cx));
            fields.push(
                div()
                    .mt(px(7.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED))
                    .child(description(p, french))
                    .into_any_element(),
            );
        }
        let e = self.editor.as_ref()?;
        fields.push(field(t(french, "Nom du compte"), None, &e.name, None));
        if has_profile(p) {
            fields.push(field(
                t(french, "Dossier du profil"),
                Some(t(french, "facultatif")),
                &e.profile,
                Some(format!(
                    "{}{p}{}",
                    t(french, "Chemin absolu du profil déjà connecté, par exemple C:\\Users\\Kev\\."),
                    t(french, "-travail.")
                )),
            ));
        }
        if is_login(p) {
            fields.push(self.login_block(p, editing, french, cx));
        }
        let e = self.editor.as_ref()?;
        if has_endpoint(p) {
            fields.push(field(
                t(french, "URL du relevé JSON"),
                None,
                &e.endpoint,
                Some(t(french, "HTTPS, ou HTTP sur localhost. Le format attendu est détaillé dans l’aide.")),
            ));
        }
        if has_secret(p) {
            let mut advanced: Vec<AnyElement> = vec![
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xc5c5cd))
                    .child(t(french, "Jeton et options avancées"))
                    .into_any_element(),
                field(
                    t(french, "Jeton d’accès"),
                    Some(if e.has_secret { t(french, "déjà enregistré") } else { t(french, "facultatif") }),
                    &e.secret,
                    None,
                ),
            ];
            if e.has_secret {
                let clear = e.clear_secret;
                advanced.push(
                    div()
                        .mt(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(13.))
                        .child(toggle(
                            "clear-secret",
                            clear,
                            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if let Some(e) = this.editor.as_mut() {
                                    e.clear_secret = !clear;
                                }
                                cx.notify();
                            })),
                        ))
                        .child(t(french, "Effacer le secret enregistré"))
                        .into_any_element(),
                );
            }
            advanced.push(field(
                t(french, "Ou une variable d’environnement"),
                Some(t(french, "facultatif")),
                &e.secret_env,
                Some(t(french, "Un secret enregistré est prioritaire. Relance l’application après avoir modifié l’environnement Windows.")),
            ));
            fields.push(
                div()
                    .mt(px(22.))
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(rgba(0xffffff12))
                    .flex()
                    .flex_col()
                    .children(advanced)
                    .into_any_element(),
            );
        }
        let e = self.editor.as_ref()?;
        if editing {
            let remove = if e.confirm_remove {
                let id = e.id.clone();
                vec![
                    div()
                        .mb(px(10.))
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(t(french, "Retirer ce compte de CodeNotch ? Sa connexion dans le fournisseur sera conservée."))
                        .into_any_element(),
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(button(
                            "editor-confirm-remove",
                            Kind::Danger,
                            None,
                            t(french, "Confirmer le retrait"),
                            Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let id = id.clone();
                                this.run(cx, move |s| {
                                    s.remove_account(&id)
                                        .map(|_| Some(t(french, "Compte retiré de CodeNotch.")))
                                });
                                if this.message.as_ref().is_some_and(|(ok, _)| *ok) {
                                    this.close_editor(cx);
                                }
                            }))),
                        ))
                        .child(button(
                            "editor-keep",
                            Kind::Secondary,
                            None,
                            t(french, "Garder le compte"),
                            Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if let Some(e) = this.editor.as_mut() {
                                    e.confirm_remove = false;
                                }
                                cx.notify();
                            }))),
                        ))
                        .into_any_element(),
                ]
            } else {
                vec![div()
                    .flex()
                    .child(button(
                        "editor-remove",
                        Kind::Ghost,
                        Some("trash-2"),
                        t(french, "Retirer ce compte"),
                        Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                            if let Some(e) = this.editor.as_mut() {
                                e.confirm_remove = true;
                            }
                            cx.notify();
                        }))),
                    ))
                    .into_any_element()]
            };
            fields.push(
                div()
                    .mt(px(22.))
                    .pt(px(13.))
                    .border_t_1()
                    .border_color(rgba(0xffffff12))
                    .children(remove)
                    .into_any_element(),
            );
        }

        let subtitle = if editing {
            self.editor.as_ref().map(|e| e.name.read(cx).text()).unwrap_or_default()
        } else {
            t(french, "Ajoutez un compte à votre encoche.")
        };
        let heading = div()
            .flex()
            .items_start()
            .gap(px(12.))
            .pt(px(24.))
            .px(px(24.))
            .pb(px(20.))
            .border_b_1()
            .border_color(rgba(0xffffff12))
            .child(div().mt(px(2.)).child(provider_icon(p, 28.)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(if editing { t(french, "Configurer le compte") } else { t(french, "Nouveau compte") }),
                    )
                    .child(div().mt(px(5.)).text_size(px(12.)).text_color(rgb(MUTED)).child(subtitle)),
            )
            .child(
                div()
                    .id("editor-close")
                    .size(px(34.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff0e)))
                    .child(icon("x", 16., 0xc3c3c8))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_editor(cx))),
            );
        let footer = div()
            .flex()
            .flex_none()
            .justify_end()
            .gap(px(8.))
            .py(px(16.))
            .px(px(24.))
            .border_t_1()
            .border_color(rgba(0xffffff12))
            .bg(rgb(0x202023))
            .child(button(
                "editor-cancel",
                Kind::Secondary,
                None,
                t(french, "Annuler"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.close_editor(cx)))),
            ))
            .child(button(
                "editor-save",
                Kind::Primary,
                Some("check"),
                t(french, "Enregistrer"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.save_editor(cx)))),
            ));
        let error = self
            .message
            .clone()
            .filter(|(ok, _)| !ok)
            .map(|(ok, text)| div().px(px(24.)).pt(px(14.)).child(message(ok, text)));

        Some(
            div()
                .id("dialog-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .bg(rgba(0x00000099))
                .occlude()
                .child(
                    div()
                        .id("dialog")
                        .w(px(520.))
                        .max_h(px(640.))
                        .h_full()
                        .flex()
                        .flex_col()
                        .rounded(px(14.))
                        .border_1()
                        .border_color(rgba(0xffffff26))
                        .bg(rgb(0x1c1c1f))
                        .shadow_2xl()
                        .overflow_hidden()
                        .child(heading)
                        .children(error)
                        .child(
                            div()
                                .id("dialog-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(div().pt(px(4.)).px(px(24.)).pb(px(24.)).flex().flex_col().children(fields)),
                        )
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    fn login_block(&self, p: &str, editing: bool, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let pending = self.login.as_ref().filter(|l| l.provider == p);
        let body: AnyElement = match pending {
            Some(login) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(12.))
                .rounded(px(6.))
                .bg(rgb(0x24303f))
                .text_color(rgb(0xc4d5ee))
                .text_size(px(12.))
                .child(tr(
                    french,
                    "Connexion en cours… Termine-la dans le navigateur ou la console ouverte. CodeNotch ajoute le compte dès que la session est détectée.",
                    "Signing in… Finish in the browser or the console that opened. CodeNotch adds the account as soon as the session is detected.",
                ))
                .when_some(login.user_code.clone(), |d, code| {
                    d.child(div().font_weight(FontWeight::SEMIBOLD).child(code))
                })
                .when_some(login.url.clone(), |d, url| d.child(div().text_size(px(11.)).opacity(0.9).child(url)))
                .into_any_element(),
            None => button(
                "login-start",
                Kind::Secondary,
                Some("log-in"),
                if editing {
                    t(french, "Reconnecter ce compte")
                } else {
                    t(french, "Se connecter à {name}").replace("{name}", &t(french, &name(p, true)))
                },
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.start_login(cx)))),
            ),
        };
        let cancel = pending.map(|_| {
            button(
                "login-cancel",
                Kind::Secondary,
                Some("x"),
                t(french, "Annuler la connexion"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.login = None;
                    cx.notify();
                }))),
            )
        });
        div()
            .mt(px(16.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .child(body)
            .children(cancel)
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED))
                    .child(t(french, "Ouvre la connexion officielle du fournisseur. Aucun mot de passe ni jeton ne transite par CodeNotch.")),
            )
            .into_any_element()
    }
}
