//! Theme tokens.
//!
//! * Notch pill and rings (`pill`, `edge`, `track`, `ink` …) and the grade
//!   colours (`ample` / `watch` / `crit`). The notch's hover card and the reset card are
//!   drawn in the pill's material (`pill` surface, `edge` hairline, `track` bar tracks) and their
//!   progress bars use the same grade colours as the rings: one severity palette on the notch.
//! * The settings window, menus and toasts use the shadcn/ui v4 "neutral" palette (light and dark `:root` / `.dark` variables:
//!   `background`, `card`, `popover`, `muted`, `primary`, `accent`, `border`, `input`, `ring`,
//!   `destructive`, `sidebar*`), plus Tailwind status tones for warning / critical / success.
//!   Monochrome: no system accent colour.
//! * Fonts: [`Fonts`], resolved once by [`resolve_fonts`] (Segoe UI Variable when installed).
//!
//! One resolved [`Theme`] per window, from `settings.theme` and the OS app theme
//! ([`Theme::resolve`] / [`Theme::for_settings`]). Every window follows it: the notch, its hover
//! card, the reset card, the strip's card and the settings window.

use codenotch_core::config::{ColorTransition, Settings, ThemeMode};
use codenotch_core::format::{grade, ramp, Grade};
use codenotch_core::model::ProviderKind;
use gpui::{hsla, rgb, rgba, Hsla, Rgba, WindowAppearance};
use std::sync::OnceLock;

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}
fn ca(hex_rgba: u32) -> Hsla {
    rgba(hex_rgba).into()
}

// ---------------------------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------------------------

/// Font families of the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fonts {
    /// Body text ("Segoe UI Variable Text", else "Segoe UI").
    pub text: &'static str,
    /// Page titles ("Segoe UI Variable Display", else "Segoe UI").
    pub display: &'static str,
    /// Glyphs ("Segoe Fluent Icons" on Windows 11, else "Segoe MDL2 Assets").
    pub icons: &'static str,
}

impl Fonts {
    /// Used until [`resolve_fonts`] ran (and in tests).
    pub const DEFAULT: Fonts = Fonts {
        text: "Segoe UI",
        display: "Segoe UI",
        icons: "Segoe Fluent Icons",
    };

    /// Pick the best installed family of each role from the system's family names.
    pub fn pick(installed: &[String]) -> Fonts {
        let has = |name: &str| installed.iter().any(|f| f.eq_ignore_ascii_case(name));
        let first = |options: &[&'static str], fallback: &'static str| {
            options.iter().copied().find(|f| has(f)).unwrap_or(fallback)
        };
        Fonts {
            text: first(&["Segoe UI Variable Text", "Segoe UI"], "Segoe UI"),
            display: first(&["Segoe UI Variable Display", "Segoe UI"], "Segoe UI"),
            icons: first(
                &["Segoe Fluent Icons", "Segoe MDL2 Assets"],
                "Segoe Fluent Icons",
            ),
        }
    }
}

static FONTS: OnceLock<Fonts> = OnceLock::new();

/// Resolve the font families once from the platform text system (called by [`crate::init`]).
pub fn resolve_fonts(cx: &gpui::App) {
    let installed = cx.text_system().all_font_names();
    let fonts = Fonts::pick(&installed);
    log::debug!("settings fonts: {fonts:?}");
    let _ = FONTS.set(fonts);
}

/// The resolved fonts ([`Fonts::DEFAULT`] before [`resolve_fonts`]).
pub fn fonts() -> Fonts {
    FONTS.get().copied().unwrap_or(Fonts::DEFAULT)
}

// ---------------------------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------------------------

/// All colour tokens for one appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub fonts: Fonts,
    // --- notch pill & rings ---
    pub pill: Hsla,
    pub edge: Hsla,
    pub rest_edge: Hsla,
    /// Ring track and the notch card's bar track: a faint grey on the pill.
    pub track: Hsla,
    /// Inner disc behind the glyph. Equal to `pill` (the glyph sits on the plain pill); kept for
    /// callers that paint a ring on another surface.
    pub hole: Hsla,
    pub ample: Hsla,
    pub watch: Hsla,
    pub crit: Hsla,
    /// Text and glyphs on the pill.
    pub ink: Hsla,
    pub ink2: Hsla,
    /// Unknown grade on the pill.
    pub ink_dim: Hsla,
    // --- shadcn/ui neutral ---
    /// Window background.
    pub background: Hsla,
    /// Body text.
    pub foreground: Hsla,
    /// Card surface (settings cards, notch hover card, reset card).
    pub card: Hsla,
    /// Popover surface (menus, dropdown lists, toasts).
    pub popover: Hsla,
    /// Card / popover / divider border.
    pub border: Hsla,
    /// Control border (inputs, outline buttons, select triggers) and the unchecked switch track.
    pub input: Hsla,
    /// Focus ring (drawn at ~50 % alpha, 3 px) and the focused input border.
    pub ring: Hsla,
    /// Muted surface (progress / chart tracks).
    pub muted: Hsla,
    /// Secondary text (descriptions, captions, statuses).
    pub muted_foreground: Hsla,
    /// Primary fill (default button, checked switch, slider range, chart bars).
    pub primary: Hsla,
    /// Text on `primary`.
    pub primary_foreground: Hsla,
    /// Hover / selected fill of ghost items (menu items, nav, icon buttons).
    pub accent: Hsla,
    pub destructive: Hsla,
    /// Fill of outline controls (light: background; dark: `input` at 30 %).
    pub control: Hsla,
    /// Hovered outline control (light: accent; dark: `input` at 50 %).
    pub control_hover: Hsla,
    /// Navigation pane.
    pub sidebar: Hsla,
    pub sidebar_border: Hsla,
    /// Hovered / active navigation item.
    pub sidebar_accent: Hsla,
    // --- status (Tailwind 600 on light, 400 on dark) ---
    pub success: Hsla,
    pub warning: Hsla,
    pub critical: Hsla,
    /// Close caption button hover (Windows convention, white glyph).
    pub caption_close: Hsla,
    /// Cards get a `shadow-sm` (light only).
    pub card_shadow: bool,
    // --- charts (series colours: see `provider_color` / `model_color`) ---
    /// "Others" series and swatch: a neutral grey.
    pub chart_others: Hsla,
    /// A single series not tied to a provider (sessions of every provider): neutral.
    pub chart_neutral: Hsla,
    // --- activity heatmap (GitHub contribution levels 0..=4) ---
    pub heat_0: Hsla,
    pub heat_1: Hsla,
    pub heat_2: Hsla,
    pub heat_3: Hsla,
    pub heat_4: Hsla,
    /// Hairline around every heat cell (~6 % of the text colour).
    pub heat_border: Hsla,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            fonts: fonts(),
            pill: c(0x000000),
            edge: c(0x2e2e2e),
            rest_edge: c(0x5c5c5c),
            track: c(0x2a2a2a),
            hole: c(0x000000),
            ample: c(0x00ff88),
            watch: c(0xf2ff00),
            crit: c(0xff3f00),
            ink: c(0xffffff),
            ink2: c(0xe8e8ea),
            ink_dim: c(0x808080),
            background: c(0x0a0a0a),
            foreground: c(0xfafafa),
            card: c(0x171717),
            popover: c(0x171717),
            border: ca(0xffffff1a),
            input: ca(0xffffff26),
            ring: c(0x737373),
            muted: c(0x262626),
            muted_foreground: c(0xa1a1a1),
            primary: c(0xe5e5e5),
            primary_foreground: c(0x171717),
            accent: c(0x262626),
            destructive: c(0xff6467),
            control: ca(0xffffff0b),
            control_hover: ca(0xffffff13),
            sidebar: c(0x171717),
            sidebar_border: ca(0xffffff1a),
            sidebar_accent: c(0x262626),
            success: c(0x4ade80),
            warning: c(0xfbbf24),
            critical: c(0xf87171),
            caption_close: c(0xc42b1c),
            card_shadow: false,
            chart_others: c(0x737373),
            chart_neutral: c(0xd4d4d4),
            // Level 0 a step above the card (#171717) so empty days still read as cells.
            heat_0: c(0x232323),
            heat_1: c(0x155d34),
            heat_2: c(0x1e8a44),
            heat_3: c(0x2db555),
            heat_4: c(0x4ae168),
            heat_border: ca(0xffffff0f),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            fonts: fonts(),
            pill: c(0xf5f5f7),
            edge: c(0xd2d2d7),
            rest_edge: c(0x86868b),
            track: ca(0x00000014),
            hole: c(0xf5f5f7),
            ample: c(0x00a356),
            watch: c(0xd17c00),
            crit: c(0xff3f00),
            ink: c(0x1d1d1f),
            ink2: c(0x1d1d1f),
            ink_dim: c(0x6b6b6b),
            background: c(0xffffff),
            foreground: c(0x0a0a0a),
            card: c(0xffffff),
            popover: c(0xffffff),
            border: c(0xe5e5e5),
            input: c(0xe5e5e5),
            ring: c(0xa1a1a1),
            muted: c(0xf5f5f5),
            muted_foreground: c(0x737373),
            primary: c(0x171717),
            primary_foreground: c(0xfafafa),
            accent: c(0xf5f5f5),
            destructive: c(0xe7000b),
            control: c(0xffffff),
            control_hover: c(0xf5f5f5),
            sidebar: c(0xfafafa),
            sidebar_border: c(0xe5e5e5),
            sidebar_accent: c(0xf5f5f5),
            success: c(0x16a34a),
            warning: c(0xd97706),
            critical: c(0xdc2626),
            caption_close: c(0xc42b1c),
            card_shadow: true,
            chart_others: c(0xa3a3a3),
            chart_neutral: c(0x404040),
            heat_0: c(0xebedf0),
            heat_1: c(0x9be9a8),
            heat_2: c(0x40c463),
            heat_3: c(0x30a14e),
            heat_4: c(0x216e39),
            heat_border: ca(0x0000000f),
        }
    }

    /// Pick dark/light from the setting and the OS appearance.
    pub fn resolve(mode: ThemeMode, appearance: WindowAppearance) -> Self {
        if is_dark(mode, appearance) {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// Theme for the current settings, using the app-wide OS appearance.
    pub fn for_settings(settings: &Settings, cx: &gpui::App) -> Self {
        Self::resolve(settings.theme, cx.window_appearance())
    }

    /// Colour of a grade on the pill. Unknown → `ink_dim`.
    pub fn grade_color(&self, grade: Grade) -> Hsla {
        match grade {
            Grade::Ok => self.ample,
            Grade::Warn => self.watch,
            Grade::Critical => self.crit,
            Grade::Unknown => self.ink_dim,
        }
    }

    /// Colour of a grade in the settings window: the status tones. Unknown → muted text.
    /// (The notch card and reset card use [`Theme::grade_color`], like the rings.)
    pub fn status_color(&self, grade: Grade) -> Hsla {
        match grade {
            Grade::Ok => self.success,
            Grade::Warn => self.warning,
            Grade::Critical => self.critical,
            Grade::Unknown => self.muted_foreground,
        }
    }

    /// Series colour of a provider: a fixed hue per provider (Claude coral, Codex teal-green,
    /// ...), the same in both themes; only the lightness moves a little for contrast.
    pub fn provider_color(&self, p: &ProviderKind) -> Hsla {
        let (h, s, l_light, l_dark) = provider_tone(p);
        hsla(h / 360.0, s, if self.dark { l_dark } else { l_light }, 1.0)
    }

    /// Colour of a provider's model: a shade of [`Theme::provider_color`] picked by the model's
    /// `rank` within its provider (0 = the provider colour itself, then alternately lighter /
    /// darker with a small hue shift). Deterministic, so a model keeps its colour everywhere.
    pub fn model_color(&self, p: &ProviderKind, rank: usize) -> Hsla {
        const SHADES: [(f32, f32); 7] = [
            (0.0, 0.0),
            (8.0, 0.13),
            (-8.0, -0.11),
            (14.0, 0.22),
            (-14.0, -0.18),
            (4.0, 0.07),
            (-4.0, -0.05),
        ];
        let (h, s, l_light, l_dark) = provider_tone(p);
        let (dh, dl) = SHADES[rank % SHADES.len()];
        let l = if self.dark { l_dark } else { l_light } + dl;
        hsla(
            (h + dh).rem_euclid(360.0) / 360.0,
            s,
            l.clamp(0.24, 0.84),
            1.0,
        )
    }

    /// Heatmap cell colour of a level 0..=4 (0 = no activity; higher levels clamp to 4).
    pub fn heat_color(&self, level: u8) -> Hsla {
        match level {
            0 => self.heat_0,
            1 => self.heat_1,
            2 => self.heat_2,
            3 => self.heat_3,
            _ => self.heat_4,
        }
    }

    /// Ring colour for a percent **used** under the user's thresholds and transition mode.
    /// (Remaining mode never changes the colour: always pass *used*.)
    pub fn usage_color(&self, used: Option<f64>, settings: &Settings) -> Hsla {
        graded(used, settings, |g| self.grade_color(g))
    }

    /// Status-tone colour for a percent **used** (same rule as [`Theme::usage_color`]), for the
    /// settings window.
    pub fn usage_status_color(&self, used: Option<f64>, settings: &Settings) -> Hsla {
        graded(used, settings, |g| self.status_color(g))
    }
}

/// (hue in degrees, saturation, lightness in light, lightness in dark) of a provider's series
/// colour. Hues are spread so no two providers read alike; unknown ids get a hashed hue.
fn provider_tone(p: &ProviderKind) -> (f32, f32, f32, f32) {
    match p {
        ProviderKind::Claude => (15.0, 0.78, 0.56, 0.62),
        ProviderKind::Grok => (44.0, 0.90, 0.47, 0.55),
        ProviderKind::Qoder => (82.0, 0.60, 0.40, 0.50),
        ProviderKind::Antigravity => (125.0, 0.50, 0.42, 0.55),
        ProviderKind::Codex => (162.0, 0.70, 0.36, 0.45),
        ProviderKind::Cursor => (198.0, 0.80, 0.42, 0.55),
        ProviderKind::Glm => (228.0, 0.80, 0.58, 0.66),
        ProviderKind::OpenCode => (262.0, 0.70, 0.60, 0.70),
        ProviderKind::Copilot => (296.0, 0.55, 0.50, 0.64),
        ProviderKind::CommandCode => (335.0, 0.72, 0.52, 0.64),
        ProviderKind::Custom => (215.0, 0.16, 0.48, 0.62),
        ProviderKind::Other(id) => {
            let hash = id.bytes().fold(2_166_136_261u32, |h, b| {
                (h ^ u32::from(b)).wrapping_mul(16_777_619)
            });
            ((hash % 360) as f32, 0.55, 0.50, 0.62)
        }
    }
}

/// Dark when the setting says so, or follows a dark OS app theme.
pub fn is_dark(mode: ThemeMode, appearance: WindowAppearance) -> bool {
    match mode {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    }
}

fn graded(used: Option<f64>, settings: &Settings, color: impl Fn(Grade) -> Hsla) -> Hsla {
    let (warn, crit) = (settings.warn_percent(), settings.critical_percent());
    match settings.color_transition {
        ColorTransition::HardStep => color(grade(used, warn, crit)),
        ColorTransition::Ramp => match ramp(used, warn) {
            Some((a, b, t)) => lerp(color(a), color(b), t),
            None => color(Grade::Unknown),
        },
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

/// Per-channel sRGB linear interpolation (ramp).
pub fn lerp(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_prefer_variable_and_fall_back() {
        let win11: Vec<String> = [
            "Segoe UI",
            "Segoe UI Variable Text",
            "Segoe UI Variable Display",
            "Segoe Fluent Icons",
        ]
        .map(String::from)
        .to_vec();
        let f = Fonts::pick(&win11);
        assert_eq!(
            (f.text, f.display, f.icons),
            (
                "Segoe UI Variable Text",
                "Segoe UI Variable Display",
                "Segoe Fluent Icons"
            )
        );
        let win10: Vec<String> = ["Segoe UI", "Segoe MDL2 Assets"].map(String::from).to_vec();
        let f = Fonts::pick(&win10);
        assert_eq!(
            (f.text, f.display, f.icons),
            ("Segoe UI", "Segoe UI", "Segoe MDL2 Assets")
        );
    }

    #[test]
    fn both_settings_themes_are_complete() {
        for t in [Theme::light(), Theme::dark()] {
            for (name, surface) in [
                ("background", t.background),
                ("card", t.card),
                ("popover", t.popover),
                ("sidebar", t.sidebar),
            ] {
                assert_eq!(surface.a, 1.0, "{name} must be opaque");
            }
            assert_ne!(t.foreground, t.muted_foreground);
            assert_ne!(t.primary, t.primary_foreground);
            assert_ne!(t.background, t.foreground);
            // Monochrome: the primary fill is a grey (no system accent hue).
            assert!(t.primary.s < 0.01, "primary must be neutral");
        }
        assert_ne!(Theme::light().background, Theme::dark().background);
        assert!(Theme::light().card_shadow && !Theme::dark().card_shadow);
    }

    #[test]
    fn chart_and_heat_tokens() {
        let l = Theme::light();
        let d = Theme::dark();
        assert_eq!(
            (0..5).map(|i| hex(l.heat_color(i))).collect::<Vec<_>>(),
            [0xebedf0, 0x9be9a8, 0x40c463, 0x30a14e, 0x216e39]
        );
        assert_eq!(d.heat_color(9), d.heat_4);
        // Dark: an empty day is a step above the card, level 1 clearly above level 0.
        assert!(luminance(d.heat_0) > luminance(d.card));
        assert!(contrast(d.heat_1, d.heat_0) > 1.4);
        for t in [l, d] {
            for i in 0..5 {
                assert_eq!(t.heat_color(i as u8).a, 1.0);
            }
            assert!(t.heat_border.a > 0.0 && t.heat_border.a < 0.1);
            assert!(t.chart_others.s < 0.01, "Others is a neutral grey");
        }
    }

    #[test]
    fn provider_colours_are_stable_and_distinct() {
        let (l, d) = (Theme::light(), Theme::dark());
        let all = ProviderKind::CANONICAL_ORDER;
        for p in all {
            // Same hue in both themes.
            let (a, b) = (l.provider_color(p), d.provider_color(p));
            assert!((a.h - b.h).abs() < 1e-4, "{p:?}");
            assert_eq!(l.model_color(p, 0), a);
        }
        // Every pair of (saturated) providers is at least 25 degrees apart; Custom is a slate.
        let hued: Vec<&ProviderKind> = all.iter().filter(|p| **p != ProviderKind::Custom).collect();
        for (i, p) in hued.iter().enumerate() {
            for q in &hued[i + 1..] {
                let (a, b) = (d.provider_color(p).h * 360.0, d.provider_color(q).h * 360.0);
                let gap = (a - b).abs().min(360.0 - (a - b).abs());
                assert!(gap >= 25.0, "{p:?} vs {q:?}: {gap}");
            }
        }
        // Claude is warm (coral), Codex green / teal.
        let claude = d.provider_color(&ProviderKind::Claude).h * 360.0;
        let codex = d.provider_color(&ProviderKind::Codex).h * 360.0;
        assert!((5.0..30.0).contains(&claude) && (140.0..180.0).contains(&codex));
        // Model shades stay in the provider's hue family and differ from each other.
        let base = d.provider_color(&ProviderKind::Claude);
        let shades: Vec<Hsla> = (0..5)
            .map(|r| d.model_color(&ProviderKind::Claude, r))
            .collect();
        for (i, s) in shades.iter().enumerate() {
            assert!((s.h - base.h).abs() * 360.0 <= 15.0);
            assert!(shades[..i].iter().all(|o| o != s));
        }
        let other = ProviderKind::Other("acme".into());
        assert_eq!(d.provider_color(&other), d.provider_color(&other));
    }

    #[test]
    fn shadcn_neutral_values() {
        let hex = |h: Hsla| {
            let r: Rgba = h.into();
            let b = |v: f32| (v * 255.0).round() as u32;
            (b(r.r) << 16) | (b(r.g) << 8) | b(r.b)
        };
        let l = Theme::light();
        assert_eq!(
            (
                hex(l.background),
                hex(l.foreground),
                hex(l.primary),
                hex(l.border)
            ),
            (0xffffff, 0x0a0a0a, 0x171717, 0xe5e5e5)
        );
        let d = Theme::dark();
        assert_eq!(
            (hex(d.background), hex(d.card), hex(d.primary), hex(d.muted)),
            (0x0a0a0a, 0x171717, 0xe5e5e5, 0x262626)
        );
    }

    #[test]
    fn system_mode_follows_the_os() {
        assert!(is_dark(ThemeMode::System, WindowAppearance::Dark));
        assert!(!is_dark(ThemeMode::System, WindowAppearance::Light));
        assert!(is_dark(ThemeMode::Dark, WindowAppearance::Light));
        assert!(!is_dark(ThemeMode::Light, WindowAppearance::Dark));
        assert!(Theme::resolve(ThemeMode::Dark, WindowAppearance::Light).dark);
    }

    fn hex(h: Hsla) -> u32 {
        let r: Rgba = h.into();
        let b = |v: f32| (v * 255.0).round() as u32;
        (b(r.r) << 16) | (b(r.g) << 8) | b(r.b)
    }

    /// WCAG relative luminance of an opaque colour.
    fn luminance(h: Hsla) -> f32 {
        let r: Rgba = h.into();
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r.r) + 0.7152 * lin(r.g) + 0.0722 * lin(r.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn notch_tokens() {
        let d = Theme::dark();
        assert_eq!(
            (hex(d.pill), hex(d.track), hex(d.hole), hex(d.edge)),
            (0x000000, 0x2a2a2a, 0x000000, 0x2e2e2e)
        );
        assert_eq!(
            (hex(d.ample), hex(d.watch), hex(d.crit)),
            (0x00ff88, 0xf2ff00, 0xff3f00)
        );
        let l = Theme::light();
        assert_eq!((hex(l.watch), hex(l.hole)), (0xd17c00, hex(l.pill)));
        // light track: ~8 % black, a faint grey rather than a grey button
        assert!((l.track.a - 0x14 as f32 / 255.0).abs() < 1e-3);
        for t in [d, l] {
            assert_eq!(t.hole, t.pill, "the glyph sits on the plain pill");
        }
    }

    #[test]
    fn grade_colours_read_on_their_pill() {
        // Graphical objects: at least 3:1 against the surface they sit on.
        let l = Theme::light();
        for (name, col) in [("ample", l.ample), ("watch", l.watch), ("crit", l.crit)] {
            for surface in [c(0xffffff), l.pill] {
                let k = contrast(col, surface);
                assert!(k >= 2.9, "light {name} {k:.2}:1");
            }
        }
        let d = Theme::dark();
        for c in [d.ample, d.watch, d.crit] {
            assert!(contrast(c, d.pill) >= 4.5);
            // the arc stands out from its track
            assert!(contrast(c, d.track) >= 3.0);
        }
        assert!(contrast(d.track, d.pill) < 1.6, "dark track stays faint");
    }

    #[test]
    fn app_theme_ignores_legacy_notch_always_dark() {
        // The notch follows the app theme: the legacy `notchAlwaysDark` flag is ignored.
        let mut s = Settings {
            theme: ThemeMode::Light,
            notch_always_dark: true,
            ..Settings::default()
        };
        assert!(!Theme::resolve(s.theme, WindowAppearance::Light).dark);
        assert!(!Theme::resolve(s.theme, WindowAppearance::Dark).dark);
        s.theme = ThemeMode::Dark;
        assert!(Theme::resolve(s.theme, WindowAppearance::Light).dark);
        s.theme = ThemeMode::System;
        assert!(Theme::resolve(s.theme, WindowAppearance::Dark).dark);
        assert!(!Theme::resolve(s.theme, WindowAppearance::Light).dark);
        assert!(Theme::resolve(s.theme, WindowAppearance::VibrantDark).dark);
    }

    #[test]
    fn settings_status_tones() {
        let t = Theme::light();
        let s = Settings::default(); // 80 / 95
        assert_eq!(t.usage_status_color(Some(10.0), &s), t.success);
        assert_eq!(t.usage_status_color(Some(85.0), &s), t.warning);
        assert_eq!(t.usage_status_color(Some(99.0), &s), t.critical);
        assert_eq!(t.usage_status_color(None, &s), t.muted_foreground);
    }

    #[test]
    fn grades_map_to_spec_colours() {
        let t = Theme::dark();
        let s = Settings::default(); // 80 / 95
        assert_eq!(t.usage_color(Some(10.0), &s), t.ample);
        assert_eq!(t.usage_color(Some(85.0), &s), t.watch);
        assert_eq!(t.usage_color(Some(99.0), &s), t.crit);
        assert_eq!(t.usage_color(None, &s), t.ink_dim);
    }

    #[test]
    fn ramp_ends_at_critical_only_at_100() {
        let t = Theme::dark();
        let s = Settings {
            color_transition: ColorTransition::Ramp,
            ..Settings::default()
        };
        let full: Rgba = t.usage_color(Some(100.0), &s).into();
        let crit: Rgba = t.crit.into();
        assert!((full.r - crit.r).abs() < 0.01 && (full.g - crit.g).abs() < 0.01);
        let zero: Rgba = t.usage_color(Some(0.0), &s).into();
        let ample: Rgba = t.ample.into();
        assert!((zero.g - ample.g).abs() < 0.01);
    }
}
