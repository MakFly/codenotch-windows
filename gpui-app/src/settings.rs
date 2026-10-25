//! The settings window, drawn to the Tauri app's settings (`.settings-*`, `.page-heading`,
//! `.accordion-*`, `.setting-row`, `.button` in notch.css). Texts are the Tauri source
//! strings, translated through its own table.
use crate::assets::icon_path;
use crate::model::Settings;
use crate::store::Store;
use crate::ui::{provider_icon, t};
use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, rgb, rgba, size, svg, AnyElement, App,
    Bounds, ClickEvent, Context, Entity, FontWeight, PathBuilder, SharedString,
    TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowHandle, WindowKind,
    WindowOptions,
};
use std::f32::consts::{FRAC_PI_2, TAU};

pub(crate) const SURFACE: u32 = 0x0e0e10;
const SIDEBAR: u32 = 0x161618;
const LINE: u32 = 0xffffff12;
pub(crate) const MUTED: u32 = 0xa3a3a8;
pub(crate) const TEXT: u32 = 0xf5f5f5;
pub(crate) const ACCENT: u32 = 0x5c9bf5;
pub(crate) const DANGER: u32 = 0xf1a1a1;

/// The two parts of the Accounts page, shown one at a time; adding goes through « Ajouter ».
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum AccountsTab {
    Configured,
    Quick,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Section {
    Home,
    Accounts,
    Appearance,
    Alerts,
    About,
}

pub(crate) type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Opens the settings window, or brings the existing one to the front.
pub fn open(store: Entity<Store>, cx: &mut App) {
    if let Some(handle) = store.read(cx).settings_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let french = store.read(cx).french();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(860.), px(690.)),
            cx,
        ))),
        // The header is drawn by the app, as in Tauri; the title still names the window in
        // the taskbar and Alt+Tab.
        titlebar: Some(TitlebarOptions {
            title: Some(title(french).into()),
            appears_transparent: true,
            ..Default::default()
        }),
        focus: true,
        show: true,
        kind: WindowKind::Normal,
        is_resizable: true,
        window_min_size: Some(size(px(640.), px(480.))),
        ..Default::default()
    };
    let handle: Option<WindowHandle<SettingsView>> = cx
        .open_window(options, |window, cx| {
            cx.new(|cx| SettingsView::new(store.clone(), window, cx))
        })
        .ok();
    store.update(cx, |s, _| s.settings_window = handle);
}

fn title(french: bool) -> String {
    t(french, "CodeNotch — Réglages")
}

pub struct SettingsView {
    pub(crate) store: Entity<Store>,
    pub(crate) section: Section,
    /// Result of the last action: (success, text).
    pub(crate) message: Option<(bool, String)>,
    /// The account being added or edited, shown as a dialog.
    pub(crate) editor: Option<crate::account_form::Editor>,
    /// A CLI sign-in in progress.
    pub(crate) login: Option<crate::account_form::Login>,
    pub(crate) channels: crate::channels::Channels,
    /// CLI install and sign-in state per login provider, for quick connect.
    pub(crate) statuses: Vec<crate::login::ProviderStatus>,
    /// Where each CLI of data/cli-agents.json is installed, in catalog order.
    pub(crate) detected: Vec<Option<String>>,
    /// The full CLI list is unfolded.
    pub(crate) show_all_agents: bool,
    /// The open dropdown, by id.
    open_select: Option<&'static str>,
    /// The open accordion item of the Appearance and Alerts pages.
    pub(crate) accounts_tab: AccountsTab,
    /// The start guide of the Home page is unfolded.
    appearance_open: Option<&'static str>,
    alerts_open: Option<&'static str>,
    /// The install command just copied, shown as « Copié » for a moment.
    pub(crate) copied: Option<String>,
    /// The computer's CPU, memory and GPU use, shown in the header while this window is open.
    usage: crate::usage::Usage,
    /// The "Check" button of the updates section is waiting for the release feed.
    checking: bool,
}

impl SettingsView {
    fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A transparent titlebar drops the initial title, which the taskbar and Alt+Tab show.
        window.set_window_title(&title(store.read(cx).french()));
        cx.observe_in(&store, window, |_, store, window, cx| {
            window.set_window_title(&title(store.read(cx).french()));
            cx.notify();
        })
        .detach();
        let (french, chat) = {
            let s = store.read(cx);
            (s.french(), s.config.settings.telegram_chat_id.clone())
        };
        let channels = crate::channels::Channels::new(french, chat, cx);
        // Sample only while this window lives: the loop ends when the view is dropped.
        cx.spawn(async move |this, cx| {
            let mut sampler = crate::usage::Sampler::new();
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let usage = sampler.sample();
                if this
                    .update(cx, |view, cx| {
                        view.usage = usage;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            store,
            section: Section::Home,
            message: None,
            editor: None,
            login: None,
            channels,
            statuses: crate::welcome::statuses(),
            detected: crate::agents::detect(),
            show_all_agents: false,
            open_select: None,
            accounts_tab: AccountsTab::Configured,
            appearance_open: Some("language"),
            alerts_open: Some("discord"),
            usage: crate::usage::Usage::default(),
            copied: None,
            checking: false,
        }
    }

    /// The manual check of the updates section; a found release shows as the banner.
    fn check_update(&mut self, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.message = None;
        cx.notify();
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let result = crate::store::check_update(&store, false, cx).await;
            let _ = this.update(cx, |view, cx| {
                view.checking = false;
                let french = view.store.read(cx).french();
                view.message = match result {
                    Ok(true) => None,
                    Ok(false) => Some((true, t(french, "CodeNotch est à jour."))),
                    Err(error) => Some((false, t(french, &error))),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        self.message = None;
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            // On success the process has already quit for the installer.
            if let Err(error) = crate::store::install_update(&store, cx).await {
                let _ = this.update(cx, |view, cx| {
                    let french = view.store.read(cx).french();
                    view.message = Some((false, t(french, &error)));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// `.update-banner`: a signed release is ready, installed in one click.
    fn update_banner(&mut self, version: &str, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let installing = self.store.read(cx).installing;
        let text = if installing {
            t(french, "Téléchargement et vérification de la signature… CodeNotch redémarrera tout seul.")
        } else {
            t(french, "Version actuelle : {current}. La mise à jour est signée et s’installe en un clic.")
                .replace("{current}", crate::updater::current())
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(12.))
            .mb(px(18.))
            .rounded(px(6.))
            .text_size(px(12.))
            .line_height(px(18.))
            .bg(rgb(0x24303f))
            .text_color(rgb(0xb8cbe6))
            .child(icon("download", 16., 0xb8cbe6))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xffffff))
                            .child(t(french, "CodeNotch {version} est disponible").replace("{version}", version)),
                    )
                    .child(text),
            )
            .child(button(
                "install-update",
                Kind::Primary,
                Some(if installing { "loader-circle" } else { "download" }),
                if installing { t(french, "Mise à jour…") } else { t(french, "Mettre à jour") },
                (!installing).then(|| -> Click {
                    Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.install_update(cx)))
                }),
            ))
            .into_any_element()
    }

    pub(crate) fn say(&mut self, ok: bool, text: String, cx: &mut Context<Self>) {
        self.message = Some((ok, text));
        cx.notify();
    }

    pub(crate) fn run(
        &mut self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Store) -> Result<Option<String>, String>,
    ) {
        let result = self.store.update(cx, |s, cx| {
            let result = action(s);
            cx.notify();
            result
        });
        self.message = match result {
            Ok(Some(text)) => Some((true, text)),
            Ok(None) => None,
            Err(error) => Some((false, error)),
        };
        cx.notify();
    }

    pub(crate) fn update_settings(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.store.read(cx).config.settings.clone();
        change(&mut settings);
        self.run(cx, move |s| s.save_settings(settings).map(|_| None));
    }

    /// A click handler that edits the settings and saves them.
    pub(crate) fn setting(
        &self,
        cx: &mut Context<Self>,
        change: impl Fn(&mut Settings) + 'static,
    ) -> Click {
        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
            this.update_settings(cx, &change)
        }))
    }
}

// ---- Shared controls ----------------------------------------------------------------

pub(crate) fn icon(name: &str, size: f32, color: u32) -> gpui::Svg {
    svg()
        .path(icon_path(name))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// `.button` with its `.primary`, `.secondary`, `.ghost` and `.danger` variants.
pub(crate) fn button(
    id: impl Into<SharedString>,
    kind: Kind,
    leading: Option<&str>,
    label: String,
    on_click: Option<Click>,
) -> AnyElement {
    let (bg, hover, fg, border) = match kind {
        Kind::Primary => (0xe7e7eb, 0xffffff, 0x17171a, None),
        Kind::Secondary => (0x262629, 0x323236, 0xf5f5f5, Some(0x3b3b3f)),
        Kind::Ghost => (0x00000000, 0xffffff0d, 0xf5f5f5, None),
        Kind::Danger => (0x4c2424, 0x5a2a2a, DANGER, None),
    };
    let enabled = on_click.is_some();
    let rgba_of = |c: u32| if c > 0xffffff { rgba(c) } else { rgb(c) };
    let mut el = div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(12.))
        .min_h(px(34.))
        .rounded(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .when(kind != Kind::Ghost, |d| d.bg(rgba_of(bg)))
        .text_color(rgb(fg))
        .when_some(border, |d, b| d.border_1().border_color(rgb(b)))
        .when(kind == Kind::Ghost, |d| d.border_1().border_color(rgba(0x00000000)))
        .when_some(leading, |d, name| d.child(icon(name, 15., fg)))
        .child(label);
    if let Some(on_click) = on_click {
        el = el
            .cursor_pointer()
            .hover(move |d| {
                let d = d.bg(rgba_of(hover));
                if kind == Kind::Ghost { d.border_color(rgb(0x3b3b3f)) } else { d }
            })
            .on_click(on_click);
    }
    el.when(!enabled, |d| d.opacity(0.45)).into_any_element()
}

/// `.text-button`: small blue link-style action.
pub(crate) fn text_button(id: impl Into<SharedString>, leading: &str, label: String, on_click: Click) -> AnyElement {
    div()
        .id(id.into())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .min_h(px(32.))
        .text_size(px(11.))
        .text_color(rgb(0xb8cbe6))
        .cursor_pointer()
        .child(icon(leading, 15., 0xb8cbe6))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// `.toggle`: 32 × 19 switch.
pub(crate) fn toggle(id: impl Into<SharedString>, checked: bool, on_click: Click) -> AnyElement {
    div()
        .id(id.into())
        .w(px(32.))
        .h(px(19.))
        .flex_none()
        .p(px(2.))
        .rounded(px(15.))
        .cursor_pointer()
        .bg(if checked { rgb(0x4c83c6) } else { rgb(0x3d3d42) })
        .flex()
        .when(checked, |d| d.justify_end())
        .child(
            div()
                .size(px(15.))
                .rounded_full()
                .bg(if checked { rgb(0xffffff) } else { rgb(0xd6d6dc) }),
        )
        .on_click(on_click)
        .into_any_element()
}

/// `.setting-row`: title and help on the left, its control on the right.
pub(crate) fn setting_row(title: String, help: Option<String>, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(25.))
        .py(px(17.))
        .border_b_1()
        .border_color(rgba(LINE))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(title))
                .when_some(help, |d, help| {
                    d.child(div().text_size(px(11.)).line_height(px(17.)).text_color(rgb(MUTED)).child(help))
                }),
        )
        .child(control)
        .into_any_element()
}

/// `.list-header`: a muted section title with an action on the right.
pub(crate) fn list_header(title: String, action: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .mb(px(6.))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(0xa8a8b0))
                .child(title),
        )
        .children(action)
        .into_any_element()
}

/// `.message.success` / `.message.error`.
pub(crate) fn message(ok: bool, text: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .p(px(12.))
        .mb(px(18.))
        .rounded(px(6.))
        .text_size(px(12.))
        .line_height(px(18.))
        .bg(if ok { rgb(0x26382c) } else { rgb(0x442b2c) })
        .text_color(if ok { rgb(0xb9d9c0) } else { rgb(0xf2b0ac) })
        .child(icon(if ok { "check" } else { "circle-alert" }, 16., if ok { 0xb9d9c0 } else { 0xf2b0ac }))
        .child(div().flex_1().min_w_0().child(text))
        .into_any_element()
}

/// `.mini-notch-icon`: the small black notch with an open ring, drawn in the header.
fn mini_notch() -> impl IntoElement {
    div()
        .w(px(12.))
        .h(px(17.))
        .flex_none()
        .rounded_l(px(7.))
        .bg(rgb(0x000000))
        .child(
            canvas(|_, _, _| {}, |bounds, _, window, _| {
                let center = point(bounds.origin.x + px(6.), bounds.origin.y + px(6.));
                let mut b = PathBuilder::stroke(px(1.5));
                for i in 0..=36 {
                    // Three quarters of a ring, open on the right like the CSS border.
                    let a = FRAC_PI_2 * 0.5 + (TAU * 0.75) * i as f32 / 36.;
                    let p = point(center.x + px(3. * a.cos()), center.y + px(3. * a.sin()));
                    if i == 0 { b.move_to(p) } else { b.line_to(p) }
                }
                if let Ok(path) = b.build() {
                    window.paint_path(path, rgb(0xe5e5e5));
                }
            })
            .size_full(),
        )
}

impl SettingsView {
    /// `.select-trigger` with its `.select-content` menu.
    pub(crate) fn select(
        &self,
        id: &'static str,
        current: String,
        options: Vec<(String, bool, Click)>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_select == Some(id);
        let view = cx.entity().downgrade();
        let trigger = div()
            .id(SharedString::from(format!("{id}-trigger")))
            .w(px(width))
            .min_h(px(36.))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .px(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(0x414147))
            .bg(rgb(0x252528))
            .text_size(px(12.))
            .cursor_pointer()
            .child(current)
            .child(icon("chevron-down", 16., 0xc3c3c8))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.open_select = if this.open_select == Some(id) { None } else { Some(id) };
                cx.notify();
            }));
        let menu = open.then(|| {
            let items: Vec<AnyElement> = options
                .into_iter()
                .enumerate()
                .map(|(i, (label, selected, on_click))| {
                    let view = view.clone();
                    div()
                        .id(SharedString::from(format!("{id}-{i}")))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .min_h(px(34.))
                        .px(px(10.))
                        .rounded(px(4.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgb(0x3c526d)))
                        .child(label)
                        .when(selected, |d| d.child(icon("check", 14., 0xf5f5f5)))
                        .on_click(move |e, w, cx| {
                            on_click(e, w, cx);
                            let _ = view.update(cx, |v, cx| {
                                v.open_select = None;
                                cx.notify();
                            });
                        })
                        .into_any_element()
                })
                .collect();
            deferred(
                anchored().snap_to_window_with_margin(px(8.)).child(
                    div()
                        .id(SharedString::from(format!("{id}-menu")))
                        .mt(px(4.))
                        .w(px(width))
                        .p(px(4.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgb(0x44444a))
                        .bg(rgb(0x242427))
                        .shadow_lg()
                        .text_color(rgb(TEXT))
                        .children(items)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.open_select = None;
                            cx.notify();
                        })),
                ),
            )
            .with_priority(1)
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(trigger)
            .children(menu)
            .into_any_element()
    }

    /// `.accordion-item`: icon, title, summary and chevron; one item open per page.
    fn accordion(
        &self,
        page: Section,
        key: &'static str,
        leading: &str,
        title: String,
        summary: AnyElement,
        disabled: bool,
        body: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = match page {
            Section::Alerts => self.alerts_open,
            _ => self.appearance_open,
        };
        let open = !disabled && current == Some(key);
        div()
            .flex()
            .flex_col()
            .rounded(px(10.))
            .border_1()
            .border_color(if open { rgba(0xffffff1f) } else { rgba(LINE) })
            .bg(rgba(0xffffff04))
            .overflow_hidden()
            .when(disabled, |d| d.opacity(0.55))
            .child(
                div()
                    .id(SharedString::from(format!("acc-{key}")))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .py(px(13.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .when(!disabled, |d| d.cursor_pointer().hover(|d| d.bg(rgba(0xffffff08))))
                    .child(icon(leading, 18., 0xf5f5f5))
                    .child(div().flex_1().child(title))
                    .child(summary)
                    .child(
                        icon(if open { "chevron-up" } else { "chevron-down" }, 16., 0x8f8f96),
                    )
                    .when(!disabled, |d| {
                        d.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let slot = match page {
                                Section::Alerts => &mut this.alerts_open,
                                _ => &mut this.appearance_open,
                            };
                            *slot = if *slot == Some(key) { None } else { Some(key) };
                            cx.notify();
                        }))
                    }),
            )
            .when(open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(16.))
                        .pb(px(12.))
                        .border_t_1()
                        .border_color(rgba(LINE))
                        .children(body),
                )
            })
            .into_any_element()
    }
}

fn summary(text: String) -> AnyElement {
    div()
        .text_size(px(11.))
        .font_weight(FontWeight::NORMAL)
        .text_color(rgb(0x8f8f96))
        .child(text)
        .into_any_element()
}

/// `.channel-badge`: Active / Configured / Not configured, or Coming soon.
pub(crate) fn channel_badge(label: String, tone: &str) -> AnyElement {
    let (fg, bg, border) = match tone {
        "on" => (0x0f1a12, Some(0x7fd69b), None),
        "ready" => (0xb8cbe6, None, Some(0x3c5370)),
        _ => (0x9a9aa3, None, Some(0x414147)),
    };
    div()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .when_some(bg, |d, bg| d.bg(rgb(bg)))
        .when_some(border, |d, b| d.border_1().border_color(rgb(b)))
        .child(label)
        .into_any_element()
}

/// A sentence with an inline `<code>` part, kept in one flowing paragraph.
fn inline_code(before: &str, code: Option<&str>, after: &str) -> gpui::StyledText {
    let Some(code) = code else {
        return gpui::StyledText::new(format!("{before}{after}"));
    };
    let start = before.len() + 1;
    let joiner = if after.starts_with([',', '.']) { "" } else { " " };
    let text = format!("{before} {code}{joiner}{after}");
    gpui::StyledText::new(text).with_highlights(vec![(
        start..start + code.len(),
        gpui::HighlightStyle {
            color: Some(rgb(0xc8d9ee).into()),
            ..Default::default()
        },
    )])
}

fn interval_label(n: u64, french: bool) -> String {
    if n < 60 {
        t(french, "{count} secondes").replace("{count}", &n.to_string())
    } else if n == 60 {
        t(french, "{count} minute").replace("{count}", "1")
    } else {
        t(french, "{count} minutes").replace("{count}", &(n / 60).to_string())
    }
}

pub(crate) fn position_label(edge: &str) -> &'static str {
    match edge {
        "left" => "À gauche",
        "top" => "En haut",
        "bottom" => "En bas",
        _ => "À droite",
    }
}

// ---- Pages --------------------------------------------------------------------------

impl SettingsView {
    /// shadcn/ui `Tabs`: a muted rounded track, the active trigger raised on it.
    fn accounts_tabs(&self, french: bool, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let tabs = [
            (AccountsTab::Configured, "Comptes configurés", Some(count)),
            (AccountsTab::Quick, "Connexion rapide", None),
        ];
        div()
            .flex()
            .w_full()
            .h(px(36.))
            .p(px(3.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(rgb(0x1c1c1f))
            .border_1()
            .border_color(rgba(LINE))
            .children(tabs.into_iter().map(|(tab, label, badge)| {
                let active = self.accounts_tab == tab;
                div()
                    .id(SharedString::from(format!("accounts-tab-{label}")))
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(6.))
                    .border_1()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(rgb(0x2a2a2e))
                            .border_color(rgba(0xffffff1a))
                            .text_color(rgb(0xffffff))
                            .shadow_sm()
                    })
                    .when(!active, |d| {
                        d.border_color(rgba(0x00000000))
                            .text_color(rgb(0xa1a1aa))
                            .hover(|d| d.text_color(rgb(0xffffff)))
                    })
                    .child(t(french, label))
                    .when_some(badge, |d, n| {
                        d.child(
                            div()
                                .px(px(6.))
                                .rounded_full()
                                .bg(if active { rgba(0xffffff14) } else { rgba(0xffffff0a) })
                                .text_size(px(10.5))
                                .text_color(rgb(0xc3c3c8))
                                .child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.accounts_tab = tab;
                        this.message = None;
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn accounts_page(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let accounts = self.store.read(cx).config.accounts.clone();
        let tab = self.accounts_tab;
        let mut out = vec![self.accounts_tabs(french, accounts.len(), cx)];
        if tab == AccountsTab::Quick {
            // The tab names the section, so the block keeps only its intro and summary.
            out.push(self.quick_connect(french, "", true, true, cx));
            return out;
        }

        let detect = text_button(
            "detect",
            "search",
            t(french, "Détecter"),
            Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.run(cx, |s| {
                    s.add_discovered().map(|_| {
                        Some(t(french, "Recherche terminée. Les profils locaux disponibles ont été ajoutés."))
                    })
                })
            })),
        );
        let mut list: Vec<AnyElement> = vec![list_header(t(french, "Comptes configurés"), Some(detect))];
        if accounts.is_empty() {
            list.push(
                div()
                    .py(px(28.))
                    .px(px(15.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .text_color(rgb(0xaaaaaa))
                    .child(icon("users-round", 28., 0xaaaaaa))
                    .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xdddddd)).child(t(french, "Aucun compte configuré")))
                    .child(div().max_w(px(350.)).text_size(px(12.)).text_center().child(t(
                        french,
                        "Connecte-toi avec « Connexion rapide », ou détecte les profils Codex, Claude, Gemini, Grok et OpenCode déjà connectés sur ce PC.",
                    )))
                    .into_any_element(),
            );
        }
        for account in accounts {
            let id = account.id.clone();
            let enabled = account.enabled;
            let edited = account.clone();
            let toggle_id = id.clone();
            list.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(rgba(LINE))
                    .hover(|d| d.bg(rgba(0xffffff04)))
                    .child(
                        div()
                            .id(SharedString::from(format!("edit-{id}")))
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .flex_1()
                            .min_w_0()
                            .py(px(8.))
                            .cursor_pointer()
                            .child(div().mx(px(3.)).child(provider_icon(&account.provider, 25.)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(account.name.clone()))
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(0x92929b))
                                            .child(t(french, &crate::account_form::name(&account.provider, true))),
                                    ),
                            )
                            .child(icon("chevron-right", 16., 0x777777))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.open_editor(Some(edited.clone()), window, cx)
                            })),
                    )
                    .child(toggle(
                        format!("enabled-{id}"),
                        enabled,
                        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let id = toggle_id.clone();
                            this.run(cx, move |s| s.set_enabled(&id, !enabled).map(|_| None))
                        })),
                    ))
                    .into_any_element(),
            );
        }
        list.push(
            div()
                .flex()
                .items_start()
                .gap(px(9.))
                .pt(px(15.))
                .child(icon("shield-check", 18., 0x777777))
                .child(div().text_size(px(11.)).line_height(px(17.)).text_color(rgb(0x9898a2)).child(t(
                    french,
                    "Les sessions existantes restent dans leur outil d’origine. Les secrets ajoutés ici sont conservés dans le Gestionnaire d’identification Windows.",
                )))
                .into_any_element(),
        );
        out.push(div().flex().flex_col().children(list).into_any_element());
        out
    }

    fn appearance_page(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = self.store.read(cx).config.settings.clone();
        let page = Section::Appearance;

        let language = self.select(
            "language",
            if french { "Français".into() } else { "English".into() },
            [("en", "English"), ("fr", "Français")]
                .into_iter()
                .map(|(code, label)| {
                    (label.to_owned(), (code == "fr") == french, self.setting(cx, move |s| s.language = code.into()))
                })
                .collect(),
            130.,
            cx,
        );
        let replay = button(
            "replay-welcome",
            Kind::Secondary,
            None,
            t(french, "Revoir l’écran d’accueil"),
            Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.load_statuses();
                this.update_settings(cx, |s| s.onboarding_done = Some(false));
            }))),
        );
        let language_item = self.accordion(
            page,
            "language",
            "languages",
            t(french, "Langue et accueil"),
            summary(if french { "Français".into() } else { "English".into() }),
            false,
            vec![
                setting_row(
                    t(french, "Langue de l’interface"),
                    Some(t(french, "S’applique immédiatement et reste enregistrée sur ce PC.")),
                    language,
                ),
                setting_row(
                    t(french, "Écran d’accueil"),
                    Some(t(french, "Reprends la connexion de tes abonnements IA pas à pas.")),
                    replay,
                ),
            ],
            cx,
        );

        let version = env!("CARGO_PKG_VERSION");
        let updates_item = self.accordion(
            page,
            "updates",
            "download",
            t(french, "Mises à jour"),
            summary(format!("v{version}")),
            false,
            vec![setting_row(
                t(french, "Mises à jour"),
                Some(
                    t(french, "Version {version}. CodeNotch vérifie au démarrage puis deux fois par jour ; chaque mise à jour est signée avant d’être installée.")
                        .replace("{version}", version),
                ),
                button(
                    "check-update",
                    Kind::Secondary,
                    Some(if self.checking { "loader-circle" } else { "refresh-cw" }),
                    if self.checking { t(french, "Vérification…") } else { t(french, "Vérifier") },
                    (!self.checking).then(|| -> Click {
                        Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.check_update(cx)))
                    }),
                ),
            )],
            cx,
        );

        let position = self.select(
            "position",
            t(french, position_label(&s.widget_position)),
            ["right", "left", "top", "bottom"]
                .into_iter()
                .map(|edge| {
                    (
                        t(french, position_label(edge)),
                        s.widget_position == edge,
                        self.setting(cx, move |s| s.widget_position = edge.into()),
                    )
                })
                .collect(),
            130.,
            cx,
        );
        let notch_item = self.accordion(
            page,
            "notch",
            "monitor",
            t(french, "Encoche"),
            summary(t(french, position_label(&s.widget_position))),
            false,
            vec![
                setting_row(
                    t(french, "Afficher l’encoche"),
                    Some(t(french, "Les quotas restent visibles au bord de l’écran. Survolez un anneau pour afficher les détails.")),
                    toggle("widget", s.widget, self.setting(cx, |s| s.widget = !s.widget)),
                ),
                setting_row(
                    t(french, "Replier automatiquement"),
                    Some(t(french, "Une languette discrète au repos. Survolez-la pour déplier les comptes ; éloignez la souris pour les masquer.")),
                    toggle("auto-hide", s.auto_hide, self.setting(cx, |s| s.auto_hide = !s.auto_hide)),
                ),
                setting_row(
                    t(french, "Position de départ"),
                    Some(t(french, "La poignée mémorise ta position et ton écran. Choisir un autre bord réinitialise ce placement.")),
                    position,
                ),
                setting_row(
                    t(french, "Masquer en plein écran"),
                    Some(t(french, "Masque l’encoche et ses alertes pendant un jeu ou une vidéo en plein écran, même sans bordure. Retour automatique après Alt+Tab.")),
                    toggle("hide-fullscreen", s.hide_in_fullscreen, self.setting(cx, |s| s.hide_in_fullscreen = !s.hide_in_fullscreen)),
                ),
                setting_row(
                    t(french, "Mode jeu"),
                    Some(t(french, "Masque l’encoche et ses alertes jusqu’à désactivation. Pour les jeux en fenêtre, active-le depuis l’icône près de l’horloge.")),
                    toggle("game-mode", s.game_mode, self.setting(cx, |s| s.game_mode = !s.game_mode)),
                ),
            ],
            cx,
        );

        let poll = self.select(
            "poll",
            interval_label(s.poll_seconds, french),
            [30u64, 60, 120, 300]
                .into_iter()
                .map(|n| (interval_label(n, french), s.poll_seconds == n, self.setting(cx, move |s| s.poll_seconds = n)))
                .collect(),
            130.,
            cx,
        );
        let refresh_item = self.accordion(
            page,
            "refresh",
            "refresh-cw",
            t(french, "Actualisation"),
            summary(interval_label(s.poll_seconds, french)),
            false,
            vec![setting_row(
                t(french, "Fréquence des relevés"),
                Some(t(french, "Claude respecte un minimum de 2 minutes. Les délais imposés par les fournisseurs sont prioritaires.")),
                poll,
            )],
            cx,
        );
        vec![div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(6.))
            .child(language_item)
            .child(updates_item)
            .child(notch_item)
            .child(refresh_item)
            .into_any_element()]
    }

    fn alerts_page(&mut self, french: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let store = self.store.read(cx);
        let s = store.config.settings.clone();
        let (discord_saved, telegram_saved) = (store.discord_saved, store.telegram_saved);
        let page = Section::Alerts;
        let badge = |on: bool, ready: bool| {
            let (label, tone) = if on && ready {
                ("Actif", "on")
            } else if ready {
                ("Configuré", "ready")
            } else {
                ("Non configuré", "off")
            };
            channel_badge(t(french, label), tone)
        };

        let discord_body = self.discord_body(french, cx);
        let discord = self.accordion(
            page,
            "discord",
            "webhook",
            t(french, "Discord"),
            badge(s.discord_enabled, discord_saved),
            false,
            discord_body,
            cx,
        );
        let telegram_available = crate::telegram::AVAILABLE;
        let telegram_body = if telegram_available { self.telegram_body(french, cx) } else { vec![] };
        let telegram = self.accordion(
            page,
            "telegram",
            "send",
            t(french, "Telegram"),
            if telegram_available {
                badge(s.telegram_enabled, telegram_saved)
            } else {
                channel_badge(t(french, "Bientôt disponible"), "off")
            },
            !telegram_available,
            telegram_body,
            cx,
        );

        let percent_select = |this: &Self, cx: &mut Context<Self>, id: &'static str, value: u8, choices: &[u8], set: fn(&mut Settings, u8)| {
            this.select(
                id,
                format!("{value} %"),
                choices
                    .iter()
                    .map(|&n| (format!("{n} %"), value == n, this.setting(cx, move |s| set(s, n))))
                    .collect(),
                130.,
                cx,
            )
        };
        let alert = percent_select(self, cx, "alert-percent", s.alert_percent, &[50, 70, 80, 90, 95, 100], |s, v| s.alert_percent = v);
        let critical = percent_select(self, cx, "critical-percent", s.critical_percent, &[80, 90, 95, 99], |s, v| s.critical_percent = v);
        let thresholds = self.accordion(
            page,
            "thresholds",
            "bell",
            t(french, "Seuils et notifications Windows"),
            summary(format!("{} % · {} %", s.alert_percent, s.critical_percent)),
            false,
            vec![
                setting_row(
                    t(french, "Notifications de quota"),
                    Some(t(french, "Une notification Windows à chaque niveau franchi, sans répétition tant qu’il reste dépassé.")),
                    toggle("notifications", s.notifications, self.setting(cx, |s| s.notifications = !s.notifications)),
                ),
                setting_row(t(french, "Me prévenir à"), Some(t(french, "Pourcentage consommé d’une fenêtre de quota.")), alert),
                setting_row(
                    t(french, "Seuil critique"),
                    Some(t(french, "Une seconde alerte, plus urgente. Le plafond atteint est toujours signalé.")),
                    critical,
                ),
                setting_row(
                    t(french, "Alertes urgentes en plein écran"),
                    Some(t(french, "La prévision, le seuil critique et le plafond s’affichent même pendant un jeu ou en mode jeu.")),
                    toggle("critical-fullscreen", s.critical_in_fullscreen, self.setting(cx, |s| s.critical_in_fullscreen = !s.critical_in_fullscreen)),
                ),
            ],
            cx,
        );

        let minutes = |n: u16| t(french, "{count} minutes").replace("{count}", &n.to_string());
        let lead = self.select(
            "forecast-minutes",
            minutes(s.forecast_minutes),
            [15u16, 30, 60]
                .into_iter()
                .map(|n| (minutes(n), s.forecast_minutes == n, self.setting(cx, move |s| s.forecast_minutes = n)))
                .collect(),
            130.,
            cx,
        );
        let forecast = self.accordion(
            page,
            "forecast",
            "timer",
            t(french, "Prévision du plafond"),
            summary(if s.forecast_alerts { minutes(s.forecast_minutes) } else { t(french, "Désactivée") }),
            false,
            vec![
                setting_row(
                    t(french, "Prévenir avant le plafond"),
                    Some(t(french, "Mesure le rythme de consommation et prévient quand le plafond arrive avant la réinitialisation. Idéal pour les agents qui tournent en fond.")),
                    toggle("forecast", s.forecast_alerts, self.setting(cx, |s| s.forecast_alerts = !s.forecast_alerts)),
                ),
                setting_row(t(french, "Anticipation"), Some(t(french, "Délai avant le plafond estimé à partir duquel l’alerte part.")), lead),
                setting_row(
                    t(french, "Prévenir quand le quota est rétabli"),
                    Some(t(french, "Après une réinitialisation, pour relancer les agents mis en pause.")),
                    toggle("reset", s.reset_alerts, self.setting(cx, |s| s.reset_alerts = !s.reset_alerts)),
                ),
            ],
            cx,
        );
        vec![div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(6.))
            .child(discord)
            .child(telegram)
            .child(thresholds)
            .child(forecast)
            .into_any_element()]
    }

    fn about_page(&mut self, french: bool) -> Vec<AnyElement> {
        let para = |text: String| {
            div().mt(px(9.)).text_size(px(12.)).line_height(px(19.)).text_color(rgb(MUTED)).child(text)
        };
        let section = |first: bool, title: String, body: Vec<AnyElement>| {
            div()
                .py(px(20.))
                .when(first, |d| d.pt_0())
                .when(!first, |d| d.border_t_1().border_color(rgba(LINE)))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title))
                .children(body)
                .into_any_element()
        };
        let provider = |name: &str, before: &str, file: Option<&str>, after: &str| {
            div()
                .mt(px(16.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child(name.to_owned()))
                .child(
                    div()
                        .mt(px(6.))
                        .text_size(px(11.))
                        .line_height(px(18.))
                        .text_color(rgb(MUTED))
                        .child(inline_code(&t(french, before), file, &t(french, after))),
                )
                .into_any_element()
        };
        let sample = format!(
            "{{\n  \"plan\": \"{}\",\n  \"windows\": [\n    {{\n      \"label\": \"{}\",\n      \"usedPercent\": 42,\n      \"resetsAt\": \"2026-11-01T00:00:00Z\"\n    }}\n  ],\n  \"tokensToday\": 12345,\n  \"lifetimeTokens\": 250000,\n  \"daily\": [{{ \"date\": \"2026-10-24\", \"tokens\": 12345 }}]\n}}",
            t(french, "Mon abonnement"),
            t(french, "Quota mensuel")
        );
        vec![div()
            .max_w(px(560.))
            .child(section(true, t(french, "Quota et tokens : deux repères différents."), vec![para(t(french, "Un quota à 40 % signifie que le fournisseur indique 40 % de consommation sur une période donnée. Il ne permet pas de déduire un nombre de tokens restants. Les tokens s’affichent uniquement quand la source communique ce nombre.")).into_any_element()]))
            .child(section(false, t(french, "Connecter tes outils"), vec![
                provider("Codex / ChatGPT", "Clique « Se connecter » dans Comptes → Connexion rapide, ou connecte-toi avec Codex CLI ou l’application Codex. CodeNotch lit les quotas via", Some("codex app-server"), ", ainsi que l’activité en tokens si ta version la prend en charge. Une clé API seule ne donne pas accès aux quotas de l’abonnement ChatGPT. Les jours d’activité sont affichés en UTC."),
                provider("Claude", "Connecte Claude Code sur Windows. Le connecteur lit le fichier", Some(".claude/.credentials.json"), "ou le jeton OAuth configuré. Les sessions expirées sont renouvelées par le CLI Claude Code installé, sans envoyer de prompt. Si le renouvellement échoue, utilise « Se connecter » dans Comptes ou « claude auth login ». L’endpoint de quota est interne et peut changer."),
                provider("Gemini", "Installe Gemini CLI, puis utilise « Se connecter » et choisis « Login with Google ». Le connecteur lit", Some(".gemini/oauth_creds.json"), "et interroge l’endpoint interne de quota Code Assist, qui peut changer. Une clé API Gemini ne donne pas accès à ces quotas. CodeNotch ne renouvelle pas la session lui-même : quand elle expire, relance Gemini CLI."),
                provider("Grok", "Installe Grok Build CLI, puis utilise « Se connecter » (« grok login »). Le connecteur lit", Some(".grok/auth.json"), "et affiche le pourcentage de crédits de la période en cours, via un endpoint interne qui peut changer. Une clé API xAI n’a pas de quota d’abonnement."),
                provider("OpenCode", "Utilise « Se connecter » (« opencode auth login ») et choisis « OpenCode Go ». Le connecteur lit", Some(".local/share/opencode/auth.json"), "ou OPENCODE_API_KEY, et affiche les fenêtres 5 h, 7 jours et mensuelle de l’abonnement Go. Les autres fournisseurs connectés dans OpenCode n’ont pas de quota OpenCode."),
                provider("GitHub Copilot", "Configure un jeton GitHub disposant de l’accès Copilot, ou la variable", Some("GH_TOKEN"), ". Le connecteur utilise l’endpoint interne de quota des éditeurs. Tous les jetons GitHub ne sont pas autorisés à le lire."),
                provider("Ollama", "Le serveur doit être lancé. L’URL par défaut est", Some("http://127.0.0.1:11434/api/ps"), ". Le relevé indique les modèles chargés, pas une consommation de tokens."),
            ]))
            .child(section(false, t(french, "Tout autre fournisseur : un endpoint JSON"), vec![
                para(t(french, "Ajoute « Endpoint personnalisé » pour OpenRouter, un proxy interne ou un autre outil. L’URL doit retourner le format ci-dessous. Ce n’est pas un connecteur natif universel : une API de chat standard ne fournit pas ces données automatiquement.")).into_any_element(),
                div().my(px(15.)).p(px(14.)).rounded(px(7.)).bg(rgb(0x1c1c20)).font_family("Consolas").text_size(px(11.)).line_height(px(20.)).text_color(rgb(0xd4d4db)).child(sample).into_any_element(),
                para(t(french, "Exemple illustratif. Les mesures inconnues doivent être omises ou nulles.")).into_any_element(),
            ]))
            .child(section(false, t(french, "Local, lisible, sans surprises."), vec![
                para(t(french, "Pas de serveur CodeNotch, pas de télémétrie, pas d’inférence générée pour mesurer l’usage. La configuration est enregistrée dans le dossier de configuration de l’application. Les secrets ajoutés sont stockés dans le Gestionnaire d’identification Windows. Les relevés sont conservés en mémoire pendant la session ; les dates permettent de repérer une valeur ancienne.")).into_any_element(),
                para(t(french, "Les quotas sont interrogés périodiquement : il peut donc y avoir un décalage avec le fournisseur. L’activité n’est jamais présentée comme une donnée en direct quand elle ne l’est pas.")).into_any_element(),
            ]))
            .child(section(false, t(french, "Référence"), vec![
                para(format!("CodeNotch {}", t(french, "est la référence visuelle et fonctionnelle. Cette adaptation Windows est un projet indépendant, développé avec Tauri."))).into_any_element(),
            ]))
            .into_any_element()]
    }
}

// ---- Window ---------------------------------------------------------------------------

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let french = self.store.read(cx).french();
        let dialog = self.account_dialog(french, cx);
        let root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .text_size(px(13.))
            .child(titlebar(crate::usage::label(self.usage, french)));

        if self.store.read(cx).config.settings.onboarding_done == Some(false) {
            let welcome = self.welcome(french, cx);
            return root.child(welcome).children(dialog);
        }

        let (heading, intro, action, content) = match self.section {
            Section::Home => {
                let (heading, intro, action) = self.home_heading(french, cx);
                (heading, intro, Some(action), self.home_page(french, cx))
            }
            Section::Accounts => (
                t(french, "Comptes"),
                t(french, "Choisissez les comptes affichés dans l’encoche."),
                Some(button(
                    "add-account",
                    Kind::Primary,
                    Some("plus"),
                    t(french, "Ajouter"),
                    Some(Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_editor(None, window, cx)
                    }))),
                )),
                self.accounts_page(french, cx),
            ),
            Section::Appearance => (
                t(french, "Apparence"),
                t(french, "Langue, encoche, fréquence des relevés et mises à jour."),
                None,
                self.appearance_page(french, cx),
            ),
            Section::Alerts => (
                t(french, "Alertes"),
                t(french, "Discord, seuils et prévision du plafond, pour surveiller les agents qui tournent en fond."),
                None,
                self.alerts_page(french, cx),
            ),
            Section::About => (
                t(french, "À propos des relevés"),
                t(french, "Ce que CodeNotch lit, et ce que chaque mesure raconte."),
                None,
                self.about_page(french),
            ),
        };

        let pending = self.store.read(cx).update.as_ref().map(|r| r.version.clone());
        let installing = self.store.read(cx).installing;
        let update_pill = pending.clone().map(|version| {
            div()
                .id("update-pill")
                .mt(px(4.))
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(4.))
                .rounded_full()
                .bg(rgb(0x24303f))
                .text_color(rgb(0xb8cbe6))
                .text_size(px(11.))
                .when(!installing, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.bg(rgb(0x2c3b4e)))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.install_update(cx)))
                })
                .child(icon(if installing { "loader-circle" } else { "download" }, 13., 0xb8cbe6))
                .child(if installing {
                    t(french, "Mise à jour…")
                } else {
                    t(french, "Mise à jour {version}").replace("{version}", &version)
                })
        });
        let update_banner = pending.map(|version| self.update_banner(&version, french, cx));

        let count = self.store.read(cx).enabled().len();
        let nav = [
            (Section::Home, "house", "Accueil"),
            (Section::Accounts, "users-round", "Comptes"),
            (Section::Appearance, "settings-2", "Apparence"),
            (Section::Alerts, "bell", "Alertes"),
            (Section::About, "info", "À propos"),
        ]
        .into_iter()
        .map(|(section, leading, label)| {
            let active = self.section == section;
            div()
                .id(SharedString::from(format!("nav-{label}")))
                .flex()
                .items_center()
                .gap(px(9.))
                .px(px(10.))
                .min_h(px(38.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(12.))
                .when(active, |d| d.bg(rgba(0xffffff18)).text_color(rgb(0xffffff)))
                .when(!active, |d| d.text_color(rgb(0xc3c3c8)).hover(|d| d.bg(rgba(0xffffff0c))))
                .child(icon(leading, 17., if active { 0xffffff } else { 0xc3c3c8 }))
                .child(div().flex_1().child(t(french, label)))
                .when(section == Section::Accounts, |d| {
                    d.child(div().text_size(px(11.)).text_color(rgb(0x999999)).child(count.to_string()))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.section = section;
                    this.message = None;
                    this.open_select = None;
                    cx.notify();
                }))
        });

        let sidebar = div()
            .w(px(184.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pt(px(18.))
            .px(px(10.))
            .pb(px(15.))
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgba(LINE))
            .children(nav)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .pt(px(15.))
                    .px(px(10.))
                    .text_size(px(10.))
                    .text_color(rgb(0x94949b))
                    .child(t(french, "Adaptation Windows"))
                    .child(t(french, "v{version} · locale").replace("{version}", env!("CARGO_PKG_VERSION")))
                    .children(update_pill),
            );

        let page_heading = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .pt(px(26.))
            .pb(px(20.))
            .px(px(30.))
            .border_b_1()
            .border_color(rgba(LINE))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().text_size(px(22.)).font_weight(FontWeight::SEMIBOLD).child(heading))
                    .child(div().max_w(px(400.)).text_size(px(12.)).line_height(px(19.)).text_color(rgb(MUTED)).child(intro)),
            )
            .children(action);

        let storage_error = self.store.read(cx).storage_error.clone();
        let banner = self
            .message
            .clone()
            .map(|(ok, text)| message(ok, text))
            .or(storage_error.map(|e| message(false, e)));
        let body = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(page_heading)
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(24.))
                            .px(px(30.))
                            .pt(px(12.))
                            .pb(px(28.))
                            .children(update_banner)
                            .children(banner)
                            .children(content),
                    ),
            );

        root.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .child(sidebar)
                .child(body),
        )
        .children(dialog)
    }
}

/// `.settings-titlebar`: the mini notch, the name, and minimize / close.
fn titlebar(usage: String) -> AnyElement {
    let caption = |id: &'static str, glyph: &'static str, close: bool, action: fn(&mut Window)| {
        div()
            .id(id)
            .occlude()
            .w(px(42.))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(close, |d| d.hover(|d| d.bg(rgb(0xb72c32))))
            .when(!close, |d| d.hover(|d| d.bg(rgba(0xffffff12))))
            .child(icon(glyph, if close { 17. } else { 15. }, 0xaaaaaa))
            .on_click(move |_, window, _| action(window))
    };
    div()
        .id("titlebar")
        .h(px(44.))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .pl(px(17.))
        .bg(rgb(0x121214))
        .border_b_1()
        .border_color(rgba(LINE))
        .window_control_area(WindowControlArea::Drag)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(9.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(mini_notch())
                .child("CodeNotch")
                .child(
                    div()
                        .ml(px(2.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(rgb(0x8f8f96))
                        .child("Windows"),
                ),
        )
        .child(
            div()
                .h_full()
                .flex()
                .items_center()
                // The whole computer's use, as Task Manager's Performance tab reports it.
                .child(
                    div()
                        .mr(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(rgb(0x8f8f96))
                        .child(icon("monitor", 13., 0x8f8f96))
                        .child(usage),
                )
                .child(caption("min", "minus", false, |w| w.minimize_window()))
                .child(caption("close", "x", true, |w| w.remove_window())),
        )
        .into_any_element()
}

/// A small hover bubble, as the mock's `data-tip`.
pub(crate) struct Tip(SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(0x44444a))
            .bg(rgb(0x242427))
            .text_size(px(11.))
            .text_color(rgb(0xe5e5ea))
            .font_family("Segoe UI")
            .child(self.0.clone())
    }
}

pub(crate) fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}
