//! `teams.json`: pairing and cursors. No secret (the device token lives in the Credential
//! Manager). Written atomically; exists only while the device is paired.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use codenotch_teams_schema::{ModelId, PlanCode, Provider, QuotaWindow, Tokens};
use serde::{Deserialize, Serialize};

use crate::filter::UsageKey;
use crate::journal::Binding;

pub const STATE_VERSION: u32 = 1;

/// Last snapshot queued for one account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotCursor {
    pub account_ref: String,
    pub provider: Provider,
    pub seq: u64,
    /// When it was queued (for the 6-hour resync).
    pub queued_at: i64,
    pub plan: PlanCode,
    /// Empty after a tombstone.
    pub windows: Vec<QuotaWindow>,
    /// The last snapshot was a tombstone.
    #[serde(default)]
    pub tombstone: bool,
}

/// Last revision queued for one `usage.hour` key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCursor {
    pub account_ref: String,
    pub provider: Provider,
    pub model: ModelId,
    pub hour_start: i64,
    pub rev: u64,
    pub tokens: Tokens,
}

impl UsageCursor {
    pub fn key(&self) -> UsageKey {
        UsageKey {
            account_ref: self.account_ref.clone(),
            provider: self.provider,
            model: self.model,
            hour_start: self.hour_start,
        }
    }
}

/// Everything Teams remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    pub version: u32,
    pub binding: Binding,
    /// Team name as the server reported it at pairing (shown in the settings window).
    pub team_name: String,
    /// First hour that may be sent (pairing or erase time, from the server).
    pub floor_at: i64,
    /// The member agreed to send the 45 days before `floor_at`.
    #[serde(default)]
    pub backfill: bool,
    #[serde(default)]
    pub last_sent_at: Option<i64>,
    #[serde(default)]
    pub last_heartbeat: Option<i64>,
    /// Highest outbox `seq` used.
    #[serde(default)]
    pub journal_seq: u64,
    #[serde(default)]
    pub snapshots: Vec<SnapshotCursor>,
    #[serde(default)]
    pub usage: Vec<UsageCursor>,
}

impl StateFile {
    pub fn new(binding: Binding, team_name: String, floor_at: i64) -> StateFile {
        StateFile {
            version: STATE_VERSION,
            binding,
            team_name,
            floor_at,
            backfill: false,
            last_sent_at: None,
            last_heartbeat: None,
            journal_seq: 0,
            snapshots: Vec::new(),
            usage: Vec::new(),
        }
    }

    /// `None` when the file does not exist. A file of another version or unreadable is an
    /// error (the caller treats the device as not paired and says so).
    pub fn load(path: &Path) -> io::Result<Option<StateFile>> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let state: StateFile = serde_json::from_slice(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "teams.json unreadable"))?;
        if state.version != STATE_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "teams.json version",
            ));
        }
        Ok(Some(state))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        codenotch_core::paths::write_atomic_checked(path, &bytes, || {
            crate::fault::fault(crate::fault::Step::State)
        })
    }
}

/// Cursors in memory, keyed for lookups.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cursors {
    pub snapshots: BTreeMap<String, SnapshotCursor>,
    pub usage: BTreeMap<UsageKey, UsageCursor>,
    pub journal_seq: u64,
}

impl Cursors {
    pub fn from_state(state: &StateFile) -> Cursors {
        Cursors {
            snapshots: state
                .snapshots
                .iter()
                .map(|c| (c.account_ref.clone(), c.clone()))
                .collect(),
            usage: state.usage.iter().map(|c| (c.key(), c.clone())).collect(),
            journal_seq: state.journal_seq,
        }
    }

    pub fn write_to(&self, state: &mut StateFile) {
        state.snapshots = self.snapshots.values().cloned().collect();
        state.usage = self.usage.values().cloned().collect();
        state.journal_seq = self.journal_seq;
    }

    pub fn next_seq(&mut self) -> u64 {
        self.journal_seq += 1;
        self.journal_seq
    }
}
