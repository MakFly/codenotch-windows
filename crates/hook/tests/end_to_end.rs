//! End to end: the activity hook server on an ephemeral port in a temp data dir, driven by the
//! real `codenotch-hook` binary and by its library entry point.

use codenotch_activity::{ActivityReport, ServerStatus};
use codenotch_core::hooks::{HookEvent, MAX_PAYLOAD};
use codenotch_core::{Account, ActivityState, Env, Paths, ProviderKind, Settings};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Rig {
    _dir: tempfile::TempDir,
    data: std::path::PathBuf,
    env: Env,
    account: Account,
    reports: Arc<Mutex<Vec<ActivityReport>>>,
    handle: codenotch_activity::ActivityHandle,
    port: u16,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let paths = Paths::at(&data);
    let env = Env::for_test(dir.path());
    let account = Account::new(ProviderKind::Claude, "Claude");
    let settings = Settings {
        hooks_enabled: true,
        hook_port: 0,
        ..Settings::default()
    };
    let reports: Arc<Mutex<Vec<ActivityReport>>> = Arc::default();
    let r2 = reports.clone();
    let handle = codenotch_activity::start(
        &paths,
        &env,
        &settings,
        std::slice::from_ref(&account),
        Box::new(move |r| r2.lock().unwrap().push(r)),
    )
    .unwrap();
    let ServerStatus::Listening(port) = handle.server_status() else {
        panic!("server not listening")
    };
    // Tell the helper which port to use, as the app's config would.
    std::fs::write(
        paths.config(),
        format!(r#"{{"accounts":[],"settings":{{"hooksEnabled":true,"hookPort":{port}}}}}"#),
    )
    .unwrap();
    Rig {
        _dir: dir,
        data,
        env,
        account,
        reports,
        handle,
        port,
    }
}

impl Rig {
    fn payload(&self, id: &str, extra: &str) -> String {
        let transcript = self
            .env
            .home
            .join(".claude")
            .join("projects")
            .join("C--w-app")
            .join(format!("{id}.jsonl"));
        let transcript = serde_json::Value::String(transcript.to_string_lossy().into_owned());
        format!(
            r#"{{"session_id":"{id}","transcript_path":{transcript},"cwd":"C:\\w\\app"{extra}}}"#
        )
    }

    fn wait_state(&self, want: ActivityState) -> Vec<codenotch_core::SessionInfo> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (state, sessions) = self.handle.report().for_account(&self.account, &self.env);
            if state == want {
                return sessions;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {want:?}, have {state:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn lib_run(&self, event: &str, body: Vec<u8>) {
        codenotch_hook::run(
            Some(event),
            Some(std::io::Cursor::new(body)),
            &Paths::at(&self.data),
        );
    }
}

#[test]
fn real_binary_and_library_drive_the_server() {
    let rig = rig();

    // 1. The real executable, stdin piped like Claude Code does.
    let mut child = Command::new(env!("CARGO_BIN_EXE_codenotch-hook"))
        .arg("running")
        .env("CODENOTCH_DATA_DIR", &rig.data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let body = rig.payload("sess-1234", r#","prompt":"refactor the parser""#);
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let t0 = Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "always exit 0");
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "never prints"
    );
    assert!(t0.elapsed() < Duration::from_secs(3));
    let sessions = rig.wait_state(ActivityState::Working);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "app · sess");
    assert_eq!(sessions[0].detail.as_deref(), Some("refactor the parser"));

    // 2. Library entry point: waiting on the user.
    rig.lib_run(
        "attention",
        rig.payload(
            "sess-1234",
            r#","message":"Claude needs your permission to use Bash""#,
        )
        .into_bytes(),
    );
    let s = rig.wait_state(ActivityState::Waiting);
    assert_eq!(
        s[0].detail.as_deref(),
        Some("Claude needs your permission to use Bash")
    );

    // 3. An oversized payload is truncated to the cap and still attributed (fields come first).
    let huge = rig.payload(
        "sess-1234",
        &format!(
            r#","tool_name":"Read","tool_response":"{}""#,
            "x".repeat(MAX_PAYLOAD)
        ),
    );
    assert!(huge.len() > MAX_PAYLOAD);
    rig.lib_run("running", huge.into_bytes());
    let s = rig.wait_state(ActivityState::Working);
    assert_eq!(s[0].detail.as_deref(), Some("Read"));

    // 4. A wrong token is rejected and changes nothing.
    assert!(codenotch_hook::send(
        rig.port,
        HookEvent::Done,
        1,
        Some("bad"),
        rig.payload("sess-1234", "").as_bytes()
    )
    .is_ok());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        rig.handle.report().for_account(&rig.account, &rig.env).0,
        ActivityState::Working
    );

    // 5. Done, then the session ends.
    rig.lib_run("done", rig.payload("sess-1234", "").into_bytes());
    rig.wait_state(ActivityState::Done);
    rig.lib_run("session_end", rig.payload("sess-1234", "").into_bytes());
    let s = rig.wait_state(ActivityState::Idle);
    assert!(s.is_empty());

    // on_change was called for each visible change.
    assert!(rig.reports.lock().unwrap().len() >= 5);
    let t0 = Instant::now();
    rig.handle.stop();
    assert!(t0.elapsed() < Duration::from_secs(1));
}

#[test]
fn helper_without_server_exits_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(
        dir.path().join("config.gpui.json"),
        format!(r#"{{"accounts":[],"settings":{{"hookPort":{port}}}}}"#),
    )
    .unwrap();
    let t0 = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_codenotch-hook"))
        .arg("done")
        .env("CODENOTCH_DATA_DIR", dir.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
    assert!(t0.elapsed() < Duration::from_secs(3));
    // Unknown event: nothing sent, still exit 0.
    let out = Command::new(env!("CARGO_BIN_EXE_codenotch-hook"))
        .arg("bogus")
        .env("CODENOTCH_DATA_DIR", dir.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success());
}
