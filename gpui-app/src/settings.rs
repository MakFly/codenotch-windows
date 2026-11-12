use crate::i18n::t;
use crate::store::Store;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Render, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size,
};

pub struct SettingsView {
    store: Entity<Store>,
    focus: FocusHandle,
}

impl SettingsView {
    fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            focus: cx.focus_handle(),
            store,
        }
    }
}

pub fn open(store: Entity<Store>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(480.), px(620.)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            focus: true,
            titlebar: Some(TitlebarOptions {
                title: Some("CodeNotch".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        |window, cx| {
            cx.new(|cx| {
                let view = SettingsView::new(store, cx);
                view.focus.focus(window);
                view
            })
        },
    );
    cx.activate(true);
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let french = store.french();
        let edge = store.config.settings.widget_position.clone();
        let poll = store.config.settings.poll_interval();
        let game = store.config.settings.game_mode;
        let hide = store.config.settings.hide_in_fullscreen;
        let error = store.storage_error.clone();
        let accounts: Vec<(String, String, String)> = store
            .config
            .accounts
            .iter()
            .map(|account| {
                let status = match store.reading(&account.id) {
                    Some(reading) if !reading.windows.is_empty() => reading
                        .windows
                        .iter()
                        .map(|window| format!("{} {:.0}%", window.label, window.used_percent))
                        .collect::<Vec<_>>()
                        .join(" · "),
                    Some(reading) => reading
                        .note
                        .clone()
                        .unwrap_or_else(|| t(french, "Unavailable", "Indisponible")),
                    None => t(french, "Not read yet", "Pas encore lu"),
                };
                (account.name.clone(), account.provider.clone(), status)
            })
            .collect();
        let _ = store;

        let mut page = div()
            .id("settings")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0x161616))
            .text_color(rgb(0xf4f4f4))
            .overflow_y_scroll()
            .child(div().text_xl().child("CodeNotch"))
            .child(div().text_sm().text_color(rgb(0x8a8f98)).child(t(
                french,
                "Rebuild 0.3.0. Readings come from the providers. Missing numbers stay a dash.",
                "Reconstruction 0.3.0. Les relevés viennent des fournisseurs. Un chiffre manquant reste un tiret.",
            )));

        if let Some(error) = error {
            page = page.child(div().text_sm().text_color(rgb(0xe15a4a)).child(error));
        }

        for (name, provider, status) in accounts {
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(rgb(0x2c2c2c))
                    .rounded(px(8.))
                    .child(div().text_sm().child(format!("{name} · {provider}")))
                    .child(div().text_xs().text_color(rgb(0xc8c8c8)).child(status)),
            );
        }

        page = page
            .child(button(
                t(french, "Refresh now", "Actualiser"),
                cx.listener(|view, _, _, cx| {
                    let store = view.store.clone();
                    cx.spawn(async move |_view, cx| {
                        crate::store::poll(&store, true, cx).await;
                    })
                    .detach();
                }),
            ))
            .child(button(
                t(
                    french,
                    &format!("Edge: {edge} (click to change)"),
                    &format!("Bord : {edge} (clic pour changer)"),
                ),
                cx.listener(|view, _, _, cx| {
                    view.store.update(cx, |store, cx| {
                        store.config.settings.widget_position = next_edge(&store.config.settings.widget_position);
                        let _ = store.save();
                        cx.notify();
                    });
                }),
            ))
            .child(button(
                t(
                    french,
                    &format!("Refresh every {poll}s"),
                    &format!("Relevé toutes les {poll} s"),
                ),
                cx.listener(|view, _, _, cx| {
                    view.store.update(cx, |store, cx| {
                        store.config.settings.poll_seconds = next_poll(store.config.settings.poll_seconds);
                        let _ = store.save();
                        cx.notify();
                    });
                }),
            ))
            .child(button(
                if french { "English" } else { "Français" }.to_string(),
                cx.listener(|view, _, _, cx| {
                    view.store.update(cx, |store, cx| {
                        store.config.settings.language = if store.french() { "en" } else { "fr" }.into();
                        let _ = store.save();
                        cx.notify();
                    });
                }),
            ))
            .child(button(
                t(
                    french,
                    if game { "Game mode: on" } else { "Game mode: off" },
                    if game { "Mode jeu : oui" } else { "Mode jeu : non" },
                ),
                cx.listener(|view, _, _, cx| {
                    view.store.update(cx, |store, cx| {
                        store.config.settings.game_mode = !store.config.settings.game_mode;
                        let _ = store.save();
                        cx.notify();
                    });
                }),
            ))
            .child(button(
                t(
                    french,
                    if hide { "Hide in full screen: on" } else { "Hide in full screen: off" },
                    if hide { "Masquer en plein écran : oui" } else { "Masquer en plein écran : non" },
                ),
                cx.listener(|view, _, _, cx| {
                    view.store.update(cx, |store, cx| {
                        store.config.settings.hide_in_fullscreen = !store.config.settings.hide_in_fullscreen;
                        let _ = store.save();
                        cx.notify();
                    });
                }),
            ))
            .child(button(
                t(french, "Quit", "Quitter"),
                cx.listener(|_, _, _, cx| cx.quit()),
            ));
        page
    }
}

fn next_edge(edge: &str) -> String {
    match edge {
        "right" => "left",
        "left" => "top",
        "top" => "bottom",
        _ => "right",
    }
    .into()
}

fn next_poll(current: u64) -> u64 {
    match current {
        30 => 60,
        60 => 120,
        120 => 300,
        _ => 30,
    }
}

fn button(
    label: String,
    click: impl Fn(&MouseDownEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(gpui::ElementId::Name(label.clone().into()))
        .px_3()
        .py_2()
        .bg(rgb(0x242424))
        .rounded(px(8.))
        .text_sm()
        .cursor_pointer()
        .child(label)
        .on_mouse_down(MouseButton::Left, click)
}
