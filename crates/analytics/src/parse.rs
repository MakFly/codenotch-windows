//! Line parsers for Claude Code transcripts and Codex rollouts.
//!
//! Only numeric usage fields, model ids, `cwd` and timestamps are read; message content is
//! skipped by the deserialiser and never stored or logged.

use memchr::memmem;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::cache::{CodexTotal, FileEntry, LastMessage, Tokens};

/// Which CLI wrote a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    pub fn id(self) -> &'static str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        }
    }
}

/// What a line contributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Not a usage line (user turn, tool output, metadata …).
    Skipped,
    /// Context only (Codex `session_meta` / `turn_context`).
    Context,
    /// Usage added to a bucket.
    Counted,
    /// Same message seen again (nothing added, or only a streaming increase).
    Duplicate,
    /// Looked like a usage line but could not be parsed.
    Malformed,
}

pub const UNKNOWN_MODEL: &str = "unknown";

/// Number from any JSON number (negative / non-finite / other types → 0).
fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .map(|f| f as u64)
            })
            .unwrap_or(0),
        _ => 0,
    })
}

/// String from a JSON string; anything else → `None`.
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::String(s) => Some(s),
        _ => None,
    })
}

/// Unix seconds from an RFC 3339 timestamp.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.timestamp())
}

/// Start of the UTC-aligned hour containing `ts`.
pub fn hour_of(ts: i64) -> i64 {
    ts - ts.rem_euclid(3600)
}

// ---------------------------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type", default, deserialize_with = "lenient_string")]
    kind: Option<String>,
    #[serde(default)]
    message: Option<ClaudeMessage>,
    #[serde(default, deserialize_with = "lenient_string")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    timestamp: Option<String>,
    #[serde(rename = "requestId", default, deserialize_with = "lenient_string")]
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    #[serde(default, deserialize_with = "lenient_string")]
    id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    model: Option<String>,
    #[serde(default)]
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default, deserialize_with = "lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    output_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    cache_creation_input_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation: Option<CacheCreation>,
    #[serde(default, deserialize_with = "lenient_string")]
    speed: Option<String>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default, deserialize_with = "lenient_u64")]
    ephemeral_1h_input_tokens: u64,
}

/// Parse one Claude Code transcript line into `entry`.
///
/// Assistant lines carry `message.usage`; the same message (`message.id`, else `requestId`) is
/// written several times while streaming and is counted once. When a re-write of the most
/// recent message carries larger counts, only the increase is added.
pub fn claude_line(line: &[u8], source: &str, entry: &mut FileEntry) -> Outcome {
    if memmem::find(line, b"\"usage\"").is_none() {
        return Outcome::Skipped;
    }
    let parsed: ClaudeLine = match serde_json::from_slice(line) {
        Ok(p) => p,
        Err(_) => return Outcome::Malformed,
    };
    if parsed.kind.as_deref() != Some("assistant") {
        return Outcome::Skipped;
    }
    let Some(message) = parsed.message else {
        return Outcome::Skipped;
    };
    let Some(usage) = message.usage else {
        return Outcome::Skipped;
    };
    let model = message.model.filter(|m| !m.trim().is_empty());
    // Local error placeholders, not billed requests.
    if model.as_deref() == Some("<synthetic>") {
        return Outcome::Skipped;
    }
    if let Some(cwd) = parsed.cwd.as_deref().filter(|c| !c.is_empty()) {
        if entry.cwd != cwd {
            entry.cwd = cwd.to_string();
        }
    }
    let Some(ts) = parsed.timestamp.as_deref().and_then(parse_timestamp) else {
        return Outcome::Malformed;
    };
    let mut model = model.unwrap_or_else(|| UNKNOWN_MODEL.to_string());
    if usage.speed.as_deref() == Some("fast") {
        model.push_str("-fast");
    }
    let one_hour = usage
        .cache_creation
        .map(|c| c.ephemeral_1h_input_tokens)
        .unwrap_or(0);
    let tokens = Tokens {
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_write: usage.cache_creation_input_tokens,
        cache_read: usage.cache_read_input_tokens,
        cache_write_1h: one_hour.min(usage.cache_creation_input_tokens),
    };
    let id = message.id.filter(|s| !s.is_empty()).or_else(|| {
        parsed
            .request_id
            .filter(|s| !s.is_empty())
            .map(|r| format!("r:{r}"))
    });
    let project = if entry.cwd.is_empty() {
        String::new()
    } else {
        entry.cwd.clone()
    };

    if let Some(id) = id.as_deref() {
        if entry.recent.iter().rev().any(|r| r == id) {
            // Streaming re-write: add only what grew, to the bucket the message was counted in.
            if let Some(last) = entry.last.clone().filter(|l| l.id == id) {
                let grown = tokens.saturating_sub(&last.tokens);
                if !grown.is_empty() {
                    entry.add(
                        last.hour,
                        source,
                        Cli::Claude.id(),
                        &last.model,
                        &last.project,
                        &grown,
                    );
                    let mut merged = last.tokens;
                    merged.add(&grown);
                    entry.last = Some(LastMessage {
                        tokens: merged,
                        ..last
                    });
                }
            }
            return Outcome::Duplicate;
        }
        entry.remember(id);
    }
    let hour = hour_of(ts);
    entry.add(hour, source, Cli::Claude.id(), &model, &project, &tokens);
    entry.last = id.map(|id| LastMessage {
        id: id.to_string(),
        hour,
        model: model.clone(),
        project,
        tokens,
    });
    entry.model = model;
    Outcome::Counted
}

// ---------------------------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type", default, deserialize_with = "lenient_string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type", default, deserialize_with = "lenient_string")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    cwd: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    model: Option<String>,
    #[serde(default)]
    info: Option<CodexInfo>,
}

#[derive(Deserialize)]
struct CodexInfo {
    #[serde(default)]
    total_token_usage: Option<CodexUsage>,
}

#[derive(Deserialize)]
struct CodexUsage {
    #[serde(default, deserialize_with = "lenient_u64")]
    input_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    cached_input_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    cache_write_input_tokens: u64,
    #[serde(default, deserialize_with = "lenient_u64")]
    output_tokens: u64,
}

/// Bucket tokens for the step from cumulative total `prev` to `next`.
///
/// Codex `input_tokens` includes cached input, so uncached input = input − cached and cached
/// input is priced as a cache read. A total that went *down* (counter restarted) counts the
/// new total from zero.
pub fn codex_delta(prev: Option<CodexTotal>, next: CodexTotal) -> Tokens {
    let base = match prev {
        Some(p)
            if next.input >= p.input
                && next.cached >= p.cached
                && next.cache_write >= p.cache_write
                && next.output >= p.output =>
        {
            p
        }
        _ => CodexTotal::default(),
    };
    let input = next.input - base.input;
    let cached = next.cached - base.cached;
    Tokens {
        input: input.saturating_sub(cached),
        output: next.output - base.output,
        cache_write: next.cache_write - base.cache_write,
        cache_read: cached,
        cache_write_1h: 0,
    }
}

/// Parse one Codex rollout line into `entry`.
///
/// `session_meta` / `turn_context` lines update the file's working dir and model;
/// `event_msg` `token_count` lines carry cumulative totals whose delta is bucketed.
pub fn codex_line(line: &[u8], source: &str, entry: &mut FileEntry) -> Outcome {
    let relevant = memmem::find(line, b"token_count").is_some()
        || memmem::find(line, b"turn_context").is_some()
        || memmem::find(line, b"session_meta").is_some();
    if !relevant {
        return Outcome::Skipped;
    }
    let parsed: CodexLine = match serde_json::from_slice(line) {
        Ok(p) => p,
        Err(_) => return Outcome::Malformed,
    };
    let Some(payload) = parsed.payload else {
        return Outcome::Skipped;
    };
    let kind = parsed.kind.as_deref().unwrap_or("");
    let pkind = payload.kind.as_deref().unwrap_or("");

    if kind == "session_meta" || kind == "turn_context" || pkind == "turn_context" {
        if let Some(cwd) = payload.cwd.filter(|c| !c.is_empty()) {
            entry.cwd = cwd;
        }
        if let Some(model) = payload.model.filter(|m| !m.trim().is_empty()) {
            entry.model = model;
        }
        return Outcome::Context;
    }
    if pkind != "token_count" {
        return Outcome::Skipped;
    }
    // Rate-limit-only ticks have `info: null`.
    let Some(total) = payload.info.and_then(|i| i.total_token_usage) else {
        return Outcome::Skipped;
    };
    let next = CodexTotal {
        input: total.input_tokens,
        cached: total.cached_input_tokens,
        cache_write: total.cache_write_input_tokens,
        output: total.output_tokens,
    };
    let delta = codex_delta(entry.total, next);
    if delta.is_empty() {
        entry.total = Some(next);
        return Outcome::Duplicate;
    }
    let Some(ts) = parsed.timestamp.as_deref().and_then(parse_timestamp) else {
        return Outcome::Malformed;
    };
    entry.total = Some(next);
    let model = if entry.model.is_empty() {
        UNKNOWN_MODEL.to_string()
    } else {
        entry.model.clone()
    };
    let project = entry.cwd.clone();
    entry.add(
        hour_of(ts),
        source,
        Cli::Codex.id(),
        &model,
        &project,
        &delta,
    );
    Outcome::Counted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(id: &str, ts: &str, input: u64, output: u64, cw: u64, cr: u64) -> String {
        format!(
            r#"{{"type":"assistant","cwd":"C:\\proj","timestamp":"{ts}","requestId":"req_{id}","message":{{"id":"{id}","model":"claude-opus-5-5","role":"assistant","content":[{{"type":"text","text":"secret words"}}],"usage":{{"input_tokens":{input},"output_tokens":{output},"cache_creation_input_tokens":{cw},"cache_read_input_tokens":{cr},"cache_creation":{{"ephemeral_1h_input_tokens":{h},"ephemeral_5m_input_tokens":0}},"service_tier":"standard","speed":"standard"}}}}}}"#,
            h = cw / 2
        )
    }

    #[test]
    fn claude_counts_usage_and_context() {
        let mut e = FileEntry::default();
        let line = claude("msg_1", "2026-11-12T10:15:00.000Z", 10, 20, 100, 1000);
        assert_eq!(
            claude_line(line.as_bytes(), "root", &mut e),
            Outcome::Counted
        );
        assert_eq!(e.buckets.len(), 1);
        let b = &e.buckets[0];
        assert_eq!(b.hour, parse_timestamp("2026-11-12T10:00:00Z").expect("ts"));
        assert_eq!(
            (
                b.cli.as_str(),
                b.model.as_str(),
                b.project.as_str(),
                b.source.as_str()
            ),
            ("claude", "claude-opus-5-5", "C:\\proj", "root")
        );
        assert_eq!(
            b.tokens,
            Tokens {
                input: 10,
                output: 20,
                cache_write: 100,
                cache_read: 1000,
                cache_write_1h: 50
            }
        );
        assert_eq!(e.model, "claude-opus-5-5");
        assert_eq!(e.cwd, "C:\\proj");
        assert_eq!(e.recent, ["msg_1"]);
    }

    #[test]
    fn claude_dedups_by_message_id_and_adds_streaming_growth() {
        let mut e = FileEntry::default();
        let a = claude("msg_1", "2026-11-12T10:15:00Z", 10, 20, 0, 5);
        let a_again = claude("msg_1", "2026-11-12T11:15:00Z", 10, 20, 0, 5);
        let a_grown = claude("msg_1", "2026-11-12T11:16:00Z", 10, 50, 0, 5);
        assert_eq!(claude_line(a.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(
            claude_line(a_again.as_bytes(), "r", &mut e),
            Outcome::Duplicate
        );
        assert_eq!(
            claude_line(a_grown.as_bytes(), "r", &mut e),
            Outcome::Duplicate
        );
        assert_eq!(e.buckets.len(), 1, "growth goes to the first hour");
        assert_eq!(e.buckets[0].tokens.output, 50);
        assert_eq!(e.buckets[0].tokens.input, 10);
        // A smaller re-write never subtracts.
        let a_small = claude("msg_1", "2026-11-12T11:17:00Z", 1, 1, 0, 0);
        assert_eq!(
            claude_line(a_small.as_bytes(), "r", &mut e),
            Outcome::Duplicate
        );
        assert_eq!(e.buckets[0].tokens.output, 50);
        // Another message is counted.
        let b = claude("msg_2", "2026-11-12T10:20:00Z", 1, 2, 3, 4);
        assert_eq!(claude_line(b.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(e.buckets[0].tokens.output, 52);
    }

    #[test]
    fn claude_skips_non_usage_synthetic_and_flags_malformed() {
        let mut e = FileEntry::default();
        assert_eq!(
            claude_line(
                br#"{"type":"user","message":{"content":"hi"}}"#,
                "r",
                &mut e
            ),
            Outcome::Skipped
        );
        assert_eq!(
            claude_line(
                br#"{"type":"user","toolUseResult":{"usage":{"input_tokens":5}}}"#,
                "r",
                &mut e
            ),
            Outcome::Skipped
        );
        let synthetic = r#"{"type":"assistant","timestamp":"2026-11-12T10:00:00Z","message":{"id":"x","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0}}}"#;
        assert_eq!(
            claude_line(synthetic.as_bytes(), "r", &mut e),
            Outcome::Skipped
        );
        assert_eq!(
            claude_line(br#"{"type":"assistant","usage" broken"#, "r", &mut e),
            Outcome::Malformed
        );
        let no_ts =
            r#"{"type":"assistant","message":{"id":"y","model":"m","usage":{"input_tokens":1}}}"#;
        assert_eq!(
            claude_line(no_ts.as_bytes(), "r", &mut e),
            Outcome::Malformed
        );
        assert!(e.buckets.is_empty());
    }

    #[test]
    fn claude_tolerates_odd_field_types_and_fast_mode() {
        let mut e = FileEntry::default();
        let line = r#"{"type":"assistant","cwd":7,"timestamp":"2026-11-12T10:00:00+01:00","message":{"id":"z","model":"claude-opus-5-5","usage":{"input_tokens":"many","output_tokens":3.0,"cache_read_input_tokens":-4,"speed":"fast"}}}"#;
        assert_eq!(claude_line(line.as_bytes(), "r", &mut e), Outcome::Counted);
        let b = &e.buckets[0];
        assert_eq!(b.model, "claude-opus-5-5-fast");
        assert_eq!(
            b.tokens,
            Tokens {
                input: 0,
                output: 3,
                cache_write: 0,
                cache_read: 0,
                cache_write_1h: 0
            }
        );
        assert_eq!(b.hour, parse_timestamp("2026-11-12T09:00:00Z").expect("ts"));
        assert_eq!(b.project, "");
    }

    #[test]
    fn claude_without_ids_counts_every_line() {
        let mut e = FileEntry::default();
        let line = r#"{"type":"assistant","timestamp":"2026-11-12T10:00:00Z","message":{"model":"m","usage":{"input_tokens":1}}}"#;
        assert_eq!(claude_line(line.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(claude_line(line.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(e.buckets[0].tokens.input, 2);
        assert!(e.recent.is_empty());
    }

    fn token_count(ts: &str, input: u64, cached: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":{t}}},"last_token_usage":{{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1}}}},"rate_limits":null}}}}"#,
            t = input + output
        )
    }

    #[test]
    fn codex_buckets_deltas_of_cumulative_totals() {
        let mut e = FileEntry::default();
        let meta = r#"{"timestamp":"2026-11-12T09:59:00Z","type":"session_meta","payload":{"id":"s","cwd":"C:\\code","base_instructions":{"text":"long"}}}"#;
        let ctx = r#"{"timestamp":"2026-11-12T09:59:01Z","type":"turn_context","payload":{"cwd":"C:\\code","model":"gpt-5-codex","effort":"high"}}"#;
        assert_eq!(codex_line(meta.as_bytes(), "r", &mut e), Outcome::Context);
        assert_eq!(codex_line(ctx.as_bytes(), "r", &mut e), Outcome::Context);
        let t1 = token_count("2026-11-12T10:00:05Z", 1000, 600, 50);
        let t1_again = token_count("2026-11-12T10:00:06Z", 1000, 600, 50);
        let t2 = token_count("2026-11-12T11:30:00Z", 1500, 1000, 80);
        assert_eq!(codex_line(t1.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(
            codex_line(t1_again.as_bytes(), "r", &mut e),
            Outcome::Duplicate
        );
        assert_eq!(codex_line(t2.as_bytes(), "r", &mut e), Outcome::Counted);
        assert_eq!(e.buckets.len(), 2);
        assert_eq!(
            e.buckets[0].tokens,
            Tokens {
                input: 400,
                output: 50,
                cache_write: 0,
                cache_read: 600,
                cache_write_1h: 0
            }
        );
        assert_eq!(
            e.buckets[1].tokens,
            Tokens {
                input: 100,
                output: 30,
                cache_write: 0,
                cache_read: 400,
                cache_write_1h: 0
            }
        );
        assert_eq!(
            (
                e.buckets[1].model.as_str(),
                e.buckets[1].project.as_str(),
                e.buckets[1].cli.as_str()
            ),
            ("gpt-5-codex", "C:\\code", "codex")
        );
        assert_eq!(
            e.total,
            Some(CodexTotal {
                input: 1500,
                cached: 1000,
                cache_write: 0,
                output: 80
            })
        );
    }

    #[test]
    fn codex_counter_reset_and_null_info() {
        let mut e = FileEntry::default();
        let tick = r#"{"timestamp":"2026-11-12T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#;
        assert_eq!(codex_line(tick.as_bytes(), "r", &mut e), Outcome::Skipped);
        assert_eq!(
            codex_line(
                token_count("2026-11-12T10:00:00Z", 500, 0, 10).as_bytes(),
                "r",
                &mut e
            ),
            Outcome::Counted
        );
        // Totals restart lower: the new total counts from zero.
        assert_eq!(
            codex_line(
                token_count("2026-11-12T10:10:00Z", 100, 0, 5).as_bytes(),
                "r",
                &mut e
            ),
            Outcome::Counted
        );
        assert_eq!(e.buckets[0].tokens.input, 600);
        assert_eq!(e.buckets[0].tokens.output, 15);
        assert_eq!(e.buckets[0].model, UNKNOWN_MODEL);
        assert_eq!(
            codex_line(
                b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\"",
                "r",
                &mut e
            ),
            Outcome::Malformed
        );
        assert_eq!(
            codex_line(
                br#"{"type":"response_item","payload":{"type":"message"}}"#,
                "r",
                &mut e
            ),
            Outcome::Skipped
        );
    }

    #[test]
    fn codex_delta_rules() {
        let a = CodexTotal {
            input: 100,
            cached: 40,
            cache_write: 5,
            output: 10,
        };
        let b = CodexTotal {
            input: 150,
            cached: 60,
            cache_write: 5,
            output: 12,
        };
        assert_eq!(
            codex_delta(None, a),
            Tokens {
                input: 60,
                output: 10,
                cache_write: 5,
                cache_read: 40,
                cache_write_1h: 0
            }
        );
        assert_eq!(
            codex_delta(Some(a), b),
            Tokens {
                input: 30,
                output: 2,
                cache_write: 0,
                cache_read: 20,
                cache_write_1h: 0
            }
        );
        assert!(codex_delta(Some(b), b).is_empty());
        assert_eq!(codex_delta(Some(b), a), codex_delta(None, a));
    }

    #[test]
    fn hours_align_to_utc() {
        assert_eq!(hour_of(7199), 3600);
        assert_eq!(hour_of(-1), -3600);
        assert_eq!(parse_timestamp("garbage"), None);
    }
}
