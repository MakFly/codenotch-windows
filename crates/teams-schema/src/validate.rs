//! Validation of the contract beyond what the types already close.
//!
//! The same checks run on both ends: the device validates every event it builds, the server
//! validates every event it receives. An error names a reason code and at most a field name;
//! it NEVER carries the offending value (which could be private data), so it can be logged or
//! returned to the device as is. serde's own messages are dropped for the same reason (they
//! quote unknown variants).

use crate::{
    Batch, EventPayload, Heartbeat, QuotaSnapshot, RawEvent, UsageHour, KIND_DEVICE_HEARTBEAT,
    KIND_QUOTA_SNAPSHOT, KIND_USAGE_HOUR, MAX_BATCH_BYTES, MAX_BATCH_EVENTS, SCHEMA_VERSION,
};
use serde_json::Value;

/// Keys refused at any depth of a payload, on the device and on the server (compared without
/// case). None of them is a field of the contract; this is a second line of defence behind the
/// closed types.
pub const FORBIDDEN_KEYS: &[&str] = &[
    "prompt",
    "content",
    "message",
    "messages",
    "text",
    "response",
    "transcript",
    "detail",
    "title",
    "note",
    "error",
    "label",
    "group",
    "cwd",
    "path",
    "profilePath",
    "project",
    "repo",
    "file",
    "identity",
    "email",
    "login",
    "user",
    "username",
    "hostname",
    "name",
    "token",
    "authorization",
    "secret",
    "apiKey",
    "cookie",
    "sessionId",
    "hook",
    "body",
];

/// uuid v4, lower case (`device_id`, `event_id`).
pub const UUID_V4_PATTERN: &str =
    "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";
/// Account reference: 16 lower-case hex characters.
pub const ACCOUNT_REF_PATTERN: &str = "^[0-9a-f]{16}$";
/// App version: `0.1.0`, `1.2.3-beta.1`.
pub const APP_VERSION_PATTERN: &str = "^[0-9A-Za-z.+-]{1,32}$";

/// Why a batch or an event is refused. Carries no value from the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    /// `schemaVersion` is not one this build reads (a server answers 426).
    UnsupportedVersion,
    /// Not JSON, wrong shape, unknown field, or a value outside a closed set.
    Malformed,
    /// `kind` is not one of [`crate::KINDS`].
    UnknownKind,
    /// A key of [`FORBIDDEN_KEYS`] appears somewhere.
    ForbiddenKey,
    /// Too many events or bytes.
    TooLarge,
    /// A field breaks a rule of the contract.
    InvalidValue(&'static str),
}

impl ValidationError {
    /// Stable reason code (`invalid_value`, `unknown_kind` …).
    pub fn reason(&self) -> &'static str {
        match self {
            ValidationError::UnsupportedVersion => "unsupported_version",
            ValidationError::Malformed => "malformed",
            ValidationError::UnknownKind => "unknown_kind",
            ValidationError::ForbiddenKey => "forbidden_key",
            ValidationError::TooLarge => "too_large",
            ValidationError::InvalidValue(_) => "invalid_value",
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::InvalidValue(field) => write!(f, "invalid_value: {field}"),
            other => f.write_str(other.reason()),
        }
    }
}

impl std::error::Error for ValidationError {}

type Result<T> = std::result::Result<T, ValidationError>;

fn check(ok: bool, field: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ValidationError::InvalidValue(field))
    }
}

fn is_hex_lower(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `s` is a lower-case uuid v4 (see [`UUID_V4_PATTERN`]).
pub fn is_uuid_v4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && is_hex_lower(p))
        && parts[2].starts_with('4')
        && parts[3].starts_with(['8', '9', 'a', 'b'])
}

/// `s` is an account reference (see [`ACCOUNT_REF_PATTERN`]).
pub fn is_account_ref(s: &str) -> bool {
    s.len() == 16 && is_hex_lower(s)
}

fn is_app_version(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// True when a key of [`FORBIDDEN_KEYS`] appears at any depth of `value`.
pub fn contains_forbidden_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| {
            FORBIDDEN_KEYS.iter().any(|f| f.eq_ignore_ascii_case(k)) || contains_forbidden_key(v)
        }),
        Value::Array(items) => items.iter().any(contains_forbidden_key),
        _ => false,
    }
}

/// `schemaVersion` of a raw batch, read before anything else so a newer version is reported as
/// such rather than as malformed.
pub fn peek_schema_version(value: &Value) -> Option<u64> {
    value.get("schemaVersion")?.as_u64()
}

impl Batch {
    /// Parse and validate the envelope of an uploaded batch: size, JSON, `schemaVersion`
    /// (before the shape, so a newer version gives [`ValidationError::UnsupportedVersion`]),
    /// forbidden keys, closed shape, then [`Batch::validate`]. Events are NOT validated here,
    /// so one bad event can be refused alone: call [`RawEvent::validate`] on each.
    pub fn parse(bytes: &[u8]) -> Result<Batch> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(ValidationError::TooLarge);
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|_| ValidationError::Malformed)?;
        match peek_schema_version(&value) {
            Some(v) if v == u64::from(SCHEMA_VERSION) => {}
            Some(_) => return Err(ValidationError::UnsupportedVersion),
            None => return Err(ValidationError::Malformed),
        }
        let envelope_keys = value.as_object().ok_or(ValidationError::Malformed)?.keys();
        for key in envelope_keys {
            if FORBIDDEN_KEYS.iter().any(|f| f.eq_ignore_ascii_case(key)) {
                return Err(ValidationError::ForbiddenKey);
            }
        }
        let batch: Batch = serde_json::from_value(value).map_err(|_| ValidationError::Malformed)?;
        batch.validate()?;
        Ok(batch)
    }

    /// Envelope rules: version, times, ids, event count and each event's envelope. Payloads
    /// are checked by [`RawEvent::validate`].
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedVersion);
        }
        check(self.sent_at > 0, "sentAt")?;
        check(is_uuid_v4(&self.device_id), "deviceId")?;
        check(is_app_version(&self.app_version), "appVersion")?;
        check(self.consent_version >= 1, "consentVersion")?;
        if self.events.len() > MAX_BATCH_EVENTS {
            return Err(ValidationError::TooLarge);
        }
        check(!self.events.is_empty(), "events")?;
        self.events.iter().try_for_each(RawEvent::validate_envelope)
    }
}

impl RawEvent {
    /// Envelope rules only: event id, time, known kind.
    pub fn validate_envelope(&self) -> Result<()> {
        check(is_uuid_v4(&self.event_id), "eventId")?;
        check(self.occurred_at > 0, "occurredAt")?;
        if !crate::KINDS.contains(&self.kind.as_str()) {
            return Err(ValidationError::UnknownKind);
        }
        Ok(())
    }

    /// Full validation: envelope, forbidden keys, then the payload parsed as the type of
    /// `kind` (closed shape and closed sets) and its rules.
    pub fn validate(&self) -> Result<EventPayload> {
        self.validate_envelope()?;
        self.payload()
    }

    /// Dispatch on `kind`, then parse and validate the payload.
    pub fn payload(&self) -> Result<EventPayload> {
        if contains_forbidden_key(&self.payload) {
            return Err(ValidationError::ForbiddenKey);
        }
        let parsed = match self.kind.as_str() {
            KIND_QUOTA_SNAPSHOT => {
                parse::<QuotaSnapshot>(&self.payload).map(EventPayload::QuotaSnapshot)
            }
            KIND_USAGE_HOUR => parse::<UsageHour>(&self.payload).map(EventPayload::UsageHour),
            KIND_DEVICE_HEARTBEAT => parse::<Heartbeat>(&self.payload).map(EventPayload::Heartbeat),
            _ => Err(ValidationError::UnknownKind),
        }?;
        parsed.validate()?;
        Ok(parsed)
    }
}

fn parse<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    T::deserialize(value).map_err(|_| ValidationError::Malformed)
}

impl EventPayload {
    /// Rules of the payload's type.
    pub fn validate(&self) -> Result<()> {
        match self {
            EventPayload::QuotaSnapshot(p) => p.validate(),
            EventPayload::UsageHour(p) => p.validate(),
            EventPayload::Heartbeat(_) => Ok(()),
        }
    }
}

impl QuotaSnapshot {
    /// Account reference, plan and windows of the provider, percentages 0–100, positive times
    /// and lengths, each window id and each role at most once.
    pub fn validate(&self) -> Result<()> {
        check(is_account_ref(&self.account_ref), "accountRef")?;
        check(self.plan.allowed_for(self.provider), "plan")?;
        check(self.observed_at > 0, "observedAt")?;
        check(self.windows.len() <= 8, "windows")?;
        for (i, w) in self.windows.iter().enumerate() {
            check(w.id.allowed_for(self.provider), "windows.id")?;
            check(self.windows[..i].iter().all(|o| o.id != w.id), "windows.id")?;
            check(
                w.role.is_none() || self.windows[..i].iter().all(|o| o.role != w.role),
                "windows.role",
            )?;
            check(
                w.used_percent
                    .is_none_or(|p| p.is_finite() && (0.0..=100.0).contains(&p)),
                "windows.usedPercent",
            )?;
            check(w.resets_at.is_none_or(|t| t > 0), "windows.resetsAt")?;
            check(
                w.window_seconds.is_none_or(|s| s > 0),
                "windows.windowSeconds",
            )?;
        }
        Ok(())
    }
}

impl UsageHour {
    /// Account reference, model of the provider, aligned UTC hour, revision ≥ 1,
    /// `cacheWrite1h ≤ cacheWrite`.
    pub fn validate(&self) -> Result<()> {
        check(is_account_ref(&self.account_ref), "accountRef")?;
        check(self.model.allowed_for(self.provider), "model")?;
        check(
            self.hour_start > 0 && self.hour_start % 3600 == 0,
            "hourStart",
        )?;
        check(self.rev >= 1, "rev")?;
        check(
            self.tokens.cache_write_1h <= self.tokens.cache_write,
            "tokens.cacheWrite1h",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_and_account_ref_checks() {
        assert!(is_uuid_v4("6f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11"));
        assert!(
            !is_uuid_v4("6F1C2A9E-0B7D-4A51-9C7E-2F0D7B1A4E11"),
            "upper case"
        );
        assert!(
            !is_uuid_v4("6f1c2a9e-0b7d-1a51-9c7e-2f0d7b1a4e11"),
            "version 1"
        );
        assert!(
            !is_uuid_v4("6f1c2a9e-0b7d-4a51-7c7e-2f0d7b1a4e11"),
            "variant"
        );
        assert!(!is_uuid_v4("6f1c2a9e0b7d4a519c7e2f0d7b1a4e11"));
        assert!(is_account_ref("a3f9c07e12b45d68"));
        assert!(!is_account_ref("a3f9c07e12b45d6"));
        assert!(!is_account_ref("a3f9c07e12b45d6g"));
        assert!(is_app_version("0.1.0"));
        assert!(!is_app_version("0.1.0 (C:\\Users\\x)"));
    }

    #[test]
    fn forbidden_keys_any_depth_any_case() {
        let v: Value = serde_json::json!({"a": [{"b": {"PROMPT": 1}}]});
        assert!(contains_forbidden_key(&v));
        let v: Value = serde_json::json!({"a": [{"b": {"apikey": 1}}]});
        assert!(contains_forbidden_key(&v));
        let v: Value = serde_json::json!({"accountRef": "x", "windows": [{"id": "session"}]});
        assert!(!contains_forbidden_key(&v));
    }

    #[test]
    fn error_text_is_a_code_and_a_field() {
        assert_eq!(
            ValidationError::InvalidValue("plan").to_string(),
            "invalid_value: plan"
        );
        assert_eq!(
            ValidationError::UnsupportedVersion.reason(),
            "unsupported_version"
        );
    }
}
