//! Token usage read from the logs Claude Code and Codex keep on this PC, for the Analytics
//! page. Only numbers, model names and working folders are extracted, never message text.
//! Each file is read from where the previous scan stopped; its requests are summed into
//! hourly buckets kept with the read position in `analytics-cache.json`.
use crate::model::Account;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Seek, SeekFrom},
    ops::AddAssign,
    path::{Path, PathBuf},
};

/// Buckets older than this are dropped; the page shows 30 days at most.
const KEEP: i64 = 35 * 86_400;
/// Claude writes one line per content block, all carrying the same usage: the last
/// response ids seen in a file are enough to count each response once.
const RECENT_IDS: usize = 256;
/// Bumped when the cache layout or the counting changes, so old caches are rebuilt.
const CACHE_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    /// Uncached input.
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, o: Self) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
    }
}

/// The requests of one hour, for one log folder, model and project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bucket {
    /// Start of the hour, Unix seconds (UTC).
    pub hour: i64,
    /// The log folder the requests come from; `Source::dir`.
    pub source: String,
    pub cli: Cli,
    pub model: String,
    /// Working folder of the session.
    pub project: String,
    pub tokens: Tokens,
}

/// A log folder to read, and the accounts it belongs to.
#[derive(Clone, Debug)]
pub struct Source {
    pub cli: Cli,
    pub dir: PathBuf,
    pub accounts: Vec<String>,
}

impl Source {
    pub fn key(&self) -> String {
        self.dir.to_string_lossy().to_string()
    }
}

/// Where each enabled Claude Code and Codex account keeps its logs; accounts sharing a
/// profile share one source, so its files are read once.
pub fn sources(accounts: &[Account]) -> Vec<Source> {
    let mut sources: Vec<Source> = vec![];
    for account in accounts.iter().filter(|a| a.enabled) {
        let (cli, sub) = match account.provider.as_str() {
            "claude" => (Cli::Claude, "projects"),
            "codex" => (Cli::Codex, "sessions"),
            _ => continue,
        };
        let profile = if account.profile_path.is_empty() {
            crate::login::default_profile(&account.provider)
        } else {
            PathBuf::from(&account.profile_path)
        };
        let dir = profile.join(sub);
        let key = dir.to_string_lossy().to_lowercase();
        match sources.iter_mut().find(|s| s.dir.to_string_lossy().to_lowercase() == key) {
            Some(s) => s.accounts.push(account.id.clone()),
            None => sources.push(Source { cli, dir, accounts: vec![account.id.clone()] }),
        }
    }
    sources
}

/// How far a file has been read, and what its next lines need to be counted.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct FileState {
    size: u64,
    offset: u64,
    /// Claude: the last response ids counted.
    #[serde(default)]
    recent: VecDeque<String>,
    /// Codex: the session's running total at the previous `token_count`.
    #[serde(default)]
    total: Option<CodexTotal>,
    #[serde(default)]
    model: String,
    #[serde(default)]
    cwd: String,
    /// What this file added, so a rewritten file can be counted again from scratch.
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct CodexTotal {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Cache {
    version: u32,
    files: HashMap<String, FileState>,
}

impl Cache {
    pub fn path() -> Option<PathBuf> {
        Some(crate::store::config_path()?.with_file_name("analytics-cache.json"))
    }

    /// A missing, unreadable or outdated cache starts over: the logs are the source of truth.
    pub fn load(path: Option<&Path>) -> Self {
        path.and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
            .filter(|c| c.version == CACHE_VERSION)
            .unwrap_or(Cache { version: CACHE_VERSION, files: HashMap::new() })
    }

    pub fn save(&self, path: Option<&Path>) {
        let Some(path) = path else {
            return;
        };
        let temporary = path.with_extension("tmp");
        if serde_json::to_vec(self)
            .ok()
            .is_some_and(|b| std::fs::write(&temporary, b).is_ok())
        {
            let _ = std::fs::rename(temporary, path);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TokenReport {
    pub buckets: Vec<Bucket>,
    pub scanned_at: i64,
    /// Lines that looked like usage but could not be read.
    pub skipped: u32,
    /// Each source and whether its folder exists.
    pub sources: Vec<(Source, bool)>,
}

/// Reads what changed in the logs of `sources` since `cache` and returns the buckets of the
/// last 35 days. Slow on a first run (hundreds of MB): call it off the UI thread.
pub fn scan(sources: &[Source], mut cache: Cache, now: i64) -> (TokenReport, Cache) {
    let since = now - KEEP;
    let mut report = TokenReport { scanned_at: now, ..Default::default() };
    let mut seen = HashSet::new();
    for source in sources {
        let exists = source.dir.is_dir();
        report.sources.push((source.clone(), exists));
        if !exists {
            continue;
        }
        let mut files = vec![];
        collect_jsonl(&source.dir, &mut files);
        for file in files {
            let key = file.to_string_lossy().to_string();
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            // Untouched for longer than the kept span: nothing in it is still shown.
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(now, |d| d.as_secs() as i64);
            if modified < since && !cache.files.contains_key(&key) {
                continue;
            }
            seen.insert(key.clone());
            let state = cache.files.entry(key).or_default();
            if meta.len() < state.offset {
                // Rewritten or truncated: count it again from the start.
                *state = FileState::default();
            }
            if meta.len() > state.offset {
                report.skipped += read_file(&file, source, state, since);
            }
            state.size = meta.len();
        }
    }
    // Files deleted, or no longer under a tracked account.
    cache.files.retain(|path, _| seen.contains(path));
    let mut merged: HashMap<(i64, String, Cli, String, String), Tokens> = HashMap::new();
    for state in cache.files.values_mut() {
        state.buckets.retain(|b| b.hour >= since);
        for b in &state.buckets {
            *merged
                .entry((b.hour, b.source.clone(), b.cli, b.model.clone(), b.project.clone()))
                .or_default() += b.tokens;
        }
    }
    report.buckets = merged
        .into_iter()
        .map(|((hour, source, cli, model, project), tokens)| Bucket { hour, source, cli, model, project, tokens })
        .collect();
    report.buckets.sort_by(|a, b| {
        (a.hour, &a.source, &a.model, &a.project).cmp(&(b.hour, &b.source, &b.model, &b.project))
    });
    (report, cache)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => collect_jsonl(&path, out),
            Ok(t) if t.is_file() && path.extension().is_some_and(|e| e == "jsonl") => out.push(path),
            _ => {}
        }
    }
}

/// Counts the complete lines after `state.offset`; a line still being written is left for
/// the next scan. Returns how many usage lines could not be read.
fn read_file(path: &Path, source: &Source, state: &mut FileState, since: i64) -> u32 {
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut bytes = vec![];
    if file.seek(SeekFrom::Start(state.offset)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return 0;
    }
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
        return 0;
    };
    let mut skipped = 0;
    for line in bytes[..end].split(|b| *b == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        let counted = match source.cli {
            Cli::Claude => claude_line(line, state),
            Cli::Codex => codex_line(line, state),
        };
        match counted {
            Line::Ignored => {}
            Line::Broken => skipped += 1,
            Line::Usage { at, tokens } if at >= since && tokens.total() > 0 => {
                let hour = at - at.rem_euclid(3600);
                let (model, project) = (state.model.clone(), state.cwd.clone());
                match state
                    .buckets
                    .iter_mut()
                    .find(|b| b.hour == hour && b.model == model && b.project == project)
                {
                    Some(b) => b.tokens += tokens,
                    None => state.buckets.push(Bucket {
                        hour,
                        source: source.key(),
                        cli: source.cli,
                        model,
                        project,
                        tokens,
                    }),
                }
            }
            Line::Usage { .. } => {}
        }
    }
    state.offset += end as u64 + 1;
    skipped
}

enum Line {
    Ignored,
    Broken,
    Usage { at: i64, tokens: Tokens },
}

fn timestamp(raw: Option<&str>) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw?).ok().map(|d| d.timestamp())
}

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

/// An assistant response: `message.usage` with its model, once per `message.id`.
fn claude_line(line: &str, state: &mut FileState) -> Line {
    if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
        return Line::Ignored;
    }
    let Ok(parsed) = serde_json::from_str::<ClaudeLine>(line) else {
        return Line::Broken;
    };
    let (Some("assistant"), Some(message)) = (parsed.kind.as_deref(), parsed.message) else {
        return Line::Ignored;
    };
    let (Some(usage), Some(at)) = (message.usage, timestamp(parsed.timestamp.as_deref())) else {
        return Line::Broken;
    };
    let model = message.model.unwrap_or_default();
    // Local notices Claude Code writes as assistant lines, not API calls.
    if model.is_empty() || model == "<synthetic>" {
        return Line::Ignored;
    }
    if let Some(id) = message.id {
        let id = format!("{id}:{}", parsed.request_id.unwrap_or_default());
        if state.recent.contains(&id) {
            return Line::Ignored;
        }
        state.recent.push_back(id);
        if state.recent.len() > RECENT_IDS {
            state.recent.pop_front();
        }
    }
    state.model = model;
    if let Some(cwd) = parsed.cwd {
        state.cwd = cwd;
    }
    Line::Usage {
        at,
        tokens: Tokens {
            input: usage.input_tokens.unwrap_or(0),
            output: usage.output_tokens.unwrap_or(0),
            cache_write: usage.cache_creation_input_tokens.unwrap_or(0),
            cache_read: usage.cache_read_input_tokens.unwrap_or(0),
        },
    }
}

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<serde_json::Value>,
}

/// Codex reports a running total per session: each `token_count` adds what changed since
/// the previous one, so a repeated count adds nothing.
fn codex_line(line: &str, state: &mut FileState) -> Line {
    if !line.contains("\"session_meta\"") && !line.contains("\"turn_context\"") && !line.contains("\"token_count\"") {
        return Line::Ignored;
    }
    let Ok(parsed) = serde_json::from_str::<CodexLine>(line) else {
        return Line::Broken;
    };
    let Some(payload) = parsed.payload else {
        return Line::Ignored;
    };
    match parsed.kind.as_deref() {
        Some("session_meta") | Some("turn_context") => {
            if let Some(cwd) = payload["cwd"].as_str() {
                state.cwd = cwd.to_owned();
            }
            if let Some(model) = payload["model"].as_str() {
                state.model = model.to_owned();
            }
            Line::Ignored
        }
        Some("event_msg") if payload["type"] == "token_count" => {
            let usage = &payload["info"]["total_token_usage"];
            if !usage.is_object() {
                // Rate-limit only updates carry no usage.
                return Line::Ignored;
            }
            let Some(at) = timestamp(parsed.timestamp.as_deref()) else {
                return Line::Broken;
            };
            let field = |name: &str| usage[name].as_u64().unwrap_or(0);
            let total = CodexTotal {
                input: field("input_tokens"),
                cached: field("cached_input_tokens"),
                cache_write: field("cache_write_input_tokens"),
                output: field("output_tokens"),
            };
            let previous = state.total.unwrap_or_default();
            // A total that went down starts a new count (a reset session).
            let base = if total.input < previous.input || total.output < previous.output {
                CodexTotal::default()
            } else {
                previous
            };
            state.total = Some(total);
            let input = total.input - base.input;
            let cached = total.cached.saturating_sub(base.cached);
            let cache_write = total.cache_write.saturating_sub(base.cache_write);
            Line::Usage {
                at,
                tokens: Tokens {
                    // Codex input counts its cached part too.
                    input: input.saturating_sub(cached + cache_write),
                    output: total.output - base.output,
                    cache_write,
                    cache_read: cached,
                },
            }
        }
        _ => Line::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codenotch-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn claude(id: &str, at: &str, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{at}","cwd":"C:\\dev\\app","requestId":"r-{id}","message":{{"id":"{id}","model":"claude-opus-5-5","content":[{{"type":"text","text":"secret"}}],"usage":{{"input_tokens":10,"output_tokens":{output},"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}}}}"#
        ) + "\n"
    }

    fn total(report: &TokenReport) -> Tokens {
        let mut sum = Tokens::default();
        for b in &report.buckets {
            sum += b.tokens;
        }
        sum
    }

    const NOW: i64 = 1_793_000_000; // 2026-10-26

    #[test]
    fn claude_counts_each_response_once_and_resumes() {
        let dir = folder("claude");
        let file = dir.join("session.jsonl");
        let at = "2026-10-25T10:15:00Z";
        // Two content blocks of one response, then a line still being written.
        append(&file, &(claude("m1", at, 5) + &claude("m1", at, 5) + "not json but usage \"usage\" \"assistant\"\n"));
        append(&file, &claude("m2", at, 7)[..40]);
        let source = Source { cli: Cli::Claude, dir: dir.clone(), accounts: vec!["a".into()] };
        let (report, cache) = scan(&[source.clone()], Cache::default(), NOW);
        assert_eq!(total(&report), Tokens { input: 10, output: 5, cache_write: 100, cache_read: 1000 });
        assert_eq!(report.skipped, 1);
        assert_eq!(report.buckets[0].model, "claude-opus-5-5");
        assert_eq!(report.buckets[0].project, "C:\\dev\\app");
        // The partial line is completed: only it is read on the next scan.
        append(&file, &claude("m2", at, 7)[40..]);
        let (report, _) = scan(&[source], cache, NOW);
        assert_eq!(total(&report).output, 12);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn codex_adds_differences_of_its_running_total() {
        let dir = folder("codex");
        let file = dir.join("2026").join("rollout.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let count = |input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"timestamp":"2026-10-25T10:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
            ) + "\n"
        };
        append(&file, r#"{"timestamp":"2026-10-25T09:59:00Z","type":"session_meta","payload":{"cwd":"C:\\dev\\api"}}"#);
        append(&file, "\n");
        append(&file, r#"{"timestamp":"2026-10-25T09:59:00Z","type":"turn_context","payload":{"model":"gpt-5-codex","cwd":"C:\\dev\\api"}}"#);
        append(&file, "\n");
        append(&file, &(count(1000, 800, 50) + &count(1000, 800, 50) + &count(1500, 1200, 80)));
        let source = Source { cli: Cli::Codex, dir: dir.clone(), accounts: vec!["b".into()] };
        let (report, _) = scan(&[source], Cache::default(), NOW);
        assert_eq!(total(&report), Tokens { input: 300, output: 80, cache_write: 0, cache_read: 1200 });
        assert_eq!(report.buckets[0].model, "gpt-5-codex");
        assert_eq!(report.buckets[0].project, "C:\\dev\\api");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rewritten_files_are_counted_again_and_old_ones_dropped() {
        let dir = folder("rewrite");
        let file = dir.join("s.jsonl");
        append(&file, &(claude("m1", "2026-10-25T10:00:00Z", 5) + &claude("m2", "2026-10-25T11:00:00Z", 5)));
        let source = Source { cli: Cli::Claude, dir: dir.clone(), accounts: vec![] };
        let (_, cache) = scan(&[source.clone()], Cache::default(), NOW);
        std::fs::write(&file, claude("m3", "2026-10-25T12:00:00Z", 9)).unwrap();
        let (report, cache) = scan(&[source.clone()], cache, NOW);
        assert_eq!(total(&report).output, 9);
        // 40 days later, the buckets are past the kept span.
        let (report, _) = scan(&[source], cache, NOW + 40 * 86_400);
        assert!(report.buckets.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    #[ignore = "reads this PC's Claude Code and Codex logs; prints totals only"]
    fn scan_local_logs() {
        let home = dirs::home_dir().unwrap();
        let list = vec![
            Source { cli: Cli::Claude, dir: home.join(".claude").join("projects"), accounts: vec![] },
            Source { cli: Cli::Codex, dir: home.join(".codex").join("sessions"), accounts: vec![] },
        ];
        let now = chrono::Utc::now().timestamp();
        let started = std::time::Instant::now();
        let (report, cache) = scan(&list, Cache::default(), now);
        let first = started.elapsed();
        let started = std::time::Instant::now();
        let (again, _) = scan(&list, cache, now);
        println!("first scan {first:?}, second {:?}", started.elapsed());
        for cli in [Cli::Claude, Cli::Codex] {
            let mut sum = Tokens::default();
            let mut models = HashSet::new();
            for b in report.buckets.iter().filter(|b| b.cli == cli) {
                sum += b.tokens;
                models.insert(b.model.clone());
            }
            println!("{cli:?}: {sum:?}, models {models:?}");
        }
        println!("buckets {}, skipped {}", report.buckets.len(), report.skipped);
        assert_eq!(report.buckets, again.buckets);
    }

    #[test]
    fn accounts_sharing_a_profile_share_a_source() {
        let account = |id: &str, provider: &str, path: &str| Account {
            id: id.into(),
            name: id.into(),
            provider: provider.into(),
            enabled: true,
            profile_path: path.into(),
            endpoint: String::new(),
            secret_env: String::new(),
            has_secret: false,
        };
        let list = sources(&[
            account("a", "claude", r"C:\Users\x\.claude"),
            account("b", "claude", r"c:\users\x\.CLAUDE"),
            account("c", "codex", r"C:\Users\x\.codex"),
            account("d", "gemini", ""),
        ]);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].accounts, vec!["a", "b"]);
        assert_eq!(list[1].cli, Cli::Codex);
        assert!(list[1].dir.ends_with("sessions"));
    }
}
