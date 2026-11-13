//! Teams page: opt-in sync with a CodeNotch Teams server. The switch, the server address and
//! the shared accounts are config edits; pairing, declarations, detach and erase are engine
//! commands. The invite link goes out as a secret (`SecretValue`) and is never saved.

use crate::i18n::{t, tf};
use crate::logic::{self, Edit};
use crate::pages::{s, tone_color};
use crate::view::{Field, SettingsView, TeamsConfirm};
use crate::widgets::{
    caption, card, eid, group, item, note, section_title, Btn, BtnKind, ComboBox, CAPTION, DESC,
    FONT,
};
use codenotch_core::format;
use codenotch_core::model::{TeamsState, TeamsVisibility};
use codenotch_core::secrets::SecretValue;
use codenotch_core::{Command, Lang, Snapshot, TeamsPayer};
use codenotch_ui_kit::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, Context, Div, FontWeight, IntoElement, ParentElement, Styled, Window,
};

const INVITE_W: f32 = 260.0;
const PRICE_W: f32 = 120.0;

/// One "what leaves / what never leaves" list.
fn privacy_list(t_: &Theme, title: String, lines: Vec<String>, mark: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(20.))
        .py(px(14.))
        .child(
            div()
                .text_size(px(FONT))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t_.foreground)
                .child(title),
        )
        .children(lines.into_iter().map(|line| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(px(DESC))
                .line_height(px(18.))
                .text_color(t_.muted_foreground)
                .child(div().flex_none().child(mark))
                .child(div().child(line))
        }))
}

impl SettingsView {
    /// Exactly what leaves this PC and what never does (shown before and after pairing).
    fn teams_privacy(&self, t_: &Theme, lang: Lang) -> Div {
        let leaves = vec![
            t(lang, "Tool, plan, quota percentages and reset times").to_string(),
            t(lang, "Token counts per model and hour").to_string(),
            t(
                lang,
                "An account reference made on this PC, the app version and queue counters",
            )
            .to_string(),
        ];
        let never = vec![
            t(lang, "Prompts, code and model responses").to_string(),
            t(lang, "File paths, folder and repository names").to_string(),
            t(lang, "Sign-in e-mail and provider tokens").to_string(),
            t(lang, "Accounts you do not share, and every other tool").to_string(),
        ];
        card(t_)
            .child(privacy_list(
                t_,
                t(lang, "What leaves this PC").to_string(),
                leaves,
                "+",
            ))
            .child(
                privacy_list(
                    t_,
                    t(lang, "What never leaves this PC").to_string(),
                    never,
                    "−",
                )
                .border_t_1()
                .border_color(t_.border),
            )
    }

    pub(crate) fn page_teams(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let st = self.edited.settings.clone();
        let mut status = snap.teams.clone();
        if st.teams_sync_enabled && status.state == TeamsState::Off {
            // Switched on, the engine has not answered yet (or no engine: --demo).
            status.state = TeamsState::NotPaired;
        }
        let paired = logic::teams_paired(&status);
        let busy = status.busy;
        let (line, tone) = logic::teams_status_line(lang, &status);

        let mut sync: Vec<AnyElement> = vec![
            item(
                t_,
                t(lang, "Team sync").to_string(),
                Some(s(t(
                    lang,
                    "Shares quota and token figures with your team's CodeNotch Teams server. Off by default.",
                ))),
                self.toggle(
                    "teams-sync",
                    st.teams_sync_enabled,
                    t_,
                    cx,
                    Edit::TeamsSyncEnabled,
                ),
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Status").to_string(),
                None,
                note(tone_color(t_, tone), line),
            )
            .into_any_element(),
            self.field_item(
                t_,
                lang,
                t(lang, "Server address").to_string(),
                Field::TeamsEndpoint,
                Some(s(t(
                    lang,
                    "Your team's server, for example https://teams.example.com. Changing it requires pairing again.",
                ))),
            )
            .into_any_element(),
        ];
        if paired {
            let mut desc: Vec<String> = Vec::new();
            if let Some(origin) = &status.origin {
                desc.push(origin.clone());
            }
            desc.push(match status.last_sent_at {
                Some(at) => tf(
                    lang,
                    "Last sent: {when}",
                    &[("when", &format::age(lang, at, snap.now))],
                ),
                None => t(lang, "Nothing sent yet").to_string(),
            });
            sync.push(
                item(
                    t_,
                    tf(
                        lang,
                        "Connected to {team}",
                        &[("team", status.team_name.as_deref().unwrap_or(""))],
                    ),
                    Some(s(desc.join(" · "))),
                    Btn::new("teams-flush", t(lang, "Send now").to_string(), *t_)
                        .disabled(
                            status.state != TeamsState::Active
                                && status.state != TeamsState::Paused,
                        )
                        .on_click(self.on_command(Command::TeamsFlush)),
                )
                .into_any_element(),
            );
            let model = self.model.clone();
            sync.push(
                item(
                    t_,
                    t(lang, "Include the 45 days before pairing").to_string(),
                    Some(s(t(
                        lang,
                        "A separate agreement: your token counts of the 45 days before pairing are sent too. Turn it off to withdraw it.",
                    ))),
                    self.switch("teams-backfill", status.backfill, t_)
                        .on_change(move |on, _, cx| {
                            model.read(cx).send(Command::TeamsAllowBackfill(on))
                        }),
                )
                .into_any_element(),
            );
        }

        let mut page = div()
            .flex()
            .flex_col()
            .child(section_title(t_, t(lang, "Team sync").to_string(), true))
            .child(group(t_, sync));

        // Joining: invite → preview (team, visibility, server) → consent.
        if st.teams_sync_enabled && !paired {
            let invite = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(self.input(Field::TeamsInvite, INVITE_W, t_))
                .child(
                    Btn::new("teams-connect", t(lang, "Connect").to_string(), *t_)
                        .disabled(busy)
                        .on_click(self.on_view(cx, |this, _, cx| this.teams_preview(cx))),
                );
            let mut join = vec![item(
                t_,
                t(lang, "Invite link").to_string(),
                Some(s(t(
                    lang,
                    "Sent by your team admin. It only goes to the server address above.",
                ))),
                invite,
            )
            .into_any_element()];
            if let Some(preview) = &status.preview {
                let sees = match preview.visibility {
                    TeamsVisibility::Named => {
                        t(lang, "Your team admin sees your usage by name").to_string()
                    }
                };
                join.push(
                    item(
                        t_,
                        preview.team_name.clone(),
                        Some(s(format!(
                            "{} · {}",
                            sees,
                            tf(lang, "Server: {origin}", &[("origin", &preview.origin)])
                        ))),
                        Btn::new(
                            "teams-accept",
                            t(lang, "Accept and connect").to_string(),
                            *t_,
                        )
                        .primary()
                        .disabled(busy)
                        .on_click(self.on_view(cx, |this, w, cx| this.teams_accept(w, cx))),
                    )
                    .into_any_element(),
                );
            }
            page = page
                .child(section_title(t_, t(lang, "Join a team").to_string(), false))
                .child(group(t_, join));
        }

        page = page
            .child(section_title(t_, t(lang, "Privacy").to_string(), false))
            .child(self.teams_privacy(t_, lang));

        if st.teams_sync_enabled {
            page = page
                .child(section_title(
                    t_,
                    t(lang, "Shared accounts").to_string(),
                    false,
                ))
                .child(self.teams_accounts(snap, t_, lang, paired, window, cx))
                .child(caption(
                    t_,
                    t(lang, "Only Claude and Codex accounts can be shared").to_string(),
                ));
        }

        if paired {
            page = page
                .child(section_title(t_, t(lang, "This device").to_string(), false))
                .child(group(t_, self.teams_device(t_, lang, busy, cx)));
        }
        page
    }

    /// One row per shareable account; when paired, who pays for each shared one.
    fn teams_accounts(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        paired: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let accounts: Vec<(codenotch_core::Account, bool)> = logic::teams_accounts(&self.edited)
            .into_iter()
            .map(|(a, on)| (a.clone(), on))
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        if accounts.is_empty() {
            rows.push(
                item(t_, t(lang, "No account to share").to_string(), None, div())
                    .into_any_element(),
            );
        }
        for (account, shared) in accounts {
            let plan = snap
                .account(&account.id)
                .and_then(|v| v.reading.as_ref())
                .and_then(|r| r.plan.clone());
            let desc = match plan {
                Some(plan) => format!("{} · {plan}", account.provider.display_name()),
                None => account.provider.display_name().to_string(),
            };
            let id = account.id.clone();
            rows.push(
                item(
                    t_,
                    account.name.clone(),
                    Some(s(desc)),
                    self.switch(eid("teams-share", &account.id), shared, t_)
                        .on_change(
                            self.on_edit(cx, move |on| Edit::TeamsShared { id: id.clone(), on }),
                        ),
                )
                .into_any_element(),
            );
            if !(paired && shared) {
                continue;
            }
            rows.push(self.teams_payer_row(&account.id, t_, lang, window, cx));
        }
        group(t_, rows)
    }

    /// "Who pays for this plan?" (+ the monthly price when the member pays) and Declare.
    fn teams_payer_row(
        &mut self,
        id: &str,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let payer = self
            .teams_payer
            .get(id)
            .copied()
            .unwrap_or(TeamsPayer::Company);
        let payers = [TeamsPayer::Company, TeamsPayer::Member];
        let key = id.to_string();
        let view = cx.entity().downgrade();
        let combo = ComboBox::new(
            eid("teams-payer", id),
            [
                t(lang, "My company").to_string(),
                t(lang, "Me, not reimbursed").to_string(),
            ],
            payers.iter().position(|p| *p == payer),
            *t_,
        )
        .on_change(move |i, _, cx| {
            let key = key.clone();
            let _ = view.update(cx, |this, cx| {
                this.teams_payer.insert(key, payers[i.min(1)]);
                cx.notify();
            });
        });
        let key = id.to_string();
        let declare = Btn::new(
            eid("teams-declare", id),
            t(lang, "Declare").to_string(),
            *t_,
        )
        .on_click(self.on_view(cx, move |this, _, cx| this.teams_declare(&key, cx)));
        let mut control = div().flex().flex_col().items_end().gap(px(6.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(combo)
                .when(payer == TeamsPayer::Company, |d| d.child(declare)),
        );
        let mut desc = None;
        if payer == TeamsPayer::Member {
            let input = self.ensure_teams_price_input(id, window, cx);
            let key = id.to_string();
            control = control.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(self.text_box(&input, PRICE_W, t_))
                    .child(
                        div()
                            .text_size(px(CAPTION))
                            .text_color(t_.muted_foreground)
                            .child(t(lang, "€ / month").to_string()),
                    )
                    .child(
                        Btn::new(
                            eid("teams-declare", id),
                            t(lang, "Declare").to_string(),
                            *t_,
                        )
                        .on_click(
                            self.on_view(cx, move |this, _, cx| this.teams_declare(&key, cx)),
                        ),
                    ),
            );
            if let Some(err) = self.teams_price_errors.get(id) {
                control = control.child(note(t_.destructive, t(lang, err).to_string()));
            }
            desc = Some(s(format!(
                "{} {}",
                t(lang, "Monthly price you pay."),
                t(lang, "Your team sees this amount but cannot change it")
            )));
        }
        item(
            t_,
            t(lang, "Who pays for this plan?").to_string(),
            desc,
            control,
        )
        .pl(px(40.))
        .into_any_element()
    }

    /// Detach and erase, each asking for a second click.
    fn teams_device(
        &self,
        t_: &Theme,
        lang: Lang,
        busy: bool,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let row = |what: TeamsConfirm, label: &str, desc: &str, confirm: &str| {
            let armed = self.teams_confirm == Some(what);
            let control = if armed {
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        Btn::new(
                            eid("teams-cancel", label),
                            t(lang, "Cancel").to_string(),
                            *t_,
                        )
                        .on_click(self.on_view(cx, |this, _, cx| {
                            this.teams_confirm = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        Btn::new(
                            eid("teams-confirm", label),
                            t(lang, confirm).to_string(),
                            *t_,
                        )
                        .kind(BtnKind::Destructive)
                        .disabled(busy)
                        .on_click(self.on_view(cx, move |this, _, cx| {
                            this.teams_confirm = None;
                            this.send(
                                match what {
                                    TeamsConfirm::Detach => Command::TeamsDetach,
                                    TeamsConfirm::Erase => Command::TeamsEraseServerData,
                                },
                                cx,
                            );
                            cx.notify();
                        })),
                    )
            } else {
                div().child(
                    Btn::new(eid("teams-arm", label), t(lang, label).to_string(), *t_)
                        .kind(BtnKind::Destructive)
                        .disabled(busy)
                        .on_click(self.on_view(cx, move |this, _, cx| {
                            this.teams_confirm = Some(what);
                            cx.notify();
                        })),
                )
            };
            item(
                t_,
                t(lang, label).to_string(),
                Some(s(t(lang, desc))),
                control,
            )
            .into_any_element()
        };
        vec![
            row(
                TeamsConfirm::Detach,
                "Detach this device",
                "This device stops syncing. Its pairing, its queue and its device token are deleted from this PC.",
                "Detach",
            ),
            row(
                TeamsConfirm::Erase,
                "Delete my data on the server",
                "Asks the team server to erase everything you sent. Sync goes on from now.",
                "Delete",
            ),
        ]
    }

    /// Read the invite in the field (the engine shows the preview in `snap.teams.preview`).
    pub(crate) fn teams_preview(&mut self, cx: &mut Context<Self>) {
        let value = self.inputs.teams_invite.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        // A server address being typed reaches the engine first.
        self.flush(cx);
        self.send(Command::TeamsPreviewInvite(SecretValue::new(value)), cx);
    }

    /// Accept the previewed invite: pair, then clear the field.
    fn teams_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.inputs.teams_invite.read(cx).value().trim().to_string();
        if value.is_empty() {
            return;
        }
        self.send(Command::TeamsPair(SecretValue::new(value)), cx);
        self.inputs
            .teams_invite
            .update(cx, |s, cx| s.set_value("", window, cx));
    }

    /// Send the payer (and the price when the member pays) of one shared account.
    pub(crate) fn teams_declare(&mut self, id: &str, cx: &mut Context<Self>) {
        let payer = self
            .teams_payer
            .get(id)
            .copied()
            .unwrap_or(TeamsPayer::Company);
        let price = match payer {
            TeamsPayer::Company => None,
            TeamsPayer::Member => {
                let text = self
                    .teams_price_inputs
                    .get(id)
                    .map(|i| i.read(cx).value().to_string())
                    .unwrap_or_default();
                match logic::parse_price(&text) {
                    Ok(Some(eur)) => Some(eur),
                    Ok(None) => {
                        self.teams_price_errors
                            .insert(id.to_string(), "Enter a number");
                        cx.notify();
                        return;
                    }
                    Err(e) => {
                        self.teams_price_errors.insert(id.to_string(), e.message());
                        cx.notify();
                        return;
                    }
                }
            }
        };
        self.teams_price_errors.remove(id);
        self.send(
            Command::TeamsDeclareAccount {
                account_id: id.to_string(),
                payer,
                price_eur_month: price,
            },
            cx,
        );
        cx.notify();
    }
}
