#[cfg(windows)]
#[test]
#[ignore = "requires the one-time elevated sandbox-principal setup on a native Windows host"]
fn native_windows_restricted_principal_execution_world_conformance() {
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_pastey"))
        .arg("--pastey-verify-windows-execution-world-v1")
        .env_clear()
        .status()
        .expect("start the Pastey Windows ExecutionWorld verifier");
    assert!(
        status.success(),
        "the product binary did not establish native Windows confinement"
    );
}
