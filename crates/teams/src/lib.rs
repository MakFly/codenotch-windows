//! # codenotch-teams
//!
//! Opt-in sync of quota and usage figures to a CodeNotch Teams server. Plain library: no UI,
//! no thread of its own; the engine drives it from one worker thread, and only while Teams
//! sync is on and the device is paired. While it is off nothing here runs: no request, no
//! file.
//!
//! | module | what |
//! |---|---|
//! | [`origin`] | the one origin Teams talks to; invite links |
//! | [`filter`] | [`Policy`]: which accounts and scan roots may be shared; translation into closed sets |
//! | [`journal`] | `teams-outbox.jsonl`, the durable queue bound to one pairing |
//! | [`state`] | `teams.json`: pairing and cursors (no secret) |
//! | [`sync`] | [`TeamsSync`]: detection, revisions, tombstones, re-filter, upload |
//! | [`pairing`] | invite preview, pairing, account declaration, detach, erase |
//!
//! What leaves the device (contract: `codenotch-teams-schema`): provider, plan code, quota
//! window ids with percent used and reset times, token counts per model and UTC hour, queue
//! counters, an account reference derived on the device. Nothing else: no prompt, code,
//! response, path, folder or repository name, e-mail, login or token is read by this crate
//! beyond what [`filter`] lists, and every outgoing string is a value of a closed set.
//!
//! Driving it (engine):
//! 1. pairing: [`origin::Origin::admitted`] → [`origin::InviteCode::parse`] →
//!    [`pairing::preview`] → [`pairing::pair`] → token to the Credential Manager →
//!    [`TeamsSync::create`];
//! 2. on start and whenever the config or the scan roots change: [`TeamsSync::open`],
//!    [`TeamsSync::set_policy`];
//! 3. on each fresh reading [`TeamsSync::observe_reading`]; on each analytics report
//!    [`TeamsSync::observe_usage`]; every 15 min [`TeamsSync::heartbeat`];
//! 4. at most every [`sync::MIN_SEND_INTERVAL_SECS`] and not before
//!    [`TeamsSync::retry_at`]: [`TeamsSync::send`]; publish [`TeamsSync::status`].
//! 5. detach: [`pairing::detach`] (best effort), then [`TeamsSync::detach_local`] and delete
//!    the token.

mod fault;
pub mod filter;
pub mod journal;
pub mod origin;
pub mod pairing;
pub mod state;
pub mod sync;

pub use filter::{account_ref, Policy};
pub use origin::{InviteCode, Origin, OriginError};
pub use pairing::{InvitePreview, PairError, Paired};
pub use sync::{PairingInfo, PauseReason, SendOutcome, TeamsSync};

/// App version sent in batches and at pairing.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// English i18n keys put in `TeamsStatus::last_error` (the settings window translates them).
pub mod i18n {
    pub const REVOKED: &str = "Device revoked by the team";
    pub const PAUSED: &str = "Sync paused, retrying";
    pub const UPDATE: &str = "Update CodeNotch to keep syncing";
    pub const NOT_A_SERVER: &str = "This server is not a CodeNotch Teams server";
    pub const CONSENT: &str = "The team changed what it sees. Review and accept to resume.";
    pub const STORAGE: &str = "Cannot save Teams sync data";
}
