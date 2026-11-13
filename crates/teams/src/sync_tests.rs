//! Unit tests of [`super::TeamsSync`]: detection, revisions, floor, crash protocol.

use super::*;
use crate::fault::arm;
use codenotch_core::model::{ReadingStatus, WindowKind as CoreKind};
use codenotch_core::{Account, LimitWindow, ProviderKind, TokenCounts};
use codenotch_teams_schema::{ModelId, PlanCode, WindowRole};

const DEVICE: &str = "6f1c2a9e-0b7d-4a51-9c7e-2f0d7b1a4e11";
/// Aligned hour: the pairing floor.
const H0: i64 = 1_794_553_200;
const NOW: i64 = H0 + 100 * 3600;

struct Fixture {
    _dir: tempfile::TempDir,
    paths: Paths,
    config: Config,
    owners: Vec<RootOwners>,
}

fn account(kind: ProviderKind, id: &str) -> Account {
    let mut a = Account::new(kind, "name");
    a.id = id.into();
    a
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path().join("data"));
    let config = Config {
        accounts: vec![
            account(ProviderKind::Claude, "c1"),
            account(ProviderKind::Codex, "x1"),
        ],
        ..Default::default()
    };
    let owner = |s: &str, p: ProviderKind, id: &str| RootOwners {
        source: s.into(),
        provider: p,
        accounts: vec![id.into()],
    };
    let owners = vec![
        owner("R-claude", ProviderKind::Claude, "c1"),
        owner("R-codex-s", ProviderKind::Codex, "x1"),
        owner("R-codex-a", ProviderKind::Codex, "x1"),
    ];
    Fixture {
        _dir: dir,
        paths,
        config,
        owners,
    }
}

fn info() -> PairingInfo {
    PairingInfo {
        origin: Origin::parse("https://teams.example.test").unwrap(),
        team_id: "team-a".into(),
        team_name: "Team A".into(),
        device_id: DEVICE.into(),
        generation: 1,
        consent_version: 1,
        data_epoch: 1,
        floor_at: H0,
    }
}

fn start(f: &Fixture) -> TeamsSync {
    let mut s = TeamsSync::create(&f.paths, info()).unwrap();
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    s
}

fn reopen(f: &Fixture) -> TeamsSync {
    let mut s = TeamsSync::open(&f.paths).unwrap().expect("paired");
    s.set_policy(&f.config, &f.owners, NOW).unwrap();
    s
}

fn row(source: &str, kind: ProviderKind, model: &str, hour: i64, input: u64) -> HourlyUsage {
    HourlyUsage {
        hour,
        source: source.into(),
        provider: kind,
        model: model.into(),
        tokens: TokenCounts {
            input,
            ..Default::default()
        },
        cache_write_1h: 0,
    }
}

fn claude(hour: i64, input: u64) -> HourlyUsage {
    row(
        "R-claude",
        ProviderKind::Claude,
        "claude-opus-5-5",
        hour,
        input,
    )
}

/// Pending usage revisions: (hour, rev, input).
fn usage(s: &TeamsSync) -> Vec<(i64, u64, u64)> {
    s.journal
        .pending()
        .filter_map(|r| match r.event.payload() {
            Ok(EventPayload::UsageHour(u)) => Some((u.hour_start, u.rev, u.tokens.input)),
            _ => None,
        })
        .collect()
}

fn snapshots(s: &TeamsSync) -> Vec<QuotaSnapshot> {
    s.journal
        .pending()
        .filter_map(|r| match r.event.payload() {
            Ok(EventPayload::QuotaSnapshot(q)) => Some(q),
            _ => None,
        })
        .collect()
}

#[test]
fn a_b_a_converges_with_three_revisions() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3 * 3600;
    for input in [100, 120, 100] {
        assert_eq!(s.observe_usage(&[claude(h, input)], NOW).unwrap(), 1);
    }
    // Only the latest revision of a key stays queued.
    assert_eq!(usage(&s), [(h, 3, 100)]);
    assert_eq!(s.observe_usage(&[claude(h, 100)], NOW).unwrap(), 0);
    assert_eq!(usage(&reopen(&f)), [(h, 3, 100)]);
}

#[test]
fn clock_going_back_does_not_reuse_revisions() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3 * 3600;
    s.observe_usage(&[claude(h, 1)], NOW).unwrap();
    s.observe_usage(&[claude(h, 2)], NOW - 7200).unwrap();
    s.observe_usage(&[claude(h, 3)], NOW + 60).unwrap();
    assert_eq!(usage(&s), [(h, 3, 3)]);
}

#[test]
fn roots_of_one_account_add_up_and_others_are_ignored() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3600;
    let rows = [
        row("R-codex-s", ProviderKind::Codex, "gpt-5", h, 10),
        row("R-codex-a", ProviderKind::Codex, "gpt-5", h, 5),
        row("default-root", ProviderKind::Codex, "gpt-5", h, 1000),
    ];
    assert_eq!(s.observe_usage(&rows, NOW).unwrap(), 1);
    assert_eq!(usage(&s), [(h, 1, 15)]);
}

#[test]
fn tombstones_only_inside_the_mutable_window() {
    let f = fixture();
    let mut s = start(&f);
    let recent = NOW - 10 * 3600;
    let old = NOW - 80 * 3600;
    s.observe_usage(&[claude(recent, 7), claude(old, 9)], NOW)
        .unwrap();
    // A changed total for an hour beyond 72 h is final: no revision.
    assert_eq!(
        s.observe_usage(&[claude(recent, 7), claude(old, 99)], NOW)
            .unwrap(),
        0
    );
    // Both disappear: only the recent one gets a zero revision.
    assert_eq!(s.observe_usage(&[], NOW).unwrap(), 1);
    let mut got = usage(&s);
    got.sort();
    assert_eq!(got, [(old, 1, 9), (recent, 2, 0)]);
    assert_eq!(s.observe_usage(&[], NOW).unwrap(), 0, "one tombstone only");
}

#[test]
fn floor_and_backfill_agreement() {
    let f = fixture();
    let mut s = start(&f);
    let before = H0 - 5 * 86_400;
    assert_eq!(s.observe_usage(&[claude(before, 1)], NOW).unwrap(), 0);
    s.allow_backfill(true).unwrap();
    assert_eq!(s.effective_floor(), H0 - BACKFILL_SECS);
    assert_eq!(s.observe_usage(&[claude(before, 1)], NOW).unwrap(), 1);
    // Withdrawn before it was sent: purged from the queue.
    s.allow_backfill(false).unwrap();
    assert!(usage(&s).is_empty());
    assert!(usage(&reopen(&f)).is_empty());
}

fn reading(plan: &str, session: f64, resets: i64, weekly_secs: i64) -> Reading {
    let acc = account(ProviderKind::Claude, "c1");
    let mut r = Reading::fresh(
        &acc,
        NOW - 5,
        vec![
            LimitWindow::new("session", "Current session", CoreKind::Session)
                .used(session)
                .resets_at(resets)
                .window_seconds(18_000),
            LimitWindow::new("seven_day", "Weekly", CoreKind::Weekly)
                .used(41.0)
                .window_seconds(weekly_secs),
        ],
    );
    r.plan = Some(plan.into());
    r
}

#[test]
fn snapshots_on_change_and_every_6_hours() {
    let f = fixture();
    let mut s = start(&f);
    let r0 = reading("Max", 62.0, NOW + 3600, 604_800);
    assert!(s.observe_reading(&r0, NOW).unwrap());
    let q = &snapshots(&s)[0];
    assert_eq!((q.seq, q.plan, q.windows.len()), (1, PlanCode::Max, 2));
    assert_eq!(q.windows[0].role, Some(WindowRole::Headline));
    assert_eq!(q.windows[1].role, Some(WindowRole::Weekly));
    assert_eq!(q.observed_at, NOW - 5);
    let mut seq = 1;
    let mut expect = |s: &mut TeamsSync, r: &Reading, t: i64, sent: bool| {
        assert_eq!(s.observe_reading(r, t).unwrap(), sent, "seq {seq}");
        if sent {
            seq += 1;
        }
        assert_eq!(snapshots(s).last().unwrap().seq, seq);
    };
    let t = NOW;
    expect(&mut s, &r0, t + 60, false);
    // Same rounded percent, reset moved by 30 s: no change.
    expect(&mut s, &reading("Max", 62.3, NOW + 3630, 604_800), t, false);
    expect(&mut s, &reading("Max", 63.0, NOW + 3600, 604_800), t, true);
    expect(&mut s, &reading("Max", 63.0, NOW + 3700, 604_800), t, true);
    expect(&mut s, &reading("Max", 63.0, NOW + 3700, 86_400), t, true);
    expect(&mut s, &reading("Pro", 63.0, NOW + 3700, 86_400), t, true);
    let mut gone = reading("Pro", 63.0, NOW + 3700, 86_400);
    gone.windows.truncate(1);
    expect(&mut s, &gone, t, true);
    let mut role = gone.clone();
    role.headline_window = Some("none".into());
    expect(&mut s, &role, t, true);
    expect(&mut s, &role, t + RESYNC_SECS - 1, false);
    expect(&mut s, &role, t + RESYNC_SECS, true);
    let mut stale = role.clone();
    stale.status = ReadingStatus::Stale;
    expect(&mut s, &stale, t + 3 * RESYNC_SECS, false);
    // A snapshot replaces the previous one in the queue.
    assert_eq!(snapshots(&s).len(), 1);
}

#[test]
fn excluding_an_account_purges_its_events_and_sends_a_tombstone() {
    let mut f = fixture();
    let mut s = start(&f);
    s.observe_reading(&reading("Max", 10.0, NOW + 60, 604_800), NOW)
        .unwrap();
    s.observe_usage(&[claude(NOW - 3600, 5)], NOW).unwrap();
    f.config.settings.teams_excluded_accounts = vec!["c1".into()];
    s.set_policy(&f.config, &f.owners, NOW + 1).unwrap();
    assert!(usage(&s).is_empty());
    let snaps = snapshots(&s);
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].windows.is_empty() && snaps[0].seq == 2);
    // Not repeated, and nothing new for the excluded account.
    s.set_policy(&f.config, &f.owners, NOW + 2).unwrap();
    assert!(!s
        .observe_reading(&reading("Max", 90.0, NOW, 604_800), NOW)
        .unwrap());
    assert_eq!(s.observe_usage(&[claude(NOW - 3600, 6)], NOW).unwrap(), 0);
    assert_eq!(s.queued(), 1);
}

#[test]
fn crash_after_each_step_loses_nothing_and_reuses_no_revision() {
    let h = NOW - 3600;
    for step in [Step::Append, Step::Sync, Step::Cursor, Step::State] {
        let f = fixture();
        let mut s = start(&f);
        s.observe_usage(&[claude(h, 1)], NOW).unwrap();
        arm(step);
        assert!(s.observe_usage(&[claude(h, 2)], NOW).is_err(), "{step:?}");
        drop(s); // the process dies here
        let mut s = reopen(&f);
        s.observe_usage(&[claude(h, 2)], NOW).unwrap();
        assert_eq!(usage(&s), [(h, 2, 2)], "{step:?}");
        s.observe_usage(&[claude(h, 3)], NOW).unwrap();
        assert_eq!(usage(&s), [(h, 3, 3)], "{step:?}");
        assert_eq!(usage(&reopen(&f)), [(h, 3, 3)], "{step:?}");
    }
}

#[test]
fn failed_write_is_an_error_status_and_does_not_advance() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3600;
    arm(Step::Append);
    assert!(s.observe_usage(&[claude(h, 1)], NOW).is_err());
    assert_eq!(s.status().state, TeamsState::Error);
    assert_eq!(s.status().last_error.as_deref(), Some(i18n::STORAGE));
    assert_eq!(s.queued(), 0);
    s.observe_usage(&[claude(h, 1)], NOW).unwrap();
    assert_eq!(usage(&s), [(h, 1, 1)], "rev 1 was not consumed");
    assert_eq!(s.status().state, TeamsState::Active);
}

#[test]
fn unknown_models_are_unknown() {
    let f = fixture();
    let mut s = start(&f);
    let h = NOW - 3600;
    let rows = [
        claude(h, 1),
        row("R-claude", ProviderKind::Claude, "gpt-5", h, 2),
        row("R-claude", ProviderKind::Claude, "sk-secret123", h, 4),
    ];
    s.observe_usage(&rows, NOW).unwrap();
    let models: Vec<(ModelId, u64)> = s
        .journal
        .pending()
        .filter_map(|r| match r.event.payload() {
            Ok(EventPayload::UsageHour(u)) => Some((u.model, u.tokens.input)),
            _ => None,
        })
        .collect();
    assert_eq!(models, [(ModelId::ClaudeOpus5_5, 1), (ModelId::Unknown, 6)]);
}

#[test]
fn no_policy_no_event_and_not_paired_no_file() {
    let f = fixture();
    assert!(TeamsSync::open(&f.paths).unwrap().is_none());
    assert!(!f.paths.root().exists(), "nothing created while not paired");
    let mut s = TeamsSync::create(&f.paths, info()).unwrap();
    assert_eq!(s.observe_usage(&[claude(NOW - 3600, 1)], NOW).unwrap(), 0);
    assert!(!s
        .observe_reading(&reading("Max", 1.0, NOW, 604_800), NOW)
        .unwrap());
    assert!(!f.paths.teams_outbox().exists());
    TeamsSync::detach_local(&f.paths).unwrap();
    assert!(!f.paths.teams_state().exists());
}

#[test]
fn records_of_another_schema_version_are_never_sent() {
    let f = fixture();
    let mut s = start(&f);
    s.observe_usage(&[claude(NOW - 3600, 1)], NOW).unwrap();
    // Relabel the queued record on disk as another version.
    let text = std::fs::read_to_string(f.paths.teams_outbox()).unwrap();
    std::fs::write(
        f.paths.teams_outbox(),
        text.replace("\"schemaVersion\":1", "\"schemaVersion\":2"),
    )
    .unwrap();
    let mut s = reopen(&f);
    assert_eq!(s.queued(), 0, "purged by the re-filter, never relabelled");
    let http = codenotch_core::FakeHttp::new();
    assert_eq!(s.send(&http, "t", NOW), SendOutcome::Nothing);
    assert!(http.requests().is_empty());
}
