//! [`UiModel`]: the one gpui entity every window observes.
//!
//! Threading:
//! * The engine runs on its own thread and publishes [`Event`]s through an [`EventSink`].
//! * [`UiModel::spawn_pump`] returns that sink. Internally it is a `futures` unbounded channel:
//!   `unbounded_send` is non-blocking and callable from any thread; the receiving end is awaited
//!   by a task on gpui's **foreground** executor, so every update runs on the UI thread.
//! * The UI sends [`Command`]s with [`UiModel::send`] (a `std::sync::mpsc::Sender`, never blocks).
//!
//! Windows subscribe with `cx.observe(&model, …)` for snapshot changes and
//! `cx.subscribe(&model, |_, _, event: &Event, cx| …)` for one-shot events (reset card, alerts,
//! open settings, notices, quit). `Event::SnapshotChanged` is consumed by the model itself and
//! is not re-emitted.

use codenotch_core::{Command, Event, EventSink, Snapshot};
use futures::StreamExt;
use gpui::{App, AppContext, Context, Entity, EventEmitter};
use std::sync::mpsc::Sender;

pub struct UiModel {
    pub snapshot: Snapshot,
    tx: Sender<Command>,
}

impl EventEmitter<Event> for UiModel {}

impl UiModel {
    pub fn new(snapshot: Snapshot, tx: Sender<Command>) -> Self {
        Self { snapshot, tx }
    }

    /// Create the entity.
    pub fn create(snapshot: Snapshot, tx: Sender<Command>, cx: &mut App) -> Entity<UiModel> {
        cx.new(|_| UiModel::new(snapshot, tx))
    }

    /// Replace the snapshot and re-render observers.
    pub fn apply(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            cx.notify();
        }
    }

    /// Send a command to the engine. Never blocks; logs when the engine is gone.
    pub fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            log::warn!("engine channel closed; command dropped");
        }
    }

    /// Clone of the command sender (for tray/hotkey callbacks).
    pub fn sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    /// Handle one engine event on the UI thread.
    pub fn handle_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::SnapshotChanged(snapshot) => self.apply(*snapshot, cx),
            other => cx.emit(other),
        }
    }

    /// Start forwarding engine events onto the gpui main thread. Give the returned sink to the
    /// engine. The pump stops when every sink clone is dropped or the model is released.
    pub fn spawn_pump(model: &Entity<UiModel>, cx: &mut App) -> EventSink {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();
        let weak = model.downgrade();
        cx.spawn(async move |cx| {
            while let Some(event) = rx.next().await {
                let Some(model) = weak.upgrade() else { break };
                if model.update(cx, |m, cx| m.handle_event(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        EventSink::new(move |event| {
            let _ = tx.unbounded_send(event);
        })
    }
}
