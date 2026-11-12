//! Claude Code hooks installer (data-spec §9.1).
//!
//! `settings.json` belongs to the user and usually carries other tools' hooks on the same
//! events. Rules:
//! * only hook commands containing one of [`HOOK_MARKERS`] are ours; everything else (members,
//!   order, number lexemes, other matcher groups) is left exactly as it was;
//! * install is idempotent (nothing is written when the file already has our entries);
//! * removal deletes our commands, a matcher group only when it held nothing but ours, an event
//!   array only when our removal emptied it, and `hooks` only when our removal emptied it;
//! * the file is written atomically; the first time we change an existing file a copy is kept as
//!   `settings.json.codenotch.bak` (never overwritten afterwards);
//! * a file that is not valid JSON, or whose `hooks` has an unexpected shape, is never written.

use crate::json::{self, Json, Layout};
use codenotch_core::hooks::{HookEvent, HOOK_MARKERS};
use codenotch_core::paths::write_atomic_checked;
use codenotch_core::{Env, Error, Result};
use std::path::{Path, PathBuf};

/// Claude Code's per-profile settings file name.
pub const SETTINGS_FILE: &str = "settings.json";
/// Backup kept the first time we change an existing settings file.
pub const BACKUP_SUFFIX: &str = ".codenotch.bak";
/// `timeout` (seconds) written on our hook entries.
pub const HOOK_TIMEOUT_SECS: u32 = 5;

/// Claude Code profile folders on this machine: `home\.claude`, every `home\.claude-*` folder,
/// and `CLAUDE_CONFIG_DIR` when set. Only existing folders are returned.
pub fn claude_profile_dirs(env: &Env) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let default = env.home.join(".claude");
    if default.is_dir() {
        out.push(default);
    }
    if let Ok(rd) = std::fs::read_dir(&env.home) {
        let mut extra: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with(".claude-")
            })
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        extra.sort();
        out.extend(extra);
    }
    if let Some(dir) = env.var("CLAUDE_CONFIG_DIR") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() && !out.iter().any(|p| crate::same_path(p, &dir)) {
            out.push(dir);
        }
    }
    out
}

/// The command line written for one event: `"<exe>" <event>`.
pub fn hook_command(hook_exe: &Path, event: HookEvent) -> String {
    format!("\"{}\" {}", hook_exe.display(), event.as_arg())
}

fn is_ours(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    HOOK_MARKERS.iter().any(|m| lower.contains(m))
}

fn hook_is_ours(hook: &Json) -> bool {
    hook.get("command")
        .and_then(Json::as_str)
        .is_some_and(is_ours)
}

/// Our matcher group for one Claude event.
fn our_group(hook_exe: &Path, event: HookEvent, matcher: bool) -> Json {
    let hook = Json::Obj(vec![
        ("type".into(), Json::Str("command".into())),
        ("command".into(), Json::Str(hook_command(hook_exe, event))),
        ("timeout".into(), Json::Num(HOOK_TIMEOUT_SECS.to_string())),
    ]);
    let mut members = Vec::new();
    if matcher {
        members.push(("matcher".into(), Json::Str("*".into())));
    }
    members.push(("hooks".into(), Json::Arr(vec![hook])));
    Json::Obj(members)
}

/// Every (Claude event name, needs matcher, internal event) we install.
fn wanted() -> Vec<(&'static str, bool, HookEvent)> {
    HookEvent::ALL
        .into_iter()
        .flat_map(|e| {
            e.claude_events()
                .iter()
                .map(move |(name, m)| (*name, *m, e))
        })
        .collect()
}

/// A matcher group whose `hooks` holds only our commands (at least one).
fn group_is_purely_ours(group: &Json) -> bool {
    match group.get("hooks") {
        Some(Json::Arr(hooks)) => !hooks.is_empty() && hooks.iter().all(hook_is_ours),
        _ => false,
    }
}

/// Remove our commands from one event array. Returns how many were removed.
fn strip_ours(groups: &mut Vec<Json>) -> usize {
    let mut removed = 0;
    groups.retain_mut(|group| {
        let Some(Json::Arr(hooks)) = group.get_mut("hooks") else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|h| !hook_is_ours(h));
        let n = before - hooks.len();
        removed += n;
        !(n > 0 && hooks.is_empty())
    });
    removed
}

fn shape_error(file: &Path, what: &str) -> Error {
    Error::other(format!("{}: {what}; file not modified", file.display()))
}

/// Pure merge: add/refresh our entries in a parsed settings document.
pub(crate) fn merge_install(doc: &mut Json, hook_exe: &Path, file: &Path) -> Result<()> {
    let Json::Obj(root) = doc else {
        return Err(shape_error(file, "top level is not an object"));
    };
    if !root.iter().any(|(k, _)| k == "hooks") {
        root.push(("hooks".into(), Json::Obj(Vec::new())));
    }
    let Some(Json::Obj(hooks)) = root.iter_mut().find(|(k, _)| k == "hooks").map(|(_, v)| v) else {
        return Err(shape_error(file, "\"hooks\" is not an object"));
    };
    // Validate every event we touch before changing anything.
    for (name, _, _) in wanted() {
        if let Some((_, v)) = hooks.iter().find(|(k, _)| k == name) {
            if !matches!(v, Json::Arr(_)) {
                return Err(shape_error(file, "a hook event is not an array"));
            }
        }
    }
    for (name, matcher, event) in wanted() {
        if !hooks.iter().any(|(k, _)| k == name) {
            hooks.push((name.to_string(), Json::Arr(Vec::new())));
        }
        let Some(Json::Arr(groups)) = hooks.iter_mut().find(|(k, _)| k == name).map(|(_, v)| v)
        else {
            continue;
        };
        let desired = our_group(hook_exe, event, matcher);
        match groups.iter().position(group_is_purely_ours) {
            Some(pos) => {
                // Refresh ours in place; strip any other copy (duplicates, legacy entries mixed
                // into foreign groups) around it.
                let mut after = groups.split_off(pos + 1);
                groups.truncate(pos);
                strip_ours(groups);
                strip_ours(&mut after);
                groups.push(desired);
                groups.extend(after);
            }
            None => {
                strip_ours(groups);
                groups.push(desired);
            }
        }
    }
    Ok(())
}

/// Pure removal of our entries. Returns how many commands were removed.
pub(crate) fn merge_remove(doc: &mut Json, file: &Path) -> Result<usize> {
    let Json::Obj(root) = doc else {
        return Err(shape_error(file, "top level is not an object"));
    };
    let Some(pos) = root.iter().position(|(k, _)| k == "hooks") else {
        return Ok(0);
    };
    let Json::Obj(hooks) = &mut root[pos].1 else {
        return Ok(0);
    };
    let mut total = 0;
    hooks.retain_mut(|(_, v)| {
        let Json::Arr(groups) = v else { return true };
        let n = strip_ours(groups);
        total += n;
        !(n > 0 && groups.is_empty())
    });
    if total > 0 && hooks.is_empty() {
        root.remove(pos);
    }
    Ok(total)
}

/// Whether a parsed settings document has our command on every event we install.
pub(crate) fn doc_has_hooks(doc: &Json) -> bool {
    let Some(hooks) = doc.get("hooks") else {
        return false;
    };
    wanted()
        .iter()
        .all(|(name, _, event)| match hooks.get(name) {
            Some(Json::Arr(groups)) => groups.iter().any(|g| match g.get("hooks") {
                Some(Json::Arr(hs)) => hs.iter().any(|h| {
                    h.get("command").and_then(Json::as_str).is_some_and(|c| {
                        is_ours(c) && c.trim_end().ends_with(&format!(" {}", event.as_arg()))
                    })
                }),
                _ => false,
            }),
            _ => false,
        })
}

/// Read and parse a settings file. `Ok(None)` = file absent. Whitespace-only = `{}`.
fn load(file: &Path) -> Result<Option<(Json, Layout, Vec<u8>)>> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(format!("read {}", file.display()), e)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        Error::Json(format!(
            "{} is not UTF-8; file not modified",
            file.display()
        ))
    })?;
    if text.trim_start_matches('\u{feff}').trim().is_empty() {
        return Ok(Some((Json::Obj(Vec::new()), Layout::default(), bytes)));
    }
    let (doc, layout) = json::parse(text).map_err(|e| {
        Error::Json(format!(
            "{} is not valid JSON ({e}); file not modified",
            file.display()
        ))
    })?;
    Ok(Some((doc, layout, bytes)))
}

fn backup_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(BACKUP_SUFFIX);
    PathBuf::from(s)
}

/// Where to write `file`: the file itself, or — when it is a symbolic link or another reparse
/// point (dotfiles setups) — the resolved target, so the link survives. A link whose target
/// cannot be resolved is refused (never replaced by a regular file).
fn decide_target(
    file: &Path,
    is_link: bool,
    resolved: std::io::Result<PathBuf>,
) -> Result<PathBuf> {
    if !is_link {
        return Ok(file.to_path_buf());
    }
    resolved.map_err(|_| {
        Error::other(format!(
            "{} is a link whose target cannot be resolved; file not modified",
            file.display()
        ))
    })
}

/// Whether `file` is a symlink or a reparse point (`false` when it does not exist).
fn is_link(file: &Path) -> bool {
    match std::fs::symlink_metadata(file) {
        Ok(meta) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                meta.file_type().is_symlink()
                    || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            }
            #[cfg(not(windows))]
            {
                meta.file_type().is_symlink()
            }
        }
        Err(_) => false,
    }
}

fn write_target(file: &Path) -> Result<PathBuf> {
    let link = is_link(file);
    decide_target(
        file,
        link,
        if link {
            std::fs::canonicalize(file)
        } else {
            Ok(file.to_path_buf())
        },
    )
}

/// Write `doc` over `file` (through a link to its target). `original` = the bytes read before
/// the merge (`None`: the file did not exist); if the file no longer matches them just before
/// the rename (another program — or another CodeNotch — changed it), nothing is written.
fn write(file: &Path, doc: &Json, layout: &Layout, original: Option<&[u8]>) -> Result<()> {
    let target = write_target(file)?;
    if let Some(orig) = original {
        let bak = backup_path(file);
        if !bak.exists() {
            std::fs::write(&bak, orig)
                .map_err(|e| Error::io(format!("write {}", bak.display()), e))?;
        }
    }
    let unchanged = || -> std::io::Result<()> {
        let now = match std::fs::read(&target) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if now.as_deref() == original {
            Ok(())
        } else {
            Err(std::io::Error::other(CHANGED_UNDERNEATH))
        }
    };
    write_atomic_checked(&target, json::to_string(doc, layout).as_bytes(), unchanged).map_err(|e| {
        if e.to_string() == CHANGED_UNDERNEATH {
            Error::other(format!(
                "{} was changed by another program while CodeNotch was updating it; file not modified, try again",
                file.display()
            ))
        } else {
            Error::io(format!("write {}", file.display()), e)
        }
    })
}

const CHANGED_UNDERNEATH: &str = "settings file changed underneath";

/// Install into one profile folder's `settings.json` (created if missing).
pub fn install_in(profile_dir: &Path, hook_exe: &Path) -> Result<bool> {
    let file = profile_dir.join(SETTINGS_FILE);
    let (mut doc, layout, original) = match load(&file)? {
        Some((d, l, b)) => (d, l, Some(b)),
        None => (Json::Obj(Vec::new()), Layout::default(), None),
    };
    let before = doc.clone();
    merge_install(&mut doc, hook_exe, &file)?;
    if doc == before && original.is_some() {
        return Ok(false);
    }
    write(&file, &doc, &layout, original.as_deref())?;
    Ok(true)
}

/// Remove from one profile folder's `settings.json`. A missing file is fine.
pub fn remove_in(profile_dir: &Path) -> Result<bool> {
    let file = profile_dir.join(SETTINGS_FILE);
    let Some((mut doc, layout, original)) = load(&file)? else {
        return Ok(false);
    };
    if merge_remove(&mut doc, &file)? == 0 {
        return Ok(false);
    }
    write(&file, &doc, &layout, Some(&original))?;
    Ok(true)
}

/// Whether one profile folder's settings carry our hooks on every event.
pub fn installed_in(profile_dir: &Path) -> bool {
    matches!(load(&profile_dir.join(SETTINGS_FILE)), Ok(Some((doc, _, _))) if doc_has_hooks(&doc))
}

pub(crate) fn check_exe(hook_exe: &Path) -> Result<()> {
    if !hook_exe.is_absolute() {
        return Err(Error::other("hook helper path must be absolute"));
    }
    if hook_exe.to_string_lossy().contains('"') {
        return Err(Error::other("hook helper path contains a quote"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Program Files\CodeNotch\codenotch-hook.exe";

    /// A settings file full of another tool's hooks on every event (and then some).
    const FOREIGN: &str = r#"{
  "model": "opus",
  "permissions": { "allow": ["Bash(git status)"], "deny": [] },
  "statusLine": { "type": "command", "command": "node C:\\tools\\status.js", "padding": 0 },
  "hooks": {
    "SessionStart": [ { "hooks": [ { "type": "command", "command": "other-tool session-start", "timeout": 10 } ] } ],
    "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "other-tool prompt" } ] } ],
    "PreToolUse": [
      { "matcher": "Bash", "hooks": [ { "type": "command", "command": "other-tool guard --strict", "timeout": 2.50 } ] },
      { "matcher": "*", "hooks": [ { "type": "command", "command": "other-tool pre" } ] }
    ],
    "PostToolUse": [ { "matcher": "Edit|Write", "hooks": [ { "type": "command", "command": "other-tool fmt" } ] } ],
    "Notification": [ { "hooks": [ { "type": "command", "command": "other-tool notify" } ] } ],
    "Stop": [ { "hooks": [ { "type": "command", "command": "other-tool stop" }, { "type": "command", "command": "other-tool log" } ] } ],
    "SessionEnd": [ { "hooks": [ { "type": "command", "command": "other-tool end" } ] } ],
    "SubagentStop": [ { "hooks": [ { "type": "command", "command": "other-tool sub" } ] } ],
    "PreCompact": [ { "matcher": "auto", "hooks": [ { "type": "command", "command": "other-tool compact" } ] } ]
  },
  "env": { "BIG": 12345678901234567890, "F": 1.0e-3 },
  "zzz": [null, true, false, "\u00e9"]
}
"#;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn settings(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap()
    }

    fn serde(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    /// The document with every command containing a marker removed (and groups/arrays/hooks that
    /// only existed for ours), compared as values.
    fn foreign_view(text: &str) -> serde_json::Value {
        let mut v = serde(text);
        if let Some(hooks) = v.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            for groups in hooks.values_mut() {
                if let Some(arr) = groups.as_array_mut() {
                    arr.retain(|g| {
                        !g["hooks"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(|h| is_ours(h["command"].as_str().unwrap_or("")))
                    });
                }
            }
            hooks.retain(|_, g| !g.as_array().unwrap().is_empty());
        }
        v
    }

    #[test]
    fn foreign_hooks_survive_install_install_remove() {
        let d = tmp();
        std::fs::write(d.path().join(SETTINGS_FILE), FOREIGN).unwrap();
        let exe = Path::new(EXE);

        assert!(install_in(d.path(), exe).unwrap());
        let once = settings(d.path());
        assert!(installed_in(d.path()));
        assert_eq!(
            foreign_view(&once),
            serde(FOREIGN),
            "foreign content changed by install"
        );
        // Our entries are appended after the foreign groups, with matcher on tool events.
        let v = serde(&once);
        let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 3);
        assert_eq!(pre[2]["matcher"], "*");
        assert_eq!(pre[2]["hooks"][0]["command"], format!("\"{EXE}\" running"));
        assert_eq!(pre[2]["hooks"][0]["timeout"], 5);
        assert!(v["hooks"]["Stop"][1].get("matcher").is_none());
        assert_eq!(
            v["hooks"]["Notification"][1]["hooks"][0]["command"],
            format!("\"{EXE}\" attention")
        );
        // Number lexemes and key order are preserved.
        assert!(
            once.contains("12345678901234567890")
                && once.contains("1.0e-3")
                && once.contains("2.50")
        );
        assert!(once.find("\"model\"").unwrap() < once.find("\"zzz\"").unwrap());
        // Backup holds the original bytes.
        assert_eq!(
            std::fs::read_to_string(backup_path(&d.path().join(SETTINGS_FILE))).unwrap(),
            FOREIGN
        );

        // Second install: no write at all.
        assert!(!install_in(d.path(), exe).unwrap());
        assert_eq!(settings(d.path()), once);

        // Remove: back to the original values exactly.
        assert!(remove_in(d.path()).unwrap());
        let after = settings(d.path());
        assert_eq!(serde(&after), serde(FOREIGN));
        assert!(!installed_in(d.path()));
        assert!(!remove_in(d.path()).unwrap(), "second remove is a no-op");
        assert_eq!(
            std::fs::read_to_string(backup_path(&d.path().join(SETTINGS_FILE))).unwrap(),
            FOREIGN
        );
    }

    #[test]
    fn install_on_missing_file_and_remove_cleans_containers() {
        let d = tmp();
        assert!(install_in(d.path(), Path::new(EXE)).unwrap());
        assert!(installed_in(d.path()));
        assert!(!backup_path(&d.path().join(SETTINGS_FILE)).exists());
        let v = serde(&settings(d.path()));
        assert_eq!(v["hooks"].as_object().unwrap().len(), 7);
        assert!(remove_in(d.path()).unwrap());
        assert_eq!(serde(&settings(d.path())), serde("{}"));
    }

    #[test]
    fn reinstall_with_new_path_replaces_in_place_and_cleans_legacy() {
        let d = tmp();
        let text = r#"{"hooks":{"Stop":[
            {"hooks":[{"type":"command","command":"\"C:\\old\\eatbean-hook.exe\" done"}]},
            {"hooks":[{"type":"command","command":"after"}]},
            {"hooks":[{"type":"command","command":"keep"},{"type":"command","command":"C:\\x\\pacman-hook.exe done"}]}
        ]}}"#;
        std::fs::write(d.path().join(SETTINGS_FILE), text).unwrap();
        install_in(d.path(), Path::new(EXE)).unwrap();
        let v = serde(&settings(d.path()));
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 3);
        assert_eq!(
            stop[0]["hooks"][0]["command"],
            format!("\"{EXE}\" done"),
            "replaced in place"
        );
        assert_eq!(stop[1]["hooks"][0]["command"], "after");
        assert_eq!(
            stop[2]["hooks"].as_array().unwrap().len(),
            1,
            "legacy hook stripped from mixed group"
        );
        assert_eq!(stop[2]["hooks"][0]["command"], "keep");
        // A new exe path converges in one more install, then is stable.
        install_in(d.path(), Path::new(r"D:\new\codenotch-hook.exe")).unwrap();
        assert!(!install_in(d.path(), Path::new(r"D:\new\codenotch-hook.exe")).unwrap());
        assert!(settings(d.path()).contains(r"D:\\new\\codenotch-hook.exe"));
    }

    #[test]
    fn invalid_or_odd_files_are_never_written() {
        for bad in [
            "{\"hooks\": {",
            "not json",
            "[1,2]",
            r#"{"hooks": []}"#,
            r#"{"hooks": {"Stop": {}}}"#,
        ] {
            let d = tmp();
            std::fs::write(d.path().join(SETTINGS_FILE), bad).unwrap();
            assert!(install_in(d.path(), Path::new(EXE)).is_err(), "{bad}");
            assert_eq!(settings(d.path()), bad, "file must be untouched");
            assert!(!backup_path(&d.path().join(SETTINGS_FILE)).exists());
            if bad == "{\"hooks\": {" || bad == "not json" {
                assert!(remove_in(d.path()).is_err());
            }
            assert_eq!(settings(d.path()), bad);
        }
    }

    #[test]
    fn crlf_bom_indent_are_kept() {
        let d = tmp();
        let text = "\u{feff}{\r\n    \"a\": 1\r\n}\r\n";
        std::fs::write(d.path().join(SETTINGS_FILE), text).unwrap();
        install_in(d.path(), Path::new(EXE)).unwrap();
        remove_in(d.path()).unwrap();
        assert_eq!(settings(d.path()), text);
    }

    #[test]
    fn a_file_changed_underneath_is_not_overwritten() {
        let d = tmp();
        let file = d.path().join(SETTINGS_FILE);
        std::fs::write(&file, r#"{"a": 1}"#).unwrap();
        let (mut doc, layout, original) = load(&file).unwrap().unwrap();
        merge_install(&mut doc, Path::new(EXE), &file).unwrap();
        // Someone else writes between our read and our rename.
        std::fs::write(&file, r#"{"a": 2}"#).unwrap();
        let err = write(&file, &doc, &layout, Some(&original))
            .unwrap_err()
            .to_string();
        assert!(err.contains("changed by another program"), "{err}");
        assert_eq!(settings(d.path()), r#"{"a": 2}"#);
        // Absent when read, created meanwhile: also refused.
        let d2 = tmp();
        let file2 = d2.path().join(SETTINGS_FILE);
        std::fs::write(&file2, "{}").unwrap();
        assert!(write(&file2, &doc, &layout, None).is_err());
        assert_eq!(settings(d2.path()), "{}");
        // No temp file left behind in either folder.
        for dir in [d.path(), d2.path()] {
            assert!(std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp")));
        }
        // Unchanged: the write goes through.
        let (mut doc, layout, original) = load(&file).unwrap().unwrap();
        merge_install(&mut doc, Path::new(EXE), &file).unwrap();
        write(&file, &doc, &layout, Some(&original)).unwrap();
        assert!(installed_in(d.path()));
    }

    #[test]
    fn link_target_decision() {
        let f = Path::new(r"C:\p\settings.json");
        let t = PathBuf::from(r"D:\dotfiles\claude\settings.json");
        assert_eq!(decide_target(f, false, Ok(f.to_path_buf())).unwrap(), f);
        assert_eq!(decide_target(f, true, Ok(t.clone())).unwrap(), t);
        let err = decide_target(
            f,
            true,
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot be resolved"), "{err}");
        // A plain file and a missing file are written in place.
        let d = tmp();
        let missing = d.path().join(SETTINGS_FILE);
        assert_eq!(write_target(&missing).unwrap(), missing);
        std::fs::write(&missing, "{}").unwrap();
        assert_eq!(write_target(&missing).unwrap(), missing);
    }

    #[cfg(windows)]
    #[test]
    fn symlinked_settings_keep_the_link() {
        let d = tmp();
        let real_dir = d.path().join("dotfiles");
        let profile = d.path().join("profile");
        std::fs::create_dir_all(&real_dir).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        let real = real_dir.join(SETTINGS_FILE);
        std::fs::write(&real, r#"{"model": "opus"}"#).unwrap();
        let link = profile.join(SETTINGS_FILE);
        if let Err(err) = std::os::windows::fs::symlink_file(&real, &link) {
            eprintln!("skipped: cannot create a symlink here ({err})");
            return;
        }
        assert!(install_in(&profile, Path::new(EXE)).unwrap());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "link kept"
        );
        assert!(installed_in(&real_dir), "target updated");
        assert!(remove_in(&profile).unwrap());
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            serde(&std::fs::read_to_string(&real).unwrap()),
            serde(r#"{"model": "opus"}"#)
        );
        // Dangling link: refused, link untouched.
        std::fs::remove_file(&real).unwrap();
        let err = install_in(&profile, Path::new(EXE))
            .unwrap_err()
            .to_string();
        assert!(err.contains("cannot be resolved"), "{err}");
        assert!(!real.exists());
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "dangling link never replaced"
        );
    }

    #[test]
    fn profile_discovery_and_exe_checks() {
        let d = tmp();
        let env = Env::for_test(d.path());
        assert!(claude_profile_dirs(&env).is_empty());
        std::fs::create_dir_all(env.home.join(".claude")).unwrap();
        std::fs::create_dir_all(env.home.join(".claude-work")).unwrap();
        std::fs::create_dir_all(env.home.join(".claudeish")).unwrap();
        let dirs = claude_profile_dirs(&env);
        assert_eq!(
            dirs,
            vec![env.home.join(".claude"), env.home.join(".claude-work")]
        );
        assert!(check_exe(Path::new("relative.exe")).is_err());
        assert!(check_exe(Path::new(EXE)).is_ok());
    }
}
