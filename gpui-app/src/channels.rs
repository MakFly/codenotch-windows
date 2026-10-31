//! Telegram and Discord alert channels, as the accordion bodies of the Tauri Alerts page:
//! secrets go to Windows Credential Manager, only the chat id and the switches are saved in
//! the configuration.
use crate::input::TextInput;
use crate::settings::{button, icon, setting_row, toggle, Kind, SettingsView, MUTED};
use crate::store::{Store, RUNTIME};
use crate::ui::t;
use crate::{discord, telegram};
use gpui::{
    div, prelude::*, px, rgb, rgba, AnyElement, ClickEvent, Context, Entity, FontWeight,
    SharedString,
};
use std::future::Future;

pub(crate) struct Channels {
    telegram_token: Entity<TextInput>,
    telegram_chat: Entity<TextInput>,
    discord_webhook: Entity<TextInput>,
    chats: Vec<telegram::Chat>,
    busy: bool,
}

impl Channels {
    pub(crate) fn new(french: bool, chat: String, cx: &mut Context<SettingsView>) -> Self {
        Self {
            telegram_token: cx.new(|cx| TextInput::new("123456789:AA…", cx).masked()),
            telegram_chat: cx.new(|cx| {
                let mut input = TextInput::new(t(french, "123456789 ou @mon_canal"), cx);
                input.set_text(chat, cx);
                input
            }),
            discord_webhook: cx.new(|cx| {
                TextInput::new("https://discord.com/api/webhooks/…", cx).masked()
            }),
            chats: vec![],
            busy: false,
        }
    }
}

/// `.channel-steps`: the numbered setup steps.
fn steps(items: [String; 3]) -> AnyElement {
    div()
        .mb(px(4.))
        .pl(px(6.))
        .flex()
        .flex_col()
        .text_size(px(11.5))
        .line_height(px(19.5))
        .text_color(rgb(0xa8a8b0))
        .children(items.into_iter().enumerate().map(|(i, text)| format!("{}. {text}", i + 1)))
        .into_any_element()
}

/// `.field > label` with its muted hint.
fn label(text: String, hint: String) -> AnyElement {
    div()
        .flex()
        .gap_1()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(text)
        .child(div().font_weight(FontWeight::NORMAL).text_color(rgb(0x8f8f96)).child(hint))
        .into_any_element()
}

fn help(text: String) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(MUTED))
        .child(text)
        .into_any_element()
}

/// Why the last alert sent to this channel did not arrive, until one does.
fn last_failure(store: &Store, channel: &str, french: bool) -> Option<AnyElement> {
    let error = store.channel_errors.lock().ok()?.get(channel).cloned()?;
    Some(
        div()
            .flex()
            .gap(px(7.))
            .text_size(px(11.5))
            .text_color(rgb(0xf1a1a1))
            .child(icon("circle-alert", 14., 0xf1a1a1))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t(french, "Dernière alerte non envoyée : {error}").replace("{error}", &t(french, &error))),
            )
            .into_any_element(),
    )
}

/// The trash `.button.ghost.danger-text` next to a saved secret.
fn erase(id: &'static str, on_click: crate::settings::Click) -> AnyElement {
    div()
        .id(id)
        .size(px(36.))
        .flex_none()
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(rgba(0x00000000))
        .hover(|d| d.bg(rgba(0xffffff0d)).border_color(rgb(0x3b3b3f)))
        .child(icon("trash-2", 16., 0xf1a1a1))
        .on_click(on_click)
        .into_any_element()
}

impl SettingsView {
    /// Runs network work on the tokio runtime and reports the outcome in the banner.
    fn run_remote(
        &mut self,
        cx: &mut Context<Self>,
        work: impl Future<Output = Result<(), String>> + Send + 'static,
        success: String,
    ) {
        let task = RUNTIME.get().expect("tokio runtime").spawn(work);
        self.channels.busy = true;
        self.message = None;
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err("Interrompu.".into()));
            let _ = this.update(cx, |this, cx| {
                this.channels.busy = false;
                match result {
                    Ok(()) => this.say(true, success, cx),
                    Err(error) => this.say(false, error, cx),
                }
            });
        })
        .detach();
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn save_secret(
        &mut self,
        cx: &mut Context<Self>,
        input: Entity<TextInput>,
        save: fn(&str) -> Result<(), String>,
        clear: fn() -> Result<(), String>,
        saved: fn(&mut Store, bool),
        is_saved: fn() -> bool,
        message: String,
    ) {
        let value = input.read(cx).text().trim().to_owned();
        let result = if value.is_empty() { clear() } else { save(&value) };
        let now = is_saved();
        self.store.update(cx, |s, cx| {
            saved(s, now);
            cx.notify();
        });
        match result {
            Ok(()) => {
                input.update(cx, |i, cx| i.set_text("", cx));
                self.say(true, message, cx);
            }
            Err(error) => self.say(false, error, cx),
        }
    }

    pub(crate) fn discord_body(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx);
        let saved = s.discord_saved;
        let enabled = s.config.settings.discord_enabled;
        let failure = last_failure(s, "discord", french);
        let busy = self.channels.busy;
        let webhook = self.channels.discord_webhook.clone();
        webhook.update(cx, |i, cx| {
            i.set_placeholder(
                if saved {
                    t(french, "Laisser vide pour conserver le webhook")
                } else {
                    "https://discord.com/api/webhooks/…".into()
                },
                cx,
            )
        });
        let save_input = webhook.clone();
        let save = button(
            "discord-save",
            Kind::Secondary,
            None,
            t(french, "Enregistrer"),
            Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.save_secret(
                    cx,
                    save_input.clone(),
                    discord::save_webhook,
                    discord::clear_webhook,
                    |s, v| s.discord_saved = v,
                    || discord::saved_webhook().is_some(),
                    t(french, "Webhook Discord enregistré dans le Gestionnaire d’identification Windows."),
                )
            }))),
        );
        let clear = saved.then(|| {
            erase(
                "discord-clear",
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let result = discord::clear_webhook();
                    this.store.update(cx, |s, cx| {
                        s.discord_saved = discord::saved_webhook().is_some();
                        cx.notify();
                    });
                    match result {
                        Ok(()) => this.say(true, t(french, "Webhook Discord effacé."), cx),
                        Err(e) => this.say(false, e, cx),
                    }
                })),
            )
        });
        let test = button(
            "discord-test",
            Kind::Primary,
            Some("send"),
            t(french, "Envoyer un test"),
            (saved && !busy).then(|| -> crate::settings::Click {
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let Some(webhook) = discord::saved_webhook() else {
                        return this.say(false, t(french, "Enregistre d’abord le webhook Discord."), cx);
                    };
                    let body = discord::test_message(french, chrono::Utc::now().timestamp());
                    this.run_remote(
                        cx,
                        async move { discord::send(&webhook, body).await },
                        t(french, "Message de test envoyé. Regarde Discord."),
                    )
                }))
            }),
        );
        vec![
            setting_row(
                t(french, "Alertes Discord"),
                Some(t(french, "Publie les alertes dans un salon Discord sous forme de carte, pour toi ou ton équipe.")),
                toggle("discord-enabled", enabled, self.setting(cx, |s| s.discord_enabled = !s.discord_enabled)),
            ),
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .pt(px(14.))
                .pb(px(6.))
                .children(failure)
                .child(steps([
                    t(french, "Dans Discord, ouvre les paramètres du salon → Intégrations → Webhooks."),
                    t(french, "Crée un webhook, puis copie son URL."),
                    t(french, "Colle l’URL ici, enregistre, puis envoie un test."),
                ]))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(label(
                            t(french, "URL du webhook"),
                            if saved { t(french, "déjà enregistré") } else { t(french, "requis") },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().min_w_0().child(webhook))
                                .child(save)
                                .children(clear),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .mt(px(6.))
                        .child(test)
                        .child(help(t(french, "L’URL reste dans le Gestionnaire d’identification Windows ; seul le message d’alerte est envoyé à Discord."))),
                )
                .into_any_element(),
        ]
    }

    pub(crate) fn telegram_body(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx);
        let saved = s.telegram_saved;
        let enabled = s.config.settings.telegram_enabled;
        let failure = last_failure(s, "telegram", french);
        let busy = self.channels.busy;
        let token = self.channels.telegram_token.clone();
        let chat = self.channels.telegram_chat.clone();
        token.update(cx, |i, cx| {
            i.set_placeholder(
                if saved { t(french, "Laisser vide pour conserver le jeton") } else { "123456789:AA…".into() },
                cx,
            )
        });

        let save_input = token.clone();
        let save = button(
            "telegram-save",
            Kind::Secondary,
            None,
            t(french, "Enregistrer"),
            Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.save_secret(
                    cx,
                    save_input.clone(),
                    telegram::save_token,
                    telegram::clear_token,
                    |s, v| s.telegram_saved = v,
                    || telegram::saved_token().is_some(),
                    t(french, "Jeton Telegram enregistré dans le Gestionnaire d’identification Windows."),
                )
            }))),
        );
        let clear = saved.then(|| {
            erase(
                "telegram-clear",
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let result = telegram::clear_token();
                    this.store.update(cx, |s, cx| {
                        s.telegram_saved = telegram::saved_token().is_some();
                        cx.notify();
                    });
                    match result {
                        Ok(()) => this.say(true, t(french, "Jeton Telegram effacé."), cx),
                        Err(e) => this.say(false, e, cx),
                    }
                })),
            )
        });

        let detect_token = token.clone();
        let detect = button(
            "telegram-detect",
            Kind::Secondary,
            Some("search"),
            t(french, "Détecter"),
            (!busy).then(|| -> crate::settings::Click {
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let typed = detect_token.read(cx).text().trim().to_owned();
                    let Some(token) = Some(typed).filter(|t| !t.is_empty()).or_else(telegram::saved_token) else {
                        return this.say(false, t(french, "Enregistre d’abord le jeton du bot."), cx);
                    };
                    let task = RUNTIME
                        .get()
                        .expect("tokio runtime")
                        .spawn(async move { telegram::detect_chats(&token).await });
                    this.channels.busy = true;
                    cx.spawn(async move |this, cx| {
                        let result = task.await.unwrap_or_else(|_| Err("Interrompu.".into()));
                        let _ = this.update(cx, |this, cx| {
                            this.channels.busy = false;
                            match result {
                                Ok(chats) if chats.is_empty() => this.say(
                                    false,
                                    t(french, "Aucun chat trouvé. Envoie /start à ton bot, puis réessaie."),
                                    cx,
                                ),
                                Ok(chats) => {
                                    this.channels.chats = chats;
                                    this.message = None;
                                    cx.notify();
                                }
                                Err(e) => this.say(false, e, cx),
                            }
                        });
                    })
                    .detach();
                    cx.notify();
                }))
            }),
        );
        let found: Vec<(String, String)> = self
            .channels
            .chats
            .iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect();
        let chats: Vec<AnyElement> = found
            .into_iter()
            .map(|(id, name)| {
                let pick = chat.clone();
                let value = id.clone();
                button(
                    SharedString::from(format!("chat-{id}")),
                    Kind::Secondary,
                    None,
                    format!("{name} · {id}"),
                    Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = value.clone();
                        pick.update(cx, |i, cx| i.set_text(id.clone(), cx));
                        this.update_settings(cx, move |s| s.telegram_chat_id = id);
                        this.channels.chats.clear();
                        cx.notify();
                    }))),
                )
            })
            .collect();

        let test_chat = chat.clone();
        let test = button(
            "telegram-test",
            Kind::Primary,
            Some("send"),
            t(french, "Envoyer un test"),
            (saved && !busy).then(|| -> crate::settings::Click {
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let chat = test_chat.read(cx).text().trim().to_owned();
                    if !telegram::valid_chat_id(&chat) {
                        return this.say(false, "Identifiant de chat Telegram invalide : un nombre (ex. 123456789 ou -100…) ou @nom_du_canal.".into(), cx);
                    }
                    let Some(token) = telegram::saved_token() else {
                        return this.say(false, t(french, "Enregistre d’abord le jeton du bot."), cx);
                    };
                    // A typed chat is saved too, so a successful test means alerts reach it.
                    if this.store.read(cx).config.settings.telegram_chat_id != chat {
                        let saved = chat.clone();
                        this.update_settings(cx, move |s| s.telegram_chat_id = saved);
                    }
                    let body = telegram::test_message(french, chrono::Utc::now().timestamp());
                    this.run_remote(
                        cx,
                        async move { telegram::send(&token, &chat, body).await },
                        t(french, "Message de test envoyé. Regarde Telegram."),
                    )
                }))
            }),
        );

        vec![
            setting_row(
                t(french, "Alertes Telegram"),
                Some(t(french, "Reçois les alertes sur ton téléphone via ton propre bot, même loin du PC. Les messages partent directement de ce PC vers Telegram.")),
                toggle(
                    "telegram-enabled",
                    enabled,
                    {
                        // Turning alerts on also keeps a chat that was typed but not saved.
                        let typed = chat.clone();
                        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let typed = typed.read(cx).text().trim().to_owned();
                            this.update_settings(cx, move |s| {
                                s.telegram_enabled = !s.telegram_enabled;
                                if s.telegram_enabled && telegram::valid_chat_id(&typed) {
                                    s.telegram_chat_id = typed;
                                }
                            })
                        }))
                    },
                ),
            ),
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .pt(px(14.))
                .pb(px(6.))
                .children(failure)
                .child(steps([
                    t(french, "Dans Telegram, ouvre @BotFather, envoie /newbot et copie le jeton du bot."),
                    t(french, "Ouvre ton bot et envoie-lui /start (ou ajoute-le à un groupe)."),
                    t(french, "Détecte le chat, puis envoie un message de test."),
                ]))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(label(
                            t(french, "Jeton du bot"),
                            if saved { t(french, "déjà enregistré") } else { t(french, "requis") },
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().min_w_0().child(token))
                                .child(save)
                                .children(clear),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(label(t(french, "Chat qui reçoit les alertes"), String::new()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().min_w_0().child(chat))
                                .child(detect),
                        ),
                )
                .when(!chats.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(8.)).children(chats)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .mt(px(6.))
                        .child(test)
                        .child(help(t(french, "Le jeton reste dans le Gestionnaire d’identification Windows ; seul le message d’alerte est envoyé à Telegram."))),
                )
                .into_any_element(),
        ]
    }
}
