//! API price table (USD per million tokens) and model-id matching.
//!
//! # Built-in table
//! [`BUILTIN`] lists Anthropic first-party API list prices as documented on **2026-11-03**
//! (Claude) and OpenAI standard-tier list prices for the models Codex has used up to the
//! GPT-5.2 line (o3 / o4-mini / GPT-4.1 / codex-mini, GPT-5, 5.1, 5.2 and their Codex variants,
//! GPT-5 pro). Newer OpenAI models seen in Codex rollouts (for example the GPT-6 line) have no
//! built-in price: their tokens are reported as *unpriced* until `pricing.json` gives one.
//! Long-context and fast-mode premiums are not modelled (a `speed: "fast"` Claude turn is
//! recorded under `<model>-fast`, which has no built-in price).
//!
//! # `pricing.json` (optional, `<data dir>\pricing.json`)
//! Overrides and extends the built-in table. Format:
//!
//! ```json
//! {
//!   "models": {
//!     "gpt-6-luna":      { "input": 1.25, "output": 10.0, "cacheRead": 0.125, "cacheWrite": 1.25 },
//!     "claude-opus-5-5": { "input": 4.0, "output": 20.0, "cacheRead": 0.2, "cacheWrite": 5.0, "cacheWrite1h": 8.0 },
//!     "claude-opus-4-1": null
//!   }
//! }
//! ```
//!
//! * Keys are model ids; they are normalised like transcript ids (lower case, provider prefixes
//!   such as `anthropic.` / `openai/`, date suffixes `-20250514` / `@20250514`, Bedrock `-v1:0`
//!   and `[1m]` tags removed), then matched **exactly** — `gpt-5.2` never prices `gpt-5.2-pro`.
//! * Every rate is USD per million tokens. `input`, `output`, `cacheRead` and `cacheWrite`
//!   (5-minute cache writes) are required; `cacheWrite1h` is optional and defaults to
//!   `cacheWrite`. `snake_case` spellings (`cache_read`, `cache_write`, `cache_write_1h`) are
//!   accepted too. An entry missing a required rate is ignored (logged), never half-guessed.
//! * `null` removes a built-in price (the model then counts as unpriced).
//! * An unreadable file is ignored (logged); the built-in table is used alone.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::ModelPrice;

/// Full rate card of one model: the [`ModelPrice`] plus the 1-hour cache-write rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Input, output, cache read and 5-minute cache write.
    pub base: ModelPrice,
    /// 1-hour cache write (Anthropic: 2 × input).
    pub cache_write_1h: f64,
}

impl Rates {
    const fn new(
        input: f64,
        output: f64,
        cache_read: f64,
        cache_write: f64,
        cache_write_1h: f64,
    ) -> Self {
        Self {
            base: ModelPrice {
                input,
                output,
                cache_read,
                cache_write,
            },
            cache_write_1h,
        }
    }
}

/// Anthropic rate card from the input price: 5-minute writes 1.25 ×, 1-hour writes 2 ×,
/// cache reads at the given multiple of input.
const fn anthropic(input: f64, output: f64, read_multiple: f64) -> Rates {
    Rates::new(
        input,
        output,
        input * read_multiple,
        input * 1.25,
        input * 2.0,
    )
}

/// OpenAI rate card: cached input at the given price, no cache-write premium.
const fn openai(input: f64, output: f64, cached: f64) -> Rates {
    Rates::new(input, output, cached, input, input)
}

/// Built-in prices, keyed by normalised model id (see module docs for sources and dates).
pub const BUILTIN: &[(&str, Rates)] = &[
    // --- Anthropic (first-party API list prices, cached 2026-11-03) ---
    ("claude-fable-5-2", anthropic(8.0, 40.0, 0.025)),
    ("claude-mythos-5-2", anthropic(8.0, 40.0, 0.025)),
    ("claude-fable-5-1", anthropic(10.0, 50.0, 0.025)),
    ("claude-mythos-5-1", anthropic(10.0, 50.0, 0.025)),
    ("claude-fable-5", anthropic(10.0, 50.0, 0.1)),
    ("claude-mythos-5", anthropic(10.0, 50.0, 0.1)),
    ("claude-opus-5-5", anthropic(4.0, 20.0, 0.05)),
    ("claude-opus-5", anthropic(5.0, 25.0, 0.1)),
    ("claude-opus-4-8", anthropic(5.0, 25.0, 0.1)),
    ("claude-opus-4-7", anthropic(5.0, 25.0, 0.1)),
    ("claude-opus-4-6", anthropic(5.0, 25.0, 0.1)),
    ("claude-opus-4-5", anthropic(5.0, 25.0, 0.1)),
    ("claude-opus-4-1", anthropic(15.0, 75.0, 0.1)),
    ("claude-opus-4", anthropic(15.0, 75.0, 0.1)),
    ("claude-sonnet-5-5", anthropic(2.0, 10.0, 0.05)),
    ("claude-sonnet-5", anthropic(2.0, 10.0, 0.1)),
    ("claude-sonnet-4-6", anthropic(3.0, 15.0, 0.1)),
    ("claude-sonnet-4-5", anthropic(3.0, 15.0, 0.1)),
    ("claude-sonnet-4", anthropic(3.0, 15.0, 0.1)),
    ("claude-3-7-sonnet", anthropic(3.0, 15.0, 0.1)),
    // Haiku 5.5: base rate for prompts up to 100K tokens (longer prompts cost 5 × more).
    ("claude-haiku-5-5", anthropic(0.10, 0.50, 0.1)),
    ("claude-haiku-4-5", anthropic(1.0, 5.0, 0.1)),
    ("claude-3-5-haiku", anthropic(0.80, 4.0, 0.1)),
    // --- OpenAI (standard-tier list prices, 2025) ---
    ("o3", openai(2.0, 8.0, 0.5)),
    ("o4-mini", openai(1.10, 4.40, 0.275)),
    ("codex-mini-latest", openai(1.50, 6.0, 0.375)),
    ("gpt-4.1", openai(2.0, 8.0, 0.5)),
    ("gpt-5", openai(1.25, 10.0, 0.125)),
    ("gpt-5-codex", openai(1.25, 10.0, 0.125)),
    ("gpt-5.1", openai(1.25, 10.0, 0.125)),
    ("gpt-5.1-codex", openai(1.25, 10.0, 0.125)),
    ("gpt-5.1-codex-max", openai(1.25, 10.0, 0.125)),
    ("gpt-5.1-codex-mini", openai(0.25, 2.0, 0.025)),
    ("gpt-5.2", openai(1.75, 14.0, 0.175)),
    ("gpt-5.2-codex", openai(1.75, 14.0, 0.175)),
    // No cached-input discount.
    ("gpt-5-pro", openai(15.0, 120.0, 15.0)),
    ("gpt-5-mini", openai(0.25, 2.0, 0.025)),
    ("gpt-5-nano", openai(0.05, 0.40, 0.005)),
];

/// Normalise a model id for matching: lower case, provider/region prefixes, `[..]` tags,
/// Vertex `@date`, Bedrock `-vN:M` and `-YYYYMMDD` / `-YYYY-MM-DD` date suffixes removed.
pub fn normalize_model(model: &str) -> String {
    let mut s = model.trim().to_ascii_lowercase();
    if let Some(i) = s.find('[') {
        s.truncate(i);
    }
    if let Some(i) = s.rfind('/') {
        s = s[i + 1..].to_string();
    }
    for prefix in ["anthropic.", "openai."] {
        if let Some(i) = s.find(prefix) {
            s = s[i + prefix.len()..].to_string();
        }
    }
    if let Some(i) = s.find('@') {
        s.truncate(i);
    }
    // Bedrock "-v1:0" / "-v2".
    if let Some(i) = s.rfind("-v") {
        let tail = &s[i + 2..];
        if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit() || c == ':') {
            s.truncate(i);
        }
    }
    // Model ids are ASCII; anything else cannot match the table, so keep it as is (and never
    // slice inside a multi-byte character below).
    if !s.is_ascii() {
        return s.trim().to_string();
    }
    // "-20250514"
    if s.len() > 9 {
        let (head, tail) = s.split_at(s.len() - 9);
        if tail.starts_with('-') && tail[1..].chars().all(|c| c.is_ascii_digit()) {
            s = head.to_string();
        }
    }
    // "-2025-08-07"
    if s.len() > 11 {
        let (head, tail) = s.split_at(s.len() - 11);
        let b = tail.as_bytes();
        let date_like = b[0] == b'-'
            && b[5] == b'-'
            && b[8] == b'-'
            && tail[1..5].chars().all(|c| c.is_ascii_digit())
            && tail[6..8].chars().all(|c| c.is_ascii_digit())
            && tail[9..].chars().all(|c| c.is_ascii_digit());
        if date_like {
            s = head.to_string();
        }
    }
    s.trim().to_string()
}

/// Built-in rates of `model` (normalised, exact match).
pub fn builtin_rates(model: &str) -> Option<Rates> {
    let key = normalize_model(model);
    BUILTIN.iter().find(|(id, _)| *id == key).map(|(_, r)| *r)
}

/// Built-in table plus `pricing.json` overrides.
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    /// Normalised id → rates (`None` = explicitly unpriced).
    overrides: HashMap<String, Option<Rates>>,
}

impl PriceTable {
    /// Built-in prices only.
    pub fn builtin() -> Self {
        Self::default()
    }

    /// Built-in prices plus the overrides in `path` (missing/invalid file = built-in only).
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::builtin(),
            Err(e) => {
                log::warn!(
                    "analytics: pricing.json unreadable ({}), using built-in prices",
                    e.kind()
                );
                Self::builtin()
            }
        }
    }

    /// Parse a `pricing.json` text (see module docs). Bad entries are skipped.
    pub fn parse(text: &str) -> Self {
        let mut table = Self::builtin();
        let root: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => {
                log::warn!("analytics: pricing.json is not valid JSON, using built-in prices");
                return table;
            }
        };
        let Some(models) = root.get("models").and_then(Value::as_object) else {
            log::warn!("analytics: pricing.json has no \"models\" object");
            return table;
        };
        for (id, entry) in models {
            let key = normalize_model(id);
            if key.is_empty() {
                continue;
            }
            if entry.is_null() {
                table.overrides.insert(key, None);
                continue;
            }
            match parse_rates(entry) {
                Some(r) => {
                    table.overrides.insert(key, Some(r));
                }
                None => log::warn!("analytics: pricing.json entry \"{key}\" ignored (needs input, output, cacheRead, cacheWrite)"),
            }
        }
        table
    }

    /// Rates of `model`, `None` when unknown.
    pub fn rates(&self, model: &str) -> Option<Rates> {
        let key = normalize_model(model);
        match self.overrides.get(&key) {
            Some(r) => *r,
            None => builtin_rates(&key),
        }
    }

    /// Number of override entries (for diagnostics).
    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }
}

fn parse_rates(v: &Value) -> Option<Rates> {
    let get = |names: &[&str]| -> Option<f64> {
        names
            .iter()
            .find_map(|n| v.get(*n).and_then(Value::as_f64))
            .filter(|x| x.is_finite() && *x >= 0.0)
    };
    let input = get(&["input"])?;
    let output = get(&["output"])?;
    let cache_read = get(&["cacheRead", "cache_read"])?;
    let cache_write = get(&["cacheWrite", "cache_write"])?;
    let cache_write_1h = get(&["cacheWrite1h", "cache_write_1h"]).unwrap_or(cache_write);
    Some(Rates::new(
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_prefixes_and_suffixes() {
        assert_eq!(
            normalize_model("claude-sonnet-4-20250514"),
            "claude-sonnet-4"
        );
        assert_eq!(
            normalize_model("us.anthropic.claude-opus-4-1-20250805-v1:0"),
            "claude-opus-4-1"
        );
        assert_eq!(
            normalize_model("claude-opus-4-5@20251101"),
            "claude-opus-4-5"
        );
        assert_eq!(
            normalize_model("anthropic/claude-haiku-4-5"),
            "claude-haiku-4-5"
        );
        assert_eq!(normalize_model("Claude-Opus-5-5[1m]"), "claude-opus-5-5");
        assert_eq!(normalize_model("openai/gpt-5-2025-08-07"), "gpt-5");
        assert_eq!(normalize_model("gpt-5.1-codex"), "gpt-5.1-codex");
        assert_eq!(normalize_model("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(normalize_model("modèle-20250514"), "modèle-20250514");
        assert_eq!(normalize_model(""), "");
    }

    #[test]
    fn exact_match_never_prices_a_longer_family() {
        assert!(builtin_rates("gpt-5").is_some());
        assert_eq!(builtin_rates("gpt-5-turbo"), None);
        assert_eq!(builtin_rates("gpt-5.2-codex-x"), None);
        assert_eq!(
            builtin_rates("gpt-5-pro").map(|r| r.base.output),
            Some(120.0)
        );
        assert_eq!(
            builtin_rates("gpt-5.2-codex").map(|r| r.base.cache_read),
            Some(0.175)
        );
        assert_eq!(builtin_rates("gpt-6-luna"), None);
        assert_eq!(builtin_rates("claude-opus-5-5-fast"), None);
        assert_eq!(
            builtin_rates("claude-opus-5-5-20261001").map(|r| r.base.input),
            Some(4.0)
        );
    }

    #[test]
    fn anthropic_cache_rates() {
        let r = builtin_rates("claude-opus-5-5").expect("priced");
        assert_eq!(r.base.output, 20.0);
        assert!((r.base.cache_read - 0.20).abs() < 1e-12);
        assert!((r.base.cache_write - 5.0).abs() < 1e-12);
        assert!((r.cache_write_1h - 8.0).abs() < 1e-12);
        let s = builtin_rates("claude-sonnet-4-5-20250929").expect("priced");
        assert!((s.base.cache_read - 0.30).abs() < 1e-12);
    }

    #[test]
    fn overrides_add_replace_and_remove() {
        let t = PriceTable::parse(
            r#"{"models":{
                "gpt-6-luna":{"input":1,"output":8,"cacheRead":0.1,"cacheWrite":1},
                "claude-opus-5-5":{"input":3,"output":15,"cache_read":0.3,"cache_write":3.75,"cache_write_1h":6},
                "claude-opus-4-1":null,
                "half":{"input":1,"output":2}
            }}"#,
        );
        assert_eq!(t.rates("gpt-6-luna").map(|r| r.base.output), Some(8.0));
        assert_eq!(t.rates("gpt-6-luna").map(|r| r.cache_write_1h), Some(1.0));
        assert_eq!(
            t.rates("claude-opus-5-5").map(|r| r.cache_write_1h),
            Some(6.0)
        );
        assert_eq!(t.rates("claude-opus-4-1-20250805"), None);
        assert_eq!(t.rates("half"), None);
        assert!(t.rates("claude-haiku-4-5").is_some());
        assert_eq!(t.override_count(), 3);
    }

    #[test]
    fn bad_pricing_file_falls_back_to_builtin() {
        let t = PriceTable::parse("not json");
        assert!(t.rates("claude-opus-5-5").is_some());
        let dir = tempfile::tempdir().expect("tempdir");
        let t = PriceTable::load(&dir.path().join("pricing.json"));
        assert_eq!(t.override_count(), 0);
    }
}
