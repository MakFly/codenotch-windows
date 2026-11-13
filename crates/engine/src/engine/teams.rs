//! Teams sync wiring (see `codenotch_teams`).
//!
//! The engine thread keeps the decisions: whether sync is active, the pairing generation and
//! the published status. Storage, compaction, batch preparation and HTTP run on ONE worker
//! thread (`codenotch-teams`): observations (readings, hourly usage) go through a bounded
//! queue of [`QUEUE_CAP`] jobs (a full queue drops the observation, the next one replaces
//! it); control jobs (pairing, detach, policy …) always get through.
//!
//! Gate: no worker, no request and no `teams*` file unless `teamsSyncEnabled` is on and the
//! device is paired (`teams.json` present) with a device token in the store. The pairing
//! commands also need the switch on. Detaching while the switch is off only deletes the local
//! files. `--demo` (no engine) and `--once` ([`crate::fetch_once`]) never get here.
//!
//! Generations: every job and every report carries the generation it was made for. Turning
//! sync off, detaching and a new pairing bump it; the worker skips jobs of an older generation
//! and re-checks it after each request; the engine drops reports of an older one (a late
//! pairing result is never stored).

use super::{Engine, Msg};
use crate::Clock;
use codenotch_core::analytics::{HourlyUsage, RootOwners};
use codenotch_core::model::{
    Command, Reading, ReadingStatus, TeamsInvitePreview, TeamsPayer, TeamsState, TeamsStatus,
};
use codenotch_core::secrets::{SecretKey, SecretValue};
use codenotch_core::{Config, Env, HttpClient, Paths};
use codenotch_teams::sync::MIN_SEND_INTERVAL_SECS;
use codenotch_teams::{
    pairing, InviteCode, Origin, OriginError, PairError, PairingInfo, Policy, SendOutcome,
    TeamsSync,
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Observations waiting for the worker at most.
pub(crate) const QUEUE_CAP: usize = 64;
/// Longest wait for the worker at shutdown.
const STOP_WAIT: Duration = Duration::from_secs(2);

// English i18n keys (engine notices: `crate::i18n::FR`; status: the settings window's table).
pub(crate) const MSG_TURN_ON: &str = "Turn on Team sync first.";
pub(crate) const MSG_DETACH_FIRST: &str = "Detach this device first.";
pub(crate) const MSG_PREVIEW_FIRST: &str = "Read the invite again before connecting.";
pub(crate) const MSG_NOT_PAIRED: &str = "This device is not connected to a team.";
pub(crate) const MSG_PAIRED: &str = "Connected to the team.";
pub(crate) const MSG_DETACHED: &str = "Device detached from the team.";
pub(crate) const MSG_DECLARED: &str = "Declaration sent to the team.";
pub(crate) const MSG_ERASED: &str = "Your data on the team server is being erased.";
pub(crate) const MSG_NO_ORIGIN: &str = "Set the Teams server address first.";
pub(crate) const MSG_BAD_ORIGIN: &str = "Invalid Teams server address.";
pub(crate) const MSG_OTHER_SERVER: &str = "This invite link is for another server.";
pub(crate) const MSG_BAD_INVITE: &str = "Invalid invite link.";
pub(crate) const MSG_INVITE_EXPIRED: &str = "This invite link is invalid or has expired.";
pub(crate) const MSG_UNREACHABLE: &str = "Cannot reach the Teams server.";
pub(crate) const MSG_SERVER_FAILED: &str = "The Teams server could not handle the request.";
pub(crate) const MSG_REFUSED: &str = "Check the account and the price.";
pub(crate) const MSG_TOKEN_NOT_SAVED: &str = "The device token could not be saved.";
/// Status keys.
pub(crate) const MSG_NOT_STARTED: &str = "Teams sync could not start";
pub(crate) const MSG_ORIGIN_CHANGED: &str =
    "The Teams server address changed. Detach this device to pair again.";

/// Every English key the engine can put in a notice or in `TeamsStatus::last_error`.
#[cfg(test)]
pub(crate) const MESSAGES: &[&str] = &[
    MSG_TURN_ON,
    MSG_DETACH_FIRST,
    MSG_PREVIEW_FIRST,
    MSG_NOT_PAIRED,
    MSG_PAIRED,
    MSG_DETACHED,
    MSG_DECLARED,
    MSG_ERASED,
    MSG_NO_ORIGIN,
    MSG_BAD_ORIGIN,
    MSG_OTHER_SERVER,
    MSG_BAD_INVITE,
    MSG_INVITE_EXPIRED,
    MSG_UNREACHABLE,
    MSG_SERVER_FAILED,
    MSG_REFUSED,
    MSG_TOKEN_NOT_SAVED,
    MSG_NOT_STARTED,
    MSG_ORIGIN_CHANGED,
    codenotch_teams::i18n::REVOKED,
    codenotch_teams::i18n::PAUSED,
    codenotch_teams::i18n::UPDATE,
    codenotch_teams::i18n::NOT_A_SERVER,
    codenotch_teams::i18n::CONSENT,
    codenotch_teams::i18n::STORAGE,
];

fn origin_message(e: OriginError) -> &'static str {
    match e {
        OriginError::NotConfigured => MSG_NO_ORIGIN,
        OriginError::Invalid => MSG_BAD_ORIGIN,
        OriginError::NotAdmitted => MSG_OTHER_SERVER,
        OriginError::BadInvite => MSG_BAD_INVITE,
    }
}

fn pair_message(e: PairError) -> &'static str {
    match e {
        PairError::InvalidInvite => MSG_INVITE_EXPIRED,
        PairError::Network => MSG_UNREACHABLE,
        PairError::Server(_) => MSG_SERVER_FAILED,
        PairError::Refused => MSG_REFUSED,
        other => other.message(),
    }
}

// ---------------------------------------------------------------------------------------------
// Messages between the engine and the worker
// ---------------------------------------------------------------------------------------------

/// Work for the worker.
pub(crate) enum Job {
    /// Load the existing pairing (`teams.json`); the token is kept in memory for uploads.
    Open {
        token: SecretValue,
    },
    /// Start a new pairing (writes `teams.json`, deletes any previous outbox).
    Activate {
        info: Box<PairingInfo>,
        token: SecretValue,
    },
    /// Current config: shared accounts, roots (computed on the worker), origin check.
    Policy(Box<Config>),
    Reading(Box<Reading>),
    Usage(Arc<Vec<HourlyUsage>>),
    /// Upload now ("Send now"), not waiting for the 60 s spacing or a short backoff.
    Flush,
    Backfill(bool),
    Preview {
        origin: Origin,
        code: InviteCode,
    },
    Pair {
        origin: Origin,
        code: InviteCode,
        consent_version: u32,
    },
    Declare {
        account_id: String,
        payer: TeamsPayer,
        price_eur_month: Option<f64>,
    },
    Erase,
    /// Best-effort detach call (only with a token), then `teams.json` and the outbox deleted.
    Detach {
        token: Option<SecretValue>,
    },
    Stop,
}

struct Envelope {
    generation: u64,
    job: Job,
    /// Counted in the bounded observation queue.
    observation: bool,
}

/// A worker result, for the generation it carries ([`Msg::Teams`]).
pub(crate) enum Report {
    Status(TeamsStatus),
    /// `Open` found no pairing.
    NotPaired,
    Preview(Result<TeamsInvitePreview, &'static str>, u32),
    Paired(Result<Box<Paired>, &'static str>),
    Detached,
    Declared(Result<(), &'static str>),
    Erased(Result<(), &'static str>),
}

/// A successful pairing, before the engine accepts it (token not stored yet).
pub(crate) struct Paired {
    info: PairingInfo,
    token: SecretValue,
}

// ---------------------------------------------------------------------------------------------
// Engine side
// ---------------------------------------------------------------------------------------------

struct Link {
    tx: Sender<Envelope>,
    pending: Arc<AtomicUsize>,
    done: Receiver<()>,
}

/// An invite read and shown, waiting for the member's confirmation.
struct Invite {
    value: SecretValue,
    consent_version: u32,
    preview: TeamsInvitePreview,
}

/// Teams state owned by the engine thread.
pub(crate) struct Teams {
    generation: Arc<AtomicU64>,
    link: Option<Link>,
    /// Generation for which the pairing was opened on the worker (`Open` / `Activate` sent).
    opened: Option<u64>,
    /// Config last sent as `Policy`.
    policy: Option<Config>,
    /// Invite being previewed (sent, no answer yet).
    previewing: Option<SecretValue>,
    invite: Option<Invite>,
    busy: bool,
    /// Last status the worker reported for the opened pairing.
    status: Option<TeamsStatus>,
    /// Observations dropped because the queue was full.
    pub(crate) dropped: u64,
    start_failed: bool,
}

impl Teams {
    pub(crate) fn new(now: i64) -> Self {
        Self {
            // Seeded from the clock: the pairing generation also binds the outbox, so it keeps
            // growing across runs.
            generation: Arc::new(AtomicU64::new(u64::try_from(now).unwrap_or(1).max(1))),
            link: None,
            opened: None,
            policy: None,
            previewing: None,
            invite: None,
            busy: false,
            status: None,
            dropped: 0,
            start_failed: false,
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn bump(&mut self) -> u64 {
        self.busy = false;
        self.previewing = None;
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The pairing is open on the worker for the current generation.
    fn active(&self) -> bool {
        self.link.is_some() && self.opened == Some(self.generation())
    }

    #[cfg(test)]
    pub(crate) fn worker_running(&self) -> bool {
        self.link.is_some()
    }
}

impl Engine {
    fn teams_token(&self) -> Option<SecretValue> {
        self.opts
            .secrets
            .get(&SecretKey::TeamsDeviceToken)
            .filter(|t| !t.trim().is_empty())
            .map(SecretValue::new)
    }

    fn teams_enabled(&self) -> bool {
        self.snap.config.settings.teams_sync_enabled
    }

    /// Start the worker if it is not running. False (and status `Error`) when it cannot start.
    fn teams_worker(&mut self) -> bool {
        if self.teams.link.is_some() {
            return true;
        }
        let (tx, rx) = std::sync::mpsc::channel::<Envelope>();
        let (done_tx, done) = std::sync::mpsc::channel::<()>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker = Worker {
            paths: self.opts.paths.clone(),
            env: self.opts.env.clone(),
            http: self.opts.http.clone(),
            clock: self.clock.clone(),
            current: self.teams.generation.clone(),
            pending: pending.clone(),
            tx: self.tx.clone(),
            sync: None,
            sync_gen: 0,
            token: None,
            config: None,
            owners: Vec::new(),
            origin_mismatch: false,
            last_attempt: None,
            flush: false,
            long_pause: false,
            halted: false,
            published: None,
        };
        let wake = self.tuning.teams_wake;
        let spawned = std::thread::Builder::new()
            .name("codenotch-teams".into())
            .spawn(move || {
                worker.run(rx, wake);
                let _ = done_tx.send(());
            });
        match spawned {
            Ok(_) => {
                self.teams.start_failed = false;
                self.teams.link = Some(Link { tx, pending, done });
                true
            }
            Err(err) => {
                log::error!("cannot start the teams worker: {err}");
                self.teams.start_failed = true;
                false
            }
        }
    }

    fn teams_stop_worker(&mut self) {
        if let Some(link) = self.teams.link.take() {
            let _ = link.tx.send(Envelope {
                generation: self.teams.generation(),
                job: Job::Stop,
                observation: false,
            });
        }
        self.teams.opened = None;
        self.teams.policy = None;
        self.teams.status = None;
    }

    /// Send a control job (never dropped). False when the worker is gone.
    fn teams_send(&mut self, job: Job) -> bool {
        let generation = self.teams.generation();
        let sent = self.teams.link.as_ref().is_some_and(|l| {
            l.tx.send(Envelope {
                generation,
                job,
                observation: false,
            })
            .is_ok()
        });
        if !sent && self.teams.link.take().is_some() {
            log::warn!("teams worker stopped");
            self.teams.start_failed = true;
            self.teams.opened = None;
        }
        sent
    }

    /// Queue an observation if the pairing is active and the queue has room.
    fn teams_observe(&mut self, job: Job) {
        if !self.teams.active() {
            return;
        }
        let generation = self.teams.generation();
        let Some(link) = &self.teams.link else {
            return;
        };
        if link.pending.load(Ordering::SeqCst) >= QUEUE_CAP {
            self.teams.dropped += 1;
            log::debug!("teams: queue full, observation dropped");
            return;
        }
        link.pending.fetch_add(1, Ordering::SeqCst);
        if link
            .tx
            .send(Envelope {
                generation,
                job,
                observation: true,
            })
            .is_err()
        {
            link.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub(super) fn teams_observe_reading(&mut self, reading: &Reading) {
        if reading.status == ReadingStatus::Fresh {
            self.teams_observe(Job::Reading(Box::new(reading.clone())));
        }
    }

    pub(super) fn teams_observe_usage(&mut self, rows: Arc<Vec<HourlyUsage>>) {
        self.teams_observe(Job::Usage(rows));
    }

    /// Re-evaluate the gate after a start, a config change or a pairing change: start / stop
    /// the worker, open the pairing, send the policy, and refresh `snap.teams`.
    pub(super) fn teams_refresh(&mut self) {
        if !self.teams_enabled() {
            if self.teams.link.is_some() || self.teams.opened.is_some() {
                self.teams.bump();
                self.teams_stop_worker();
            }
            self.teams.invite = None;
            self.teams_publish();
            return;
        }
        let token = self.teams_token();
        // Just paired: `teams.json` may not be written yet (the worker creates it).
        let paired = self.teams.active() || self.opts.paths.teams_state().is_file();
        match token {
            Some(token) if paired => {
                if !self.teams.active() {
                    if !self.teams_worker() {
                        self.teams_publish();
                        return;
                    }
                    self.teams.opened = Some(self.teams.generation());
                    self.teams.policy = None;
                    self.teams.status = None;
                    self.teams_send(Job::Open { token });
                }
                if self.teams.policy.as_ref() != Some(&self.snap.config) {
                    self.teams.policy = Some(self.snap.config.clone());
                    self.teams_send(Job::Policy(Box::new(self.snap.config.clone())));
                }
            }
            _ => {
                // Not paired: no worker unless a pairing request is running.
                if self.teams.opened.is_some() {
                    self.teams.opened = None;
                    self.teams.status = None;
                }
                if !self.teams.busy && self.teams.link.is_some() {
                    self.teams_stop_worker();
                }
            }
        }
        self.teams_publish();
    }

    /// `snap.teams` from the engine's view and the worker's last status.
    fn teams_publish(&mut self) {
        let t = &self.teams;
        let mut status = if !self.snap.config.settings.teams_sync_enabled {
            TeamsStatus::default()
        } else if t.start_failed {
            TeamsStatus {
                state: TeamsState::Error,
                last_error: Some(MSG_NOT_STARTED.into()),
                ..TeamsStatus::default()
            }
        } else if t.active() {
            t.status.clone().unwrap_or(TeamsStatus {
                state: TeamsState::Active,
                ..TeamsStatus::default()
            })
        } else {
            TeamsStatus {
                state: TeamsState::NotPaired,
                ..TeamsStatus::default()
            }
        };
        status.preview = t.invite.as_ref().map(|i| i.preview.clone());
        status.busy = t.busy;
        self.snap.teams = status;
    }

    pub(super) fn teams_command(&mut self, command: Command) {
        match command {
            Command::TeamsPreviewInvite(invite) => self.teams_preview(invite),
            Command::TeamsPair(invite) => self.teams_pair(invite),
            Command::TeamsDetach => self.teams_detach(),
            Command::TeamsFlush => {
                if self.teams.active() {
                    self.teams_send(Job::Flush);
                }
            }
            Command::TeamsAllowBackfill(allow) => {
                if self.teams.active() {
                    self.teams_send(Job::Backfill(allow));
                }
            }
            Command::TeamsDeclareAccount {
                account_id,
                payer,
                price_eur_month,
            } => {
                if !self.teams.active() {
                    self.notice(MSG_NOT_PAIRED);
                } else if self.teams_send(Job::Declare {
                    account_id,
                    payer,
                    price_eur_month,
                }) {
                    self.teams.busy = true;
                }
            }
            Command::TeamsEraseServerData => {
                if !self.teams.active() {
                    self.notice(MSG_NOT_PAIRED);
                } else if self.teams_send(Job::Erase) {
                    self.teams.busy = true;
                }
            }
            _ => {}
        }
        self.teams_publish();
    }

    /// Checks shared by preview and pair: switch on, not paired, invite readable for the
    /// admitted origin. Pure (no request).
    fn teams_invite(&mut self, invite: &SecretValue) -> Option<(Origin, InviteCode)> {
        if !self.teams_enabled() {
            self.notice(MSG_TURN_ON);
            return None;
        }
        if self.teams.active() || self.opts.paths.teams_state().is_file() {
            self.notice(MSG_DETACH_FIRST);
            return None;
        }
        let parsed = Origin::admitted(&self.snap.config.settings).and_then(|origin| {
            InviteCode::parse(invite.expose(), &origin).map(|code| (origin, code))
        });
        match parsed {
            Ok(p) => Some(p),
            Err(e) => {
                self.notice(origin_message(e));
                None
            }
        }
    }

    fn teams_preview(&mut self, invite: SecretValue) {
        self.teams.invite = None;
        let Some((origin, code)) = self.teams_invite(&invite) else {
            return;
        };
        if self.teams_worker() && self.teams_send(Job::Preview { origin, code }) {
            self.teams.busy = true;
            self.teams.previewing = Some(invite);
        }
    }

    fn teams_pair(&mut self, invite: SecretValue) {
        let consent_version = match &self.teams.invite {
            Some(i) if i.value.expose().trim() == invite.expose().trim() => i.consent_version,
            _ => {
                self.notice(MSG_PREVIEW_FIRST);
                return;
            }
        };
        let Some((origin, code)) = self.teams_invite(&invite) else {
            return;
        };
        if self.teams_worker()
            && self.teams_send(Job::Pair {
                origin,
                code,
                consent_version,
            })
        {
            self.teams.busy = true;
        }
    }

    fn teams_detach(&mut self) {
        let token = self.teams_token();
        let paired = self.opts.paths.teams_state().is_file();
        self.teams.invite = None;
        if token.is_none() && !paired && !self.teams.active() {
            self.notice(MSG_NOT_PAIRED);
            return;
        }
        if let Err(err) = self.opts.secrets.delete(&SecretKey::TeamsDeviceToken) {
            log::warn!("teams: device token not deleted: {err}");
        }
        // Results still in flight for the old pairing are dropped from now on.
        self.teams.bump();
        self.teams.opened = None;
        self.teams.policy = None;
        self.teams.status = None;
        // Sync off: local files only, no request.
        let token = token.filter(|_| self.teams_enabled());
        if self.teams_worker() && self.teams_send(Job::Detach { token }) {
            self.teams.busy = true;
        }
    }

    /// A worker report.
    pub(super) fn on_teams(&mut self, generation: u64, report: Report) {
        if generation != self.teams.generation() {
            log::debug!("teams: stale result dropped");
            return;
        }
        match report {
            Report::Status(status) => {
                // Frequent: no gate re-check (it reads the credential store).
                if self.teams.active() {
                    self.teams.status = Some(status);
                }
                self.teams_publish();
                return;
            }
            Report::NotPaired => {
                self.teams.opened = None;
                self.teams.status = None;
            }
            Report::Preview(result, consent_version) => {
                self.teams.busy = false;
                let value = self.teams.previewing.take();
                match (result, value) {
                    (Ok(preview), Some(value)) => {
                        self.teams.invite = Some(Invite {
                            value,
                            consent_version,
                            preview,
                        })
                    }
                    (Err(key), _) => self.notice(key),
                    (Ok(_), None) => {}
                }
            }
            Report::Paired(result) => {
                self.teams.busy = false;
                match result {
                    Ok(paired) => self.teams_paired(*paired),
                    Err(key) => self.notice(key),
                }
            }
            Report::Detached => {
                self.teams.busy = false;
                self.notice(MSG_DETACHED);
            }
            Report::Declared(result) => {
                self.teams.busy = false;
                self.notice(result.err().unwrap_or(MSG_DECLARED));
            }
            Report::Erased(result) => {
                self.teams.busy = false;
                self.notice(result.err().unwrap_or(MSG_ERASED));
            }
        }
        self.teams_refresh();
    }

    fn teams_paired(&mut self, paired: Paired) {
        let Paired { mut info, token } = paired;
        if !self.teams_enabled() {
            return;
        }
        if let Err(err) = self
            .opts
            .secrets
            .set(&SecretKey::TeamsDeviceToken, token.expose())
        {
            log::warn!("teams: device token not saved: {err}");
            self.notice(MSG_TOKEN_NOT_SAVED);
            return;
        }
        self.teams.invite = None;
        // A new team: whatever was queued for an older generation is dropped.
        let generation = self.teams.bump();
        info.generation = generation;
        self.teams.opened = Some(generation);
        self.teams.policy = Some(self.snap.config.clone());
        self.teams.status = None;
        self.teams_send(Job::Activate {
            info: Box::new(info),
            token,
        });
        self.teams_send(Job::Policy(Box::new(self.snap.config.clone())));
        // What is already known goes out at once.
        let fresh: Vec<Reading> = self
            .snap
            .accounts
            .iter()
            .filter_map(|v| v.reading.clone())
            .collect();
        for reading in &fresh {
            self.teams_observe_reading(reading);
        }
        let hourly = self
            .analytics_shared
            .lock()
            .ok()
            .and_then(|r| r.as_ref().map(|r| r.hourly.clone()));
        if let Some(rows) = hourly {
            self.teams_observe_usage(rows);
        }
        self.notice(MSG_PAIRED);
    }

    /// Engine shutdown: stop the worker, wait for it at most [`STOP_WAIT`].
    pub(super) fn teams_shutdown(&mut self) {
        if let Some(link) = self.teams.link.take() {
            let _ = link.tx.send(Envelope {
                generation: self.teams.generation(),
                job: Job::Stop,
                observation: false,
            });
            let _ = link.done.recv_timeout(STOP_WAIT);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------------------------

struct Worker {
    paths: Paths,
    env: Env,
    http: Arc<dyn HttpClient>,
    clock: Clock,
    /// The engine's current generation.
    current: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    tx: Sender<Msg>,
    sync: Option<TeamsSync>,
    /// Generation `sync` was opened for.
    sync_gen: u64,
    token: Option<SecretValue>,
    config: Option<Config>,
    owners: Vec<RootOwners>,
    /// The admitted origin is no longer the pairing's: nothing is sent.
    origin_mismatch: bool,
    last_attempt: Option<i64>,
    flush: bool,
    /// The last upload was paused for hours (402/426): "Send now" does not override it.
    long_pause: bool,
    /// Revoked or consent needed: no more uploads for this pairing.
    halted: bool,
    published: Option<TeamsStatus>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Envelope>, wake: Duration) {
        loop {
            match rx.recv_timeout(wake) {
                Ok(envelope) => {
                    if envelope.observation {
                        self.pending.fetch_sub(1, Ordering::SeqCst);
                    }
                    if matches!(envelope.job, Job::Stop) {
                        break;
                    }
                    self.handle(envelope.generation, envelope.job);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.periodic();
            self.publish();
        }
    }

    fn stale(&self, generation: u64) -> bool {
        generation != self.current.load(Ordering::SeqCst)
    }

    fn report(&self, generation: u64, report: Report) {
        let _ = self.tx.send(Msg::Teams { generation, report });
    }

    /// The open pairing, when it belongs to `generation` (the current one).
    fn sync_for(&mut self, generation: u64) -> Option<&mut TeamsSync> {
        if self.sync_gen != generation || self.stale(generation) {
            return None;
        }
        self.sync.as_mut()
    }

    fn reset(&mut self) {
        self.sync = None;
        self.token = None;
        self.origin_mismatch = false;
        self.last_attempt = None;
        self.flush = false;
        self.long_pause = false;
        self.halted = false;
        self.published = None;
    }

    fn storage_error(&self, generation: u64) {
        self.report(
            generation,
            Report::Status(TeamsStatus {
                state: TeamsState::Error,
                last_error: Some(codenotch_teams::i18n::STORAGE.into()),
                ..TeamsStatus::default()
            }),
        );
    }

    fn handle(&mut self, generation: u64, job: Job) {
        // Detach always runs (it only deletes); everything else only for the current pairing.
        if self.stale(generation) && !matches!(job, Job::Detach { .. }) {
            return;
        }
        let now = (self.clock)();
        match job {
            Job::Open { token } => {
                self.reset();
                match TeamsSync::open(&self.paths) {
                    Ok(Some(sync)) => {
                        self.sync = Some(sync);
                        self.sync_gen = generation;
                        self.token = Some(token);
                    }
                    Ok(None) => self.report(generation, Report::NotPaired),
                    Err(err) => {
                        log::warn!("teams: pairing not loaded: {err}");
                        self.storage_error(generation);
                    }
                }
            }
            Job::Activate { info, token } => {
                self.reset();
                match TeamsSync::create(&self.paths, *info) {
                    Ok(sync) => {
                        self.sync = Some(sync);
                        self.sync_gen = generation;
                        self.token = Some(token);
                    }
                    Err(err) => {
                        log::warn!("teams: pairing not saved: {err}");
                        self.storage_error(generation);
                    }
                }
            }
            Job::Policy(config) => {
                // Scan roots read the profile folders: done here, not on the engine thread.
                self.owners = codenotch_analytics::root_owners(&config, &self.env);
                let owners = std::mem::take(&mut self.owners);
                if let Some(sync) = self.sync_for(generation) {
                    let admitted = Origin::admitted(&config.settings).ok();
                    let mismatch = admitted.as_ref() != Some(sync.origin());
                    if let Err(err) = sync.set_policy(&config, &owners, now) {
                        log::warn!("teams: policy not applied: {err}");
                    }
                    self.origin_mismatch = mismatch;
                }
                self.owners = owners;
                self.config = Some(*config);
            }
            Job::Reading(reading) => {
                if let Some(sync) = self.sync_for(generation) {
                    if let Err(err) = sync.observe_reading(&reading, now) {
                        log::warn!("teams: snapshot not queued: {err}");
                    }
                }
            }
            Job::Usage(rows) => {
                if let Some(sync) = self.sync_for(generation) {
                    if let Err(err) = sync.observe_usage(&rows, now) {
                        log::warn!("teams: usage not queued: {err}");
                    }
                }
            }
            Job::Flush => self.flush = true,
            Job::Backfill(allow) => {
                if let Some(sync) = self.sync_for(generation) {
                    if let Err(err) = sync.allow_backfill(allow) {
                        log::warn!("teams: backfill agreement not saved: {err}");
                    }
                }
            }
            Job::Preview { origin, code } => {
                let result = pairing::preview(&*self.http, &origin, &code);
                let consent = result.as_ref().map_or(0, |p| p.consent_version);
                let result = result
                    .map(|p| TeamsInvitePreview {
                        team_name: p.team_name,
                        visibility: p.visibility,
                        origin: origin.as_str().to_string(),
                    })
                    .map_err(pair_message);
                self.report(generation, Report::Preview(result, consent));
            }
            Job::Pair {
                origin,
                code,
                consent_version,
            } => {
                let device_id = pairing::new_device_id();
                let result =
                    pairing::pair(&*self.http, &origin, &code, &device_id, consent_version)
                        .map(|p| {
                            Box::new(Paired {
                                info: PairingInfo {
                                    origin: origin.clone(),
                                    team_id: p.team_id,
                                    team_name: p.team_name,
                                    device_id,
                                    generation: 0,
                                    consent_version,
                                    data_epoch: p.data_epoch,
                                    floor_at: p.floor_at,
                                },
                                token: p.device_token,
                            })
                        })
                        .map_err(pair_message);
                // The engine drops it when the generation moved meanwhile.
                self.report(generation, Report::Paired(result));
            }
            Job::Declare {
                account_id,
                payer,
                price_eur_month,
            } => {
                let result = self.declare(generation, &account_id, payer, price_eur_month);
                self.report(generation, Report::Declared(result));
            }
            Job::Erase => {
                let result = match (self.sync_for(generation).is_some(), &self.token) {
                    (true, Some(token)) => {
                        let origin = self.sync.as_ref().map(|s| s.origin().clone());
                        match origin {
                            Some(origin) => pairing::erase(&*self.http, &origin, token.expose())
                                .map_err(pair_message),
                            None => Err(MSG_NOT_PAIRED),
                        }
                    }
                    _ => Err(MSG_NOT_PAIRED),
                };
                self.report(generation, Report::Erased(result));
            }
            Job::Detach { token } => {
                let origin = match &self.sync {
                    Some(sync) => Some(sync.origin().clone()),
                    None => TeamsSync::open(&self.paths)
                        .ok()
                        .flatten()
                        .map(|s| s.origin().clone()),
                };
                self.reset();
                if let (Some(token), Some(origin)) = (token, origin) {
                    if let Err(e) = pairing::detach(&*self.http, &origin, token.expose()) {
                        log::info!("teams: detach not confirmed by the server ({e:?})");
                    }
                }
                if let Err(err) = TeamsSync::detach_local(&self.paths) {
                    log::warn!("teams: local pairing not deleted: {err}");
                }
                self.report(generation, Report::Detached);
            }
            Job::Stop => {}
        }
    }

    fn declare(
        &mut self,
        generation: u64,
        account_id: &str,
        payer: TeamsPayer,
        price: Option<f64>,
    ) -> Result<(), &'static str> {
        let (Some(sync), Some(token), Some(config)) = (
            self.sync.as_ref().filter(|_| self.sync_gen == generation),
            self.token.as_ref(),
            self.config.as_ref(),
        ) else {
            return Err(MSG_NOT_PAIRED);
        };
        let policy = Policy::new(
            config,
            &self.owners,
            &sync.binding().device_id,
            sync.effective_floor(),
        );
        pairing::declare_account(
            &*self.http,
            sync.origin(),
            token.expose(),
            &policy,
            account_id,
            payer,
            price,
        )
        .map_err(pair_message)
    }

    /// Heartbeat and upload of the open pairing, at most every 60 s and not before the
    /// backoff deadline.
    fn periodic(&mut self) {
        let generation = self.sync_gen;
        if self.halted || self.origin_mismatch || self.sync_for(generation).is_none() {
            return;
        }
        let now = (self.clock)();
        let (flush, long_pause, last_attempt) = (self.flush, self.long_pause, self.last_attempt);
        let Some(token) = self.token.clone() else {
            return;
        };
        let http = self.http.clone();
        let Some(sync) = self.sync.as_mut() else {
            return;
        };
        if let Err(err) = sync.heartbeat(now) {
            log::warn!("teams: heartbeat not queued: {err}");
        }
        if sync.queued() == 0 {
            self.flush = false;
            return;
        }
        let spaced = flush || last_attempt.is_none_or(|t| now - t >= MIN_SEND_INTERVAL_SECS);
        let after_backoff = sync.retry_at().is_none_or(|at| now >= at) || (flush && !long_pause);
        if !(spaced && after_backoff) {
            return;
        }
        self.flush = false;
        let outcome = sync.send(&*http, token.expose(), now);
        if outcome != SendOutcome::Nothing {
            // `Nothing` made no request (no policy yet, everything purged).
            self.last_attempt = Some(now);
        }
        match outcome {
            SendOutcome::Revoked | SendOutcome::ConsentNeeded => self.halted = true,
            SendOutcome::Paused { .. } => self.long_pause = true,
            SendOutcome::Sent { .. } | SendOutcome::EpochChanged => self.long_pause = false,
            _ => {}
        }
    }

    fn publish(&mut self) {
        let generation = self.sync_gen;
        let mismatch = self.origin_mismatch;
        let Some(sync) = self.sync_for(generation) else {
            return;
        };
        let mut status = sync.status();
        if mismatch {
            status.state = TeamsState::Error;
            status.last_error = Some(MSG_ORIGIN_CHANGED.into());
        }
        if self.published.as_ref() != Some(&status) {
            self.published = Some(status.clone());
            self.report(generation, Report::Status(status));
        }
    }
}

#[cfg(test)]
#[path = "teams_tests.rs"]
mod tests;
