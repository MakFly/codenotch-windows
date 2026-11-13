//! Transcript discovery and the incremental indexer.

use std::collections::{BTreeMap, HashMap};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use codenotch_core::analytics::RootOwners;
use codenotch_core::model::ProviderKind;
use codenotch_core::{Config, Env};

use crate::cache::{Cache, FileEntry};
use crate::parse::{claude_line, codex_line, Cli, Outcome};

/// Lines longer than this are skipped without being buffered (tool outputs, images).
pub const MAX_LINE: usize = 32 * 1024 * 1024;
/// Bytes hashed to recognise a file replaced under the same path.
const HEAD_LEN: u64 = 256;
/// Directory depth searched under a root.
const MAX_DEPTH: usize = 8;

/// A folder scanned for transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: PathBuf,
    pub cli: Cli,
}

impl Root {
    /// The `source` string stored in buckets.
    pub fn source(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

fn dedup_key(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Profile folder of a CLI: the account's `profilePath`, else the CLI's env override
/// (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`), else `~\.claude` / `~\.codex`.
fn profile_dir(env: &Env, cli: Cli, profile_path: &str) -> PathBuf {
    if !profile_path.trim().is_empty() {
        return PathBuf::from(profile_path.trim());
    }
    let (var, dir) = match cli {
        Cli::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
        Cli::Codex => ("CODEX_HOME", ".codex"),
    };
    env.var(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home_path(dir))
}

/// Roots to scan: the default Claude and Codex profiles plus the profile folder of every
/// enabled Claude / Codex account (duplicates removed).
pub fn roots(config: &Config, env: &Env) -> Vec<Root> {
    roots_with_owners(config, env)
        .into_iter()
        .map(|(root, _)| root)
        .collect()
}

/// Every root of [`roots`] (same order, same `source` strings as the cache buckets) with the
/// ids of the enabled accounts whose profile folder it is. The default profile counts for an
/// account whose `profilePath` is empty or points at the same folder.
pub fn root_owners(config: &Config, env: &Env) -> Vec<RootOwners> {
    roots_with_owners(config, env)
        .into_iter()
        .map(|(root, accounts)| RootOwners {
            source: root.source(),
            provider: match root.cli {
                Cli::Claude => ProviderKind::Claude,
                Cli::Codex => ProviderKind::Codex,
            },
            accounts,
        })
        .collect()
}

fn roots_with_owners(config: &Config, env: &Env) -> Vec<(Root, Vec<String>)> {
    let mut profiles: Vec<(Cli, PathBuf, Option<&str>)> = vec![
        (Cli::Claude, profile_dir(env, Cli::Claude, ""), None),
        (Cli::Codex, profile_dir(env, Cli::Codex, ""), None),
    ];
    for account in config.enabled_accounts() {
        let cli = match account.provider {
            ProviderKind::Claude => Cli::Claude,
            ProviderKind::Codex => Cli::Codex,
            _ => continue,
        };
        profiles.push((
            cli,
            profile_dir(env, cli, &account.profile_path),
            Some(account.id.as_str()),
        ));
    }
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<(Root, Vec<String>)> = Vec::new();
    for (cli, dir, account) in profiles {
        let subs: &[&str] = match cli {
            Cli::Claude => &["projects"],
            Cli::Codex => &["sessions", "archived_sessions"],
        };
        for sub in subs {
            let path = dir.join(sub);
            let i = *index.entry(dedup_key(&path)).or_insert_with(|| {
                out.push((Root { path, cli }, Vec::new()));
                out.len() - 1
            });
            if let Some(id) = account {
                let owners = &mut out[i].1;
                if !owners.iter().any(|o| o == id) {
                    owners.push(id.to_string());
                }
            }
        }
    }
    out
}

fn mtime_secs(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_transcript(name: &str, cli: Cli) -> bool {
    let lower = name.to_ascii_lowercase();
    match cli {
        Cli::Claude => lower.ends_with(".jsonl"),
        Cli::Codex => lower.starts_with("rollout-") && lower.ends_with(".jsonl"),
    }
}

/// Transcript files under `root` modified at or after `since` (symlinks are not followed).
pub fn discover(root: &Root, since: i64) -> Vec<(PathBuf, Metadata)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.path.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in rd.flatten() {
            let Ok(ft) = item.file_type() else { continue };
            if ft.is_dir() {
                if depth < MAX_DEPTH {
                    stack.push((item.path(), depth + 1));
                }
            } else if ft.is_file() && is_transcript(&item.file_name().to_string_lossy(), root.cli) {
                if let Ok(meta) = item.metadata() {
                    if mtime_secs(&meta) >= since {
                        out.push((item.path(), meta));
                    }
                }
            }
        }
    }
    out
}

/// Counters of one indexing pass (diagnostics only; no paths).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IndexStats {
    /// Transcript files within the horizon.
    pub files: u32,
    /// Files that had new bytes to read.
    pub files_read: u32,
    /// Files parsed again from the start (shrunk or replaced).
    pub files_restarted: u32,
    pub bytes_read: u64,
    pub lines: u64,
    pub counted: u64,
    pub duplicates: u64,
    pub malformed: u64,
    /// Files that could not be opened/read this time (kept as they were).
    pub errors: u32,
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn head_sig(bytes: &[u8]) -> String {
    format!("{}:{:016x}", bytes.len(), fnv64(bytes))
}

fn read_head(file: &mut File, len: u64) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut buf = Vec::with_capacity(len as usize);
    file.by_ref().take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

enum LineRead {
    /// A complete line (without the newline) is in the buffer; `n` bytes consumed.
    Line(usize),
    /// A line longer than [`MAX_LINE`] was consumed and dropped.
    TooLong(usize),
    /// End of file before a newline: the rest is an incomplete line, left for next time.
    Eof,
}

fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<LineRead> {
    buf.clear();
    let mut consumed = 0usize;
    let mut too_long = false;
    loop {
        let avail = reader.fill_buf()?;
        if avail.is_empty() {
            return Ok(LineRead::Eof);
        }
        match memchr::memchr(b'\n', avail) {
            Some(i) => {
                if !too_long && buf.len() + i <= MAX_LINE {
                    buf.extend_from_slice(&avail[..i]);
                } else {
                    too_long = true;
                }
                reader.consume(i + 1);
                consumed += i + 1;
                return Ok(if too_long {
                    LineRead::TooLong(consumed)
                } else {
                    LineRead::Line(consumed)
                });
            }
            None => {
                let n = avail.len();
                if !too_long && buf.len() + n <= MAX_LINE {
                    buf.extend_from_slice(avail);
                } else {
                    too_long = true;
                    buf.clear();
                }
                reader.consume(n);
                consumed += n;
            }
        }
    }
}

/// Bring `entry` up to date with the file at `path`: parse only bytes after `entry.offset`,
/// restart when the file shrank or its first bytes changed.
pub fn index_file(
    path: &Path,
    meta: &Metadata,
    root: &Root,
    entry: &mut FileEntry,
    stats: &mut IndexStats,
) -> std::io::Result<()> {
    let size = meta.len();
    let mtime = mtime_secs(meta);
    let unchanged =
        size == entry.size && entry.offset <= size && entry.mtime.is_none_or(|m| m == mtime);
    if unchanged && (entry.offset > 0 || size == 0) {
        entry.mtime = Some(mtime);
        return Ok(());
    }
    let mut file = File::open(path)?;
    if size < entry.offset {
        entry.reset();
        stats.files_restarted += 1;
    } else if entry.offset > 0 {
        if let Some(sig) = entry.head.clone() {
            let len = sig
                .split(':')
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(HEAD_LEN);
            if head_sig(&read_head(&mut file, len)?) != sig {
                entry.reset();
                stats.files_restarted += 1;
            }
        }
    }
    entry.head = Some(head_sig(&read_head(&mut file, HEAD_LEN.min(size))?));

    file.seek(SeekFrom::Start(entry.offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    let source = root.source();
    let start = entry.offset;
    loop {
        match read_line(&mut reader, &mut buf)? {
            LineRead::Eof => break,
            LineRead::TooLong(n) => {
                entry.offset += n as u64;
                stats.lines += 1;
            }
            LineRead::Line(n) => {
                entry.offset += n as u64;
                stats.lines += 1;
                let line = buf.strip_suffix(b"\r").unwrap_or(&buf);
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let outcome = match root.cli {
                    Cli::Claude => claude_line(line, &source, entry),
                    Cli::Codex => codex_line(line, &source, entry),
                };
                match outcome {
                    Outcome::Counted => stats.counted += 1,
                    Outcome::Duplicate => stats.duplicates += 1,
                    Outcome::Malformed => stats.malformed += 1,
                    Outcome::Skipped | Outcome::Context => {}
                }
            }
        }
    }
    stats.files_read += 1;
    stats.bytes_read += entry.offset - start;
    entry.size = size.max(entry.offset);
    entry.mtime = Some(mtime);
    Ok(())
}

/// Update `cache` from every root: new files are added, grown files resumed, files that
/// disappeared or fell out of the horizon (`since`, unix secs) are dropped.
pub fn update(cache: &mut Cache, roots: &[Root], since: i64) -> IndexStats {
    let mut stats = IndexStats::default();
    let mut old = std::mem::take(&mut cache.files);
    let mut files = BTreeMap::new();
    for root in roots {
        for (path, meta) in discover(root, since) {
            let key = path.to_string_lossy().into_owned();
            if files.contains_key(&key) {
                continue;
            }
            stats.files += 1;
            let mut entry = old.remove(&key).unwrap_or_default();
            let before = entry.clone();
            if let Err(e) = index_file(&path, &meta, root, &mut entry, &mut stats) {
                // Keep what we had; try again next time.
                log::debug!("analytics: transcript unreadable ({})", e.kind());
                stats.errors += 1;
                entry = before;
            }
            files.insert(key, entry);
        }
    }
    cache.files = files;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(id: &str, hour: u32, out: u64) -> String {
        format!(
            "{{\"type\":\"assistant\",\"cwd\":\"C:\\\\p\",\"timestamp\":\"2026-11-12T{hour:02}:30:00Z\",\"message\":{{\"id\":\"{id}\",\"model\":\"claude-opus-5-5\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":{out}}}}}}}\n"
        )
    }

    fn total_output(e: &FileEntry) -> u64 {
        e.buckets.iter().map(|b| b.tokens.output).sum()
    }

    fn run(path: &Path, root: &Root, e: &mut FileEntry) -> IndexStats {
        let mut s = IndexStats::default();
        let meta = std::fs::metadata(path).expect("meta");
        index_file(path, &meta, root, e, &mut s).expect("index");
        s
    }

    #[test]
    fn resumes_tolerates_partial_lines_and_restarts_on_shrink_or_replace() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = Root {
            path: dir.path().to_path_buf(),
            cli: Cli::Claude,
        };
        let path = dir.path().join("s.jsonl");
        let mut f = File::create(&path).expect("create");
        let full = line("m1", 10, 5);
        let second = line("m2", 11, 7);
        f.write_all(full.as_bytes()).expect("w");
        f.write_all(&second.as_bytes()[..20]).expect("w"); // partial line
        drop(f);

        let mut e = FileEntry::default();
        let s = run(&path, &root, &mut e);
        assert_eq!((s.counted, e.offset), (1, full.len() as u64));
        assert_eq!(total_output(&e), 5);

        // Finish the line plus a garbage line and a duplicate.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        f.write_all(&second.as_bytes()[20..]).expect("w");
        f.write_all(b"{not json \"usage\"\n\n").expect("w");
        f.write_all(full.as_bytes()).expect("w");
        drop(f);
        let s = run(&path, &root, &mut e);
        assert_eq!((s.counted, s.malformed, s.duplicates), (1, 1, 1));
        assert_eq!(total_output(&e), 12);
        assert_eq!(e.offset, std::fs::metadata(&path).expect("m").len());

        // Second pass with no change reads nothing.
        let s = run(&path, &root, &mut e);
        assert_eq!(s.files_read, 0);

        // Shrink: start over.
        std::fs::write(&path, line("m9", 12, 3)).expect("w");
        let s = run(&path, &root, &mut e);
        assert_eq!(s.files_restarted, 1);
        assert_eq!(total_output(&e), 3);

        // Replaced by a larger, different file: head changes → start over.
        let other = format!("{}{}", line("x1", 9, 100), line("x2", 9, 200));
        std::fs::write(&path, &other).expect("w");
        let s = run(&path, &root, &mut e);
        assert_eq!(s.files_restarted, 1);
        assert_eq!(total_output(&e), 300);
    }

    #[test]
    fn crlf_and_oversized_lines() {
        let mut data = line("a", 10, 1).replace('\n', "\r\n").into_bytes();
        data.extend(std::iter::repeat_n(b'x', MAX_LINE + 10));
        data.push(b'\n');
        data.extend(line("b", 10, 2).into_bytes());
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, &data).expect("w");
        let root = Root {
            path: dir.path().to_path_buf(),
            cli: Cli::Claude,
        };
        let mut e = FileEntry::default();
        let s = run(&path, &root, &mut e);
        assert_eq!(s.counted, 2);
        assert_eq!(s.lines, 3);
        assert_eq!(e.offset, data.len() as u64);
    }

    #[test]
    fn roots_cover_defaults_and_accounts_once() {
        let dir = tempfile::tempdir().expect("tmp");
        let env = Env::for_test(dir.path());
        let mut config = Config::default();
        let mut a = codenotch_core::Account::new(ProviderKind::Claude, "work");
        a.profile_path = dir
            .path()
            .join("work-claude")
            .to_string_lossy()
            .into_owned();
        config.accounts.push(a);
        let mut b = codenotch_core::Account::new(ProviderKind::Claude, "default");
        b.profile_path = env
            .home_path(".claude")
            .to_string_lossy()
            .replace('\\', "/");
        config.accounts.push(b);
        let mut c = codenotch_core::Account::new(ProviderKind::Codex, "off");
        c.profile_path = dir.path().join("off").to_string_lossy().into_owned();
        c.enabled = false;
        config.accounts.push(c);
        let r = roots(&config, &env);
        let names: Vec<String> = r
            .iter()
            .map(|r| {
                r.path
                    .strip_prefix(dir.path())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            names,
            [
                "home\\.claude\\projects",
                "home\\.codex\\sessions",
                "home\\.codex\\archived_sessions",
                "work-claude\\projects"
            ]
        );
        let env = env.with_var("CODEX_HOME", "D:\\cx");
        assert!(roots(&Config::default(), &env)
            .iter()
            .any(|r| r.path == Path::new("D:\\cx\\sessions")));
    }

    #[test]
    fn update_discovers_filters_and_drops() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = Root {
            path: dir.path().join("sessions"),
            cli: Cli::Codex,
        };
        let day = root.path.join("2026").join("11").join("12");
        std::fs::create_dir_all(&day).expect("mkdir");
        let rollout = day.join("rollout-2026-11-12T10-00-00-abc.jsonl");
        std::fs::write(&rollout, "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-5\",\"cwd\":\"C:\\\\c\"}}\n{\"timestamp\":\"2026-11-12T10:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":10,\"cached_input_tokens\":4,\"output_tokens\":2}}}}\n").expect("w");
        std::fs::write(day.join("notes.jsonl"), "x\n").expect("w");
        let mut cache = Cache::empty();
        cache
            .files
            .insert("C:\\gone.jsonl".into(), FileEntry::default());
        let s = update(&mut cache, std::slice::from_ref(&root), 0);
        assert_eq!((s.files, s.counted), (1, 1));
        assert_eq!(cache.files.len(), 1);
        let e = cache.files.values().next().expect("entry");
        assert_eq!(e.buckets[0].source, root.source());
        assert_eq!(e.buckets[0].tokens.input, 6);
        // Horizon in the future: the file is skipped and dropped from the cache.
        let s = update(&mut cache, &[root], i64::MAX);
        assert_eq!(s.files, 0);
        assert!(cache.files.is_empty());
    }

    #[test]
    fn root_owners_name_the_enabled_accounts_of_each_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let env = Env::for_test(dir.path());
        // No account: the default roots are scanned but belong to nobody.
        let owners = root_owners(&Config::default(), &env);
        assert_eq!(owners.len(), 3);
        assert!(owners.iter().all(|o| o.accounts.is_empty()));

        let mut config = Config::default();
        let a = codenotch_core::Account::new(ProviderKind::Claude, "default");
        let mut b = codenotch_core::Account::new(ProviderKind::Claude, "same folder");
        b.profile_path = env.home_path(".claude").to_string_lossy().into_owned();
        let mut c = codenotch_core::Account::new(ProviderKind::Codex, "work");
        c.profile_path = dir.path().join("cx").to_string_lossy().into_owned();
        let mut d = codenotch_core::Account::new(ProviderKind::Codex, "off");
        d.enabled = false;
        let glm = codenotch_core::Account::new(ProviderKind::Glm, "glm");
        let ids = [a.id.clone(), b.id.clone(), c.id.clone()];
        config.accounts = vec![a, b, c, d, glm];
        let owners = root_owners(&config, &env);
        let sources: Vec<String> = roots(&config, &env).iter().map(Root::source).collect();
        assert_eq!(
            owners.iter().map(|o| o.source.clone()).collect::<Vec<_>>(),
            sources
        );
        let of = |suffix: &str| {
            owners
                .iter()
                .find(|o| o.source.ends_with(suffix))
                .expect("root")
        };
        // Shared default Claude profile: two owners.
        assert_eq!(
            of(".claude\\projects").accounts,
            [ids[0].clone(), ids[1].clone()]
        );
        assert_eq!(of(".claude\\projects").provider, ProviderKind::Claude);
        // Default Codex profile: the disabled account does not count.
        assert!(of(".codex\\sessions").accounts.is_empty());
        // Both Codex roots of the work profile belong to the work account.
        assert_eq!(of("cx\\sessions").accounts, [ids[2].clone()]);
        assert_eq!(of("cx\\archived_sessions").accounts, [ids[2].clone()]);
        assert_eq!(of("cx\\archived_sessions").provider, ProviderKind::Codex);
    }
}
