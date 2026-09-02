//! Thin Pastey adapter over the upstream-derived Codex Windows sandbox.
//!
//! Pastey resolves authority, resources, revisions, executable binding,
//! budgets, evidence, and cancellation before this module is called. This
//! module translates those already-authorized values into concrete sandbox
//! mechanics only.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs,
    io::{self, Read, Write},
    os::windows::{io::AsRawHandle, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{Arc, OnceLock},
    time::Duration,
};

use crate::{
    effect_authority::{ConfinementPropertyV1, ExecutionWorldRefV1},
    error::{AppError, AppResult},
    execution_backend::{
        PlatformExecutionBackendV1, PlatformExecutionWorldV1, PlatformProcessLaunchV1,
        PlatformProcessV1, PreparedPlatformWorldV1, SpawnedPlatformProcessV1,
    },
    execution_world::{
        domain_hash, ExecutionWorldAvailabilityV1, PlatformWorldKindV1, EXECUTION_WORLD_VERSION,
    },
    managed_resources::ExecutionWorldMountV1,
    windows_verifier_diagnostics::{production_unavailable_reason, verifier_failure_reason},
};
use codex_windows_sandbox::{
    run_host_setup, setup_is_complete, spawn_sandboxed_process, ProcessHandle, WindowsSandboxLaunch,
};
use tokio::sync::mpsc;

const SETUP_CLI: &str = "--pastey-setup-windows-codex-sandbox-v1";
const VERIFY_CLI: &str = "--pastey-verify-windows-codex-sandbox-v1";
const PROBE_CHILD_CLI: &str = "--pastey-windows-codex-probe-child-v1";
const PROBE_CANCEL_CHILD_CLI: &str = "--pastey-windows-codex-probe-cancel-child-v1";
const BACKEND_VERSION: &str = "pastey-windows-codex-backend-v1";

pub(crate) struct WindowsCodexBackendV1;

impl PlatformExecutionBackendV1 for WindowsCodexBackendV1 {
    fn availability(
        &self,
        required: &BTreeSet<ConfinementPropertyV1>,
    ) -> ExecutionWorldAvailabilityV1 {
        static RESULT: OnceLock<Result<String, String>> = OnceLock::new();
        let result = RESULT.get_or_init(|| {
            let sandbox_home = sandbox_home().map_err(|error| error.to_string())?;
            if !setup_is_complete(&sandbox_home) {
                return Err("Codex Windows sandbox setup is missing or out of date.".into());
            }
            native_conformance_probe(&sandbox_home).map_err(|error| error.to_string())?;
            domain_hash(
                "pastey-windows-codex-execution-world-v1",
                &(BACKEND_VERSION, EXECUTION_WORLD_VERSION, required),
            )
            .map_err(|error| error.to_string())
        });
        match result {
            Ok(identity_digest) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsCodexSandbox,
                available: true,
                identity_digest: identity_digest.clone(),
                verified_properties: required.clone(),
                unavailable_reason: None,
            },
            Err(reason) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsCodexSandbox,
                available: false,
                identity_digest: "pastey-windows-codex-unverified-v1".into(),
                verified_properties: BTreeSet::new(),
                unavailable_reason: Some(production_unavailable_reason(reason)),
            },
        }
    }

    fn prepare_world(
        &self,
        availability: &ExecutionWorldAvailabilityV1,
        _world_ref: &ExecutionWorldRefV1,
        mounts: &[ExecutionWorldMountV1],
    ) -> AppResult<PreparedPlatformWorldV1> {
        if !availability.available || availability.kind != PlatformWorldKindV1::WindowsCodexSandbox
        {
            return Err(AppError::InvalidInput(
                "The Codex Windows execution backend is unavailable.".into(),
            ));
        }
        Ok(PreparedPlatformWorldV1 {
            mounts: mounts.to_vec(),
            world: Arc::new(WindowsCodexWorldV1 {
                sandbox_home: sandbox_home()?,
            }),
        })
    }
}

struct WindowsCodexWorldV1 {
    sandbox_home: PathBuf,
}

impl PlatformExecutionWorldV1 for WindowsCodexWorldV1 {
    fn spawn(&self, launch: PlatformProcessLaunchV1<'_>) -> AppResult<SpawnedPlatformProcessV1> {
        let cwd = launch.cwd.map(Path::to_path_buf).ok_or_else(|| {
            AppError::InvalidInput(
                "The Codex Windows backend requires an authorized working-directory resource."
                    .into(),
            )
        })?;
        let mut read_roots = launch
            .mounts
            .iter()
            .map(|mount| mount.source_path.clone())
            .collect::<Vec<_>>();
        let write_roots = launch
            .mounts
            .iter()
            .filter(|mount| mount.writable)
            .map(|mount| mount.source_path.clone())
            .collect::<Vec<_>>();
        read_roots.push(launch.executable.source_path.clone());
        read_roots.push(cwd.clone());
        read_roots.sort();
        read_roots.dedup();

        let mut command = vec![path_argument(
            &launch.executable.source_path,
            "authorized Windows executable",
        )?];
        command.extend(launch.invocation.argv.iter().cloned());
        let request = WindowsSandboxLaunch {
            sandbox_home: self.sandbox_home.clone(),
            command,
            cwd,
            environment: launch
                .invocation
                .environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            read_roots,
            write_roots,
            stdin_open: launch.invocation.stdin.is_some(),
        };
        let spawned = block_on_codex_spawn(request)?;
        let session = spawned.session;
        let stdin =
            launch.invocation.stdin.is_some().then(|| {
                Box::new(CodexStdinV1::new(session.writer_sender())) as Box<dyn Write + Send>
            });
        Ok(SpawnedPlatformProcessV1 {
            process: Box::new(CodexProcessV1 { session }),
            stdin,
            stdout: Box::new(CodexChannelReaderV1::new(spawned.stdout_rx)),
            stderr: Box::new(CodexChannelReaderV1::new(spawned.stderr_rx)),
        })
    }
}

struct CodexProcessV1 {
    session: ProcessHandle,
}

impl PlatformProcessV1 for CodexProcessV1 {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if !self.session.has_exited() {
            return Ok(None);
        }
        let code = self.session.exit_code().unwrap_or(1) as u32;
        Ok(Some(ExitStatus::from_raw(code)))
    }

    fn request_termination(&mut self) {
        self.session.request_terminate();
    }

    fn close_stdin(&mut self) {
        self.session.close_stdin();
    }
}

struct CodexChannelReaderV1 {
    receiver: mpsc::Receiver<Vec<u8>>,
    buffered: VecDeque<u8>,
}

impl CodexChannelReaderV1 {
    fn new(receiver: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            buffered: VecDeque::new(),
        }
    }
}

impl Read for CodexChannelReaderV1 {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        while self.buffered.is_empty() {
            let Some(chunk) = self.receiver.blocking_recv() else {
                return Ok(0);
            };
            self.buffered.extend(chunk);
        }
        let count = output.len().min(self.buffered.len());
        for byte in output.iter_mut().take(count) {
            *byte = self
                .buffered
                .pop_front()
                .expect("buffer length was checked");
        }
        Ok(count)
    }
}

struct CodexStdinV1 {
    sender: Option<mpsc::Sender<Vec<u8>>>,
}

impl CodexStdinV1 {
    fn new(sender: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            sender: Some(sender),
        }
    }
}

impl Write for CodexStdinV1 {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "sandbox stdin is closed"))?;
        sender.try_send(input.to_vec()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "sandbox stdin queue is unavailable",
            )
        })?;
        Ok(input.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for CodexStdinV1 {
    fn drop(&mut self) {
        self.sender.take();
    }
}

fn block_on_codex_spawn(
    request: WindowsSandboxLaunch,
) -> AppResult<codex_windows_sandbox::SpawnedProcess> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("pastey-windows-codex")
            .build()
            .expect("create Codex Windows sandbox runtime")
    });
    let handle = runtime.handle().clone();
    std::thread::spawn(move || handle.block_on(spawn_sandboxed_process(request)))
        .join()
        .map_err(|_| AppError::InvalidInput("Codex Windows sandbox spawn task panicked.".into()))?
        .map_err(|_| AppError::InvalidInput("Codex Windows sandbox failed to spawn.".into()))
}

fn sandbox_home() -> AppResult<PathBuf> {
    let root = std::env::var_os("PASTEY_APP_DATA_DIR")
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| {
            AppError::InvalidInput("Windows app-data directory is unavailable.".into())
        })?;
    Ok(root.join("windows-codex-sandbox"))
}

fn path_argument(path: &Path, label: &str) -> AppResult<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| AppError::InvalidInput(format!("The {label} path is not valid Unicode.")))
}

fn native_conformance_probe(sandbox_home: &Path) -> AppResult<()> {
    #[cfg(test)]
    let current_exe = std::env::var_os("PASTEY_WINDOWS_NATIVE_VERIFIER_EXE_FOR_TESTS")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    #[cfg(not(test))]
    let current_exe = std::env::current_exe()?;
    let probe_root = sandbox_home.join("pastey-native-probe");
    let input = probe_root.join("input.txt");
    let output = probe_root.join("output");
    fs::create_dir_all(&output)?;
    fs::write(&input, b"probe-input")?;
    let sentinel_file = fs::File::open(&input)?;
    let sentinel = sentinel_file.as_raw_handle() as usize;
    let inheritable = unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(
            sentinel as _,
            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
            windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
        )
    };
    if inheritable == 0 {
        return Err(io::Error::last_os_error().into());
    }
    std::env::set_var("PASTEY_HOST_SECRET_SENTINEL", "must-not-cross");
    let request = WindowsSandboxLaunch {
        sandbox_home: sandbox_home.to_path_buf(),
        command: vec![
            path_argument(&current_exe, "Pastey verifier executable")?,
            PROBE_CHILD_CLI.into(),
            path_argument(&input, "native verifier input")?,
            path_argument(&output, "native verifier output")?,
            sentinel.to_string(),
        ],
        cwd: probe_root.clone(),
        environment: HashMap::from([("PASTEY_CODEX_PROBE".into(), "ok".into())]),
        read_roots: vec![current_exe, input, output.clone(), probe_root.clone()],
        write_roots: vec![output],
        stdin_open: false,
    };
    let spawned = block_on_codex_spawn(request);
    std::env::remove_var("PASTEY_HOST_SECRET_SENTINEL");
    let mut spawned = spawned?;
    let exit = wait_for_codex_exit(&mut spawned.exit_rx, Duration::from_secs(20));
    let cancellation = if matches!(exit, Ok(Ok(0))) {
        probe_cancellation(sandbox_home, &probe_root)
    } else {
        Err(AppError::InvalidInput(
            "Codex Windows native conformance probe failed.".into(),
        ))
    };
    let _ = fs::remove_dir_all(&probe_root);
    cancellation
}

fn wait_for_codex_exit(
    exit_rx: &mut tokio::sync::oneshot::Receiver<i32>,
    maximum: Duration,
) -> Result<Result<i32, tokio::sync::oneshot::error::RecvError>, tokio::time::error::Elapsed> {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let runtime = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("create native verifier runtime")
    });
    runtime.block_on(async { tokio::time::timeout(maximum, exit_rx).await })
}

fn probe_cancellation(sandbox_home: &Path, probe_root: &Path) -> AppResult<()> {
    let current_exe = std::env::current_exe()?;
    let request = WindowsSandboxLaunch {
        sandbox_home: sandbox_home.to_path_buf(),
        command: vec![
            path_argument(&current_exe, "Pastey verifier executable")?,
            PROBE_CANCEL_CHILD_CLI.into(),
        ],
        cwd: probe_root.to_path_buf(),
        environment: HashMap::new(),
        read_roots: vec![current_exe, probe_root.to_path_buf()],
        write_roots: Vec::new(),
        stdin_open: false,
    };
    let mut spawned = block_on_codex_spawn(request)?;
    spawned.session.request_terminate();
    match wait_for_codex_exit(&mut spawned.exit_rx, Duration::from_secs(10)) {
        Ok(Ok(_)) => Ok(()),
        _ => Err(AppError::InvalidInput(
            "Codex Windows cancellation conformance probe failed.".into(),
        )),
    }
}

fn run_probe_child(arguments: &[String]) -> ! {
    let valid = arguments
        .get(2)
        .is_some_and(|input| fs::read(input).ok().as_deref() == Some(b"probe-input"))
        && arguments.get(3).is_some_and(|output| {
            fs::write(Path::new(output).join("probe-output.txt"), b"ok").is_ok()
        })
        && std::env::var("PASTEY_CODEX_PROBE").ok().as_deref() == Some("ok")
        && std::env::var("PASTEY_HOST_SECRET_SENTINEL").is_err()
        && arguments
            .get(4)
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|handle| {
                let mut flags = 0_u32;
                unsafe {
                    windows_sys::Win32::Foundation::GetHandleInformation(handle as _, &mut flags)
                        == 0
                }
            })
        && [
            std::net::SocketAddr::from(([1, 1, 1, 1], 443)),
            std::net::SocketAddr::from(([127, 0, 0, 1], 9)),
        ]
        .into_iter()
        .all(|address| {
            std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))
                .err()
                .and_then(|error| error.raw_os_error())
                == Some(10013)
        });
    std::process::exit(if valid { 0 } else { 90 });
}

/// Pre-Tauri dispatch for explicit Host setup and native verification.
pub(crate) fn run_helper_if_requested() -> bool {
    let arguments = std::env::args().collect::<Vec<_>>();
    match arguments.get(1).map(String::as_str) {
        Some(SETUP_CLI) => {
            let result = sandbox_home().and_then(|home| {
                let username = std::env::var("USERNAME").map_err(|_| {
                    AppError::InvalidInput("Windows Host username is unavailable.".into())
                })?;
                run_host_setup(&home, &username).map_err(|_| {
                    AppError::InvalidInput("Codex Windows sandbox setup failed.".into())
                })
            });
            match &result {
                Ok(()) => println!("PASTEY_WINDOWS_CODEX_SANDBOX_SETUP_OK"),
                Err(error) => eprintln!("PASTEY_WINDOWS_CODEX_SANDBOX_SETUP_FAILED: {error}"),
            }
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
        Some(VERIFY_CLI) => {
            let result = sandbox_home().and_then(|home| native_conformance_probe(&home));
            match &result {
                Ok(()) => println!("PASTEY_WINDOWS_CODEX_SANDBOX_VERIFIED"),
                Err(error) => eprintln!(
                    "PASTEY_WINDOWS_CODEX_SANDBOX_VERIFY_FAILED: {}",
                    verifier_failure_reason(&error.to_string())
                ),
            }
            std::process::exit(if result.is_ok() { 0 } else { 1 });
        }
        Some(PROBE_CHILD_CLI) => run_probe_child(&arguments),
        Some(PROBE_CANCEL_CHILD_CLI) => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_adapter_never_invents_a_fallback_channel() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let mut stdin = CodexStdinV1::new(sender);
        assert!(stdin.write_all(b"no fallback").is_err());
    }
}
