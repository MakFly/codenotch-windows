//! Images bundled into the binary: the app icon and the provider logos.
use gpui::{AssetSource, Image, ImageFormat, SharedString};
use std::{borrow::Cow, collections::HashMap, sync::Arc, sync::OnceLock};

/// The provider logos the Tauri app draws, as unit-square SVG paths.
const GLYPHS: &str = include_str!("../../src/lib/glyphs.json");

fn glyphs() -> &'static HashMap<String, String> {
    static GLYPHS_MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    GLYPHS_MAP.get_or_init(|| serde_json::from_str(GLYPHS).unwrap_or_default())
}

/// Serves `logos/<key>.svg`; `svg()` tints them with the text color, like `currentColor`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
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
