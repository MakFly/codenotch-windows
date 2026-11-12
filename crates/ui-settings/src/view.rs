//! The settings window view: state, editing model, re-sync, chrome (sidebar, header, toast).
//! Page bodies are in `pages.rs`.

use crate::i18n::t;
use crate::logic::{self, Edit, FieldError, Sync};
use crate::widgets::{self, glyph, nav_icon, FONT};
use crate::Page;
use codenotch_core::config::Config;
use codenotch_core::secrets::{SecretKey, SecretValue};
use codenotch_core::{Command, Event, Lang};
use codenotch_ui_kit::controls::{
    self, InputEvent, InputState, SliderEvent, SliderState, SliderValue,
};
use codenotch_ui_kit::metrics::{
    SETTINGS_AUTO_COLLAPSE_W, SETTINGS_CAPTION_W, SETTINGS_CONTENT_MAX_W, SETTINGS_CONTENT_PAD,
    SETTINGS_CONTENT_PAD_NARROW, SETTINGS_HEADER_H, SETTINGS_INSET_MARGIN, SETTINGS_INSET_RADIUS,
    SETTINGS_SIDEBAR_ANIM_MS, SETTINGS_SIDEBAR_RAIL_W, SETTINGS_SIDEBAR_W, SETTINGS_STATS_MAX_W,
};
use codenotch_ui_kit::toast::{self, ToastId, ToastKind, ToastOptions, Toaster};
use codenotch_ui_kit::window::MonitorInfo;
use codenotch_ui_kit::{SideTip, Theme, UiModel};
use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, Context, Entity, EntityId, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, Window, WindowControlArea,
};
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

/// Debounce for text fields and sliders.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// How long "Account removed" offers its undo.
const REMOVE_UNDO_MS: u64 = 6000;

/// Text fields of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Field {
    ChatId,
    TelegramToken,
    DiscordWebhook,
    Hotkey,
    Rate,
    CustomName,
    CustomUrl,
    CustomToken,
    Rename,
    UpdateUrl,
}

pub(crate) struct Inputs {
    pub chat_id: Entity<InputState>,
    pub telegram_token: Entity<InputState>,
    pub discord_webhook: Entity<InputState>,
    pub hotkey: Entity<InputState>,
    pub rate: Entity<InputState>,
    pub custom_name: Entity<InputState>,
    pub custom_url: Entity<InputState>,
    pub custom_token: Entity<InputState>,
    pub rename: Entity<InputState>,
    pub update_url: Entity<InputState>,
}

impl Inputs {
    pub fn get(&self, field: Field) -> &Entity<InputState> {
        match field {
            Field::ChatId => &self.chat_id,
            Field::TelegramToken => &self.telegram_token,
            Field::DiscordWebhook => &self.discord_webhook,
            Field::Hotkey => &self.hotkey,
            Field::Rate => &self.rate,
            Field::CustomName => &self.custom_name,
            Field::CustomUrl => &self.custom_url,
            Field::CustomToken => &self.custom_token,
            Field::Rename => &self.rename,
            Field::UpdateUrl => &self.update_url,
        }
    }
}

/// Which threshold slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Limit {
    Warn,
    Critical,
}

/// Dev-only UI state for window captures (no mouse or keyboard needed), debug builds only:
/// `CODENOTCH_DEV_UI=focus-plan,heat=<cell>,hour=<cell>,drag=<from>:<slot>,sidebar=collapsed`
/// focuses the first plan price field, shows the heatmap / hours tooltip of a cell, the account
/// drag preview, or forces the sidebar collapsed / `expanded` (nothing saved). `toasts=demo`
/// (or `demo-br`, bottom-right) shows a sticky success (with an action) + loading + error stack;
/// `confirm-remove=<index>` opens the remove-account dialog of that account.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct DevUi {
    pub focus_plan: bool,
    pub heat: Option<usize>,
    pub hour: Option<usize>,
    pub drag: Option<(usize, usize)>,
    /// Forced sidebar state: `Some(true)` = icon rail.
    pub sidebar: Option<bool>,
    /// Toast demo stack to show once: "demo" or "demo-br".
    pub toasts: Option<String>,
    /// Open the remove-account dialog for the account at this index, once.
    pub confirm_remove: Option<usize>,
}

impl DevUi {
    pub fn parse(text: &str) -> Self {
        let mut dev = DevUi::default();
        for part in text.split(',').map(str::trim) {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            match key {
                "focus-plan" => dev.focus_plan = true,
                "heat" => dev.heat = value.parse().ok(),
                "hour" => dev.hour = value.parse().ok(),
                "drag" => {
                    dev.drag = value
                        .split_once(':')
                        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                }
                "toasts" if !value.is_empty() => dev.toasts = Some(value.to_string()),
                "confirm-remove" => dev.confirm_remove = value.parse().ok(),
                "sidebar" => {
                    dev.sidebar = match value {
                        "collapsed" => Some(true),
                        "expanded" => Some(false),
                        _ => None,
                    }
                }
                _ => {}
            }
        }
        dev
    }

    fn from_env() -> Self {
        if cfg!(debug_assertions) {
            std::env::var("CODENOTCH_DEV_UI")
                .map(|v| Self::parse(&v))
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }
}

/// An account card being dragged on the Accounts page.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Reorder {
    /// The dragged account.
    pub id: String,
    /// Order the page shows (and a drop saves): every account id.
    pub preview: Vec<String>,
    /// Dev capture: kept without a real pointer drag.
    pub held: bool,
}

pub(crate) struct SettingsView {
    pub model: Entity<UiModel>,
    pub page: Page,
    /// Onboarding step 0..=2.
    pub step: usize,
    /// Local edited copy of the config (always starts from the snapshot's config).
    pub edited: Config,
    pub sync: Sync,
    save_task: Option<Task<()>>,
    /// Deletes a removed custom endpoint's token once its undo expired.
    secret_purge: Option<Task<()>>,
    /// Toast layer of the window (also the app's toast host while the window is open).
    pub toaster: Entity<Toaster>,
    /// The "Saved" toast still on screen: a new save restarts it instead of stacking another.
    saved_toast: Option<ToastId>,
    /// Message of the next save confirmation (default "Saved").
    save_label: Option<&'static str>,
    /// Loading toast of an analytics refresh, and the `generated_at` it waits to change.
    pub analytics_toast: Option<(ToastId, i64)>,
    /// Account id whose removal waits for confirmation (the alert dialog is open).
    pub confirm_remove: Option<String>,
    /// The dialog's buttons (Cancel has the focus when it opens).
    confirm_cancel: FocusHandle,
    confirm_ok: FocusHandle,
    root_focus: FocusHandle,
    nav_focus: Vec<FocusHandle>,
    pub scroll: ScrollHandle,
    pub monitors: Vec<MonitorInfo>,
    pub inputs: Inputs,
    pub plan_inputs: BTreeMap<String, Entity<InputState>>,
    /// Placeholder last set on each plan input.
    plan_placeholders: HashMap<String, String>,
    pub warn: Entity<SliderState>,
    pub crit: Entity<SliderState>,
    pub warn_focus: FocusHandle,
    pub crit_focus: FocusHandle,
    pub errors: HashMap<Field, &'static str>,
    pub plan_errors: HashMap<String, &'static str>,
    /// Account id being renamed.
    pub renaming: Option<String>,
    pub adding_custom: bool,
    /// `generated_at` of the analytics shown when Refresh was pressed (0 = none).
    pub analytics_requested: Option<i64>,
    /// Statistics page filters and chart metric (local, not saved).
    pub stats: crate::stats::StatsUi,
    component_dark: Option<bool>,
    /// Text field focused at the start of this frame (focus ring around it).
    pub focused_input: Option<EntityId>,
    /// Dev-only forced UI state (see [`DevUi`]).
    pub dev: DevUi,
    /// Account card drag in progress (Accounts page).
    pub reorder: Option<Reorder>,
    /// Auto-scroll speed while dragging near the pane's edge (px per frame, 0 = none).
    pub drag_scroll: f32,
    pub drag_scroll_task: Option<Task<()>>,
    /// Sidebar toggle button (header bar).
    toggle_focus: FocusHandle,
    /// The window is narrower than [`SETTINGS_AUTO_COLLAPSE_W`] (sidebar auto-collapsed,
    /// tighter body padding, compact controls). Updated at the start of each frame.
    pub narrow: bool,
    /// Sidebar expanded for this session while the window is narrow (not saved).
    narrow_expanded: bool,
    /// Collapsed state drawn last frame (`None` before the first frame): a change animates.
    sidebar_was_collapsed: Option<bool>,
    /// Sidebar width animation: start, from, to (outer widths).
    sidebar_anim: Option<(Instant, f32, f32)>,
    /// Outer sidebar width drawn this frame (0 without a sidebar).
    pub sidebar_w: f32,
    subs: Vec<Subscription>,
}

/// Outer width of the expanded sidebar (the inset panel follows with no margin).
const SIDEBAR_OPEN_W: f32 = SETTINGS_SIDEBAR_W;
/// Outer width of the collapsed sidebar: the icon rail plus the inset margin before the panel.
const SIDEBAR_RAIL_OUTER_W: f32 = SETTINGS_SIDEBAR_RAIL_W + SETTINGS_INSET_MARGIN;

/// Whether the sidebar shows as the icon rail: the saved preference, except in a narrow window
/// where it is collapsed unless expanded for this session.
pub(crate) fn sidebar_collapsed(pref: bool, narrow: bool, narrow_expanded: bool) -> bool {
    if narrow {
        !narrow_expanded
    } else {
        pref
    }
}

/// Ease-out cubic of a 0..=1 progress.
pub(crate) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Expansion of the sidebar for an outer width: 0 = icon rail, 1 = expanded.
pub(crate) fn sidebar_progress(outer_w: f32) -> f32 {
    ((outer_w - SIDEBAR_RAIL_OUTER_W) / (SIDEBAR_OPEN_W - SIDEBAR_RAIL_OUTER_W)).clamp(0.0, 1.0)
}

/// Every fixed text field.
const FIELDS: [Field; 10] = [
    Field::ChatId,
    Field::TelegramToken,
    Field::DiscordWebhook,
    Field::Hotkey,
    Field::Rate,
    Field::CustomName,
    Field::CustomUrl,
    Field::CustomToken,
    Field::Rename,
    Field::UpdateUrl,
];

/// Sidebar pages, in order.
pub(crate) const NAV: [Page; 5] = [
    Page::Accounts,
    Page::Appearance,
    Page::Notifications,
    Page::Analytics,
    Page::General,
];

pub(crate) fn page_title(page: Page) -> &'static str {
    match page {
        Page::Onboarding => "Welcome to CodeNotch",
        Page::Accounts => "Accounts",
        Page::Appearance => "Appearance",
        Page::Notifications => "Notifications",
        Page::Analytics => "Analytics",
        Page::General => "General",
    }
}

/// Navigation glyph (Segoe Fluent Icons) per page.
fn page_glyph(page: Page) -> &'static str {
    match page {
        Page::Accounts => glyph::ACCOUNTS,
        Page::Appearance => glyph::APPEARANCE,
        Page::Notifications => glyph::NOTIFICATIONS,
        Page::Analytics => glyph::ANALYTICS,
        Page::General | Page::Onboarding => glyph::GENERAL,
    }
}

/// Caption buttons of the title bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Caption {
    Minimize,
    Maximize,
    Close,
}

impl Caption {
    fn id(self) -> &'static str {
        match self {
            Caption::Minimize => "caption-min",
            Caption::Maximize => "caption-max",
            Caption::Close => "caption-close",
        }
    }

    /// Native hit-test code of the button (`HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE`).
    fn area(self) -> WindowControlArea {
        match self {
            Caption::Minimize => WindowControlArea::Min,
            Caption::Maximize => WindowControlArea::Max,
            Caption::Close => WindowControlArea::Close,
        }
    }

    /// Segoe Fluent glyph; the maximise button shows "restore" while maximised.
    fn glyph(self, maximized: bool) -> &'static str {
        match self {
            Caption::Minimize => glyph::MINIMIZE,
            Caption::Maximize if maximized => glyph::RESTORE,
            Caption::Maximize => glyph::MAXIMIZE,
            Caption::Close => glyph::CLOSE,
        }
    }
}

/// A press on the title bar's drag region that maximises / restores the window (the second
/// press of a double-click; a triple-click does not toggle twice).
fn is_title_bar_double_click(click_count: usize) -> bool {
    click_count == 2
}

fn slider_value(v: SliderValue) -> f32 {
    match v {
        SliderValue::Single(v) => v,
        SliderValue::Range(_, end) => end,
    }
}

impl SettingsView {
    pub fn new(
        model: Entity<UiModel>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let edited = model.read(cx).snapshot.config.clone();
        let lang = edited.settings.lang();
        let s = &edited.settings;

        let text =
            |window: &mut Window, cx: &mut Context<Self>, value: String, placeholder: &str| {
                let placeholder = SharedString::from(placeholder.to_string());
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let secret = |window: &mut Window, cx: &mut Context<Self>, placeholder: &str| {
            let placeholder = SharedString::from(placeholder.to_string());
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(true)
            })
        };
        let inputs = Inputs {
            chat_id: text(
                window,
                cx,
                s.telegram_chat_id.as_str().to_string(),
                "123456789",
            ),
            telegram_token: secret(window, cx, t(lang, "Paste the bot token")),
            discord_webhook: secret(window, cx, t(lang, "Paste the webhook URL")),
            hotkey: text(
                window,
                cx,
                s.hotkey.clone().unwrap_or_default(),
                "Ctrl+Alt+N",
            ),
            rate: text(window, cx, logic::format_number(lang, s.usd_to_eur), "0.92"),
            custom_name: text(window, cx, String::new(), t(lang, "Name")),
            custom_url: text(window, cx, String::new(), "https://"),
            custom_token: secret(window, cx, t(lang, "Bearer token (optional)")),
            rename: text(window, cx, String::new(), t(lang, "Name")),
            update_url: text(
                window,
                cx,
                s.update_manifest_url.clone(),
                codenotch_core::DEFAULT_UPDATE_MANIFEST_URL,
            ),
        };
        let warn = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(99.)
                .step(1.)
                .default_value(f32::from(s.alert_percent))
        });
        let crit = cx.new(|_| {
            SliderState::new()
                .min(2.)
                .max(100.)
                .step(1.)
                .default_value(f32::from(s.critical_percent))
        });

        let mut subs = Vec::new();
        subs.push(cx.observe_in(&model, window, |this, _, window, cx| {
            this.on_snapshot(window, cx)
        }));
        subs.push(
            cx.subscribe_in(&model, window, |this, _, event: &Event, _, cx| {
                if let Event::Notice(text) = event {
                    this.notice(text, cx);
                }
            }),
        );
        for field in FIELDS {
            let input = inputs.get(field).clone();
            subs.push(cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    this.on_input(field, event, window, cx)
                },
            ));
        }
        for (slider, limit) in [(&warn, Limit::Warn), (&crit, Limit::Critical)] {
            subs.push(cx.subscribe_in(
                slider,
                window,
                move |this, _, event: &SliderEvent, window, cx| {
                    let SliderEvent::Change(v) = event;
                    this.set_limit(limit, slider_value(*v).round() as u8, window, cx);
                },
            ));
        }
        subs.push(cx.observe_window_appearance(window, |_, _, cx| cx.notify()));
        // Focus visuals only after keyboard navigation (cleared by a click on the root).
        subs.push(controls::track_focus_visible(cx));
        // Pending debounced edits are sent when the window goes away (no timer outlives it).
        subs.push(cx.on_release(|this: &mut Self, cx: &mut App| {
            if this.sync.has_pending() {
                this.model
                    .read(cx)
                    .send(Command::SaveConfig(Box::new(this.edited.clone())));
            }
        }));

        let toaster =
            cx.new(|_| Toaster::new(Theme::resolve(edited.settings.theme, window.appearance())));
        toast::set_host(&toaster, cx);
        {
            let id = toaster.entity_id();
            subs.push(cx.on_release(move |_: &mut Self, cx: &mut App| toast::clear_host(id, cx)));
        }

        let root_focus = cx.focus_handle();
        window.focus(&root_focus);
        let sync = Sync::new(edited.clone());
        Self {
            model,
            page,
            step: 0,
            edited,
            sync,
            save_task: None,
            secret_purge: None,
            toaster,
            saved_toast: None,
            save_label: None,
            analytics_toast: None,
            confirm_remove: None,
            confirm_cancel: cx.focus_handle(),
            confirm_ok: cx.focus_handle(),
            root_focus,
            nav_focus: NAV.iter().map(|_| cx.focus_handle()).collect(),
            scroll: ScrollHandle::new(),
            monitors: codenotch_ui_kit::window::monitors(),
            inputs,
            plan_inputs: BTreeMap::new(),
            plan_placeholders: HashMap::new(),
            warn,
            crit,
            warn_focus: cx.focus_handle(),
            crit_focus: cx.focus_handle(),
            errors: HashMap::new(),
            plan_errors: HashMap::new(),
            renaming: None,
            adding_custom: false,
            analytics_requested: None,
            stats: Default::default(),
            component_dark: None,
            focused_input: None,
            dev: DevUi::from_env(),
            reorder: None,
            drag_scroll: 0.0,
            drag_scroll_task: None,
            toggle_focus: cx.focus_handle(),
            narrow: false,
            narrow_expanded: false,
            sidebar_was_collapsed: None,
            sidebar_anim: None,
            sidebar_w: 0.0,
            subs,
        }
    }

    /// Padding of the page body (tighter in a narrow window).
    pub fn content_pad(&self) -> f32 {
        if self.narrow {
            SETTINGS_CONTENT_PAD_NARROW
        } else {
            SETTINGS_CONTENT_PAD
        }
    }

    /// Width of the page's content column for the current window and sidebar: the inset panel
    /// minus its border and the body padding, at most `max_w`.
    pub fn content_width(&self, window: &Window, max_w: f32) -> f32 {
        let left = if self.page == Page::Onboarding {
            SETTINGS_INSET_MARGIN
        } else {
            self.sidebar_w
        };
        let w = f32::from(window.viewport_size().width)
            - left
            - SETTINGS_INSET_MARGIN
            - 2.0
            - 2.0 * self.content_pad();
        w.clamp(300.0, max_w)
    }

    /// Collapse / expand the sidebar (toggle button, Ctrl+B). In a wide window this flips the
    /// saved preference (no toast); in a narrow one it only expands it for this session.
    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.page == Page::Onboarding {
            return;
        }
        if let Some(forced) = self.dev.sidebar {
            // Dev capture state: flip it, save nothing.
            self.dev.sidebar = Some(!forced);
            cx.notify();
            return;
        }
        if self.narrow {
            self.narrow_expanded = !self.narrow_expanded;
            cx.notify();
            return;
        }
        let next = !self.edited.settings.settings_sidebar_collapsed;
        let before = self.edited.clone();
        if logic::apply(&mut self.edited, &Edit::SidebarCollapsed(next)).is_ok()
            && self.edited != before
        {
            self.send_with_toast(false, cx);
        }
        cx.notify();
    }

    /// Update the narrow flag, the collapsed state and the animated sidebar width for this
    /// frame (before the page body, whose layout depends on them).
    fn update_sidebar(&mut self, window: &mut Window) {
        self.narrow = f32::from(window.viewport_size().width) < SETTINGS_AUTO_COLLAPSE_W;
        if !self.narrow {
            self.narrow_expanded = false;
        }
        if self.page == Page::Onboarding {
            self.sidebar_w = 0.0;
            self.sidebar_anim = None;
            self.sidebar_was_collapsed = None;
            return;
        }
        let collapsed = self.dev.sidebar.unwrap_or_else(|| {
            sidebar_collapsed(
                self.edited.settings.settings_sidebar_collapsed,
                self.narrow,
                self.narrow_expanded,
            )
        });
        let target = if collapsed {
            SIDEBAR_RAIL_OUTER_W
        } else {
            SIDEBAR_OPEN_W
        };
        let changed = self.sidebar_was_collapsed.is_some_and(|c| c != collapsed);
        self.sidebar_was_collapsed = Some(collapsed);
        if changed {
            self.sidebar_anim = (!codenotch_ui_kit::window::reduced_motion())
                .then(|| (Instant::now(), self.sidebar_w, target));
        }
        self.sidebar_w = match self.sidebar_anim {
            Some((start, from, to)) => {
                let t = start.elapsed().as_secs_f32() * 1000.0 / SETTINGS_SIDEBAR_ANIM_MS as f32;
                if t >= 1.0 || to != target {
                    self.sidebar_anim = None;
                    target
                } else {
                    window.request_animation_frame();
                    from + (to - from) * ease_out(t)
                }
            }
            None => target,
        };
    }

    pub fn lang(&self) -> Lang {
        self.edited.settings.lang()
    }

    /// Switch page (sidebar, onboarding end, `open_onboarding` on an open window).
    pub fn go(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        // A page opens with no text field focused (a field focused on the page left behind
        // would otherwise still hold the focus — and draw its ring — when it shows again).
        self.release_input_focus(window, cx);
        if self.page != page {
            self.flush(cx);
            self.page = page;
            self.scroll = ScrollHandle::new();
            if page == Page::Appearance {
                self.monitors = codenotch_ui_kit::window::monitors();
            }
            crate::remember_page(page, cx);
            cx.notify();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Editing model
    // -----------------------------------------------------------------------------------------

    /// Apply an edit and save at once.
    pub fn commit(&mut self, edit: Edit, cx: &mut Context<Self>) {
        let before = self.edited.clone();
        match logic::apply(&mut self.edited, &edit) {
            Ok(()) if self.edited != before || self.sync.has_pending() => self.send_now(cx),
            Ok(()) => {}
            Err(refusal) => {
                let lang = self.lang();
                self.toast_error(t(lang, refusal.message()).to_string(), cx);
            }
        }
        cx.notify();
    }

    /// Apply an edit now and save after the debounce (text fields, sliders).
    pub fn stage(&mut self, edit: Edit, cx: &mut Context<Self>) {
        let before = self.edited.clone();
        if logic::apply(&mut self.edited, &edit).is_err() || self.edited == before {
            return;
        }
        self.sync.record_pending(edit);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| this.send_now(cx));
        }));
        cx.notify();
    }

    /// Send any debounced edit now.
    pub fn flush(&mut self, cx: &mut Context<Self>) {
        if self.sync.has_pending() {
            self.send_now(cx);
        }
    }

    fn send_now(&mut self, cx: &mut Context<Self>) {
        self.send_with_toast(true, cx);
    }

    /// Send the edited config; `toast`: confirm with "Saved" (not for window chrome state).
    fn send_with_toast(&mut self, toast: bool, cx: &mut Context<Self>) {
        self.save_task = None;
        self.sync.record_sent(self.edited.clone());
        self.model
            .read(cx)
            .send(Command::SaveConfig(Box::new(self.edited.clone())));
        let label = self.save_label.take();
        if toast {
            let lang = self.lang();
            self.toast_saved(t(lang, label.unwrap_or("Saved")).to_string(), cx);
        }
    }

    /// Apply an edit and save at once, confirming with `label` instead of "Saved".
    pub fn commit_with_label(&mut self, edit: Edit, label: &'static str, cx: &mut Context<Self>) {
        self.save_label = Some(label);
        self.commit(edit, cx);
        self.save_label = None;
    }

    pub fn send(&self, command: Command, cx: &App) {
        self.model.read(cx).send(command);
    }

    /// Success toast.
    pub fn toast_success(&mut self, text: String, cx: &mut Context<Self>) -> ToastId {
        self.toaster.update(cx, |t, cx| t.success(text, cx))
    }

    /// Error toast.
    pub fn toast_error(&mut self, text: String, cx: &mut Context<Self>) -> ToastId {
        self.toaster.update(cx, |t, cx| t.error(text, cx))
    }

    /// Any toast (action button, custom duration).
    pub fn toast(&mut self, options: ToastOptions, cx: &mut Context<Self>) -> ToastId {
        self.toaster.update(cx, |t, cx| t.show(options, cx))
    }

    /// Save confirmation: the one still shown is updated and restarted, never stacked.
    fn toast_saved(&mut self, text: String, cx: &mut Context<Self>) {
        let toaster = self.toaster.clone();
        let reused = self.saved_toast.is_some_and(|id| {
            toaster.update(cx, |t, cx| {
                t.is_shown(id) && t.update(id, ToastKind::Success, text.clone(), cx)
            })
        });
        if !reused {
            self.saved_toast = Some(self.toast_success(text, cx));
        }
    }

    /// An engine notice ("Test message sent", a sign-in failure …) as a toast of its kind.
    pub fn notice(&mut self, text: &str, cx: &mut Context<Self>) {
        let lang = self.lang();
        let text = t(lang, text).to_string();
        let kind = match logic::notice_kind(&text) {
            logic::NoticeKind::Success => ToastKind::Success,
            logic::NoticeKind::Info => ToastKind::Blank,
            logic::NoticeKind::Error => ToastKind::Error,
        };
        self.toast(ToastOptions::new(kind, text), cx);
    }

    /// Dev capture (`CODENOTCH_DEV_UI=toasts=demo`): one toast of each main kind, sticky.
    fn demo_toasts(&mut self, mode: &str, cx: &mut Context<Self>) {
        use codenotch_ui_kit::toast::ToastPosition;
        let lang = self.lang();
        if mode == "demo-br" {
            self.toaster
                .update(cx, |t, cx| t.set_position(ToastPosition::BottomRight, cx));
        }
        let saved = t(lang, "Saved").to_string();
        let undo = t(lang, "Undo").to_string();
        self.toast(
            ToastOptions::new(ToastKind::Success, saved)
                .action(undo, |_, _| log::debug!("demo toast action"))
                .sticky(),
            cx,
        );
        let loading = t(lang, "Refreshing statistics…").to_string();
        self.toast(ToastOptions::new(ToastKind::Loading, loading), cx);
        let failed = crate::i18n::tf(
            lang,
            "Analytics could not be computed: {msg}",
            &[("msg", "transcripts folder not found")],
        );
        self.toast(ToastOptions::new(ToastKind::Error, failed).sticky(), cx);
    }

    // -----------------------------------------------------------------------------------------
    // Remove account (alert dialog, undo toast)
    // -----------------------------------------------------------------------------------------

    /// Ask before removing account `id` (focus on Cancel).
    pub fn open_remove_confirm(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.edited.account(&id).is_none() {
            return;
        }
        self.confirm_remove = Some(id);
        window.focus(&self.confirm_cancel);
        cx.notify();
    }

    pub fn cancel_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_remove.take().is_some() {
            window.focus(&self.root_focus);
            cx.notify();
        }
    }

    /// Remove the account in the dialog, then offer an undo for 6 s. A custom endpoint's
    /// token is deleted only once the undo is no longer possible.
    pub fn confirm_remove_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.confirm_remove.take() else {
            return;
        };
        window.focus(&self.root_focus);
        let Some(removal) = logic::removal_of(&self.edited, &id) else {
            cx.notify();
            return;
        };
        let lang = self.lang();
        if let Err(refusal) = logic::apply(&mut self.edited, &Edit::RemoveAccount(id.clone())) {
            self.toast_error(t(lang, refusal.message()).to_string(), cx);
            cx.notify();
            return;
        }
        self.send_with_toast(false, cx);
        let view = cx.entity().downgrade();
        let undo = removal.clone();
        self.toast(
            ToastOptions::new(ToastKind::Success, t(lang, "Account removed").to_string())
                .duration(REMOVE_UNDO_MS)
                .action(t(lang, "Undo").to_string(), move |_, cx| {
                    let undo = undo.clone();
                    let _ = view.update(cx, |this, cx| {
                        this.commit_with_label(
                            Edit::RestoreAccount(Box::new(undo)),
                            "Account restored",
                            cx,
                        )
                    });
                }),
            cx,
        );
        if removal.account.uses_custom_endpoint() && removal.account.has_secret {
            self.secret_purge = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(REMOVE_UNDO_MS + 1500))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.edited.account(&id).is_none() {
                        this.clear_secret(SecretKey::Account(id.clone()), cx);
                    }
                });
            }));
        }
        cx.notify();
    }

    /// Keys while the dialog is open (capture phase, before any button): Escape cancels,
    /// Enter confirms; nothing else reaches the page behind.
    fn on_key_capture(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_remove.is_none() || event.keystroke.modifiers.modified() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.cancel_remove(window, cx),
            "enter" => self.confirm_remove_now(window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    /// shadcn AlertDialog: dimmed backdrop over the whole window, centred card with title,
    /// description and Cancel (outline) / Remove (destructive).
    fn render_remove_dialog(&self, t_: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.confirm_remove.clone()?;
        let account = self.edited.account(&id)?;
        let lang = self.lang();
        let name = account.name.clone();
        let detected = !account.uses_custom_endpoint();
        let backdrop = if t_.dark { 0.6 } else { 0.5 };
        let cancel = self.on_view(cx, |this, window, cx| this.cancel_remove(window, cx));
        let confirm = self.on_view(cx, |this, window, cx| this.confirm_remove_now(window, cx));
        let backdrop_click = self.on_view(cx, |this, window, cx| this.cancel_remove(window, cx));
        let mut text = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(18.))
                    .line_height(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t_.foreground)
                    .child(crate::i18n::tf(lang, "Remove “{name}”?", &[("name", &name)])),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(t_.muted_foreground)
                    .child(crate::i18n::tf(
                        lang,
                        "{name} is removed from CodeNotch only. Its sign-in, its files and its profile folder stay on this PC, untouched by CodeNotch.",
                        &[("name", &name)],
                    )),
            );
        if detected {
            text = text.child(
                div()
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(t_.muted_foreground)
                    .child(t(lang, "A detected account is not added back automatically: find it under Removed accounts to add it again.").to_string()),
            );
        }
        let card = div()
            .id("remove-dialog")
            .w(px(460.))
            .max_w_full()
            .mx(px(16.))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .rounded(px(widgets::RADIUS + 2.))
            .bg(t_.background)
            .border_1()
            .border_color(t_.border)
            .shadow_lg()
            // Clicks inside the card never reach the backdrop.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(text)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        widgets::Btn::new("remove-cancel", t(lang, "Cancel").to_string(), *t_)
                            .focus(self.confirm_cancel.clone())
                            .on_click(cancel),
                    )
                    .child(
                        widgets::Btn::new("remove-ok", t(lang, "Remove").to_string(), *t_)
                            .kind(widgets::BtnKind::Destructive)
                            .focus(self.confirm_ok.clone())
                            .on_click(confirm),
                    ),
            );
        Some(
            div()
                .id("remove-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .bg(gpui::hsla(0., 0., 0., backdrop))
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    backdrop_click(window, cx)
                })
                .child(card)
                .into_any_element(),
        )
    }

    /// Analytics refresh asked: loading toast until a new summary arrives.
    pub fn start_analytics_toast(&mut self, generated_at: i64, cx: &mut Context<Self>) {
        let lang = self.lang();
        let text = t(lang, "Refreshing statistics…").to_string();
        let toaster = self.toaster.clone();
        let reused = self.analytics_toast.and_then(|(id, _)| {
            toaster
                .update(cx, |t, cx| {
                    t.is_shown(id) && t.update(id, ToastKind::Loading, text.clone(), cx)
                })
                .then_some(id)
        });
        let id = match reused {
            Some(id) => id,
            None => self.toast(ToastOptions::new(ToastKind::Loading, text), cx),
        };
        self.analytics_toast = Some((id, generated_at));
    }

    /// A snapshot arrived: settle the analytics toast when its summary changed.
    fn settle_analytics_toast(&mut self, cx: &mut Context<Self>) {
        let Some((id, requested)) = self.analytics_toast else {
            return;
        };
        let lang = self.lang();
        let summary = self.model.read(cx).snapshot.analytics.clone();
        let Some(outcome) = logic::analytics_outcome(summary.as_ref(), requested) else {
            return;
        };
        let (kind, text) = match outcome {
            Ok(key) => (ToastKind::Success, t(lang, key).to_string()),
            Err(msg) => (
                ToastKind::Error,
                crate::i18n::tf(
                    lang,
                    "Analytics could not be computed: {msg}",
                    &[("msg", &msg)],
                ),
            ),
        };
        self.analytics_toast = None;
        self.toaster.update(cx, |t, cx| {
            if !t.update(id, kind.clone(), text.clone(), cx) {
                t.show(ToastOptions::new(kind, text), cx);
            }
        });
    }

    /// New snapshot from the engine: re-sync the edited copy and the idle fields.
    fn on_snapshot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settle_analytics_toast(cx);
        let incoming = self.model.read(cx).snapshot.config.clone();
        if self.sync.on_snapshot(&mut self.edited, &incoming) {
            self.sync_fields(window, cx);
        }
        cx.notify();
    }

    /// Move the focus off any text field (to the content pane, which draws nothing). Used when
    /// a page opens, when the window is brought back and after Enter commits a field.
    pub fn release_input_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find_focused_input(window, cx).is_some() {
            window.focus(&self.root_focus);
            self.focused_input = None;
            cx.notify();
        }
    }

    fn focused(&self, input: &Entity<InputState>, window: &Window, cx: &App) -> bool {
        input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The text field that has focus, if any (fixed fields and plan prices).
    fn find_focused_input(&self, window: &Window, cx: &App) -> Option<EntityId> {
        FIELDS
            .iter()
            .map(|f| self.inputs.get(*f))
            .chain(self.plan_inputs.values())
            .find(|input| self.focused(input, window, cx))
            .map(|input| input.entity_id())
    }

    fn set_if_idle(
        &self,
        input: &Entity<InputState>,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.focused(input, window, cx) || input.read(cx).value().as_ref() == value {
            return false;
        }
        input.update(cx, |state, cx| state.set_value(value, window, cx));
        true
    }

    /// Copy config values into fields the user is not typing in.
    pub fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lang = self.lang();
        let s = self.edited.settings.clone();
        if self.set_if_idle(
            &self.inputs.chat_id.clone(),
            s.telegram_chat_id.as_str().to_string(),
            window,
            cx,
        ) {
            self.errors.remove(&Field::ChatId);
        }
        if self.set_if_idle(
            &self.inputs.hotkey.clone(),
            s.hotkey.clone().unwrap_or_default(),
            window,
            cx,
        ) {
            self.errors.remove(&Field::Hotkey);
        }
        if self.set_if_idle(
            &self.inputs.update_url.clone(),
            s.update_manifest_url.clone(),
            window,
            cx,
        ) {
            self.errors.remove(&Field::UpdateUrl);
        }
        let rate_text = self.inputs.rate.read(cx).value().to_string();
        if logic::parse_rate(&rate_text).ok() != Some(s.usd_to_eur)
            && self.set_if_idle(
                &self.inputs.rate.clone(),
                logic::format_number(lang, s.usd_to_eur),
                window,
                cx,
            )
        {
            self.errors.remove(&Field::Rate);
        }
        let plans: Vec<(String, Entity<InputState>)> = self
            .plan_inputs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, input) in plans {
            let price = logic::plan_price(&s, &key);
            let current = logic::parse_price(&input.read(cx).value()).ok().flatten();
            if current != price {
                let text = price
                    .map(|p| logic::format_number(lang, p))
                    .unwrap_or_default();
                if self.set_if_idle(&input, text, window, cx) {
                    self.plan_errors.remove(&key);
                }
            }
        }
        if !self.sync.pending_thresholds() {
            let (w, c) = (f32::from(s.alert_percent), f32::from(s.critical_percent));
            self.warn.update(cx, |st, cx| st.set_value(w, window, cx));
            self.crit.update(cx, |st, cx| st.set_value(c, window, cx));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Fields
    // -----------------------------------------------------------------------------------------

    fn value(&self, field: Field, cx: &App) -> String {
        self.inputs.get(field).read(cx).value().to_string()
    }

    fn set_error(&mut self, field: Field, result: Result<(), FieldError>) {
        match result {
            Ok(()) => {
                self.errors.remove(&field);
            }
            Err(e) => {
                self.errors.insert(field, e.message());
            }
        }
    }

    fn on_input(
        &mut self,
        field: Field,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = self.value(field, cx);
                match field {
                    Field::ChatId => match logic::validate_chat_id(&text) {
                        Ok(id) => {
                            self.set_error(field, Ok(()));
                            self.stage(Edit::TelegramChatId(id), cx);
                        }
                        Err(e) => self.set_error(field, Err(e)),
                    },
                    Field::Hotkey => match logic::normalize_hotkey(&text) {
                        Ok(key) => {
                            self.set_error(field, Ok(()));
                            self.stage(Edit::Hotkey(key), cx);
                        }
                        Err(e) => self.set_error(field, Err(e)),
                    },
                    Field::Rate => match logic::parse_rate(&text) {
                        Ok(v) => {
                            self.set_error(field, Ok(()));
                            self.stage(Edit::UsdToEur(v), cx);
                        }
                        Err(e) => self.set_error(field, Err(e)),
                    },
                    Field::UpdateUrl => match logic::validate_update_url(&text) {
                        Ok(url) => {
                            self.set_error(field, Ok(()));
                            self.stage(Edit::UpdateManifestUrl(url), cx);
                        }
                        Err(e) => self.set_error(field, Err(e)),
                    },
                    _ => {
                        self.errors.remove(&field);
                    }
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => match field {
                Field::TelegramToken => self.save_secret(SecretKey::TelegramToken, window, cx),
                Field::DiscordWebhook => self.save_secret(SecretKey::DiscordWebhook, window, cx),
                Field::Rename => self.commit_rename(window, cx),
                Field::CustomName | Field::CustomUrl | Field::CustomToken => {
                    self.add_custom(window, cx)
                }
                Field::Rate => {
                    self.flush(cx);
                    self.release_input_focus(window, cx);
                }
                _ => self.flush(cx),
            },
            InputEvent::Blur => {
                self.flush(cx);
                // Show the canonical form once the user leaves the field.
                if field == Field::Hotkey && !self.errors.contains_key(&Field::Hotkey) {
                    let canonical = self.edited.settings.hotkey.clone().unwrap_or_default();
                    self.set_if_idle(&self.inputs.hotkey.clone(), canonical, window, cx);
                }
            }
            InputEvent::Focus => {}
        }
    }

    /// Store a secret typed in a field, then clear the field. The value is never kept,
    /// logged or shown again.
    pub fn save_secret(&mut self, key: SecretKey, window: &mut Window, cx: &mut Context<Self>) {
        let field = match key {
            SecretKey::TelegramToken => Field::TelegramToken,
            SecretKey::DiscordWebhook => Field::DiscordWebhook,
            SecretKey::Account(_) => return,
        };
        let value = self.value(field, cx);
        if value.trim().is_empty() {
            return;
        }
        let check = match field {
            Field::TelegramToken => logic::validate_telegram_token(&value),
            _ => logic::validate_discord_webhook(&value),
        };
        self.set_error(field, check);
        if check.is_err() {
            cx.notify();
            return;
        }
        self.send(
            Command::SetSecret {
                key,
                value: SecretValue::new(value.trim()),
            },
            cx,
        );
        self.inputs
            .get(field)
            .update(cx, |s, cx| s.set_value("", window, cx));
        let lang = self.lang();
        self.toast_saved(t(lang, "Saved").to_string(), cx);
    }

    pub fn clear_secret(&mut self, key: SecretKey, cx: &mut Context<Self>) {
        self.send(Command::ClearSecret(key), cx);
        cx.notify();
    }

    pub fn start_rename(
        &mut self,
        id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming = Some(id);
        self.errors.remove(&Field::Rename);
        let input = self.inputs.rename.clone();
        input.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    pub fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.renaming.clone() else {
            return;
        };
        match logic::validate_name(&self.value(Field::Rename, cx)) {
            Ok(name) => {
                self.renaming = None;
                self.commit(Edit::RenameAccount { id, name }, cx);
                window.focus(&self.root_focus);
            }
            Err(e) => self.set_error(Field::Rename, Err(e)),
        }
        cx.notify();
    }

    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    /// Validate the custom endpoint form; add the account, store the token, clear the form.
    pub fn add_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = logic::validate_name(&self.value(Field::CustomName, cx));
        let url = logic::validate_endpoint(&self.value(Field::CustomUrl, cx));
        self.set_error(Field::CustomName, name.as_ref().map(|_| ()).map_err(|e| *e));
        self.set_error(Field::CustomUrl, url.as_ref().map(|_| ()).map_err(|e| *e));
        let (Ok(name), Ok(url)) = (name, url) else {
            cx.notify();
            return;
        };
        let token = self.value(Field::CustomToken, cx).trim().to_string();
        let account = logic::new_custom_account(&name, &url, !token.is_empty());
        let id = account.id.clone();
        self.commit(Edit::AddAccount(Box::new(account)), cx);
        if !token.is_empty() {
            self.send(
                Command::SetSecret {
                    key: SecretKey::Account(id),
                    value: SecretValue::new(token),
                },
                cx,
            );
        }
        for field in [Field::CustomName, Field::CustomUrl, Field::CustomToken] {
            self.inputs
                .get(field)
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        self.adding_custom = false;
        window.focus(&self.root_focus);
        cx.notify();
    }

    pub fn cancel_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [Field::CustomName, Field::CustomUrl, Field::CustomToken] {
            self.inputs
                .get(field)
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.errors.remove(&field);
        }
        self.adding_custom = false;
        cx.notify();
    }

    /// Text field per plan key (created on first use, kept for the window's lifetime). Its
    /// placeholder (the list price, "—" without one) follows the rate.
    pub fn ensure_plan_input(
        &mut self,
        key: &str,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = self.plan_inputs.get(key).cloned() {
            if self.plan_placeholders.get(key).map(String::as_str) != Some(placeholder) {
                self.plan_placeholders
                    .insert(key.to_string(), placeholder.to_string());
                let text = SharedString::from(placeholder.to_string());
                input.update(cx, |s, cx| s.set_placeholder(text, window, cx));
            }
            return input;
        }
        let lang = self.lang();
        let value = logic::plan_price(&self.edited.settings, key)
            .map(|p| logic::format_number(lang, p))
            .unwrap_or_default();
        let text = SharedString::from(placeholder.to_string());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(text)
                .default_value(value)
        });
        self.plan_placeholders
            .insert(key.to_string(), placeholder.to_string());
        let k = key.to_string();
        self.subs.push(cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    match logic::parse_price(&text) {
                        Ok(eur) => {
                            this.plan_errors.remove(&k);
                            this.stage(
                                Edit::PlanPrice {
                                    key: k.clone(),
                                    eur,
                                },
                                cx,
                            );
                        }
                        Err(e) => {
                            this.plan_errors.insert(k.clone(), e.message());
                        }
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    // Enter commits and leaves the field (no ring left behind).
                    this.flush(cx);
                    this.release_input_focus(window, cx);
                }
                InputEvent::Blur => this.flush(cx),
                InputEvent::Focus => {}
            },
        ));
        self.plan_inputs.insert(key.to_string(), input.clone());
        #[cfg(debug_assertions)]
        if std::mem::take(&mut self.dev.focus_plan) {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        input
    }

    // -----------------------------------------------------------------------------------------
    // Thresholds
    // -----------------------------------------------------------------------------------------

    pub fn set_limit(
        &mut self,
        limit: Limit,
        value: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = match limit {
            Limit::Warn => Edit::WarnPercent(value),
            Limit::Critical => Edit::CriticalPercent(value),
        };
        self.stage(edit, cx);
        // The other slider may have been pushed (warn < critical).
        let (w, c) = (
            f32::from(self.edited.settings.alert_percent),
            f32::from(self.edited.settings.critical_percent),
        );
        self.warn.update(cx, |st, cx| st.set_value(w, window, cx));
        self.crit.update(cx, |st, cx| st.set_value(c, window, cx));
    }

    pub fn reset_limits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(Edit::ResetLimits, cx);
        let (w, c) = (
            f32::from(self.edited.settings.alert_percent),
            f32::from(self.edited.settings.critical_percent),
        );
        self.warn.update(cx, |st, cx| st.set_value(w, window, cx));
        self.crit.update(cx, |st, cx| st.set_value(c, window, cx));
    }

    /// Arrow keys on a focused slider (gpui-component's slider takes no keyboard input).
    pub fn on_slider_key(
        &mut self,
        limit: Limit,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let s = &self.edited.settings;
        let current = i32::from(match limit {
            Limit::Warn => s.alert_percent,
            Limit::Critical => s.critical_percent,
        });
        let next = match event.keystroke.key.as_str() {
            "left" | "down" => current - 1,
            "right" | "up" => current + 1,
            "pagedown" => current - 10,
            "pageup" => current + 10,
            "home" => 0,
            "end" => 100,
            _ => return,
        };
        cx.stop_propagation();
        self.set_limit(limit, next.clamp(1, 100) as u8, window, cx);
    }

    // -----------------------------------------------------------------------------------------
    // Closures for controls
    // -----------------------------------------------------------------------------------------

    /// `Fn(value)` that commits `make(value)`.
    pub fn on_edit<T: 'static>(
        &self,
        cx: &Context<Self>,
        make: impl Fn(T) -> Edit + 'static,
    ) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let view = cx.entity().downgrade();
        move |v, _, cx| {
            let _ = view.update(cx, |this, cx| this.commit(make(v), cx));
        }
    }

    /// `Fn()` that sends a command.
    pub fn on_command(&self, command: Command) -> impl Fn(&mut Window, &mut App) + 'static {
        let model = self.model.clone();
        move |_, cx| model.read(cx).send(command.clone())
    }

    /// `Fn()` that runs `f` on the view.
    pub fn on_view(
        &self,
        cx: &Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let view = cx.entity().downgrade();
        move |window, cx| {
            let _ = view.update(cx, |this, cx| f(this, window, cx));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------------------------

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if event.keystroke.key == "b" && m.control && !m.alt && !m.shift && !m.platform {
            // Ctrl+B: collapse / expand the sidebar (shadcn `SIDEBAR_KEYBOARD_SHORTCUT`).
            self.toggle_sidebar(cx);
            cx.stop_propagation();
            return;
        }
        if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
            if self.cancel_drag(window, cx) {
                // Escape during a card drag only cancels the drag.
            } else if self.renaming.is_some() {
                self.cancel_rename(window, cx);
            } else {
                self.close(window, cx);
            }
            cx.stop_propagation();
        }
    }

    fn on_nav_key(
        &mut self,
        index: usize,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = NAV.len() - 1;
        let next = match event.keystroke.key.as_str() {
            "up" => index.saturating_sub(1),
            "down" => (index + 1).min(last),
            "home" => 0,
            "end" => last,
            _ => return,
        };
        cx.stop_propagation();
        self.go(NAV[next], window, cx);
        window.focus(&self.nav_focus[next]);
    }

    // -----------------------------------------------------------------------------------------
    // Chrome
    // -----------------------------------------------------------------------------------------

    /// Navigation sidebar in the shadcn `dashboard-01` look (`variant="inset"`,
    /// `collapsible="icon"`): the window's `sidebar` fill behind the inset panel, no border.
    /// Header: the app identity (logo + "CodeNotch"; a window drag region aligned with the
    /// panel's header bar). Items 32 px, 8 px radius, 8 px padding, 16 px glyph + 14 px label;
    /// hover and the active item use the `sidebar_accent` fill, the active label is medium
    /// weight. Footer: Quit. Collapsed, it is a 48 px icon rail (the glyphs stay in place, the
    /// labels fade out) and each item shows its label in an instant tooltip on its right. The
    /// width is `sidebar_w` (animated by `update_sidebar`). The focus ring is drawn only after
    /// keyboard navigation.
    fn render_sidebar(
        &self,
        t_: &Theme,
        lang: Lang,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let keyboard = controls::focus_visible(cx);
        let progress = sidebar_progress(self.sidebar_w);
        let rail = progress <= 0.001;
        // Labels fade out faster than the width closes (no clipped text under the panel).
        let label_alpha = progress * progress;
        let theme = *t_;
        let nav_item = |id: gpui::ElementId,
                        selected: bool,
                        focused: bool,
                        icon: &'static str,
                        label: String| {
            div()
                .id(id)
                .h(px(32.))
                .px(px(8.))
                .rounded(px(widgets::RADIUS))
                .relative()
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .text_color(t_.foreground)
                .when(selected, |d| {
                    d.bg(t_.sidebar_accent).font_weight(FontWeight::MEDIUM)
                })
                .hover(|s| s.bg(t_.sidebar_accent))
                .when(focused, |d| {
                    d.child(widgets::focus_ring(t_, widgets::RADIUS))
                })
                .child(nav_icon(t_, icon))
                .when(!rail, |d| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(FONT))
                            .opacity(label_alpha)
                            .child(label),
                    )
                })
        };
        let mut nav = div()
            .flex_1()
            .min_h(px(0.))
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.));
        for (i, page) in NAV.iter().copied().enumerate() {
            let selected = self.page == page;
            let focused = keyboard && self.nav_focus[i].is_focused(window);
            let label = t(lang, page_title(page)).to_string();
            let item = nav_item(
                ("nav", i).into(),
                selected,
                focused,
                page_glyph(page),
                label.clone(),
            )
            .track_focus(&self.nav_focus[i].clone().tab_stop(true))
            .on_click(cx.listener(move |this, _, window, cx| this.go(page, window, cx)))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                this.on_nav_key(i, e, window, cx)
            }));
            nav = nav.child(SideTip::new(("nav-tip", i), theme, label, item).disabled(!rail));
        }
        let quit = self.on_command(Command::Quit);
        let quit_label = t(lang, "Quit CodeNotch").to_string();
        let footer = div().flex_none().p(px(8.)).child(
            SideTip::new(
                "nav-quit-tip",
                theme,
                quit_label.clone(),
                nav_item("nav-quit".into(), false, false, glyph::QUIT, quit_label)
                    .on_click(move |_, window, cx| quit(window, cx)),
            )
            .disabled(!rail),
        );
        // Identity: the 32 px logo (centred on the rail like the glyphs) and the name, level
        // with the panel's header bar; it moves the window like a title bar.
        let identity = self
            .drag_region()
            .flex_none()
            .h(px(SETTINGS_INSET_MARGIN + SETTINGS_HEADER_H))
            .pt(px(SETTINGS_INSET_MARGIN))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(app_logo(t_))
            .when(!rail, |d| {
                d.child(
                    div()
                        .min_w(px(0.))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(FONT))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t_.foreground)
                        .opacity(label_alpha)
                        .child("CodeNotch"),
                )
            });
        // The column is as wide as the rail / the expanded sidebar; while collapsed the outer
        // box adds the inset margin before the panel.
        let inner_w = self.sidebar_w - SETTINGS_INSET_MARGIN * (1.0 - progress);
        div()
            .w(px(self.sidebar_w))
            .flex_shrink_0()
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .w(px(inner_w))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(identity)
                    .child(nav)
                    .child(footer),
            )
    }

    /// The sidebar toggle of the header bar (shadcn `SidebarTrigger`): ghost 28 px icon button
    /// with the "panel left" icon; Ctrl+B does the same.
    fn sidebar_toggle(
        &self,
        t_: &Theme,
        lang: Lang,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focused = controls::focus_visible(cx) && self.toggle_focus.is_focused(window);
        let tip = t(lang, "Toggle sidebar (Ctrl+B)").to_string();
        let theme = *t_;
        div()
            .id("sidebar-toggle")
            .relative()
            .flex_none()
            .size(px(28.))
            .rounded(px(widgets::RADIUS))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(t_.accent))
            .track_focus(&self.toggle_focus.clone().tab_stop(true))
            .when(focused, |d| {
                d.child(widgets::focus_ring(t_, widgets::RADIUS))
            })
            .tooltip(move |_, cx| widgets::tip_view(tip.clone(), theme, cx))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx)))
            .child(panel_left_icon(t_))
    }

    /// A part of the window that moves it like a title bar: native `HTCAPTION` hit-test region
    /// (drag, Snap, system menu). A double-click maximises / restores the window here, not
    /// through `DefWindowProc`, so it works whatever the rest of the tree does with the press.
    ///
    /// Nothing between this element and the window root may call `prevent_default` or
    /// `stop_propagation` on a mouse-down, or gpui treats the press as handled and Windows never
    /// starts the move: that is why the root focus handle lives on the page body, not on the
    /// window root or the panel (gpui's focus-on-click listener calls `prevent_default`).
    fn drag_region(&self) -> gpui::Div {
        div()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |e: &MouseDownEvent, window, cx| {
                if is_title_bar_double_click(e.click_count) {
                    window.prevent_default();
                    cx.stop_propagation();
                    codenotch_ui_kit::window::toggle_maximize(window);
                }
            })
    }

    /// One caption button (46 px wide, as tall as the header bar, flush with the panel's
    /// top-right corner; the close button follows its rounded corner).
    ///
    /// It keeps its native hit-test code (`HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE`, so Snap
    /// layouts show on the maximise button) but acts on gpui's own click: gpui 0.2.2 forwards
    /// `WM_NCLBUTTONDOWN/UP` to the element tree first and only runs its native caption action
    /// when no handler marked the press as handled — and the focus-on-click listener of any
    /// focusable ancestor always does (`prevent_default`), so the native path never fired (the
    /// buttons did nothing). The press is marked handled here on purpose, so the native path
    /// can never run as well: exactly one action per click.
    fn caption_button(
        &self,
        t_: &Theme,
        which: Caption,
        maximized: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let close = which == Caption::Close;
        let (hover_bg, hover_fg) = if close {
            (t_.caption_close, gpui::white())
        } else {
            (t_.accent, t_.foreground)
        };
        div()
            .id(which.id())
            .w(px(SETTINGS_CAPTION_W))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(close, |d| d.rounded_tr(px(SETTINGS_INSET_RADIUS - 1.)))
            .text_color(t_.foreground)
            .font_family(t_.fonts.icons)
            .text_size(px(10.))
            .window_control_area(which.area())
            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
            .active(move |s| s.bg(hover_bg.opacity(0.85)).text_color(hover_fg))
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| this.on_caption(which, window, cx)))
            .child(which.glyph(maximized))
    }

    fn on_caption(&mut self, which: Caption, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            Caption::Minimize => window.minimize_window(),
            Caption::Maximize => codenotch_ui_kit::window::toggle_maximize(window),
            Caption::Close => self.close(window, cx),
        }
    }

    /// Close the settings window (close button, Escape). Pending edits are sent first. Only
    /// this window goes away: the notch, the tray and the engine keep running.
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush(cx);
        window.remove_window();
    }

    /// The inset panel's header bar (shadcn `SiteHeader`, 48 px, 1 px bottom border): sidebar
    /// toggle, a 16 px separator and the page title (16 px medium) on the left — the free part
    /// moves the window —, the page's actions and the caption buttons on the right.
    #[allow(clippy::too_many_arguments)]
    fn render_header_bar(
        &self,
        t_: &Theme,
        lang: Lang,
        title: String,
        actions: Option<AnyElement>,
        maximized: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let onboarding = self.page == Page::Onboarding;
        div()
            .h(px(SETTINGS_HEADER_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(t_.border)
            .pl(px(if onboarding { self.content_pad() } else { 12. }))
            .when(!onboarding, |d| {
                d.child(self.sidebar_toggle(t_, lang, window, cx)).child(
                    div()
                        .flex_none()
                        .mx(px(8.))
                        .w(px(1.))
                        .h(px(16.))
                        .bg(t_.border),
                )
            })
            .child(
                self.drag_region()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t_.foreground)
                            .child(title),
                    ),
            )
            .children(actions.map(|a| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .child(a)
            }))
            .child(self.caption_button(t_, Caption::Minimize, maximized, cx))
            .child(self.caption_button(t_, Caption::Maximize, maximized, cx))
            .child(self.caption_button(t_, Caption::Close, maximized, cx))
    }

    /// Buttons of the current page in the header bar (Statistics: Refresh).
    fn header_actions(&self, t_: &Theme, lang: Lang, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.page {
            Page::Analytics => Some(self.stats_refresh_button(t_, lang, cx).into_any_element()),
            _ => None,
        }
    }
}

/// The CodeNotch mark of the sidebar header: a 32 px `primary` tile holding a ring (the notch's
/// usage ring) in `primary_foreground`.
fn app_logo(t_: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .size(px(32.))
        .rounded(px(widgets::RADIUS))
        .bg(t_.primary)
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .size(px(14.))
                .rounded_full()
                .border(px(2.5))
                .border_color(t_.primary_foreground),
        )
}

/// The lucide "panel-left" icon (16 px box): a rounded frame with a divider at a third.
fn panel_left_icon(t_: &Theme) -> gpui::Div {
    div()
        .relative()
        .size(px(14.))
        .rounded(px(3.))
        .border(px(1.5))
        .border_color(t_.foreground)
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(3.))
                .w(px(1.5))
                .bg(t_.foreground),
        )
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lang = self.lang();
        let theme = Theme::resolve(self.edited.settings.theme, window.appearance());
        self.toaster.update(cx, |t, _| t.set_theme(theme));
        if let Some(mode) = self.dev.toasts.take() {
            self.demo_toasts(&mode, cx);
        }
        // (Waits for the accounts: the demo snapshot arrives after the first frame.)
        if let Some(i) = self.dev.confirm_remove {
            if let Some(id) = self.edited.accounts.get(i).map(|a| a.id.clone()) {
                self.dev.confirm_remove = None;
                self.open_remove_confirm(id, window, cx);
            }
        }
        if self.component_dark != Some(theme.dark) {
            self.component_dark = Some(theme.dark);
            controls::apply_component_theme(&theme, Some(window), cx);
        }
        self.focused_input = self.find_focused_input(window, cx);
        // Narrow flag and sidebar width first: the page layout depends on them.
        self.update_sidebar(window);
        let snap = self.model.read(cx).snapshot.clone();
        let onboarding = self.page == Page::Onboarding;
        let title = t(lang, page_title(self.page)).to_string();
        let body = self.render_page(&snap, &theme, window, cx);
        let actions = self.header_actions(&theme, lang, cx);
        let storage_error = snap.storage_error.clone();
        let maximized = window.is_maximized();
        let pad = self.content_pad();

        // Page body: optional error alert and the cards (max 1000 px, the statistics dashboard
        // 1120 px), centred, scrolling under the header bar. It holds the root focus handle
        // (see `drag_region`).
        let max_w = if self.page == Page::Analytics {
            SETTINGS_STATS_MAX_W
        } else {
            SETTINGS_CONTENT_MAX_W
        };
        let pane = div()
            .id("pane")
            .track_focus(&self.root_focus)
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("body")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(px(pad))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(max_w))
                            .mx_auto()
                            // Block layout on purpose: as a flex column clamped by its max
                            // width, taffy stretched the children to the unclamped width.
                            .when_some(storage_error, |d, err| {
                                d.child(widgets::error_strip(&theme, err).mb(px(16.)))
                            })
                            .child(body),
                    ),
            );
        // The inset panel (shadcn `SidebarInset`, variant "inset"): `background` fill, 12 px
        // radius, 1 px border, `shadow-sm` in light; 8 px from the window edges (and from the
        // collapsed rail, see `render_sidebar`).
        let header = self.render_header_bar(&theme, lang, title, actions, maximized, window, cx);
        let panel = div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .pt(px(SETTINGS_INSET_MARGIN))
            .pr(px(SETTINGS_INSET_MARGIN))
            .pb(px(SETTINGS_INSET_MARGIN))
            .when(onboarding, |d| d.pl(px(SETTINGS_INSET_MARGIN)))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .rounded(px(SETTINGS_INSET_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .when(theme.card_shadow, |d| d.shadow_sm())
                    .overflow_hidden()
                    .child(header)
                    .child(pane),
            );

        div()
            .id("settings-root")
            .key_context("CodeNotchSettings")
            .capture_key_down(cx.listener(Self::on_key_capture))
            .on_key_down(cx.listener(Self::on_key))
            .capture_any_mouse_down(|_, window, cx| controls::set_focus_visible(false, window, cx))
            .size_full()
            .relative()
            .flex()
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .text_size(px(FONT))
            .font_family(theme.fonts.text)
            .when(!onboarding, |d| {
                d.child(self.render_sidebar(&theme, lang, window, cx))
            })
            .child(panel)
            // The margin above the panel moves the window too.
            .child(
                self.drag_region()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(SETTINGS_INSET_MARGIN)),
            )
            .children(self.render_remove_dialog(&theme, cx))
            // Toasts over everything (top-centre), the dialog included.
            .child(self.toaster.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_buttons_map_to_their_native_area_and_glyph() {
        assert_eq!(Caption::Minimize.area(), WindowControlArea::Min);
        assert_eq!(Caption::Maximize.area(), WindowControlArea::Max);
        assert_eq!(Caption::Close.area(), WindowControlArea::Close);
        assert_eq!(Caption::Maximize.glyph(false), glyph::MAXIMIZE);
        assert_eq!(Caption::Maximize.glyph(true), glyph::RESTORE);
        assert_eq!(Caption::Close.glyph(true), glyph::CLOSE);
        let ids = [Caption::Minimize, Caption::Maximize, Caption::Close].map(Caption::id);
        assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    }

    #[test]
    fn dev_ui_parses_its_flags() {
        let d = DevUi::parse("focus-plan, heat=12,hour=40,drag=0:3,sidebar=collapsed,bogus");
        assert_eq!(
            d,
            DevUi {
                focus_plan: true,
                heat: Some(12),
                hour: Some(40),
                drag: Some((0, 3)),
                sidebar: Some(true),
                toasts: None,
                confirm_remove: None,
            }
        );
        assert_eq!(DevUi::parse("sidebar=expanded").sidebar, Some(false));
        assert_eq!(DevUi::parse("sidebar=what").sidebar, None);
        assert_eq!(DevUi::parse(""), DevUi::default());
        assert_eq!(DevUi::parse("toasts=demo").toasts.as_deref(), Some("demo"));
        assert_eq!(DevUi::parse("confirm-remove=1").confirm_remove, Some(1));
    }

    #[test]
    fn narrow_windows_collapse_the_sidebar_without_touching_the_preference() {
        // Wide: the saved preference.
        assert!(!sidebar_collapsed(false, false, false));
        assert!(sidebar_collapsed(true, false, false));
        // Narrow: the rail, unless expanded for this session.
        assert!(sidebar_collapsed(false, true, false));
        assert!(!sidebar_collapsed(true, true, true));
    }

    #[test]
    fn sidebar_width_animation_helpers() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
        // Ease-out: ahead of linear in the middle.
        assert!(ease_out(0.5) > 0.5);
        assert_eq!(sidebar_progress(SIDEBAR_OPEN_W), 1.0);
        assert_eq!(sidebar_progress(SIDEBAR_RAIL_OUTER_W), 0.0);
        assert!(
            (sidebar_progress((SIDEBAR_OPEN_W + SIDEBAR_RAIL_OUTER_W) / 2.0) - 0.5).abs() < 1e-6
        );
        // The rail is 48 px plus the inset margin before the panel.
        assert_eq!(SIDEBAR_RAIL_OUTER_W, 56.0);
    }

    #[test]
    fn only_the_second_press_toggles_maximise() {
        assert!(!is_title_bar_double_click(1));
        assert!(is_title_bar_double_click(2));
        assert!(!is_title_bar_double_click(3));
    }
}
