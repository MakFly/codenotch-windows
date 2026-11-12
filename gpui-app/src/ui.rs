use crate::i18n::t;
use crate::settings;
use crate::store::{self, Store};
use crate::usage::{self, Reading};
use gpui::{
    App, Bounds, Context, Entity, IntoElement, MouseButton, ParentElement, Pixels, Point, Render,
    Size, Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    div, point, prelude::*, px, rgb, size,
};

pub struct Rail {
    store: Entity<Store>,
    expanded: bool,
    placed: Option<(bool, String, i64)>,
}

impl Rail {
    pub fn open(store: Entity<Store>, cx: &mut App) {
        let bounds = Bounds {
            origin: point(px(40.), px(40.)),
            size: size(px(22.), px(88.)),
        };
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                window_background: WindowBackgroundAppearance::Transparent,
                focus: false,
                show: true,
                kind: WindowKind::PopUp,
                is_movable: false,
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Rail::new(store, cx)),
        )
        .ok();
    }

    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            expanded: false,
            placed: None,
        }
    }

    fn desired_size(&self, reading_rows: usize, hidden: bool) -> Size<Pixels> {
        if hidden {
            return size(px(1.), px(1.));
        }
        if self.expanded {
            let height = 56.0 + reading_rows as f32 * 64.0;
            size(px(320.), px(height.clamp(140.0, 640.0)))
        } else {
            size(px(22.), px(88.))
        }
    }

    fn sync_bounds(&mut self, window: &mut Window, cx: &mut Context<Self>, hidden: bool, rows: usize) {
        let edge = self
            .store
            .read(cx)
            .config
            .settings
            .widget_position
            .clone();
        let y_bits = self
            .store
            .read(cx)
            .config
            .settings
            .widget_placement
            .y
            .to_bits();
        let key = (self.expanded || hidden, edge.clone(), y_bits as i64);
        if self.placed.as_ref() == Some(&key) {
            return;
        }
        let Some(display) = cx.displays().into_iter().next() else {
            return;
        };
        let screen = display.bounds();
        let desired = self.desired_size(rows, hidden);
        let origin = origin_for(&edge, screen, desired, self.store.read(cx).config.settings.widget_placement.y);
        window.resize(desired);
        let _ = origin;
        self.placed = Some(key);
        // `resize` does not move the window. Position is applied when the method exists;
        // the call below is the GPUI 0.2 name used by Zed.
        position_window(window, Bounds { origin, size: desired });
    }
}

fn origin_for(edge: &str, screen: Bounds<Pixels>, desired: Size<Pixels>, y_frac: f64) -> Point<Pixels> {
    let y_frac = y_frac.clamp(0.0, 1.0) as f32;
    let x_span = (screen.size.width - desired.width).max(px(0.));
    let y_span = (screen.size.height - desired.height).max(px(0.));
    match edge {
        "left" => point(screen.origin.x, screen.origin.y + y_span * y_frac),
        "top" => point(screen.origin.x + x_span * y_frac, screen.origin.y),
        "bottom" => point(
            screen.origin.x + x_span * y_frac,
            screen.origin.y + screen.size.height - desired.height,
        ),
        _ => point(
            screen.origin.x + screen.size.width - desired.width,
            screen.origin.y + y_span * y_frac,
        ),
    }
}

fn position_window(window: &mut Window, bounds: Bounds<Pixels>) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE,
        };
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win) = handle.as_raw() else {
            return;
        };
        let device = bounds.to_device_pixels(window.scale_factor());
        unsafe {
            SetWindowPos(
                win.hwnd.get() as _,
                HWND_TOPMOST,
                device.origin.x.into(),
                device.origin.y.into(),
                device.size.width.into(),
                device.size.height.into(),
                SWP_NOACTIVATE,
            );
        }
    }
    #[cfg(not(windows))]
    {
        window.resize(bounds.size);
        let _ = bounds;
    }
}

impl Render for Rail {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let french = store.french();
        let hidden = store.hidden();
        let warn = store.config.settings.alert_percent;
        let critical = store.config.settings.critical_percent;
        let rows = store
            .config
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .count();
        let worst = store.worst_percent();
        let accounts: Vec<(String, String, Option<Reading>)> = store
            .config
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .map(|account| {
                (
                    account.id.clone(),
                    account.name.clone(),
                    store.reading(&account.id).cloned(),
                )
            })
            .collect();
        let _ = store;
        self.sync_bounds(window, cx, hidden, rows);
        let color = store::usage_color(worst, warn, critical);
        let expanded = self.expanded && !hidden;

        div()
            .id("codenotch-rail")
            .size_full()
            .on_hover(cx.listener(|rail, hovered: &bool, window, cx| {
                if rail.store.read(cx).config.settings.auto_hide {
                    rail.expanded = *hovered;
                    rail.placed = None;
                    rail.sync_bounds(window, cx, rail.store.read(cx).hidden(), 1);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|rail, _, _, cx| {
                    settings::open(rail.store.clone(), cx);
                }),
            )
            .child(if hidden {
                div().size_full()
            } else if expanded {
                expanded_card(french, color, warn, critical, accounts)
            } else {
                div()
                    .size_full()
                    .bg(rgb(0x111111))
                    .border_1()
                    .border_color(color)
                    .rounded(px(8.))
            })
    }
}

fn expanded_card(
    french: bool,
    color: gpui::Hsla,
    warn: u8,
    critical: u8,
    accounts: Vec<(String, String, Option<Reading>)>,
) -> gpui::Div {
    let mut card = div()
        .size_full()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .bg(rgb(0x111111))
        .text_color(rgb(0xf2f2f2))
        .border_1()
        .border_color(color)
        .rounded(px(12.))
        .child(
            div()
                .text_sm()
                .child(t(french, "Quotas", "Quotas")),
        );
    if accounts.is_empty() {
        card = card.child(div().text_sm().text_color(rgb(0x8a8f98)).child(t(
            french,
            "No account yet. Click to open settings.",
            "Aucun compte. Clique pour ouvrir les réglages.",
        )));
    }
    let now = usage::now_secs();
    for (_id, name, reading) in accounts {
        card = card.child(account_block(french, &name, reading.as_ref(), warn, critical, now));
    }
    card
}

fn account_block(
    french: bool,
    name: &str,
    reading: Option<&Reading>,
    warn: u8,
    critical: u8,
    now: i64,
) -> gpui::Div {
    let mut block = div().flex().flex_col().gap_1().child(
        div()
            .text_sm()
            .child(name.to_string()),
    );
    match reading {
        None => {
            block = block.child(div().text_xs().text_color(rgb(0x8a8f98)).child(t(
                french,
                "Waiting for the first reading",
                "En attente du premier relevé",
            )));
        }
        Some(reading) if reading.windows.is_empty() => {
            let note = reading
                .note
                .clone()
                .unwrap_or_else(|| t(french, "Unavailable", "Indisponible"));
            block = block
                .child(div().text_xs().text_color(rgb(0xe6b15a)).child(note));
        }
        Some(reading) => {
            for window in &reading.windows {
                let color = store::usage_color(Some(window.used_percent), warn, critical);
                block = block.child(
                    div()
                        .flex()
                        .justify_between()
                        .text_sm()
                        .child(
                            div().flex().gap_2().child(
                                div()
                                    .size(px(14.))
                                    .rounded(px(7.))
                                    .border_2()
                                    .border_color(color),
                            )
                            .child(window.label.clone())
                            .child(format!("{:.0}%", window.used_percent)),
                        )
                        .child(
                            div()
                                .text_color(rgb(0x8a8f98))
                                .child(usage::format_reset(window.resets_at, now, french)),
                        ),
                );
            }
            if let Some(note) = &reading.note {
                block = block
                    .child(div().text_xs().text_color(rgb(0xe6b15a)).child(note.clone()));
            }
        }
    }
    block
}
