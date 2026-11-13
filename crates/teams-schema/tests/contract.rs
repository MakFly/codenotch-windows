//! Contract tests: closed shapes at every level, closed value sets, versioning, examples and
//! the committed JSON Schema.

use codenotch_teams_schema::validate::{contains_forbidden_key, is_uuid_v4};
use codenotch_teams_schema::*;
use serde_json::{json, Value};
use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example(name: &str) -> Value {
    let path = crate_dir().join("examples").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    serde_json::from_str(&text).unwrap()
}

fn event(value: Value) -> RawEvent {
    serde_json::from_value(value).expect("event envelope")
}

fn batch_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

const SENTINELS: &[&str] = &[
    "sk-secret123",
    "C_Users_Kev_client",
    "please refactor the parser module",
];

// ---------------------------------------------------------------------------------------------
// Examples and round trips
// ---------------------------------------------------------------------------------------------

#[test]
fn every_example_is_valid() {
    for name in [
        "quota-snapshot.json",
        "quota-snapshot-tombstone.json",
        "usage-hour.json",
        "device-heartbeat.json",
    ] {
        let e = event(example(name));
        e.validate().unwrap_or_else(|err| panic!("{name}: {err}"));
    }
    let parsed = Batch::parse(&batch_bytes(&example("batch.json"))).unwrap();
    assert_eq!(parsed.events.len(), 3);
    let kinds: Vec<_> = parsed.accepted().map(|(_, p)| p.kind()).collect();
    assert_eq!(kinds, KINDS);
    // Round trip: the accepted events, sent again as a batch, parse to the same result.
    let batch = Batch {
        schema_version: parsed.schema_version,
        sent_at: parsed.sent_at,
        device_id: parsed.device_id.clone(),
        app_version: parsed.app_version.clone(),
        consent_version: parsed.consent_version,
        data_epoch: parsed.data_epoch,
        events: parsed.accepted().map(|(e, _)| e.clone()).collect(),
    };
    batch.validate().unwrap();
    let again = Batch::parse(&serde_json::to_vec(&batch).unwrap()).unwrap();
    assert_eq!(again, parsed);
}

#[test]
fn typed_payloads_round_trip_with_the_wire_names() {
    let usage = UsageHour {
        account_ref: "a3f9c07e12b45d68".into(),
        provider: Provider::Codex,
        model: ModelId::Gpt5_1CodexMax,
        hour_start: 1_794_553_200,
        rev: 1,
        tokens: Tokens {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write_1h: 4,
        },
    };
    let payload = EventPayload::UsageHour(usage.clone());
    let e = RawEvent::new(
        "5c0e7a13-2d44-4b9e-9a61-70c1f2b8d3e9",
        1_794_556_700,
        &payload,
    )
    .unwrap();
    assert_eq!(e.kind, KIND_USAGE_HOUR);
    assert_eq!(e.payload["model"], "gpt-5.1-codex-max");
    assert_eq!(e.payload["tokens"]["cacheWrite1h"], 4);
    assert_eq!(e.payload["tokens"]["cacheRead"], 3);
    assert_eq!(e.validate().unwrap(), payload);

    let snap = QuotaSnapshot {
        account_ref: "a3f9c07e12b45d68".into(),
        provider: Provider::Claude,
        plan: PlanCode::Max5x,
        seq: 1,
        observed_at: 1_794_556_735,
        windows: vec![QuotaWindow {
            id: WindowId::SevenDayOpus,
            kind: WindowKind::Weekly,
            role: None,
            used_percent: None,
            resets_at: None,
            window_seconds: None,
        }],
    };
    let e = RawEvent::new(
        "0d4b6c1e-5a0f-4e0b-8f51-1c2f9d3a7b60",
        1_794_556_740,
        &EventPayload::QuotaSnapshot(snap),
    )
    .unwrap();
    assert_eq!(e.payload["plan"], "max_5x");
    assert_eq!(e.payload["windows"][0]["id"], "seven_day_opus");
    assert!(
        e.payload["windows"][0]["usedPercent"].is_null(),
        "unknown stays null, never 0"
    );
}

// ---------------------------------------------------------------------------------------------
// Closed shapes
// ---------------------------------------------------------------------------------------------

#[test]
fn unknown_fields_are_refused_at_every_level() {
    let base = example("batch.json");
    let mut b = base.clone();
    b["extra"] = json!(1);
    assert_eq!(
        Batch::parse(&batch_bytes(&b)),
        Err(ValidationError::Malformed),
        "envelope level"
    );
    // Event envelope level: that event alone is refused.
    let mut b = base.clone();
    b["events"][0]["extra"] = json!(1);
    let parsed = Batch::parse(&batch_bytes(&b)).unwrap();
    assert_eq!(
        parsed.events[0].outcome,
        EventOutcome::Rejected(ValidationError::Malformed)
    );
    assert_eq!(parsed.accepted().count(), 2);
    // Payload, window and tokens levels: the event alone is refused.
    let mut snap = example("quota-snapshot.json");
    snap["payload"]["extra"] = json!(1);
    assert_eq!(event(snap).validate(), Err(ValidationError::Malformed));
    let mut snap = example("quota-snapshot.json");
    snap["payload"]["windows"][0]["extra"] = json!(1);
    assert_eq!(event(snap).validate(), Err(ValidationError::Malformed));
    let mut usage = example("usage-hour.json");
    usage["payload"]["tokens"]["reasoning"] = json!(1);
    assert_eq!(event(usage).validate(), Err(ValidationError::Malformed));
    let mut beat = example("device-heartbeat.json");
    beat["payload"]["uptime"] = json!(1);
    assert_eq!(event(beat).validate(), Err(ValidationError::Malformed));
}

#[test]
fn forbidden_keys_are_refused_with_their_own_reason() {
    let mut snap = example("quota-snapshot.json");
    snap["payload"]["windows"][1]["label"] = json!("Current session");
    assert_eq!(event(snap).validate(), Err(ValidationError::ForbiddenKey));
    let mut usage = example("usage-hour.json");
    usage["payload"]["tokens"]["Project"] = json!("x");
    assert_eq!(event(usage).validate(), Err(ValidationError::ForbiddenKey));
    let mut b = example("batch.json");
    b["hostname"] = json!("PC");
    assert_eq!(
        Batch::parse(&batch_bytes(&b)),
        Err(ValidationError::ForbiddenKey)
    );
    // No field of the contract is a forbidden key.
    assert!(!contains_forbidden_key(&example("batch.json")));
}

#[test]
fn kind_is_dispatched_before_the_payload_is_parsed() {
    let mut e = example("device-heartbeat.json");
    e["kind"] = json!("session.prompt");
    assert_eq!(event(e).validate(), Err(ValidationError::UnknownKind));
    // A payload of another kind does not parse as this kind.
    let mut e = example("device-heartbeat.json");
    e["kind"] = json!(KIND_USAGE_HOUR);
    assert_eq!(event(e).validate(), Err(ValidationError::Malformed));
}

// ---------------------------------------------------------------------------------------------
// Closed value sets (privacy boundary)
// ---------------------------------------------------------------------------------------------

#[test]
fn free_text_in_an_allowed_field_never_parses_and_is_never_echoed() {
    for sentinel in SENTINELS {
        let mut cases = Vec::new();
        let mut a = example("quota-snapshot.json");
        a["payload"]["windows"][0]["id"] = json!(sentinel);
        cases.push(a);
        let mut b = example("quota-snapshot.json");
        b["payload"]["plan"] = json!(sentinel);
        cases.push(b);
        let mut c = example("usage-hour.json");
        c["payload"]["model"] = json!(sentinel);
        cases.push(c);
        let mut d = example("usage-hour.json");
        d["payload"]["provider"] = json!(sentinel);
        cases.push(d);
        let mut e = example("usage-hour.json");
        e["payload"]["accountRef"] = json!(sentinel);
        cases.push(e);
        for case in cases {
            let err = event(case).validate().unwrap_err();
            let shown = format!("{err} {err:?} {}", err.reason());
            assert!(!shown.contains(sentinel), "error echoes the value: {shown}");
        }
        // Translation into the closed sets.
        assert_eq!(ModelId::parse(Provider::Claude, sentinel), ModelId::Unknown);
        assert_eq!(
            PlanCode::parse(Provider::Codex, sentinel),
            PlanCode::Unknown
        );
        assert_eq!(WindowId::parse(Provider::Claude, sentinel), None);
        assert_eq!(Provider::parse(sentinel), None);
    }
    // A custom endpoint's label turned into an id is not a window of any provider.
    assert_eq!(WindowId::parse(Provider::Claude, "my_proxy_budget"), None);
    assert_eq!(WindowId::parse(Provider::Codex, "spend"), None);
    assert_eq!(WindowId::parse(Provider::Claude, "extra_usage"), None);
}

#[test]
fn closed_sets_translate_provider_values() {
    assert_eq!(
        PlanCode::parse(Provider::Claude, "Max 20x"),
        PlanCode::Max20x
    );
    assert_eq!(PlanCode::parse(Provider::Claude, "max_5x"), PlanCode::Max5x);
    assert_eq!(PlanCode::parse(Provider::Claude, " Pro "), PlanCode::Pro);
    assert_eq!(
        PlanCode::parse(Provider::Claude, "Plus"),
        PlanCode::Unknown,
        "Codex plan"
    );
    assert_eq!(PlanCode::parse(Provider::Codex, "plus"), PlanCode::Plus);
    assert_eq!(
        PlanCode::parse(Provider::Codex, "Max"),
        PlanCode::Unknown,
        "Claude plan"
    );
    assert_eq!(
        PlanCode::parse(Provider::Codex, "unknown"),
        PlanCode::Unknown
    );
    assert_eq!(
        WindowId::parse(Provider::Claude, "seven_day"),
        Some(WindowId::SevenDay)
    );
    assert_eq!(WindowId::parse(Provider::Codex, "seven_day"), None);
    assert_eq!(
        WindowId::parse(Provider::Codex, "primary"),
        Some(WindowId::Primary)
    );
    assert_eq!(
        ModelId::parse(Provider::Claude, "claude-opus-5-5"),
        ModelId::ClaudeOpus5_5
    );
    assert_eq!(
        ModelId::parse(Provider::Claude, "claude-opus-5-5-fast"),
        ModelId::Unknown
    );
    assert_eq!(
        ModelId::parse(Provider::Codex, "claude-opus-5-5"),
        ModelId::Unknown
    );
    assert_eq!(ModelId::parse(Provider::Codex, "gpt-5.2"), ModelId::Gpt5_2);
    assert_eq!(
        ModelId::parse(Provider::Codex, "GPT-5.2"),
        ModelId::Unknown,
        "exact"
    );

    // Every table entry serialises to its id and back; ids are unique.
    let mut ids = std::collections::HashSet::new();
    for (model, id, provider) in ModelId::TABLE {
        assert!(ids.insert(*id), "duplicate {id}");
        assert_eq!(serde_json::to_value(model).unwrap(), json!(id));
        assert_eq!(model.as_str(), *id);
        assert_eq!(model.provider(), Some(*provider));
    }
    assert_eq!(
        serde_json::to_value(ModelId::Unknown).unwrap(),
        json!("unknown")
    );
    for plan in PlanCode::ALL {
        assert_eq!(serde_json::to_value(plan).unwrap(), json!(plan.as_str()));
    }
    for window in WindowId::ALL {
        assert_eq!(
            serde_json::to_value(window).unwrap(),
            json!(window.as_str())
        );
    }
}

#[test]
fn values_must_belong_to_the_provider() {
    let mut a = example("quota-snapshot.json");
    a["payload"]["windows"][0]["id"] = json!("primary");
    assert_eq!(
        event(a).validate(),
        Err(ValidationError::InvalidValue("windows.id"))
    );
    let mut b = example("quota-snapshot.json");
    b["payload"]["plan"] = json!("plus");
    assert_eq!(
        event(b).validate(),
        Err(ValidationError::InvalidValue("plan"))
    );
    let mut c = example("usage-hour.json");
    c["payload"]["model"] = json!("gpt-5");
    assert_eq!(
        event(c).validate(),
        Err(ValidationError::InvalidValue("model"))
    );
    let mut d = example("usage-hour.json");
    d["payload"]["model"] = json!("unknown");
    assert!(event(d).validate().is_ok());
}

// ---------------------------------------------------------------------------------------------
// Value rules
// ---------------------------------------------------------------------------------------------

#[test]
fn value_rules() {
    let bad = |file: &str, mutate: &dyn Fn(&mut Value), field: &'static str| {
        let mut e = example(file);
        mutate(&mut e);
        assert_eq!(
            event(e).validate(),
            Err(ValidationError::InvalidValue(field)),
            "{field}"
        );
    };
    let q = "quota-snapshot.json";
    let u = "usage-hour.json";
    bad(
        q,
        &|e| e["payload"]["windows"][0]["usedPercent"] = json!(100.5),
        "windows.usedPercent",
    );
    bad(
        q,
        &|e| e["payload"]["windows"][0]["usedPercent"] = json!(-1),
        "windows.usedPercent",
    );
    bad(
        q,
        &|e| e["payload"]["windows"][0]["windowSeconds"] = json!(0),
        "windows.windowSeconds",
    );
    bad(
        q,
        &|e| e["payload"]["windows"][0]["resetsAt"] = json!(-5),
        "windows.resetsAt",
    );
    bad(
        q,
        &|e| e["payload"]["windows"][1]["id"] = json!("session"),
        "windows.id",
    );
    bad(
        q,
        &|e| e["payload"]["windows"][1]["role"] = json!("headline"),
        "windows.role",
    );
    bad(q, &|e| e["payload"]["observedAt"] = json!(0), "observedAt");
    bad(
        u,
        &|e| e["payload"]["hourStart"] = json!(1_794_553_201),
        "hourStart",
    );
    bad(u, &|e| e["payload"]["rev"] = json!(0), "rev");
    bad(
        u,
        &|e| e["payload"]["tokens"]["cacheWrite1h"] = json!(36_501),
        "tokens.cacheWrite1h",
    );
    bad(
        u,
        &|e| e["payload"]["accountRef"] = json!("A3F9C07E12B45D68"),
        "accountRef",
    );
    bad(u, &|e| e["eventId"] = json!("not-a-uuid"), "eventId");
    bad(u, &|e| e["occurredAt"] = json!(0), "occurredAt");

    // Two windows without a role are fine; a NaN percent cannot be built.
    let mut ok = example(q);
    ok["payload"]["windows"][0]["role"] = Value::Null;
    ok["payload"]["windows"][1]["role"] = Value::Null;
    assert!(event(ok).validate().is_ok());
    let nan = QuotaSnapshot {
        account_ref: "a3f9c07e12b45d68".into(),
        provider: Provider::Codex,
        plan: PlanCode::Plus,
        seq: 2,
        observed_at: 1,
        windows: vec![QuotaWindow {
            id: WindowId::Primary,
            kind: WindowKind::Session,
            role: Some(WindowRole::Headline),
            used_percent: Some(f64::NAN),
            resets_at: None,
            window_seconds: Some(18_000),
        }],
    };
    assert_eq!(
        RawEvent::new(
            "0d4b6c1e-5a0f-4e0b-8f51-1c2f9d3a7b60",
            1,
            &EventPayload::QuotaSnapshot(nan)
        ),
        Err(ValidationError::InvalidValue("windows.usedPercent"))
    );
}

#[test]
fn envelope_rules_and_limits() {
    let base = example("batch.json");
    let refused = |mutate: &dyn Fn(&mut Value)| {
        let mut b = base.clone();
        mutate(&mut b);
        Batch::parse(&batch_bytes(&b)).unwrap_err()
    };
    assert_eq!(
        refused(&|b| b["deviceId"] = json!("PC-OF-KEV")),
        ValidationError::InvalidValue("deviceId")
    );
    assert_eq!(
        refused(&|b| b["appVersion"] = json!("0.1.0 C:\\Users\\kev")),
        ValidationError::InvalidValue("appVersion")
    );
    assert_eq!(
        refused(&|b| b["consentVersion"] = json!(0)),
        ValidationError::InvalidValue("consentVersion")
    );
    assert_eq!(
        refused(&|b| b["events"] = json!([])),
        ValidationError::InvalidValue("events")
    );
    let beat = base["events"][2].clone();
    assert_eq!(
        refused(&|b| b["events"] = Value::Array(vec![beat.clone(); MAX_BATCH_EVENTS + 1])),
        ValidationError::TooLarge
    );
    assert!(is_uuid_v4(base["deviceId"].as_str().unwrap()));
    let huge = vec![b' '; MAX_BATCH_BYTES + 1];
    assert_eq!(Batch::parse(&huge), Err(ValidationError::TooLarge));
    assert_eq!(Batch::parse(b"{ nope"), Err(ValidationError::Malformed));
    assert_eq!(Batch::parse(b"[1]"), Err(ValidationError::Malformed));
}

// ---------------------------------------------------------------------------------------------
// Per-event outcomes
// ---------------------------------------------------------------------------------------------

fn statuses(parsed: &ParsedBatch) -> Vec<Value> {
    parsed
        .events
        .iter()
        .map(|e| serde_json::to_value(e.status()).unwrap())
        .collect()
}

#[test]
fn unknown_kind_is_ignored_and_the_other_events_are_kept() {
    // A kind added after this build, with a payload this build cannot read.
    let mut b = example("batch.json");
    let id = "e1d2c3b4-a5f6-4a7b-8c9d-0e1f2a3b4c5d";
    b["events"].as_array_mut().unwrap().insert(
        1,
        json!({"eventId": id, "occurredAt": 1_794_556_800_i64, "kind": "device.inventory",
               "payload": {"anything": [1, 2], "newField": "x"}, "newEnvelopeField": 1}),
    );
    let parsed = Batch::parse(&batch_bytes(&b)).unwrap();
    assert_eq!(parsed.events.len(), 4);
    assert_eq!(parsed.events[1].outcome, EventOutcome::Ignored);
    let kinds: Vec<_> = parsed.accepted().map(|(_, p)| p.kind()).collect();
    assert_eq!(kinds, KINDS);
    assert_eq!(
        statuses(&parsed)[1],
        json!({"index": 1, "eventId": id, "status": "ignored", "reason": "unknown_kind"})
    );
    assert_eq!(
        statuses(&parsed)[0],
        json!({"index": 0, "eventId": "0d4b6c1e-5a0f-4e0b-8f51-1c2f9d3a7b60", "status": "accepted"})
    );
    // Only unknown kinds: still a batch, every event ignored, the kind never echoed.
    let mut only = example("batch.json");
    for e in only["events"].as_array_mut().unwrap() {
        e["kind"] = json!("session.prompt");
    }
    let parsed = Batch::parse(&batch_bytes(&only)).unwrap();
    assert!(parsed
        .events
        .iter()
        .all(|e| e.outcome == EventOutcome::Ignored));
    assert!(!serde_json::to_string(&statuses(&parsed))
        .unwrap()
        .contains("session.prompt"));
}

#[test]
fn malformed_event_is_rejected_alone() {
    let mut b = example("batch.json");
    let events = b["events"].as_array_mut().unwrap();
    // Wrong type, no kind, a value outside its closed set, a forbidden key, a broken rule.
    events.push(json!(42));
    events.push(json!({"eventId": "not-a-uuid", "occurredAt": 1, "payload": {}}));
    let mut plan = events[0].clone();
    plan["payload"]["plan"] = json!(SENTINELS[2]);
    events.push(plan);
    let mut key = events[2].clone();
    key["payload"]["prompt"] = json!(SENTINELS[2]);
    events.push(key);
    let mut rule = events[0].clone();
    rule["payload"]["plan"] = json!("plus");
    events.push(rule);
    let parsed = Batch::parse(&batch_bytes(&b)).unwrap();
    assert_eq!(parsed.events.len(), 8);
    assert_eq!(parsed.accepted().count(), 3, "the valid events survive");
    let outcomes: Vec<_> = parsed.events[3..].iter().map(|e| &e.outcome).collect();
    assert_eq!(
        outcomes,
        [
            &EventOutcome::Rejected(ValidationError::Malformed),
            &EventOutcome::Rejected(ValidationError::Malformed),
            &EventOutcome::Rejected(ValidationError::Malformed),
            &EventOutcome::Rejected(ValidationError::ForbiddenKey),
            &EventOutcome::Rejected(ValidationError::InvalidValue("plan")),
        ]
    );
    let s = statuses(&parsed);
    assert_eq!(
        s[4],
        json!({"index": 4, "status": "rejected", "reason": "malformed"}),
        "an invalid event id is not echoed"
    );
    assert_eq!(s[7]["reason"], "invalid_value");
    assert_eq!(s[7]["field"], "plan");
    let text = serde_json::to_string(&s).unwrap();
    assert!(!text.contains(SENTINELS[2]), "no input value echoed");
    // The wire shape reads back.
    let back: EventStatus = serde_json::from_value(s[7].clone()).unwrap();
    assert_eq!(back, parsed.events[7].status());
}

#[test]
fn envelope_and_version_errors_stay_batch_level() {
    // Even next to an unknown kind and a malformed event, a bad envelope refuses the batch.
    let mut b = example("batch.json");
    b["events"][0]["kind"] = json!("device.inventory");
    b["events"][1] = json!("garbage");
    type Mutation = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(Mutation, ValidationError)> = vec![
        (
            Box::new(|b| b["deviceId"] = json!("PC-OF-KEV")),
            ValidationError::InvalidValue("deviceId"),
        ),
        (
            Box::new(|b| b["schemaVersion"] = json!(2)),
            ValidationError::UnsupportedVersion,
        ),
        (
            Box::new(|b| b["extra"] = json!(1)),
            ValidationError::Malformed,
        ),
        (
            Box::new(|b| b["events"] = json!({})),
            ValidationError::Malformed,
        ),
        (
            Box::new(|b| b["events"] = json!([])),
            ValidationError::InvalidValue("events"),
        ),
        (
            Box::new(|b| b["events"] = Value::Array(vec![json!(1); MAX_BATCH_EVENTS + 1])),
            ValidationError::TooLarge,
        ),
    ];
    for (mutate, err) in cases {
        let mut bad = b.clone();
        mutate(&mut bad);
        assert_eq!(Batch::parse(&batch_bytes(&bad)), Err(err));
    }
    let parsed = Batch::parse(&batch_bytes(&b)).unwrap();
    assert_eq!(parsed.events[0].outcome, EventOutcome::Ignored);
    assert_eq!(
        parsed.events[1].outcome,
        EventOutcome::Rejected(ValidationError::Malformed)
    );
    assert_eq!(parsed.accepted().count(), 1);
}

// ---------------------------------------------------------------------------------------------
// Versioning
// ---------------------------------------------------------------------------------------------

#[test]
fn version_is_read_before_the_shape() {
    // A later version may add fields: it is reported as unsupported (server: 426), not malformed.
    let mut v2 = example("batch.json");
    v2["schemaVersion"] = json!(2);
    v2["newEnvelopeField"] = json!({"x": 1});
    assert_eq!(
        Batch::parse(&batch_bytes(&v2)),
        Err(ValidationError::UnsupportedVersion)
    );
    let mut v0 = example("batch.json");
    v0["schemaVersion"] = json!(0);
    assert_eq!(
        Batch::parse(&batch_bytes(&v0)),
        Err(ValidationError::UnsupportedVersion)
    );
    let mut none = example("batch.json");
    none.as_object_mut().unwrap().remove("schemaVersion");
    assert_eq!(
        Batch::parse(&batch_bytes(&none)),
        Err(ValidationError::Malformed)
    );
    assert_eq!(SCHEMA_VERSION, 1);
}

// ---------------------------------------------------------------------------------------------
// JSON Schema
// ---------------------------------------------------------------------------------------------

fn schema_path() -> PathBuf {
    crate_dir().join("schema").join(schema::FILE_NAME)
}

/// Fails when the types and `schema/usage-events.v1.json` drift apart. With
/// `CODENOTCH_WRITE_SCHEMA=1` the file is (re)written instead.
#[test]
fn schema_file_matches_the_types() {
    let generated = schema::generate();
    if std::env::var_os("CODENOTCH_WRITE_SCHEMA").is_some() {
        std::fs::write(schema_path(), schema::generate_pretty()).unwrap();
    }
    let text = std::fs::read_to_string(schema_path()).expect("schema file missing");
    let committed: Value = serde_json::from_str(&text).unwrap();
    assert!(
        committed == generated,
        "schema/{} is out of date with the types",
        schema::FILE_NAME
    );
}

/// Visit every subschema (properties, items, $defs, oneOf …).
fn walk<'a>(value: &'a Value, out: &mut Vec<&'a serde_json::Map<String, Value>>) {
    match value {
        Value::Object(map) => {
            out.push(map);
            for v in map.values() {
                walk(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
        _ => {}
    }
}

#[test]
fn schema_is_closed_at_every_level() {
    let generated = schema::generate();
    let mut nodes = Vec::new();
    walk(&generated, &mut nodes);
    let mut objects = 0;
    for node in nodes {
        let is_object =
            node.get("type") == Some(&json!("object")) || node.contains_key("properties");
        if is_object {
            objects += 1;
            assert_eq!(
                node.get("additionalProperties"),
                Some(&json!(false)),
                "open object: {}",
                serde_json::to_string(node).unwrap()
            );
        }
        if let Some(props) = node.get("properties").and_then(Value::as_object) {
            for key in props.keys() {
                assert!(
                    !FORBIDDEN_KEYS.iter().any(|f| f.eq_ignore_ascii_case(key)),
                    "forbidden key in schema: {key}"
                );
            }
        }
    }
    // Batch, RawEvent, QuotaSnapshot, QuotaWindow, UsageHour, Tokens, Heartbeat.
    assert_eq!(objects, 7);
    let defs = generated["$defs"].as_object().unwrap();
    for name in [
        "RawEvent",
        "QuotaSnapshot",
        "QuotaWindow",
        "UsageHour",
        "Tokens",
        "Heartbeat",
    ] {
        assert!(defs.contains_key(name), "$defs.{name}");
    }
    assert_eq!(
        defs["ModelId"]["enum"].as_array().unwrap().len(),
        ModelId::TABLE.len() + 1
    );
}
