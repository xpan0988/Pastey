use std::{
    collections::BTreeMap,
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

use crate::error::{AppError, AppResult};

const COPY_BUFFER_BYTES: usize = 64 * 1024;
/// Bounds traversal, identity state, manifests, and seal evidence for one
/// managed regular-file-set. This is intentionally a fixed representation
/// limit rather than a product policy surface.
pub(crate) const MAX_REGULAR_FILE_SET_ENTRIES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceIdentity {
    pub(crate) digest: String,
    pub(crate) byte_count: u64,
    pub(crate) fingerprint: SourceFingerprint,
}

/// Host-private identity for a bounded tree of regular files.  This is a
/// representation helper, not a managed-object or authority primitive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RegularFileSetIdentity {
    pub(crate) files: BTreeMap<String, SourceIdentity>,
    pub(crate) digest: String,
    pub(crate) byte_count: u64,
}

/// The one selector contract shared by managed resources and managed-object
/// file-set acquisition.  It deliberately admits only portable relative file
/// selectors (and the resource-root selector `.`).
pub(crate) fn validate_managed_selector(selector: &str) -> AppResult<()> {
    if selector.is_empty()
        || selector.len() > 512
        || selector.contains('\0')
        || selector.contains('\\')
        || selector.to_ascii_lowercase().starts_with("file:")
    {
        return Err(AppError::InvalidInput(
            "Managed resource selector is invalid.".into(),
        ));
    }
    if selector == "." {
        return Ok(());
    }
    let path = Path::new(selector);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::InvalidInput(
            "Managed resource selector must be normalized and relative.".into(),
        ));
    }
    Ok(())
}

pub(crate) fn capture_regular_file_set_identity(
    root: &Path,
    maximum_bytes: u64,
) -> AppResult<RegularFileSetIdentity> {
    let (files, total) = scan_regular_file_set(root, maximum_bytes)?;
    if files.is_empty() {
        return Err(AppError::InvalidInput(
            "Managed regular-file-set must contain a regular file.".into(),
        ));
    }
    let digest = regular_file_set_digest(&files)?;
    Ok(RegularFileSetIdentity {
        files,
        digest,
        byte_count: total,
    })
}

/// Canonically enumerate a Host-local tree of regular files. Callers that
/// model a managed artifact must reject the empty result; private execution
/// overlays may use an empty result while retaining exactly the same physical
/// safety, selector, byte, and entry-count rules.
pub(crate) fn scan_regular_file_set(
    root: &Path,
    maximum_bytes: u64,
) -> AppResult<(BTreeMap<String, SourceIdentity>, u64)> {
    let root_metadata = fs::symlink_metadata(root)?;
    if unsafe_regular_file_set_directory(&root_metadata) {
        return Err(AppError::InvalidInput(
            "Managed regular-file-set root must be a safe directory.".into(),
        ));
    }
    let canonical_root = root.canonicalize()?;
    let mut pending = vec![canonical_root.clone()];
    let mut files = BTreeMap::new();
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let metadata = fs::symlink_metadata(&directory)?;
        if unsafe_regular_file_set_directory(&metadata) || !directory.starts_with(&canonical_root) {
            return Err(AppError::InvalidInput(
                "Managed regular-file-set directory is unsafe.".into(),
            ));
        }
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || windows_reparse_point(&metadata) {
                return Err(AppError::InvalidInput(
                    "Managed regular-file-set contains a symlink or reparse point.".into(),
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(AppError::InvalidInput(
                    "Managed regular-file-set contains a special file.".into(),
                ));
            }
            if files.len() >= MAX_REGULAR_FILE_SET_ENTRIES {
                return Err(AppError::InvalidInput(
                    "Managed regular-file-set exceeds its entry-count limit.".into(),
                ));
            }
            let relative = path.strip_prefix(&canonical_root).map_err(|_| {
                AppError::InvalidInput("Managed regular-file-set escaped its root.".into())
            })?;
            let selector = relative
                .components()
                .map(|component| {
                    component.as_os_str().to_str().ok_or_else(|| {
                        AppError::InvalidInput(
                            "Managed regular-file-set selector is not portable UTF-8.".into(),
                        )
                    })
                })
                .collect::<AppResult<Vec<_>>>()?
                .join("/");
            validate_managed_selector(&selector)?;
            let remaining = maximum_bytes.saturating_sub(total);
            let identity = capture_source_identity(&path, &canonical_root, remaining)?;
            total = total.checked_add(identity.byte_count).ok_or_else(|| {
                AppError::InvalidInput("Managed regular-file-set quota overflowed.".into())
            })?;
            if total > maximum_bytes || files.insert(selector, identity).is_some() {
                return Err(AppError::InvalidInput(
                    "Managed regular-file-set exceeds its quota or aliases a selector.".into(),
                ));
            }
        }
    }
    Ok((files, total))
}

fn unsafe_regular_file_set_directory(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || !metadata.is_dir() || windows_reparse_point(metadata)
}

fn windows_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

pub(crate) fn regular_file_set_digest(
    files: &BTreeMap<String, SourceIdentity>,
) -> AppResult<String> {
    let entries = files
        .iter()
        .map(|(selector, identity)| {
            (
                selector.as_str(),
                identity.digest.as_str(),
                identity.byte_count,
            )
        })
        .collect::<Vec<_>>();
    regular_file_set_digest_from_entries(entries)
}

/// Computes the stable logical digest used by regular-file-set identities
/// without exposing physical file fingerprints. Transport framing uses this to
/// validate its claimed logical manifest before it is materialized.
pub(crate) fn regular_file_set_digest_from_entries<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str, u64)>,
) -> AppResult<String> {
    let entries = entries.into_iter().collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(AppError::InvalidInput(
            "Managed regular-file-set must contain a regular file.".into(),
        ));
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"pastey-managed-regular-file-set-v1\0");
    let mut previous = None;
    for (selector, digest, byte_count) in entries {
        validate_managed_selector(selector)?;
        if previous.is_some_and(|value: &str| value >= selector) {
            return Err(AppError::InvalidInput(
                "Managed regular-file-set selectors must be canonical and unique.".into(),
            ));
        }
        previous = Some(selector);
        hasher.update(&(selector.len() as u64).to_be_bytes());
        hasher.update(selector.as_bytes());
        hasher.update(&(digest.len() as u64).to_be_bytes());
        hasher.update(digest.as_bytes());
        hasher.update(&byte_count.to_be_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
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

/// Reads an exact previously captured regular file through the same
/// descriptor-relative, no-follow path used for identity capture. Callers get
/// bytes only when the identity is unchanged before, during, and after the
/// read; the physical path never needs to leave the Host-private resolver.
pub(crate) fn read_source_if_identity_matches(
    source_path: &Path,
    scope_root: &Path,
    expected: &SourceIdentity,
    maximum_bytes: u64,
) -> AppResult<Vec<u8>> {
    let mut source = open_regular_source(source_path, scope_root)?;
    let before = source_fingerprint(&source)?;
    if before != expected.fingerprint || expected.byte_count > maximum_bytes {
        return Err(AppError::InvalidInput(
            "Safe file source identity is stale or mismatched.".into(),
        ));
    }
    source.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let next_len = bytes
            .len()
            .checked_add(read)
            .ok_or_else(|| AppError::InvalidInput("Safe file source is too large.".into()))?;
        if next_len as u64 > maximum_bytes {
            return Err(AppError::InvalidInput(
                "Safe file source exceeds the identity limit.".into(),
            ));
        }
        hasher.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
    }
    let after = source_fingerprint(&source)?;
    let digest = hasher.finalize().to_hex().to_string();
    if before != after
        || after != expected.fingerprint
        || bytes.len() as u64 != expected.byte_count
        || digest != expected.digest
    {
        return Err(AppError::InvalidInput(
            "Safe file source changed while it was read.".into(),
        ));
    }
    Ok(bytes)
}

/// Streams one exact, already-captured source through a caller-owned writer.
/// It uses the same no-follow descriptor opening and validates the identity
/// before and after the complete read, without retaining source contents.
pub(crate) fn stream_source_if_identity_matches(
    source_path: &Path,
    scope_root: &Path,
    expected: &SourceIdentity,
    maximum_bytes: u64,
    destination: &mut impl Write,
) -> AppResult<u64> {
    let mut source = open_regular_source(source_path, scope_root)?;
    let before = source_fingerprint(&source)?;
    if before != expected.fingerprint || expected.byte_count > maximum_bytes {
        return Err(AppError::InvalidInput(
            "Safe file source identity is stale or mismatched.".into(),
        ));
    }
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
        destination.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    let after = source_fingerprint(&source)?;
    if before != after
        || after != expected.fingerprint
        || bytes != expected.byte_count
        || hasher.finalize().to_hex().as_str() != expected.digest
    {
        return Err(AppError::InvalidInput(
            "Safe file source changed while it was streamed.".into(),
        ));
    }
    Ok(bytes)
}

/// Creates a fresh app-private tree through descriptor-relative/no-follow
/// traversal. The returned path is still rescanned before it can become a
/// managed binding.
pub(crate) fn create_private_tree_root(
    temp_dir: &Path,
    namespace: &str,
    unique: &str,
) -> AppResult<PathBuf> {
    validate_private_component(namespace)?;
    validate_private_component(unique)?;
    #[cfg(unix)]
    {
        let root = open_private_directory(temp_dir)?;
        let namespace_dir = open_or_create_private_directory(&root, namespace)?;
        let unique_dir = open_or_create_private_directory(&namespace_dir, unique)?;
        let _tree = open_or_create_private_directory(&unique_dir, "tree")?;
        return Ok(temp_dir.join(namespace).join(unique).join("tree"));
    }
    #[cfg(windows)]
    {
        reject_windows_reparse_or_wrong_type(temp_dir, true)?;
        let canonical_temp = fs::canonicalize(temp_dir)
            .map_err(|_| AppError::InvalidInput("Private tree root is unavailable.".into()))?;
        reject_windows_reparse_or_wrong_type(&canonical_temp, true)?;
        let mut current = canonical_temp;
        for component in [namespace, unique, "tree"] {
            current.push(component);
            if !current.exists() {
                fs::create_dir(&current)?;
            }
            reject_windows_reparse_or_wrong_type(&current, true)?;
        }
        return Ok(current);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (temp_dir, namespace, unique);
        Err(AppError::InvalidInput(
            "Safe private tree creation is not supported on this platform.".into(),
        ))
    }
}

/// Creates exactly one regular file below a freshly created private tree. No
/// parent directory or leaf is followed while it is opened.
pub(crate) fn create_private_regular_file(root: &Path, selector: &str) -> AppResult<File> {
    validate_managed_selector(selector)?;
    if selector == "." {
        return Err(AppError::InvalidInput(
            "Private tree file selector must name a file.".into(),
        ));
    }
    let components = Path::new(selector).components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AppError::InvalidInput(
            "Private tree file selector is invalid.".into(),
        ));
    }
    #[cfg(unix)]
    {
        let mut directory = open_private_directory(root)?;
        for component in &components[..components.len() - 1] {
            let Component::Normal(component) = component else {
                unreachable!("validated normal selector component")
            };
            let component_text = component.to_str().ok_or_else(|| {
                AppError::InvalidInput("Private tree file selector is invalid.".into())
            })?;
            directory = open_or_create_private_directory(&directory, component_text)?;
        }
        let Component::Normal(component) = components.last().expect("nonempty selector") else {
            unreachable!("validated normal selector component")
        };
        let name = CString::new(component.as_bytes())
            .map_err(|_| AppError::InvalidInput("Private tree file selector is invalid.".into()))?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(AppError::InvalidInput(
                "Private tree output is unavailable or unsafe.".into(),
            ));
        }
        return Ok(unsafe { File::from_raw_fd(fd) });
    }
    #[cfg(windows)]
    {
        reject_windows_reparse_or_wrong_type(root, true)?;
        let canonical_root = fs::canonicalize(root)
            .map_err(|_| AppError::InvalidInput("Private tree root is unavailable.".into()))?;
        reject_windows_reparse_or_wrong_type(&canonical_root, true)?;
        let mut destination = canonical_root.clone();
        for component in &components {
            let Component::Normal(component) = component else {
                unreachable!("validated normal selector component")
            };
            destination.push(component);
        }
        let mut current = canonical_root.clone();
        for component in &components[..components.len() - 1] {
            let Component::Normal(component) = component else {
                unreachable!("validated normal selector component")
            };
            current.push(component);
            if !current.exists() {
                fs::create_dir(&current)?;
            }
            reject_windows_reparse_or_wrong_type(&current, true)?;
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN);
        let file = options.open(&destination).map_err(|_| {
            AppError::InvalidInput("Private tree output is unavailable or unsafe.".into())
        })?;
        let opened = normalized_windows_path(&final_windows_handle_path(&file)?);
        let root = normalized_windows_path(&canonical_root);
        if !windows_path_is_within(&opened, &root) {
            return Err(AppError::InvalidInput(
                "Private tree output escaped its root.".into(),
            ));
        }
        return Ok(file);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Err(AppError::InvalidInput(
            "Safe private file creation is not supported on this platform.".into(),
        ))
    }
}

fn validate_private_component(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
    {
        return Err(AppError::InvalidInput(
            "Private tree component is invalid.".into(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_private_directory(path: &Path) -> AppResult<File> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| AppError::InvalidInput("Private tree directory is unavailable.".into()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Private tree directory is unsafe.".into(),
        ));
    }
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| {
            AppError::InvalidInput("Private tree directory is unavailable or unsafe.".into())
        })
}

#[cfg(unix)]
fn open_or_create_private_directory(parent: &File, name: &str) -> AppResult<File> {
    validate_private_component(name)?;
    let name = CString::new(name.as_bytes())
        .map_err(|_| AppError::InvalidInput("Private tree component is invalid.".into()))?;
    let mut fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
        )
    };
    if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
        {
            return Err(AppError::InvalidInput(
                "Private tree directory could not be created.".into(),
            ));
        }
        fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
    }
    if fd < 0 {
        return Err(AppError::InvalidInput(
            "Private tree directory is unavailable or unsafe.".into(),
        ));
    }
    Ok(unsafe { File::from_raw_fd(fd) })
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
    use std::io::{self, Write};

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

    #[test]
    fn regular_file_set_scan_is_deterministic_and_entry_bounded() {
        let (scope, _) = fixture();
        fs::create_dir_all(scope.join("nested")).unwrap();
        fs::write(scope.join("zeta.txt"), b"z").unwrap();
        fs::write(scope.join("nested/alpha.txt"), b"a").unwrap();
        let first = capture_regular_file_set_identity(&scope, 1024).unwrap();
        let second = capture_regular_file_set_identity(&scope, 1024).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(
            first.files.keys().cloned().collect::<Vec<_>>(),
            vec!["candidate.txt", "nested/alpha.txt", "zeta.txt"]
        );

        let bounded = scope.join("bounded");
        fs::create_dir(&bounded).unwrap();
        for index in 0..MAX_REGULAR_FILE_SET_ENTRIES {
            fs::write(bounded.join(format!("{index:04}.txt")), b"").unwrap();
        }
        assert_eq!(
            capture_regular_file_set_identity(&bounded, 0)
                .unwrap()
                .files
                .len(),
            MAX_REGULAR_FILE_SET_ENTRIES
        );
        fs::write(bounded.join("overflow.txt"), b"").unwrap();
        assert!(capture_regular_file_set_identity(&bounded, 0).is_err());

        let root = scope.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn managed_selectors_are_portable_and_confined() {
        for selector in [
            "",
            "/absolute",
            "../escape",
            "nested\\escape",
            "file:escape",
        ] {
            assert!(validate_managed_selector(selector).is_err(), "{selector}");
        }
        assert!(validate_managed_selector("nested/file.txt").is_ok());
    }

    #[test]
    fn source_streaming_uses_fixed_size_writes_without_payload_buffering() {
        struct BoundedWriter {
            bytes: usize,
            writes: usize,
        }

        impl Write for BoundedWriter {
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                assert!(buffer.len() <= COPY_BUFFER_BYTES);
                self.bytes += buffer.len();
                self.writes += 1;
                Ok(buffer.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let (scope, path) = fixture();
        let contents = vec![0x5a; COPY_BUFFER_BYTES * 3 + 17];
        fs::write(&path, &contents).unwrap();
        let identity = capture_source_identity(&path, &scope, contents.len() as u64).unwrap();
        let mut writer = BoundedWriter {
            bytes: 0,
            writes: 0,
        };
        assert_eq!(
            stream_source_if_identity_matches(
                &path,
                &scope,
                &identity,
                contents.len() as u64,
                &mut writer,
            )
            .unwrap(),
            contents.len() as u64
        );
        assert_eq!(writer.bytes, contents.len());
        assert!(writer.writes >= 4);
        let root = scope.parent().unwrap().to_path_buf();
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unix_private_materialization_rejects_symlink_parent_without_escape() {
        use std::os::unix::fs::symlink;

        let base =
            std::env::temp_dir().join(format!("pastey-private-tree-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let tree = create_private_tree_root(&base, "private", "tree-id").unwrap();
        let outside = base.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, tree.join("link")).unwrap();
        assert!(create_private_regular_file(&tree, "link/escaped.txt").is_err());
        assert!(!outside.join("escaped.txt").exists());
        let _ = fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn unix_descriptor_open_rejects_symlinks_and_non_regular_files() {
        use std::os::unix::fs::symlink;
        let (scope, path) = fixture();
        let link = scope.join("link.txt");
        symlink(&path, &link).unwrap();
        assert!(capture_source_identity(&link, &scope, 1024).is_err());
        assert!(capture_regular_file_set_identity(&scope, 1024).is_err());
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
            assert!(capture_regular_file_set_identity(&scope, 1024).is_err());
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

    #[cfg(windows)]
    #[test]
    fn windows_private_materialization_rejects_reparse_parent() {
        let base =
            std::env::temp_dir().join(format!("pastey-private-tree-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&base).unwrap();
        let tree = create_private_tree_root(&base, "private", "tree-id").unwrap();
        let outside = base.join("outside");
        fs::create_dir(&outside).unwrap();
        let link = tree.join("link");
        if std::os::windows::fs::symlink_dir(&outside, &link).is_ok() {
            assert!(create_private_regular_file(&tree, "link/escaped.txt").is_err());
            assert!(!outside.join("escaped.txt").exists());
        }
        let _ = fs::remove_dir_all(base);
    }
}
