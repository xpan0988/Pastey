use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path},
};

#[cfg(test)]
use std::path::PathBuf;

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

use crate::error::{AppError, AppResult};

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
            "Safe file candidate changed while its identity was captured.".into(),
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
fn open_regular_source(source_path: &Path, scope_root: &Path) -> AppResult<File> {
    #[cfg(unix)]
    {
        let canonical_root = fs::canonicalize(scope_root)
            .map_err(|_| AppError::InvalidInput("Safe file source scope is unavailable.".into()))?;
        let relative = source_path.strip_prefix(scope_root).map_err(|_| {
            AppError::InvalidInput("Safe file source escaped its approved scope.".into())
        })?;
        let components: Vec<_> = relative.components().collect();
        if components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput(
                "Safe file source path is invalid.".into(),
            ));
        }
        let root_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY)
            .open(&canonical_root)?;
        let mut directory = root_file;
        for component in &components[..components.len() - 1] {
            let name = CString::new(component.as_os_str().as_bytes())
                .map_err(|_| AppError::InvalidInput("Safe file source path is invalid.".into()))?;
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
                )
            };
            if fd < 0 {
                return Err(AppError::InvalidInput(
                    "Safe file source directory changed.".into(),
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
        .map_err(|_| AppError::InvalidInput("Safe file source path is invalid.".into()))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(AppError::InvalidInput(
                "Safe file source is unavailable or unsafe.".into(),
            ));
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let fingerprint = source_fingerprint(&file)?;
        if fingerprint.link_count != 1 {
            return Err(AppError::InvalidInput(
                "Safe file source has unsupported hard links.".into(),
            ));
        }
        return Ok(file);
    }
    #[cfg(windows)]
    {
        let canonical_root = fs::canonicalize(scope_root)
            .map_err(|_| AppError::InvalidInput("Safe file source scope is unavailable.".into()))?;
        reject_windows_reparse_or_wrong_type(&canonical_root, true)?;
        let relative = source_path.strip_prefix(scope_root).map_err(|_| {
            AppError::InvalidInput("Safe file source escaped its approved scope.".into())
        })?;
        let components = relative.components().collect::<Vec<_>>();
        if components.is_empty()
            || components
                .iter()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput(
                "Safe file source path is invalid.".into(),
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
            AppError::InvalidInput("Safe file source is unavailable or unsafe.".into())
        })?;
        let fingerprint = source_fingerprint(&file)?;
        if fingerprint.link_count != 1 {
            return Err(AppError::InvalidInput(
                "Safe file source has unsupported hard links.".into(),
            ));
        }
        let opened_path = final_windows_handle_path(&file)?;
        let expected_path = fs::canonicalize(source_path).map_err(|_| {
            AppError::InvalidInput("Safe file source changed while opening.".into())
        })?;
        let opened = normalized_windows_path(&opened_path);
        let expected = normalized_windows_path(&expected_path);
        let root = normalized_windows_path(&canonical_root);
        if opened != expected || !windows_path_is_within(&opened, &root) {
            return Err(AppError::InvalidInput(
                "Safe file source escaped its approved scope.".into(),
            ));
        }
        return Ok(file);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (source_path, scope_root);
        Err(AppError::InvalidInput(
            "Safe file identity capture is not supported on this platform.".into(),
        ))
    }
}

fn source_fingerprint(file: &File) -> AppResult<SourceFingerprint> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(AppError::InvalidInput(
            "Safe file source must be a regular file.".into(),
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
                "Safe file source has an unsupported file type.".into(),
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
                "Safe file source has an unsupported file type.".into(),
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
        AppError::InvalidInput("Safe file source path is unavailable or unsafe.".into())
    })?;
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != expect_directory
    {
        return Err(AppError::InvalidInput(
            "Safe file source path contains an unsafe reparse point or file type.".into(),
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
            "Safe file source path changed or contains a reparse point.".into(),
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
        |_| AppError::InvalidInput("Safe file source handle path is invalid.".into()),
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
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| AppError::InvalidInput("Safe file source is too large.".into()))?;
        if bytes > maximum_bytes {
            return Err(AppError::InvalidInput(
                "Safe file source exceeds the identity limit.".into(),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("pastey-safe-file-{}", uuid::Uuid::new_v4()));
        let scope = root.join("scope");
        fs::create_dir_all(&scope).unwrap();
        let path = scope.join("candidate.txt");
        fs::write(&path, b"approved bytes").unwrap();
        (scope, path)
    }

    #[test]
    fn identity_is_bounded_and_rejects_hard_links_and_unsafe_types() {
        let (scope, path) = fixture();
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        assert_eq!(identity.byte_count, 14);
        assert_eq!(
            identity.digest,
            blake3::hash(b"approved bytes").to_hex().to_string()
        );
        assert!(capture_source_identity(&path, &scope, 4).is_err());
        let hard_link = scope.join("hard-link");
        fs::hard_link(&path, &hard_link).unwrap();
        assert!(capture_source_identity(&path, &scope, 1024).is_err());
        let root = scope.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unix_descriptor_open_rejects_symlinks_and_non_regular_files() {
        use std::os::unix::fs::symlink;
        let (scope, path) = fixture();
        let link = scope.join("link.txt");
        symlink(&path, &link).unwrap();
        assert!(capture_source_identity(&link, &scope, 1024).is_err());
        assert!(capture_source_identity(&scope, scope.parent().unwrap(), 1024).is_err());
        let root = scope.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_and_path_substitution_are_rejected() {
        let (scope, path) = fixture();
        let identity = capture_source_identity(&path, &scope, 1024).unwrap();
        let link = scope.join("link.txt");
        if std::os::windows::fs::symlink_file(&path, &link).is_ok() {
            assert!(capture_source_identity(&link, &scope, 1024).is_err());
        }
        let replacement = scope.join("replacement.txt");
        fs::write(&replacement, b"approved bytes").unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(&replacement, &path).unwrap();
        let changed = capture_source_identity(&path, &scope, 1024).unwrap();
        assert_ne!(identity.fingerprint, changed.fingerprint);
        let root = scope.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(root);
    }
}
