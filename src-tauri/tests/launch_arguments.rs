//! Starting the Pastey executable with an argument it does not take must end
//! at once, before any Host starts. A stale MCP entry (`pastey --physical-mcp
//! <grant>`) once opened a full second Host each time Claude Code began a
//! session.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[test]
fn an_unsupported_argument_exits_before_any_host_starts() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data_dir: PathBuf = std::env::temp_dir().join(format!(
        "pastey-launch-arguments-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&data_dir).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_pastey"))
        .args(["--physical-mcp", "/nonexistent/grant.json"])
        // If the refusal ever regresses, the Host it starts uses this folder,
        // not this device's real data.
        .env("PASTEY_APP_DATA_DIR", &data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&data_dir);
            panic!("pastey --physical-mcp was still running after 10 s: it started the app");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let created: Vec<_> = std::fs::read_dir(&data_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    let _ = std::fs::remove_dir_all(&data_dir);

    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("unsupported argument \"--physical-mcp\""),
        "stderr: {stderr}"
    );
    assert!(created.is_empty(), "the Host wrote {created:?}");
}
