//! A tiny order-preserving JSON document model.
//!
//! Used for files that belong to the user (Claude Code's `settings.json`): object members keep
//! their order (duplicates included) and numbers keep their exact lexeme, so rewriting a file
//! changes nothing but the members we deliberately touch. Parsing is strict RFC 8259 (plus an
//! optional UTF-8 BOM); [`parse_prefix_object`] is a lenient variant for possibly truncated hook
//! payloads.

use std::fmt::Write as _;

/// One JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number exactly as written in the source.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    /// Members in source order.
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    /// First member named `key`.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    /// First member named `key`, mutable.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Obj(m) => m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// How a document was laid out, so a rewrite looks like the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub bom: bool,
    pub crlf: bool,
    pub trailing_newline: bool,
    pub indent: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            bom: false,
            crlf: false,
            trailing_newline: true,
            indent: 2,
        }
    }
}

/// Parse error with a byte offset (no content echoed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub what: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.offset)
    }
}

const MAX_DEPTH: usize = 256;

/// Strict parse of a whole document. Returns the value and the detected layout.
pub fn parse(text: &str) -> Result<(Json, Layout), ParseError> {
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut p = Parser {
        s: body.as_bytes(),
        i: 0,
        depth: 0,
    };
    p.ws();
    let value = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("trailing characters"));
    }
    let layout = Layout {
        bom,
        crlf: body.contains("\r\n"),
        trailing_newline: body.ends_with('\n'),
        indent: detect_indent(body),
    };
    Ok((value, layout))
}

/// Lenient parse of a JSON object that may be cut short: returns every complete top-level
/// member read before the end or the first error. `None` when the text does not start with `{`.
pub fn parse_prefix_object(text: &[u8]) -> Option<Vec<(String, Json)>> {
    let body = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(text);
    let mut p = Parser {
        s: body,
        i: 0,
        depth: 0,
    };
    p.ws();
    if p.peek() != Some(b'{') {
        return None;
    }
    p.i += 1;
    p.depth = 1;
    let mut out = Vec::new();
    loop {
        p.ws();
        match p.peek() {
            Some(b'}') | None => return Some(out),
            Some(b',') => {
                p.i += 1;
                continue;
            }
            _ => {}
        }
        let Ok(key) = p.string() else {
            return Some(out);
        };
        p.ws();
        if p.peek() != Some(b':') {
            return Some(out);
        }
        p.i += 1;
        p.ws();
        let Ok(value) = p.value() else {
            return Some(out);
        };
        out.push((key, value));
    }
}

fn detect_indent(body: &str) -> usize {
    for line in body.lines().skip(1) {
        let n = line.len() - line.trim_start_matches(' ').len();
        if n > 0 && !line.trim().is_empty() {
            return n.min(8);
        }
        if line.starts_with('\t') {
            return 2;
        }
    }
    2
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> ParseError {
        ParseError {
            offset: self.i,
            what,
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn lit(&mut self, word: &'static [u8], v: Json) -> Result<Json, ParseError> {
        if self.s[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("invalid literal"))
        }
    }
    fn value(&mut self) -> Result<Json, ParseError> {
        match self.peek() {
            None => Err(self.err("unexpected end")),
            Some(b'n') => self.lit(b"null", Json::Null),
            Some(b't') => self.lit(b"true", Json::Bool(true)),
            Some(b'f') => self.lit(b"false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }
    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        Ok(())
    }
    fn array(&mut self) -> Result<Json, ParseError> {
        self.enter()?;
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected , or ]")),
            }
        }
    }
    fn object(&mut self) -> Result<Json, ParseError> {
        self.enter()?;
        self.i += 1;
        let mut members = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Json::Obj(members));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected member name"));
            }
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected :"));
            }
            self.i += 1;
            self.ws();
            let value = self.value()?;
            members.push((key, value));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    self.depth -= 1;
                    return Ok(Json::Obj(members));
                }
                _ => return Err(self.err("expected , or }")),
            }
        }
    }
    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            _ => return Err(self.err("invalid number")),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err("invalid number"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.i += 1;
            }
        }
        let lexeme =
            std::str::from_utf8(&self.s[start..self.i]).map_err(|_| self.err("invalid number"))?;
        Ok(Json::Num(lexeme.to_string()))
    }
    fn hex4(&mut self) -> Result<u32, ParseError> {
        let Some(chunk) = self.s.get(self.i..self.i + 4) else {
            return Err(self.err("bad escape"));
        };
        let text = std::str::from_utf8(chunk).map_err(|_| self.err("bad escape"))?;
        let v = u32::from_str_radix(text, 16).map_err(|_| self.err("bad escape"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, ParseError> {
        if self.peek() != Some(b'"') {
            return Err(self.err("expected string"));
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.err("unterminated string"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return String::from_utf8(out).map_err(|_| self.err("invalid UTF-8"));
                }
                b'\\' => {
                    self.i += 1;
                    let Some(e) = self.peek() else {
                        return Err(self.err("bad escape"));
                    };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if (0xD800..0xDC00).contains(&hi) {
                                if self.s.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(self.err("lone surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("lone surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("lone surrogate"));
                            } else {
                                hi
                            };
                            char::from_u32(code).ok_or_else(|| self.err("bad escape"))?
                        }
                        _ => return Err(self.err("bad escape")),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0..=0x1f => return Err(self.err("control character in string")),
                _ => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }
}

/// Serialise with the given layout (pretty-printed, members in order).
pub fn to_string(value: &Json, layout: &Layout) -> String {
    let mut out = String::new();
    if layout.bom {
        out.push('\u{feff}');
    }
    write_value(&mut out, value, 0, layout.indent.max(1));
    if layout.trailing_newline {
        out.push('\n');
    }
    if layout.crlf {
        out = out.replace('\n', "\r\n");
    }
    out
}

fn pad(out: &mut String, level: usize, indent: usize) {
    for _ in 0..level * indent {
        out.push(' ');
    }
}

fn write_value(out: &mut String, value: &Json, level: usize, indent: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(n),
        Json::Str(s) => write_string(out, s),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                pad(out, level + 1, indent);
                write_value(out, item, level + 1, indent);
            }
            out.push('\n');
            pad(out, level, indent);
            out.push(']');
        }
        Json::Obj(members) if members.is_empty() => out.push_str("{}"),
        Json::Obj(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('\n');
                pad(out, level + 1, indent);
                write_string(out, k);
                out.push_str(": ");
                write_value(out, v, level + 1, indent);
            }
            out.push('\n');
            pad(out, level, indent);
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_serde(j: &Json) -> serde_json::Value {
        serde_json::from_str(&to_string(j, &Layout::default())).unwrap()
    }

    #[test]
    fn round_trip_keeps_order_numbers_and_values() {
        let text = "{\n    \"z\": 1.50,\n    \"a\": [\n        1e3,\n        -0,\n        true,\n        null,\n        \"\\u00e9\\ud83d\\ude00\\n\"\n    ],\n    \"m\": {}\n}\n";
        let (v, layout) = parse(text).unwrap();
        assert_eq!(layout.indent, 4);
        let back = to_string(&v, &layout);
        assert_eq!(back, text.replace("\\u00e9\\ud83d\\ude00", "é😀"));
        assert_eq!(
            to_serde(&v),
            serde_json::from_str::<serde_json::Value>(text).unwrap()
        );
        if let Json::Obj(m) = &v {
            assert_eq!(m[0].0, "z");
            assert_eq!(m[0].1, Json::Num("1.50".into()));
        } else {
            panic!("object expected");
        }
    }

    #[test]
    fn rejects_invalid_documents() {
        for bad in [
            "",
            "{",
            "{\"a\":1,}",
            "[1 2]",
            "{\"a\" 1}",
            "01",
            "1.",
            "\"\\x\"",
            "\"a\nb\"",
            "nul",
            "{} x",
            "\"\\ud800\"",
        ] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
        let deep = "[".repeat(300) + &"]".repeat(300);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn layout_bom_and_crlf_survive() {
        let text = "\u{feff}{\r\n  \"a\": 1\r\n}";
        let (v, layout) = parse(text).unwrap();
        assert!(layout.bom && layout.crlf && !layout.trailing_newline);
        assert_eq!(to_string(&v, &layout), text);
    }

    #[test]
    fn prefix_object_keeps_complete_members() {
        let full = br#"{"session_id":"abc","cwd":"C:\\x","tool_input":{"command":"ls"},"tool_response":"very long"#;
        let m = parse_prefix_object(full).unwrap();
        let keys: Vec<_> = m.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["session_id", "cwd", "tool_input"]);
        assert_eq!(m[1].1.as_str(), Some("C:\\x"));
        assert!(parse_prefix_object(b"[1]").is_none());
        assert_eq!(parse_prefix_object(b"{}").unwrap().len(), 0);
    }
}
