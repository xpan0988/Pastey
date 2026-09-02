//! Thin Pastey adapter over the upstream-derived Codex Windows sandbox.
//!
//! Pastey resolves authority, resources, revisions, executable binding,
//! budgets, evidence, and cancellation before this module is called. This
//! module translates those already-authorized values into concrete sandbox
//! mechanics only.

#[cfg(test)]
use std::sync::Mutex;
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
    windows_verifier_diagnostics::{
        production_unavailable_reason, verifier_failure_reason, NativeProbeDiagnosticReport,
        ProbeNetworkDiagnostic, PROBE_DIAGNOSTIC_FILENAME,
    },
};
use codex_windows_sandbox::{
    run_host_setup, setup_is_complete, spawn_failure_diagnostic, spawn_sandboxed_process,
    ProcessHandle, WindowsSandboxLaunch,
};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT},
    Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
};

const SETUP_CLI: &str = "--pastey-setup-windows-codex-sandbox-v1";
const VERIFY_CLI: &str = "--pastey-verify-windows-codex-sandbox-v1";
const PROBE_CHILD_CLI: &str = "--pastey-windows-codex-probe-child-v1";
const PROBE_CANCEL_CHILD_CLI: &str = "--pastey-windows-codex-probe-cancel-child-v1";
const BACKEND_VERSION: &str = "pastey-windows-codex-backend-v1";
const PROBE_CHILD_EXIT_INPUT_READ: i32 = 91;
const PROBE_CHILD_EXIT_OUTPUT_WRITE: i32 = 92;
const PROBE_CHILD_EXIT_EXPLICIT_ENV: i32 = 93;
const PROBE_CHILD_EXIT_HOST_SECRET: i32 = 94;
const PROBE_CHILD_EXIT_HANDLE_INHERITED: i32 = 95;
const PROBE_CHILD_EXIT_EXTERNAL_NETWORK: i32 = 96;
const PROBE_CHILD_EXIT_LOOPBACK_NETWORK: i32 = 97;
const WSAEACCES: i32 = 10013;
const WSAEPROVIDERFAILEDINIT: i32 = 10106;

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeProbeLaunchDiagnosticForTests {
    pub(crate) phase: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) sanitized_error: String,
    pub(crate) windows_error_codes: String,
    pub(crate) sanitized_messages: String,
    pub(crate) credential_refresh_attempted: String,
    pub(crate) retry_spawn_attempted: String,
    pub(crate) runner_process_created: &'static str,
}

#[cfg(test)]
static LAST_NATIVE_PROBE_LAUNCH_DIAGNOSTIC: OnceLock<
    Mutex<Option<NativeProbeLaunchDiagnosticForTests>>,
> = OnceLock::new();

#[cfg(test)]
pub(crate) fn native_probe_launch_diagnostic_for_tests(
) -> Option<NativeProbeLaunchDiagnosticForTests> {
    LAST_NATIVE_PROBE_LAUNCH_DIAGNOSTIC
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("lock native probe launch diagnostic")
        .clone()
}

#[cfg(test)]
fn clear_native_probe_launch_diagnostic_for_tests() {
    *LAST_NATIVE_PROBE_LAUNCH_DIAGNOSTIC
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("lock native probe launch diagnostic") = None;
}

#[cfg(test)]
fn record_native_probe_launch_failure_for_tests(error: &AppError) {
    let sanitized_error = error.to_string();
    let first_attempt = sanitized_error
        .split("first_attempt=")
        .nth(1)
        .and_then(|value| value.split(';').next())
        .unwrap_or("");
    let runner_process_created = if first_attempt.contains("runner_logon(") {
        "false"
    } else if first_attempt.contains("runner_startup(") {
        "true"
    } else {
        "unknown"
    };
    let diagnostic = NativeProbeLaunchDiagnosticForTests {
        phase: "native_conformance.initial_spawn",
        operation: "spawn_sandboxed_process",
        windows_error_codes: bounded_diagnostic_values(
            &sanitized_error,
            &["CreateProcessWithLogonW_error=", "windows_error_code="],
        ),
        sanitized_messages: bounded_diagnostic_values(&sanitized_error, &["message="]),
        credential_refresh_attempted: bounded_diagnostic_value(
            &sanitized_error,
            "credential_refresh_attempted=",
        ),
        retry_spawn_attempted: bounded_diagnostic_value(&sanitized_error, "retry_spawn_attempted="),
        runner_process_created,
        sanitized_error,
    };
    *LAST_NATIVE_PROBE_LAUNCH_DIAGNOSTIC
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("lock native probe launch diagnostic") = Some(diagnostic);
}

#[cfg(test)]
fn bounded_diagnostic_values(value: &str, markers: &[&str]) -> String {
    let values = markers
        .iter()
        .flat_map(|marker| {
            value
                .match_indices(marker)
                .map(|(index, _)| &value[index + marker.len()..])
        })
        .filter_map(|value| {
            let token = value
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect::<String>();
            (!token.is_empty()).then_some(token)
        })
        .collect::<BTreeSet<_>>();
    if values.is_empty() {
        "none".into()
    } else {
        values.into_iter().collect::<Vec<_>>().join(",")
    }
}

#[cfg(test)]
fn bounded_diagnostic_value(value: &str, marker: &str) -> String {
    bounded_diagnostic_values(value, &[marker])
}

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
        let access = project_codex_access(
            launch
                .mounts
                .iter()
                .map(|mount| (mount.source_path.as_path(), mount.writable)),
            &cwd,
            &launch.executable.source_path,
        );

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
            read_roots: access.read_roots,
            write_roots: access.write_roots,
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

/// Projects Pastey's already-authorized mount access into Codex's permission
/// vocabulary. A Codex write root includes read capability, while a nested
/// Codex read root remains an intentional read-only carveout.
#[derive(Debug, Eq, PartialEq)]
struct CodexAccessProjection {
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProbeFileIdentity {
    volume_serial_number: u32,
    file_index_high: u32,
    file_index_low: u32,
}

struct NativeProbeWorkspace {
    root: PathBuf,
    input: PathBuf,
    output: PathBuf,
    handle_sentinel: PathBuf,
}

impl NativeProbeWorkspace {
    fn create(sandbox_home: &Path) -> io::Result<Self> {
        let root = sandbox_home.join(format!(
            "pastey-native-probe-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let output = root.join("output");
        fs::create_dir_all(&output)?;
        Ok(Self {
            input: root.join("input.txt"),
            handle_sentinel: root.join("handle-inheritance-sentinel"),
            root,
            output,
        })
    }

    fn verify_ready_for_launch(&self) -> AppResult<()> {
        if self.root.is_dir() {
            Ok(())
        } else {
            Err(AppError::InvalidInput(
                "native probe working directory was unavailable before spawn.".into(),
            ))
        }
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn project_codex_access<'a>(
    mounts: impl IntoIterator<Item = (&'a Path, bool)>,
    cwd: &Path,
    executable: &Path,
) -> CodexAccessProjection {
    let mut read_roots = Vec::new();
    let mut write_roots = Vec::new();

    for (path, writable) in mounts {
        if writable {
            write_roots.push(path.to_path_buf());
        } else {
            read_roots.push(path.to_path_buf());
        }
    }
    write_roots.sort();
    write_roots.dedup();

    // An exact duplicate is never an intentional carveout: Write already
    // includes read capability in Codex. Do not remove nested read roots;
    // those encode explicit read-only carveouts under writable parents.
    read_roots.retain(|read_root| !write_roots.iter().any(|write_root| read_root == write_root));

    add_read_support_root(&mut read_roots, &write_roots, cwd);
    add_read_support_root(&mut read_roots, &write_roots, executable);
    read_roots.sort();
    read_roots.dedup();

    CodexAccessProjection {
        read_roots,
        write_roots,
    }
}

fn add_read_support_root(read_roots: &mut Vec<PathBuf>, write_roots: &[PathBuf], path: &Path) {
    if !path_is_covered_by(path, write_roots) && !path_is_covered_by(path, read_roots) {
        read_roots.push(path.to_path_buf());
    }
}

fn path_is_covered_by(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path.starts_with(root))
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
        .map_err(|error| {
            AppError::InvalidInput(format!(
                "Codex Windows sandbox failed to spawn: {}",
                spawn_failure_diagnostic(&error)
            ))
        })
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

fn native_probe_access_projection(
    verifier_executable: &Path,
    input: &Path,
    probe_root: &Path,
    output: &Path,
) -> CodexAccessProjection {
    CodexAccessProjection {
        // The probe root remains available for its working directory and
        // input. Its writable output child is deliberately not also emitted
        // as a Codex read root.
        read_roots: vec![
            verifier_executable.to_path_buf(),
            input.to_path_buf(),
            probe_root.to_path_buf(),
        ],
        write_roots: vec![output.to_path_buf()],
    }
}

fn probe_file_identity(handle: HANDLE) -> io::Result<ProbeFileIdentity> {
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(ProbeFileIdentity {
        volume_serial_number: info.dwVolumeSerialNumber,
        file_index_high: info.nFileIndexHigh,
        file_index_low: info.nFileIndexLow,
    })
}

fn parse_probe_file_identity(arguments: &[String]) -> Option<ProbeFileIdentity> {
    Some(ProbeFileIdentity {
        volume_serial_number: arguments.get(5)?.parse().ok()?,
        file_index_high: arguments.get(6)?.parse().ok()?,
        file_index_low: arguments.get(7)?.parse().ok()?,
    })
}

/// A process-local handle value is not evidence by itself. Only a candidate
/// resolving to this exact Host sentinel file proves inheritance.
fn handle_not_inherited(
    candidate_identity: Option<ProbeFileIdentity>,
    expected_identity: ProbeFileIdentity,
) -> bool {
    candidate_identity != Some(expected_identity)
}

fn native_conformance_probe(sandbox_home: &Path) -> AppResult<()> {
    #[cfg(test)]
    clear_native_probe_launch_diagnostic_for_tests();
    #[cfg(test)]
    let current_exe = std::env::var_os("PASTEY_WINDOWS_NATIVE_VERIFIER_EXE_FOR_TESTS")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    #[cfg(not(test))]
    let current_exe = std::env::current_exe()?;
    let workspace = NativeProbeWorkspace::create(sandbox_home)?;
    let verification = (|| -> AppResult<()> {
        fs::write(&workspace.input, b"probe-input")?;
        let sentinel_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&workspace.handle_sentinel)?;
        let sentinel = sentinel_file.as_raw_handle() as usize;
        let sentinel_identity = probe_file_identity(sentinel as HANDLE)?;
        let inheritable = unsafe {
            SetHandleInformation(sentinel as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        };
        if inheritable == 0 {
            return Err(io::Error::last_os_error().into());
        }

        std::env::set_var("PASTEY_HOST_SECRET_SENTINEL", "must-not-cross");
        let spawned = (|| -> AppResult<_> {
            workspace.verify_ready_for_launch()?;
            let command = vec![
                path_argument(&current_exe, "Pastey verifier executable")?,
                PROBE_CHILD_CLI.into(),
                path_argument(&workspace.input, "native verifier input")?,
                path_argument(&workspace.output, "native verifier output")?,
                sentinel.to_string(),
                sentinel_identity.volume_serial_number.to_string(),
                sentinel_identity.file_index_high.to_string(),
                sentinel_identity.file_index_low.to_string(),
            ];
            let access = native_probe_access_projection(
                &current_exe,
                &workspace.input,
                &workspace.root,
                &workspace.output,
            );
            let spawned = block_on_codex_spawn(WindowsSandboxLaunch {
                sandbox_home: sandbox_home.to_path_buf(),
                command,
                cwd: workspace.root.clone(),
                environment: HashMap::from([("PASTEY_CODEX_PROBE".into(), "ok".into())]),
                read_roots: access.read_roots,
                write_roots: access.write_roots,
                stdin_open: false,
            });
            #[cfg(test)]
            if let Err(error) = &spawned {
                record_native_probe_launch_failure_for_tests(error);
            }
            spawned
        })();
        std::env::remove_var("PASTEY_HOST_SECRET_SENTINEL");

        let mut spawned = spawned.map_err(|error| {
            AppError::InvalidInput(format!("native probe spawn failed: {error}"))
        })?;
        match wait_for_codex_exit(&mut spawned.exit_rx, Duration::from_secs(20)) {
            Ok(Ok(0)) => probe_cancellation(sandbox_home, &workspace.root),
            Ok(Ok(exit_code)) => Err(AppError::InvalidInput(format!(
                "native probe child exited {exit_code}: {}",
                child_exit_summary(exit_code, &workspace.output)
            ))),
            Ok(Err(_)) => Err(AppError::InvalidInput(
                "native probe exit channel failed.".into(),
            )),
            Err(_) => Err(AppError::InvalidInput(
                "native probe timed out after 20 seconds.".into(),
            )),
        }
    })();
    workspace.cleanup();
    verification
}

fn child_exit_summary(exit_code: i32, output: &Path) -> String {
    fs::read_to_string(output.join(PROBE_DIAGNOSTIC_FILENAME))
        .ok()
        .and_then(|report| NativeProbeDiagnosticReport::parse(&report))
        .map(|report| report.summary())
        .unwrap_or_else(|| match exit_code {
            PROBE_CHILD_EXIT_INPUT_READ => "authorized input was unreadable or mismatched".into(),
            PROBE_CHILD_EXIT_OUTPUT_WRITE => {
                "authorized output directory was not writable; no diagnostic report is trusted"
                    .into()
            }
            PROBE_CHILD_EXIT_EXPLICIT_ENV => "PASTEY_CODEX_PROBE was not ok".into(),
            PROBE_CHILD_EXIT_HOST_SECRET => "Host secret sentinel was present".into(),
            PROBE_CHILD_EXIT_HANDLE_INHERITED => "inherited Host handle remained accessible".into(),
            PROBE_CHILD_EXIT_EXTERNAL_NETWORK => "external network confinement failed".into(),
            PROBE_CHILD_EXIT_LOOPBACK_NETWORK => "loopback network confinement failed".into(),
            _ => "no valid diagnostic report was available".into(),
        })
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
    let current_exe = std::env::current_exe().map_err(|_| {
        AppError::InvalidInput(
            "native probe cancellation-probe executable resolution failed.".into(),
        )
    })?;
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
    let mut spawned = block_on_codex_spawn(request).map_err(|_| {
        AppError::InvalidInput("native probe cancellation-probe spawn failed.".into())
    })?;
    spawned.session.request_terminate();
    match wait_for_codex_exit(&mut spawned.exit_rx, Duration::from_secs(10)) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(AppError::InvalidInput(
            "native probe cancellation-probe exit channel failed.".into(),
        )),
        Err(_) => Err(AppError::InvalidInput(
            "native probe cancellation-probe timed out after 10 seconds.".into(),
        )),
    }
}

fn run_probe_child(arguments: &[String]) -> ! {
    let input_read = arguments
        .get(2)
        .is_some_and(|input| fs::read(input).ok().as_deref() == Some(b"probe-input"));
    let output = arguments.get(3).map(PathBuf::from);
    let output_write = output
        .as_ref()
        .is_some_and(|output| fs::write(output.join("probe-output.txt"), b"ok").is_ok());
    let explicit_env = std::env::var("PASTEY_CODEX_PROBE").ok().as_deref() == Some("ok");
    let host_secret_absent = std::env::var("PASTEY_HOST_SECRET_SENTINEL").is_err();
    let candidate_identity = arguments
        .get(4)
        .and_then(|value| value.parse::<usize>().ok())
        .and_then(|handle| probe_file_identity(handle as HANDLE).ok());
    let handle_not_inherited = parse_probe_file_identity(arguments)
        .is_some_and(|expected| handle_not_inherited(candidate_identity, expected));
    let external_network = probe_network(std::net::SocketAddr::from(([1, 1, 1, 1], 443)));
    let loopback_network = probe_network(std::net::SocketAddr::from(([127, 0, 0, 1], 9)));

    if !output_write {
        std::process::exit(PROBE_CHILD_EXIT_OUTPUT_WRITE);
    }

    let report = NativeProbeDiagnosticReport {
        input_read,
        output_write,
        explicit_env,
        host_secret_absent,
        handle_not_inherited,
        external_network,
        loopback_network,
    };
    if let Some(output) = output {
        let _ = fs::write(output.join(PROBE_DIAGNOSTIC_FILENAME), report.render());
    }

    let exit_code = if !input_read {
        PROBE_CHILD_EXIT_INPUT_READ
    } else if !explicit_env {
        PROBE_CHILD_EXIT_EXPLICIT_ENV
    } else if !host_secret_absent {
        PROBE_CHILD_EXIT_HOST_SECRET
    } else if !handle_not_inherited {
        PROBE_CHILD_EXIT_HANDLE_INHERITED
    } else if !network_is_denied(external_network) {
        PROBE_CHILD_EXIT_EXTERNAL_NETWORK
    } else if !network_is_denied(loopback_network) {
        PROBE_CHILD_EXIT_LOOPBACK_NETWORK
    } else {
        0
    };
    std::process::exit(exit_code);
}

fn probe_network(address: std::net::SocketAddr) -> ProbeNetworkDiagnostic {
    match std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500)) {
        Ok(_) => ProbeNetworkDiagnostic::Connected,
        Err(error) => ProbeNetworkDiagnostic::Denied(error.raw_os_error()),
    }
}

fn network_is_denied(result: ProbeNetworkDiagnostic) -> bool {
    matches!(
        result,
        ProbeNetworkDiagnostic::Denied(Some(WSAEACCES | WSAEPROVIDERFAILEDINIT))
    )
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

    fn probe_file_identity_for_test(
        volume_serial_number: u32,
        file_index_high: u32,
        file_index_low: u32,
    ) -> ProbeFileIdentity {
        ProbeFileIdentity {
            volume_serial_number,
            file_index_high,
            file_index_low,
        }
    }

    fn native_probe_test_sandbox_home() -> PathBuf {
        std::env::temp_dir().join(format!(
            "pastey-native-probe-workspace-test-{}",
            uuid::Uuid::new_v4()
        ))
    }

    fn project_for_test(
        mounts: &[(&str, bool)],
        cwd: &str,
        executable: &str,
    ) -> CodexAccessProjection {
        project_codex_access(
            mounts
                .iter()
                .map(|(path, writable)| (Path::new(path), *writable)),
            Path::new(cwd),
            Path::new(executable),
        )
    }

    #[test]
    fn writable_mount_projects_only_a_codex_write_root() {
        let root = r"C:\sandbox\workspace";
        let access = project_for_test(&[(root, true)], root, r"C:\sandbox\workspace\tool.exe");

        assert_eq!(access.write_roots, vec![PathBuf::from(root)]);
        assert!(access.read_roots.is_empty());
    }

    #[test]
    fn read_only_mount_projects_a_codex_read_root() {
        let root = r"C:\sandbox\input";
        let access = project_for_test(&[(root, false)], root, r"C:\sandbox\input\tool.exe");

        assert_eq!(access.read_roots, vec![PathBuf::from(root)]);
        assert!(access.write_roots.is_empty());
    }

    #[test]
    fn nested_read_only_mount_remains_a_carveout_under_writable_parent() {
        let parent = r"C:\sandbox\workspace";
        let child = r"C:\sandbox\workspace\input";
        let access = project_for_test(
            &[(parent, true), (child, false)],
            r"C:\sandbox\workspace\run",
            r"C:\sandbox\workspace\tool.exe",
        );

        assert_eq!(access.write_roots, vec![PathBuf::from(parent)]);
        assert_eq!(access.read_roots, vec![PathBuf::from(child)]);
    }

    #[test]
    fn cwd_equal_to_writable_root_does_not_add_a_read_carveout() {
        let root = r"C:\sandbox\workspace";
        let access = project_for_test(&[(root, true)], root, r"C:\sandbox\workspace\tool.exe");

        assert!(!access.read_roots.contains(&PathBuf::from(root)));
    }

    #[test]
    fn cwd_inside_writable_root_does_not_make_its_subtree_read_only() {
        let root = r"C:\sandbox\workspace";
        let cwd = r"C:\sandbox\workspace\nested\run";
        let access = project_for_test(&[(root, true)], cwd, r"C:\sandbox\workspace\tool.exe");

        assert!(!access.read_roots.contains(&PathBuf::from(cwd)));
        assert!(access.read_roots.is_empty());
    }

    #[test]
    fn executable_inside_writable_root_does_not_add_a_read_carveout() {
        let root = r"C:\sandbox\workspace";
        let executable = r"C:\sandbox\workspace\bin\worker.exe";
        let access = project_for_test(&[(root, true)], r"C:\sandbox\workspace\run", executable);

        assert!(!access.read_roots.contains(&PathBuf::from(executable)));
        assert!(access.read_roots.is_empty());
    }

    #[test]
    fn native_probe_output_is_not_explicitly_read_and_writable() {
        let output = PathBuf::from(r"C:\sandbox\probe\output");
        let access = native_probe_access_projection(
            Path::new(r"C:\pastey\pastey.exe"),
            Path::new(r"C:\sandbox\probe\input.txt"),
            Path::new(r"C:\sandbox\probe"),
            &output,
        );

        assert_eq!(access.write_roots, vec![output.clone()]);
        assert!(!access.read_roots.contains(&output));
    }

    #[test]
    fn test_only_launch_diagnostic_preserves_bounded_runner_spawn_classification() {
        clear_native_probe_launch_diagnostic_for_tests();
        let error = AppError::InvalidInput(
            "Codex Windows sandbox failed to spawn: first_attempt=runner_startup(stage=SpawnChild, message=CreateProcessAsUserW_failed, windows_error_code=1312); credential_refresh_attempted=true; credential_refresh=completed; retry_spawn_attempted=true; retry_failure=runner_logon(CreateProcessWithLogonW_error=1326)".into(),
        );

        record_native_probe_launch_failure_for_tests(&error);

        assert_eq!(
            native_probe_launch_diagnostic_for_tests(),
            Some(NativeProbeLaunchDiagnosticForTests {
                phase: "native_conformance.initial_spawn",
                operation: "spawn_sandboxed_process",
                sanitized_error: error.to_string(),
                windows_error_codes: "1312,1326".into(),
                sanitized_messages: "CreateProcessAsUserW_failed".into(),
                credential_refresh_attempted: "true".into(),
                retry_spawn_attempted: "true".into(),
                runner_process_created: "true",
            })
        );
    }

    #[test]
    fn native_probe_workspaces_have_distinct_invocation_roots() {
        let sandbox_home = native_probe_test_sandbox_home();
        let first = NativeProbeWorkspace::create(&sandbox_home).unwrap();
        let second = NativeProbeWorkspace::create(&sandbox_home).unwrap();

        assert_ne!(first.root, second.root);
        assert!(first.root.starts_with(&sandbox_home));
        assert!(second.root.starts_with(&sandbox_home));

        first.cleanup();
        second.cleanup();
        let _ = fs::remove_dir_all(sandbox_home);
    }

    #[test]
    fn cleanup_of_one_native_probe_workspace_cannot_remove_another() {
        let sandbox_home = native_probe_test_sandbox_home();
        let first = NativeProbeWorkspace::create(&sandbox_home).unwrap();
        let second = NativeProbeWorkspace::create(&sandbox_home).unwrap();

        first.cleanup();

        assert!(!first.root.exists());
        assert!(second.root.is_dir());

        second.cleanup();
        let _ = fs::remove_dir_all(sandbox_home);
    }

    #[test]
    fn native_probe_workspace_exists_immediately_before_launch() {
        let sandbox_home = native_probe_test_sandbox_home();
        let workspace = NativeProbeWorkspace::create(&sandbox_home).unwrap();

        assert!(workspace.verify_ready_for_launch().is_ok());

        workspace.cleanup();
        let _ = fs::remove_dir_all(sandbox_home);
    }

    #[test]
    fn native_probe_resources_stay_within_their_invocation_root() {
        let sandbox_home = native_probe_test_sandbox_home();
        let workspace = NativeProbeWorkspace::create(&sandbox_home).unwrap();

        assert!(workspace.input.starts_with(&workspace.root));
        assert!(workspace.output.starts_with(&workspace.root));
        assert!(workspace.handle_sentinel.starts_with(&workspace.root));
        assert!(workspace.output.is_dir());

        workspace.cleanup();
        let _ = fs::remove_dir_all(sandbox_home);
    }

    #[test]
    fn native_probe_cleanup_is_best_effort_and_does_not_touch_other_workspaces() {
        let sandbox_home = native_probe_test_sandbox_home();
        let failed_probe = NativeProbeWorkspace::create(&sandbox_home).unwrap();
        let active_probe = NativeProbeWorkspace::create(&sandbox_home).unwrap();

        failed_probe.cleanup();

        assert!(!failed_probe.root.exists());
        assert!(active_probe.verify_ready_for_launch().is_ok());

        active_probe.cleanup();
        let _ = fs::remove_dir_all(sandbox_home);
    }

    #[test]
    fn invalid_candidate_handle_is_not_the_host_sentinel() {
        let sentinel = probe_file_identity_for_test(1, 2, 3);

        assert!(handle_not_inherited(None, sentinel));
    }

    #[test]
    fn different_file_object_is_not_the_host_sentinel() {
        let sentinel = probe_file_identity_for_test(1, 2, 3);
        let different_file = probe_file_identity_for_test(1, 2, 4);

        assert!(handle_not_inherited(Some(different_file), sentinel));
    }

    #[test]
    fn same_sentinel_file_identity_proves_handle_inheritance() {
        let sentinel = probe_file_identity_for_test(1, 2, 3);

        assert!(!handle_not_inherited(Some(sentinel), sentinel));
    }

    #[test]
    fn numeric_handle_collision_with_a_different_object_does_not_fail_the_verifier() {
        let sentinel = probe_file_identity_for_test(1, 2, 3);
        // The same process-local numeric handle value can resolve to a
        // different object in the child. Identity, not that number, decides.
        let colliding_child_object = probe_file_identity_for_test(9, 8, 7);

        assert!(handle_not_inherited(Some(colliding_child_object), sentinel));
    }

    #[test]
    fn stdin_adapter_never_invents_a_fallback_channel() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let mut stdin = CodexStdinV1::new(sender);
        assert!(stdin.write_all(b"no fallback").is_err());
    }

    #[test]
    fn child_exit_fallbacks_identify_every_conformance_invariant() {
        let missing_output = Path::new("pastey-no-native-probe-diagnostics");
        for (exit_code, expected) in [
            (
                PROBE_CHILD_EXIT_INPUT_READ,
                "authorized input was unreadable or mismatched",
            ),
            (
                PROBE_CHILD_EXIT_OUTPUT_WRITE,
                "authorized output directory was not writable; no diagnostic report is trusted",
            ),
            (
                PROBE_CHILD_EXIT_EXPLICIT_ENV,
                "PASTEY_CODEX_PROBE was not ok",
            ),
            (
                PROBE_CHILD_EXIT_HOST_SECRET,
                "Host secret sentinel was present",
            ),
            (
                PROBE_CHILD_EXIT_HANDLE_INHERITED,
                "inherited Host handle remained accessible",
            ),
            (
                PROBE_CHILD_EXIT_EXTERNAL_NETWORK,
                "external network confinement failed",
            ),
            (
                PROBE_CHILD_EXIT_LOOPBACK_NETWORK,
                "loopback network confinement failed",
            ),
        ] {
            assert_eq!(child_exit_summary(exit_code, missing_output), expected);
        }
    }

    #[test]
    fn network_confinement_accepts_only_documented_windows_denials() {
        assert!(!network_is_denied(ProbeNetworkDiagnostic::Connected));
        assert!(network_is_denied(ProbeNetworkDiagnostic::Denied(Some(
            WSAEACCES
        ))));
        assert!(network_is_denied(ProbeNetworkDiagnostic::Denied(Some(
            WSAEPROVIDERFAILEDINIT
        ))));
        assert!(!network_is_denied(ProbeNetworkDiagnostic::Denied(Some(
            10061
        ))));
        assert!(!network_is_denied(ProbeNetworkDiagnostic::Denied(None)));
    }
}
