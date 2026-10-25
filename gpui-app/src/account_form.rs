//! Adding and editing an account, and signing in through the provider's official CLI,
//! as the Tauri settings do.
use crate::input::TextInput;
use crate::model::Account;
use crate::settings::{button, card, toggle, SettingsView, DANGER, MUTED, TEXT};
use crate::store::{self, RUNTIME};
use crate::ui::{provider_icon, tr};
use crate::{codex, login};
use gpui::{
    div, prelude::*, rgb, rgba, AnyElement, ClickEvent, Context, Entity, Focusable,
    FontWeight, SharedString, Task, Window,
};
use std::{path::PathBuf, time::Duration};

const PROVIDERS: [&str; 8] = [
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

fn description(provider: &str, french: bool) -> String {
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

fn field(label: String, hint: Option<String>, input: &Entity<TextInput>, help: Option<String>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .py_2()
        .child(
            div()
                .flex()
                .gap_2()
                .text_sm()
                .child(div().font_weight(FontWeight::BOLD).child(label))
                .when_some(hint, |d, hint| d.child(div().text_color(rgb(MUTED)).child(hint))),
        )
        .child(input.clone())
        .when_some(help, |d, help| {
            d.child(div().text_xs().text_color(rgb(MUTED)).child(help))
        })
        .into_any_element()
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

    pub(crate) fn account_editor(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(e) = self.editor.as_ref() else {
            return vec![];
        };
        let provider = e.provider.clone();
        let p = provider.as_str();
        let editing = !e.id.is_empty();
        let mut rows: Vec<AnyElement> = vec![];

        if editing {
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pb_2()
                    .child(provider_icon(p, 22.))
                    .child(div().font_weight(FontWeight::BOLD).child(name(p, french)))
                    .into_any_element(),
            );
        } else {
            rows.push(
                div()
                    .pb_1()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(tr(french, "Fournisseur", "Provider"))
                    .into_any_element(),
            );
            rows.push(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .pb_2()
                    .children(PROVIDERS.into_iter().map(|choice| {
                        let active = choice == p;
                        div()
                            .id(SharedString::from(format!("provider-{choice}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .rounded_lg()
                            .border_1()
                            .cursor_pointer()
                            .text_sm()
                            .when(active, |d| d.bg(rgb(0x1d2a3d)).border_color(rgb(0x5c9bf5)))
                            .when(!active, |d| {
                                d.border_color(rgba(0xffffff1a)).hover(|d| d.bg(rgba(0xffffff0a)))
                            })
                            .child(provider_icon(choice, 16.))
                            .child(name(choice, french))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.pick_provider(choice, cx)
                            }))
                    }))
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .pb_2()
                .text_sm()
                .text_color(rgb(MUTED))
                .child(description(p, french))
                .into_any_element(),
        );
        rows.push(field(tr(french, "Nom du compte", "Account name"), None, &e.name, None));
        if has_profile(p) {
            rows.push(field(
                tr(french, "Dossier du profil", "Profile folder"),
                Some(tr(french, "facultatif", "optional")),
                &e.profile,
                Some(tr(
                    french,
                    &format!("Chemin absolu du profil déjà connecté, par exemple C:\\Users\\Kev\\.{p}-travail."),
                    &format!("Absolute path of a signed-in profile, for example C:\\Users\\Kev\\.{p}-work."),
                )),
            ));
        }
        if is_login(p) {
            rows.push(self.login_block(p, editing, french, cx));
        }
        if has_endpoint(p) {
            rows.push(field(
                tr(french, "URL du relevé JSON", "JSON reading URL"),
                None,
                &e.endpoint,
                Some(tr(
                    french,
                    "HTTPS, ou HTTP sur localhost.",
                    "HTTPS, or HTTP on localhost.",
                )),
            ));
        }
        if has_secret(p) {
            let hint = if e.has_secret {
                tr(french, "déjà enregistré", "already saved")
            } else {
                tr(french, "facultatif", "optional")
            };
            rows.push(field(tr(french, "Jeton d’accès", "Access token"), Some(hint), &e.secret, None));
            if e.has_secret {
                let clear = e.clear_secret;
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_1()
                        .text_sm()
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
                        .child(tr(french, "Effacer le secret enregistré", "Erase the saved secret"))
                        .into_any_element(),
                );
            }
            rows.push(field(
                tr(french, "Ou une variable d’environnement", "Or an environment variable"),
                Some(tr(french, "facultatif", "optional")),
                &e.secret_env,
                Some(tr(
                    french,
                    "Un secret enregistré est prioritaire. Relance l’application après avoir modifié l’environnement Windows.",
                    "A saved secret takes priority. Restart the app after changing the Windows environment.",
                )),
            ));
        }

        // Footer: remove on the left, cancel and save on the right.
        let left = if !editing {
            div().into_any_element()
        } else if e.confirm_remove {
            let id = e.id.clone();
            div()
                .flex()
                .gap_2()
                .child(button(
                    "editor-confirm-remove",
                    tr(french, "Confirmer le retrait", "Confirm removal"),
                    rgb(DANGER),
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        this.run(cx, move |s| {
                            s.remove_account(&id).map(|_| {
                                Some(tr(french, "Compte retiré de CodeNotch.", "Account removed from CodeNotch."))
                            })
                        });
                        if this.message.as_ref().is_some_and(|(ok, _)| *ok) {
                            this.close_editor(cx);
                        }
                    })),
                ))
                .child(button(
                    "editor-keep",
                    tr(french, "Garder le compte", "Keep the account"),
                    rgb(TEXT),
                    Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(e) = this.editor.as_mut() {
                            e.confirm_remove = false;
                        }
                        cx.notify();
                    })),
                ))
                .into_any_element()
        } else {
            button(
                "editor-remove",
                tr(french, "Retirer ce compte", "Remove this account"),
                rgb(DANGER),
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(e) = this.editor.as_mut() {
                        e.confirm_remove = true;
                    }
                    cx.notify();
                })),
            )
        };
        rows.push(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pt_4()
                .mt_2()
                .border_t_1()
                .border_color(rgba(0xffffff0d))
                .child(left)
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button(
                            "editor-cancel",
                            tr(french, "Annuler", "Cancel"),
                            rgb(TEXT),
                            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.close_editor(cx))),
                        ))
                        .child(
                            div()
                                .id("editor-save")
                                .px_4()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(0x355d8b))
                                .hover(|d| d.bg(rgb(0x3f6c9f)))
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child(tr(french, "Enregistrer", "Save"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_editor(cx))),
                        ),
                )
                .into_any_element(),
        );

        let title = if editing {
            tr(french, "Configurer le compte", "Configure the account")
        } else {
            tr(french, "Nouveau compte", "New account")
        };
        vec![
            div()
                .id("editor-back")
                .text_sm()
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .hover(|d| d.text_color(rgb(TEXT)))
                .child(tr(french, "← Tous les comptes", "← All accounts"))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.close_editor(cx)))
                .into_any_element(),
            card(title, rows),
        ]
    }

    fn login_block(&self, p: &str, editing: bool, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let status = login::status(p, None);
        let state = if !status.installed {
            tr(french, "CLI non installé sur ce PC", "CLI not installed on this PC")
        } else if status.signed_in {
            tr(french, "CLI installé · session trouvée", "CLI installed · session found")
        } else {
            tr(french, "CLI installé · pas encore connecté", "CLI installed · not signed in yet")
        };
        let pending = self.login.as_ref().filter(|l| l.provider == p);
        let body = match pending {
            Some(login) => div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_lg()
                .bg(rgba(0x5c9bf514))
                .border_1()
                .border_color(rgba(0x5c9bf533))
                .text_sm()
                .child(tr(
                    french,
                    "Connexion en cours… Termine-la dans le navigateur ou la console ouverte. CodeNotch ajoute le compte dès que la session est détectée.",
                    "Signing in… Finish in the browser or the console that opened. CodeNotch adds the account as soon as the session is detected.",
                ))
                .when_some(login.user_code.clone(), |d, code| {
                    d.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(tr(french, "Code :", "Code:"))
                            .child(div().font_weight(FontWeight::BOLD).child(code)),
                    )
                })
                .when_some(login.url.clone(), |d, url| {
                    d.child(button(
                        "login-open",
                        tr(french, "Rouvrir la page de connexion", "Open the sign-in page again"),
                        rgb(TEXT),
                        Box::new(move |_, _, _| {
                            let _ = login::open_url(&url);
                        }),
                    ))
                })
                .child(button(
                    "login-cancel",
                    tr(french, "Annuler la connexion", "Cancel sign-in"),
                    rgb(MUTED),
                    Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.login = None;
                        cx.notify();
                    })),
                ))
                .into_any_element(),
            None => {
                let label = if editing {
                    tr(french, "Reconnecter ce compte", "Sign in again")
                } else {
                    tr(french, &format!("Se connecter à {}", name(p, true)), &format!("Sign in to {}", name(p, false)))
                };
                div()
                    .flex()
                    .child(
                        div()
                            .id("login-start")
                            .px_4()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .border_1()
                            .border_color(rgba(0x5c9bf566))
                            .text_color(rgb(0x8dbbff))
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .hover(|d| d.bg(rgba(0x5c9bf51a)))
                            .child(label)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.start_login(cx))),
                    )
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .py_2()
            .child(body)
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{state}. {}",
                        tr(
                            french,
                            "Ouvre la connexion officielle du fournisseur. Aucun mot de passe ni jeton ne transite par CodeNotch.",
                            "Opens the provider’s official sign-in. No password or token goes through CodeNotch.",
                        )
                    )),
            )
            .into_any_element()
    }
}
