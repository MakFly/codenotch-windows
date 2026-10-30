//! Quick connect (`.quick-connect`), shared by the Accounts page and the first-run welcome
//! screen (`.welcome`), which replaces the settings while `onboarding_done` is `Some(false)`.
use crate::account_form::name;
use crate::login::{self, ProviderStatus};
use crate::model::Account;
use crate::settings::{button, icon, message, toggle, Kind, SettingsView, MUTED};
use crate::ui::{provider_icon, t};
use gpui::{
    div, prelude::*, px, rgb, rgba, Animation, AnimationExt, AnyElement, ClickEvent, ClipboardItem, Context,
    FontWeight, SharedString, Transformation,
};
use std::time::Duration;

/// Hover group of a quick-connect row, so its actions can react to the whole row.
const ROW_GROUP: &str = "quick-row";

pub(crate) fn statuses() -> Vec<ProviderStatus> {
    login::PROVIDERS
        .iter()
        .map(|p| login::status(p, None))
        .collect()
}

fn install_hint(provider: &str) -> &'static str {
    match provider {
        "codex" => "npm i -g @openai/codex",
        "claude" => "irm https://claude.ai/install.ps1 | iex",
        "gemini" => "npm i -g @google/gemini-cli",
        "grok" => "x.ai/cli",
        _ => "npm i -g opencode-ai",
    }
}

fn same_path(a: &str, b: &str) -> bool {
    let key = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    key(a) == key(b)
}

/// The profile this status describes is already in the notch.
fn tracked(accounts: &[Account], status: &ProviderStatus) -> bool {
    accounts.iter().any(|a| {
        a.provider == status.provider
            && (a.profile_path.is_empty() || same_path(&a.profile_path, &status.profile_path))
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Ok,
    Warn,
    Info,
    Idle,
}

/// A status pill with its dot, as in the quick-connect mock.
fn pill(tone: Tone, text: String) -> AnyElement {
    let (fg, bg, dot) = match tone {
        Tone::Ok => (0x7fd69b, rgba(0x7fd69b1a), 0x7fd69b),
        Tone::Warn => (0xefa44f, rgba(0xefa44f1a), 0xefa44f),
        Tone::Info => (0xb8cbe6, rgba(0x24303fff), 0xb8cbe6),
        Tone::Idle => (0x9a9aa3, rgba(0xffffff0d), 0x6b6b72),
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_size(px(10.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(fg))
        .child(div().size(px(6.)).rounded_full().bg(rgb(dot)))
        .child(text)
        .into_any_element()
}

fn group_label(text: String) -> AnyElement {
    div()
        .mb(px(8.))
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(0x77777f))
        .child(text.to_uppercase())
        .into_any_element()
}

/// Rows inside one rounded, divided card.
fn row_card(rows: Vec<AnyElement>, dashed: bool) -> AnyElement {
    let count = rows.len();
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .border_1()
        .border_color(if dashed { rgba(0xffffff1a) } else { rgba(0xffffff12) })
        .bg(rgba(0xffffff05))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(move |(i, row)| {
            div()
                .when(i + 1 < count, |d| d.border_b_1().border_color(rgba(0xffffff0f)))
                .child(row)
        }))
        .into_any_element()
}

/// The CLI path as "codex.exe · application Codex", the full path left for the tooltip.
fn short_source(path: &str, french: bool) -> (String, String) {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path).to_owned();
    let lower = path.to_lowercase();
    let place = if lower.contains("openai\\codex\\bin\\") || lower.contains("programs\\openai\\codex") {
        t(french, "application Codex")
    } else if lower.contains("claude-data") {
        t(french, "application Claude")
    } else {
        let dir = path[..path.len() - file.len()].trim_end_matches(['\\', '/']);
        match dirs::home_dir().map(|h| h.to_string_lossy().to_string()) {
            Some(home) if dir.to_lowercase().starts_with(&home.to_lowercase()) => {
                format!("~{}", &dir[home.len()..])
            }
            _ => dir.to_owned(),
        }
    };
    (file, place)
}

/// The agent's logo when CodeNotch has one, otherwise its initial on a small tile.
fn agent_mark(agent: &crate::agents::Agent, size: f32) -> AnyElement {
    match &agent.logo {
        Some(logo) => provider_icon(logo, size),
        None => div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .border_1()
            .border_color(rgba(0xffffff1a))
            .bg(rgb(0x1f1f22))
            .text_size(px(size * 0.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(0xc3c3c8))
            .child(agent.name.chars().next().unwrap_or('?').to_uppercase().to_string())
            .into_any_element(),
    }
}

fn homepage_button(id: String, url: Option<String>, french: bool) -> AnyElement {
    let Some(url) = url else {
        return div().into_any_element();
    };
    div()
        .id(SharedString::from(id))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(rgb(0xc3c3c8))
        .hover(|d| d.bg(rgba(0xffffff0f)).text_color(rgb(0xffffff)))
        .tooltip(crate::settings::tip(url.clone()))
        .child(icon("external-link", 13., 0xc3c3c8))
        .child(t(french, "Site officiel"))
        .on_click(move |_, _, _| {
            let _ = login::open_url(&url);
        })
        .into_any_element()
}

impl SettingsView {
    /// Installed CLIs CodeNotch has no sign-in for: quotas unpublished, or added by token.
    fn other_cli_row(&self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let agent = &crate::agents::all()[index];
        let path = self.detected[index].clone().unwrap_or_default();
        let (file, place) = short_source(&path, french);
        let by_token = agent.quota.is_some();
        let action = if by_token {
            let provider = agent.quota.clone().unwrap_or_default();
            button(
                SharedString::from(format!("agent-add-{}", agent.id)),
                Kind::Primary,
                Some("plus"),
                t(french, "Ajouter"),
                Some(Box::new(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    let account = Account {
                        id: String::new(),
                        name: String::new(),
                        provider: provider.clone(),
                        enabled: true,
                        profile_path: String::new(),
                        endpoint: String::new(),
                        secret_env: String::new(),
                        has_secret: false,
                    };
                    this.open_editor(Some(account), window, cx)
                }))),
            )
        } else {
            homepage_button(format!("agent-site-{}", agent.id), agent.homepage.clone(), french)
        };
        div()
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(16.))
            .py(px(12.))
            .hover(|d| d.bg(rgba(0xffffff08)))
            .child(agent_mark(agent, 22.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(agent.name.clone()))
                            .child(if by_token {
                                pill(Tone::Info, t(french, "Ajout par jeton"))
                            } else {
                                pill(Tone::Idle, t(french, "Pas de quota publié"))
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("agent-source-{}", agent.id)))
                            .mt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.))
                            .text_color(rgb(0x7e7e88))
                            .tooltip(crate::settings::tip(path.clone()))
                            .child(icon("terminal", 12., 0x7e7e88))
                            .child(div().font_family("Consolas").child(file))
                            .child(div().text_color(rgb(0x5c5c64)).child("·"))
                            .child(place),
                    ),
            )
            .child(action)
            .into_any_element()
    }

    /// Every CLI in the catalog, folded by default: name, command, state and official site.
    fn all_agents(&self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let agents = crate::agents::all();
        let open = self.show_all_agents;
        let header = div()
            .id("all-agents-toggle")
            .mt(px(20.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 14., 0x77777f))
            .child(
                div()
                    .text_size(px(10.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x77777f))
                    .child(t(french, "Tous les CLI").to_uppercase()),
            )
            .child(
                div()
                    .px(px(6.))
                    .rounded_full()
                    .bg(rgba(0xffffff0a))
                    .text_size(px(10.5))
                    .text_color(rgb(0x9a9aa3))
                    .child(agents.len().to_string()),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_all_agents = !this.show_all_agents;
                cx.notify();
            }));
        let rows: Vec<AnyElement> = open
            .then(|| {
                agents
                    .iter()
                    .enumerate()
                    .map(|(i, agent)| {
                        let installed = self.detected.get(i).is_some_and(|p| p.is_some());
                        let state = if agent.quota.is_some() {
                            pill(Tone::Ok, t(french, "Quotas lus"))
                        } else if installed {
                            pill(Tone::Info, t(french, "Installé"))
                        } else {
                            div().into_any_element()
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(16.))
                            .py(px(8.))
                            .hover(|d| d.bg(rgba(0xffffff08)))
                            .child(agent_mark(agent, 18.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(agent.name.clone()))
                                    .child(
                                        div()
                                            .font_family("Consolas")
                                            .text_size(px(11.))
                                            .text_color(rgb(0x7e7e88))
                                            .child(agent.commands.first().cloned().unwrap_or_default()),
                                    )
                                    .child(state),
                            )
                            .child(homepage_button(format!("all-site-{}", agent.id), agent.homepage.clone(), french))
                            .into_any_element()
                    })
                    .collect()
            })
            .unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .child(header)
            .when(open, |d| {
                d.child(div().mt(px(8.)).child(row_card(rows, false))).child(
                    div()
                        .mt(px(8.))
                        .text_size(px(10.5))
                        .text_color(rgb(0x6b6b72))
                        .child(t(french, "Liste tirée du catalogue d’Orca (github.com/stablyai/orca), mise à jour par « bun run sync:agents ».")),
                )
            })
            .into_any_element()
    }

    pub(crate) fn load_statuses(&mut self) {
        self.statuses = statuses();
        self.detected = crate::agents::detect();
    }

    /// Opens the provider's official sign-in from a quick-connect row.
    fn quick_sign_in(&self, provider: String, cx: &mut Context<Self>) -> crate::settings::Click {
        Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| this.sign_in(provider.clone(), String::new(), cx)))
    }

    /// Signed in on this PC: the session is tracked, or only needs adding.
    fn signed_in_row(&mut self, index: usize, tracked_now: bool, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let status = &self.statuses[index];
        let p = status.provider.clone();
        let path = status.cli_path.clone();
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let (status_pill, action) = if waiting {
            (Some(pill(Tone::Info, t(french, "Connexion en cours"))), self.waiting_action(french, cx))
        } else if tracked_now {
            // Icon only at rest; the label shows while the row is hovered.
            let reconnect = div()
                .id(SharedString::from(format!("reconnect-{p}")))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(32.))
                .px(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgba(0x00000000))
                .text_size(px(12.))
                .text_color(rgb(0xc3c3c8))
                .opacity(0.6)
                .cursor_pointer()
                .group_hover(ROW_GROUP, |s| s.opacity(1.))
                .hover(|d| d.bg(rgba(0xffffff0f)).border_color(rgb(0x3b3b3f)).text_color(rgb(0xffffff)))
                .tooltip(crate::settings::tip(t(french, "Relancer la connexion officielle")))
                .child(icon("refresh-cw", 14., 0xc3c3c8))
                .child(div().opacity(0.).group_hover(ROW_GROUP, |s| s.opacity(1.)).child(t(french, "Reconnecter")))
                .when(self.login.is_none(), |d| d.on_click(self.quick_sign_in(p.clone(), cx)))
                .into_any_element();
            // The "In the notch" group label already says it: no pill.
            (None, reconnect)
        } else {
            let provider = p.clone();
            let add = button(
                SharedString::from(format!("add-{p}")),
                Kind::Primary,
                Some("plus"),
                t(french, "Ajouter"),
                Some(Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let provider = provider.clone();
                    this.run(cx, move |s| {
                        s.finish_login(&provider)
                            .map(|_| Some(t(french, "Compte enregistré. Le suivi démarrera automatiquement.")))
                    });
                    this.load_statuses();
                    this.refresh_now(cx);
                }))),
            );
            (Some(pill(Tone::Warn, t(french, "Connecté sur ce PC"))), add)
        };
        let source = path.map(|path| {
            let (file, place) = short_source(&path, french);
            div()
                .id(SharedString::from(format!("source-{p}")))
                .mt(px(4.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(11.))
                .text_color(rgb(0x7e7e88))
                .tooltip(crate::settings::tip(path))
                .child(icon("terminal", 12., 0x7e7e88))
                .child(div().font_family("Consolas").child(file))
                .child(div().text_color(rgb(0x5c5c64)).child("·"))
                .child(place)
        });
        let hint = (!tracked_now && !waiting)
            .then(|| t(french, "Session trouvée sur ce PC, pas encore dans l’encoche."))
            .or(waiting.then(|| t(french, "Termine dans la fenêtre ouverte, l’ajout est automatique.")));
        self.row(&p, french, status_pill, hint, source.map(|s| s.into_any_element()), action)
    }

    /// Installed but signed out, or a sign-in in progress.
    fn signed_out_row(&mut self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.statuses[index].provider.clone();
        let waiting = self.login.as_ref().is_some_and(|l| l.provider == p);
        let (status_pill, hint, action) = if waiting {
            (
                pill(Tone::Info, t(french, "Connexion en cours")),
                t(french, "Termine dans la fenêtre ouverte, l’ajout est automatique."),
                self.waiting_action(french, cx),
            )
        } else {
            let click = self.login.is_none().then(|| self.quick_sign_in(p.clone(), cx));
            (
                pill(Tone::Info, t(french, "Installé")),
                t(french, "CLI trouvé, pas encore connecté."),
                button(SharedString::from(format!("sign-in-{p}")), Kind::Primary, Some("log-in"), t(french, "Se connecter"), click),
            )
        };
        self.row(&p, french, Some(status_pill), Some(hint), None, action)
    }

    fn waiting_action(&self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(34.))
                    .px(px(12.))
                    .rounded(px(6.))
                    .bg(rgb(0x24303f))
                    .text_size(px(12.))
                    .text_color(rgb(0xb8cbe6))
                    .child(
                        icon("loader-circle", 14., 0xb8cbe6).with_animation(
                            "quick-spinner",
                            Animation::new(Duration::from_millis(900)).repeat(),
                            |icon, delta| icon.with_transformation(Transformation::rotate(gpui::percentage(delta))),
                        ),
                    )
                    .child(t(french, "En attente du navigateur…")),
            )
            .child(button(
                "quick-cancel",
                Kind::Ghost,
                None,
                t(french, "Annuler"),
                Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.login = None;
                    cx.notify();
                }))),
            ))
            .into_any_element()
    }

    fn row(
        &self,
        provider: &str,
        french: bool,
        status_pill: Option<AnyElement>,
        hint: Option<String>,
        source: Option<AnyElement>,
        action: AnyElement,
    ) -> AnyElement {
        div()
            .group(ROW_GROUP)
            .flex()
            .items_center()
            .gap(px(14.))
            .px(px(16.))
            .py(px(12.))
            .hover(|d| d.bg(rgba(0xffffff08)))
            .child(provider_icon(provider, 22.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(t(french, &name(provider, true))))
                            .children(status_pill),
                    )
                    .when_some(hint, |d, hint| d.child(div().mt(px(4.)).text_size(px(11.)).text_color(rgb(0x9999a2)).child(hint)))
                    .children(source),
            )
            .child(action)
            .into_any_element()
    }

    /// CLI not installed: the install command is the action, copyable in one click.
    fn install_card(&mut self, index: usize, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = self.statuses[index].provider.clone();
        let command = install_hint(&p).to_owned();
        let copied = self.copied.as_deref() == Some(command.as_str());
        let to_copy = command.clone();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(px(12.))
            .border_1()
            .border_color(rgba(0xffffff12))
            .bg(rgba(0xffffff05))
            // The "Available" group label already says the CLI is missing: no pill, so the
            // description gets the whole width.
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(12.))
                    .child(div().mt(px(1.)).child(provider_icon(&p, 20.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).child(t(french, &name(&p, true))))
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .text_color(rgb(0x9999a2))
                                    .child(crate::account_form::description(&p, french)),
                            ),
                    ),
            )
            .child(
                // Pinned to the bottom, so two cards of a row line up whatever their text.
                div()
                    .mt_auto()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(12.))
                    .pr(px(4.))
                    .py(px(4.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(0x2c2c30))
                    .bg(rgb(0x17171a))
                    .child(div().font_family("Consolas").text_size(px(11.)).text_color(rgb(0x5c9bf5)).child("$"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Consolas")
                            .text_size(px(11.5))
                            .text_color(rgb(0xd4d4db))
                            .child(command.clone()),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("copy-{p}")))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px(4.))
                            .h(px(28.))
                            .px(px(8.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(if copied { rgb(0x7fd69b) } else { rgb(0xc3c3c8) })
                            .hover(|d| d.bg(rgba(0xffffff0f)).text_color(rgb(0xffffff)))
                            .child(icon(if copied { "check" } else { "copy" }, 13., if copied { 0x7fd69b } else { 0xc3c3c8 }))
                            .child(if copied { t(french, "Copié") } else { t(french, "Copier") })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(to_copy.clone()));
                                this.copied = Some(to_copy.clone());
                                cx.notify();
                                let done = to_copy.clone();
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(Duration::from_millis(1200)).await;
                                    let _ = this.update(cx, |this, cx| {
                                        if this.copied.as_deref() == Some(done.as_str()) {
                                            this.copied = None;
                                            cx.notify();
                                        }
                                    });
                                })
                                .detach();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(rgb(0x7e7e88))
                    .child(t(french, "Puis reviens ici et clique sur « Vérifier »."))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_color(rgb(0x6b6b72))
                            .child(icon("log-in", 12., 0x6b6b72))
                            .child(t(french, "Se connecter")),
                    ),
            )
            .into_any_element()
    }

    /// Quick connect: a summary, the sessions in the notch, those to connect, and the CLIs
    /// still to install.
    pub(crate) fn quick_connect(
        &mut self,
        french: bool,
        heading: &str,
        intro: bool,
        catalog: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accounts = self.store.read(cx).config.accounts.clone();
        let (mut in_notch, mut to_connect, mut to_install) = (vec![], vec![], vec![]);
        for i in 0..self.statuses.len() {
            let s = &self.statuses[i];
            let waiting = self.login.as_ref().is_some_and(|l| l.provider == s.provider);
            if s.signed_in && tracked(&accounts, s) && !waiting {
                in_notch.push(i);
            } else if s.signed_in || s.installed || waiting {
                to_connect.push(i);
            } else {
                to_install.push(i);
            }
        }
        let summary = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .when(!in_notch.is_empty(), |d| {
                d.child(pill(Tone::Ok, t(french, "{count} suivis").replace("{count}", &in_notch.len().to_string())))
            })
            .when(!to_install.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(10.))
                        .py(px(3.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(0x414147))
                        .text_size(px(11.))
                        .text_color(rgb(0x9a9aa3))
                        .child(t(french, "{count} à installer").replace("{count}", &to_install.len().to_string())),
                )
            })
            .child(
                div()
                    .id("quick-check")
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff0d)))
                    .tooltip(crate::settings::tip(t(french, "Vérifier à nouveau")))
                    .child(icon("refresh-cw", 15., 0xb8cbe6))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.load_statuses();
                        cx.notify();
                    })),
            );

        let in_notch_rows: Vec<AnyElement> = in_notch.iter().map(|&i| self.signed_in_row(i, true, french, cx)).collect();
        let to_connect_rows: Vec<AnyElement> = to_connect
            .iter()
            .map(|&i| {
                if self.statuses[i].signed_in {
                    self.signed_in_row(i, false, french, cx)
                } else {
                    self.signed_out_row(i, french, cx)
                }
            })
            .collect();
        // Installed CLIs CodeNotch cannot sign in to: shown with their official site, or added
        // by token when CodeNotch reads their quotas.
        let other_rows: Vec<AnyElement> = crate::agents::all()
            .iter()
            .enumerate()
            .filter(|(i, agent)| {
                self.detected.get(*i).is_some_and(|p| p.is_some()) && !login::PROVIDERS.contains(&agent.id.as_str())
            })
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|i| self.other_cli_row(i, french, cx))
            .collect();
        let all_agents = catalog.then(|| self.all_agents(french, cx));
        let install_cards: Vec<AnyElement> = to_install.iter().map(|&i| self.install_card(i, french, cx)).collect();
        // Two cards per line, like the mock's grid.
        let mut install_grid: Vec<AnyElement> = vec![];
        let mut cards = install_cards.into_iter().peekable();
        while cards.peek().is_some() {
            let first = cards.next();
            let second = cards.next();
            install_grid.push(
                div()
                    .flex()
                    .gap(px(12.))
                    .children(first)
                    .child(second.unwrap_or_else(|| div().flex_1().into_any_element()))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(12.))
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!heading.is_empty(), |d| {
                                d.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(t(french, heading)))
                            })
                            .when(intro, |d| {
                                d.child(
                                    div()
                                        .mt(px(4.))
                                        .max_w(px(440.))
                                        .text_size(px(11.))
                                        .line_height(px(17.))
                                        .text_color(rgb(0x9999a2))
                                        .child(t(french, "Connexion officielle du fournisseur. CodeNotch ajoute le compte dès que la session est détectée — il ne voit jamais ton mot de passe.")),
                                )
                            }),
                    )
                    .child(summary),
            )
            .when(!in_notch_rows.is_empty(), |d| {
                d.child(group_label(t(french, "Dans l’encoche"))).child(row_card(in_notch_rows, false))
            })
            .when(!to_connect_rows.is_empty(), |d| {
                d.child(div().mt(px(20.)).child(group_label(t(french, "À connecter"))))
                    .child(row_card(to_connect_rows, false))
            })
            .when(!install_grid.is_empty(), |d| {
                d.child(div().mt(px(20.)).child(group_label(t(french, "Disponibles"))))
                    .child(div().flex().flex_col().gap(px(12.)).children(install_grid))
            })
            .when(!other_rows.is_empty(), |d| {
                d.child(div().mt(px(20.)).child(group_label(t(french, "Autres CLI installés"))))
                    .child(row_card(other_rows, false))
            })
            .children(all_agents)
            .into_any_element()
    }

    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        self.update_settings(cx, |s| s.onboarding_done = Some(true));
    }

    /// Two segments, so switching language is one click and no menu.
    fn language_toggle(&self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let segment = |code: &'static str, label: &'static str, on: bool, click: crate::settings::Click| {
            div()
                .id(SharedString::from(format!("welcome-lang-{code}")))
                .flex()
                .items_center()
                .h_full()
                .px(px(8.))
                .rounded(px(4.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .when(on, |d| d.bg(rgb(0x3a3a3f)).text_color(rgb(0xffffff)))
                .when(!on, |d| {
                    d.text_color(rgb(0x9a9aa3))
                        .cursor_pointer()
                        .hover(|d| d.text_color(rgb(0xffffff)))
                        .on_click(click)
                })
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(28.))
            .p(px(2.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(0x414147))
            .bg(rgb(0x1c1c1f))
            .child(div().px(px(6.)).child(icon("languages", 14., 0x9a9aa3)))
            .child(segment("fr", "FR", french, self.setting(cx, |s| s.language = "fr".into())))
            .child(segment("en", "EN", !french, self.setting(cx, |s| s.language = "en".into())))
            .into_any_element()
    }

    pub(crate) fn welcome(&mut self, french: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self.store.read(cx).enabled().len();
        let startup = self.store.read(cx).config.settings.launch_at_startup;
        let language = self.language_toggle(french, cx);
        let subscriptions = self.quick_connect(french, "Tes abonnements", false, false, cx);
        let banner = self.message.clone().map(|(ok, text)| message(ok, text));

        let header = div()
            .mb(px(20.))
            .w_full()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .mb(px(6.))
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xb8cbe6))
                                    .child(t(french, "Bienvenue dans CodeNotch").to_uppercase()),
                            )
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t(french, "Connecte tes abonnements IA")),
                            ),
                    )
                    .child(language),
            )
            .child(
                div()
                    .mt(px(8.))
                    .w_full()
                    .max_w(px(580.))
                    .text_size(px(12.5))
                    .line_height(px(20.))
                    .text_color(rgb(MUTED))
                    .child(t(french, "Tes quotas ChatGPT, Claude, Gemini, Grok et OpenCode, dans une petite encoche au bord de l’écran. La connexion se fait chez le fournisseur : CodeNotch ne voit jamais ton mot de passe.")),
            );

        let body = div()
            .id("welcome-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(700.))
                    .min_w_0()
                    .px(px(30.))
                    .pt(px(26.))
                    .pb(px(24.))
                    .flex()
                    .flex_col()
                    .children(banner)
                    .child(header)
                    .child(stepper(if count > 0 { 1 } else { 0 }, french))
                    .child(subscriptions),
            );

        let finish = button(
            "welcome-finish",
            Kind::Primary,
            Some("check"),
            t(french, "Terminer"),
            (count > 0).then(|| -> crate::settings::Click {
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx)))
            }),
        );
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(30.))
            .py(px(12.))
            .border_t_1()
            .border_color(rgba(0xffffff12))
            .bg(rgb(0x121214))
            .child(div().flex_1().text_size(px(11.)).text_color(rgb(0x9999a2)).child(match count {
                0 => t(french, "Connecte au moins un compte pour terminer."),
                1 => t(french, "{count} compte suivi").replace("{count}", "1"),
                n => t(french, "{count} comptes suivis").replace("{count}", &n.to_string()),
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .text_color(rgb(0x9999a2))
                    .child(t(french, "Lancer au démarrage de Windows"))
                    .child(toggle("welcome-startup", startup, self.setting(cx, |s| s.launch_at_startup = !s.launch_at_startup))),
            )
            // Once an account is tracked, "Later" would do the same as "Finish".
            .when(count == 0, |d| {
                d.child(button(
                    "welcome-later",
                    Kind::Secondary,
                    None,
                    t(french, "Plus tard"),
                    Some(Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.finish_onboarding(cx)))),
                ))
            })
            .child(finish);

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(body)
            .child(footer)
            .into_any_element()
    }
}

/// The three first-run steps as a strip: done ones checked, the current one lit.
pub(crate) fn stepper(active: usize, french: bool) -> AnyElement {
    let steps = [
        ("Connecte", "Un compte suffit pour commencer."),
        ("Survole", "Passe la souris sur la languette au bord de l’écran."),
        ("Ajuste", "Position, alertes et autres comptes dans les réglages."),
    ];
    let last = steps.len() - 1;
    div()
        .mb(px(24.))
        .flex()
        .gap(px(12.))
        .children(steps.into_iter().enumerate().map(move |(i, (title, text))| {
            let (done, current) = (i < active, i == active);
            let badge = div()
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .when(done, |d| d.bg(rgba(0x7fd69b26)).child(icon("check", 12., 0x7fd69b)))
                .when(current, |d| d.bg(rgb(0xe7e7eb)).text_color(rgb(0x17171a)).child((i + 1).to_string()))
                .when(!done && !current, |d| {
                    d.border_1().border_color(rgb(0x414147)).text_color(rgb(0x77777f)).child((i + 1).to_string())
                });
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(badge)
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if current {
                                    rgb(0xffffff)
                                } else if done {
                                    rgb(0xc3c3c8)
                                } else {
                                    rgb(0x8f8f96)
                                })
                                .child(t(french, title)),
                        )
                        .when(i < last, |d| {
                            d.child(div().flex_1().h(px(1.)).bg(if done { rgba(0x7fd69b40) } else { rgba(0xffffff14) }))
                        }),
                )
                .child(
                    div()
                        .pl(px(30.))
                        .pr(px(8.))
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(if current { rgb(0x9999a2) } else { rgb(0x6b6b72) })
                        .child(t(french, text)),
                )
        }))
        .into_any_element()
}
