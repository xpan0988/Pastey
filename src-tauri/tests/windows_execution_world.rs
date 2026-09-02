#[cfg(windows)]
#[test]
#[ignore = "requires the Host-owned Codex sandbox setup on a configured native Windows host"]
fn native_windows_codex_execution_world_conformance() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_pastey"))
        .arg("--pastey-verify-windows-codex-sandbox-v1")
        .status()
        .expect("start the Pastey Windows ExecutionWorld verifier");
    assert!(
        status.success(),
        "the product binary did not establish native Windows confinement"
    );
}
