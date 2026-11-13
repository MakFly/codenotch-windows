//! Receiving side: [`Batch::parse`] reads an uploaded batch and gives one outcome per event.
//!
//! Order (binding):
//! 1. size, JSON, `schemaVersion` (before the shape, so a newer version is reported as such);
//! 2. the envelope (closed shape, forbidden keys, field rules, 1 to [`MAX_BATCH_EVENTS`]
//!    events). Only these steps fail the whole batch;
//! 3. each event on its own: `kind` is read first; an unknown kind is IGNORED (a kind added
//!    after this build must not break it), then a known kind is parsed as its closed type.
//!    A bad event is REJECTED alone; the other events of the batch are kept.

use crate::validate::{is_uuid_v4, peek_schema_version, Result, ValidationError};
use crate::{
    Batch, EventPayload, RawEvent, FORBIDDEN_KEYS, KINDS, MAX_BATCH_BYTES, MAX_BATCH_EVENTS,
    SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A received batch: the checked envelope and one [`ParsedEvent`] per event, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBatch {
    pub schema_version: u32,
    pub sent_at: i64,
    pub device_id: String,
    pub app_version: String,
    pub consent_version: u32,
    pub data_epoch: u32,
    pub events: Vec<ParsedEvent>,
}

/// One event of a received batch.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEvent {
    /// Position in `events`.
    pub index: usize,
    /// The event id, only when it is a valid uuid v4 (never an arbitrary string).
    pub event_id: Option<String>,
    pub outcome: EventOutcome,
}

/// What the receiver does with one event.
#[derive(Debug, Clone, PartialEq)]
pub enum EventOutcome {
    /// Known kind, valid envelope and payload.
    Accepted {
        event: RawEvent,
        payload: EventPayload,
    },
    /// `kind` is not one this build knows: skipped, not an error.
    Ignored,
    /// The event breaks the contract; only this event is refused.
    Rejected(ValidationError),
}

/// Wire shape of one event's outcome, as a server returns it to the device:
/// `{"index":0,"eventId":"…","status":"accepted"}`,
/// `{"index":1,"eventId":"…","status":"ignored","reason":"unknown_kind"}`,
/// `{"index":2,"status":"rejected","reason":"invalid_value","field":"plan"}`.
/// Carries no value from the input except a checked uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventStatus {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// `accepted`, `ignored` or `rejected`.
    pub status: String,
    /// Reason code ([`ValidationError::reason`]) for `ignored` and `rejected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Field name for `invalid_value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl ParsedEvent {
    /// The wire shape of this outcome.
    pub fn status(&self) -> EventStatus {
        let (status, error) = match &self.outcome {
            EventOutcome::Accepted { .. } => ("accepted", None),
            EventOutcome::Ignored => ("ignored", Some(ValidationError::UnknownKind)),
            EventOutcome::Rejected(e) => ("rejected", Some(*e)),
        };
        EventStatus {
            index: self.index,
            event_id: self.event_id.clone(),
            status: status.to_string(),
            reason: error.map(|e| e.reason().to_string()),
            field: match error {
                Some(ValidationError::InvalidValue(field)) => Some(field.to_string()),
                _ => None,
            },
        }
    }

    /// The typed payload when the event was accepted.
    pub fn accepted(&self) -> Option<&EventPayload> {
        match &self.outcome {
            EventOutcome::Accepted { payload, .. } => Some(payload),
            _ => None,
        }
    }
}

impl ParsedBatch {
    /// The accepted events with their typed payloads.
    pub fn accepted(&self) -> impl Iterator<Item = (&RawEvent, &EventPayload)> {
        self.events.iter().filter_map(|e| match &e.outcome {
            EventOutcome::Accepted { event, payload } => Some((event, payload)),
            _ => None,
        })
    }
}

/// The envelope as received: events stay raw JSON until each one is dispatched on `kind`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireBatch {
    schema_version: u32,
    sent_at: i64,
    device_id: String,
    app_version: String,
    consent_version: u32,
    data_epoch: u32,
    events: Vec<Value>,
}

impl Batch {
    /// Parse an uploaded batch (see the module doc for the order). `Err` only for the size,
    /// the JSON, the version, the envelope or the event count; every event gets its own
    /// outcome otherwise.
    pub fn parse(bytes: &[u8]) -> Result<ParsedBatch> {
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
        let wire: WireBatch =
            serde_json::from_value(value).map_err(|_| ValidationError::Malformed)?;
        // The envelope rules of `Batch::validate`, without the per-event part.
        let header = Batch {
            schema_version: wire.schema_version,
            sent_at: wire.sent_at,
            device_id: wire.device_id,
            app_version: wire.app_version,
            consent_version: wire.consent_version,
            data_epoch: wire.data_epoch,
            events: Vec::new(),
        };
        header.validate_header()?;
        if wire.events.len() > MAX_BATCH_EVENTS {
            return Err(ValidationError::TooLarge);
        }
        if wire.events.is_empty() {
            return Err(ValidationError::InvalidValue("events"));
        }
        let events = wire
            .events
            .into_iter()
            .enumerate()
            .map(|(index, raw)| parse_event(index, raw))
            .collect();
        Ok(ParsedBatch {
            schema_version: header.schema_version,
            sent_at: header.sent_at,
            device_id: header.device_id,
            app_version: header.app_version,
            consent_version: header.consent_version,
            data_epoch: header.data_epoch,
            events,
        })
    }
}

/// One event: dispatch on `kind`, then the closed type of that kind.
fn parse_event(index: usize, raw: Value) -> ParsedEvent {
    let event_id = raw
        .get("eventId")
        .and_then(Value::as_str)
        .filter(|id| is_uuid_v4(id))
        .map(str::to_string);
    let outcome = match raw.get("kind").and_then(Value::as_str) {
        None => EventOutcome::Rejected(ValidationError::Malformed),
        Some(kind) if !KINDS.contains(&kind) => EventOutcome::Ignored,
        Some(_) => match serde_json::from_value::<RawEvent>(raw) {
            Err(_) => EventOutcome::Rejected(ValidationError::Malformed),
            Ok(event) => match event.validate() {
                Ok(payload) => EventOutcome::Accepted { event, payload },
                Err(e) => EventOutcome::Rejected(e),
            },
        },
    };
    ParsedEvent {
        index,
        event_id,
        outcome,
    }
}
