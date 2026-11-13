//! JSON Schema of the contract (feature `schema`).
//!
//! [`generate`] builds the schema of a [`Batch`] (draft 2020-12). The result is committed as
//! `schema/usage-events.v1.json`; a test fails when the types and the file drift apart. To
//! rewrite the file after a deliberate change (only allowed before version 1 is published):
//!
//! ```text
//! set CODENOTCH_WRITE_SCHEMA=1 && cargo test -p codenotch-teams-schema schema_file
//! ```

use crate::{Batch, Heartbeat, QuotaSnapshot, UsageHour, KINDS};
use schemars::{json_schema, Schema, SchemaGenerator};

/// File name of the committed schema of this version.
pub const FILE_NAME: &str = "usage-events.v1.json";

/// `$id` of the schema of this version.
pub const SCHEMA_ID: &str =
    "https://raw.githubusercontent.com/MakFly/codenotch-windows/main/crates/teams-schema/schema/usage-events.v1.json";

/// The schema of a [`Batch`] of version 1 as a JSON value.
pub fn generate() -> serde_json::Value {
    let mut schema = schemars::generate::SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<Batch>();
    schema.insert("$id".into(), SCHEMA_ID.into());
    schema.insert("title".into(), "CodeNotch Teams usage events v1".into());
    schema.to_value()
}

/// The schema as pretty JSON text with a final newline (the committed file's format).
pub fn generate_pretty() -> String {
    let mut text = serde_json::to_string_pretty(&generate()).unwrap_or_default();
    text.push('\n');
    text
}

/// `RawEvent.kind`: one of [`KINDS`].
pub(crate) fn kind_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "enum": KINDS })
}

/// `RawEvent.payload`: one of the payload types (which one is fixed by `kind`).
pub(crate) fn payload_schema(generator: &mut SchemaGenerator) -> Schema {
    let quota = generator.subschema_for::<QuotaSnapshot>();
    let usage = generator.subschema_for::<UsageHour>();
    let heartbeat = generator.subschema_for::<Heartbeat>();
    json_schema!({ "oneOf": [quota, usage, heartbeat] })
}
