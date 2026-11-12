//! What the user sees of the self-updater: in-app toasts driven by `Snapshot::update`.
//!
//! Toasts go to the current toast host — the settings window when it is open, else the small
//! toast window near the notch (`codenotch_ui_notch::toast_host`). The decision of what to show
//! for a state change is pure ([`plan`], unit tested); [`install`] wires it to the model.
//!
//! * a check from the settings button: "Checking…" → "Up to date" / the offer / an error;
//! * a new version (automatic check, notify mode): "CodeNotch 1.1.0 is available" with
//!   **Update** and **Notes** (once per version and session);
//! * after **Update**: "Downloading… 42 %" → "Restarting…";
//! * a forced update: a sticky, non-dismissable "Required update to 1.1.0… 42 %";
//! * after the restart (`--updated-from=`): "Updated to 1.1.0".

use crate::i18n::tf;
use codenotch_core::model::{Command, UpdateState};
use codenotch_core::Lang;
use codenotch_ui_kit::toast::{ToastId, ToastKind, ToastOptions, Toaster};
use codenotch_ui_kit::UiModel;
use gpui::{App, Entity, WeakEntity};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How long result toasts stay.
const RESULT_MS: u64 = 5000;
const ERROR_MS: u64 = 8000;
/// Delay of the "Updated to …" toast after start (the notch places itself first).
const WELCOME_DELAY: Duration = Duration::from_millis(1500);

/// One toast to show (or none) for a state change.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    None,
    Loading {
        text: String,
        persistent: bool,
    },
    Success(String),
    Error(String),
    Offer {
        version: String,
        text: String,
        notes_url: Option<String>,
    },
}

/// What the controller remembers between states.
#[derive(Debug, Default)]
pub struct Memory {
    /// Versions already offered this session (an automatic re-check does not offer again).
    pub offered: Vec<String>,
    /// The user pressed **Update**: progress and failures are shown.
    pub install_requested: bool,
}

/// The toast for the change `prev` → `next`.
pub fn plan(lang: Lang, prev: &UpdateState, next: &UpdateState, mem: &mut Memory) -> Plan {
    let pct = |n: &u8| n.to_string();
    match next {
        UpdateState::Idle | UpdateState::Ready { .. } => Plan::None,
        UpdateState::Checking { manual } => {
            if *manual {
                Plan::Loading {
                    text: tf(lang, "Checking for updates…", &[]),
                    persistent: false,
                }
            } else {
                Plan::None
            }
        }
        UpdateState::UpToDate { manual, .. } => {
            if *manual {
                Plan::Success(tf(
                    lang,
                    "CodeNotch is up to date ({version}).",
                    &[("version", codenotch_core::VERSION)],
                ))
            } else {
                Plan::None
            }
        }
        UpdateState::Available {
            version,
            notes_url,
            forced,
            manual,
            ..
        } => {
            if *forced {
                return Plan::Loading {
                    text: tf(
                        lang,
                        "Required update to {version}…",
                        &[("version", version)],
                    ),
                    persistent: true,
                };
            }
            if !*manual && mem.offered.contains(version) {
                return Plan::None;
            }
            if !mem.offered.contains(version) {
                mem.offered.push(version.clone());
            }
            Plan::Offer {
                version: version.clone(),
                text: tf(
                    lang,
                    "CodeNotch {version} is available",
                    &[("version", version)],
                ),
                notes_url: notes_url.clone(),
            }
        }
        UpdateState::Downloading {
            version,
            progress,
            forced,
        } => {
            if *forced {
                Plan::Loading {
                    text: tf(
                        lang,
                        "Required update to {version}… {n}%",
                        &[("version", version), ("n", &pct(progress))],
                    ),
                    persistent: true,
                }
            } else if mem.install_requested {
                Plan::Loading {
                    text: tf(lang, "Downloading… {n}%", &[("n", &pct(progress))]),
                    persistent: false,
                }
            } else {
                Plan::None
            }
        }
        UpdateState::Installing { .. } => Plan::Loading {
            text: tf(lang, "Restarting…", &[]),
            persistent: true,
        },
        UpdateState::Failed { message, manual } => {
            let during_update = matches!(
                prev,
                UpdateState::Downloading { .. }
                    | UpdateState::Installing { .. }
                    | UpdateState::Ready { .. }
            );
            if !(*manual || mem.install_requested || during_update) {
                return Plan::None;
            }
            mem.install_requested = false;
            let msg = codenotch_core::tr(lang, message);
            let key = if matches!(prev, UpdateState::Checking { .. }) {
                "Could not check for updates: {msg}"
            } else {
                "Update failed: {msg}"
            };
            Plan::Error(tf(lang, key, &[("msg", msg)]))
        }
    }
}

/// The "Updated to …" text after a restart, `None` when nothing changed.
pub fn welcome(lang: Lang, updated_from: &str) -> Option<String> {
    let from = codenotch_engine::Version::parse(updated_from)?;
    let now = codenotch_engine::Version::parse(codenotch_core::VERSION)?;
    (from != now).then(|| {
        tf(
            lang,
            "Updated to {version}",
            &[("version", codenotch_core::VERSION)],
        )
    })
}

struct Controller {
    model: Entity<UiModel>,
    last: UpdateState,
    mem: Memory,
    /// The update toast on screen: its toaster and id.
    toast: Option<(WeakEntity<Toaster>, ToastId)>,
    /// Set by the toast's **Update** button.
    requested: Rc<Cell<bool>>,
}

impl Controller {
    fn on_state(&mut self, next: UpdateState, cx: &mut App) {
        if next == self.last {
            return;
        }
        let prev = std::mem::replace(&mut self.last, next.clone());
        if self.requested.take() {
            self.mem.install_requested = true;
        }
        let lang = self.model.read(cx).snapshot.lang();
        let p = plan(lang, &prev, &next, &mut self.mem);
        self.show(p, cx);
    }

    fn options(&self, p: Plan, lang: Lang) -> Option<ToastOptions> {
        Some(match p {
            Plan::None => return None,
            Plan::Loading { text, persistent } => {
                let o = ToastOptions::new(ToastKind::Loading, text).sticky();
                if persistent {
                    o.persistent()
                } else {
                    o
                }
            }
            Plan::Success(text) => ToastOptions::new(ToastKind::Success, text).duration(RESULT_MS),
            Plan::Error(text) => ToastOptions::new(ToastKind::Error, text).duration(ERROR_MS),
            Plan::Offer {
                text, notes_url, ..
            } => {
                let model = self.model.clone();
                let requested = self.requested.clone();
                let mut o = ToastOptions::new(ToastKind::Blank, text).sticky().action(
                    tf(lang, "Update", &[]),
                    move |_, cx| {
                        requested.set(true);
                        model.read(cx).send(Command::InstallUpdate);
                    },
                );
                if let Some(url) = notes_url {
                    let model = self.model.clone();
                    o = o.action(tf(lang, "Notes", &[]), move |_, cx| {
                        model.read(cx).send(Command::OpenUrl(url.clone()));
                    });
                }
                o
            }
        })
    }

    fn show(&mut self, p: Plan, cx: &mut App) {
        let lang = self.model.read(cx).snapshot.lang();
        let Some(options) = self.options(p, lang) else {
            return;
        };
        let current = self
            .toast
            .as_ref()
            .and_then(|(weak, id)| Some((weak.upgrade()?, *id)));
        let (toaster, id) = match current {
            Some((t, id)) => (t, Some(id)),
            None => match codenotch_ui_notch::toast_host(&self.model, cx) {
                Some(t) => (t, None),
                None => return,
            },
        };
        let shown = toaster.update(cx, |t, cx| t.show_or_replace(id, options, cx));
        self.toast = Some((toaster.downgrade(), shown));
    }
}

/// Drive the update toasts from `model`. `updated_from`: the `--updated-from=` flag of a
/// restart after an update.
pub fn install(model: Entity<UiModel>, updated_from: Option<String>, cx: &mut App) {
    let initial = model.read(cx).snapshot.update.clone();
    let controller = Rc::new(RefCell::new(Controller {
        model: model.clone(),
        last: initial,
        mem: Memory::default(),
        toast: None,
        requested: Rc::new(Cell::new(false)),
    }));
    {
        let controller = controller.clone();
        cx.observe(&model, move |model, cx| {
            let state = model.read(cx).snapshot.update.clone();
            controller.borrow_mut().on_state(state, cx);
        })
        .detach();
    }
    if let Some(from) = updated_from {
        let lang = model.read(cx).snapshot.lang();
        if let Some(text) = welcome(lang, &from) {
            log::info!(
                "update: running {} (updated from {from})",
                codenotch_core::VERSION
            );
            cx.spawn(async move |cx| {
                cx.background_executor().timer(WELCOME_DELAY).await;
                let _ = cx.update(|cx| {
                    let mut c = controller.borrow_mut();
                    c.show(Plan::Success(text), cx);
                });
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn available(version: &str, forced: bool, manual: bool) -> UpdateState {
        UpdateState::Available {
            version: version.into(),
            notes: String::new(),
            notes_url: Some("https://github.com/x".into()),
            forced,
            manual,
        }
    }

    #[test]
    fn manual_check_shows_every_outcome() {
        let mut m = Memory::default();
        let idle = UpdateState::Idle;
        let checking = UpdateState::Checking { manual: true };
        assert!(matches!(
            plan(Lang::Fr, &idle, &checking, &mut m),
            Plan::Loading { ref text, persistent: false } if text == "Recherche de mises à jour…"
        ));
        let up = UpdateState::UpToDate {
            checked_at: 0,
            manual: true,
        };
        assert!(
            matches!(plan(Lang::En, &checking, &up, &mut m), Plan::Success(t) if t.contains("up to date"))
        );
        let failed = UpdateState::Failed {
            message: "No update information published yet.".into(),
            manual: true,
        };
        assert!(matches!(
            plan(Lang::En, &checking, &failed, &mut m),
            Plan::Error(t) if t.starts_with("Could not check for updates")
        ));
        // Automatic checks stay quiet unless there is news.
        let auto = UpdateState::Checking { manual: false };
        assert_eq!(plan(Lang::En, &idle, &auto, &mut m), Plan::None);
        let up_auto = UpdateState::UpToDate {
            checked_at: 0,
            manual: false,
        };
        assert_eq!(plan(Lang::En, &auto, &up_auto, &mut m), Plan::None);
        let failed_auto = UpdateState::Failed {
            message: "offline".into(),
            manual: false,
        };
        assert_eq!(plan(Lang::En, &auto, &failed_auto, &mut m), Plan::None);
    }

    #[test]
    fn offers_once_per_version_unless_asked() {
        let mut m = Memory::default();
        let auto = UpdateState::Checking { manual: false };
        let p = plan(Lang::Fr, &auto, &available("1.1.0", false, false), &mut m);
        assert!(matches!(
            p,
            Plan::Offer { ref text, ref notes_url, .. }
                if text == "CodeNotch 1.1.0 est disponible" && notes_url.is_some()
        ));
        assert_eq!(
            plan(Lang::En, &auto, &available("1.1.0", false, false), &mut m),
            Plan::None
        );
        assert!(matches!(
            plan(Lang::En, &auto, &available("1.1.0", false, true), &mut m),
            Plan::Offer { .. }
        ));
        assert!(matches!(
            plan(Lang::En, &auto, &available("1.2.0", false, false), &mut m),
            Plan::Offer { .. }
        ));
    }

    #[test]
    fn requested_install_shows_progress_and_restart() {
        let mut m = Memory::default();
        let dl = |n| UpdateState::Downloading {
            version: "1.1.0".into(),
            progress: n,
            forced: false,
        };
        // Background download (auto mode): silent.
        assert_eq!(
            plan(Lang::En, &UpdateState::Idle, &dl(10), &mut m),
            Plan::None
        );
        m.install_requested = true;
        assert!(matches!(
            plan(Lang::Fr, &dl(10), &dl(42), &mut m),
            Plan::Loading { ref text, persistent: false } if text == "Téléchargement… 42 %"
        ));
        let installing = UpdateState::Installing {
            version: "1.1.0".into(),
        };
        assert!(matches!(
            plan(Lang::Fr, &dl(100), &installing, &mut m),
            Plan::Loading { ref text, persistent: true } if text == "Redémarrage…"
        ));
        let failed = UpdateState::Failed {
            message: "The update package is not valid.".into(),
            manual: false,
        };
        assert!(
            matches!(plan(Lang::En, &dl(100), &failed, &mut m), Plan::Error(t) if t.starts_with("Update failed"))
        );
        assert!(!m.install_requested, "reset after a failure");
    }

    #[test]
    fn forced_update_is_sticky_and_not_dismissable() {
        let mut m = Memory::default();
        assert!(matches!(
            plan(Lang::Fr, &UpdateState::Idle, &available("2.0.0", true, false), &mut m),
            Plan::Loading { ref text, persistent: true } if text == "Mise à jour obligatoire vers 2.0.0…"
        ));
        let dl = UpdateState::Downloading {
            version: "2.0.0".into(),
            progress: 7,
            forced: true,
        };
        assert!(matches!(
            plan(Lang::En, &available("2.0.0", true, false), &dl, &mut m),
            Plan::Loading { ref text, persistent: true } if text == "Required update to 2.0.0… 7%"
        ));
    }

    #[test]
    fn welcome_only_after_a_real_change() {
        assert_eq!(welcome(Lang::En, codenotch_core::VERSION), None);
        assert_eq!(welcome(Lang::En, "not a version"), None);
        assert_eq!(
            welcome(Lang::Fr, "0.0.1"),
            Some(format!("Mis à jour vers {}", codenotch_core::VERSION))
        );
    }
}
