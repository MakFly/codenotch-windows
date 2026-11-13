//! Failure injection for the crash tests: a test arms one step, the next time the code reaches
//! that step it fails as if the process died or the disk was full there. Compiled to nothing
//! outside `cfg(test)`.

use std::io;

/// Steps of the durable protocol, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Writing the lines to the outbox.
    Append,
    /// `sync_data` of the outbox.
    Sync,
    /// After the cursors advanced in memory, before `teams.json` is written.
    Cursor,
    /// Writing `teams.json`.
    State,
    /// Rewriting the outbox.
    Compaction,
}

#[cfg(test)]
thread_local! {
    static ARMED: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
}

/// Fail once at `step` (this thread only).
#[cfg(test)]
pub(crate) fn arm(step: Step) {
    ARMED.with(|a| a.set(Some(step)));
}

#[cfg(test)]
pub(crate) fn fault(step: Step) -> io::Result<()> {
    if ARMED.with(|a| a.get()) == Some(step) {
        ARMED.with(|a| a.set(None));
        return Err(io::Error::other("injected failure"));
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
pub(crate) fn fault(_step: Step) -> io::Result<()> {
    Ok(())
}
