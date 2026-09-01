//! Native Windows restricted-principal execution backend.
//!
//! Machine setup is an explicit elevated operation. It reconciles one local
//! offline sandbox account, a Host-private DPAPI credential record, protected
//! Pastey run roots, and an outbound Windows Firewall rule scoped to that
//! account. Per-run launch is non-elevated: a trusted Pastey bootstrap is
//! logged on as the sandbox account, placed in a kill-on-close Job before its
//! first instruction, and proves its fresh logon SID over a private pipe. The
//! Host then ACLs only that run root to the logon SID. The bootstrap creates
//! the Worker with a write-restricted token restricted to the same logon SID,
//! an explicit stdio-only handle list, a private desktop, and a bounded
//! synthesized environment. AppContainer is intentionally not used.

#![cfg(windows)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{c_void, OsStr},
    fs::{self, File},
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::ExitStatus,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use parking_lot::Mutex;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use windows::{
    core::{Interface, BSTR},
    Win32::{
        Foundation::{RPC_E_CHANGED_MODE, VARIANT_TRUE},
        NetworkManagement::WindowsFirewall::{
            INetFwPolicy2, INetFwRule3, NetFwPolicy2, NetFwRule, NET_FW_ACTION_BLOCK,
            NET_FW_IP_PROTOCOL_ANY, NET_FW_MODIFY_STATE_OK, NET_FW_PROFILE2_ALL,
            NET_FW_PROFILE2_DOMAIN, NET_FW_PROFILE2_PRIVATE, NET_FW_PROFILE2_PUBLIC,
            NET_FW_RULE_DIR_OUT,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
    },
};
use windows_sys::Win32::{
    Foundation::{
        GetHandleInformation, GetLastError, LocalFree, SetHandleInformation,
        ERROR_INSUFFICIENT_BUFFER, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING,
        GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, HLOCAL, INVALID_HANDLE_VALUE,
        STILL_ACTIVE,
    },
    NetworkManagement::NetManagement::{
        NERR_Success, NERR_UserNotFound, NetApiBufferFree, NetUserAdd, NetUserGetInfo,
        NetUserGetLocalGroups, NetUserSetInfo, LG_INCLUDE_INDIRECT, MAX_PREFERRED_LENGTH,
        UF_DONT_EXPIRE_PASSWD, UF_NORMAL_ACCOUNT, UF_NOT_DELEGATED, UF_PASSWD_CANT_CHANGE,
        UF_SCRIPT, USER_INFO_1, USER_INFO_1003, USER_INFO_1008, USER_INFO_4, USER_PRIV_ADMIN,
        USER_PRIV_GUEST, USER_PRIV_USER,
    },
    Security::{
        AllocateAndInitializeSid,
        Authentication::Identity::{
            LsaAddAccountRights, LsaClose, LsaNtStatusToWinError, LsaOpenPolicy, LSA_HANDLE,
            LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING, POLICY_CREATE_ACCOUNT, POLICY_LOOKUP_NAMES,
        },
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
            SDDL_REVISION_1, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN,
        },
        CheckTokenMembership, CreateRestrictedToken,
        Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE,
            CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
        FreeSid, GetLengthSid, GetTokenInformation, IsTokenRestricted, IsValidSid, LogonUserW,
        TokenGroups, TokenUser, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        DISABLE_MAX_PRIVILEGE, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LUA_TOKEN,
        OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, SECURITY_NT_AUTHORITY, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT,
        TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY,
        TOKEN_USER, WRITE_RESTRICTED,
    },
    Storage::FileSystem::{
        CreateFileW, MoveFileExW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
        FILE_TRAVERSE, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND, READ_CONTROL, SYNCHRONIZE,
        WRITE_DAC,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
            JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
            JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
            JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
            JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        },
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_NOWAIT,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        },
        StationsAndDesktops::{CloseDesktop, CreateDesktopW, HDESK},
        SystemInformation::GetWindowsDirectoryW,
        SystemServices::SE_GROUP_LOGON_ID,
        Threading::{
            CreateProcessAsUserW, CreateProcessWithLogonW, DeleteProcThreadAttributeList,
            GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
            OpenProcessToken, ResumeThread, TerminateProcess, UpdateProcThreadAttribute,
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
        },
    },
    UI::Shell::{SHGetFolderPathW, CSIDL_COMMON_APPDATA},
};

use crate::{
    effect_authority::{
        ConfinementPropertyV1, ExecutionWorldRefV1, ResourceHandleRefV1, ResourceKindV1,
    },
    error::{AppError, AppResult},
    execution_world::{ExecutionWorldAvailabilityV1, PlatformWorldKindV1, EXECUTION_WORLD_VERSION},
    managed_resources::ExecutionWorldMountV1,
    windows_setup_state::{
        select_setup_plan, validate_exact_local_user_identity, LegacyPrivilegeClassV1,
        RecoveryEvidenceV1, RecoveryStageV1, SetupEvidenceV1, SetupPlanV1,
    },
};

const WINDOWS_WORLD_ADAPTER_VERSION: &str = "pastey-windows-restricted-principal-job-v1";
const SETUP_VERSION: u32 = 1;
const SANDBOX_USERNAME: &str = "PasteySandboxOffline";
const FIREWALL_RULE_NAME: &str = "pastey_sandbox_offline_block_outbound_v1";
const FIREWALL_RULE_DESCRIPTION: &str = "Pastey Sandbox Offline - Block All Outbound";
const SETUP_CLI: &str = "--pastey-setup-windows-execution-world-v1";
const VERIFICATION_CLI: &str = "--pastey-verify-windows-execution-world-v1";
const RUNNER_CLI: &str = "--pastey-windows-world-runner-v1";
const PROBE_PARENT: &str = "--pastey-windows-world-probe-parent-v2";
const PROBE_CHILD: &str = "--pastey-windows-world-probe-child-v2";
pub(crate) const TEST_BASIC: &str = "--pastey-windows-world-test-basic-v1";
pub(crate) const TEST_SLEEP: &str = "--pastey-windows-world-test-sleep-v1";
pub(crate) const TEST_SPAM: &str = "--pastey-windows-world-test-spam-v1";
const MAX_JOB_PROCESSES: u32 = 16;
const MAX_CONTROL_FRAME: usize = 512 * 1024;
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const CONTROL_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DESKTOP_GENERIC_ALL: u32 = 0x1000_0000;
const ACCOUNT_SETUP_FLAGS: u32 = UF_SCRIPT
    | UF_NORMAL_ACCOUNT
    | UF_DONT_EXPIRE_PASSWD
    | UF_PASSWD_CANT_CHANGE
    | UF_NOT_DELEGATED;
const PRIVILEGED_LOCAL_GROUP_SIDS: &[&str] = &[
    "S-1-5-32-544", // Administrators
    "S-1-5-32-547", // Power Users
    "S-1-5-32-548", // Account Operators
    "S-1-5-32-549", // Server Operators
    "S-1-5-32-550", // Print Operators
    "S-1-5-32-551", // Backup Operators
    "S-1-5-32-552", // Replicators
    "S-1-5-32-555", // Remote Desktop Users
    "S-1-5-32-556", // Network Configuration Operators
    "S-1-5-32-578", // Hyper-V Administrators
    "S-1-5-32-580", // Remote Management Users
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupMarkerV1 {
    version: u32,
    username: String,
    account_sid: String,
    host_sid: String,
    encrypted_password: String,
    managed_root: PathBuf,
    runner_root: PathBuf,
    firewall_rule_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupRecoveryV1 {
    version: u32,
    username: String,
    host_sid: String,
    encrypted_password: String,
    managed_root: PathBuf,
    runner_root: PathBuf,
    firewall_rule_name: String,
    stage: RecoveryStageV1,
    account_sid: Option<String>,
}

#[derive(Clone, Debug)]
struct SetupPathsV1 {
    base: PathBuf,
    secrets: PathBuf,
    marker: PathBuf,
    recovery: PathBuf,
    managed_root: PathBuf,
    runner_root: PathBuf,
}

#[derive(Clone, Debug)]
struct WindowsSetupV1 {
    marker: SetupMarkerV1,
    account_sid: Vec<u8>,
    host_sid: Vec<u8>,
    system_sid: Vec<u8>,
    administrators_sid: Vec<u8>,
    owner_rights_sid: Vec<u8>,
    paths: SetupPathsV1,
}

#[derive(Clone, Debug)]
struct LocalAccountV1 {
    name: String,
    sid: Vec<u8>,
    sid_string: String,
    comment: String,
    flags: u32,
    home_dir: String,
    script_path: String,
}

impl LocalAccountV1 {
    fn matches_legacy_partial_fingerprint(&self) -> bool {
        self.name.eq_ignore_ascii_case(SANDBOX_USERNAME)
            && self.comment == "Pastey managed Worker offline sandbox principal"
            && self.flags == ACCOUNT_SETUP_FLAGS
            && self.home_dir.is_empty()
            && self.script_path.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunnerHelloV1 {
    version: String,
    logon_sid: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunnerLaunchV1 {
    version: String,
    executable: PathBuf,
    argv: Vec<String>,
    environment: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    stdin_pipe: String,
    stdout_pipe: String,
    stderr_pipe: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunnerReadyV1 {
    version: String,
    child_pid: u32,
    restricted: bool,
    contained: bool,
}

pub(crate) struct WindowsWorldV1 {
    root: PathBuf,
    input_root: PathBuf,
    scratch_root: PathBuf,
    output_root: PathBuf,
    tools_root: PathBuf,
    setup: Arc<WindowsSetupV1>,
    active: AtomicBool,
    acl_lock: Mutex<()>,
}

impl Drop for WindowsWorldV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct PrivateDesktopV1(HDESK);

unsafe impl Send for PrivateDesktopV1 {}
unsafe impl Sync for PrivateDesktopV1 {}

impl Drop for PrivateDesktopV1 {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseDesktop(self.0) };
        }
    }
}

pub(crate) struct WindowsProcessV1 {
    process: OwnedHandle,
    job: OwnedHandle,
    world: Arc<WindowsWorldV1>,
}

impl WindowsProcessV1 {
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        use std::os::windows::process::ExitStatusExt;
        let mut code = 0_u32;
        if unsafe { GetExitCodeProcess(raw(&self.process), &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if code == STILL_ACTIVE as u32 {
            Ok(None)
        } else {
            Ok(Some(ExitStatus::from_raw(code)))
        }
    }

    pub(crate) fn terminate_tree(&mut self) -> bool {
        let result = unsafe { TerminateJobObject(raw(&self.job), 0xC000_013Au32) };
        result != 0 || self.try_wait().ok().flatten().is_some()
    }
}

impl Drop for WindowsProcessV1 {
    fn drop(&mut self) {
        let _ = unsafe { TerminateJobObject(raw(&self.job), 0xC000_013Au32) };
        self.world.active.store(false, Ordering::Release);
    }
}

pub(crate) struct WindowsSpawnedProcessV1 {
    pub(crate) process: WindowsProcessV1,
    pub(crate) stdin: Option<File>,
    pub(crate) stdout: File,
    pub(crate) stderr: File,
}

struct ControlPipeServerV1 {
    handle: OwnedHandle,
    name: String,
}

struct StdioPipeServersV1 {
    stdin: OwnedHandle,
    stdout: OwnedHandle,
    stderr: OwnedHandle,
    stdin_name: String,
    stdout_name: String,
    stderr_name: String,
}

struct AttributeListV1 {
    storage: Vec<usize>,
    ptr: *mut c_void,
}

impl AttributeListV1 {
    fn new(count: u32) -> AppResult<Self> {
        let mut bytes = 0_usize;
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut bytes) };
        if bytes == 0 {
            return unavailable("Windows process attribute sizing failed.");
        }
        let words = bytes.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let ptr = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(ptr, count, 0, &mut bytes) } == 0 {
            return unavailable("Windows process attribute initialization failed.");
        }
        Ok(Self { storage, ptr })
    }

    fn update(&mut self, attribute: usize, value: *const c_void, bytes: usize) -> AppResult<()> {
        let _ = self.storage.len();
        if unsafe {
            UpdateProcThreadAttribute(
                self.ptr,
                0,
                attribute,
                value,
                bytes,
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return unavailable("Windows process attribute installation failed.");
        }
        Ok(())
    }
}

impl Drop for AttributeListV1 {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.ptr) };
    }
}

struct SecurityDescriptorV1(PSECURITY_DESCRIPTOR);

impl SecurityDescriptorV1 {
    fn from_sddl(sddl: &str) -> AppResult<Self> {
        let sddl = wide(sddl);
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return unavailable("Windows could not construct a private security descriptor.");
        }
        Ok(Self(descriptor))
    }

    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptorV1 {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0 as HLOCAL) };
        }
    }
}

pub(crate) fn availability(
    required: BTreeSet<ConfinementPropertyV1>,
) -> ExecutionWorldAvailabilityV1 {
    static AVAILABILITY: OnceLock<ExecutionWorldAvailabilityV1> = OnceLock::new();
    AVAILABILITY
        .get_or_init(|| match native_conformance_probe(&required) {
            Ok(identity_digest) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsRestrictedPrincipal,
                available: true,
                identity_digest,
                verified_properties: required,
                unavailable_reason: None,
            },
            Err(_) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsRestrictedPrincipal,
                available: false,
                identity_digest: "pastey-windows-restricted-principal-unverified-v1".into(),
                verified_properties: BTreeSet::new(),
                unavailable_reason: Some(
                    "The elevated PasteySandboxOffline setup or native restricted-token, ACL, Firewall, handle-list, Job, descendant, filesystem, and NoRawNetwork conformance probe did not complete successfully."
                        .into(),
                ),
            },
        })
        .clone()
}

pub(crate) fn prepare_world(
    world_ref: &ExecutionWorldRefV1,
    mounts: &[ExecutionWorldMountV1],
) -> AppResult<(Vec<ExecutionWorldMountV1>, Arc<WindowsWorldV1>)> {
    let setup = Arc::new(load_setup()?);
    ensure_runner_binary(&setup)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let root = setup.paths.managed_root.join(format!(
        "run-{}-{}",
        short_hash(world_ref.as_str().as_bytes()),
        nonce
    ));
    fs::create_dir(&root)?;
    set_exact_dacl(&root, &host_only_entries(&setup), true)?;
    let world = Arc::new(WindowsWorldV1 {
        input_root: root.join("input"),
        scratch_root: root.join("scratch"),
        output_root: root.join("output"),
        tools_root: root.join("tools"),
        root,
        setup,
        active: AtomicBool::new(false),
        acl_lock: Mutex::new(()),
    });
    for directory in [
        &world.input_root,
        &world.scratch_root,
        &world.output_root,
        &world.tools_root,
    ] {
        fs::create_dir(directory)?;
        set_exact_dacl(directory, &host_only_entries(&world.setup), true)?;
    }
    for directory in [
        world.scratch_root.join("temp"),
        world.scratch_root.join("profile"),
        world
            .scratch_root
            .join("profile")
            .join("AppData")
            .join("Roaming"),
        world
            .scratch_root
            .join("profile")
            .join("AppData")
            .join("Local"),
    ] {
        fs::create_dir_all(&directory)?;
    }
    let prepared = stage_mounts(&world, mounts)?;
    Ok((prepared, world))
}

fn stage_mounts(
    world: &WindowsWorldV1,
    mounts: &[ExecutionWorldMountV1],
) -> AppResult<Vec<ExecutionWorldMountV1>> {
    let mut prepared = Vec::with_capacity(mounts.len());
    for mount in mounts {
        if mount.kind == ResourceKindV1::Secret {
            return unavailable("Windows Worker process worlds do not project secret resources.");
        }
        let category = match mount.kind {
            ResourceKindV1::Executable => &world.tools_root,
            ResourceKindV1::OutputSlot => &world.output_root,
            ResourceKindV1::Scratch => &world.scratch_root,
            ResourceKindV1::Workspace if mount.writable => &world.scratch_root,
            _ => &world.input_root,
        };
        let mount_root = category.join(&mount.mount_name);
        fs::create_dir(&mount_root)?;
        let source_metadata = fs::symlink_metadata(&mount.source_path)?;
        reject_reparse(&source_metadata)?;
        let destination = if source_metadata.is_file() {
            let file_name = mount.source_path.file_name().ok_or_else(|| {
                AppError::InvalidInput("Windows staged file name is unavailable.".into())
            })?;
            mount_root.join(file_name)
        } else {
            mount_root.join("root")
        };
        copy_without_reparse(&mount.source_path, &destination, mount.quota_bytes)?;
        let mut staged = mount.clone();
        staged.source_path = destination.canonicalize()?;
        prepared.push(staged);
    }
    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    world: Arc<WindowsWorldV1>,
    executable: &Path,
    argv: &[String],
    environment: &BTreeMap<String, String>,
    stdin_requested: bool,
    cwd: Option<&Path>,
    cpu_millis: u64,
    memory_bytes: u64,
) -> AppResult<WindowsSpawnedProcessV1> {
    if cpu_millis == 0 || memory_bytes < 1024 * 1024 {
        return unavailable("Reserved Windows process limits are unsafe.");
    }
    if world
        .active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return unavailable("A Windows run world already owns an active process tree.");
    }
    let result = spawn_inner(
        world.clone(),
        executable,
        argv,
        environment,
        stdin_requested,
        cwd,
        cpu_millis,
        memory_bytes,
    );
    if result.is_err() {
        world.active.store(false, Ordering::Release);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn spawn_inner(
    world: Arc<WindowsWorldV1>,
    executable: &Path,
    argv: &[String],
    environment: &BTreeMap<String, String>,
    stdin_requested: bool,
    cwd: Option<&Path>,
    cpu_millis: u64,
    memory_bytes: u64,
) -> AppResult<WindowsSpawnedProcessV1> {
    let acl_guard = world.acl_lock.lock();
    let job = create_job(cpu_millis, memory_bytes)?;
    let control_pipe = create_control_pipe_server(&world.setup)?;
    let runner_path = runner_binary_path(&world.setup)?;
    let (runner, runner_thread) = spawn_runner_suspended(
        &world.setup,
        &runner_path,
        &control_pipe.name,
        &world.setup.paths.runner_root,
    )?;
    if unsafe { AssignProcessToJobObject(raw(&job), raw(&runner)) } == 0 {
        unsafe { TerminateProcess(raw(&runner), 1) };
        return unavailable(
            "Windows could not contain the suspended sandbox bootstrap in its Job.",
        );
    }
    if unsafe { ResumeThread(raw(&runner_thread)) } == u32::MAX {
        unsafe { TerminateJobObject(raw(&job), 1) };
        return unavailable("Windows could not resume the contained sandbox bootstrap.");
    }
    drop(runner_thread);
    connect_control_pipe(&control_pipe.handle, &runner, CONTROL_HANDSHAKE_TIMEOUT)?;
    verify_pipe_client(&control_pipe.handle, process_id(&runner)?)?;
    let mut control = file_from_handle(control_pipe.handle);
    let hello: RunnerHelloV1 =
        read_frame_bounded(&mut control, &runner, CONTROL_HANDSHAKE_TIMEOUT)?;
    if hello.version != WINDOWS_WORLD_ADAPTER_VERSION {
        unsafe { TerminateJobObject(raw(&job), 1) };
        return unavailable("Windows sandbox bootstrap protocol is mismatched.");
    }
    apply_job_ui_restrictions(&job)?;
    let logon_sid = sid_from_string(&hello.logon_sid)?;
    stamp_world_for_logon(&world, &logon_sid)?;
    let pipes = create_stdio_pipe_servers(&world.setup, &logon_sid)?;
    let launch = RunnerLaunchV1 {
        version: WINDOWS_WORLD_ADAPTER_VERSION.into(),
        executable: executable.to_owned(),
        argv: argv.to_vec(),
        environment: synthesized_environment(&world, environment)?,
        cwd: cwd.map(Path::to_owned),
        stdin_pipe: pipes.stdin_name.clone(),
        stdout_pipe: pipes.stdout_name.clone(),
        stderr_pipe: pipes.stderr_name.clone(),
    };
    write_frame_bounded(&mut control, &runner, &launch, CONTROL_HANDSHAKE_TIMEOUT)?;
    let ready: RunnerReadyV1 =
        read_frame_bounded(&mut control, &runner, CONTROL_HANDSHAKE_TIMEOUT)?;
    if ready.version != WINDOWS_WORLD_ADAPTER_VERSION || !ready.restricted || !ready.contained {
        unsafe { TerminateJobObject(raw(&job), 1) };
        return unavailable("Windows Worker did not prove restricted-token Job containment.");
    }
    connect_pipe(&pipes.stdin)?;
    connect_pipe(&pipes.stdout)?;
    connect_pipe(&pipes.stderr)?;
    let stdin = if stdin_requested {
        Some(file_from_handle(pipes.stdin))
    } else {
        drop(pipes.stdin);
        None
    };
    drop(acl_guard);
    Ok(WindowsSpawnedProcessV1 {
        process: WindowsProcessV1 {
            process: runner,
            job,
            world,
        },
        stdin,
        stdout: file_from_handle(pipes.stdout),
        stderr: file_from_handle(pipes.stderr),
    })
}

fn create_job(cpu_millis: u64, memory_bytes: u64) -> AppResult<OwnedHandle> {
    let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    let job = unsafe { owned(handle)? };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_JOB_TIME
        | JOB_OBJECT_LIMIT_JOB_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = MAX_JOB_PROCESSES;
    limits.BasicLimitInformation.PerJobUserTimeLimit =
        cpu_millis.saturating_mul(10_000).min(i64::MAX as u64) as i64;
    limits.JobMemoryLimit = memory_bytes.min(usize::MAX as u64) as usize;
    if unsafe {
        SetInformationJobObject(
            raw(&job),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return unavailable("Windows Job resource limits are unavailable.");
    }
    Ok(job)
}

fn apply_job_ui_restrictions(job: &OwnedHandle) -> AppResult<()> {
    let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: JOB_OBJECT_UILIMIT_HANDLES
            | JOB_OBJECT_UILIMIT_READCLIPBOARD
            | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
            | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
            | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
            | JOB_OBJECT_UILIMIT_GLOBALATOMS
            | JOB_OBJECT_UILIMIT_DESKTOP
            | JOB_OBJECT_UILIMIT_EXITWINDOWS,
    };
    if unsafe {
        SetInformationJobObject(
            raw(&job),
            JobObjectBasicUIRestrictions,
            (&ui as *const JOBOBJECT_BASIC_UI_RESTRICTIONS).cast(),
            std::mem::size_of_val(&ui) as u32,
        )
    } == 0
    {
        return unavailable("Windows Job UI restrictions are unavailable.");
    }
    Ok(())
}

fn create_control_pipe_server(setup: &WindowsSetupV1) -> AppResult<ControlPipeServerV1> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let name = format!(r"\\.\pipe\pastey-world-{nonce}-control");
    let sddl = format!(
        "D:P(A;;GA;;;{})(A;;GA;;;{})(A;;GA;;;SY)(A;;GA;;;BA)",
        setup.marker.host_sid, setup.marker.account_sid
    );
    Ok(ControlPipeServerV1 {
        handle: create_pipe_server(&name, PIPE_ACCESS_DUPLEX, &sddl, true)?,
        name,
    })
}

fn create_stdio_pipe_servers(
    setup: &WindowsSetupV1,
    logon_sid: &[u8],
) -> AppResult<StdioPipeServersV1> {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let prefix = format!(r"\\.\pipe\pastey-world-{nonce}");
    let logon_sid = sid_to_string(sid_ptr(logon_sid))?;
    let sddl = format!(
        "D:P(A;;GA;;;{})(A;;GA;;;{})(A;;GA;;;SY)(A;;GA;;;BA)",
        setup.marker.host_sid, logon_sid
    );
    Ok(StdioPipeServersV1 {
        stdin_name: format!("{prefix}-stdin"),
        stdout_name: format!("{prefix}-stdout"),
        stderr_name: format!("{prefix}-stderr"),
        stdin: create_pipe_server(
            &format!("{prefix}-stdin"),
            PIPE_ACCESS_OUTBOUND,
            &sddl,
            false,
        )?,
        stdout: create_pipe_server(
            &format!("{prefix}-stdout"),
            PIPE_ACCESS_INBOUND,
            &sddl,
            false,
        )?,
        stderr: create_pipe_server(
            &format!("{prefix}-stderr"),
            PIPE_ACCESS_INBOUND,
            &sddl,
            false,
        )?,
    })
}

fn create_pipe_server(
    name: &str,
    access: u32,
    sddl: &str,
    nonblocking: bool,
) -> AppResult<OwnedHandle> {
    let mut descriptor = SecurityDescriptorV1::from_sddl(sddl)?;
    let attributes = descriptor.attributes();
    let name = wide(name);
    let wait_mode = if nonblocking { PIPE_NOWAIT } else { PIPE_WAIT };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            access | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | wait_mode | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            5_000,
            &attributes,
        )
    };
    unsafe { owned(handle) }
}

fn connect_control_pipe(
    handle: &OwnedHandle,
    runner: &OwnedHandle,
    maximum: Duration,
) -> AppResult<()> {
    let deadline = Instant::now() + maximum;
    loop {
        if unsafe { ConnectNamedPipe(raw(handle), ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED
        {
            return Ok(());
        }
        let error = unsafe { GetLastError() };
        if error != ERROR_PIPE_LISTENING && error != ERROR_NO_DATA {
            return unavailable("Windows sandbox bootstrap pipe connection failed.");
        }
        if !process_is_active(runner)? || Instant::now() >= deadline {
            return unavailable("Windows sandbox bootstrap control handshake did not connect.");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn connect_pipe(handle: &OwnedHandle) -> AppResult<()> {
    if unsafe { ConnectNamedPipe(raw(handle), ptr::null_mut()) } != 0 {
        return Ok(());
    }
    if unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
        Ok(())
    } else {
        unavailable("Windows sandbox bootstrap pipe connection failed.")
    }
}

fn verify_pipe_client(handle: &OwnedHandle, expected_pid: u32) -> AppResult<()> {
    let mut pid = 0_u32;
    if unsafe { GetNamedPipeClientProcessId(raw(handle), &mut pid) } == 0 || pid != expected_pid {
        return unavailable("Windows sandbox bootstrap pipe client was substituted.");
    }
    Ok(())
}

fn process_id(process: &OwnedHandle) -> AppResult<u32> {
    let id = unsafe { windows_sys::Win32::System::Threading::GetProcessId(raw(process)) };
    if id == 0 {
        unavailable("Windows sandbox bootstrap process identity is unavailable.")
    } else {
        Ok(id)
    }
}

fn process_is_active(process: &OwnedHandle) -> AppResult<bool> {
    let mut code = 0_u32;
    if unsafe { GetExitCodeProcess(raw(process), &mut code) } == 0 {
        return unavailable("Windows sandbox bootstrap state is unavailable.");
    }
    Ok(code == STILL_ACTIVE as u32)
}

fn spawn_runner_suspended(
    setup: &WindowsSetupV1,
    runner_path: &Path,
    control_pipe: &str,
    cwd: &Path,
) -> AppResult<(OwnedHandle, OwnedHandle)> {
    let password = decrypt_password(&setup.marker.encrypted_password)?;
    let mut password_wide = wide_os(OsStr::new(&password));
    let username = wide(SANDBOX_USERNAME);
    let domain = wide(".");
    let runner_wide = wide_os(runner_path.as_os_str());
    let args = vec![RUNNER_CLI.to_string(), control_pipe.to_string()];
    let mut command_line = windows_command_line(runner_path, &args);
    let cwd = wide_os(cwd.as_os_str());
    let environment = [0_u16, 0_u16];
    let startup = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    let created = unsafe {
        CreateProcessWithLogonW(
            username.as_ptr(),
            domain.as_ptr(),
            password_wide.as_ptr(),
            0,
            runner_wide.as_ptr(),
            command_line.as_mut_ptr(),
            CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            environment.as_ptr().cast(),
            cwd.as_ptr(),
            &startup,
            &mut process,
        )
    };
    password_wide.fill(0);
    let mut password = password.into_bytes();
    password.fill(0);
    if created == 0 {
        return unavailable("Windows could not log on the Pastey sandbox bootstrap account.");
    }
    let process_handle = unsafe { owned(process.hProcess)? };
    let thread_handle = unsafe { owned(process.hThread)? };
    Ok((process_handle, thread_handle))
}

fn create_private_desktop(logon_sid: &[u8]) -> AppResult<(PrivateDesktopV1, String)> {
    let logon_sid_string = sid_to_string(sid_ptr(logon_sid))?;
    let name = format!("PasteyWorld{}", uuid::Uuid::new_v4().simple());
    let sddl = format!("D:P(A;;GA;;;{logon_sid_string})(A;;GA;;;SY)(A;;GA;;;BA)");
    let mut descriptor = SecurityDescriptorV1::from_sddl(&sddl)?;
    let attributes = descriptor.attributes();
    let name_wide = wide(&name);
    let desktop = unsafe {
        CreateDesktopW(
            name_wide.as_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            DESKTOP_GENERIC_ALL,
            &attributes,
        )
    };
    if desktop.is_null() {
        return unavailable("Windows could not create a private Worker desktop.");
    }
    Ok((PrivateDesktopV1(desktop), name))
}

fn stamp_world_for_logon(world: &WindowsWorldV1, logon_sid: &[u8]) -> AppResult<()> {
    let traversal = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;
    let root_entries = host_and_subject_entries(&world.setup, logon_sid, traversal);
    set_exact_dacl(&world.root, &root_entries, true)?;
    stamp_tree(
        &world.input_root,
        &host_and_subject_entries(&world.setup, logon_sid, FILE_GENERIC_READ),
    )?;
    stamp_tree(
        &world.scratch_root,
        &host_and_subject_entries(
            &world.setup,
            logon_sid,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_DELETE_CHILD,
        ),
    )?;
    stamp_tree(
        &world.output_root,
        &host_and_subject_entries(
            &world.setup,
            logon_sid,
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_DELETE_CHILD,
        ),
    )?;
    stamp_tree(
        &world.tools_root,
        &host_and_subject_entries(
            &world.setup,
            logon_sid,
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        ),
    )?;
    Ok(())
}

fn stamp_tree(path: &Path, entries: &[(Vec<u8>, u32)]) -> AppResult<()> {
    let mut pending = vec![path.to_owned()];
    while let Some(current) = pending.pop() {
        let metadata = fs::symlink_metadata(&current)?;
        reject_reparse(&metadata)?;
        set_exact_dacl(&current, entries, metadata.is_dir())?;
        if metadata.is_dir() {
            for entry in fs::read_dir(&current)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}

fn synthesized_environment(
    world: &WindowsWorldV1,
    requested: &BTreeMap<String, String>,
) -> AppResult<BTreeMap<String, String>> {
    let windows = windows_directory()?;
    let profile = world.scratch_root.join("profile");
    let temp = world.scratch_root.join("temp");
    let mut environment: BTreeMap<String, String> = BTreeMap::from([
        ("SYSTEMROOT".into(), windows.to_string_lossy().into_owned()),
        ("WINDIR".into(), windows.to_string_lossy().into_owned()),
        ("TEMP".into(), temp.to_string_lossy().into_owned()),
        ("TMP".into(), temp.to_string_lossy().into_owned()),
        ("USERPROFILE".into(), profile.to_string_lossy().into_owned()),
        (
            "APPDATA".into(),
            profile
                .join("AppData")
                .join("Roaming")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "LOCALAPPDATA".into(),
            profile
                .join("AppData")
                .join("Local")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "PATH".into(),
            world.tools_root.to_string_lossy().into_owned(),
        ),
        ("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into()),
    ]);
    for (name, value) in requested {
        if environment
            .keys()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            return invalid(
                "Managed process environment attempted to replace a Host-synthesized binding.",
            );
        }
        environment.insert(name.clone(), value.clone());
    }
    Ok(environment)
}

fn runner_main(control_pipe: &str) -> AppResult<()> {
    let token = current_process_restriction_token()?;
    let logon_sid = token_logon_sid(&token)?;
    validate_sandbox_base_token(&token)?;
    let (_desktop, desktop_name) = create_private_desktop(&logon_sid)?;
    let hello = RunnerHelloV1 {
        version: WINDOWS_WORLD_ADAPTER_VERSION.into(),
        logon_sid: sid_to_string(sid_ptr(&logon_sid))?,
    };
    let mut control = file_from_handle(open_pipe_client(
        control_pipe,
        GENERIC_READ | GENERIC_WRITE,
    )?);
    write_frame(&mut control, &hello)?;
    let launch: RunnerLaunchV1 = read_frame(&mut control)?;
    if launch.version != WINDOWS_WORLD_ADAPTER_VERSION {
        return unavailable("Windows sandbox bootstrap launch protocol is mismatched.");
    }
    let restricted = restricted_token(&token, &logon_sid)?;
    if unsafe { IsTokenRestricted(raw(&restricted)) } == 0 {
        return unavailable("Windows Worker token is not restricted.");
    }
    let stdin = open_pipe_client(&launch.stdin_pipe, GENERIC_READ)?;
    let stdout = open_pipe_client(&launch.stdout_pipe, GENERIC_WRITE)?;
    let stderr = open_pipe_client(&launch.stderr_pipe, GENERIC_WRITE)?;
    for handle in [&stdin, &stdout, &stderr] {
        if unsafe { SetHandleInformation(raw(handle), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }
            == 0
        {
            return unavailable(
                "Windows could not mark an intended Worker stdio handle inheritable.",
            );
        }
    }
    let child_handles = [raw(&stdin), raw(&stdout), raw(&stderr)];
    let mut attributes = AttributeListV1::new(1)?;
    attributes.update(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        child_handles.as_ptr().cast(),
        std::mem::size_of_val(&child_handles),
    )?;
    let executable = wide_os(launch.executable.as_os_str());
    let mut command_line = windows_command_line(&launch.executable, &launch.argv);
    let environment = environment_block(&launch.environment);
    let cwd = launch.cwd.as_ref().map(|value| wide_os(value.as_os_str()));
    let mut desktop = wide(&desktop_name);
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_handles[0];
    startup.StartupInfo.hStdOutput = child_handles[1];
    startup.StartupInfo.hStdError = child_handles[2];
    startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    startup.lpAttributeList = attributes.ptr;
    let mut child = PROCESS_INFORMATION::default();
    if unsafe {
        CreateProcessAsUserW(
            raw(&restricted),
            executable.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            cwd.as_ref().map_or(ptr::null(), |value| value.as_ptr()),
            (&startup as *const STARTUPINFOEXW).cast(),
            &mut child,
        )
    } == 0
    {
        return unavailable("Windows restricted Worker process creation failed.");
    }
    let process = unsafe { owned(child.hProcess)? };
    let thread_handle = unsafe { owned(child.hThread)? };
    let mut contained = 0;
    if unsafe { IsProcessInJob(raw(&process), ptr::null_mut(), &mut contained) } == 0
        || contained == 0
    {
        unsafe { TerminateProcess(raw(&process), 1) };
        return unavailable("Windows Worker did not inherit its Host-owned Job.");
    }
    if unsafe { ResumeThread(raw(&thread_handle)) } == u32::MAX {
        unsafe { TerminateProcess(raw(&process), 1) };
        return unavailable("Windows could not resume the restricted Worker.");
    }
    drop(thread_handle);
    drop(stdin);
    drop(stdout);
    drop(stderr);
    write_frame(
        &mut control,
        &RunnerReadyV1 {
            version: WINDOWS_WORLD_ADAPTER_VERSION.into(),
            child_pid: child.dwProcessId,
            restricted: true,
            contained: true,
        },
    )?;
    let exit_code = wait_process(&process)?;
    std::process::exit(exit_code as i32);
}

fn restricted_token(base: &OwnedHandle, logon_sid: &[u8]) -> AppResult<OwnedHandle> {
    let restrictions = [SID_AND_ATTRIBUTES {
        Sid: sid_ptr(logon_sid),
        Attributes: 0,
    }];
    let mut token = ptr::null_mut();
    if unsafe {
        CreateRestrictedToken(
            raw(base),
            DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
            0,
            ptr::null(),
            0,
            ptr::null(),
            restrictions.len() as u32,
            restrictions.as_ptr(),
            &mut token,
        )
    } == 0
    {
        return unavailable("Windows could not create the restricted Worker token.");
    }
    unsafe { owned(token) }
}

fn wait_process(process: &OwnedHandle) -> AppResult<u32> {
    loop {
        let mut code = 0_u32;
        if unsafe { GetExitCodeProcess(raw(process), &mut code) } == 0 {
            return unavailable("Windows could not observe the restricted Worker exit.");
        }
        if code != STILL_ACTIVE as u32 {
            return Ok(code);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn open_pipe_client(name: &str, access: u32) -> AppResult<OwnedHandle> {
    let name = wide(name);
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    unsafe { owned(handle) }
}

fn current_process_query_token() -> AppResult<OwnedHandle> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return unavailable("Windows Host identity token is unavailable.");
    }
    unsafe { owned(token) }
}

fn current_process_restriction_token() -> AppResult<OwnedHandle> {
    let mut token = ptr::null_mut();
    let rights = TOKEN_QUERY
        | TOKEN_DUPLICATE
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_SESSIONID;
    if unsafe { OpenProcessToken(GetCurrentProcess(), rights, &mut token) } == 0 {
        return unavailable("Windows sandbox bootstrap token is unavailable.");
    }
    unsafe { owned(token) }
}

fn token_logon_sid(token: &OwnedHandle) -> AppResult<Vec<u8>> {
    let buffer = token_information(token, TokenGroups)?;
    let groups = unsafe { &*(buffer.as_ptr().cast::<TOKEN_GROUPS>()) };
    let entries =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    let mask = SE_GROUP_LOGON_ID as u32;
    let sid = entries
        .iter()
        .find(|entry| entry.Attributes & mask == mask)
        .map(|entry| entry.Sid)
        .ok_or_else(|| {
            AppError::InvalidInput("Windows sandbox logon SID is unavailable.".into())
        })?;
    copy_sid(sid)
}

fn token_user_sid(token: &OwnedHandle) -> AppResult<Vec<u8>> {
    let buffer = token_information(token, TokenUser)?;
    let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
    copy_sid(user.User.Sid)
}

fn validate_sandbox_base_token(token: &OwnedHandle) -> AppResult<()> {
    if token_user_sid(token)? != lookup_account_sid(SANDBOX_USERNAME)? {
        return unavailable("Windows sandbox bootstrap has the wrong principal identity.");
    }
    reject_privileged_token_groups(token)
}

fn reject_privileged_token_groups(token: &OwnedHandle) -> AppResult<()> {
    let buffer = token_information(token, TokenGroups)?;
    let groups = unsafe { &*(buffer.as_ptr().cast::<TOKEN_GROUPS>()) };
    let entries =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    for privileged_group in PRIVILEGED_LOCAL_GROUP_SIDS {
        let privileged_sid = sid_from_string(privileged_group)?;
        for entry in entries {
            if copy_sid(entry.Sid)? == privileged_sid {
                return unavailable(
                    "Windows sandbox principal belongs to a privileged local group.",
                );
            }
        }
    }
    Ok(())
}

fn token_information(token: &OwnedHandle, class: i32) -> AppResult<Vec<usize>> {
    let mut bytes = 0_u32;
    unsafe { GetTokenInformation(raw(token), class, ptr::null_mut(), 0, &mut bytes) };
    if bytes == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return unavailable("Windows token information sizing failed.");
    }
    let mut buffer = vec![0_usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            raw(token),
            class,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        )
    } == 0
    {
        return unavailable("Windows token information lookup failed.");
    }
    Ok(buffer)
}

fn write_frame<T: Serialize>(writer: &mut File, value: &T) -> AppResult<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_CONTROL_FRAME {
        return invalid("Windows sandbox bootstrap frame exceeds its bound.");
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

fn write_frame_bounded<T: Serialize>(
    writer: &mut File,
    runner: &OwnedHandle,
    value: &T,
    maximum: Duration,
) -> AppResult<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_CONTROL_FRAME {
        return invalid("Windows sandbox bootstrap frame exceeds its bound.");
    }
    let mut frame = Vec::with_capacity(bytes.len() + 4);
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(&bytes);
    write_all_bounded(writer, runner, &frame, maximum)
}

fn write_all_bounded(
    writer: &mut File,
    runner: &OwnedHandle,
    mut bytes: &[u8],
    maximum: Duration,
) -> AppResult<()> {
    let deadline = Instant::now() + maximum;
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => return unavailable("Windows sandbox bootstrap control pipe closed."),
            Ok(written) => bytes = &bytes[written..],
            Err(error) if pipe_operation_pending(&error) => {
                if !process_is_active(runner)? || Instant::now() >= deadline {
                    return unavailable("Windows sandbox bootstrap control write timed out.");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    writer.flush()?;
    Ok(())
}

fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut File) -> AppResult<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_CONTROL_FRAME {
        return invalid("Windows sandbox bootstrap frame length is invalid.");
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_frame_bounded<T: for<'de> Deserialize<'de>>(
    reader: &mut File,
    runner: &OwnedHandle,
    maximum: Duration,
) -> AppResult<T> {
    let mut length = [0_u8; 4];
    read_exact_bounded(reader, runner, &mut length, maximum)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_CONTROL_FRAME {
        return invalid("Windows sandbox bootstrap frame length is invalid.");
    }
    let mut bytes = vec![0_u8; length];
    read_exact_bounded(reader, runner, &mut bytes, maximum)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn read_exact_bounded(
    reader: &mut File,
    runner: &OwnedHandle,
    mut bytes: &mut [u8],
    maximum: Duration,
) -> AppResult<()> {
    let deadline = Instant::now() + maximum;
    while !bytes.is_empty() {
        match reader.read(bytes) {
            Ok(0) => return unavailable("Windows sandbox bootstrap control pipe closed."),
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if pipe_operation_pending(&error) => {
                if !process_is_active(runner)? || Instant::now() >= deadline {
                    return unavailable("Windows sandbox bootstrap control read timed out.");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn pipe_operation_pending(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || matches!(
            error.raw_os_error(),
            Some(value) if value == ERROR_NO_DATA as i32 || value == ERROR_PIPE_LISTENING as i32
        )
}

fn setup_paths() -> AppResult<SetupPathsV1> {
    let program_data = program_data_directory()?;
    let base = program_data.join("Pastey").join("ExecutionWorldV1");
    let secrets = base.join("secrets");
    Ok(SetupPathsV1 {
        marker: secrets.join("setup.json"),
        recovery: secrets.join("setup.provisional.json"),
        managed_root: base.join("runs"),
        runner_root: base.join("runner"),
        base,
        secrets,
    })
}

fn program_data_directory() -> AppResult<PathBuf> {
    let mut buffer = [0_u16; 260];
    let status = unsafe {
        SHGetFolderPathW(
            ptr::null_mut(),
            CSIDL_COMMON_APPDATA as i32,
            ptr::null_mut(),
            0,
            buffer.as_mut_ptr(),
        )
    };
    if status < 0 {
        return unavailable("Windows ProgramData directory lookup failed.");
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    if length == 0 {
        return unavailable("Windows ProgramData directory is unavailable.");
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn windows_directory() -> AppResult<PathBuf> {
    let mut buffer = vec![0_u16; 32_768];
    let chars = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if chars == 0 || chars >= buffer.len() {
        return unavailable("Windows system directory is unavailable.");
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..chars])))
}

fn run_elevated_setup() -> AppResult<()> {
    if !is_elevated()? {
        return unavailable(
            "Windows ExecutionWorld setup must run once from an elevated Pastey process.",
        );
    }
    let paths = setup_paths()?;
    let host_token = current_process_query_token()?;
    let host_sid = token_user_sid(&host_token)?;
    let host_sid_string = sid_to_string(sid_ptr(&host_sid))?;
    let existing: Option<SetupMarkerV1> = read_optional_json(&paths.marker)?;
    if let Some(marker) = existing.as_ref() {
        validate_setup_marker(marker, &paths, &host_sid_string)?;
    }
    let recovery: Option<SetupRecoveryV1> = read_optional_json(&paths.recovery)?;
    if let Some(provisional) = recovery.as_ref() {
        validate_setup_recovery(provisional, &paths, &host_sid_string)?;
    }
    let local_account = lookup_local_account(SANDBOX_USERNAME)?;
    // The prior implementation created the account before it created this
    // ExecutionWorldV1 root. This one-time compatibility case is deliberately
    // narrower than the account metadata alone so an arbitrary marker-less
    // installation is never adopted.
    let legacy_account_fingerprint = local_account
        .as_ref()
        .is_some_and(LocalAccountV1::matches_legacy_partial_fingerprint);
    let legacy_fingerprint_matches = legacy_account_fingerprint
        && (recovery.as_ref().is_some_and(|provisional| {
            provisional.stage == RecoveryStageV1::LegacyPasswordRotationPending
        }) || (existing.is_none() && recovery.is_none() && !paths.base.exists()));
    let plan = select_setup_plan(SetupEvidenceV1 {
        final_account_sid: existing.as_ref().map(|marker| marker.account_sid.clone()),
        recovery: recovery.as_ref().map(|provisional| RecoveryEvidenceV1 {
            stage: provisional.stage,
            account_sid: provisional.account_sid.clone(),
        }),
        local_account_sid: local_account
            .as_ref()
            .map(|account| account.sid_string.clone()),
        legacy_fingerprint_matches,
    })
    .map_err(|reason| {
        AppError::InvalidInput(format!(
            "Windows ExecutionWorld setup state is not safely resumable: {reason}."
        ))
    })?;

    let mut password;
    let account_sid;
    match plan {
        SetupPlanV1::Repeat {
            account_sid: expected_sid,
        } => {
            let marker = existing.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Windows setup marker disappeared during setup.".into())
            })?;
            password = decrypt_password(&marker.encrypted_password)?;
            account_sid = require_local_account_sid(&expected_sid)?;
            set_account_password(&password)?;
        }
        SetupPlanV1::Fresh => {
            ensure_protected_setup_state_dirs(&paths, &host_sid)?;
            password = generate_password();
            write_setup_recovery(
                &paths,
                &setup_recovery(
                    &paths,
                    &host_sid_string,
                    &password,
                    RecoveryStageV1::AccountPending,
                    None,
                )?,
                &host_sid,
            )?;
            create_account(&password)?;
            let account = require_local_account()?;
            authenticate_local_account(&password, &account.sid)?;
            write_bound_recovery(&paths, &host_sid_string, &host_sid, &password, &account)?;
            account_sid = account.sid;
        }
        SetupPlanV1::ResumeCreate => {
            let provisional = recovery.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Windows provisional setup state disappeared.".into())
            })?;
            password = decrypt_password(&provisional.encrypted_password)?;
            create_account(&password)?;
            let account = require_local_account()?;
            authenticate_local_account(&password, &account.sid)?;
            write_bound_recovery(&paths, &host_sid_string, &host_sid, &password, &account)?;
            account_sid = account.sid;
        }
        SetupPlanV1::ResumeAuthenticate {
            account_sid: expected_sid,
        } => {
            let provisional = recovery.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Windows provisional setup state disappeared.".into())
            })?;
            password = decrypt_password(&provisional.encrypted_password)?;
            account_sid = require_local_account_sid(&expected_sid)?;
            authenticate_local_account(&password, &account_sid)?;
            write_bound_recovery(
                &paths,
                &host_sid_string,
                &host_sid,
                &password,
                &require_local_account()?,
            )?;
        }
        SetupPlanV1::BeginLegacyRecovery {
            account_sid: expected_sid,
        } => {
            ensure_protected_setup_state_dirs(&paths, &host_sid)?;
            password = generate_password();
            write_setup_recovery(
                &paths,
                &setup_recovery(
                    &paths,
                    &host_sid_string,
                    &password,
                    RecoveryStageV1::LegacyPasswordRotationPending,
                    Some(expected_sid.clone()),
                )?,
                &host_sid,
            )?;
            account_sid = require_legacy_partial_account_sid(&expected_sid)?;
            set_account_password(&password)?;
            authenticate_local_account(&password, &account_sid)?;
            write_bound_recovery(
                &paths,
                &host_sid_string,
                &host_sid,
                &password,
                &require_local_account()?,
            )?;
        }
        SetupPlanV1::ResumeLegacyRotation {
            account_sid: expected_sid,
        } => {
            let provisional = recovery.as_ref().ok_or_else(|| {
                AppError::InvalidInput("Windows provisional setup state disappeared.".into())
            })?;
            password = decrypt_password(&provisional.encrypted_password)?;
            account_sid = require_legacy_partial_account_sid(&expected_sid)?;
            set_account_password(&password)?;
            authenticate_local_account(&password, &account_sid)?;
            write_bound_recovery(
                &paths,
                &host_sid_string,
                &host_sid,
                &password,
                &require_local_account()?,
            )?;
        }
    }
    reconcile_account_flags()?;
    let account_sid_string = sid_to_string(sid_ptr(&account_sid))?;
    let system_sid = sid_from_string("S-1-5-18")?;
    let administrators_sid = sid_from_string("S-1-5-32-544")?;
    let owner_rights_sid = sid_from_string("S-1-3-4")?;
    reconcile_account_rights(&account_sid)?;
    for directory in [
        &paths.base,
        &paths.secrets,
        &paths.managed_root,
        &paths.runner_root,
    ] {
        fs::create_dir_all(directory)?;
    }
    let setup = WindowsSetupV1 {
        marker: SetupMarkerV1 {
            version: SETUP_VERSION,
            username: SANDBOX_USERNAME.into(),
            account_sid: account_sid_string,
            host_sid: host_sid_string,
            encrypted_password: String::new(),
            managed_root: paths.managed_root.clone(),
            runner_root: paths.runner_root.clone(),
            firewall_rule_name: FIREWALL_RULE_NAME.into(),
        },
        account_sid,
        host_sid,
        system_sid,
        administrators_sid,
        owner_rights_sid,
        paths: paths.clone(),
    };
    set_exact_dacl(
        &paths.base,
        &host_and_subject_entries(
            &setup,
            &setup.account_sid,
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        ),
        true,
    )?;
    set_exact_dacl(&paths.secrets, &host_only_entries(&setup), true)?;
    set_exact_dacl(
        &paths.managed_root,
        &host_and_subject_entries(
            &setup,
            &setup.account_sid,
            FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        ),
        true,
    )?;
    set_exact_dacl(
        &paths.runner_root,
        &host_and_subject_entries(
            &setup,
            &setup.account_sid,
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        ),
        true,
    )?;
    reconcile_firewall_rule(&setup.marker.account_sid)?;
    let encrypted_password = encrypt_password(&password)?;
    unsafe { password.as_bytes_mut().fill(0) };
    let mut marker = setup.marker;
    marker.encrypted_password = encrypted_password;
    write_setup_marker(&paths, &marker, &setup.host_sid)?;
    remove_setup_recovery(&paths)?;
    Ok(())
}

fn load_setup() -> AppResult<WindowsSetupV1> {
    let paths = setup_paths()?;
    let bytes = fs::read(&paths.marker).map_err(|_| {
        AppError::InvalidInput("Elevated Windows ExecutionWorld setup is missing.".into())
    })?;
    let marker: SetupMarkerV1 = serde_json::from_slice(&bytes)?;
    let host_token = current_process_query_token()?;
    let host_sid = token_user_sid(&host_token)?;
    let host_sid_string = sid_to_string(sid_ptr(&host_sid))?;
    let account_sid = lookup_account_sid(SANDBOX_USERNAME)?;
    let account_sid_string = sid_to_string(sid_ptr(&account_sid))?;
    let system_sid = sid_from_string("S-1-5-18")?;
    let administrators_sid = sid_from_string("S-1-5-32-544")?;
    let owner_rights_sid = sid_from_string("S-1-3-4")?;
    if marker.version != SETUP_VERSION
        || marker.username != SANDBOX_USERNAME
        || marker.host_sid != host_sid_string
        || marker.account_sid != account_sid_string
        || marker.managed_root != paths.managed_root
        || marker.runner_root != paths.runner_root
        || marker.firewall_rule_name != FIREWALL_RULE_NAME
    {
        return unavailable("Windows ExecutionWorld setup marker is stale or substituted.");
    }
    verify_firewall_rule(&marker.account_sid)?;
    let password = decrypt_password(&marker.encrypted_password)?;
    if password.is_empty() {
        return unavailable("Windows sandbox credential record is invalid.");
    }
    let mut password = password.into_bytes();
    password.fill(0);
    Ok(WindowsSetupV1 {
        marker,
        account_sid,
        host_sid,
        system_sid,
        administrators_sid,
        owner_rights_sid,
        paths,
    })
}

fn read_optional_json<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_setup_marker(
    marker: &SetupMarkerV1,
    paths: &SetupPathsV1,
    host_sid: &str,
) -> AppResult<()> {
    if marker.version != SETUP_VERSION
        || marker.username != SANDBOX_USERNAME
        || marker.host_sid != host_sid
        || marker.managed_root != paths.managed_root
        || marker.runner_root != paths.runner_root
        || marker.firewall_rule_name != FIREWALL_RULE_NAME
    {
        return unavailable(
            "Existing Windows ExecutionWorld setup belongs to a different identity or version.",
        );
    }
    Ok(())
}

fn validate_setup_recovery(
    recovery: &SetupRecoveryV1,
    paths: &SetupPathsV1,
    host_sid: &str,
) -> AppResult<()> {
    if recovery.version != SETUP_VERSION
        || recovery.username != SANDBOX_USERNAME
        || recovery.host_sid != host_sid
        || recovery.managed_root != paths.managed_root
        || recovery.runner_root != paths.runner_root
        || recovery.firewall_rule_name != FIREWALL_RULE_NAME
        || recovery.encrypted_password.is_empty()
    {
        return unavailable(
            "Windows ExecutionWorld provisional setup state is stale or substituted.",
        );
    }
    Ok(())
}

fn setup_recovery(
    paths: &SetupPathsV1,
    host_sid: &str,
    password: &str,
    stage: RecoveryStageV1,
    account_sid: Option<String>,
) -> AppResult<SetupRecoveryV1> {
    Ok(SetupRecoveryV1 {
        version: SETUP_VERSION,
        username: SANDBOX_USERNAME.into(),
        host_sid: host_sid.into(),
        // Provisional state is additionally bound to the exact Host user's DPAPI
        // profile. Its Host SID and protected ProgramData ACL are validated before
        // the credential can authorize a resumed account mutation.
        encrypted_password: encrypt_recovery_password(password)?,
        managed_root: paths.managed_root.clone(),
        runner_root: paths.runner_root.clone(),
        firewall_rule_name: FIREWALL_RULE_NAME.into(),
        stage,
        account_sid,
    })
}

fn write_bound_recovery(
    paths: &SetupPathsV1,
    host_sid_string: &str,
    host_sid: &[u8],
    password: &str,
    account: &LocalAccountV1,
) -> AppResult<()> {
    write_setup_recovery(
        paths,
        &setup_recovery(
            paths,
            host_sid_string,
            password,
            RecoveryStageV1::Bound,
            Some(account.sid_string.clone()),
        )?,
        host_sid,
    )
}

fn ensure_protected_setup_state_dirs(paths: &SetupPathsV1, host_sid: &[u8]) -> AppResult<()> {
    fs::create_dir_all(&paths.secrets)?;
    let entries = protected_setup_state_entries(host_sid)?;
    set_exact_dacl(&paths.base, &entries, true)?;
    set_exact_dacl(&paths.secrets, &entries, true)
}

fn protected_setup_state_entries(host_sid: &[u8]) -> AppResult<Vec<(Vec<u8>, u32)>> {
    Ok(vec![
        (host_sid.to_vec(), FILE_ALL_ACCESS),
        (sid_from_string("S-1-5-18")?, FILE_ALL_ACCESS),
        (sid_from_string("S-1-5-32-544")?, FILE_ALL_ACCESS),
        (sid_from_string("S-1-3-4")?, READ_CONTROL),
    ])
}

fn create_account(password: &str) -> AppResult<()> {
    let mut username = wide(SANDBOX_USERNAME);
    let mut password_wide = wide(password);
    let mut comment = wide("Pastey managed Worker offline sandbox principal");
    let mut info = USER_INFO_1 {
        usri1_name: username.as_mut_ptr(),
        usri1_password: password_wide.as_mut_ptr(),
        usri1_password_age: 0,
        usri1_priv: USER_PRIV_USER,
        usri1_home_dir: ptr::null_mut(),
        usri1_comment: comment.as_mut_ptr(),
        usri1_flags: ACCOUNT_SETUP_FLAGS,
        usri1_script_path: ptr::null_mut(),
    };
    let mut parameter = 0_u32;
    let result = unsafe {
        NetUserAdd(
            ptr::null(),
            1,
            (&mut info as *mut USER_INFO_1).cast(),
            &mut parameter,
        )
    };
    password_wide.fill(0);
    if result != NERR_Success {
        return net_api_failure("NetUserAdd(local Pastey sandbox account)", result);
    }
    Ok(())
}

fn set_account_password(password: &str) -> AppResult<()> {
    let username = wide(SANDBOX_USERNAME);
    let mut password_wide = wide(password);
    let mut password_info = USER_INFO_1003 {
        usri1003_password: password_wide.as_mut_ptr(),
    };
    let mut parameter = 0_u32;
    let result = unsafe {
        NetUserSetInfo(
            ptr::null(),
            username.as_ptr(),
            1003,
            (&mut password_info as *mut USER_INFO_1003).cast(),
            &mut parameter,
        )
    };
    password_wide.fill(0);
    if result != NERR_Success {
        return net_api_failure("NetUserSetInfo(local Pastey sandbox credential)", result);
    }
    Ok(())
}

fn reconcile_account_flags() -> AppResult<()> {
    let username = wide(SANDBOX_USERNAME);
    let mut parameter = 0_u32;
    let mut flags = USER_INFO_1008 {
        usri1008_flags: ACCOUNT_SETUP_FLAGS,
    };
    let flags_result = unsafe {
        NetUserSetInfo(
            ptr::null(),
            username.as_ptr(),
            1008,
            (&mut flags as *mut USER_INFO_1008).cast(),
            &mut parameter,
        )
    };
    if flags_result != NERR_Success {
        return net_api_failure("NetUserSetInfo(local Pastey sandbox flags)", flags_result);
    }
    Ok(())
}

fn reconcile_account_rights(account_sid: &[u8]) -> AppResult<()> {
    let attributes = LSA_OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<LSA_OBJECT_ATTRIBUTES>() as u32,
        ..Default::default()
    };
    let mut policy: LSA_HANDLE = 0;
    let status = unsafe {
        LsaOpenPolicy(
            ptr::null(),
            &attributes,
            (POLICY_LOOKUP_NAMES | POLICY_CREATE_ACCOUNT) as u32,
            &mut policy,
        )
    };
    if status != 0 {
        return unavailable("Windows local security policy is unavailable for sandbox setup.");
    }
    let rights = [
        "SeDenyNetworkLogonRight",
        "SeDenyRemoteInteractiveLogonRight",
        "SeDenyServiceLogonRight",
        "SeDenyBatchLogonRight",
    ];
    let mut wide_rights = rights.iter().map(|right| wide(right)).collect::<Vec<_>>();
    let descriptors = wide_rights
        .iter_mut()
        .map(|right| LSA_UNICODE_STRING {
            Length: ((right.len() - 1) * 2) as u16,
            MaximumLength: (right.len() * 2) as u16,
            Buffer: right.as_mut_ptr(),
        })
        .collect::<Vec<_>>();
    let add = unsafe {
        LsaAddAccountRights(
            policy,
            sid_ptr(account_sid),
            descriptors.as_ptr(),
            descriptors.len() as u32,
        )
    };
    unsafe { LsaClose(policy) };
    if add != 0 {
        let _ = unsafe { LsaNtStatusToWinError(add) };
        return unavailable("Windows could not apply sandbox logon restrictions.");
    }
    Ok(())
}

fn is_elevated() -> AppResult<bool> {
    let mut administrators = ptr::null_mut();
    if unsafe {
        AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            2,
            32,
            544,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut administrators,
        )
    } == 0
    {
        return unavailable("Windows administrator membership SID is unavailable.");
    }
    let mut member = 0;
    let checked = unsafe { CheckTokenMembership(ptr::null_mut(), administrators, &mut member) };
    unsafe { FreeSid(administrators) };
    if checked == 0 {
        unavailable("Windows administrator membership check failed.")
    } else {
        Ok(member != 0)
    }
}

fn write_setup_marker(
    paths: &SetupPathsV1,
    marker: &SetupMarkerV1,
    host_sid: &[u8],
) -> AppResult<()> {
    write_protected_setup_json(
        &paths.secrets,
        &paths.marker,
        "setup",
        marker,
        host_sid,
        "final setup marker",
    )
}

fn write_setup_recovery(
    paths: &SetupPathsV1,
    recovery: &SetupRecoveryV1,
    host_sid: &[u8],
) -> AppResult<()> {
    write_protected_setup_json(
        &paths.secrets,
        &paths.recovery,
        "setup-provisional",
        recovery,
        host_sid,
        "provisional setup state",
    )
}

fn write_protected_setup_json<T: Serialize>(
    secrets: &Path,
    destination: &Path,
    temp_prefix: &str,
    value: &T,
    host_sid: &[u8],
    description: &str,
) -> AppResult<()> {
    let temp = secrets.join(format!("{temp_prefix}-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp, serde_json::to_vec(value)?)?;
    set_exact_dacl(&temp, &protected_setup_state_entries(host_sid)?, false)?;
    let temp_wide = wide_os(temp.as_os_str());
    let destination_wide = wide_os(destination.as_os_str());
    if unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let _ = fs::remove_file(&temp);
        return Err(AppError::InvalidInput(format!(
            "Windows could not atomically commit the protected {description}: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

fn remove_setup_recovery(paths: &SetupPathsV1) -> AppResult<()> {
    match fs::remove_file(&paths.recovery) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn encrypt_password(password: &str) -> AppResult<String> {
    encrypt_password_with_flags(
        password,
        CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
    )
}

fn encrypt_recovery_password(password: &str) -> AppResult<String> {
    encrypt_password_with_flags(password, CRYPTPROTECT_UI_FORBIDDEN)
}

fn encrypt_password_with_flags(password: &str, flags: u32) -> AppResult<String> {
    let mut bytes = password.as_bytes().to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    if unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            flags,
            &mut output,
        )
    } == 0
    {
        bytes.fill(0);
        return unavailable("Windows could not protect the sandbox credential.");
    }
    let encrypted =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as HLOCAL) };
    bytes.fill(0);
    Ok(BASE64_STANDARD.encode(encrypted))
}

fn decrypt_password(encrypted: &str) -> AppResult<String> {
    let mut encrypted = BASE64_STANDARD
        .decode(encrypted)
        .map_err(|_| AppError::InvalidInput("Windows sandbox credential is malformed.".into()))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    if unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    } == 0
    {
        encrypted.fill(0);
        return unavailable("Windows could not unprotect the sandbox credential.");
    }
    let mut clear =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as HLOCAL) };
    encrypted.fill(0);
    let password = match String::from_utf8(clear.clone()) {
        Ok(password) => password,
        Err(_) => {
            clear.fill(0);
            return unavailable("Windows sandbox credential is invalid.");
        }
    };
    clear.fill(0);
    if password.is_empty() {
        return unavailable("Windows sandbox credential record is empty.");
    }
    Ok(password)
}

fn generate_password() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let value = format!("P!a9{}", hex::encode(bytes));
    bytes.fill(0);
    value
}

fn ensure_runner_binary(setup: &WindowsSetupV1) -> AppResult<()> {
    let source = std::env::current_exe()?;
    let bytes = fs::read(&source)?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    let destination = setup
        .paths
        .runner_root
        .join(format!("pastey-world-runner-{}.exe", &digest[..24]));
    if !destination.is_file() {
        let temp = setup
            .paths
            .runner_root
            .join(format!("runner-{}.tmp", uuid::Uuid::new_v4()));
        fs::copy(&source, &temp)?;
        set_exact_dacl(
            &temp,
            &host_and_subject_entries(
                setup,
                &setup.account_sid,
                FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            ),
            false,
        )?;
        fs::rename(temp, &destination)?;
    }
    Ok(())
}

fn runner_binary_path(setup: &WindowsSetupV1) -> AppResult<PathBuf> {
    let bytes = fs::read(std::env::current_exe()?)?;
    let digest = blake3::hash(&bytes).to_hex().to_string();
    let path = setup
        .paths
        .runner_root
        .join(format!("pastey-world-runner-{}.exe", &digest[..24]));
    if !path.is_file() {
        return unavailable("Windows trusted sandbox bootstrap is unavailable.");
    }
    Ok(path)
}

fn reconcile_firewall_rule(account_sid: &str) -> AppResult<()> {
    with_firewall_policy(|policy| unsafe {
        if policy.LocalPolicyModifyState().map_err(firewall_error)? != NET_FW_MODIFY_STATE_OK {
            return unavailable("Local Windows Firewall policy changes are overridden.");
        }
        verify_firewall_profiles(policy)?;
        let rules = policy.Rules().map_err(firewall_error)?;
        let name = BSTR::from(FIREWALL_RULE_NAME);
        let rule: INetFwRule3 = match rules.Item(&name) {
            Ok(existing) => existing.cast().map_err(firewall_error)?,
            Err(_) => {
                let rule: INetFwRule3 = CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER)
                    .map_err(firewall_error)?;
                rule.SetName(&name).map_err(firewall_error)?;
                configure_firewall_rule(&rule, account_sid)?;
                rules.Add(&rule).map_err(firewall_error)?;
                rule
            }
        };
        configure_firewall_rule(&rule, account_sid)?;
        verify_firewall_rule_object(&rule, account_sid)
    })
}

fn verify_firewall_rule(account_sid: &str) -> AppResult<()> {
    with_firewall_policy(|policy| unsafe {
        if policy.LocalPolicyModifyState().map_err(firewall_error)? != NET_FW_MODIFY_STATE_OK {
            return unavailable("Local Windows Firewall rules are ineffective.");
        }
        verify_firewall_profiles(policy)?;
        let rules = policy.Rules().map_err(firewall_error)?;
        let rule: INetFwRule3 = rules
            .Item(&BSTR::from(FIREWALL_RULE_NAME))
            .map_err(firewall_error)?
            .cast()
            .map_err(firewall_error)?;
        verify_firewall_rule_object(&rule, account_sid)
    })
}

fn with_firewall_policy(operation: impl FnOnce(&INetFwPolicy2) -> AppResult<()>) -> AppResult<()> {
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let should_uninitialize = initialized.is_ok();
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return Err(firewall_error(initialized));
    }
    let result = unsafe {
        CoCreateInstance::<_, INetFwPolicy2>(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
            .map_err(firewall_error)
            .and_then(|policy| operation(&policy))
    };
    if should_uninitialize {
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn configure_firewall_rule(rule: &INetFwRule3, account_sid: &str) -> AppResult<()> {
    let local_user = BSTR::from(format!("O:LSD:(A;;CC;;;{account_sid})"));
    rule.SetApplicationName(&BSTR::new())
        .map_err(firewall_error)?;
    rule.SetServiceName(&BSTR::new()).map_err(firewall_error)?;
    rule.SetDescription(&BSTR::from(FIREWALL_RULE_DESCRIPTION))
        .map_err(firewall_error)?;
    rule.SetDirection(NET_FW_RULE_DIR_OUT)
        .map_err(firewall_error)?;
    rule.SetAction(NET_FW_ACTION_BLOCK)
        .map_err(firewall_error)?;
    rule.SetEnabled(VARIANT_TRUE).map_err(firewall_error)?;
    rule.SetProfiles(NET_FW_PROFILE2_ALL.0)
        .map_err(firewall_error)?;
    rule.SetProtocol(NET_FW_IP_PROTOCOL_ANY.0)
        .map_err(firewall_error)?;
    rule.SetRemoteAddresses(&BSTR::from("*"))
        .map_err(firewall_error)?;
    rule.SetLocalAddresses(&BSTR::from("*"))
        .map_err(firewall_error)?;
    rule.SetInterfaceTypes(&BSTR::from("All"))
        .map_err(firewall_error)?;
    rule.SetLocalUserAuthorizedList(&local_user)
        .map_err(firewall_error)?;
    Ok(())
}

unsafe fn verify_firewall_rule_object(rule: &INetFwRule3, account_sid: &str) -> AppResult<()> {
    let expected_local_user = format!("O:LSD:(A;;CC;;;{account_sid})");
    let local_user = rule
        .LocalUserAuthorizedList()
        .map_err(firewall_error)?
        .to_string();
    let remote = rule.RemoteAddresses().map_err(firewall_error)?.to_string();
    let local = rule.LocalAddresses().map_err(firewall_error)?.to_string();
    let interfaces = rule.InterfaceTypes().map_err(firewall_error)?.to_string();
    if rule.Direction().map_err(firewall_error)? != NET_FW_RULE_DIR_OUT
        || rule.Action().map_err(firewall_error)? != NET_FW_ACTION_BLOCK
        || rule.Enabled().map_err(firewall_error)? != VARIANT_TRUE
        || rule.Profiles().map_err(firewall_error)? != NET_FW_PROFILE2_ALL.0
        || rule.Protocol().map_err(firewall_error)? != NET_FW_IP_PROTOCOL_ANY.0
        || remote != "*"
        || local != "*"
        || !interfaces.eq_ignore_ascii_case("All")
        || !rule.ApplicationName().map_err(firewall_error)?.is_empty()
        || !rule.ServiceName().map_err(firewall_error)?.is_empty()
        || !local_user.eq_ignore_ascii_case(&expected_local_user)
    {
        return unavailable("Windows Firewall sandbox rule is missing or widened.");
    }
    Ok(())
}

unsafe fn verify_firewall_profiles(policy: &INetFwPolicy2) -> AppResult<()> {
    for profile in [
        NET_FW_PROFILE2_DOMAIN,
        NET_FW_PROFILE2_PRIVATE,
        NET_FW_PROFILE2_PUBLIC,
    ] {
        if policy
            .get_FirewallEnabled(profile)
            .map_err(firewall_error)?
            != VARIANT_TRUE
        {
            return unavailable("Windows Firewall is disabled for a network profile.");
        }
    }
    Ok(())
}

fn firewall_error<T>(_error: T) -> AppError {
    AppError::InvalidInput("Windows Firewall sandbox policy is unavailable.".into())
}

fn host_only_entries(setup: &WindowsSetupV1) -> Vec<(Vec<u8>, u32)> {
    vec![
        (setup.host_sid.clone(), FILE_ALL_ACCESS),
        (setup.system_sid.clone(), FILE_ALL_ACCESS),
        (setup.administrators_sid.clone(), FILE_ALL_ACCESS),
        (setup.owner_rights_sid.clone(), READ_CONTROL),
    ]
}

fn host_and_subject_entries(
    setup: &WindowsSetupV1,
    subject: &[u8],
    subject_access: u32,
) -> Vec<(Vec<u8>, u32)> {
    let mut entries = host_only_entries(setup);
    entries.push((subject.to_vec(), subject_access));
    entries
}

fn set_exact_dacl(path: &Path, entries: &[(Vec<u8>, u32)], directory: bool) -> AppResult<()> {
    if entries.iter().any(|(sid, _)| sid.is_empty()) {
        return unavailable("Windows protected ACL subject is unavailable.");
    }
    let explicit = entries
        .iter()
        .map(|(sid, permissions)| {
            let mut entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: *permissions,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: if directory {
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
                } else {
                    0
                },
                ..Default::default()
            };
            entry.Trustee.TrusteeForm = TRUSTEE_IS_SID;
            entry.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
            entry.Trustee.ptstrName = sid_ptr(sid).cast();
            entry
        })
        .collect::<Vec<_>>();
    let mut acl: *mut ACL = ptr::null_mut();
    if unsafe {
        SetEntriesInAclW(
            explicit.len() as u32,
            explicit.as_ptr(),
            ptr::null_mut(),
            &mut acl,
        )
    } != 0
    {
        return unavailable("Windows could not construct a protected managed-root ACL.");
    }
    let mut path_wide = wide_os(path.as_os_str());
    let status = unsafe {
        SetNamedSecurityInfoW(
            path_wide.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null_mut(),
        )
    };
    unsafe { LocalFree(acl.cast()) };
    if status != 0 {
        return unavailable("Windows could not install a protected managed-root ACL.");
    }
    Ok(())
}

struct NetApiBufferV1(*mut u8);

impl Drop for NetApiBufferV1 {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { NetApiBufferFree(self.0.cast()) };
        }
    }
}

fn query_local_group_membership_count(name: &str) -> AppResult<u32> {
    let name_wide = wide(name);
    let mut buffer = ptr::null_mut();
    let mut entries_read = 0_u32;
    let mut total_entries = 0_u32;
    let status = unsafe {
        NetUserGetLocalGroups(
            ptr::null(),
            name_wide.as_ptr(),
            0,
            LG_INCLUDE_INDIRECT,
            &mut buffer,
            MAX_PREFERRED_LENGTH,
            &mut entries_read,
            &mut total_entries,
        )
    };
    let _buffer = NetApiBufferV1(buffer);
    if status != NERR_Success {
        return net_api_failure(
            "NetUserGetLocalGroups(local Pastey sandbox account)",
            status,
        );
    }
    if entries_read != total_entries {
        return unavailable(
            "NetUserGetLocalGroups returned incomplete local sandbox group membership.",
        );
    }
    Ok(entries_read)
}

fn lookup_local_account(name: &str) -> AppResult<Option<LocalAccountV1>> {
    let name_wide = wide(name);
    let mut buffer = ptr::null_mut();
    let status = unsafe { NetUserGetInfo(ptr::null(), name_wide.as_ptr(), 4, &mut buffer) };
    if status == NERR_UserNotFound {
        return Ok(None);
    }
    if status != NERR_Success {
        return net_api_failure("NetUserGetInfo(local Pastey sandbox account)", status);
    }
    if buffer.is_null() {
        return unavailable("NetUserGetInfo returned an empty local account record.");
    }
    let buffer = NetApiBufferV1(buffer);
    let info = unsafe { &*(buffer.0.cast::<USER_INFO_4>()) };
    let returned_name = wide_ptr_string(info.usri4_name)?;
    let valid_sid =
        !info.usri4_user_sid.is_null() && unsafe { IsValidSid(info.usri4_user_sid) } != 0;
    let local_group_membership_count = query_local_group_membership_count(name)?;
    let legacy_privilege = match info.usri4_priv {
        USER_PRIV_GUEST => LegacyPrivilegeClassV1::Guest,
        USER_PRIV_USER => LegacyPrivilegeClassV1::User,
        USER_PRIV_ADMIN => LegacyPrivilegeClassV1::Administrator,
        _ => LegacyPrivilegeClassV1::Unknown,
    };
    validate_exact_local_user_identity(
        name,
        &returned_name,
        valid_sid,
        legacy_privilege,
        info.usri4_flags == ACCOUNT_SETUP_FLAGS,
        info.usri4_auth_flags == 0,
        local_group_membership_count,
    )
    .map_err(|reason| {
        AppError::InvalidInput(format!(
            "NetUserGetInfo did not resolve the exact local Pastey sandbox user: {reason}."
        ))
    })?;
    let sid = copy_sid(info.usri4_user_sid)?;
    let sid_string = sid_to_string(sid_ptr(&sid))?;
    Ok(Some(LocalAccountV1 {
        name: returned_name,
        sid,
        sid_string,
        comment: wide_ptr_string(info.usri4_comment)?,
        flags: info.usri4_flags,
        home_dir: wide_ptr_string(info.usri4_home_dir)?,
        script_path: wide_ptr_string(info.usri4_script_path)?,
    }))
}

fn lookup_account_sid(name: &str) -> AppResult<Vec<u8>> {
    lookup_local_account(name)?
        .map(|account| account.sid)
        .ok_or_else(|| {
            AppError::InvalidInput(format!(
                "NetUserGetInfo could not find the exact local {name} user (NET_API_STATUS {NERR_UserNotFound})."
            ))
        })
}

fn require_local_account() -> AppResult<LocalAccountV1> {
    lookup_local_account(SANDBOX_USERNAME)?.ok_or_else(|| {
        AppError::InvalidInput(format!(
            "NetUserGetInfo could not find the exact local {SANDBOX_USERNAME} user (NET_API_STATUS {NERR_UserNotFound})."
        ))
    })
}

fn require_local_account_sid(expected_sid: &str) -> AppResult<Vec<u8>> {
    let account = require_local_account()?;
    if account.sid_string != expected_sid {
        return unavailable("Windows sandbox principal identity was replaced during setup.");
    }
    Ok(account.sid)
}

fn require_legacy_partial_account_sid(expected_sid: &str) -> AppResult<Vec<u8>> {
    let account = require_local_account()?;
    if account.sid_string != expected_sid || !account.matches_legacy_partial_fingerprint() {
        return unavailable(
            "The pre-marker Windows sandbox account no longer matches Pastey's legacy partial-setup fingerprint.",
        );
    }
    Ok(account.sid)
}

fn authenticate_local_account(password: &str, expected_sid: &[u8]) -> AppResult<()> {
    let username = wide(SANDBOX_USERNAME);
    let domain = wide(".");
    let mut password_wide = wide(password);
    let mut token = ptr::null_mut();
    let authenticated = unsafe {
        LogonUserW(
            username.as_ptr(),
            domain.as_ptr(),
            password_wide.as_ptr(),
            LOGON32_LOGON_INTERACTIVE,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        )
    };
    password_wide.fill(0);
    if authenticated == 0 {
        return win32_failure("LogonUserW(local Pastey sandbox credential proof)");
    }
    let token = unsafe { owned(token)? };
    if token_user_sid(&token)? != expected_sid {
        return unavailable(
            "Windows sandbox credential proof returned a substituted principal identity.",
        );
    }
    reject_privileged_token_groups(&token)
}

fn wide_ptr_string(value: *const u16) -> AppResult<String> {
    if value.is_null() {
        return Ok(String::new());
    }
    let mut length = 0_usize;
    while length < 32_768 && unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    if length == 32_768 {
        return unavailable("Windows local account metadata exceeded its safety bound.");
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(value, length) })
        .map_err(|_| AppError::InvalidInput("Windows local account metadata is not UTF-16.".into()))
}

fn net_api_failure<T>(operation: &str, status: u32) -> AppResult<T> {
    Err(AppError::InvalidInput(format!(
        "{operation} failed with NET_API_STATUS {status}: {}",
        io::Error::from_raw_os_error(status as i32)
    )))
}

fn win32_failure<T>(operation: &str) -> AppResult<T> {
    let status = unsafe { GetLastError() };
    Err(AppError::InvalidInput(format!(
        "{operation} failed with Win32 error {status}: {}",
        io::Error::from_raw_os_error(status as i32)
    )))
}

fn sid_from_string(value: &str) -> AppResult<Vec<u8>> {
    let value = wide(value);
    let mut sid: PSID = ptr::null_mut();
    if unsafe {
        windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW(
            value.as_ptr(),
            &mut sid,
        )
    } == 0
    {
        return unavailable("Windows sandbox logon SID is malformed.");
    }
    let result = copy_sid(sid);
    unsafe { LocalFree(sid as HLOCAL) };
    result
}

fn sid_to_string(sid: PSID) -> AppResult<String> {
    let mut value = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut value) } == 0 {
        return unavailable("Windows SID string conversion failed.");
    }
    let mut length = 0_usize;
    while unsafe { *value.add(length) } != 0 {
        length += 1;
    }
    let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(value, length) });
    unsafe { LocalFree(value as HLOCAL) };
    Ok(result)
}

fn copy_sid(sid: PSID) -> AppResult<Vec<u8>> {
    if sid.is_null() {
        return unavailable("Windows SID is unavailable.");
    }
    let length = unsafe { GetLengthSid(sid) } as usize;
    if length == 0 {
        return unavailable("Windows SID length is invalid.");
    }
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length).to_vec() })
}

fn sid_ptr(bytes: &[u8]) -> PSID {
    bytes.as_ptr().cast_mut().cast()
}

fn copy_without_reparse(source: &Path, destination: &Path, quota: u64) -> AppResult<()> {
    let metadata = fs::symlink_metadata(source)?;
    reject_reparse(&metadata)?;
    if metadata.is_file() {
        if metadata.len() > quota {
            return invalid("Windows staged resource exceeds its exact quota.");
        }
        fs::copy(source, destination)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return invalid("Windows staged resource is not a regular file or directory.");
    }
    fs::create_dir(destination)?;
    let canonical = source.canonicalize()?;
    let mut total = 0_u64;
    let mut pending = vec![canonical.clone()];
    while let Some(from) = pending.pop() {
        for entry in fs::read_dir(&from)? {
            let entry = entry?;
            let child = entry.path();
            let metadata = fs::symlink_metadata(&child)?;
            reject_reparse(&metadata)?;
            let relative = child.strip_prefix(&canonical).map_err(|_| {
                AppError::InvalidInput("Windows staged resource escaped its lease.".into())
            })?;
            let target = destination.join(relative);
            if metadata.is_dir() {
                fs::create_dir(&target)?;
                pending.push(child);
            } else if metadata.is_file() {
                total = total.checked_add(metadata.len()).ok_or_else(|| {
                    AppError::InvalidInput("Windows staged resource quota overflowed.".into())
                })?;
                if total > quota {
                    return invalid("Windows staged resource exceeds its exact quota.");
                }
                fs::copy(child, target)?;
            } else {
                return invalid("Windows staged resource contains a special file.");
            }
        }
    }
    Ok(())
}

fn reject_reparse(metadata: &fs::Metadata) -> AppResult<()> {
    if metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return invalid("Windows staged resource contains a reparse point.");
    }
    Ok(())
}

fn environment_block(environment: &BTreeMap<String, String>) -> Vec<u16> {
    let mut bindings = environment.iter().collect::<Vec<_>>();
    bindings.sort_by(|(left, _), (right, _)| {
        left.to_ascii_uppercase()
            .cmp(&right.to_ascii_uppercase())
            .then(left.cmp(right))
    });
    let mut block = Vec::new();
    for (name, value) in bindings {
        block.extend(OsStr::new(&format!("{name}={value}")).encode_wide());
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

fn windows_command_line(executable: &Path, argv: &[String]) -> Vec<u16> {
    let mut value = quote_windows(executable.as_os_str());
    for argument in argv {
        value.push(' ');
        value.push_str(&quote_windows(OsStr::new(argument)));
    }
    wide(&value)
}

fn quote_windows(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    let mut result = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                result.push_str(&"\\".repeat(slashes * 2 + 1));
                result.push('"');
                slashes = 0;
            }
            _ => {
                result.push_str(&"\\".repeat(slashes));
                slashes = 0;
                result.push(character);
            }
        }
    }
    result.push_str(&"\\".repeat(slashes * 2));
    result.push('"');
    result
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn short_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..16].to_string()
}

fn file_from_handle(handle: OwnedHandle) -> File {
    unsafe { File::from_raw_handle(std::os::windows::io::IntoRawHandle::into_raw_handle(handle)) }
}

unsafe fn owned(handle: HANDLE) -> AppResult<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error().into());
    }
    Ok(OwnedHandle::from_raw_handle(handle as RawHandle))
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle() as HANDLE
}

fn native_conformance_probe(required: &BTreeSet<ConfinementPropertyV1>) -> AppResult<String> {
    let setup = load_setup()?;
    let current = std::env::current_exe()?;
    let probe_root = std::env::temp_dir().join(format!(
        "pastey-windows-world-probe-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&probe_root)?;
    set_exact_dacl(&probe_root, &host_only_entries(&setup), true)?;
    let denied = probe_root.join("host-secret.txt");
    fs::write(&denied, b"must remain denied")?;
    set_exact_dacl(&denied, &host_only_entries(&setup), false)?;
    let source_root = probe_root.join("sources");
    let scratch_root = source_root.join("scratch");
    let output_root = source_root.join("output");
    fs::create_dir_all(&scratch_root)?;
    fs::create_dir_all(&output_root)?;
    let input = source_root.join("input.txt");
    fs::write(&input, b"probe-input")?;
    let world_a_ref: ExecutionWorldRefV1 =
        serde_json::from_value(serde_json::json!("probe-world-a"))?;
    let world_b_ref: ExecutionWorldRefV1 =
        serde_json::from_value(serde_json::json!("probe-world-b"))?;
    let mounts_a = vec![
        probe_mount(
            "probe-a-executable",
            ResourceKindV1::Executable,
            current.clone(),
            false,
        ),
        probe_mount(
            "probe-a-input",
            ResourceKindV1::ManagedRevision,
            input.clone(),
            false,
        ),
        probe_mount(
            "probe-a-scratch",
            ResourceKindV1::Scratch,
            scratch_root.clone(),
            true,
        ),
        probe_mount(
            "probe-a-output",
            ResourceKindV1::OutputSlot,
            output_root,
            true,
        ),
    ];
    let other_input = source_root.join("other-input.txt");
    fs::write(&other_input, b"other-run-secret")?;
    let other_scratch = source_root.join("other-scratch");
    fs::create_dir(&other_scratch)?;
    let mounts_b = vec![
        probe_mount(
            "probe-b-executable",
            ResourceKindV1::Executable,
            current.clone(),
            false,
        ),
        probe_mount(
            "probe-b-input",
            ResourceKindV1::ManagedRevision,
            other_input,
            false,
        ),
        probe_mount(
            "probe-b-scratch",
            ResourceKindV1::Scratch,
            other_scratch,
            true,
        ),
    ];
    let result = (|| -> AppResult<String> {
        let (prepared_b, world_b) = prepare_world(&world_b_ref, &mounts_b)?;
        let executable_b = find_mount(&prepared_b, ResourceKindV1::Executable)?;
        let scratch_b = find_mount(&prepared_b, ResourceKindV1::Scratch)?;
        let input_b = find_mount(&prepared_b, ResourceKindV1::ManagedRevision)?;
        let mut sleeping = spawn(
            world_b,
            &executable_b.source_path,
            &[TEST_SLEEP.into()],
            &BTreeMap::new(),
            false,
            Some(&scratch_b.source_path),
            10_000,
            256 * 1024 * 1024,
        )?;
        let (prepared_a, world_a) = prepare_world(&world_a_ref, &mounts_a)?;
        let executable_a = find_mount(&prepared_a, ResourceKindV1::Executable)?;
        let input_a = find_mount(&prepared_a, ResourceKindV1::ManagedRevision)?;
        let scratch_a = find_mount(&prepared_a, ResourceKindV1::Scratch)?;
        let output_a = find_mount(&prepared_a, ResourceKindV1::OutputSlot)?;
        let marker = scratch_a.source_path.join("detached-child-marker.txt");
        let sentinel = File::open(&denied)?;
        unsafe {
            SetHandleInformation(
                sentinel.as_raw_handle() as HANDLE,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            );
        }
        std::env::set_var("PASTEY_HOST_SECRET_SENTINEL", "must-not-cross");
        let args = vec![
            PROBE_PARENT.into(),
            input_a.source_path.to_string_lossy().into_owned(),
            scratch_a.source_path.to_string_lossy().into_owned(),
            output_a.source_path.to_string_lossy().into_owned(),
            denied.to_string_lossy().into_owned(),
            setup.paths.marker.to_string_lossy().into_owned(),
            input_b.source_path.to_string_lossy().into_owned(),
            marker.to_string_lossy().into_owned(),
            (sentinel.as_raw_handle() as usize).to_string(),
            executable_a.source_path.to_string_lossy().into_owned(),
        ];
        let environment = BTreeMap::from([("PASTEY_ALLOWED_PROBE".into(), "ok".into())]);
        let mut spawned = spawn(
            world_a,
            &executable_a.source_path,
            &args,
            &environment,
            false,
            Some(&scratch_a.source_path),
            4_000,
            256 * 1024 * 1024,
        )?;
        std::env::remove_var("PASTEY_HOST_SECRET_SENTINEL");
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = spawned.process.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                spawned.process.terminate_tree();
                sleeping.process.terminate_tree();
                return unavailable("Windows native confinement probe timed out.");
            }
            thread::sleep(Duration::from_millis(10));
        };
        spawned.process.terminate_tree();
        sleeping.process.terminate_tree();
        thread::sleep(Duration::from_millis(1_400));
        if !status.success()
            || marker.exists()
            || fs::read(scratch_a.source_path.join("scratch-ok.txt"))
                .ok()
                .as_deref()
                != Some(b"scratch")
            || fs::read(output_a.source_path.join("output-ok.txt"))
                .ok()
                .as_deref()
                != Some(b"output")
        {
            return unavailable("Windows native confinement behavior was incomplete.");
        }
        let bytes = fs::read(&current)?;
        crate::execution_world::domain_hash(
            "pastey-windows-execution-world-identity-v2",
            &(
                WINDOWS_WORLD_ADAPTER_VERSION,
                EXECUTION_WORLD_VERSION,
                required,
                setup.marker.account_sid,
                setup.marker.firewall_rule_name,
                blake3::hash(&bytes).to_hex().to_string(),
            ),
        )
    })();
    std::env::remove_var("PASTEY_HOST_SECRET_SENTINEL");
    let _ = fs::remove_dir_all(&probe_root);
    result
}

fn find_mount(
    mounts: &[ExecutionWorldMountV1],
    kind: ResourceKindV1,
) -> AppResult<&ExecutionWorldMountV1> {
    mounts
        .iter()
        .find(|mount| mount.kind == kind)
        .ok_or_else(|| AppError::InvalidInput("Windows probe mount is unavailable.".into()))
}

fn probe_mount(
    handle: &str,
    kind: ResourceKindV1,
    source_path: PathBuf,
    writable: bool,
) -> ExecutionWorldMountV1 {
    let handle_ref: ResourceHandleRefV1 =
        serde_json::from_value(serde_json::json!(handle)).unwrap();
    ExecutionWorldMountV1 {
        mount_name: short_hash(handle_ref.as_str().as_bytes()),
        handle_ref,
        kind,
        source_path,
        writable,
        quota_bytes: 512 * 1024 * 1024,
        allowed_verbs: BTreeSet::new(),
        private_overlay: writable,
        initial_bytes: 0,
    }
}

/// Runs before Tauri initialization for setup, the trusted bootstrap, and the
/// native verifier. None of these modes is a Worker-selected command.
pub(crate) fn run_probe_helper_if_requested() -> bool {
    let arguments = std::env::args().collect::<Vec<_>>();
    match arguments.get(1).map(String::as_str) {
        Some(SETUP_CLI) => match run_elevated_setup() {
            Ok(()) => {
                println!("PASTEY_WINDOWS_EXECUTION_WORLD_SETUP_OK");
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("PASTEY_WINDOWS_EXECUTION_WORLD_SETUP_FAILED: {error}");
                std::process::exit(1);
            }
        },
        Some(VERIFICATION_CLI) => {
            let availability = availability(crate::execution_world::required_properties());
            if availability.available {
                println!("PASTEY_WINDOWS_EXECUTION_WORLD_VERIFIED");
                std::process::exit(0);
            }
            eprintln!(
                "PASTEY_WINDOWS_EXECUTION_WORLD_UNAVAILABLE: {}",
                availability
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("native conformance failed")
            );
            std::process::exit(1);
        }
        Some(RUNNER_CLI) => {
            let Some(control_pipe) = arguments.get(2) else {
                std::process::exit(70);
            };
            let code = if runner_main(control_pipe).is_ok() {
                0
            } else {
                71
            };
            std::process::exit(code);
        }
        Some(TEST_BASIC) => {
            let valid = std::env::var("PASTEY_HOST_SECRET_SENTINEL").is_err()
                && std::env::var("PASTEY_ALLOWED_PROBE").ok().as_deref() == Some("ok");
            std::process::exit(if valid { 0 } else { 81 });
        }
        Some(TEST_SLEEP) => loop {
            thread::sleep(Duration::from_secs(1));
        },
        Some(TEST_SPAM) => loop {
            println!("0123456789abcdef");
        },
        Some(PROBE_CHILD) => {
            let Some(marker) = arguments.get(2) else {
                std::process::exit(90);
            };
            thread::sleep(Duration::from_millis(1_200));
            std::process::exit(if fs::write(marker, b"escaped").is_ok() {
                0
            } else {
                91
            });
        }
        Some(PROBE_PARENT) => run_probe_parent(&arguments),
        _ => false,
    }
}

fn run_probe_parent(arguments: &[String]) -> ! {
    let success = (|| -> bool {
        let (
            Some(input),
            Some(scratch),
            Some(output),
            Some(denied),
            Some(private_state),
            Some(other_run),
            Some(marker),
            Some(sentinel),
            Some(executable),
        ) = (
            arguments.get(2),
            arguments.get(3),
            arguments.get(4),
            arguments.get(5),
            arguments.get(6),
            arguments.get(7),
            arguments.get(8),
            arguments.get(9),
            arguments.get(10),
        )
        else {
            return false;
        };
        if fs::read(input).ok().as_deref() != Some(b"probe-input")
            || fs::write(input, b"mutated").is_ok()
            || fs::write(Path::new(scratch).join("scratch-ok.txt"), b"scratch").is_err()
            || fs::write(Path::new(output).join("output-ok.txt"), b"output").is_err()
            || can_open_for_acl_rewrite(&Path::new(scratch).join("scratch-ok.txt"))
            || fs::read(denied).is_ok()
            || fs::write(denied, b"mutated").is_ok()
            || fs::read(private_state).is_ok()
            || fs::read(other_run).is_ok()
            || std::env::var("PASTEY_HOST_SECRET_SENTINEL").is_ok()
            || std::env::var("PASTEY_ALLOWED_PROBE").ok().as_deref() != Some("ok")
        {
            return false;
        }
        let allowed = [
            "SYSTEMROOT",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "PATH",
            "PATHEXT",
            "PASTEY_ALLOWED_PROBE",
        ];
        if std::env::vars().any(|(name, _)| {
            !allowed
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(&name))
        }) {
            return false;
        }
        let sentinel = sentinel.parse::<usize>().ok().unwrap_or_default() as HANDLE;
        let mut flags = 0_u32;
        if unsafe { GetHandleInformation(sentinel, &mut flags) } != 0 {
            return false;
        }
        for address in [
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9),
        ] {
            let result = TcpStream::connect_timeout(&address, Duration::from_millis(500));
            if result.as_ref().err().and_then(io::Error::raw_os_error) != Some(10013) {
                return false;
            }
        }
        if std::process::Command::new(executable)
            .arg(PROBE_CHILD)
            .arg(marker)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .env_clear()
            .spawn()
            .is_ok()
        {
            return false;
        }
        let child = std::process::Command::new(executable)
            .arg(PROBE_CHILD)
            .arg(marker)
            .env_clear()
            .spawn();
        let Ok(child) = child else {
            return false;
        };
        let mut contained = 0;
        if unsafe {
            IsProcessInJob(
                child.as_raw_handle() as HANDLE,
                ptr::null_mut(),
                &mut contained,
            )
        } == 0
            || contained == 0
        {
            return false;
        }
        let spawned = (0..(MAX_JOB_PROCESSES + 4))
            .filter(|_| {
                std::process::Command::new(executable)
                    .arg(PROBE_CHILD)
                    .arg(marker)
                    .env_clear()
                    .spawn()
                    .is_ok()
            })
            .count();
        spawned > 0 && spawned < (MAX_JOB_PROCESSES + 4) as usize
    })();
    std::process::exit(if success { 0 } else { 92 });
}

fn can_open_for_acl_rewrite(path: &Path) -> bool {
    let path = wide_os(path.as_os_str());
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            WRITE_DAC,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    unsafe { owned(handle) }.is_ok()
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn unavailable<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting_and_environment_blocks_are_deterministic() {
        assert_eq!(quote_windows(OsStr::new("a b")), "\"a b\"");
        assert_eq!(quote_windows(OsStr::new("a\\\"b")), "\"a\\\\\\\"b\"");
        assert_eq!(environment_block(&BTreeMap::new()), vec![0, 0]);
    }

    #[test]
    fn resource_layout_keeps_tools_inputs_and_writable_roots_distinct() {
        assert_ne!(ResourceKindV1::Executable, ResourceKindV1::ManagedRevision);
        assert_ne!(ResourceKindV1::Scratch, ResourceKindV1::OutputSlot);
        assert_eq!(SANDBOX_USERNAME, "PasteySandboxOffline");
        assert!(!WINDOWS_WORLD_ADAPTER_VERSION.contains("appcontainer"));
    }

    #[test]
    fn native_windows_execution_world_is_complete_or_explicitly_unavailable() {
        let required = crate::execution_world::required_properties();
        let availability = availability(required.clone());
        if availability.available {
            assert_eq!(
                availability.kind,
                PlatformWorldKindV1::WindowsRestrictedPrincipal
            );
            assert!(availability.verified_properties.is_superset(&required));
        } else {
            assert!(availability.verified_properties.is_empty());
            assert!(availability.unavailable_reason.is_some());
        }
    }
}
