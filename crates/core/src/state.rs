//! `state.json`: what the engine persists between runs (last readings, back-off deadlines,
//! alert bookkeeping). Owned and written by `codenotch-engine` only; other crates read the
//! types. Unknown keys survive a round trip. A missing or unreadable file gives an empty state.

use crate::model::{Reading, ReadingStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedState {
    #[serde(default = "version")]
    pub version: u32,
    /// Last reading per account id.
    #[serde(default)]
    pub readings: BTreeMap<String, Reading>,
    /// Rate-limit deadline per account id (unix secs).
    #[serde(default)]
    pub backoff_until: BTreeMap<String, i64>,
    /// Alert dedupe: key chosen by notify (e.g. "acc-1/session/critical") → last sent (unix secs).
    #[serde(default)]
    pub alerts_sent: BTreeMap<String, i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn version() -> u32 {
    STATE_VERSION
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            readings: BTreeMap::new(),
            backoff_until: BTreeMap::new(),
            alerts_sent: BTreeMap::new(),
            extra: Map::new(),
        }
    }
}

impl PersistedState {
    /// Load; missing or bad file → default (logged at warn level).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                log::warn!("state file unreadable, starting empty: {err}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Readings restored at start-up: anything with windows that was Fresh becomes Stale
    /// (persisted snapshots are never shown as live).
    pub fn restored_readings(&self) -> BTreeMap<String, Reading> {
        self.readings
            .iter()
            .map(|(id, r)| {
                let mut r = r.clone();
                if r.status == ReadingStatus::Fresh && !r.windows.is_empty() {
                    r.status = ReadingStatus::Stale;
                }
                (id.clone(), r)
            })
            .collect()
    }

    /// Atomic save.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        crate::paths::write_atomic(path, text.as_bytes())
            .map_err(|e| crate::Error::io(path.display().to_string(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Account;
    use crate::model::{LimitWindow, ProviderKind, WindowKind};

    #[test]
    fn round_trip_and_restore_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        assert_eq!(PersistedState::load(&path), PersistedState::default());
        let a = Account::new(ProviderKind::Codex, "x");
        let mut s = PersistedState::default();
        s.readings.insert(
            a.id.clone(),
            Reading::fresh(
                &a,
                5,
                vec![LimitWindow::new("primary", "Current session", WindowKind::Session).used(3.0)],
            ),
        );
        s.alerts_sent.insert("k".into(), 7);
        s.save(&path).unwrap();
        let loaded = PersistedState::load(&path);
        assert_eq!(loaded, s);
        assert_eq!(
            loaded.restored_readings()[&a.id].status,
            ReadingStatus::Stale
        );
        std::fs::write(&path, "nope").unwrap();
        assert_eq!(PersistedState::load(&path), PersistedState::default());
    }
}
