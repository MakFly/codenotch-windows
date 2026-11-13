//! Upload behaviour: statuses, acknowledgements by id, backoff, origin.

mod common;

use codenotch_core::model::TeamsState;
use codenotch_core::{HttpResponse, Method};
use codenotch_teams::sync::BACKFILL_SECS;
use codenotch_teams::{i18n, PauseReason, SendOutcome, TeamsSync};
use codenotch_teams_schema::Batch;
use common::*;

const TOKEN: &str = "cnt_test_token";

#[test]
fn nothing_queued_or_no_policy_means_no_request() {
    let f = fixture();
    let http = SeqHttp::new(vec![]);
    let mut s = TeamsSync::create(&f.paths, info("a")).unwrap();
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::Nothing, "no policy");
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    assert_eq!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Nothing,
        "empty queue"
    );
    assert!(http.requests().is_empty());
}

#[test]
fn server_error_then_success() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 3);
    let http = SeqHttp::new(vec![Reply::Status(500, String::new()), accept_all()]);
    let SendOutcome::Retry { at } = s.send(&http, TOKEN, NOW) else {
        panic!("retry expected");
    };
    assert!((NOW + 24..=NOW + 36).contains(&at), "30 s ± 20 %");
    assert_eq!(s.retry_at(), Some(at));
    assert_eq!(s.status().state, TeamsState::Paused);
    assert_eq!(s.status().last_error.as_deref(), Some(i18n::PAUSED));
    assert_eq!(s.queued(), 3);
    assert_eq!(
        s.send(&http, TOKEN, at),
        SendOutcome::Sent {
            removed: 3,
            more: false
        }
    );
    let st = s.status();
    assert_eq!(
        (st.state, st.queued, st.last_sent_at),
        (TeamsState::Active, 0, Some(at))
    );
    assert_eq!(st.team_name.as_deref(), Some("Team a"));
    assert_eq!(st.last_error, None);

    // Every request went to the origin, as a POST, with the token, and is a valid batch.
    let requests = http.requests();
    assert_eq!(requests.len(), 2);
    for r in &requests {
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.url, format!("{ORIGIN}/v1/events"));
        assert_eq!(
            r.header_value("authorization"),
            Some("Bearer cnt_test_token")
        );
        let batch = Batch::parse(r.body.as_deref().unwrap()).expect("valid batch");
        assert_eq!(batch.device_id, DEVICE);
        assert_eq!(batch.app_version, "0.1.0");
        assert!(batch.events.iter().all(|e| e.validate().is_ok()));
    }
    // Nothing left: the outbox file is gone.
    assert!(!f.paths.teams_outbox().exists());
}

#[test]
fn backoff_grows_and_is_capped() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 1);
    let http = SeqHttp::new(vec![]);
    let mut delays = Vec::new();
    for _ in 0..8 {
        let SendOutcome::Retry { at } = s.send(&http, TOKEN, NOW) else {
            panic!("retry expected");
        };
        delays.push(at - NOW);
    }
    assert!((48..=72).contains(&delays[1]), "{delays:?}");
    assert!(delays.iter().all(|d| *d <= 15 * 60 * 6 / 5), "{delays:?}");
    assert!(delays[7] >= 15 * 60 * 4 / 5, "{delays:?}");
}

#[test]
fn rate_limited_waits_for_retry_after() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 1);
    let http = SeqHttp::new(vec![Reply::Response(
        HttpResponse::new(429, Vec::new()).with_header("Retry-After", "120"),
    )]);
    assert_eq!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Retry { at: NOW + 120 }
    );
    assert_eq!(s.queued(), 1);
}

#[test]
fn revoked_consent_update_and_billing() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 2);
    let http = SeqHttp::new(vec![
        Reply::Status(401, String::new()),
        Reply::Status(409, r#"{"error":"consent_required"}"#.into()),
        Reply::Status(426, r#"{"accepted":[1]}"#.into()),
        Reply::Status(402, String::new()),
    ]);
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::Revoked);
    assert_eq!(s.status().state, TeamsState::Revoked);
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::ConsentNeeded);
    assert_eq!(s.status().state, TeamsState::ConsentNeeded);
    assert_eq!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Paused {
            until: NOW + 6 * 3600,
            reason: PauseReason::UpdateRequired
        }
    );
    assert_eq!(s.status().last_error.as_deref(), Some(i18n::UPDATE));
    assert!(matches!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Paused {
            reason: PauseReason::Billing,
            ..
        }
    ));
    assert_eq!(s.queued(), 2, "nothing lost");
}

#[test]
fn too_large_splits_the_batch_then_drops_a_single_event() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 8);
    let http = SeqHttp::new(vec![
        Reply::Status(413, String::new()),
        Reply::Status(413, String::new()),
        accept_all(),
    ]);
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::Retry { at: NOW });
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::Retry { at: NOW });
    assert_eq!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Sent {
            removed: 2,
            more: true
        }
    );
    let sizes: Vec<usize> = http.requests().iter().map(|r| batch_ids(r).len()).collect();
    assert_eq!(sizes, [8, 4, 2]);

    // A single event refused as too large is dropped, the rest stays.
    let mut s = start(&f);
    queue(&mut s, 2);
    let http = SeqHttp::new(vec![
        Reply::Status(413, String::new()),
        Reply::Status(413, String::new()),
    ]);
    s.send(&http, TOKEN, NOW);
    s.send(&http, TOKEN, NOW);
    assert_eq!(s.queued(), 1);
}

#[test]
fn individual_rejection_drops_only_that_event() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 3);
    let http = SeqHttp::new(vec![Reply::With(Box::new(|req| {
        let ids = batch_ids(req);
        HttpResponse::new(
            200,
            serde_json::json!({
                "accepted": [ids[0]],
                "rejected": [{ "eventId": ids[1], "reason": "invalid_value" }]
            })
            .to_string(),
        )
    }))]);
    assert_eq!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Sent {
            removed: 2,
            more: true
        }
    );
    let left = batch_ids(&http.requests()[0])[2].clone();
    let http = SeqHttp::new(vec![accept_all()]);
    s.send(&http, TOKEN, NOW + 60);
    assert_eq!(batch_ids(&http.requests()[0]), [left]);
}

#[test]
fn lost_reply_after_commit_resends_the_same_id() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 1);
    let http = SeqHttp::new(vec![
        Reply::Fail,
        Reply::With(Box::new(|req| {
            HttpResponse::new(
                200,
                serde_json::json!({ "duplicate": batch_ids(req) }).to_string(),
            )
        })),
    ]);
    assert!(matches!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::Retry { .. }
    ));
    // The app dies and starts again before the retry.
    drop(s);
    let mut s = TeamsSync::open(&f.paths).unwrap().unwrap();
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    assert!(matches!(
        s.send(&http, TOKEN, NOW + 60),
        SendOutcome::Sent { removed: 1, .. }
    ));
    let r = http.requests();
    assert_eq!(batch_ids(&r[0]), batch_ids(&r[1]));
    assert_eq!(s.queued(), 0);
}

#[test]
fn a_revision_made_while_sending_survives_the_late_ack() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3600;
    s.observe_usage(&[claude(h, 1)], NOW).unwrap();
    let http = SeqHttp::new(vec![Reply::Fail]);
    s.send(&http, TOKEN, NOW);
    let first = batch_ids(&http.requests()[0]);
    // A new total arrives before the server answered: new revision, new id.
    s.observe_usage(&[claude(h, 2)], NOW).unwrap();
    let http = SeqHttp::new(vec![Reply::Status(
        200,
        serde_json::json!({ "accepted": first }).to_string(),
    )]);
    let out = s.send(&http, TOKEN, NOW + 60);
    assert_eq!(
        out,
        SendOutcome::Sent {
            removed: 0,
            more: true
        }
    );
    assert_eq!(s.queued(), 1);
}

#[test]
fn epoch_changed_purges_the_queue_and_moves_the_floor() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 3);
    let erased_at = NOW - 2 * 3600;
    let http = SeqHttp::new(vec![Reply::Status(
        409,
        serde_json::json!({ "error": "epoch_changed", "dataEpoch": 2, "floorAt": erased_at })
            .to_string(),
    )]);
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::EpochChanged);
    assert_eq!(s.queued(), 0);
    assert!(!f.paths.teams_outbox().exists());
    assert_eq!(s.binding().data_epoch, 2);
    assert_eq!(s.effective_floor(), erased_at);
    // Hours before the erase are never sent again; later ones are, with the new epoch.
    queue(&mut s, 1);
    assert_eq!(s.queued(), 1);
    let http = SeqHttp::new(vec![accept_all()]);
    s.send(&http, TOKEN, NOW);
    let batch = Batch::parse(http.requests()[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(batch.data_epoch, 2);
    // Persisted.
    let s = TeamsSync::open(&f.paths).unwrap().unwrap();
    assert_eq!(
        (s.binding().data_epoch, s.effective_floor()),
        (2, erased_at)
    );
}

#[test]
fn redirect_is_not_followed() {
    let f = fixture();
    let mut s = start(&f);
    queue(&mut s, 1);
    let http = SeqHttp::new(vec![Reply::Response(
        HttpResponse::new(302, Vec::new()).with_header("Location", "https://evil.example.test/"),
    )]);
    assert!(matches!(
        s.send(&http, TOKEN, NOW),
        SendOutcome::NotATeamsServer { .. }
    ));
    assert_eq!(http.requests().len(), 1);
    assert_eq!(s.status().state, TeamsState::Error);
    assert_eq!(s.status().last_error.as_deref(), Some(i18n::NOT_A_SERVER));
}

#[test]
fn team_a_queue_never_reaches_team_b() {
    let f = fixture();
    let mut a = start(&f);
    queue(&mut a, 2);
    drop(a);
    // Detach (always destructive), then pair with team B.
    TeamsSync::detach_local(&f.paths).unwrap();
    assert!(!f.paths.teams_outbox().exists() && !f.paths.teams_state().exists());
    let mut b = TeamsSync::create(&f.paths, info("b")).unwrap();
    b.set_policy(&f.config, &f.owners, NOW).unwrap();
    assert_eq!(b.queued(), 0);

    // Even without the detach step, a new pairing never inherits the old queue.
    let mut a = start(&f);
    queue(&mut a, 2);
    drop(a);
    let mut b = TeamsSync::create(&f.paths, info("b")).unwrap();
    b.set_policy(&f.config, &f.owners, NOW).unwrap();
    let http = SeqHttp::new(vec![accept_all()]);
    assert_eq!(b.send(&http, TOKEN, NOW), SendOutcome::Nothing);
    assert!(http.requests().is_empty());
}

#[test]
fn excluded_after_queueing_is_purged_before_sending() {
    let mut f = fixture();
    let mut s = start(&f);
    queue(&mut s, 2);
    f.config.settings.teams_excluded_accounts = vec!["c1".into()];
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    let http = SeqHttp::new(vec![accept_all()]);
    assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::Nothing);
    assert!(http.requests().is_empty());
}

#[test]
fn epoch_changed_never_moves_the_floor_back() {
    let conflict = |floor_at: &serde_json::Value| {
        Reply::Status(
            409,
            serde_json::json!({ "error": "epoch_changed", "dataEpoch": 2, "floorAt": floor_at })
                .to_string(),
        )
    };
    let cases = [
        (serde_json::json!(1), H0),
        (serde_json::json!(i64::MIN), H0),
        (serde_json::json!(i64::MAX), NOW),
        (serde_json::Value::Null, NOW),
        (serde_json::json!(NOW - 3600), NOW - 3600),
    ];
    for (floor_at, expected) in cases {
        let f = fixture();
        let mut s = start(&f);
        queue(&mut s, 1);
        let http = SeqHttp::new(vec![conflict(&floor_at)]);
        assert_eq!(s.send(&http, TOKEN, NOW), SendOutcome::EpochChanged);
        assert_eq!(s.effective_floor(), expected, "{floor_at}");
        s.allow_backfill(true).unwrap();
        assert_eq!(s.effective_floor(), expected - BACKFILL_SECS);
    }
}
