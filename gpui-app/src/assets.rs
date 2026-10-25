//! Images bundled into the binary: the app icon and the provider logos.
use gpui::{AssetSource, Image, ImageFormat, SharedString};
use std::{borrow::Cow, collections::HashMap, sync::Arc, sync::OnceLock};

/// The provider logos the Tauri app draws, as unit-square SVG paths.
const GLYPHS: &str = include_str!("../../src/lib/glyphs.json");

fn glyphs() -> &'static HashMap<String, String> {
    static GLYPHS_MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    GLYPHS_MAP.get_or_init(|| serde_json::from_str(GLYPHS).unwrap_or_default())
}

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// Lucide icons the Tauri app uses, as `icons/<name>.svg`.
        fn icon(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../assets/icons/", $name, ".svg"))),)*
                _ => None,
            }
        }
    };
}

icons!(
    "settings-2", "refresh-cw", "x", "minus", "users-round", "info", "plus", "grip-vertical",
    "circle-alert", "check", "chevrons-right", "bell", "download", "loader-circle", "send",
    "timer", "languages", "webhook", "log-in", "search", "chevron-right", "chevron-down",
    "chevron-up", "shield-check", "trash-2", "monitor", "external-link", "sliders-horizontal",
    "terminal", "copy",
);

/// Path of a bundled Lucide icon, for `svg().path(..)`.
pub fn icon_path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

/// Serves `logos/<key>.svg` and `icons/<name>.svg`; `svg()` tints them with the text color,
/// like `currentColor`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
        {
            return Ok(icon(name).map(Cow::Borrowed));
        }
        let Some(key) = path
            .strip_prefix("logos/")
            .and_then(|p| p.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        Ok(glyphs().get(key).map(|d| {
            Cow::Owned(
                format!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 1 1"><path d="{d}" fill="#fff" fill-rule="evenodd"/></svg>"##
                )
                .into_bytes(),
            )
        }))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(if path.starts_with("logos") {
            glyphs()
                .keys()
                .map(|k| format!("logos/{k}.svg").into())
                .collect()
        } else {
            vec![]
        })
    }
}

/// Logo path for a provider, `None` for custom JSON sources.
pub fn logo(provider: &str) -> Option<SharedString> {
    let key = if provider == "codex" { "openai" } else { provider };
    glyphs()
        .contains_key(key)
        .then(|| format!("logos/{key}.svg").into())
}

pub fn app_icon() -> Arc<Image> {
    static ICON: OnceLock<Arc<Image>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../../src-tauri/icons/128x128.png").to_vec(),
        ))
    })
    .clone()
}
