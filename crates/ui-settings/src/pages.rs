//! Page bodies. Thin rendering over `logic` models; every change goes through
//! `SettingsView::commit` / `stage` (config) or `send` (commands).

use crate::i18n::{t, tf};
use crate::logic::{self, Edit, Tone, UpdateAction};
use crate::view::{Field, Limit, SettingsView};
use crate::widgets::{
    self, caption, eid, fill_rest, group, item, item_disabled, mark_avatar, mark_for, note,
    section_title, Btn, BtnKind, ComboBox, MenuEntry, MoreMenu, Toggle, CAPTION, CONTROL_H, DESC,
    FONT, LOGO,
};
use crate::Page;
use codenotch_core::config::{
    AntigravityLimit, AntigravityModel, ColorTransition, Edge, NotchSize, ShowMode, TaskbarMode,
    ThemeMode, UpdateMode, WeeklyRing,
};
use codenotch_core::format;
use codenotch_core::secrets::SecretKey;
use codenotch_core::{Command, Lang, ProviderKind, Snapshot};
use codenotch_ui_kit::controls::{self, Input, InputState, Slider};
use codenotch_ui_kit::Theme;
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, AppContext, Context, Div, DragMoveEvent, Entity, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};

const FIELD_W: f32 = 220.0;
/// Price / rate inputs of the plans card and their unit column (right edges aligned).
const PLAN_INPUT_W: f32 = 120.0;
const PLAN_SUFFIX_W: f32 = 56.0;

fn s(text: impl Into<String>) -> SharedString {
    SharedString::from(text.into())
}

fn tone_color(t_: &Theme, tone: Tone) -> gpui::Hsla {
    match tone {
        Tone::Normal => t_.muted_foreground,
        Tone::Muted => t_.muted_foreground.opacity(0.75),
        Tone::Warn => t_.warning,
        Tone::Error => t_.destructive,
    }
}

fn index_of<T: PartialEq>(options: &[T], value: &T) -> Option<usize> {
    options.iter().position(|o| o == value)
}

impl SettingsView {
    pub(crate) fn render_page(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lang = self.lang();
        match self.page {
            Page::Onboarding => self.page_onboarding(snap, t_, lang, cx),
            Page::Accounts => self.page_accounts(snap, t_, lang, window, cx),
            Page::Appearance => self.page_appearance(snap, t_, lang, cx),
            Page::Notifications => self.page_notifications(snap, t_, lang, cx),
            Page::Analytics => self.page_analytics(snap, t_, lang, window, cx),
            Page::General => self.page_general(snap, t_, lang, cx),
        }
        .into_any_element()
    }

    // -----------------------------------------------------------------------------------------
    // Shared bits
    // -----------------------------------------------------------------------------------------

    fn toggle(
        &self,
        id: &'static str,
        on: bool,
        t_: &Theme,
        cx: &Context<Self>,
        make: impl Fn(bool) -> Edit + 'static,
    ) -> Toggle {
        self.switch(id, on, t_).on_change(self.on_edit(cx, make))
    }

    /// A switch (no On/Off text, as in shadcn).
    fn switch(&self, id: impl Into<gpui::ElementId>, on: bool, t_: &Theme) -> Toggle {
        Toggle::new(id, on, *t_)
    }

    /// One choice among several: a Select.
    fn segmented<T: Clone + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: Vec<(T, String)>,
        current: &T,
        t_: &Theme,
        cx: &Context<Self>,
        make: impl Fn(T) -> Edit + 'static,
    ) -> ComboBox {
        let values: Vec<T> = options.iter().map(|(v, _)| v.clone()).collect();
        let selected = index_of(&values, current);
        let on_edit = self.on_edit(cx, make);
        ComboBox::new(id, options.into_iter().map(|(_, l)| l), selected, *t_).on_change(
            move |i, window, cx| {
                if let Some(v) = values.get(i) {
                    on_edit(v.clone(), window, cx)
                }
            },
        )
    }

    fn input(&self, field: Field, width: f32, t_: &Theme) -> impl IntoElement {
        self.text_box(self.inputs.get(field), width, t_)
    }

    /// shadcn Input: 36 px, 8 px radius, 1 px `input` border, control fill; while focused
    /// (click or Tab) the border turns `ring` and the 3 px `ring`/50 ring sits just outside it.
    /// The frame is drawn here, once, around the gpui-component `Input` (which draws no border
    /// or fill of its own: its own focus border was a few px off this frame, so the two looked
    /// like a doubled ring).
    fn text_box(&self, input: &Entity<InputState>, width: f32, t_: &Theme) -> impl IntoElement {
        let focused = self.focused_input == Some(input.entity_id());
        let frame = div()
            .size_full()
            .flex()
            .items_center()
            .rounded(px(widgets::RADIUS))
            .border_1()
            .border_color(if focused { t_.ring } else { t_.input })
            .bg(t_.control)
            .when(t_.card_shadow, |d| d.shadow_xs())
            .child(
                Input::new(input)
                    .appearance(false)
                    .w_full()
                    .h_full()
                    .px(px(11.)),
            );
        div()
            .flex_none()
            .w(px(width))
            .h(px(CONTROL_H))
            .relative()
            .child(frame)
            .when(focused, |d| {
                d.child(widgets::focus_ring(t_, widgets::RADIUS))
            })
    }

    fn field_error(&self, field: Field, t_: &Theme, lang: Lang) -> Option<Div> {
        self.errors
            .get(&field)
            .map(|msg| note(t_.destructive, t(lang, msg).to_string()))
    }

    fn slider(&self, limit: Limit, t_: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (state, focus, value, id) = match limit {
            Limit::Warn => (
                &self.warn,
                &self.warn_focus,
                self.edited.settings.alert_percent,
                "slider-warn",
            ),
            Limit::Critical => (
                &self.crit,
                &self.crit_focus,
                self.edited.settings.critical_percent,
                "slider-crit",
            ),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .id(id)
                    .w(px(180.))
                    .px(px(4.))
                    .py(px(4.))
                    .rounded(px(widgets::RADIUS))
                    .border_2()
                    .border_color(gpui::transparent_black())
                    .track_focus(&focus.clone().tab_stop(true))
                    .when(controls::focus_visible(cx), |d| {
                        d.focus(|s| s.border_color(t_.ring.opacity(0.5)))
                    })
                    .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                        this.on_slider_key(limit, e, window, cx)
                    }))
                    .child(Slider::new(state)),
            )
            .child(
                div()
                    .min_w(px(36.))
                    .flex()
                    .justify_end()
                    .text_color(t_.muted_foreground)
                    .child(format!("{value} %")),
            )
    }

    // -----------------------------------------------------------------------------------------
    // Accounts
    // -----------------------------------------------------------------------------------------

    fn page_accounts(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = logic::account_rows(lang, &self.edited, &snap.accounts, snap.now);
        // No "Accounts" section header: it would repeat the page title.
        let mut page = div().flex().flex_col();

        let list = if rows.is_empty() {
            group(
                t_,
                vec![item(
                    t_,
                    t(lang, "No account found yet.").to_string(),
                    None,
                    div(),
                )
                .into_any_element()],
            )
        } else {
            self.account_list(rows, t_, lang, window, cx)
        };
        page = page
            // Dropping anywhere on the page commits the order shown (released outside the
            // window, the drag is cancelled).
            .on_drop(cx.listener(|this, _: &AccountDrag, _, cx| this.drop_account(cx)))
            .child(list)
            .child(caption(
                t_,
                t(lang, "Enabled accounts appear on the notch in this order. Drag a card by its handle on the left, or use Move up / Move down in its … menu, to reorder them. Accounts of one provider stay together.")
                    .to_string(),
            ));

        // Antigravity options (only when an Antigravity account is on).
        if self
            .edited
            .enabled_accounts()
            .any(|a| a.provider == ProviderKind::Antigravity)
        {
            let st = &self.edited.settings;
            let limit = self.segmented(
                "ag-limit",
                vec![
                    (
                        AntigravityLimit::Automatic,
                        t(lang, "Automatic").to_string(),
                    ),
                    (
                        AntigravityLimit::FiveHour,
                        t(lang, "5-Hour Limit").to_string(),
                    ),
                    (
                        AntigravityLimit::Weekly,
                        t(lang, "Weekly limit").to_string(),
                    ),
                ],
                &st.antigravity_limit,
                t_,
                cx,
                Edit::AntigravityLimit,
            );
            let model = self.segmented(
                "ag-model",
                vec![
                    (
                        AntigravityModel::Gemini,
                        t(lang, "Gemini models").to_string(),
                    ),
                    (
                        AntigravityModel::ThirdParty,
                        t(lang, "Claude and GPT models").to_string(),
                    ),
                ],
                &st.antigravity_model,
                t_,
                cx,
                Edit::AntigravityModel,
            );
            page = page
                .child(section_title(t_, "Antigravity", false))
                .child(group(
                    t_,
                    vec![
                        item(t_, t(lang, "Notch reads").to_string(), None, limit)
                            .into_any_element(),
                        item(t_, t(lang, "Model data").to_string(), None, model).into_any_element(),
                    ],
                ));
        }

        // Claude Code hooks.
        let installed = self.edited.settings.hooks_enabled;
        let hooks_btn = if installed {
            Btn::new("hooks", t(lang, "Remove hooks").to_string(), *t_)
                .on_click(self.on_command(Command::RemoveHooks))
        } else {
            Btn::new("hooks", t(lang, "Install hooks").to_string(), *t_)
                .primary()
                .on_click(self.on_command(Command::InstallHooks))
        };
        let status = if installed {
            t(lang, "Hooks installed")
        } else {
            t(lang, "Hooks not installed")
        };
        page = page.child(section_title(t_, "Claude Code", false)).child(group(
            t_,
            vec![item(
                t_,
                t(lang, "Let Claude Code notify CodeNotch").to_string(),
                Some(s(t(lang, "Shows when Claude Code is working or waiting for you. Adds small hooks to Claude Code's settings file."))),
                div().flex().items_center().gap(px(12.)).child(div().text_size(px(CAPTION)).text_color(t_.muted_foreground).child(status.to_string())).child(hooks_btn),
            )
            .into_any_element()],
        ));

        // Add accounts.
        let mut add_items = vec![item(
            t_,
            t(lang, "Add a Claude account").to_string(),
            Some(s(t(
                lang,
                "Signs in a second Claude profile in the browser.",
            ))),
            Btn::new("add-claude", t(lang, "Add…").to_string(), *t_)
                .on_click(self.on_command(Command::AddClaudeAccount)),
        )
        .into_any_element()];
        if self.adding_custom {
            add_items.push(self.custom_form(t_, lang, cx).into_any_element());
        } else {
            add_items.push(
                item(
                    t_,
                    t(lang, "Add a custom endpoint").to_string(),
                    Some(s(t(
                        lang,
                        "A JSON usage endpoint of your own (team proxy, gateway).",
                    ))),
                    Btn::new("add-custom", t(lang, "Add…").to_string(), *t_).on_click(
                        self.on_view(cx, |this, window, cx| {
                            this.adding_custom = true;
                            let input = this.inputs.custom_name.clone();
                            input.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element(),
            );
        }
        page = page
            .child(section_title(
                t_,
                t(lang, "Add an account").to_string(),
                false,
            ))
            .child(group(t_, add_items));

        // Detected accounts the user removed: not added back by discovery until asked here.
        let removed = self.edited.settings.removed_accounts.clone();
        if removed.is_empty() {
            return page;
        }
        let rows = removed
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let name = if m.name.trim().is_empty() {
                    m.provider.display_name().to_string()
                } else {
                    m.name.clone()
                };
                let desc = if m.profile_path.trim().is_empty() {
                    t(lang, "Not detected again automatically.").to_string()
                } else {
                    format!(
                        "{} · {}",
                        m.profile_path,
                        t(lang, "Not detected again automatically.")
                    )
                };
                item(
                    t_,
                    name,
                    Some(s(desc)),
                    Btn::new(eid("add-back", i), t(lang, "Add back").to_string(), *t_).on_click(
                        self.on_view(cx, move |this, _, cx| {
                            this.commit(Edit::ForgetRemoved(i), cx);
                            // Discovery runs with a full refresh.
                            this.send(Command::Refresh(None), cx);
                        }),
                    ),
                )
                .into_any_element()
            })
            .collect();
        page.child(section_title(
            t_,
            t(lang, "Removed accounts").to_string(),
            false,
        ))
        .child(group(t_, rows))
    }

    /// The account cards in the order shown (the live preview order while a card is dragged):
    /// each card has a drag handle (⋮⋮, shown on hover) on its left edge; the dragged card's
    /// slot is a dashed placeholder and the other cards move around it as the pointer moves.
    fn account_list(
        &mut self,
        rows: Vec<logic::AccountRow>,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let order: Vec<(String, ProviderKind)> = rows
            .iter()
            .map(|r| (r.id.clone(), r.provider.clone()))
            .collect();
        let ids: Vec<String> = order.iter().map(|(id, _)| id.clone()).collect();
        // Dev captures: a forced drag state (see `DevUi`).
        #[cfg(debug_assertions)]
        if let Some((from, slot)) = self.dev.drag.take() {
            if let Some((id, _)) = order.get(from) {
                self.reorder = Some(crate::view::Reorder {
                    id: id.clone(),
                    preview: logic::drop_order(&order, from, slot).unwrap_or_else(|| ids.clone()),
                    held: true,
                });
            }
        }
        // A drag that ended without a drop on the page (released outside, Escape) leaves
        // nothing behind.
        if self
            .reorder
            .as_ref()
            .is_some_and(|r| !r.held && !cx.has_active_drag())
        {
            self.reorder = None;
        }
        // Rows deleted meanwhile: forget the drag.
        if self
            .reorder
            .as_ref()
            .is_some_and(|r| r.preview.len() != ids.len() || !ids.contains(&r.id))
        {
            self.reorder = None;
        }
        let shown: Vec<String> = self
            .reorder
            .as_ref()
            .map_or_else(|| ids.clone(), |r| r.preview.clone());
        let dragged = self.reorder.as_ref().map(|r| r.id.clone());
        let held = self.reorder.as_ref().is_some_and(|r| r.held);
        let card_w = self.content_width(window, codenotch_ui_kit::metrics::SETTINGS_CONTENT_MAX_W);

        let mut rows = rows;
        rows.sort_by_key(|r| {
            shown
                .iter()
                .position(|id| *id == r.id)
                .unwrap_or(usize::MAX)
        });
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            // Every move of an account drag (inside or outside the list): auto-scroll near the
            // top / bottom of the scrolling pane.
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<AccountDrag>, _, cx| {
                this.drag_autoscroll(f32::from(e.event.position.y), cx)
            }));
        for row in rows {
            let id = row.id.clone();
            let index = ids.iter().position(|i| *i == id).unwrap_or(0);
            let is_dragged = dragged.as_deref() == Some(id.as_str());
            let preview = AccountDragPreview {
                theme: *t_,
                name: row.name.clone(),
                subtitle: row.subtitle.clone(),
                letters: row.letters,
                provider: row.provider.clone(),
                enabled: row.enabled,
                width: card_w,
            };
            let body = self.account_row(row, index, &order, t_, lang, cx);
            let group_name = SharedString::from(format!("acct:{id}"));
            let over_id = id.clone();
            let mut slot = div()
                .id(eid("acct", &id))
                .relative()
                .group(group_name.clone())
                .on_drag_move(
                    cx.listener(move |this, e: &DragMoveEvent<AccountDrag>, _, cx| {
                        this.drag_over_account(&over_id, e, cx)
                    }),
                );
            if is_dragged {
                // The slot the card will land in: dashed outline, same height.
                slot = slot.child(
                    div()
                        .rounded(px(widgets::CARD_RADIUS))
                        .border_1()
                        .border_dashed()
                        .border_color(t_.muted_foreground.opacity(0.7))
                        .bg(t_.muted.opacity(0.35))
                        .child(body.opacity(0.)),
                );
                if held {
                    // Dev capture only: no real pointer, draw the floating copy by hand.
                    slot = slot.child(
                        div()
                            .absolute()
                            .top(px(26.))
                            .left(px(18.))
                            .child(preview.render_card(mark_for(&preview.provider, cx))),
                    );
                }
            } else {
                let view = cx.entity().downgrade();
                let drag = AccountDrag { id: id.clone() };
                let handle = div()
                    .id(eid("grip", &id))
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(gpui::CursorStyle::ResizeUpDown)
                    .child(
                        grip(t_)
                            .opacity(0.)
                            .group_hover(group_name.clone(), |s| s.opacity(1.)),
                    )
                    .on_drag(drag, move |drag: &AccountDrag, _, _, cx| {
                        let id = drag.id.clone();
                        let _ = view.update(cx, |this, cx| this.start_drag(id, cx));
                        let preview = preview.clone();
                        cx.new(|_| preview)
                    });
                slot = slot.child(widgets::card(t_).child(body)).child(handle);
            }
            list = list.child(slot);
        }
        list
    }

    fn account_row(
        &self,
        row: logic::AccountRow,
        index: usize,
        order: &[(String, ProviderKind)],
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = row.id.clone();
        let renaming = self.renaming.as_deref() == Some(row.id.as_str());
        let enable = {
            let id = id.clone();
            self.switch(eid("enable", &id), row.enabled, t_)
                .on_change(self.on_edit(cx, move |on| Edit::AccountEnabled { id: id.clone(), on }))
        };
        let mut status = row.status.clone();
        if row.enabled && !row.visible {
            status = format!(
                "{status}{}{}",
                if status.is_empty() { "" } else { " · " },
                t(lang, "hidden from the notch")
            );
        }
        if row.refreshing {
            status = format!("{status} {}", format::ELLIPSIS);
        }

        // Windows 11 Settings card: logo, name / identity · plan / status, then Sign in (when
        // needed), the On/Off switch and a "More options" menu with the other actions.
        let kind = row.provider.clone();
        let mut entries: Vec<MenuEntry> = Vec::new();
        if row.enabled {
            let (vid, visible) = (id.clone(), row.visible);
            let label = if visible {
                t(lang, "Hide from notch")
            } else {
                t(lang, "Show on notch")
            };
            entries.push(MenuEntry::item(
                label.to_string(),
                false,
                self.on_view(cx, move |this, _, cx| {
                    this.commit(
                        Edit::AccountVisible {
                            id: vid.clone(),
                            on: !visible,
                        },
                        cx,
                    )
                }),
            ));
            entries.push(MenuEntry::Separator);
        }
        // Keyboard / menu reorder: past the neighbour (with its provider when it is the
        // neighbouring provider), the same order model as dragging.
        let _ = kind;
        for (up, label, enabled) in [
            (true, "Move up", row.can_move_up),
            (false, "Move down", row.can_move_down),
        ] {
            let next = logic::step_order(order, index, up);
            entries.push(MenuEntry::item(
                t(lang, label).to_string(),
                !enabled || next.is_none(),
                self.on_view(cx, move |this, _, cx| {
                    if let Some(ids) = next.clone() {
                        this.commit_with_label(Edit::ReorderAccounts(ids), "Order saved", cx)
                    }
                }),
            ));
        }
        entries.push(MenuEntry::Separator);
        let (rid, rname) = (id.clone(), row.name.clone());
        entries.push(MenuEntry::item(
            t(lang, "Rename").to_string(),
            false,
            self.on_view(cx, move |this, w, cx| {
                this.start_rename(rid.clone(), rname.clone(), w, cx)
            }),
        ));
        // Every account can leave CodeNotch, except the last one on the notch.
        entries.push(MenuEntry::Separator);
        let did = id.clone();
        entries.push(MenuEntry::destructive(
            t(lang, "Remove account…").to_string(),
            !row.removable,
            (!row.removable).then(|| s(t(lang, "The last account on the notch cannot be removed"))),
            self.on_view(cx, move |this, window, cx| {
                this.open_remove_confirm(did.clone(), window, cx)
            }),
        ));
        let more = MoreMenu::new(
            eid("more", &id),
            t(lang, "More options").to_string(),
            entries,
            *t_,
        );

        let text = fill_rest(div().flex().flex_col())
            .child(
                div()
                    .text_size(px(FONT))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t_.foreground)
                    .truncate()
                    .child(row.name.clone()),
            )
            .when_some(row.subtitle.clone(), |d, sub| {
                d.child(
                    div()
                        .text_size(px(DESC))
                        .line_height(px(18.))
                        .text_color(t_.muted_foreground)
                        .truncate()
                        .child(sub),
                )
            })
            .when(!status.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(DESC))
                        .line_height(px(18.))
                        .text_color(tone_color(t_, row.tone))
                        .child(status.clone()),
                )
            });

        let controls = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .when(row.signed_out && row.enabled, |d| {
                d.child(
                    Btn::new(eid("signin", &id), t(lang, "Sign in").to_string(), *t_)
                        .primary()
                        .on_click(self.on_command(Command::SignIn(id.clone()))),
                )
            })
            .child(enable)
            .child(more);

        let header = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .min_h(px(40.))
            .child(mark_avatar(
                t_,
                row.letters,
                mark_for(&row.provider, cx),
                !row.enabled,
            ))
            .child(text)
            .child(controls);

        let editor = renaming.then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .pl(px(LOGO + 16.))
                .child(self.input(Field::Rename, 220., t_))
                .child(
                    Btn::new(eid("rename-ok", &id), t(lang, "Save").to_string(), *t_)
                        .primary()
                        .on_click(self.on_view(cx, |this, w, cx| this.commit_rename(w, cx))),
                )
                .child(
                    Btn::new(
                        eid("rename-cancel", &id),
                        t(lang, "Cancel").to_string(),
                        *t_,
                    )
                    .on_click(self.on_view(cx, |this, w, cx| this.cancel_rename(w, cx))),
                )
                .children(self.field_error(Field::Rename, t_, lang))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .py(px(13.))
            // Same height for every account card (three text lines), content centred.
            .min_h(px(78.))
            .justify_center()
            .child(header)
            .children(editor)
    }

    fn custom_form(&self, t_: &Theme, lang: Lang, cx: &mut Context<Self>) -> Div {
        let row = |label: String, field: Field, this: &Self| {
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(div().text_size(px(FONT)).child(label))
                        .child(this.input(field, 300., t_)),
                )
                .children(
                    this.field_error(field, t_, lang)
                        .map(|e| e.flex().justify_end()),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(16.))
            .py(px(14.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(t(lang, "Add a custom endpoint").to_string()))
            .child(row(t(lang, "Name").to_string(), Field::CustomName, self))
            .child(row(t(lang, "Endpoint URL").to_string(), Field::CustomUrl, self))
            .child(row(t(lang, "Bearer token").to_string(), Field::CustomToken, self))
            .child(div().text_size(px(CAPTION)).text_color(t_.muted_foreground).child(
                t(lang, "The token is kept in Windows Credential Manager, never in the config file.").to_string(),
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(Btn::new("custom-cancel", t(lang, "Cancel").to_string(), *t_).on_click(self.on_view(cx, |this, w, cx| this.cancel_custom(w, cx))))
                    .child(Btn::new("custom-add", t(lang, "Add").to_string(), *t_).primary().on_click(self.on_view(cx, |this, w, cx| this.add_custom(w, cx)))),
            )
    }

    // -----------------------------------------------------------------------------------------
    // Appearance
    // -----------------------------------------------------------------------------------------

    fn page_appearance(
        &mut self,
        _snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        let st = self.edited.settings.clone();
        let taskbar_on = st.taskbar_mode != TaskbarMode::Off;

        // Taskbar: the notch's rings as a strip in the taskbar's free space. The floating-notch
        // rows below are greyed out while it is on.
        let taskbar = self.segmented(
            "taskbar",
            vec![
                (TaskbarMode::Off, t(lang, "Notch").to_string()),
                (TaskbarMode::Left, t(lang, "Taskbar, left").to_string()),
                (TaskbarMode::Right, t(lang, "Taskbar, right").to_string()),
            ],
            &st.taskbar_mode,
            t_,
            cx,
            Edit::TaskbarMode,
        );
        let mut page = div()
            .flex()
            .flex_col()
            .child(section_title(t_, t(lang, "Taskbar").to_string(), true))
            .child(group(
                t_,
                vec![item(
                    t_,
                    t(lang, "Show rings in").to_string(),
                    Some(s(t(
                        lang,
                        "In the taskbar the rings use its free space: hover one for its card, click to refresh, right-click for Settings. They hide when there is no room, the taskbar is hidden or an app is full screen.",
                    ))),
                    taskbar,
                )
                .into_any_element()],
            ));

        let show = self
            .segmented(
                "show",
                vec![
                    (ShowMode::Always, t(lang, "Always show").to_string()),
                    (ShowMode::Hover, t(lang, "Show on hover").to_string()),
                    (ShowMode::Hidden, t(lang, "Hide").to_string()),
                ],
                &st.show_mode(),
                t_,
                cx,
                Edit::ShowMode,
            )
            .disabled(taskbar_on);
        let show_caption = match st.show_mode() {
            ShowMode::Always => t(lang, "The pill stays open on the edge of the screen."),
            ShowMode::Hover => t(
                lang,
                "A slim tab rests on the edge and opens when the pointer touches it.",
            ),
            ShowMode::Hidden => t(
                lang,
                "No pill. The tray icon stays, and reset cards still appear.",
            ),
        };
        let size = self.segmented(
            "size",
            vec![
                (NotchSize::Small, t(lang, "Small").to_string()),
                (NotchSize::Medium, t(lang, "Medium").to_string()),
                (NotchSize::Large, t(lang, "Large").to_string()),
            ],
            &st.notch_size,
            t_,
            cx,
            Edit::NotchSize,
        );
        let theme = self.segmented(
            "theme",
            vec![
                (ThemeMode::System, t(lang, "System").to_string()),
                (ThemeMode::Light, t(lang, "Light").to_string()),
                (ThemeMode::Dark, t(lang, "Dark").to_string()),
            ],
            &st.theme,
            t_,
            cx,
            Edit::Theme,
        );
        let weekly = self.segmented(
            "weekly",
            vec![
                (WeeklyRing::Off, t(lang, "Off").to_string()),
                (WeeklyRing::Inside, t(lang, "Inside").to_string()),
                (WeeklyRing::Outside, t(lang, "Outside").to_string()),
            ],
            &st.weekly_ring,
            t_,
            cx,
            Edit::WeeklyRing,
        );
        let edge = self
            .segmented(
                "edge",
                vec![
                    (Edge::Left, t(lang, "Left").to_string()),
                    (Edge::Right, t(lang, "Right").to_string()),
                    (Edge::Top, t(lang, "Top").to_string()),
                    (Edge::Bottom, t(lang, "Bottom").to_string()),
                ],
                &st.widget_position,
                t_,
                cx,
                Edit::Edge,
            )
            .disabled(taskbar_on);

        let mut notch: Vec<AnyElement> = vec![
            item_disabled(t_, t(lang, "Show").to_string(), Some(s(show_caption)), show, taskbar_on).into_any_element(),
            item_disabled(
                t_,
                t(lang, "Fold for full-screen apps").to_string(),
                Some(s(t(lang, "Folds the pill at once while a full-screen app is in front. Hovering still opens it."))),
                self.toggle("fold", st.fold_for_full_screen, t_, cx, Edit::FoldForFullScreen).disabled(taskbar_on),
                taskbar_on,
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Adaptive pill").to_string(),
                Some(s(t(lang, "The folded pill turns light over dark backgrounds and dark over light ones."))),
                self.toggle("adaptive", st.adaptive_pill, t_, cx, Edit::AdaptivePill).disabled(taskbar_on),
                taskbar_on,
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Game mode").to_string(),
                Some(s(t(lang, "Hides the notch and keeps only critical alerts."))),
                self.toggle("game", st.game_mode, t_, cx, Edit::GameMode),
            )
            .into_any_element(),
            item(t_, t(lang, "Size").to_string(), None, size).into_any_element(),
            item(t_, t(lang, "Theme").to_string(), None, theme).into_any_element(),
            item(t_, t(lang, "Weekly ring").to_string(), Some(s(t(lang, "A second, thinner ring for the weekly limit."))), weekly).into_any_element(),
        ];
        if st.weekly_ring != WeeklyRing::Off {
            notch.push(
                item(
                    t_,
                    t(lang, "Dashed weekly ring").to_string(),
                    None,
                    self.toggle(
                        "dashed",
                        st.weekly_ring_dashed,
                        t_,
                        cx,
                        Edit::WeeklyRingDashed,
                    ),
                )
                .into_any_element(),
            );
        }
        notch.push(
            item(
                t_,
                t(lang, "Show remaining instead of used").to_string(),
                Some(s(t(
                    lang,
                    "Figures and rings count what is left. Colours still follow what is used.",
                ))),
                self.toggle("remaining", st.show_remaining, t_, cx, Edit::ShowRemaining),
            )
            .into_any_element(),
        );
        notch.push(
            item(
                t_,
                t(lang, "Show usage pace").to_string(),
                Some(s(t(lang, "Shows how far ahead of an even pace you are."))),
                self.toggle("pace", st.show_usage_pace, t_, cx, Edit::ShowUsagePace),
            )
            .into_any_element(),
        );
        notch.push(
            item(
                t_,
                t(lang, "Show the forecast").to_string(),
                Some(s(t(
                    lang,
                    "Estimates from your recent activity when each limit will run out, or how much will be used at the reset.",
                ))),
                self.toggle("forecast", st.show_forecast, t_, cx, Edit::ShowForecast),
            )
            .into_any_element(),
        );
        notch.push(
            item_disabled(t_, t(lang, "Edge").to_string(), None, edge, taskbar_on)
                .into_any_element(),
        );
        if self.monitors.len() > 1 {
            let list: Vec<(String, bool)> = self
                .monitors
                .iter()
                .map(|m| (m.name.clone(), m.primary))
                .collect();
            let (labels, selected, values) =
                logic::monitor_choices(lang, &list, &st.widget_placement.monitor);
            let on_edit = self.on_edit(cx, Edit::Monitor);
            let screen = ComboBox::new("screen", labels, Some(selected), *t_)
                .disabled(taskbar_on)
                .on_change(move |i, w, cx| {
                    if let Some(v) = values.get(i) {
                        on_edit(v.clone(), w, cx)
                    }
                });
            notch.push(
                item_disabled(t_, t(lang, "Screen").to_string(), None, screen, taskbar_on)
                    .into_any_element(),
            );
        }
        let recentre = {
            let model = self.model.clone();
            let view = cx.entity().downgrade();
            Btn::new("recentre", t(lang, "Recentre").to_string(), *t_)
                .disabled(taskbar_on)
                .on_click(move |_, cx| {
                    model.read(cx).send(Command::Recentre);
                    let _ = view.update(cx, |this, cx| {
                        let lang = this.lang();
                        this.toast_success(
                            t(lang, "The notch is back in the middle").to_string(),
                            cx,
                        );
                    });
                })
        };
        notch.push(
            item_disabled(
                t_,
                t(lang, "Position").to_string(),
                Some(s(t(
                    lang,
                    "Hold Alt and drag the notch, or use its grip, to move it along the edges.",
                ))),
                recentre,
                taskbar_on,
            )
            .into_any_element(),
        );
        let notch_title = if taskbar_on {
            t(lang, "Indicators")
        } else {
            t(lang, "Notch")
        };
        page = page
            .child(section_title(t_, notch_title.to_string(), false))
            .child(group(t_, notch));

        page.child(section_title(
            t_,
            t(lang, "Usage Limits").to_string(),
            false,
        ))
        .child(self.limits_group(t_, lang, cx))
    }

    fn limits_group(&self, t_: &Theme, lang: Lang, cx: &mut Context<Self>) -> Div {
        let st = &self.edited.settings;
        let transition = self.segmented(
            "transition",
            vec![
                (ColorTransition::HardStep, t(lang, "Hard step").to_string()),
                (ColorTransition::Ramp, t(lang, "Colour ramp").to_string()),
            ],
            &st.color_transition,
            t_,
            cx,
            Edit::ColorTransition,
        );
        let transition_caption = match st.color_transition {
            ColorTransition::HardStep => t(lang, "Rings jump from green to amber at the watch limit, then to red at the critical limit."),
            ColorTransition::Ramp => t(lang, "Rings blend from green to amber up to the watch limit, then towards red at 100 %."),
        };
        group(
            t_,
            vec![
                item(
                    t_,
                    t(lang, "Colour transition").to_string(),
                    Some(s(transition_caption)),
                    transition,
                )
                .into_any_element(),
                item(
                    t_,
                    t(lang, "Watch limit").to_string(),
                    None,
                    self.slider(Limit::Warn, t_, cx),
                )
                .into_any_element(),
                item(
                    t_,
                    t(lang, "Critical limit").to_string(),
                    None,
                    self.slider(Limit::Critical, t_, cx),
                )
                .into_any_element(),
                item(
                    t_,
                    t(lang, "Reset to defaults").to_string(),
                    Some(s(tf(
                        lang,
                        "Watch {w} %, critical {c} %",
                        &[
                            ("w", &logic::WARN_DEFAULT.to_string()),
                            ("c", &logic::CRIT_DEFAULT.to_string()),
                        ],
                    ))),
                    Btn::new("limits-reset", t(lang, "Reset").to_string(), *t_)
                        .on_click(self.on_view(cx, |this, w, cx| this.reset_limits(w, cx))),
                )
                .into_any_element(),
            ],
        )
    }

    // -----------------------------------------------------------------------------------------
    // Notifications
    // -----------------------------------------------------------------------------------------

    fn page_notifications(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        let st = self.edited.settings.clone();
        let off = !st.notifications;

        let mut alerts = vec![
            item(
                t_,
                t(lang, "Desktop notifications").to_string(),
                Some(s(t(
                    lang,
                    "Alerts when a limit crosses the watch or critical threshold.",
                ))),
                self.toggle(
                    "notifications",
                    st.notifications,
                    t_,
                    cx,
                    Edit::Notifications,
                ),
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Watch limit").to_string(),
                None,
                self.slider(Limit::Warn, t_, cx),
                off,
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Critical limit").to_string(),
                None,
                self.slider(Limit::Critical, t_, cx),
                off,
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Alerts in full screen when critical").to_string(),
                Some(s(t(
                    lang,
                    "Lets critical alerts through while a full-screen app or game is in front.",
                ))),
                self.toggle(
                    "crit-fs",
                    st.critical_in_fullscreen,
                    t_,
                    cx,
                    Edit::CriticalInFullscreen,
                )
                .disabled(off),
                off,
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Forecast alerts").to_string(),
                Some(s(t(
                    lang,
                    "Warns when a limit is projected to run out soon at the current pace.",
                ))),
                self.toggle("forecast", st.forecast_alerts, t_, cx, Edit::ForecastAlerts)
                    .disabled(off),
                off,
            )
            .into_any_element(),
        ];
        if st.forecast_alerts {
            let options = logic::choices_with_current(logic::FORECAST_CHOICES, st.forecast_minutes)
                .into_iter()
                .map(|m| (m, logic::minutes_label(m)))
                .collect();
            let minutes = self
                .segmented(
                    "forecast-min",
                    options,
                    &st.forecast_minutes,
                    t_,
                    cx,
                    Edit::ForecastMinutes,
                )
                .disabled(off);
            alerts.push(
                item_disabled(t_, t(lang, "Warn ahead by").to_string(), None, minutes, off)
                    .into_any_element(),
            );
        }

        let resets = vec![
            item(
                t_,
                t(lang, "Reset notifications").to_string(),
                Some(s(t(lang, "A desktop notification when a limit renews."))),
                self.toggle("reset-alerts", st.reset_alerts, t_, cx, Edit::ResetAlerts),
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Reset cards").to_string(),
                Some(s(t(lang, "A card next to the notch when a limit renews."))),
                self.toggle("reset-cards", st.reset_cards, t_, cx, Edit::ResetCards),
            )
            .into_any_element(),
            item_disabled(
                t_,
                t(lang, "Notification sound").to_string(),
                None,
                self.toggle("reset-sound", st.reset_sound, t_, cx, Edit::ResetSound)
                    .disabled(!st.reset_cards),
                !st.reset_cards,
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Preview the reset card").to_string(),
                None,
                Btn::new("preview", t(lang, "Preview card").to_string(), *t_)
                    .on_click(self.on_command(Command::PreviewResetCard)),
            )
            .into_any_element(),
        ];

        // Telegram.
        let tg_token = snap.secrets.telegram_token;
        let mut telegram = vec![
            item(
                t_,
                t(lang, "Send alerts to Telegram").to_string(),
                None,
                self.toggle(
                    "telegram",
                    st.telegram_enabled,
                    t_,
                    cx,
                    Edit::TelegramEnabled,
                ),
            )
            .into_any_element(),
            self.field_item(
                t_,
                lang,
                t(lang, "Chat id").to_string(),
                Field::ChatId,
                None,
            )
            .into_any_element(),
            self.secret_item(
                t_,
                lang,
                cx,
                t(lang, "Bot token").to_string(),
                Field::TelegramToken,
                SecretKey::TelegramToken,
                tg_token,
            )
            .into_any_element(),
        ];
        let tg_ready = st.telegram_enabled && tg_token && !st.telegram_chat_id.is_empty();
        telegram.push(
            item(
                t_,
                t(lang, "Send a test").to_string(),
                (!tg_ready).then(|| {
                    s(t(
                        lang,
                        "Turn it on and set the chat id and the bot token first.",
                    ))
                }),
                Btn::new("tg-test", t(lang, "Test").to_string(), *t_)
                    .disabled(!tg_ready)
                    .on_click(self.on_command(Command::TestTelegram)),
            )
            .into_any_element(),
        );

        // Discord.
        let dc_hook = snap.secrets.discord_webhook;
        let dc_ready = st.discord_enabled && dc_hook;
        let discord = vec![
            item(
                t_,
                t(lang, "Send alerts to Discord").to_string(),
                None,
                self.toggle("discord", st.discord_enabled, t_, cx, Edit::DiscordEnabled),
            )
            .into_any_element(),
            self.secret_item(
                t_,
                lang,
                cx,
                t(lang, "Webhook URL").to_string(),
                Field::DiscordWebhook,
                SecretKey::DiscordWebhook,
                dc_hook,
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Send a test").to_string(),
                (!dc_ready).then(|| s(t(lang, "Turn it on and set the webhook first."))),
                Btn::new("dc-test", t(lang, "Test").to_string(), *t_)
                    .disabled(!dc_ready)
                    .on_click(self.on_command(Command::TestDiscord)),
            )
            .into_any_element(),
        ];

        div()
            .flex()
            .flex_col()
            .child(section_title(t_, t(lang, "Alerts").to_string(), true))
            .child(group(t_, alerts))
            .child(section_title(t_, t(lang, "Resets").to_string(), false))
            .child(group(t_, resets))
            .child(section_title(t_, "Telegram", false))
            .child(group(t_, telegram))
            .child(section_title(t_, "Discord", false))
            .child(group(t_, discord))
            .child(caption(
                t_,
                t(lang, "Tokens and webhooks are kept in Windows Credential Manager (or the CODENOTCH_TELEGRAM_TOKEN / CODENOTCH_DISCORD_WEBHOOK variables), never in the config file.")
                    .to_string(),
            ))
    }

    /// Label + text field (+ error under it).
    fn field_item(
        &self,
        t_: &Theme,
        lang: Lang,
        label: String,
        field: Field,
        desc: Option<SharedString>,
    ) -> Div {
        let control = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(4.))
            .child(self.input(field, FIELD_W, t_))
            .children(self.field_error(field, t_, lang));
        item(t_, label, desc, control)
    }

    /// Secret field: shows only "Saved" / "Not set"; Save sends `SetSecret` and clears the field.
    #[allow(clippy::too_many_arguments)]
    fn secret_item(
        &self,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
        label: String,
        field: Field,
        key: SecretKey,
        present: bool,
    ) -> Div {
        let status = if present {
            t(lang, "Saved")
        } else {
            t(lang, "Not set")
        };
        let save_key = key.clone();
        let control = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(self.input(field, 200., t_))
                    .child(
                        Btn::new(
                            eid("secret-save", key.target_name()),
                            t(lang, "Save").to_string(),
                            *t_,
                        )
                        .on_click(self.on_view(cx, move |this, w, cx| {
                            this.save_secret(save_key.clone(), w, cx)
                        })),
                    )
                    .when(present, |d| {
                        let key = key.clone();
                        d.child(
                            Btn::new(
                                eid("secret-clear", key.target_name()),
                                t(lang, "Remove").to_string(),
                                *t_,
                            )
                            .kind(BtnKind::Destructive)
                            .on_click(
                                self.on_view(cx, move |this, _, cx| {
                                    this.clear_secret(key.clone(), cx)
                                }),
                            ),
                        )
                    }),
            )
            .children(self.field_error(field, t_, lang));
        item(t_, label, Some(s(status)), control)
    }

    // -----------------------------------------------------------------------------------------
    // Analytics
    // -----------------------------------------------------------------------------------------

    // The page itself: `pages_stats.rs`.

    pub(crate) fn plans_group(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        // Every row: the same input width and a fixed suffix column, so the fields' right
        // edges line up; all rows in one card, divided by hairlines.
        let suffix = |text: &str| {
            div()
                .flex_none()
                .w(px(PLAN_SUFFIX_W))
                .text_size(px(CAPTION))
                .text_color(t_.muted_foreground)
                .child(text.to_string())
        };
        let field = |input: AnyElement, unit: &str, error: Option<Div>| {
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(input)
                        .child(suffix(unit)),
                )
                .children(error)
        };
        let mut items = vec![item(
            t_,
            t(lang, "1 US dollar in euros").to_string(),
            Some(s(t(lang, "Used to show costs in euros."))),
            field(
                self.input(Field::Rate, PLAN_INPUT_W, t_).into_any_element(),
                "€",
                self.field_error(Field::Rate, t_, lang),
            ),
        )];
        let settings = self.edited.settings.clone();
        // Whole amounts: "200 $", "184 €" ("$200" in English).
        let whole = |v: f64, currency: &str| {
            format::money(lang, v.round(), currency)
                .replace(",00", "")
                .replace(".00", "")
        };
        for line in logic::plan_lines(&self.edited, &snap.accounts) {
            let plan = line.plan.as_deref();
            let list_usd = codenotch_core::config::list_price_usd(&line.provider, plan);
            let list_eur = logic::list_price_eur(&settings, &line.provider, plan);
            // An empty field uses the list price: show it as the placeholder.
            let placeholder = list_eur.map_or_else(
                || format::DASH.to_string(),
                |eur| format!("≈ {}", logic::format_number(lang, eur.round())),
            );
            let input = self.ensure_plan_input(&line.key, &placeholder, window, cx);
            let mut desc: Vec<String> = Vec::new();
            if line.accounts > 1 {
                desc.push(tf(
                    lang,
                    "{n} accounts",
                    &[("n", &line.accounts.to_string())],
                ));
            }
            match logic::resolve_plan_price(&settings, &line.provider, plan) {
                logic::PlanPrice::List(eur) => desc.push(tf(
                    lang,
                    "List price {usd} / month (≈ {eur}), used while the field is empty",
                    &[
                        ("usd", &whole(list_usd.unwrap_or_default(), "USD")),
                        ("eur", &whole(eur, "EUR")),
                    ],
                )),
                logic::PlanPrice::Off => {
                    desc.push(t(lang, "Left out of the comparison (0)").to_string())
                }
                logic::PlanPrice::User(_) | logic::PlanPrice::Unknown => {}
            }
            let desc = (!desc.is_empty()).then(|| s(desc.join(" · ")));
            let error = self
                .plan_errors
                .get(&line.key)
                .map(|m| note(t_.destructive, t(lang, m).to_string()));
            items.push(item(
                t_,
                line.key.clone(),
                desc,
                field(
                    self.text_box(&input, PLAN_INPUT_W, t_).into_any_element(),
                    t(lang, "€ / month"),
                    error,
                ),
            ));
        }
        let rows = items
            .into_iter()
            .enumerate()
            .map(|(i, row)| row.when(i > 0, |d| d.border_t_1().border_color(t_.border)));
        div()
            .flex()
            .flex_col()
            .child(crate::widgets::card(t_).children(rows))
            .child(caption(t_, t(lang, "Monthly price of each plan, to compare with the API-equivalent cost. Leave empty to use the public list price when there is one, enter 0 to leave a plan out.").to_string()))
    }

    // -----------------------------------------------------------------------------------------
    // General
    // -----------------------------------------------------------------------------------------

    fn page_general(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        let st = self.edited.settings.clone();
        let language = self.segmented(
            "language",
            Lang::ALL
                .iter()
                .map(|l| (*l, l.native_name().to_string()))
                .collect(),
            &st.lang(),
            t_,
            cx,
            Edit::Language,
        );
        let poll = self.segmented(
            "poll",
            logic::choices_with_current(logic::POLL_CHOICES, st.poll_seconds)
                .into_iter()
                .map(|v| (v, logic::seconds_label(v)))
                .collect(),
            &st.poll_seconds,
            t_,
            cx,
            Edit::PollSeconds,
        );
        let history = self.segmented(
            "history",
            logic::choices_with_current(logic::HISTORY_CHOICES, st.history_hours)
                .into_iter()
                .map(|v| (v, logic::hours_label(lang, v)))
                .collect(),
            &st.history_hours,
            t_,
            cx,
            Edit::HistoryHours,
        );
        let app = vec![
            item(t_, t(lang, "Language").to_string(), None, language).into_any_element(),
            item(
                t_,
                t(lang, "Open CodeNotch at login").to_string(),
                None,
                self.toggle(
                    "startup",
                    st.launch_at_startup,
                    t_,
                    cx,
                    Edit::LaunchAtStartup,
                ),
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Tray shows usage").to_string(),
                Some(s(t(lang, "The tray icon tooltip lists the percentages."))),
                self.toggle("tray", st.tray_shows_usage, t_, cx, Edit::TrayShowsUsage),
            )
            .into_any_element(),
            self.field_item(
                t_,
                lang,
                t(lang, "Global shortcut").to_string(),
                Field::Hotkey,
                Some(s(t(
                    lang,
                    "Shows or hides the notch. Example: Ctrl+Alt+N. Leave empty for none.",
                ))),
            )
            .into_any_element(),
            item(t_, t(lang, "Refresh every").to_string(), None, poll).into_any_element(),
            item(t_, t(lang, "Keep history for").to_string(), None, history).into_any_element(),
        ];

        let (line, action) =
            logic::update_line(lang, &snap.update, snap.update_checked_at, snap.now);
        let check_btn = |busy: bool| {
            let b = Btn::new("update", t(lang, "Check for updates").to_string(), *t_);
            if busy {
                b.disabled(true)
            } else {
                b.on_click(self.on_command(Command::CheckUpdate))
            }
        };
        let update_btn = match action {
            UpdateAction::Install => div().flex().gap(px(8.)).child(check_btn(false)).child(
                Btn::new("update-install", t(lang, "Install update").to_string(), *t_)
                    .primary()
                    .on_click(self.on_command(Command::InstallUpdate)),
            ),
            UpdateAction::Check => div().child(check_btn(false)),
            UpdateAction::Busy => div().child(check_btn(true)),
        };
        let mode = self.segmented(
            "update-mode",
            vec![
                (UpdateMode::Notify, t(lang, "Propose").to_string()),
                (
                    UpdateMode::Auto,
                    t(lang, "Install automatically").to_string(),
                ),
                (UpdateMode::Off, t(lang, "Off").to_string()),
            ],
            &st.update_mode(),
            t_,
            cx,
            Edit::UpdateMode,
        );
        let updates = vec![
            item(
                t_,
                tf(lang, "Version {v}", &[("v", codenotch_core::VERSION)]),
                Some(s(line)),
                update_btn,
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "New versions").to_string(),
                Some(s(t(
                    lang,
                    "Propose: a notification offers each new version. Install automatically: downloaded in the background and installed when CodeNotch is idle. Off: only when you check.",
                ))),
                mode,
            )
            .into_any_element(),
            self.field_item(
                t_,
                lang,
                t(lang, "Update address").to_string(),
                Field::UpdateUrl,
                Some(s(t(
                    lang,
                    "Advanced. The update.json file CodeNotch reads. Leave empty for the official one.",
                ))),
            )
            .into_any_element(),
        ];

        let data = vec![
            item(
                t_,
                t(lang, "Data folder").to_string(),
                Some(s(t(lang, "Settings, history and logs."))),
                Btn::new("data", t(lang, "Open folder").to_string(), *t_)
                    .on_click(self.on_command(Command::OpenDataFolder)),
            )
            .into_any_element(),
            item(
                t_,
                t(lang, "Quit CodeNotch").to_string(),
                None,
                Btn::new("quit", t(lang, "Quit").to_string(), *t_)
                    .kind(BtnKind::Destructive)
                    .on_click(self.on_command(Command::Quit)),
            )
            .into_any_element(),
        ];

        let credits = div()
            .flex()
            .flex_col()
            .pt(px(8.))
            .child(caption(
                t_,
                t(
                    lang,
                    "Design after Codenotch by @hivinz_ (vinzdg/codenotch).",
                )
                .to_string(),
            ))
            .child(
                caption(
                    t_,
                    t(lang, "Provider marks from lobe-icons (MIT licence).").to_string(),
                )
                .pt(px(0.)),
            );

        div()
            .flex()
            .flex_col()
            .child(section_title(t_, t(lang, "App").to_string(), true))
            .child(group(t_, app))
            .child(section_title(t_, t(lang, "Updates").to_string(), false))
            .child(group(t_, updates))
            .child(section_title(t_, t(lang, "Data").to_string(), false))
            .child(group(t_, data))
            .child(credits)
    }

    // -----------------------------------------------------------------------------------------
    // Onboarding
    // -----------------------------------------------------------------------------------------

    fn page_onboarding(
        &mut self,
        snap: &Snapshot,
        t_: &Theme,
        lang: Lang,
        cx: &mut Context<Self>,
    ) -> Div {
        const STEPS: usize = 3;
        let step = self.step.min(STEPS - 1);
        let st = self.edited.settings.clone();
        let body: Div = match step {
            0 => div()
                .flex()
                .flex_col()
                .child(section_title(t_, t(lang, "Language").to_string(), true))
                .child(group(
                    t_,
                    vec![item(
                        t_,
                        t(lang, "Language").to_string(),
                        None,
                        self.segmented("ob-language", Lang::ALL.iter().map(|l| (*l, l.native_name().to_string())).collect(), &st.lang(), t_, cx, Edit::Language),
                    )
                    .into_any_element()],
                ))
                .child(caption(t_, t(lang, "CodeNotch shows how much of your AI subscriptions you have used, in a small notch on the edge of the screen.").to_string())),
            1 => {
                let found = logic::discovered_accounts(&snap.accounts);
                let mut items: Vec<AnyElement> = Vec::new();
                for v in &found {
                    let Some(acc) = self.edited.account(&v.account.id) else { continue };
                    let id = acc.id.clone();
                    let (status, tone) = logic::status_line_full(lang, Some(v), acc.enabled, &st, snap.now);
                    items.push(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(16.))
                            .py(px(11.))
                            .min_h(px(66.))
                            .child(mark_avatar(t_, acc.provider.glyph_letters(), mark_for(&acc.provider, cx), !acc.enabled))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(div().truncate().child(acc.name.clone()))
                                    .when(!status.is_empty(), |d| d.child(div().text_size(px(CAPTION)).text_color(tone_color(t_, tone)).truncate().child(status))),
                            )
                            .child(
                                self.switch(eid("ob-enable", &id), acc.enabled, t_)
                                    .on_change(self.on_edit(cx, move |on| Edit::AccountEnabled { id: id.clone(), on })),
                            )
                            .into_any_element(),
                    );
                }
                let mut d = div().flex().flex_col().child(section_title(t_, t(lang, "Accounts").to_string(), true));
                if items.is_empty() {
                    d = d.child(caption(t_, t(lang, "No account found yet.").to_string())).child(caption(
                        t_,
                        t(lang, "Sign in to Claude Code, Codex or another supported tool, then add it in Settings › Accounts.").to_string(),
                    ));
                } else {
                    d = d.child(caption(t_, t(lang, "We found these accounts on this PC.").to_string())).child(group(t_, items));
                }
                d.child(group(
                    t_,
                    vec![item(
                        t_,
                        t(lang, "Add a Claude account").to_string(),
                        None,
                        Btn::new("ob-add-claude", t(lang, "Add…").to_string(), *t_).on_click(self.on_command(Command::AddClaudeAccount)),
                    )
                    .into_any_element()],
                ))
            }
            _ => {
                let edge = self.segmented(
                    "ob-edge",
                    vec![
                        (Edge::Left, t(lang, "Left").to_string()),
                        (Edge::Right, t(lang, "Right").to_string()),
                        (Edge::Top, t(lang, "Top").to_string()),
                        (Edge::Bottom, t(lang, "Bottom").to_string()),
                    ],
                    &st.widget_position,
                    t_,
                    cx,
                    Edit::Edge,
                );
                div()
                    .flex()
                    .flex_col()
                    .child(section_title(t_, t(lang, "Notch").to_string(), true))
                    .child(group(
                        t_,
                        vec![
                            item(t_, t(lang, "Edge").to_string(), Some(s(t(lang, "Hold Alt and drag the notch to move it later."))), edge).into_any_element(),
                            item(
                                t_,
                                t(lang, "Open CodeNotch at login").to_string(),
                                None,
                                self.toggle("ob-startup", st.launch_at_startup, t_, cx, Edit::LaunchAtStartup),
                            )
                            .into_any_element(),
                        ],
                    ))
            }
        };

        let finish = self.on_view(cx, |this, window, cx| {
            this.commit(Edit::MarkOnboarded, cx);
            this.step = 0;
            this.go(Page::Accounts, window, cx);
        });
        let back = self.on_view(cx, |this, _, cx| {
            this.step = this.step.saturating_sub(1);
            cx.notify();
        });
        let next = self.on_view(cx, |this, _, cx| {
            this.flush(cx);
            this.step = (this.step + 1).min(STEPS - 1);
            cx.notify();
        });
        let skip = self.on_view(cx, |this, window, cx| {
            this.commit(Edit::MarkOnboarded, cx);
            this.step = 0;
            this.go(Page::Accounts, window, cx);
        });
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .mt(px(20.))
            .child(
                Btn::new("ob-skip", t(lang, "Skip").to_string(), *t_)
                    .kind(BtnKind::Ghost)
                    .on_click(skip),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(CAPTION))
                    .text_color(t_.muted_foreground)
                    .flex()
                    .justify_center()
                    .child(tf(
                        lang,
                        "Step {n} of {total}",
                        &[
                            ("n", &(step + 1).to_string()),
                            ("total", &STEPS.to_string()),
                        ],
                    )),
            )
            .when(step > 0, |d| {
                d.child(Btn::new("ob-back", t(lang, "Back").to_string(), *t_).on_click(back))
            })
            .map(|d| {
                if step + 1 < STEPS {
                    d.child(
                        Btn::new("ob-next", t(lang, "Continue").to_string(), *t_)
                            .primary()
                            .on_click(next),
                    )
                } else {
                    d.child(
                        Btn::new("ob-done", t(lang, "Finish").to_string(), *t_)
                            .primary()
                            .on_click(finish),
                    )
                }
            });

        div()
            .max_w(px(640.))
            .w_full()
            .mx_auto()
            .flex()
            .flex_col()
            .child(body)
            .child(footer)
    }
}

// ---------------------------------------------------------------------------------------------
// Account drag and drop
// ---------------------------------------------------------------------------------------------

/// Payload of an account card drag.
#[derive(Debug, Clone)]
pub(crate) struct AccountDrag {
    pub id: String,
}

/// The floating, semi-transparent copy of a card that follows the pointer while dragging.
#[derive(Clone)]
pub(crate) struct AccountDragPreview {
    theme: Theme,
    name: String,
    subtitle: Option<String>,
    letters: &'static str,
    provider: ProviderKind,
    enabled: bool,
    width: f32,
}

impl AccountDragPreview {
    fn render_card(&self, mark: Option<SharedString>) -> Div {
        let t_ = &self.theme;
        div()
            .w(px(self.width))
            .opacity(0.88)
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(16.))
            .py(px(13.))
            .min_h(px(78.))
            .rounded(px(widgets::CARD_RADIUS))
            .border_1()
            .border_color(t_.ring.opacity(0.6))
            .bg(t_.card)
            .shadow_lg()
            .font_family(t_.fonts.text)
            .text_color(t_.foreground)
            .child(mark_avatar(t_, self.letters, mark, !self.enabled))
            .child(
                fill_rest(div().flex().flex_col())
                    .child(
                        div()
                            .text_size(px(FONT))
                            .line_height(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(self.name.clone()),
                    )
                    .when_some(self.subtitle.clone(), |d, sub| {
                        d.child(
                            div()
                                .text_size(px(DESC))
                                .line_height(px(18.))
                                .text_color(t_.muted_foreground)
                                .truncate()
                                .child(sub),
                        )
                    }),
            )
    }
}

impl gpui::Render for AccountDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mark = mark_for(&self.provider, cx);
        self.render_card(mark)
    }
}

/// ⋮⋮ grip: 2 × 3 dots.
fn grip(t_: &Theme) -> Div {
    div().flex().gap(px(2.)).children((0..2).map(|_| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children((0..3).map(|_| div().size(px(2.5)).rounded_full().bg(t_.muted_foreground)))
    }))
}

/// Auto-scroll speed (px per 16 ms frame) for a pointer at `y` in a pane spanning
/// `top..bottom`: up to 18 px within 48 px of an edge, 0 elsewhere.
pub(crate) fn autoscroll_speed(y: f32, top: f32, bottom: f32) -> f32 {
    const ZONE: f32 = 48.0;
    const MAX: f32 = 18.0;
    if bottom - top < 3.0 * ZONE {
        return 0.0;
    }
    if y < top + ZONE {
        -MAX * ((top + ZONE - y) / ZONE).min(1.0)
    } else if y > bottom - ZONE {
        MAX * ((y - (bottom - ZONE)) / ZONE).min(1.0)
    } else {
        0.0
    }
}

impl SettingsView {
    /// A card started moving (`on_drag`): the preview order starts as the current one.
    pub(crate) fn start_drag(&mut self, id: String, cx: &mut Context<Self>) {
        let snap = self.model.read(cx).snapshot.clone();
        let ids: Vec<String> =
            logic::account_rows(self.lang(), &self.edited, &snap.accounts, snap.now)
                .into_iter()
                .map(|r| r.id)
                .collect();
        self.reorder = Some(crate::view::Reorder {
            id,
            preview: ids,
            held: false,
        });
        cx.notify();
    }

    /// The pointer moved over card `target` while dragging: dropping now would put the dragged
    /// card before it (upper half) or after it (lower half). Re-renders only when the preview
    /// order changes.
    fn drag_over_account(
        &mut self,
        target: &str,
        e: &DragMoveEvent<AccountDrag>,
        cx: &mut Context<Self>,
    ) {
        if !e.bounds.contains(&e.event.position) {
            return;
        }
        let Some(r) = self.reorder.as_ref() else {
            return;
        };
        if r.id == target {
            return;
        }
        let rows: Vec<(String, ProviderKind)> = r
            .preview
            .iter()
            .filter_map(|id| {
                self.edited
                    .account(id)
                    .map(|a| (id.clone(), a.provider.clone()))
            })
            .collect();
        let (Some(from), Some(j)) = (
            rows.iter().position(|(id, _)| *id == r.id),
            rows.iter().position(|(id, _)| id == target),
        ) else {
            return;
        };
        let mid = e.bounds.origin.y + e.bounds.size.height / 2.0;
        let slot = if e.event.position.y < mid { j } else { j + 1 };
        if let Some(next) = logic::drop_order(&rows, from, slot) {
            if let Some(r) = self.reorder.as_mut() {
                r.preview = next;
            }
            cx.notify();
        }
    }

    /// Dropped on the page: save the order shown (through the usual config save, which also
    /// reorders the notch cells).
    fn drop_account(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.reorder.take() else {
            return;
        };
        self.drag_scroll = 0.0;
        let snap = self.model.read(cx).snapshot.clone();
        let current: Vec<String> =
            logic::account_rows(self.lang(), &self.edited, &snap.accounts, snap.now)
                .into_iter()
                .map(|row| row.id)
                .collect();
        if r.preview != current {
            self.commit_with_label(Edit::ReorderAccounts(r.preview), "Order saved", cx);
        } else {
            cx.notify();
        }
    }

    /// Escape while dragging: put everything back.
    pub(crate) fn cancel_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.reorder.take().is_none() {
            return false;
        }
        self.drag_scroll = 0.0;
        cx.stop_active_drag(window);
        cx.notify();
        true
    }

    /// Scroll the pane while the pointer is near its top / bottom edge during a drag (a 16 ms
    /// timer runs only while needed).
    fn drag_autoscroll(&mut self, y: f32, cx: &mut Context<Self>) {
        let b = self.scroll.bounds();
        self.drag_scroll = autoscroll_speed(y, f32::from(b.top()), f32::from(b.bottom()));
        if self.drag_scroll == 0.0 || self.drag_scroll_task.is_some() {
            return;
        }
        self.drag_scroll_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(16))
                .await;
            let more = this
                .update(cx, |this, cx| {
                    if this.reorder.is_none() || this.drag_scroll == 0.0 || !cx.has_active_drag() {
                        this.drag_scroll_task = None;
                        return false;
                    }
                    let off = this.scroll.offset();
                    let max = this.scroll.max_offset().height;
                    let y = (off.y - px(this.drag_scroll)).clamp(-max, px(0.));
                    this.scroll.set_offset(gpui::point(off.x, y));
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !more {
                break;
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::autoscroll_speed;

    #[test]
    fn autoscroll_only_near_the_pane_edges() {
        // Pane 100..700.
        assert_eq!(autoscroll_speed(400.0, 100.0, 700.0), 0.0);
        assert!(autoscroll_speed(110.0, 100.0, 700.0) < 0.0);
        assert!(autoscroll_speed(690.0, 100.0, 700.0) > 0.0);
        // Faster closer to the edge, capped (and past it).
        assert!(autoscroll_speed(101.0, 100.0, 700.0) < autoscroll_speed(140.0, 100.0, 700.0));
        assert_eq!(autoscroll_speed(800.0, 100.0, 700.0), 18.0);
        // A pane too short for two edge zones never scrolls.
        assert_eq!(autoscroll_speed(110.0, 100.0, 200.0), 0.0);
    }
}
