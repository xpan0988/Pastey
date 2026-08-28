//! Host-private Windows managed execution adapter.
//!
//! The adapter creates a unique capabilityless AppContainer for each world,
//! stages only the exact leased resources beneath one private ACL root, and
//! creates the process suspended with an explicit stdio-only handle list. The
//! complete process tree is assigned to a non-breakaway, kill-on-close Job
//! before its first instruction runs.

#![cfg(windows)]

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{c_void, OsStr},
    fs::{self, File},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle},
    },
    path::{Path, PathBuf},
    process::ExitStatus,
    ptr,
    sync::{Arc, OnceLock},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, STILL_ACTIVE,
    },
    Security::{
        Authorization::{
            GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
            GRANT_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER,
        },
        FreeSid, GetLengthSid,
        Isolation::DeriveAppContainerSidFromAppContainerName,
        ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE,
        PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    },
    Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT,
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicUIRestrictions,
            JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
            JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_JOB_TIME,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
            JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
            JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
            JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
            JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        },
        Pipes::CreatePipe,
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
            InitializeProcThreadAttributeList, ResumeThread, UpdateProcThreadAttribute,
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

use crate::{
    effect_authority::{
        ConfinementPropertyV1, ExecutionWorldRefV1, ResourceHandleRefV1, ResourceKindV1,
    },
    error::{AppError, AppResult},
    execution_world::{ExecutionWorldAvailabilityV1, PlatformWorldKindV1, EXECUTION_WORLD_VERSION},
    managed_resources::ExecutionWorldMountV1,
};

const WINDOWS_WORLD_ADAPTER_VERSION: &str = "pastey-windows-appcontainer-job-v1";
const MAX_JOB_PROCESSES: u32 = 16;
const PROBE_PARENT: &str = "--pastey-windows-world-probe-parent-v1";
const PROBE_CHILD: &str = "--pastey-windows-world-probe-child-v1";
pub(crate) const TEST_BASIC: &str = "--pastey-windows-world-test-basic-v1";
pub(crate) const TEST_SLEEP: &str = "--pastey-windows-world-test-sleep-v1";
pub(crate) const TEST_SPAM: &str = "--pastey-windows-world-test-spam-v1";
const VERIFICATION_CLI: &str = "--pastey-verify-windows-execution-world-v1";

pub(crate) struct WindowsWorldV1 {
    root: PathBuf,
    sid: Vec<u8>,
}

impl WindowsWorldV1 {
    fn sid(&self) -> PSID {
        self.sid.as_ptr().cast_mut().cast()
    }
}

impl Drop for WindowsWorldV1 {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(crate) struct WindowsProcessV1 {
    process: OwnedHandle,
    job: OwnedHandle,
    _world: Arc<WindowsWorldV1>,
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

pub(crate) struct WindowsSpawnedProcessV1 {
    pub(crate) process: WindowsProcessV1,
    pub(crate) stdin: Option<File>,
    pub(crate) stdout: File,
    pub(crate) stderr: File,
}

pub(crate) fn availability(
    required: BTreeSet<ConfinementPropertyV1>,
) -> ExecutionWorldAvailabilityV1 {
    static AVAILABILITY: OnceLock<ExecutionWorldAvailabilityV1> = OnceLock::new();
    AVAILABILITY
        .get_or_init(|| match native_conformance_probe(&required) {
            Ok(identity_digest) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsAppContainer,
                available: true,
                identity_digest,
                verified_properties: required,
                unavailable_reason: None,
            },
            Err(_) => ExecutionWorldAvailabilityV1 {
                kind: PlatformWorldKindV1::WindowsAppContainer,
                available: false,
                identity_digest: "pastey-windows-appcontainer-unverified-v1".into(),
                verified_properties: BTreeSet::new(),
                unavailable_reason: Some(
                    "The native AppContainer, ACL, handle-list, Job, descendant, filesystem, and NoRawNetwork probe did not complete successfully."
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
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let profile = format!("Pastey.ManagedWorld.{nonce}");
    let profile_name = wide(&profile);
    let mut sid: PSID = ptr::null_mut();
    let result =
        unsafe { DeriveAppContainerSidFromAppContainerName(profile_name.as_ptr(), &mut sid) };
    if result < 0 || sid.is_null() {
        return unavailable("Windows could not derive a unique AppContainer identity.");
    }
    let sid_bytes = unsafe {
        let length = GetLengthSid(sid) as usize;
        if length == 0 {
            FreeSid(sid);
            return unavailable("Windows returned an invalid AppContainer SID.");
        }
        let bytes = std::slice::from_raw_parts(sid.cast::<u8>(), length).to_vec();
        FreeSid(sid);
        bytes
    };
    let root = std::env::temp_dir().join(format!(
        "pastey-managed-world-{}-{}",
        short_hash(world_ref.as_str().as_bytes()),
        nonce
    ));
    if let Err(error) = fs::create_dir(&root) {
        return Err(error.into());
    }
    let world = Arc::new(WindowsWorldV1 {
        root,
        sid: sid_bytes,
    });
    let result = stage_mounts(&world, mounts);
    match result {
        Ok(prepared) => Ok((prepared, world)),
        Err(error) => Err(error),
    }
}

fn stage_mounts(
    world: &WindowsWorldV1,
    mounts: &[ExecutionWorldMountV1],
) -> AppResult<Vec<ExecutionWorldMountV1>> {
    grant_world_root(&world.root, world.sid())?;
    let mut prepared = Vec::with_capacity(mounts.len());
    for mount in mounts {
        let destination = world.root.join(&mount.mount_name);
        copy_without_reparse(&mount.source_path, &destination, mount.quota_bytes)?;
        grant_tree(&destination, world.sid(), mount.writable)?;
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
    let job = create_job(cpu_millis, memory_bytes)?;
    let (stdin_parent, stdin_child) = if stdin_requested {
        let (read, write) = pipe_pair(false)?;
        (Some(write), read)
    } else {
        let null = open_null(true)?;
        (None, null)
    };
    let (stdout_parent, stdout_child) = pipe_pair(true)?;
    let (stderr_parent, stderr_child) = pipe_pair(true)?;
    let child_handles = [raw(&stdin_child), raw(&stdout_child), raw(&stderr_child)];
    let mut security_capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: world.sid(),
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let mut attributes = AttributeList::new(2)?;
    attributes.update(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
        (&mut security_capabilities as *mut SECURITY_CAPABILITIES).cast(),
        std::mem::size_of::<SECURITY_CAPABILITIES>(),
    )?;
    attributes.update(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        child_handles.as_ptr().cast(),
        std::mem::size_of_val(&child_handles),
    )?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_handles[0];
    startup.StartupInfo.hStdOutput = child_handles[1];
    startup.StartupInfo.hStdError = child_handles[2];
    startup.lpAttributeList = attributes.ptr;
    let executable_wide = wide_os(executable.as_os_str());
    let mut command_line = windows_command_line(executable, argv);
    let environment = environment_block(environment);
    let cwd_wide = cwd.map(|value| wide_os(value.as_os_str()));
    let mut process_info = PROCESS_INFORMATION::default();
    let created = unsafe {
        CreateProcessW(
            executable_wide.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT
                | EXTENDED_STARTUPINFO_PRESENT,
            environment.as_ptr().cast(),
            cwd_wide
                .as_ref()
                .map_or(ptr::null(), |value| value.as_ptr()),
            (&startup as *const STARTUPINFOEXW).cast(),
            &mut process_info,
        )
    };
    if created == 0 {
        return unavailable("Windows AppContainer process creation failed.");
    }
    let process = unsafe { owned(process_info.hProcess)? };
    let thread_handle = unsafe { owned(process_info.hThread)? };
    if unsafe { AssignProcessToJobObject(raw(&job), raw(&process)) } == 0 {
        unsafe { windows_sys::Win32::System::Threading::TerminateProcess(raw(&process), 1) };
        return unavailable("Windows could not contain the suspended process in its Job.");
    }
    if unsafe { ResumeThread(raw(&thread_handle)) } == u32::MAX {
        unsafe { TerminateJobObject(raw(&job), 1) };
        return unavailable("Windows could not resume the contained process.");
    }
    drop(thread_handle);
    drop(stdin_child);
    drop(stdout_child);
    drop(stderr_child);
    Ok(WindowsSpawnedProcessV1 {
        process: WindowsProcessV1 {
            process,
            job,
            _world: world,
        },
        stdin: stdin_parent.map(file_from_handle),
        stdout: file_from_handle(stdout_parent),
        stderr: file_from_handle(stderr_parent),
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
    Ok(job)
}

struct AttributeList {
    storage: Vec<usize>,
    ptr: *mut c_void,
}

impl AttributeList {
    fn new(count: u32) -> AppResult<Self> {
        let mut bytes = 0_usize;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut bytes);
        }
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

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.ptr) };
    }
}

fn pipe_pair(parent_reads: bool) -> AppResult<(OwnedHandle, OwnedHandle)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return unavailable("Windows contained stdio pipe creation failed.");
    }
    let read = unsafe { owned(read)? };
    let write = unsafe { owned(write)? };
    let parent = if parent_reads { &read } else { &write };
    if unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(raw(parent), HANDLE_FLAG_INHERIT, 0)
    } == 0
    {
        return unavailable("Windows could not make the Host pipe end non-inheritable.");
    }
    if parent_reads {
        Ok((read, write))
    } else {
        Ok((read, write))
    }
}

fn open_null(readable: bool) -> AppResult<OwnedHandle> {
    use std::fs::OpenOptions;
    let file = OpenOptions::new()
        .read(readable)
        .write(!readable)
        .open("NUL")?;
    let handle: OwnedHandle = file.into();
    if unsafe {
        windows_sys::Win32::Foundation::SetHandleInformation(
            raw(&handle),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    } == 0
    {
        return unavailable("Windows could not mark the null stdin handle inheritable.");
    }
    Ok(handle)
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

fn grant_tree(path: &Path, sid: PSID, writable: bool) -> AppResult<()> {
    let mut pending = vec![path.to_owned()];
    while let Some(current) = pending.pop() {
        let metadata = fs::symlink_metadata(&current)?;
        reject_reparse(&metadata)?;
        grant_path(&current, sid, writable, metadata.is_dir())?;
        if metadata.is_dir() {
            for entry in fs::read_dir(&current)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}

fn grant_path(path: &Path, sid: PSID, writable: bool, directory: bool) -> AppResult<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    };
    let permissions = if writable {
        FILE_ALL_ACCESS
    } else {
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
    };
    grant_path_permissions(
        path,
        sid,
        permissions,
        if directory {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        },
    )
}

fn grant_world_root(path: &Path, sid: PSID) -> AppResult<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_READ_ATTRIBUTES, FILE_TRAVERSE, READ_CONTROL, SYNCHRONIZE,
    };
    grant_path_permissions(
        path,
        sid,
        FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        0,
    )
}

fn grant_path_permissions(
    path: &Path,
    sid: PSID,
    permissions: u32,
    inheritance: u32,
) -> AppResult<()> {
    let mut path_wide = wide_os(path.as_os_str());
    let mut old_acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_mut_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut old_acl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return unavailable("Windows could not read the private world ACL.");
    }
    let mut entry = EXPLICIT_ACCESS_W::default();
    entry.grfAccessPermissions = permissions;
    entry.grfAccessMode = GRANT_ACCESS;
    entry.grfInheritance = inheritance;
    entry.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    entry.Trustee.TrusteeType = TRUSTEE_IS_USER;
    entry.Trustee.ptstrName = sid.cast();
    let mut new_acl: *mut ACL = ptr::null_mut();
    let merge = unsafe { SetEntriesInAclW(1, &entry, old_acl, &mut new_acl) };
    if merge == 0 {
        let set = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new_acl,
                ptr::null_mut(),
            )
        };
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(new_acl.cast());
            windows_sys::Win32::Foundation::LocalFree(descriptor.cast());
        }
        if set != 0 {
            return unavailable("Windows could not install the private world ACL.");
        }
        Ok(())
    } else {
        unsafe { windows_sys::Win32::Foundation::LocalFree(descriptor.cast()) };
        unavailable("Windows could not construct the private world ACL.")
    }
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
    let mut block = Vec::new();
    for (name, value) in environment {
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

fn native_conformance_probe(required: &BTreeSet<ConfinementPropertyV1>) -> AppResult<String> {
    let current = std::env::current_exe()?;
    let probe_root = std::env::temp_dir().join(format!(
        "pastey-windows-world-probe-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&probe_root)?;
    let denied = probe_root.join("host-secret.txt");
    fs::write(&denied, b"must remain denied")?;
    let source_root = probe_root.join("sources");
    let scratch_root = source_root.join("scratch");
    fs::create_dir_all(&scratch_root)?;
    let input = source_root.join("input.txt");
    fs::write(&input, b"probe-input")?;
    let executable_handle: ResourceHandleRefV1 =
        serde_json::from_value(serde_json::json!("probe-executable")).unwrap();
    let input_handle: ResourceHandleRefV1 =
        serde_json::from_value(serde_json::json!("probe-input")).unwrap();
    let scratch_handle: ResourceHandleRefV1 =
        serde_json::from_value(serde_json::json!("probe-scratch")).unwrap();
    let world_ref: ExecutionWorldRefV1 =
        serde_json::from_value(serde_json::json!("probe-world")).unwrap();
    let mounts = vec![
        probe_mount(
            executable_handle,
            ResourceKindV1::Executable,
            current.clone(),
            false,
        ),
        probe_mount(input_handle, ResourceKindV1::ManagedRevision, input, false),
        probe_mount(scratch_handle, ResourceKindV1::Scratch, scratch_root, true),
    ];
    let result = (|| -> AppResult<String> {
        let (prepared, world) = prepare_world(&world_ref, &mounts)?;
        let executable = prepared
            .iter()
            .find(|mount| mount.kind == ResourceKindV1::Executable)
            .unwrap();
        let input = prepared
            .iter()
            .find(|mount| mount.kind == ResourceKindV1::ManagedRevision)
            .unwrap();
        let scratch = prepared
            .iter()
            .find(|mount| mount.kind == ResourceKindV1::Scratch)
            .unwrap();
        let marker = scratch.source_path.join("detached-child-marker.txt");
        let sentinel = File::open(&denied)?;
        unsafe {
            windows_sys::Win32::Foundation::SetHandleInformation(
                sentinel.as_raw_handle() as HANDLE,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            );
        }
        let args = vec![
            PROBE_PARENT.into(),
            input.source_path.to_string_lossy().into_owned(),
            denied.to_string_lossy().into_owned(),
            marker.to_string_lossy().into_owned(),
            (sentinel.as_raw_handle() as usize).to_string(),
            executable.source_path.to_string_lossy().into_owned(),
        ];
        let mut spawned = spawn(
            world,
            &executable.source_path,
            &args,
            &BTreeMap::new(),
            false,
            Some(&scratch.source_path),
            2_000,
            256 * 1024 * 1024,
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = spawned.process.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                spawned.process.terminate_tree();
                return unavailable("Windows native confinement probe timed out.");
            }
            thread::sleep(Duration::from_millis(10));
        };
        spawned.process.terminate_tree();
        thread::sleep(Duration::from_millis(1_400));
        if !status.success() || marker.exists() {
            return unavailable("Windows native confinement behavior was incomplete.");
        }
        let bytes = fs::read(&current)?;
        crate::execution_world::domain_hash(
            "pastey-windows-execution-world-identity-v1",
            &(
                WINDOWS_WORLD_ADAPTER_VERSION,
                EXECUTION_WORLD_VERSION,
                required,
                blake3::hash(&bytes).to_hex().to_string(),
            ),
        )
    })();
    let _ = fs::remove_dir_all(&probe_root);
    result
}

fn probe_mount(
    handle_ref: ResourceHandleRefV1,
    kind: ResourceKindV1,
    source_path: PathBuf,
    writable: bool,
) -> ExecutionWorldMountV1 {
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

/// Runs before Tauri initialization when the native availability probe starts
/// this exact executable inside its AppContainer. It is not a Worker command.
pub(crate) fn run_probe_helper_if_requested() -> bool {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments.get(1).map(String::as_str) == Some(VERIFICATION_CLI) {
        let availability = availability(crate::execution_world::required_properties());
        if availability.available {
            println!("PASTEY_WINDOWS_EXECUTION_WORLD_VERIFIED");
            std::process::exit(0);
        }
        eprintln!("PASTEY_WINDOWS_EXECUTION_WORLD_UNAVAILABLE");
        std::process::exit(1);
    }
    if arguments.get(1).map(String::as_str) == Some(TEST_BASIC) {
        let Some(denied) = arguments.get(2) else {
            std::process::exit(80);
        };
        let valid = std::env::vars_os().next().is_none()
            && fs::read("project/input.txt").ok().as_deref() == Some(b"authoritative revision N")
            && fs::read(denied).is_err()
            && fs::write("windows-result.txt", b"generated").is_ok();
        if valid {
            println!("contained");
        }
        std::process::exit(if valid { 0 } else { 81 });
    }
    if arguments.get(1).map(String::as_str) == Some(TEST_SLEEP) {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    if arguments.get(1).map(String::as_str) == Some(TEST_SPAM) {
        loop {
            println!("0123456789abcdef");
        }
    }
    if arguments.get(1).map(String::as_str) == Some(PROBE_CHILD) {
        let Some(marker) = arguments.get(2) else {
            std::process::exit(90);
        };
        thread::sleep(Duration::from_millis(1_200));
        if fs::write(marker, b"escaped").is_err() {
            std::process::exit(91);
        }
        std::process::exit(0);
    }
    if arguments.get(1).map(String::as_str) != Some(PROBE_PARENT) {
        return false;
    }
    let success = (|| -> bool {
        let (Some(input), Some(denied), Some(marker), Some(sentinel), Some(executable)) = (
            arguments.get(2),
            arguments.get(3),
            arguments.get(4),
            arguments.get(5),
            arguments.get(6),
        ) else {
            return false;
        };
        if fs::read(input).ok().as_deref() != Some(b"probe-input")
            || fs::write(input, b"mutated").is_ok()
            || fs::read(denied).is_ok()
            || fs::read_dir("..").is_ok()
            || std::env::vars_os().next().is_some()
        {
            return false;
        }
        let sentinel = sentinel.parse::<usize>().ok().unwrap_or_default() as HANDLE;
        let mut flags = 0_u32;
        if unsafe { GetHandleInformation(sentinel, &mut flags) } != 0 {
            return false;
        }
        let network = TcpStream::connect_timeout(
            &SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443),
            Duration::from_millis(350),
        );
        if network.as_ref().err().and_then(io::Error::raw_os_error) != Some(10013) {
            return false;
        }
        use std::os::windows::process::CommandExt;
        if std::process::Command::new(executable)
            .arg(PROBE_CHILD)
            .arg(marker)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .is_ok()
        {
            return false;
        }
        let spawned = (0..(MAX_JOB_PROCESSES + 4))
            .filter(|_| {
                std::process::Command::new(executable)
                    .arg(PROBE_CHILD)
                    .arg(marker)
                    .spawn()
                    .is_ok()
            })
            .count();
        spawned > 0 && spawned < (MAX_JOB_PROCESSES + 4) as usize
    })();
    std::process::exit(if success { 0 } else { 92 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting_and_empty_environment_are_deterministic() {
        assert_eq!(quote_windows(OsStr::new("a b")), "\"a b\"");
        assert_eq!(quote_windows(OsStr::new("a\\\"b")), "\"a\\\\\\\"b\"");
        assert_eq!(environment_block(&BTreeMap::new()), vec![0, 0]);
    }

    #[test]
    fn native_windows_execution_world_is_complete_or_explicitly_unavailable() {
        let required = crate::execution_world::required_properties();
        let availability = availability(required.clone());
        if availability.available {
            assert_eq!(availability.kind, PlatformWorldKindV1::WindowsAppContainer);
            assert!(availability.verified_properties.is_superset(&required));
        } else {
            assert!(availability.verified_properties.is_empty());
            assert!(availability.unavailable_reason.is_some());
        }
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

fn unavailable<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}
