//! [`TeamsSync`]: detection, revisions, the durable protocol and the upload of one pairing.
//!
//! Durable order for every observation: (1) the new events are appended to the outbox and
//! synced to disk; (2) only then do the cursors advance in memory; (3) `teams.json` is written
//! atomically. On start, cursors are the maximum of `teams.json` and the outbox, so a crash
//! between any two steps neither loses an event nor reuses a revision. The outbox is only
//! compacted once `teams.json` is up to date.
//!
//! Blocking and single-threaded: the caller (the engine's Teams worker) owns the value and
//! calls it from one thread. Nothing here spawns a thread.

use std::collections::HashSet;
use std::io;
use std::path::PathBuf;

use codenotch_core::analytics::{HourlyUsage, RootOwners};
use codenotch_core::model::{TeamsState, TeamsStatus};
use codenotch_core::{Config, HttpClient, HttpRequest, Paths, Reading};
use codenotch_teams_schema::{
    Batch, EventPayload, Heartbeat, QuotaSnapshot, RawEvent, Tokens, UsageHour, MAX_BATCH_BYTES,
    MAX_BATCH_EVENTS, SCHEMA_VERSION,
};
use serde::Deserialize;

use crate::fault::{fault, Step};
use crate::filter::{Policy, QuotaDraft, UsageKey};
use crate::journal::{Binding, Journal, Record};
use crate::origin::Origin;
use crate::state::{Cursors, SnapshotCursor, StateFile, UsageCursor};
use crate::{i18n, APP_VERSION};

/// Revisions and tombstones only touch hours of the last 72 hours; older sent hours are final.
pub const MUTABLE_SECS: i64 = 72 * 3600;
/// A snapshot is sent again after 6 hours even without a change.
pub const RESYNC_SECS: i64 = 6 * 3600;
/// Heartbeat period.
pub const HEARTBEAT_SECS: i64 = 15 * 60;
/// Days before pairing sent with the member's separate agreement.
pub const BACKFILL_SECS: i64 = 45 * 86_400;
/// Smallest pause between two uploads (the worker schedules; `send` does not wait).
pub const MIN_SEND_INTERVAL_SECS: i64 = 60;
/// Pause after a 402 (team billing) or a 426 (app too old).
pub const LONG_PAUSE_SECS: i64 = 6 * 3600;
/// Usage cursors older than this are forgotten (outside the hourly export).
const CURSOR_KEEP_SECS: i64 = 46 * 86_400;
/// `resetsAt` moves smaller than this are not a change.
const RESET_TOLERANCE_SECS: i64 = 60;
/// Backoff: 30 s × 2ⁿ, ±20 %, at most 15 min.
const BACKOFF_BASE_SECS: i64 = 30;
const BACKOFF_MAX_SECS: i64 = 15 * 60;

/// What the server said at pairing (see [`crate::pairing::pair`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingInfo {
    pub origin: Origin,
    pub team_id: String,
    pub team_name: String,
    pub device_id: String,
    /// Pairing generation of the engine (bumped on detach / team change / erase).
    pub generation: u64,
    pub consent_version: u32,
    pub data_epoch: u32,
    /// First hour that may be sent (unix secs).
    pub floor_at: i64,
}

/// Why sending paused for a long time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    /// 402: the team's subscription is not active.
    Billing,
    /// 426: the server no longer accepts this contract version.
    UpdateRequired,
}

/// Result of one [`TeamsSync::send`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Nothing to send (or no policy yet): no request was made.
    Nothing,
    /// The server answered; `removed` events left the queue (accepted, duplicate or
    /// rejected). `more` = events are still waiting.
    Sent { removed: usize, more: bool },
    /// Try again at `at` (network error, 5xx, 429, 413 split).
    Retry { at: i64 },
    /// Do not send before `until`.
    Paused { until: i64, reason: PauseReason },
    /// 401/403: the team revoked this device.
    Revoked,
    /// 409 `consent_required`: the member must review and accept again.
    ConsentNeeded,
    /// 409 `epoch_changed`: the member's server data was erased; the queue was purged and
    /// the floor moved.
    EpochChanged,
    /// The origin does not answer like a Teams server (redirect, 404, unreadable reply).
    NotATeamsServer { retry_at: i64 },
    /// The outbox or `teams.json` cannot be written.
    StorageError,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ack {
    #[serde(default)]
    accepted: Vec<String>,
    #[serde(default)]
    duplicate: Vec<String>,
    #[serde(default)]
    rejected: Vec<Rejected>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rejected {
    event_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Conflict {
    #[serde(default)]
    error: String,
    #[serde(default)]
    data_epoch: Option<u32>,
    #[serde(default)]
    floor_at: Option<i64>,
}

/// Sync state of one pairing. See the module docs.
pub struct TeamsSync {
    state_path: PathBuf,
    outbox_path: PathBuf,
    origin: Origin,
    state: StateFile,
    cursors: Cursors,
    journal: Journal,
    policy: Option<Policy>,
    status: TeamsState,
    last_error: Option<&'static str>,
    failures: u32,
    retry_at: Option<i64>,
    batch_limit: usize,
    /// `teams.json` is behind the cursors (a write failed): no compaction until it is saved.
    dirty: bool,
}

/// Counts and states only.
impl std::fmt::Debug for TeamsSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsSync")
            .field("status", &self.status)
            .field("queued", &self.journal.len())
            .field("failures", &self.failures)
            .field("retry_at", &self.retry_at)
            .finish()
    }
}

impl TeamsSync {
    /// The pairing of this device, or `None` when it is not paired (`teams.json` missing).
    /// Creates no file.
    pub fn open(paths: &Paths) -> io::Result<Option<TeamsSync>> {
        let state_path = paths.teams_state();
        let Some(state) = StateFile::load(&state_path)? else {
            return Ok(None);
        };
        let origin = Origin::parse(&state.binding.origin)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "teams origin"))?;
        let journal = Journal::open(&paths.teams_outbox(), &state.binding)?;
        let mut sync = TeamsSync::assemble(paths, origin, state, journal);
        sync.rebuild_from_journal();
        Ok(Some(sync))
    }

    /// Start a new pairing: any previous outbox is deleted, `teams.json` is written.
    pub fn create(paths: &Paths, info: PairingInfo) -> io::Result<TeamsSync> {
        let binding = Binding {
            origin: info.origin.as_str().to_string(),
            team_id: info.team_id,
            device_id: info.device_id,
            generation: info.generation,
            consent_version: info.consent_version,
            data_epoch: info.data_epoch,
        };
        Journal::destroy(&paths.teams_outbox())?;
        let state = StateFile::new(binding, info.team_name, info.floor_at);
        state.save(&paths.teams_state())?;
        let journal = Journal::open(&paths.teams_outbox(), &state.binding)?;
        Ok(TeamsSync::assemble(paths, info.origin, state, journal))
    }

    /// Forget the pairing on this device: `teams.json` and the outbox are deleted (always,
    /// nothing is kept for a later pairing). The caller deletes the device token.
    pub fn detach_local(paths: &Paths) -> io::Result<()> {
        Journal::destroy(&paths.teams_outbox())?;
        Journal::destroy(&paths.teams_state())
    }

    fn assemble(paths: &Paths, origin: Origin, state: StateFile, journal: Journal) -> TeamsSync {
        TeamsSync {
            state_path: paths.teams_state(),
            outbox_path: paths.teams_outbox(),
            origin,
            cursors: Cursors::from_state(&state),
            state,
            journal,
            policy: None,
            status: TeamsState::Active,
            last_error: None,
            failures: 0,
            retry_at: None,
            batch_limit: MAX_BATCH_EVENTS,
            dirty: false,
        }
    }

    /// Cursors = max(teams.json, outbox).
    fn rebuild_from_journal(&mut self) {
        let mut changed = false;
        for record in self.journal.pending() {
            if record.seq > self.cursors.journal_seq {
                self.cursors.journal_seq = record.seq;
                changed = true;
            }
            match record.event.payload() {
                Ok(EventPayload::UsageHour(u)) => {
                    let cursor = UsageCursor {
                        account_ref: u.account_ref,
                        provider: u.provider,
                        model: u.model,
                        hour_start: u.hour_start,
                        rev: u.rev,
                        tokens: u.tokens,
                    };
                    let key = cursor.key();
                    if self
                        .cursors
                        .usage
                        .get(&key)
                        .is_none_or(|c| c.rev < cursor.rev)
                    {
                        self.cursors.usage.insert(key, cursor);
                        changed = true;
                    }
                }
                Ok(EventPayload::QuotaSnapshot(s))
                    if self
                        .cursors
                        .snapshots
                        .get(&s.account_ref)
                        .is_none_or(|c| c.seq < s.seq) =>
                {
                    let cursor = snapshot_cursor(&s, record.event.occurred_at);
                    self.cursors.snapshots.insert(s.account_ref, cursor);
                    changed = true;
                }
                _ => {}
            }
        }
        self.dirty |= changed;
    }

    // -----------------------------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------------------------

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn binding(&self) -> &Binding {
        self.journal.binding()
    }

    pub fn team_name(&self) -> &str {
        &self.state.team_name
    }

    /// Events waiting in the outbox.
    pub fn queued(&self) -> usize {
        self.journal.len()
    }

    /// When the last [`send`](Self::send) asked to be retried (backoff, `Retry-After`, pause).
    pub fn retry_at(&self) -> Option<i64> {
        self.retry_at
    }

    /// First hour that may be sent now (the pairing floor, 45 days earlier with backfill).
    pub fn effective_floor(&self) -> i64 {
        if self.state.backfill {
            self.state.floor_at - BACKFILL_SECS
        } else {
            self.state.floor_at
        }
    }

    /// Status for `Snapshot.teams`.
    pub fn status(&self) -> TeamsStatus {
        TeamsStatus {
            state: self.status,
            team_name: Some(self.state.team_name.clone()),
            queued: u32::try_from(self.journal.len()).unwrap_or(u32::MAX),
            last_sent_at: self.state.last_sent_at,
            last_error: self.last_error.map(str::to_string),
            origin: Some(self.origin.as_str().to_string()),
            backfill: self.state.backfill,
            preview: None,
            busy: false,
        }
    }

    /// The member agreed to send the days before pairing (see [`allow_backfill`](Self::allow_backfill)).
    pub fn backfill(&self) -> bool {
        self.state.backfill
    }

    // -----------------------------------------------------------------------------------------
    // Policy
    // -----------------------------------------------------------------------------------------

    /// Apply the current config and scan roots. Queued events that are no longer allowed are
    /// purged; an account that stopped being shared gets an empty (tombstone) snapshot.
    pub fn set_policy(
        &mut self,
        config: &Config,
        owners: &[RootOwners],
        now: i64,
    ) -> io::Result<()> {
        let policy = Policy::new(
            config,
            owners,
            &self.state.binding.device_id,
            self.effective_floor(),
        );
        self.policy = Some(policy);
        self.refilter();
        let policy = self.policy.as_ref().expect("set above");
        let mut cursors = self.cursors.clone();
        let mut events = Vec::new();
        for c in cursors.snapshots.values_mut() {
            if c.tombstone || policy.by_ref(&c.account_ref).is_some() {
                continue;
            }
            let snapshot = QuotaSnapshot {
                account_ref: c.account_ref.clone(),
                provider: c.provider,
                plan: c.plan,
                seq: c.seq + 1,
                observed_at: now,
                windows: Vec::new(),
            };
            *c = snapshot_cursor(&snapshot, now);
            events.push(EventPayload::QuotaSnapshot(snapshot));
        }
        self.commit(cursors, events, now)?;
        self.compact()
    }

    /// The member's separate agreement to send the 45 days before pairing (or its withdrawal,
    /// which purges queued hours before the pairing floor).
    pub fn allow_backfill(&mut self, allow: bool) -> io::Result<()> {
        self.state.backfill = allow;
        let floor = self.effective_floor();
        if let Some(p) = self.policy.as_mut() {
            p.set_floor(floor);
        }
        self.refilter();
        self.persist()?;
        self.compact()
    }

    /// Purge queued events the current policy no longer allows.
    fn refilter(&mut self) -> usize {
        let Some(policy) = self.policy.as_ref() else {
            return 0;
        };
        self.journal.retain(|r| allowed(policy, r))
    }

    // -----------------------------------------------------------------------------------------
    // Observations
    // -----------------------------------------------------------------------------------------

    /// A provider reading. Queues a snapshot when the account is shared, the reading is fresh
    /// and something changed (or 6 hours passed). Returns whether one was queued.
    pub fn observe_reading(&mut self, reading: &Reading, now: i64) -> io::Result<bool> {
        let Some(draft) = self.policy.as_ref().and_then(|p| p.quota(reading)) else {
            return Ok(false);
        };
        let mut cursors = self.cursors.clone();
        let prev = cursors.snapshots.get(&draft.account_ref);
        let due = prev.is_none_or(|c| changed(c, &draft) || now - c.queued_at >= RESYNC_SECS);
        if !due {
            return Ok(false);
        }
        let snapshot = QuotaSnapshot {
            account_ref: draft.account_ref.clone(),
            provider: draft.provider,
            plan: draft.plan,
            seq: prev.map_or(1, |c| c.seq + 1),
            observed_at: draft.observed_at,
            windows: draft.windows,
        };
        cursors
            .snapshots
            .insert(draft.account_ref, snapshot_cursor(&snapshot, now));
        Ok(self.commit(cursors, vec![EventPayload::QuotaSnapshot(snapshot)], now)? > 0)
    }

    /// The hourly export of an analytics run. Queues a revision for every key whose total
    /// changed, the first revision of new keys, and a zero revision (tombstone) for keys of
    /// the last 72 hours that disappeared. Returns how many events were queued.
    pub fn observe_usage(&mut self, rows: &[HourlyUsage], now: i64) -> io::Result<usize> {
        let Some(policy) = self.policy.as_ref() else {
            return Ok(0);
        };
        let current = policy.usage(rows);
        let mutable_from = now - MUTABLE_SECS;
        let mut cursors = self.cursors.clone();
        cursors
            .usage
            .retain(|k, _| k.hour_start >= now - CURSOR_KEEP_SECS);
        let mut events = Vec::new();
        let mut revise = |cursors: &mut Cursors, key: &UsageKey, tokens: Tokens| {
            let rev = cursors.usage.get(key).map_or(1, |c| c.rev + 1);
            let hour = UsageHour {
                account_ref: key.account_ref.clone(),
                provider: key.provider,
                model: key.model,
                hour_start: key.hour_start,
                rev,
                tokens,
            };
            cursors.usage.insert(
                key.clone(),
                UsageCursor {
                    account_ref: key.account_ref.clone(),
                    provider: key.provider,
                    model: key.model,
                    hour_start: key.hour_start,
                    rev,
                    tokens,
                },
            );
            events.push(EventPayload::UsageHour(hour));
        };
        for (key, tokens) in &current {
            match cursors.usage.get(key) {
                None if tokens.is_zero() => {}
                None => revise(&mut cursors, key, *tokens),
                Some(c) if key.hour_start >= mutable_from && c.tokens != *tokens => {
                    revise(&mut cursors, key, *tokens)
                }
                Some(_) => {}
            }
        }
        let vanished: Vec<UsageKey> = cursors
            .usage
            .iter()
            .filter(|(k, c)| {
                k.hour_start >= mutable_from
                    && k.hour_start >= policy.floor()
                    && !c.tokens.is_zero()
                    && !current.contains_key(k)
                    && policy.by_ref(&k.account_ref).is_some_and(|a| a.usage)
            })
            .map(|(k, _)| k.clone())
            .collect();
        for key in vanished {
            revise(&mut cursors, &key, Tokens::default());
        }
        self.commit(cursors, events, now)
    }

    /// Queue a heartbeat when 15 minutes passed since the last one. Returns whether it did.
    pub fn heartbeat(&mut self, now: i64) -> io::Result<bool> {
        if self
            .state
            .last_heartbeat
            .is_some_and(|t| (0..HEARTBEAT_SECS).contains(&(now - t)))
        {
            return Ok(false);
        }
        let payload = EventPayload::Heartbeat(Heartbeat {
            queued: u32::try_from(self.journal.len()).unwrap_or(u32::MAX),
            dropped: self.journal.take_dropped(),
        });
        self.state.last_heartbeat = Some(now);
        let cursors = self.cursors.clone();
        Ok(self.commit(cursors, vec![payload], now)? > 0)
    }

    /// Steps (1)–(3) of the durable protocol for `payloads` built on `cursors`.
    fn commit(
        &mut self,
        mut cursors: Cursors,
        payloads: Vec<EventPayload>,
        now: i64,
    ) -> io::Result<usize> {
        if payloads.is_empty() {
            return Ok(0);
        }
        let mut records = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let id = uuid::Uuid::new_v4().to_string();
            // Every payload was built from closed values; one that still fails is skipped.
            if let Ok(event) = RawEvent::new(id, now, payload) {
                records.push(Record {
                    seq: cursors.next_seq(),
                    schema_version: SCHEMA_VERSION,
                    event,
                });
            }
        }
        let n = records.len();
        if let Err(e) = self.journal.append(records) {
            self.storage_failed();
            return Err(e);
        }
        self.cursors = cursors;
        if let Err(e) = fault(Step::Cursor) {
            self.dirty = true;
            return Err(e);
        }
        self.persist()?;
        Ok(n)
    }

    /// Write `teams.json` from the cursors.
    fn persist(&mut self) -> io::Result<()> {
        self.cursors.write_to(&mut self.state);
        match self.state.save(&self.state_path) {
            Ok(()) => {
                self.dirty = false;
                if self.status == TeamsState::Error && self.last_error == Some(i18n::STORAGE) {
                    self.status = TeamsState::Active;
                    self.last_error = None;
                }
                Ok(())
            }
            Err(e) => {
                self.dirty = true;
                self.storage_failed();
                Err(e)
            }
        }
    }

    /// Compact the outbox when it holds stale lines and `teams.json` is up to date.
    fn compact(&mut self) -> io::Result<()> {
        if self.dirty {
            self.persist()?;
        }
        if self.journal.needs_compaction() {
            self.journal
                .compact()
                .inspect_err(|_| self.storage_failed())?;
        }
        Ok(())
    }

    fn storage_failed(&mut self) {
        self.status = TeamsState::Error;
        self.last_error = Some(i18n::STORAGE);
    }

    // -----------------------------------------------------------------------------------------
    // Upload
    // -----------------------------------------------------------------------------------------

    /// Upload one batch to `<origin>/v1/events` with the device token. Re-filters the outbox
    /// against the current policy first. Makes no request when nothing is queued or no
    /// policy was set. The worker decides when to call (see [`retry_at`](Self::retry_at),
    /// [`MIN_SEND_INTERVAL_SECS`]).
    pub fn send(&mut self, http: &dyn HttpClient, token: &str, now: i64) -> SendOutcome {
        if self.policy.is_none() {
            return SendOutcome::Nothing;
        }
        self.refilter();
        if self.compact().is_err() {
            return SendOutcome::StorageError;
        }
        let Some((body, ids)) = self.next_batch(now) else {
            return SendOutcome::Nothing;
        };
        let mut request = HttpRequest::post(self.origin.url("/v1/events"))
            .bearer(token)
            .header("Content-Type", "application/json");
        request.body = Some(body);
        let response = match http.send(&request) {
            Ok(r) => r,
            Err(_) => return self.backoff(now, i18n::PAUSED),
        };
        match response.status {
            200..=299 => {
                let Ok(ack) = response.json::<Ack>() else {
                    return self.not_a_server(now);
                };
                let mut done: HashSet<&str> = HashSet::new();
                let sent: HashSet<&str> = ids.iter().map(String::as_str).collect();
                let acked = ack
                    .accepted
                    .iter()
                    .chain(&ack.duplicate)
                    .chain(ack.rejected.iter().map(|r| &r.event_id));
                for id in acked {
                    if let Some(id) = sent.get(id.as_str()) {
                        done.insert(id);
                    }
                }
                let removed = self.journal.remove_ids(&done);
                self.state.last_sent_at = Some(now);
                self.failures = 0;
                self.retry_at = None;
                // After a 413 split, grow back gradually.
                self.batch_limit = (self.batch_limit * 2).min(MAX_BATCH_EVENTS);
                self.status = TeamsState::Active;
                self.last_error = None;
                if self.persist().is_err() || self.compact().is_err() {
                    return SendOutcome::StorageError;
                }
                SendOutcome::Sent {
                    removed,
                    more: !self.journal.is_empty(),
                }
            }
            401 | 403 => {
                self.status = TeamsState::Revoked;
                self.last_error = Some(i18n::REVOKED);
                SendOutcome::Revoked
            }
            402 => self.pause(now, PauseReason::Billing),
            409 => {
                let conflict: Option<Conflict> = response.json().ok();
                match conflict {
                    Some(c) if c.error == "consent_required" => {
                        self.status = TeamsState::ConsentNeeded;
                        self.last_error = Some(i18n::CONSENT);
                        SendOutcome::ConsentNeeded
                    }
                    Some(Conflict {
                        error,
                        data_epoch: Some(epoch),
                        floor_at,
                    }) if error == "epoch_changed" => {
                        match self.epoch_changed(epoch, floor_at.unwrap_or(now)) {
                            Ok(()) => SendOutcome::EpochChanged,
                            Err(_) => SendOutcome::StorageError,
                        }
                    }
                    _ => self.backoff(now, i18n::PAUSED),
                }
            }
            413 => {
                if ids.len() <= 1 {
                    // One event alone is too large: it can never be sent.
                    let one: HashSet<&str> = ids.iter().map(String::as_str).collect();
                    self.journal.remove_ids(&one);
                    if self.compact().is_err() {
                        return SendOutcome::StorageError;
                    }
                } else {
                    self.batch_limit = (ids.len() / 2).max(1);
                }
                self.retry_at = Some(now);
                SendOutcome::Retry { at: now }
            }
            426 => self.pause(now, PauseReason::UpdateRequired),
            429 => match response.retry_after_secs(now) {
                Some(secs) => {
                    let at = now + secs.clamp(1, LONG_PAUSE_SECS);
                    self.status = TeamsState::Paused;
                    self.last_error = Some(i18n::PAUSED);
                    self.retry_at = Some(at);
                    SendOutcome::Retry { at }
                }
                None => self.backoff(now, i18n::PAUSED),
            },
            300..=399 | 404 | 405 => self.not_a_server(now),
            _ => self.backoff(now, i18n::PAUSED),
        }
    }

    /// The oldest queued events that fit one batch: (body bytes, event ids).
    fn next_batch(&mut self, now: i64) -> Option<(Vec<u8>, Vec<String>)> {
        let binding = self.journal.binding().clone();
        let mut batch = Batch {
            schema_version: SCHEMA_VERSION,
            sent_at: now,
            device_id: binding.device_id,
            app_version: APP_VERSION.to_string(),
            consent_version: binding.consent_version,
            data_epoch: binding.data_epoch,
            events: Vec::new(),
        };
        let mut size = serde_json::to_vec(&batch).ok()?.len();
        for record in self.journal.pending() {
            if batch.events.len() >= self.batch_limit.min(MAX_BATCH_EVENTS) {
                break;
            }
            let n = serde_json::to_vec(&record.event).ok()?.len() + 1;
            if size + n > MAX_BATCH_BYTES {
                break;
            }
            size += n;
            batch.events.push(record.event.clone());
        }
        if batch.events.is_empty() || batch.validate().is_err() {
            return None;
        }
        let ids = batch.events.iter().map(|e| e.event_id.clone()).collect();
        Some((serde_json::to_vec(&batch).ok()?, ids))
    }

    fn backoff(&mut self, now: i64, error: &'static str) -> SendOutcome {
        let exp = BACKOFF_BASE_SECS.saturating_mul(1 << self.failures.min(10));
        let delay = exp.min(BACKOFF_MAX_SECS);
        // ±20 % jitter from the OS random source behind uuid v4.
        let r = (uuid::Uuid::new_v4().as_u128() % 4001) as i64 - 2000; // -2000..=2000
        let jittered = (delay + delay * r / 10_000).max(1);
        self.failures = self.failures.saturating_add(1);
        let at = now + jittered;
        self.status = TeamsState::Paused;
        self.last_error = Some(error);
        self.retry_at = Some(at);
        SendOutcome::Retry { at }
    }

    fn not_a_server(&mut self, now: i64) -> SendOutcome {
        let SendOutcome::Retry { at } = self.backoff(now, i18n::NOT_A_SERVER) else {
            unreachable!("backoff always retries");
        };
        self.status = TeamsState::Error;
        SendOutcome::NotATeamsServer { retry_at: at }
    }

    fn pause(&mut self, now: i64, reason: PauseReason) -> SendOutcome {
        let until = now + LONG_PAUSE_SECS;
        self.status = TeamsState::Paused;
        self.last_error = Some(match reason {
            PauseReason::Billing => i18n::PAUSED,
            PauseReason::UpdateRequired => i18n::UPDATE,
        });
        self.retry_at = Some(until);
        SendOutcome::Paused { until, reason }
    }

    /// The member's server data was erased: purge the outbox (bound to the old epoch), move
    /// the floor, forget the backfill agreement, and send full snapshots again.
    fn epoch_changed(&mut self, epoch: u32, floor_at: i64) -> io::Result<()> {
        Journal::destroy(&self.outbox_path)?;
        self.state.binding.data_epoch = epoch;
        self.state.floor_at = floor_at;
        self.state.backfill = false;
        for c in self.cursors.snapshots.values_mut() {
            c.windows.clear();
            c.tombstone = true;
        }
        self.journal = Journal::open(&self.outbox_path, &self.state.binding)?;
        let floor = self.effective_floor();
        if let Some(p) = self.policy.as_mut() {
            p.set_floor(floor);
        }
        self.status = TeamsState::Active;
        self.last_error = None;
        self.persist()
    }
}

fn snapshot_cursor(s: &QuotaSnapshot, queued_at: i64) -> SnapshotCursor {
    SnapshotCursor {
        account_ref: s.account_ref.clone(),
        provider: s.provider,
        seq: s.seq,
        queued_at,
        plan: s.plan,
        windows: s.windows.clone(),
        tombstone: s.windows.is_empty(),
    }
}

/// A new snapshot differs from the last one queued: window ids, rounded percent, reset time
/// (beyond 60 s), length, role, kind, plan.
fn changed(prev: &SnapshotCursor, draft: &QuotaDraft) -> bool {
    if prev.tombstone || prev.plan != draft.plan || prev.windows.len() != draft.windows.len() {
        return true;
    }
    let round = |p: Option<f64>| p.map(|p| p.round() as i64);
    draft.windows.iter().any(|w| {
        let Some(o) = prev.windows.iter().find(|o| o.id == w.id) else {
            return true;
        };
        let reset_moved = match (o.resets_at, w.resets_at) {
            (Some(a), Some(b)) => (a - b).abs() > RESET_TOLERANCE_SECS,
            (a, b) => a.is_some() != b.is_some(),
        };
        o.kind != w.kind
            || o.role != w.role
            || o.window_seconds != w.window_seconds
            || round(o.used_percent) != round(w.used_percent)
            || reset_moved
    })
}

/// The re-filter before sending: the event is valid, of this schema version, and its account
/// is still shared (tombstones always pass), its hour at or after the floor.
fn allowed(policy: &Policy, record: &Record) -> bool {
    if record.schema_version != SCHEMA_VERSION {
        return false;
    }
    match record.event.validate() {
        Ok(EventPayload::Heartbeat(_)) => true,
        Ok(EventPayload::QuotaSnapshot(s)) => {
            s.windows.is_empty()
                || policy
                    .by_ref(&s.account_ref)
                    .is_some_and(|a| a.provider == s.provider)
        }
        Ok(EventPayload::UsageHour(u)) => {
            u.hour_start >= policy.floor()
                && policy
                    .by_ref(&u.account_ref)
                    .is_some_and(|a| a.usage && a.provider == u.provider)
        }
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
