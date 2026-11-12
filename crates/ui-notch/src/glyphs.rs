//! Provider marks (visual-spec §4).
//!
//! The SVGs under `assets/glyphs/` are lobe-icons (MIT, © LobeHub), copied unmodified with their
//! `NOTICE.md`. They are embedded and served to gpui through [`Assets`], which the app passes to
//! `Application::new().with_assets(...)`. Lookup order: user override
//! `<data dir>\glyphs\<id>.svg|png` → built-in SVG → fallback letters.

use codenotch_core::model::ProviderKind;
use gpui::{AssetSource, SharedString};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Asset path prefix served by [`Assets`].
pub const PREFIX: &str = "codenotch-glyphs/";

/// Licence notice of the embedded marks.
pub static NOTICE: &str = include_str!("../assets/glyphs/NOTICE.md");

static BUILTIN: &[(&str, &[u8])] = &[
    ("claude", include_bytes!("../assets/glyphs/claude.svg")),
    ("codex", include_bytes!("../assets/glyphs/codex.svg")),
    (
        "codex-alt",
        include_bytes!("../assets/glyphs/codex-alt.svg"),
    ),
    (
        "commandcode",
        include_bytes!("../assets/glyphs/commandcode.svg"),
    ),
    ("copilot", include_bytes!("../assets/glyphs/copilot.svg")),
    ("cursor", include_bytes!("../assets/glyphs/cursor.svg")),
    ("deepseek", include_bytes!("../assets/glyphs/deepseek.svg")),
    ("gemini", include_bytes!("../assets/glyphs/gemini.svg")),
    (
        "gemini-alt",
        include_bytes!("../assets/glyphs/gemini-alt.svg"),
    ),
    ("grok", include_bytes!("../assets/glyphs/grok.svg")),
    ("opencode", include_bytes!("../assets/glyphs/opencode.svg")),
    ("qoder", include_bytes!("../assets/glyphs/qoder.svg")),
];

/// Set once an [`Assets`] exists, i.e. once the app registered it with gpui. Without it the
/// notch draws letters instead of asking gpui for assets it cannot load.
static REGISTERED: AtomicBool = AtomicBool::new(false);

pub fn builtin(name: &str) -> Option<&'static [u8]> {
    BUILTIN.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// File names to look for, in order: the provider's own id, then the reference app's file name
/// (Antigravity's mark ships as `gemini.svg`).
pub fn names(kind: &ProviderKind) -> Vec<&str> {
    let mut v = vec![kind.glyph_id()];
    let alias = match kind {
        ProviderKind::Antigravity => Some("gemini"),
        _ => None,
    };
    if let Some(a) = alias {
        v.push(a);
    }
    v
}

/// How to draw a provider mark.
#[derive(Debug, Clone, PartialEq)]
pub enum Glyph {
    /// gpui asset path (served by [`Assets`]; user SVG overrides included).
    Svg(SharedString),
    /// User PNG override on disk.
    Png(PathBuf),
    Letters(&'static str),
}

/// Resolve the mark for `kind`. `user_dir` is the glyph override folder; `exists` is injected
/// for tests. Without registered assets only PNG overrides and letters are possible.
pub fn resolve(
    kind: &ProviderKind,
    user_dir: Option<&Path>,
    assets: bool,
    exists: impl Fn(&Path) -> bool,
) -> Glyph {
    let names = names(kind);
    if let Some(dir) = user_dir {
        for n in &names {
            if assets && exists(&dir.join(format!("{n}.svg"))) {
                return Glyph::Svg(format!("{PREFIX}{n}.svg").into());
            }
            let png = dir.join(format!("{n}.png"));
            if exists(&png) {
                return Glyph::Png(png);
            }
        }
    }
    if assets {
        if let Some(n) = names.iter().find(|n| builtin(n).is_some()) {
            return Glyph::Svg(format!("{PREFIX}{n}.svg").into());
        }
    }
    Glyph::Letters(kind.glyph_letters())
}

/// [`resolve`] against the real file system and registration state.
pub fn resolve_now(kind: &ProviderKind, user_dir: Option<&Path>) -> Glyph {
    resolve(kind, user_dir, REGISTERED.load(Ordering::Relaxed), |p| {
        p.is_file()
    })
}

/// Glyph override folder (`<data dir>\glyphs`). Only read, never created.
pub fn user_dir() -> PathBuf {
    codenotch_core::paths::Paths::resolve().glyphs_dir()
}

/// gpui asset source for the notch: serves `codenotch-glyphs/<name>.svg` (user override first,
/// then the embedded mark) and forwards every other path to an optional fallback source.
///
/// ```ignore
/// Application::new().with_assets(codenotch_ui_notch::Assets::new()).run(...)
/// // or, chaining another crate's assets:
/// Application::new().with_assets(codenotch_ui_notch::Assets::new().with_fallback(other))
/// ```
pub struct Assets {
    user_dir: Option<PathBuf>,
    fallback: Option<Box<dyn AssetSource>>,
}

impl Assets {
    pub fn new() -> Self {
        REGISTERED.store(true, Ordering::Relaxed);
        Self {
            user_dir: Some(user_dir()),
            fallback: None,
        }
    }

    /// Serve user overrides from `dir` instead of the data folder (tests).
    pub fn with_user_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.user_dir = dir;
        self
    }

    /// Forward non-glyph paths to `source`.
    pub fn with_fallback(mut self, source: impl AssetSource) -> Self {
        self.fallback = Some(Box::new(source));
        self
    }
}

impl Default for Assets {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = path.strip_prefix(PREFIX) {
            let name = file.strip_suffix(".svg").unwrap_or(file);
            if name.contains(['/', '\\', ':']) || name.contains("..") {
                return Ok(None);
            }
            if let Some(dir) = &self.user_dir {
                let p = dir.join(format!("{name}.svg"));
                if p.is_file() {
                    match std::fs::read(&p) {
                        Ok(bytes) => return Ok(Some(Cow::Owned(bytes))),
                        Err(err) => log::warn!("glyph override {name}.svg unreadable: {err}"),
                    }
                }
            }
            return Ok(builtin(name).map(Cow::Borrowed));
        }
        match &self.fallback {
            Some(f) => f.load(path),
            None => Ok(None),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if path.starts_with(PREFIX) || PREFIX.starts_with(path) {
            return Ok(BUILTIN
                .iter()
                .map(|(n, _)| SharedString::from(format!("{PREFIX}{n}.svg")))
                .collect());
        }
        match &self.fallback {
            Some(f) => f.list(path),
            None => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_is_an_svg_with_a_notice() {
        for (name, bytes) in BUILTIN {
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.trim_start().starts_with("<svg"), "{name}");
        }
        assert!(NOTICE.contains("MIT"));
    }

    #[test]
    fn lookup_order() {
        let dir = Path::new("X:/glyphs");
        let none = |_: &Path| false;
        assert_eq!(
            resolve(&ProviderKind::Claude, Some(dir), true, none),
            Glyph::Svg("codenotch-glyphs/claude.svg".into())
        );
        assert_eq!(
            resolve(&ProviderKind::Antigravity, None, true, none),
            Glyph::Svg("codenotch-glyphs/gemini.svg".into())
        );
        assert_eq!(
            resolve(&ProviderKind::Glm, None, true, none),
            Glyph::Letters("Z.ai")
        );
        assert_eq!(
            resolve(&ProviderKind::Custom, None, true, none),
            Glyph::Letters("•")
        );
        // no registered assets: letters
        assert_eq!(
            resolve(&ProviderKind::Claude, None, false, none),
            Glyph::Letters("C")
        );
        // user overrides win
        let svg = |p: &Path| p.ends_with("glm.svg");
        assert_eq!(
            resolve(&ProviderKind::Glm, Some(dir), true, svg),
            Glyph::Svg("codenotch-glyphs/glm.svg".into())
        );
        let png = |p: &Path| p.ends_with("claude.png");
        assert_eq!(
            resolve(&ProviderKind::Claude, Some(dir), true, png),
            Glyph::Png(dir.join("claude.png"))
        );
    }

    #[test]
    fn asset_source_serves_overrides_then_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("claude.svg"), b"<svg>user</svg>").unwrap();
        let a = Assets::new().with_user_dir(Some(tmp.path().to_path_buf()));
        assert_eq!(
            a.load("codenotch-glyphs/claude.svg")
                .unwrap()
                .unwrap()
                .as_ref(),
            b"<svg>user</svg>"
        );
        assert!(a.load("codenotch-glyphs/grok.svg").unwrap().is_some());
        assert!(a.load("codenotch-glyphs/nope.svg").unwrap().is_none());
        assert!(a.load("codenotch-glyphs/../x.svg").unwrap().is_none());
        assert!(a.load("icons/other.svg").unwrap().is_none());
        assert_eq!(a.list(PREFIX).unwrap().len(), BUILTIN.len());
    }
}
