//! `analytics-cache.json`: the incremental per-file index.
//!
//! Format (version 1, compatible with the file the 0.3 app left behind):
//!
//! ```json
//! { "version": 1,
//!   "files": {
//!     "<absolute transcript path>": {
//!       "size": 123, "offset": 120,            // bytes seen / bytes parsed (complete lines)
//!       "recent": ["msg_…"],                   // last message ids (de-duplication), capped
//!       "total": null | {"input":…,"cached":…,"cache_write":…,"output":…},  // Codex cumulative
//!       "model": "…", "cwd": "…",              // last model / working dir seen in the file
//!       "buckets": [ {"hour": 1762941600, "source": "<scan root>", "cli": "claude"|"codex",
//!                     "model": "…", "project": "…",
//!                     "tokens": {"input":…,"output":…,"cache_write":…,"cache_read":…}} ],
//!       // 1.0 additions (optional, ignored by older readers):
//!       "mtime": 1762941234, "head": "256:0123abcd…", "last": {…}
//!     } } }
//! ```
//!
//! `tokens.cache_write` is the total of cache writes; the optional `tokens.cache_write_1h` is
//! the part of it written with a 1-hour TTL (priced higher).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;
/// How many message ids per file are remembered for de-duplication.
pub const RECENT_CAP: usize = 256;

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// Token counts as stored in the cache (snake_case keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    /// All cache writes (5-minute and 1-hour).
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub cache_read: u64,
    /// Part of `cache_write` written with a 1-hour TTL.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_write_1h: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
    pub fn add(&mut self, o: &Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
        self.cache_write_1h += o.cache_write_1h;
    }
    /// Component-wise `self - o`, saturating at 0.
    pub fn saturating_sub(&self, o: &Tokens) -> Tokens {
        Tokens {
            input: self.input.saturating_sub(o.input),
            output: self.output.saturating_sub(o.output),
            cache_write: self.cache_write.saturating_sub(o.cache_write),
            cache_read: self.cache_read.saturating_sub(o.cache_read),
            cache_write_1h: self.cache_write_1h.saturating_sub(o.cache_write_1h),
        }
    }
}

/// Codex cumulative totals (`info.total_token_usage`) as last seen in a rollout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodexTotal {
    /// Input including cached input.
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub cached: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub output: u64,
}

/// One hour × model × project of one file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Bucket {
    /// Unix seconds at the start of the (UTC-aligned) hour.
    pub hour: i64,
    /// Scan root the file was found under (`…\.claude\projects`, `…\.codex\sessions`).
    #[serde(default)]
    pub source: String,
    /// `"claude"` or `"codex"`.
    #[serde(default)]
    pub cli: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub tokens: Tokens,
}

/// Usage counted for the most recent Claude message, so a later streaming re-write of the same
/// message with larger counts adds only the difference.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LastMessage {
    pub id: String,
    pub hour: i64,
    pub model: String,
    pub project: String,
    pub tokens: Tokens,
}

/// Index state of one transcript file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FileEntry {
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub recent: Vec<String>,
    #[serde(default)]
    pub total: Option<CodexTotal>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub buckets: Vec<Bucket>,
    /// Last modification time seen (unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
    /// `"<len>:<fnv64 hex>"` of the first bytes, to notice a file replaced under the same path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<LastMessage>,
}

impl FileEntry {
    /// Forget everything parsed so far (file shrank or was replaced).
    pub fn reset(&mut self) {
        *self = FileEntry::default();
    }

    /// Add `tokens` to the bucket (hour, cli, model, project), creating it if needed.
    pub fn add(
        &mut self,
        hour: i64,
        source: &str,
        cli: &str,
        model: &str,
        project: &str,
        tokens: &Tokens,
    ) {
        if tokens.is_empty() {
            return;
        }
        // Buckets are appended in time order, so the match is almost always near the end.
        if let Some(b) =
            self.buckets.iter_mut().rev().find(|b| {
                b.hour == hour && b.cli == cli && b.model == model && b.project == project
            })
        {
            b.tokens.add(tokens);
            if b.source.is_empty() {
                b.source = source.to_string();
            }
            return;
        }
        self.buckets.push(Bucket {
            hour,
            source: source.to_string(),
            cli: cli.to_string(),
            model: model.to_string(),
            project: project.to_string(),
            tokens: *tokens,
        });
    }

    /// Remember a message id (bounded).
    pub fn remember(&mut self, id: &str) {
        self.recent.push(id.to_string());
        if self.recent.len() > RECENT_CAP {
            let extra = self.recent.len() - RECENT_CAP;
            self.recent.drain(..extra);
        }
    }
}

/// The whole cache file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Cache {
    pub version: u32,
    pub files: BTreeMap<String, FileEntry>,
}

impl Cache {
    /// Read the cache tolerantly: a missing/unreadable/foreign file is an empty cache, and a
    /// malformed file entry is dropped (that file is simply parsed again).
    pub fn load(path: &Path) -> Cache {
        let text = match std::fs::read(path) {
            Ok(t) => t,
            Err(_) => return Cache::empty(),
        };
        Self::parse(&text)
    }

    pub fn parse(bytes: &[u8]) -> Cache {
        let root: Value = match serde_json::from_slice(bytes) {
            Ok(v) => v,
            Err(_) => {
                log::warn!("analytics: cache unreadable, rebuilding");
                return Cache::empty();
            }
        };
        if root.get("version").and_then(Value::as_u64) != Some(u64::from(VERSION)) {
            return Cache::empty();
        }
        let mut cache = Cache::empty();
        if let Some(files) = root.get("files").and_then(Value::as_object) {
            for (path, entry) in files {
                if let Ok(e) = serde_json::from_value::<FileEntry>(entry.clone()) {
                    cache.files.insert(path.clone(), e);
                }
            }
        }
        cache
    }

    pub fn empty() -> Cache {
        Cache {
            version: VERSION,
            files: BTreeMap::new(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        codenotch_core::paths::write_atomic(path, &self.to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = r#"{"version":1,"files":{
        "C:\\t\\a.jsonl":{"size":10,"offset":10,"recent":["msg_1"],"total":null,"model":"claude-opus-5-5","cwd":"C:\\p",
          "buckets":[{"hour":3600,"source":"C:\\t","cli":"claude","model":"claude-opus-5-5","project":"C:\\p",
            "tokens":{"input":1,"output":2,"cache_write":3,"cache_read":4}}]},
        "C:\\t\\b.jsonl":{"size":5,"offset":5,"recent":[],"total":{"input":9,"cached":4,"cache_write":0,"output":2},
          "model":"gpt-x","cwd":"","buckets":[]},
        "C:\\t\\bad.jsonl":{"size":"oops"}
    }}"#;

    #[test]
    fn reads_the_old_format_and_drops_bad_entries() {
        let c = Cache::parse(OLD.as_bytes());
        assert_eq!(c.files.len(), 2);
        let a = &c.files["C:\\t\\a.jsonl"];
        assert_eq!(
            a.buckets[0].tokens,
            Tokens {
                input: 1,
                output: 2,
                cache_write: 3,
                cache_read: 4,
                cache_write_1h: 0
            }
        );
        assert_eq!(a.mtime, None);
        let b = &c.files["C:\\t\\b.jsonl"];
        assert_eq!(
            b.total,
            Some(CodexTotal {
                input: 9,
                cached: 4,
                cache_write: 0,
                output: 2
            })
        );
    }

    #[test]
    fn round_trips_with_the_same_keys() {
        let c = Cache::parse(OLD.as_bytes());
        let v: Value = serde_json::from_slice(&c.to_bytes()).expect("json");
        let a = &v["files"]["C:\\t\\a.jsonl"];
        for k in [
            "size", "offset", "recent", "total", "model", "cwd", "buckets",
        ] {
            assert!(a.get(k).is_some(), "missing {k}");
        }
        assert!(a["total"].is_null());
        let tok = a["buckets"][0]["tokens"].as_object().expect("tokens");
        let mut keys: Vec<_> = tok.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["cache_read", "cache_write", "input", "output"]);
        assert_eq!(Cache::parse(&c.to_bytes()), c);
    }

    #[test]
    fn foreign_or_broken_cache_is_empty() {
        assert!(Cache::parse(b"{\"version\":7,\"files\":{}}")
            .files
            .is_empty());
        assert!(Cache::parse(b"garbage").files.is_empty());
        assert!(Cache::load(Path::new("Z:\\does\\not\\exist.json"))
            .files
            .is_empty());
    }

    #[test]
    fn recent_is_capped() {
        let mut e = FileEntry::default();
        for i in 0..(RECENT_CAP + 10) {
            e.remember(&format!("m{i}"));
        }
        assert_eq!(e.recent.len(), RECENT_CAP);
        assert_eq!(e.recent[0], "m10");
    }
}
