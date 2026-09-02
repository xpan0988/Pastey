#[cfg(windows)]
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

#[cfg(windows)]
const TEST_CODEX_RUNNER_ENV: &str = "PASTEY_WINDOWS_TEST_CODEX_RUNNER_EXE";

#[cfg(windows)]
struct DevRunnerSiblingGuard {
    destination: PathBuf,
    backup: Option<PathBuf>,
}

#[cfg(windows)]
impl DevRunnerSiblingGuard {
    fn install(dev_pastey: &Path, installed_runner: &Path) -> io::Result<Self> {
        if !installed_runner.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "The Stage 4 Codex command runner source is unavailable.",
            ));
        }
        let parent = dev_pastey.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "The Cargo Pastey executable has no parent directory.",
            )
        })?;
        let destination = parent.join("codex-command-runner.exe");
        if destination.exists() && !destination.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "The Cargo Pastey runner sibling is not a regular file.",
            ));
        }
        let backup = if destination.is_file() {
            let backup = parent.join(format!(
                ".pastey-stage4-codex-command-runner-backup-{}.exe",
                uuid::Uuid::new_v4()
            ));
            fs::rename(&destination, &backup)?;
            Some(backup)
        } else {
            None
        };

        if let Err(error) = fs::copy(installed_runner, &destination) {
            if let Some(backup) = &backup {
                let _ = fs::rename(backup, &destination);
            }
            return Err(error);
        }

        Ok(Self {
            destination,
            backup,
        })
    }
}

#[cfg(windows)]
impl Drop for DevRunnerSiblingGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.destination);
        if let Some(backup) = &self.backup {
            let _ = fs::rename(backup, &self.destination);
        }
    }
}

#[cfg(windows)]
fn dev_runner_guard_test_root() -> PathBuf {
    env::temp_dir().join(format!(
        "pastey-stage4-runner-guard-{}",
        uuid::Uuid::new_v4()
    ))
}

#[cfg(windows)]
#[test]
fn dev_runner_sibling_guard_restores_a_preexisting_helper() {
    let root = dev_runner_guard_test_root();
    fs::create_dir_all(&root).expect("create test root");
    let dev_pastey = root.join("pastey.exe");
    let installed_runner = root.join("installed-codex-command-runner.exe");
    let sibling = root.join("codex-command-runner.exe");
    fs::write(&installed_runner, b"installed runner").expect("write source helper");
    fs::write(&sibling, b"preexisting helper").expect("write existing helper");

    {
        let guard = DevRunnerSiblingGuard::install(&dev_pastey, &installed_runner)
            .expect("install test-scoped helper");
        assert_eq!(fs::read(&guard.destination).unwrap(), b"installed runner");
    }

    assert_eq!(fs::read(&sibling).unwrap(), b"preexisting helper");
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn dev_runner_sibling_guard_removes_only_the_helper_it_created() {
    let root = dev_runner_guard_test_root();
    fs::create_dir_all(&root).expect("create test root");
    let dev_pastey = root.join("pastey.exe");
    let installed_runner = root.join("installed-codex-command-runner.exe");
    let sibling = root.join("codex-command-runner.exe");
    fs::write(&installed_runner, b"installed runner").expect("write source helper");

    {
        let guard = DevRunnerSiblingGuard::install(&dev_pastey, &installed_runner)
            .expect("install test-scoped helper");
        assert!(guard.destination.is_file());
    }

    assert!(!sibling.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
#[ignore = "requires the Host-owned Codex sandbox setup on a configured native Windows host"]
fn native_windows_codex_execution_world_conformance() {
    let dev_pastey = PathBuf::from(env!("CARGO_BIN_EXE_pastey"));
    let installed_runner = env::var_os(TEST_CODEX_RUNNER_ENV)
        .map(PathBuf::from)
        .expect("Stage 4 must supply the installed Codex command runner path");
    let runner_guard = DevRunnerSiblingGuard::install(&dev_pastey, &installed_runner)
        .expect("place the installed Codex command runner beside the Cargo Pastey executable");

    println!("DEV_PASTEY_EXE={}", dev_pastey.display());
    println!(
        "DEV_CODEX_COMMAND_RUNNER={}",
        runner_guard.destination.display()
    );
    println!(
        "DEV_CODEX_COMMAND_RUNNER_EXISTS={}",
        runner_guard.destination.is_file()
    );

    let status = std::process::Command::new(&dev_pastey)
        .arg("--pastey-verify-windows-codex-sandbox-v1")
        .status()
        .expect("start the Pastey Windows ExecutionWorld verifier");
    assert!(
        status.success(),
        "the product binary did not establish native Windows confinement"
    );
}
