//! Telegram and Discord alert channels: secrets go to Windows Credential Manager, only the
//! chat id and the on/off switches are saved in the configuration.
use crate::input::TextInput;
use crate::settings::{button, toggle, SettingsView, DANGER, MUTED, TEXT};
use crate::store::{Store, RUNTIME};
use crate::ui::tr;
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
                let mut input =
                    TextInput::new(tr(french, "123456789 ou @mon_canal", "123456789 or @my_channel"), cx);
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

fn steps(items: [String; 3]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .py_2()
        .text_sm()
        .text_color(rgb(0xc0c6cf))
        .children(items.into_iter().enumerate().map(|(i, text)| {
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .size(px(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(rgb(0x1f1f22))
                        .text_xs()
                        .child(format!("{}", i + 1)),
                )
                .child(div().flex_1().min_w_0().child(text))
        }))
        .into_any_element()
}

fn label(text: String, hint: String) -> AnyElement {
    div()
        .flex()
        .gap_2()
        .pt_2()
        .text_sm()
        .child(div().font_weight(FontWeight::BOLD).child(text))
        .child(div().text_color(rgb(MUTED)).child(hint))
        .into_any_element()
}

fn help(text: String) -> AnyElement {
    div()
        .pt_1()
        .text_xs()
        .text_color(rgb(MUTED))
        .child(text)
        .into_any_element()
}

/// A channel card: title, description and switch on top, then the setup.
fn channel_card(title: String, description: String, control: AnyElement, body: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .px_5()
        .py_4()
        .rounded_xl()
        .bg(rgb(0x151517))
        .border_1()
        .border_color(rgba(0xffffff12))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::BOLD).child(title))
                        .child(div().text_sm().text_color(rgb(MUTED)).child(description)),
                )
                .child(control),
        )
        .children(body)
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

    pub(crate) fn discord_card(&mut self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let saved = s.discord_saved;
        let enabled = s.config.settings.discord_enabled;
        let busy = self.channels.busy;
        let webhook = self.channels.discord_webhook.clone();
        let save_input = webhook.clone();
        let mut actions = div().flex().gap_2().child(button(
            "discord-save",
            tr(french, "Enregistrer", "Save"),
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.save_secret(
                    cx,
                    save_input.clone(),
                    discord::save_webhook,
                    discord::clear_webhook,
                    |s, v| s.discord_saved = v,
                    || discord::saved_webhook().is_some(),
                    tr(
                        french,
                        "Webhook Discord enregistré dans le Gestionnaire d’identification Windows.",
                        "Discord webhook saved in Windows Credential Manager.",
                    ),
                )
            })),
        ));
        if saved {
            actions = actions.child(button(
                "discord-clear",
                tr(french, "Effacer", "Erase"),
                rgb(DANGER),
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let result = discord::clear_webhook();
                    this.store.update(cx, |s, cx| {
                        s.discord_saved = discord::saved_webhook().is_some();
                        cx.notify();
                    });
                    match result {
                        Ok(()) => this.say(true, tr(french, "Webhook Discord effacé.", "Discord webhook erased."), cx),
                        Err(e) => this.say(false, e, cx),
                    }
                })),
            ));
        }
        let test = button(
            "discord-test",
            if busy { tr(french, "Envoi…", "Sending…") } else { tr(french, "Envoyer un test", "Send a test") },
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.channels.busy {
                    return;
                }
                let Some(webhook) = discord::saved_webhook() else {
                    return this.say(false, tr(french, "Enregistre d’abord le webhook Discord.", "Save the Discord webhook first."), cx);
                };
                let body = discord::test_message(french, chrono::Utc::now().timestamp());
                this.run_remote(
                    cx,
                    async move { discord::send(&webhook, body).await },
                    tr(french, "Message de test envoyé. Regarde Discord.", "Test message sent. Check Discord."),
                )
            })),
        );
        channel_card(
            tr(french, "Alertes Discord", "Discord alerts"),
            tr(
                french,
                "Publie les alertes dans un salon Discord sous forme de carte, pour toi ou ton équipe.",
                "Posts alerts to a Discord channel as a card, for you or your team.",
            ),
            toggle("discord-enabled", enabled, self.setting(cx, |s| s.discord_enabled = !s.discord_enabled)),
            vec![
                steps([
                    tr(french, "Dans Discord, ouvre les paramètres du salon → Intégrations → Webhooks.", "In Discord, open the channel settings → Integrations → Webhooks."),
                    tr(french, "Crée un webhook, puis copie son URL.", "Create a webhook, then copy its URL."),
                    tr(french, "Colle l’URL ici, enregistre, puis envoie un test.", "Paste the URL here, save, then send a test."),
                ]),
                label(
                    tr(french, "URL du webhook", "Webhook URL"),
                    if saved { tr(french, "déjà enregistré", "already saved") } else { tr(french, "requis", "required") },
                ),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .child(div().flex_1().min_w_0().child(webhook))
                    .child(actions)
                    .into_any_element(),
                div().flex().pt_3().child(test).into_any_element(),
                help(tr(
                    french,
                    "L’URL reste dans le Gestionnaire d’identification Windows ; seul le message d’alerte est envoyé à Discord.",
                    "The URL stays in Windows Credential Manager; only the alert message is sent to Discord.",
                )),
            ],
        )
    }

    pub(crate) fn telegram_card(&mut self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = self.store.read(cx);
        let saved = s.telegram_saved;
        let enabled = s.config.settings.telegram_enabled;
        let busy = self.channels.busy;
        let token = self.channels.telegram_token.clone();
        let chat = self.channels.telegram_chat.clone();

        let save_input = token.clone();
        let mut token_actions = div().flex().gap_2().child(button(
            "telegram-save",
            tr(french, "Enregistrer", "Save"),
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.save_secret(
                    cx,
                    save_input.clone(),
                    telegram::save_token,
                    telegram::clear_token,
                    |s, v| s.telegram_saved = v,
                    || telegram::saved_token().is_some(),
                    tr(
                        french,
                        "Jeton Telegram enregistré dans le Gestionnaire d’identification Windows.",
                        "Telegram token saved in Windows Credential Manager.",
                    ),
                )
            })),
        ));
        if saved {
            token_actions = token_actions.child(button(
                "telegram-clear",
                tr(french, "Effacer", "Erase"),
                rgb(DANGER),
                Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let result = telegram::clear_token();
                    this.store.update(cx, |s, cx| {
                        s.telegram_saved = telegram::saved_token().is_some();
                        cx.notify();
                    });
                    match result {
                        Ok(()) => this.say(true, tr(french, "Jeton Telegram effacé.", "Telegram token erased."), cx),
                        Err(e) => this.say(false, e, cx),
                    }
                })),
            ));
        }

        let detect_token = token.clone();
        let detect = button(
            "telegram-detect",
            tr(french, "Détecter", "Detect"),
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.channels.busy {
                    return;
                }
                let typed = detect_token.read(cx).text().trim().to_owned();
                let Some(token) = Some(typed).filter(|t| !t.is_empty()).or_else(telegram::saved_token) else {
                    return this.say(false, tr(french, "Enregistre d’abord le jeton du bot.", "Save the bot token first."), cx);
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
                                tr(french, "Aucun chat trouvé. Envoie /start à ton bot, puis réessaie.", "No chat found. Send /start to your bot, then try again."),
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
            })),
        );
        let save_chat_input = chat.clone();
        let save_chat = button(
            "telegram-chat-save",
            tr(french, "Enregistrer", "Save"),
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let id = save_chat_input.read(cx).text();
                this.update_settings(cx, move |s| s.telegram_chat_id = id);
                if this.message.is_none() {
                    this.say(true, tr(french, "Chat Telegram enregistré.", "Telegram chat saved."), cx);
                }
            })),
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
                div()
                    .id(SharedString::from(format!("chat-{id}")))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .cursor_pointer()
                    .text_sm()
                    .hover(|d| d.bg(rgba(0xffffff12)))
                    .child(format!("{name} · {id}"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let id = id.clone();
                        pick.update(cx, |i, cx| i.set_text(id.clone(), cx));
                        this.update_settings(cx, move |s| s.telegram_chat_id = id);
                        this.channels.chats.clear();
                        if this.message.is_none() {
                            this.say(true, tr(french, "Chat Telegram enregistré.", "Telegram chat saved."), cx);
                        }
                    }))
                    .into_any_element()
            })
            .collect();

        let test_chat = chat.clone();
        let test = button(
            "telegram-test",
            if busy { tr(french, "Envoi…", "Sending…") } else { tr(french, "Envoyer un test", "Send a test") },
            rgb(TEXT),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                if this.channels.busy {
                    return;
                }
                // Same gate as the Tauri app: sending stays off until validated end to end.
                if !telegram::AVAILABLE {
                    return this.say(false, tr(french, "Les alertes Telegram arrivent bientôt.", "Telegram alerts are coming soon."), cx);
                }
                let chat = test_chat.read(cx).text().trim().to_owned();
                if !telegram::valid_chat_id(&chat) {
                    return this.say(false, "Identifiant de chat Telegram invalide : un nombre (ex. 123456789 ou -100…) ou @nom_du_canal.".into(), cx);
                }
                let Some(token) = telegram::saved_token() else {
                    return this.say(false, tr(french, "Enregistre d’abord le jeton du bot.", "Save the bot token first."), cx);
                };
                let body = telegram::test_message(french, chrono::Utc::now().timestamp());
                this.run_remote(
                    cx,
                    async move { telegram::send(&token, &chat, body).await },
                    tr(french, "Message de test envoyé. Regarde Telegram.", "Test message sent. Check Telegram."),
                )
            })),
        );

        let control = if telegram::AVAILABLE {
            toggle("telegram-enabled", enabled, self.setting(cx, |s| s.telegram_enabled = !s.telegram_enabled))
        } else {
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .bg(rgba(0xefa44f22))
                .text_xs()
                .text_color(rgb(0xefa44f))
                .child(tr(french, "Bientôt disponible", "Coming soon"))
                .into_any_element()
        };
        channel_card(
            tr(french, "Alertes Telegram", "Telegram alerts"),
            tr(
                french,
                "Reçois les alertes sur ton téléphone via ton propre bot, même loin du PC. Les messages partent directement de ce PC vers Telegram.",
                "Get alerts on your phone through your own bot, even away from the PC. Messages go straight from this PC to Telegram.",
            ),
            control,
            vec![
                steps([
                    tr(french, "Dans Telegram, ouvre @BotFather, envoie /newbot et copie le jeton du bot.", "In Telegram, open @BotFather, send /newbot and copy the bot token."),
                    tr(french, "Ouvre ton bot et envoie-lui /start (ou ajoute-le à un groupe).", "Open your bot and send it /start (or add it to a group)."),
                    tr(french, "Détecte le chat, puis envoie un message de test.", "Detect the chat, then send a test message."),
                ]),
                label(
                    tr(french, "Jeton du bot", "Bot token"),
                    if saved { tr(french, "déjà enregistré", "already saved") } else { tr(french, "requis", "required") },
                ),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .child(div().flex_1().min_w_0().child(token))
                    .child(token_actions)
                    .into_any_element(),
                label(tr(french, "Chat qui reçoit les alertes", "Chat that gets the alerts"), String::new()),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pt_1()
                    .child(div().flex_1().min_w_0().child(chat))
                    .child(div().flex().gap_2().child(detect).child(save_chat))
                    .into_any_element(),
                div().flex().flex_wrap().gap_2().pt_2().children(chats).into_any_element(),
                div().flex().pt_2().child(test).into_any_element(),
                help(tr(
                    french,
                    "Le jeton reste dans le Gestionnaire d’identification Windows ; seul le message d’alerte est envoyé à Telegram.",
                    "The token stays in Windows Credential Manager; only the alert message is sent to Telegram.",
                )),
            ],
        )
    }
}
