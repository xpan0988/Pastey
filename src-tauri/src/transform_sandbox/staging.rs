use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::ffi::CString;

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
};

#[cfg(windows)]
use std::os::windows::{
    fs::{MetadataExt as WindowsMetadataExt, OpenOptionsExt as WindowsOpenOptionsExt},
    io::AsRawHandle,
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_READ,
    },
};

use crate::{
    error::{AppError, AppResult},
    logging,
};

use super::profile::TransformStagingProfile;

const STAGING_DIRECTORY_NAME: &str = "transform-staging";
const STAGING_ID_PREFIX: &str = "transform-stage-";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceIdentity {
    pub(crate) digest: String,
    pub(crate) byte_count: u64,
    pub(crate) fingerprint: SourceFingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceFingerprint {
    device: u64,
    inode: u64,
    byte_count: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    link_count: u64,
}

/// Opaque, Rust-private staging state. It is neither a consent token nor an
/// execution capability, and it must never cross the Tauri boundary.
#[derive(Clone, Debug)]
pub(crate) struct StagedSnapshot {
    pub(crate) app_data_dir: PathBuf,
    pub(crate) staging_id: String,
    pub(crate) root: PathBuf,
    pub(crate) input_path: PathBuf,
    pub(crate) work_dir: PathBuf,
    #[cfg(test)]
    pub(crate) digest: String,
    #[cfg(test)]
    pub(crate) byte_count: u64,
}

pub(crate) fn capture_source_identity(
    source_path: &Path,
    scope_root: &Path,
    maximum_bytes: u64,
) -> AppResult<SourceIdentity> {
    let mut source = open_regular_source(source_path, scope_root)?;
    let before = source_fingerprint(&source)?;
    let (digest, byte_count) = digest_open_source(&mut source, maximum_bytes)?;
    let after = source_fingerprint(&source)?;
    if before != after || byte_count != before.byte_count {
        return Err(AppError::InvalidInput(
            "Artifact Transform candidate changed while its identity was captured.".into(),
        ));
    }
    Ok(SourceIdentity {
        digest,
        byte_count,
        fingerprint: before,
    })
}

/// Creates an immutable, normalized receiver-local copy. The source is opened
/// once by descriptor and is never reopened by path while bytes are copied.
pub(crate) fn prepare_staged_snapshot(
    app_data_dir: &Path,
    source_path: &Path,
    scope_root: &Path,
    expected_identity: &SourceIdentity,
    profile: TransformStagingProfile,
) -> AppResult<StagedSnapshot> {
    if expected_identity.digest.is_empty()
        || expected_identity.byte_count > profile.maximum_input_bytes
    {
        return Err(AppError::InvalidInput(
            "Artifact Transform staging identity is invalid.".into(),
        ));
    }
    let mut source = open_regular_source(source_path, scope_root)?;
    let before = source_fingerprint(&source)?;
    if before != expected_identity.fingerprint || before.byte_count > profile.maximum_input_bytes {
        return Err(AppError::InvalidInput(
            "Artifact Transform candidate changed before staging.".into(),
        ));
    }
    let parent = canonical_staging_parent(app_data_dir)?;
    let (staging_id, root) = create_staging_root(&parent)?;
    let result = stage_open_source(
        &mut source,
        &root,
        expected_identity,
        profile,
        before.clone(),
    );
    match result {
        #[cfg(test)]
        Ok((input_path, work_dir, digest, byte_count)) => Ok(StagedSnapshot {
            app_data_dir: app_data_dir.to_path_buf(),
            staging_id,
            root,
            input_path,
            work_dir,
            digest,
            byte_count,
        }),
        #[cfg(not(test))]
        Ok((input_path, work_dir, _, _)) => Ok(StagedSnapshot {
            app_data_dir: app_data_dir.to_path_buf(),
            staging_id,
            root,
            input_path,
            work_dir,
        }),
        Err(error) => {
            if remove_partial_staging_root(&parent, &root).is_err() {
                logging::write_error_line(
                    "[pastey:transform-staging] event=partial_cleanup_failed location=transform_staging_root error_code=cleanup_failed",
                );
            }
            Err(error)
        }
    }
}

pub(crate) fn canonical_staging_parent(app_data_dir: &Path) -> AppResult<PathBuf> {
    let app_data = fs::canonicalize(app_data_dir).map_err(|_| {
        AppError::InvalidInput("Transform staging app-data directory is unavailable.".into())
    })?;
    #[cfg(windows)]
    reject_windows_reparse_or_wrong_type(&app_data, true)?;
    let parent = app_data.join(STAGING_DIRECTORY_NAME);
    match fs::symlink_metadata(&parent) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(AppError::InvalidInput(
                    "Transform staging parent is invalid.".into(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => create_private_dir(&parent)?,
        Err(error) => return Err(error.into()),
    }
    let canonical = fs::canonicalize(&parent)?;
    if !canonical.starts_with(&app_data) || canonical != parent {
        return Err(AppError::InvalidInput(
            "Transform staging parent escaped app data.".into(),
        ));
    }
    #[cfg(windows)]
    reject_windows_reparse_or_wrong_type(&canonical, true)?;
    set_private_directory_permissions(&canonical)?;
    Ok(canonical)
}

pub(super) fn existing_staging_parent(app_data_dir: &Path) -> AppResult<Option<PathBuf>> {
    let app_data = fs::canonicalize(app_data_dir).map_err(|_| {
        AppError::InvalidInput("Transform staging app-data directory is unavailable.".into())
    })?;
    #[cfg(windows)]
    reject_windows_reparse_or_wrong_type(&app_data, true)?;
    let parent = app_data.join(STAGING_DIRECTORY_NAME);
    let metadata = match fs::symlink_metadata(&parent) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Transform staging parent is invalid.".into(),
        ));
    }
    let canonical = fs::canonicalize(&parent)?;
    if !canonical.starts_with(&app_data) || canonical != parent {
        return Err(AppError::InvalidInput(
            "Transform staging parent escaped app data.".into(),
        ));
    }
    #[cfg(windows)]
    reject_windows_reparse_or_wrong_type(&canonical, true)?;
    Ok(Some(canonical))
}

pub(crate) fn is_valid_staging_id(value: &str) -> bool {
    value
        .strip_prefix(STAGING_ID_PREFIX)
        .and_then(|uuid| uuid::Uuid::parse_str(uuid).ok())
        .is_some()
}

fn stage_open_source(
    source: &mut File,
    root: &Path,
    expected_identity: &SourceIdentity,
    profile: TransformStagingProfile,
    before: SourceFingerprint,
) -> AppResult<(PathBuf, PathBuf, String, u64)> {
    stage_open_source_with_before_copy(source, root, expected_identity, profile, before, || {})
}

fn stage_open_source_with_before_copy<F: FnOnce()>(
    source: &mut File,
    root: &Path,
    expected_identity: &SourceIdentity,
    profile: TransformStagingProfile,
    before: SourceFingerprint,
    before_copy: F,
) -> AppResult<(PathBuf, PathBuf, String, u64)> {
    let input_root = root.join("input");
    let work_dir = root.join("work");
    create_private_dir(&input_root)?;
    create_private_dir(&work_dir)?;
    let input_path = input_root.join(profile.internal_input_name);
    let mut destination = create_exclusive_staged_file(&input_path)?;
    before_copy();
    let (digest, byte_count) =
        copy_open_source_to_staging(source, &mut destination, profile.maximum_input_bytes)?;
    let after = source_fingerprint(source)?;
    if before != after
        || before != expected_identity.fingerprint
        || byte_count != expected_identity.byte_count
        || digest != expected_identity.digest
    {
        return Err(AppError::InvalidInput(
            "Artifact Transform candidate changed while staging.".into(),
        ));
    }
    destination.sync_all()?;
    set_read_only_file_permissions(&input_path)?;
    Ok((input_path, work_dir, digest, byte_count))
}

fn open_regular_source(source_path: &Path, scope_root: &Path) -> AppResult<File> {
    #[cfg(unix)]
    {
        let canonical_root = fs::canonicalize(scope_root).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source scope is unavailable.".into())
        })?;
        let relative = source_path.strip_prefix(scope_root).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source escaped its approved scope.".into())
        })?;
        let components: Vec<_> = relative.components().collect();
        if components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput(
                "Artifact Transform source path is invalid.".into(),
            ));
        }
        let root_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY)
            .open(&canonical_root)?;
        let mut directory = root_file;
        for component in &components[..components.len() - 1] {
            let name = CString::new(component.as_os_str().as_bytes()).map_err(|_| {
                AppError::InvalidInput("Artifact Transform source path is invalid.".into())
            })?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
                )
            };
            if fd < 0 {
                return Err(AppError::InvalidInput(
                    "Artifact Transform source directory changed.".into(),
                ));
            }
            directory = unsafe { File::from_raw_fd(fd) };
        }
        let name = CString::new(
            components
                .last()
                .expect("nonempty component list")
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|_| AppError::InvalidInput("Artifact Transform source path is invalid.".into()))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(AppError::InvalidInput(
                "Artifact Transform source is unavailable or unsafe.".into(),
            ));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let fingerprint = source_fingerprint(&file)?;
        if fingerprint.link_count != 1 {
            return Err(AppError::InvalidInput(
                "Artifact Transform source has unsupported hard links.".into(),
            ));
        }
        return Ok(file);
    }
    #[cfg(windows)]
    {
        let canonical_root = fs::canonicalize(scope_root).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source scope is unavailable.".into())
        })?;
        reject_windows_reparse_or_wrong_type(&canonical_root, true)?;
        let relative = source_path.strip_prefix(scope_root).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source escaped its approved scope.".into())
        })?;
        let components = relative.components().collect::<Vec<_>>();
        if components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput(
                "Artifact Transform source path is invalid.".into(),
            ));
        }
        let mut checked = canonical_root.clone();
        for component in &components[..components.len() - 1] {
            checked.push(component.as_os_str());
            reject_windows_reparse_or_wrong_type(&checked, true)?;
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
        let file = options.open(source_path).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source is unavailable or unsafe.".into())
        })?;
        let fingerprint = source_fingerprint(&file)?;
        if fingerprint.link_count != 1 {
            return Err(AppError::InvalidInput(
                "Artifact Transform source has unsupported hard links.".into(),
            ));
        }
        let opened_path = final_windows_handle_path(&file)?;
        let expected_path = fs::canonicalize(source_path).map_err(|_| {
            AppError::InvalidInput("Artifact Transform source changed while opening.".into())
        })?;
        let opened = normalized_windows_path(&opened_path);
        let expected = normalized_windows_path(&expected_path);
        let root = normalized_windows_path(&canonical_root);
        if opened != expected || !windows_path_is_within(&opened, &root) {
            return Err(AppError::InvalidInput(
                "Artifact Transform source escaped its approved scope.".into(),
            ));
        }
        return Ok(file);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source_path, scope_root);
        Err(AppError::InvalidInput(
            "Artifact Transform staging is not supported on this platform.".into(),
        ))
    }
}

fn source_fingerprint(file: &File) -> AppResult<SourceFingerprint> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(AppError::InvalidInput(
            "Artifact Transform source must be a regular file.".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_symlink()
            || file_type.is_dir()
            || file_type.is_fifo()
            || file_type.is_socket()
            || file_type.is_block_device()
            || file_type.is_char_device()
        {
            return Err(AppError::InvalidInput(
                "Artifact Transform source has an unsupported file type.".into(),
            ));
        }
        return Ok(SourceFingerprint {
            device: metadata.dev(),
            inode: metadata.ino(),
            byte_count: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
            link_count: metadata.nlink(),
        });
    }
    #[cfg(windows)]
    {
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) }
            == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let info = unsafe { info.assume_init() };
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(AppError::InvalidInput(
                "Artifact Transform source has an unsupported file type.".into(),
            ));
        }
        return Ok(SourceFingerprint {
            device: info.dwVolumeSerialNumber as u64,
            inode: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
            byte_count: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
            modified_seconds: info.ftLastWriteTime.dwHighDateTime as i64,
            modified_nanoseconds: info.ftLastWriteTime.dwLowDateTime as i64,
            changed_seconds: info.ftCreationTime.dwHighDateTime as i64,
            changed_nanoseconds: info.ftCreationTime.dwLowDateTime as i64,
            link_count: info.nNumberOfLinks as u64,
        });
    }
    #[cfg(not(any(unix, windows)))]
    Ok(SourceFingerprint {
        device: 0,
        inode: 0,
        byte_count: metadata.len(),
        modified_seconds: 0,
        modified_nanoseconds: 0,
        changed_seconds: 0,
        changed_nanoseconds: 0,
        link_count: 1,
    })
}

#[cfg(windows)]
fn reject_windows_reparse_or_wrong_type(path: &Path, expect_directory: bool) -> AppResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        AppError::InvalidInput("Artifact Transform source path is unavailable or unsafe.".into())
    })?;
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != expect_directory
    {
        return Err(AppError::InvalidInput(
            "Artifact Transform source path contains an unsafe reparse point or file type.".into(),
        ));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    let opened = options.open(path)?;
    let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    if unsafe { GetFileInformationByHandle(opened.as_raw_handle() as HANDLE, info.as_mut_ptr()) }
        == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != expect_directory
    {
        return Err(AppError::InvalidInput(
            "Artifact Transform source path changed or contains a reparse point.".into(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn final_windows_handle_path(file: &File) -> AppResult<PathBuf> {
    let handle = file.as_raw_handle() as HANDLE;
    let required = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0) };
    if required == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut buffer = vec![0_u16; required as usize + 1];
    let written =
        unsafe { GetFinalPathNameByHandleW(handle, buffer.as_mut_ptr(), buffer.len() as u32, 0) };
    if written == 0 || written as usize >= buffer.len() {
        return Err(std::io::Error::last_os_error().into());
    }
    buffer.truncate(written as usize);
    Ok(PathBuf::from(String::from_utf16(&buffer).map_err(
        |_| AppError::InvalidInput("Artifact Transform source handle path is invalid.".into()),
    )?))
}

#[cfg(windows)]
fn normalized_windows_path(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('/', "\\");
    if let Some(without_prefix) = value.strip_prefix("\\\\?\\UNC\\") {
        value = format!("\\\\{without_prefix}");
    } else if let Some(without_prefix) = value.strip_prefix("\\\\?\\") {
        value = without_prefix.to_owned();
    }
    value.trim_end_matches('\\').to_lowercase()
}

#[cfg(windows)]
fn windows_path_is_within(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn digest_open_source(source: &mut File, maximum_bytes: u64) -> AppResult<(String, u64)> {
    source.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.checked_add(read as u64).ok_or_else(|| {
            AppError::InvalidInput("Artifact Transform source is too large.".into())
        })?;
        if bytes > maximum_bytes {
            return Err(AppError::InvalidInput(
                "Artifact Transform source exceeds the staging limit.".into(),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

fn copy_open_source_to_staging(
    source: &mut File,
    destination: &mut File,
    maximum_bytes: u64,
) -> AppResult<(String, u64)> {
    source.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.checked_add(read as u64).ok_or_else(|| {
            AppError::InvalidInput("Artifact Transform staged input is too large.".into())
        })?;
        if bytes > maximum_bytes {
            return Err(AppError::InvalidInput(
                "Artifact Transform staged input exceeds the profile limit.".into(),
            ));
        }
        destination.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

fn create_staging_root(parent: &Path) -> AppResult<(String, PathBuf)> {
    for _ in 0..8 {
        let staging_id = format!("{STAGING_ID_PREFIX}{}", uuid::Uuid::new_v4());
        let root = parent.join(&staging_id);
        match create_private_dir(&root) {
            Ok(()) => return Ok((staging_id, root)),
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue
            }
            Err(error) => return Err(error),
        }
    }
    Err(AppError::InvalidInput(
        "Could not allocate a private Transform staging directory.".into(),
    ))
}

fn create_private_dir(path: &Path) -> AppResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    }
    builder.create(path)?;
    #[cfg(windows)]
    reject_windows_reparse_or_wrong_type(path, true)?;
    set_private_directory_permissions(path)
}

fn create_exclusive_staged_file(path: &Path) -> AppResult<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        options
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
    }
    Ok(options.open(path)?)
}

fn set_private_directory_permissions(_path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(_path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
    }
    Ok(())
}

fn set_read_only_file_permissions(path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o400))?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn remove_partial_staging_root(parent: &Path, root: &Path) -> AppResult<()> {
    super::cleanup::cleanup_partial_staging_root(parent, root)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;
    use crate::transform_sandbox::{
        cleanup::cleanup_staged_snapshot, profile::DETERMINISTIC_STAGED_INPUT_TEST,
    };

    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("pts-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn source(root: &Path, name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
        let scope = root.join("source");
        fs::create_dir(&scope).unwrap();
        let path = scope.join(name);
        fs::write(&path, bytes).unwrap();
        (scope, path)
    }

    #[test]
    fn stages_empty_one_byte_and_exact_limit_without_source_names() {
        for (name, bytes) in [
            ("original name.txt", Vec::new()),
            ("secret.pdf", vec![7]),
            ("very-private.bin", vec![9; 1024 * 1024]),
        ] {
            let root = root();
            let (scope, path) = source(&root, name, &bytes);
            let identity = capture_source_identity(&path, &scope, 1024 * 1024).unwrap();
            let snapshot = prepare_staged_snapshot(
                &root,
                &path,
                &scope,
                &identity,
                DETERMINISTIC_STAGED_INPUT_TEST,
            )
            .unwrap();
            assert_eq!(fs::read(&snapshot.input_path).unwrap(), bytes);
            assert_eq!(snapshot.byte_count, bytes.len() as u64);
            assert_eq!(snapshot.digest, identity.digest);
            assert_eq!(snapshot.input_path.file_name().unwrap(), "artifact");
            assert!(!snapshot.root.to_string_lossy().contains(name));
            assert!(snapshot
                .root
                .starts_with(canonical_staging_parent(&root).unwrap()));
            assert_ne!(
                snapshot.input_path.parent(),
                Some(snapshot.work_dir.as_path())
            );
            cleanup_staged_snapshot(&snapshot).unwrap();
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_over_limit_hard_links_and_unsafe_source_types() {
        let root = root();
        let (scope, path) = source(&root, "candidate", &vec![1; 1024 * 1024 + 1]);
        assert!(capture_source_identity(&path, &scope, 1024 * 1024).is_err());

        let hard_link = scope.join("hard-link");
        fs::hard_link(&path, &hard_link).unwrap();
        assert!(capture_source_identity(&path, &scope, 2 * 1024 * 1024).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::{fs::symlink, net::UnixListener};
            let link = scope.join("link");
            symlink(&path, &link).unwrap();
            assert!(capture_source_identity(&link, &scope, 2 * 1024 * 1024).is_err());
            assert!(capture_source_identity(&scope, &root, 2 * 1024 * 1024).is_err());
            let fifo = scope.join("fifo");
            assert_eq!(
                unsafe {
                    libc::mkfifo(
                        CString::new(fifo.as_os_str().as_bytes()).unwrap().as_ptr(),
                        0o600,
                    )
                },
                0
            );
            assert!(capture_source_identity(&fifo, &scope, 2 * 1024 * 1024).is_err());
            let socket = scope.join("socket");
            if let Ok(_listener) = UnixListener::bind(&socket) {
                assert!(capture_source_identity(&socket, &scope, 2 * 1024 * 1024).is_err());
            }
            if Path::new("/dev/null").exists() {
                assert!(capture_source_identity(
                    Path::new("/dev/null"),
                    Path::new("/dev"),
                    2 * 1024 * 1024
                )
                .is_err());
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn deleted_source_after_descriptor_open_keeps_the_opened_bytes() {
        let root = root();
        let (scope, path) = source(&root, "candidate", b"authorized bytes");
        let mut source = open_regular_source(&path, &scope).unwrap();
        fs::remove_file(&path).unwrap();
        let destination = root.join("destination");
        let mut output = create_exclusive_staged_file(&destination).unwrap();
        let (_, bytes) = copy_open_source_to_staging(&mut source, &mut output, 1024).unwrap();
        output.sync_all().unwrap();
        assert_eq!(bytes, 16);
        assert_eq!(fs::read(destination).unwrap(), b"authorized bytes");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staged_snapshot_rejects_digest_or_size_changes_and_destination_collisions() {
        let root = root();
        let (scope, path) = source(&root, "candidate", b"first");
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        fs::write(&path, b"second").unwrap();
        assert!(prepare_staged_snapshot(
            &root,
            &path,
            &scope,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST
        )
        .is_err());
        fs::write(root.join("replacement"), b"first").unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(root.join("replacement"), &path).unwrap();
        assert!(prepare_staged_snapshot(
            &root,
            &path,
            &scope,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST
        )
        .is_err());
        let destination = root.join("destination");
        fs::write(&destination, b"existing").unwrap();
        assert!(create_exclusive_staged_file(&destination).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn staging_rejects_source_mutation_between_pre_and_post_copy_fstat() {
        let root = root();
        let (scope, path) = source(&root, "candidate", b"first");
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        let parent = canonical_staging_parent(&root).unwrap();
        let (_, staging_root) = create_staging_root(&parent).unwrap();
        let mut opened = open_regular_source(&path, &scope).unwrap();
        let before = source_fingerprint(&opened).unwrap();
        assert!(stage_open_source_with_before_copy(
            &mut opened,
            &staging_root,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST,
            before,
            || fs::write(&path, b"changed during staging").unwrap(),
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn staging_rejects_growth_beyond_the_profile_limit_during_copy() {
        let root = root();
        let (scope, path) = source(&root, "candidate", b"small");
        let identity = capture_source_identity(&path, &scope, 1024 * 1024).unwrap();
        let parent = canonical_staging_parent(&root).unwrap();
        let (_, staging_root) = create_staging_root(&parent).unwrap();
        let mut opened = open_regular_source(&path, &scope).unwrap();
        let before = source_fingerprint(&opened).unwrap();
        assert!(stage_open_source_with_before_copy(
            &mut opened,
            &staging_root,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST,
            before,
            || fs::write(&path, vec![1; 1024 * 1024 + 1]).unwrap(),
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staging_parent_and_input_permissions_are_private_where_supported() {
        let root = root();
        let (scope, path) = source(&root, "candidate", b"private");
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        let snapshot = prepare_staged_snapshot(
            &root,
            &path,
            &scope,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&snapshot.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&snapshot.input_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o400
            );
        }
        cleanup_staged_snapshot(&snapshot).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_source_handle_denies_write_delete_and_revalidates_identity() {
        let root = root();
        let (scope, path) = source(&root, "candidate.txt", b"authorized");
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        let opened = open_regular_source(&path, &scope).unwrap();
        assert!(fs::write(&path, b"changed").is_err());
        assert!(fs::remove_file(&path).is_err());
        drop(opened);
        fs::write(scope.join("replacement.txt"), b"authorized").unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(scope.join("replacement.txt"), &path).unwrap();
        assert!(prepare_staged_snapshot(
            &root,
            &path,
            &scope,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST,
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_rejects_directory_and_reparse_inputs() {
        let root = root();
        let scope = root.join("source");
        fs::create_dir(&scope).unwrap();
        assert!(capture_source_identity(&scope, &root, 1024).is_err());
        let target = scope.join("target.txt");
        fs::write(&target, b"target").unwrap();
        let link = scope.join("link.txt");
        if std::os::windows::fs::symlink_file(&target, &link).is_ok() {
            assert!(capture_source_identity(&link, &scope, 1024).is_err());
        }
        let _ = fs::remove_dir_all(root);
    }
}
