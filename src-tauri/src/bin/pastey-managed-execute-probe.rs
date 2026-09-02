//! Deterministic child executable for the opt-in native Windows Managed Execute test.

#[cfg(windows)]
fn main() {
    use std::io::{Read as _, Write as _};

    const EXPECTED_STDIN: &[u8] = b"pastey-managed-execute-stdin-v1";
    const STDOUT_TOKEN: &[u8] = b"PASTEY_MANAGED_EXECUTE_STDOUT_V1\n";
    const STDERR_TOKEN: &[u8] = b"PASTEY_MANAGED_EXECUTE_STDERR_V1\n";

    let mut stdin = Vec::new();
    if std::io::stdin().read_to_end(&mut stdin).is_err() || stdin != EXPECTED_STDIN {
        std::process::exit(65);
    }
    if std::io::stdout().write_all(STDOUT_TOKEN).is_err()
        || std::io::stdout().flush().is_err()
        || std::io::stderr().write_all(STDERR_TOKEN).is_err()
        || std::io::stderr().flush().is_err()
    {
        std::process::exit(66);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("pastey-managed-execute-probe is only for native Windows acceptance");
    std::process::exit(64);
}
