//! `teams-outbox.jsonl`: the durable queue of events waiting to be sent.
//!
//! Format: the first line is the binding header ([`Binding`] plus `"teamsOutbox": 1`); every
//! other line is a [`Record`] `{ "seq", "schemaVersion", "event" }`. A record is never
//! relabelled to another schema version.
//!
//! Rules:
//! * The file belongs to one pairing: if its header differs from the current binding (other
//!   server, team, device, generation, consent or data epoch) it is deleted unsent.
//! * Append = write the lines, then `sync_data`. Only after that does the caller advance its
//!   cursors. A failed write leaves the queue as it was.
//! * A truncated last line (crash during a write) is cut back to the last complete line
//!   before anything else is appended.
//! * Only the latest pending event of a key is kept (same `usage.hour` key: highest `rev`;
//!   same account snapshot: highest `seq`; heartbeat: latest). Older ones are superseded and
//!   disappear at the next compaction (or the next load).
//! * At most [`MAX_EVENTS`] events and [`MAX_BYTES`] bytes: the oldest are dropped and counted.
//! * Compaction rewrites the file atomically with the header and the pending records.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use codenotch_teams_schema::{
    RawEvent, KIND_DEVICE_HEARTBEAT, KIND_QUOTA_SNAPSHOT, KIND_USAGE_HOUR,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fault::{fault, Step};

/// Most events kept in the queue.
pub const MAX_EVENTS: usize = 10_000;
/// Most bytes kept in the queue.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// What the queue (and every event in it) is bound to. `Debug` leaves out the team id.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub origin: String,
    pub team_id: String,
    pub device_id: String,
    pub generation: u64,
    pub consent_version: u32,
    pub data_epoch: u32,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("origin", &self.origin)
            .field("generation", &self.generation)
            .field("consent_version", &self.consent_version)
            .field("data_epoch", &self.data_epoch)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    teams_outbox: u32,
    #[serde(flatten)]
    binding: Binding,
}

/// One queued event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    /// Position in the queue (monotonic per pairing).
    pub seq: u64,
    pub schema_version: u32,
    pub event: RawEvent,
}

impl Record {
    /// Supersession key and rank: a pending record is replaced by a later one with the same
    /// key and a rank at least as high.
    fn key(&self) -> (String, u64) {
        let p = &self.event.payload;
        let s = |k: &str| p.get(k).map(Value::to_string).unwrap_or_default();
        let n = |k: &str| p.get(k).and_then(Value::as_u64).unwrap_or(0);
        match self.event.kind.as_str() {
            KIND_USAGE_HOUR => (
                format!(
                    "u|{}|{}|{}|{}",
                    s("accountRef"),
                    s("provider"),
                    s("model"),
                    s("hourStart")
                ),
                n("rev"),
            ),
            KIND_QUOTA_SNAPSHOT => (format!("s|{}", s("accountRef")), n("seq")),
            KIND_DEVICE_HEARTBEAT => ("h".into(), self.seq),
            _ => (format!("x|{}", self.event.event_id), self.seq),
        }
    }
}

struct Pending {
    record: Record,
    key: String,
    rank: u64,
    size: u64,
}

/// The queue in memory, mirrored by the file.
pub struct Journal {
    path: PathBuf,
    binding: Binding,
    pending: Vec<Pending>,
    /// Bytes of complete lines in the file (0 = no file yet).
    valid_len: u64,
    /// The file holds lines no longer pending (superseded, sent, dropped).
    stale: bool,
    /// Events dropped because the queue was full, since the counter was last taken.
    dropped: u32,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal")
            .field("pending", &self.pending.len())
            .field("bytes", &self.bytes())
            .field("dropped", &self.dropped)
            .finish()
    }
}

impl Journal {
    /// Load the queue of `binding`. A missing file is an empty queue (no file is created). A
    /// file bound to anything else, or without a readable header, is deleted unsent.
    pub fn open(path: &Path, binding: &Binding) -> io::Result<Journal> {
        let mut journal = Journal {
            path: path.to_path_buf(),
            binding: binding.clone(),
            pending: Vec::new(),
            valid_len: 0,
            stale: false,
            dropped: 0,
        };
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(journal),
            Err(e) => return Err(e),
        };
        let complete = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        let mut lines = bytes[..complete].split(|b| *b == b'\n');
        let header_ok = lines
            .next()
            .and_then(|l| serde_json::from_slice::<Header>(l).ok())
            .is_some_and(|h| h.teams_outbox == 1 && h.binding == *binding);
        if !header_ok {
            std::fs::remove_file(path)?;
            return Ok(journal);
        }
        for line in lines.filter(|l| !l.is_empty()) {
            match serde_json::from_slice::<Record>(line) {
                Ok(record) => journal.insert(record, line.len() as u64 + 1),
                Err(_) => journal.stale = true,
            }
        }
        if complete < bytes.len() {
            // Truncated last line: cut it off now, before anything is appended.
            OpenOptions::new()
                .write(true)
                .open(path)?
                .set_len(complete as u64)?;
        }
        journal.valid_len = complete as u64;
        journal.enforce_cap();
        Ok(journal)
    }

    /// Delete the file (detach, team change, data erased). Missing is fine.
    pub fn destroy(path: &Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    /// Pending records, oldest first.
    pub fn pending(&self) -> impl Iterator<Item = &Record> {
        self.pending.iter().map(|p| &p.record)
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Bytes of the pending records.
    pub fn bytes(&self) -> u64 {
        self.pending.iter().map(|p| p.size).sum()
    }

    /// Highest `seq` pending (0 = none).
    pub fn last_seq(&self) -> u64 {
        self.pending.iter().map(|p| p.record.seq).max().unwrap_or(0)
    }

    /// Dropped events since the last call.
    pub fn take_dropped(&mut self) -> u32 {
        std::mem::take(&mut self.dropped)
    }

    /// Durably append `records`. On error nothing is pending that was not before (a partial
    /// line left in the file is cut off by the next append or load).
    pub fn append(&mut self, records: Vec<Record>) -> io::Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut lines: Vec<(Record, Vec<u8>)> = Vec::with_capacity(records.len());
        for r in records {
            let mut line = serde_json::to_vec(&r).map_err(io::Error::other)?;
            line.push(b'\n');
            lines.push((r, line));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&self.path)?;
        let len = file.metadata()?.len();
        let mut out = Vec::new();
        if self.valid_len == 0 || len == 0 {
            // New file (or one emptied by a crash before its header was complete).
            file.set_len(0)?;
            self.valid_len = 0;
            let header = Header {
                teams_outbox: 1,
                binding: self.binding.clone(),
            };
            out.extend(serde_json::to_vec(&header).map_err(io::Error::other)?);
            out.push(b'\n');
        } else if len != self.valid_len {
            file.set_len(self.valid_len)?;
        }
        for (_, line) in &lines {
            out.extend_from_slice(line);
        }
        use std::io::Seek;
        file.seek(io::SeekFrom::Start(self.valid_len))?;
        if fault(Step::Append).is_err() {
            // Simulated crash in the middle of the write.
            file.write_all(&out[..out.len() / 2])?;
            return Err(io::Error::other("injected append failure"));
        }
        file.write_all(&out)?;
        fault(Step::Sync)?;
        file.sync_data()?;
        self.valid_len += out.len() as u64;
        for (r, line) in lines {
            self.insert(r, line.len() as u64);
        }
        self.enforce_cap();
        Ok(())
    }

    /// Remove the pending records whose event id is in `ids`; returns how many.
    pub fn remove_ids(&mut self, ids: &HashSet<&str>) -> usize {
        self.retain(|r| !ids.contains(r.event.event_id.as_str()))
    }

    /// Keep only the records `keep` accepts; returns how many were removed.
    pub fn retain(&mut self, mut keep: impl FnMut(&Record) -> bool) -> usize {
        let before = self.pending.len();
        self.pending.retain(|p| keep(&p.record));
        let removed = before - self.pending.len();
        self.stale |= removed > 0;
        removed
    }

    /// The file holds lines that are no longer pending.
    pub fn needs_compaction(&self) -> bool {
        self.stale
    }

    /// Rewrite the file with the header and the pending records only (atomic). With nothing
    /// pending the file is deleted.
    pub fn compact(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            fault(Step::Compaction)?;
            Journal::destroy(&self.path)?;
            self.valid_len = 0;
            self.stale = false;
            return Ok(());
        }
        let header = Header {
            teams_outbox: 1,
            binding: self.binding.clone(),
        };
        let mut out = serde_json::to_vec(&header).map_err(io::Error::other)?;
        out.push(b'\n');
        for p in &self.pending {
            out.extend(serde_json::to_vec(&p.record).map_err(io::Error::other)?);
            out.push(b'\n');
        }
        codenotch_core::paths::write_atomic_checked(&self.path, &out, || fault(Step::Compaction))?;
        self.valid_len = out.len() as u64;
        self.stale = false;
        Ok(())
    }

    fn insert(&mut self, record: Record, size: u64) {
        let (key, rank) = record.key();
        if let Some(i) = self.pending.iter().position(|p| p.key == key) {
            if self.pending[i].rank > rank {
                self.stale = true;
                return;
            }
            self.pending.remove(i);
            self.stale = true;
        }
        self.pending.push(Pending {
            record,
            key,
            rank,
            size,
        });
    }

    fn enforce_cap(&mut self) {
        while self.pending.len() > MAX_EVENTS || self.bytes() > MAX_BYTES {
            // Oldest first: records are kept in queue order.
            self.pending.remove(0);
            self.dropped += 1;
            self.stale = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codenotch_teams_schema::{EventPayload, Heartbeat};

    pub(crate) fn binding() -> Binding {
        Binding {
            origin: "https://teams.example.test".into(),
            team_id: "team-a".into(),
            device_id: "6f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11".into(),
            generation: 1,
            consent_version: 1,
            data_epoch: 1,
        }
    }

    fn hb(seq: u64, queued: u32) -> Record {
        let id = uuid::Uuid::new_v4().to_string();
        let payload = EventPayload::Heartbeat(Heartbeat { queued, dropped: 0 });
        Record {
            seq,
            schema_version: 1,
            event: RawEvent::new(id, 100, &payload).unwrap(),
        }
    }

    #[test]
    fn missing_file_is_empty_and_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let j = Journal::open(&path, &binding()).unwrap();
        assert!(j.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn header_mismatch_destroys_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let mut j = Journal::open(&path, &binding()).unwrap();
        j.append(vec![hb(1, 0)]).unwrap();
        assert_eq!(Journal::open(&path, &binding()).unwrap().len(), 1);
        let changes: [fn(&mut Binding); 6] = [
            |b: &mut Binding| b.origin = "https://other.example.test".into(),
            |b: &mut Binding| b.team_id = "team-b".into(),
            |b: &mut Binding| b.device_id = "7f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11".into(),
            |b: &mut Binding| b.generation = 2,
            |b: &mut Binding| b.consent_version = 2,
            |b: &mut Binding| b.data_epoch = 2,
        ];
        for change in changes {
            let mut j = Journal::open(&path, &binding()).unwrap();
            if j.is_empty() {
                j.append(vec![hb(1, 0)]).unwrap();
            }
            let mut other = binding();
            change(&mut other);
            assert!(Journal::open(&path, &other).unwrap().is_empty());
            assert!(!path.exists());
        }
        std::fs::write(&path, "garbage\n{}\n").unwrap();
        assert!(Journal::open(&path, &binding()).unwrap().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn truncated_tail_is_cut_before_the_next_append() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let mut j = Journal::open(&path, &binding()).unwrap();
        j.append(vec![hb(1, 1)]).unwrap();
        let good = std::fs::read(&path).unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"seq\":2,\"schemaVer").unwrap();
        drop(f);
        let mut j = Journal::open(&path, &binding()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), good);
        assert_eq!(j.len(), 1);
        j.append(vec![hb(3, 2)]).unwrap();
        let j = Journal::open(&path, &binding()).unwrap();
        // Heartbeats supersede each other: only the latest stays.
        assert_eq!(j.pending().map(|r| r.seq).collect::<Vec<_>>(), [3]);
    }

    #[test]
    fn failed_write_enqueues_nothing_and_the_next_append_repairs_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let mut j = Journal::open(&path, &binding()).unwrap();
        j.append(vec![hb(1, 1)]).unwrap();
        crate::fault::arm(Step::Append);
        assert!(j.append(vec![hb(2, 2)]).is_err());
        assert_eq!(j.pending().map(|r| r.seq).collect::<Vec<_>>(), [1]);
        crate::fault::arm(Step::Sync);
        assert!(j.append(vec![hb(3, 3)]).is_err());
        assert_eq!(j.len(), 1);
        j.append(vec![hb(4, 4)]).unwrap();
        let reloaded = Journal::open(&path, &binding()).unwrap();
        assert_eq!(reloaded.pending().map(|r| r.seq).collect::<Vec<_>>(), [4]);
        assert!(!reloaded.needs_compaction() || reloaded.len() == 1);
    }

    #[test]
    fn cap_drops_the_oldest_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let mut j = Journal::open(&path, &binding()).unwrap();
        let mut batch = Vec::new();
        for seq in 1..=(MAX_EVENTS as u64 + 5) {
            let mut r = hb(seq, 0);
            // Distinct kinds of key so nothing supersedes: fake unknown-kind records.
            r.event.kind = "test.unique".into();
            batch.push(r);
        }
        j.append(batch).unwrap();
        assert_eq!(j.len(), MAX_EVENTS);
        assert_eq!(j.take_dropped(), 5);
        assert_eq!(j.pending().next().unwrap().seq, 6);
        j.compact().unwrap();
        assert_eq!(Journal::open(&path, &binding()).unwrap().len(), MAX_EVENTS);
    }

    #[test]
    fn compaction_failure_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("teams-outbox.jsonl");
        let mut j = Journal::open(&path, &binding()).unwrap();
        let (a, b) = (hb(1, 1), hb(2, 2));
        let mut b2 = b.clone();
        b2.event.kind = "test.unique".into();
        j.append(vec![a.clone(), b2]).unwrap();
        j.remove_ids(&[a.event.event_id.as_str()].into_iter().collect());
        assert!(j.needs_compaction());
        let before = std::fs::read(&path).unwrap();
        crate::fault::arm(Step::Compaction);
        assert!(j.compact().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        j.compact().unwrap();
        assert_eq!(Journal::open(&path, &binding()).unwrap().len(), 1);
        j.remove_ids(&HashSet::from(["00000000-0000-4000-8000-000000000000"]));
        assert_eq!(j.len(), 1, "unknown id: nothing removed");
        drop(b);
    }
}
