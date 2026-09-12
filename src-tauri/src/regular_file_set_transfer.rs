//! Host-private framing for a bounded regular-file-set over the existing
//! single-file encrypted Room transfer. It is not a managed artifact or Plan primitive.

use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    safe_file_identity::{self, RegularFileSetIdentity, MAX_REGULAR_FILE_SET_ENTRIES},
    storage::MAX_FILE_SIZE_BYTES,
};

const MAGIC: &[u8] = b"pastey-rfs-transfer-v1\0";
const VERSION: u16 = 1;
const REPRESENTATION_REGULAR_FILE_SET: u8 = 1;
const MAX_SELECTOR_BYTES: usize = 512;
const MAX_DIGEST_BYTES: usize = 128;
const STREAM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct PackageHeader {
    entry_count: usize,
    aggregate_digest: String,
    logical_byte_count: u64,
}

#[derive(Debug)]
struct EntryHeader {
    selector: String,
    byte_count: u64,
    content_digest: String,
}

/// Streams identity-matched source files into a deterministic package.
pub(crate) fn prepare_package(
    root: &Path,
    scope_root: &Path,
    expected: &RegularFileSetIdentity,
    temp_dir: &Path,
) -> AppResult<PathBuf> {
    let observed =
        safe_file_identity::capture_regular_file_set_identity(root, MAX_FILE_SIZE_BYTES)?;
    if &observed != expected {
        return invalid("Managed regular-file-set changed before Transfer packaging.");
    }
    let package_root = temp_dir
        .join("native-v2-transfer-packages")
        .join(Uuid::new_v4().to_string());
    fs::create_dir_all(&package_root)?;
    let package_path = package_root.join("input");
    let result = (|| {
        let mut package = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&package_path)?;
        write_header(&mut package, expected)?;
        for (selector, identity) in &expected.files {
            safe_file_identity::validate_managed_selector(selector)?;
            write_entry_header(
                &mut package,
                selector,
                identity.byte_count,
                &identity.digest,
            )?;
            safe_file_identity::stream_source_if_identity_matches(
                &root.join(selector),
                scope_root,
                identity,
                MAX_FILE_SIZE_BYTES,
                &mut package,
            )?;
        }
        package.sync_all()?;
        if package.metadata()?.len() > MAX_FILE_SIZE_BYTES {
            return invalid("Regular-file-set transport package exceeds the transfer file limit.");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&package_root);
    }
    result.map(|()| package_path)
}

pub(crate) fn cleanup_package(package_path: &Path) {
    if let Some(root) = package_path.parent().filter(|root| {
        root.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "native-v2-transfer-packages")
    }) {
        let _ = fs::remove_dir_all(root);
    }
}

/// Streams package payload bytes into a fresh no-follow private tree. Memory
/// use is one fixed buffer plus at most one bounded metadata record per entry.
pub(crate) fn materialize_package(
    package_path: &Path,
    temp_dir: &Path,
    expected_digest: &str,
    expected_bytes: u64,
) -> AppResult<PathBuf> {
    let package_metadata = fs::metadata(package_path)?;
    if !package_metadata.is_file() || package_metadata.len() > MAX_FILE_SIZE_BYTES {
        return invalid("Regular-file-set package is unavailable or exceeds its limit.");
    }
    let mut reader = BufReader::with_capacity(STREAM_BUFFER_BYTES, File::open(package_path)?);
    let header = read_header(&mut reader)?;
    if header.aggregate_digest != expected_digest || header.logical_byte_count != expected_bytes {
        return invalid("Regular-file-set package does not match the authored logical identity.");
    }
    let root = safe_file_identity::create_private_tree_root(
        temp_dir,
        "native-v2-file-sets",
        &Uuid::new_v4().to_string(),
    )?;
    let result = (|| {
        let mut entries = Vec::with_capacity(header.entry_count);
        let mut total = 0_u64;
        let mut previous: Option<String> = None;
        for _ in 0..header.entry_count {
            let entry = read_entry_header(&mut reader)?;
            safe_file_identity::validate_managed_selector(&entry.selector)?;
            if entry.selector == "."
                || previous
                    .as_ref()
                    .is_some_and(|value| value.as_str() >= entry.selector.as_str())
            {
                return invalid("Regular-file-set package selectors are duplicated or unordered.");
            }
            total = total.checked_add(entry.byte_count).ok_or_else(|| {
                AppError::InvalidInput("Regular-file-set package byte quota overflowed.".into())
            })?;
            if total > MAX_FILE_SIZE_BYTES || total > header.logical_byte_count {
                return invalid("Regular-file-set package logical bytes exceed their limit.");
            }
            let mut output =
                safe_file_identity::create_private_regular_file(&root, &entry.selector)?;
            stream_entry_payload(&mut reader, &mut output, &entry)?;
            output.sync_all()?;
            previous = Some(entry.selector.clone());
            entries.push(entry);
        }
        if total != header.logical_byte_count || reader.read(&mut [0_u8; 1])? != 0 {
            return invalid("Regular-file-set package contains trailing or inconsistent data.");
        }
        let digest = safe_file_identity::regular_file_set_digest_from_entries(entries.iter().map(
            |entry| {
                (
                    entry.selector.as_str(),
                    entry.content_digest.as_str(),
                    entry.byte_count,
                )
            },
        ))?;
        if digest != header.aggregate_digest {
            return invalid("Regular-file-set package aggregate digest is invalid.");
        }
        let observed =
            safe_file_identity::capture_regular_file_set_identity(&root, MAX_FILE_SIZE_BYTES)?;
        if observed.digest != expected_digest
            || observed.byte_count != expected_bytes
            || observed.files.len() != entries.len()
            || !observed
                .files
                .keys()
                .zip(entries.iter().map(|entry| &entry.selector))
                .all(|(a, b)| a == b)
        {
            return invalid("Materialized regular-file-set does not match its package identity.");
        }
        Ok(())
    })();
    if result.is_err() {
        cleanup_materialized_tree(&root);
    }
    result.map(|()| root)
}

pub(crate) fn cleanup_materialized_tree(tree: &Path) {
    if let Some(root) = tree.parent().filter(|root| {
        root.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "native-v2-file-sets")
    }) {
        let _ = fs::remove_dir_all(root);
    }
}

pub(crate) fn cleanup_received_package(package_path: &Path) {
    if let Some(root) = package_path.parent().filter(|root| {
        root.parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "native-v2-transfers")
    }) {
        let _ = fs::remove_dir_all(root);
    }
}

fn write_header(package: &mut File, identity: &RegularFileSetIdentity) -> AppResult<()> {
    package.write_all(MAGIC)?;
    package.write_all(&VERSION.to_be_bytes())?;
    package.write_all(&[REPRESENTATION_REGULAR_FILE_SET])?;
    package.write_all(
        &u32::try_from(identity.files.len())
            .map_err(|_| AppError::InvalidInput("Regular-file-set entry count overflowed.".into()))?
            .to_be_bytes(),
    )?;
    write_string(package, &identity.digest, MAX_DIGEST_BYTES)?;
    package.write_all(&identity.byte_count.to_be_bytes())?;
    Ok(())
}

fn write_entry_header(
    package: &mut File,
    selector: &str,
    byte_count: u64,
    digest: &str,
) -> AppResult<()> {
    write_string(package, selector, MAX_SELECTOR_BYTES)?;
    package.write_all(&byte_count.to_be_bytes())?;
    write_string(package, digest, MAX_DIGEST_BYTES)
}

fn write_string(writer: &mut impl Write, value: &str, maximum: usize) -> AppResult<()> {
    if value.len() > maximum {
        return invalid("Regular-file-set package text field exceeds its limit.");
    }
    writer.write_all(
        &u16::try_from(value.len())
            .map_err(|_| {
                AppError::InvalidInput("Regular-file-set package text field overflowed.".into())
            })?
            .to_be_bytes(),
    )?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

fn read_header(reader: &mut impl Read) -> AppResult<PackageHeader> {
    let mut magic = vec![0_u8; MAGIC.len()];
    reader.read_exact(&mut magic)?;
    if magic != MAGIC {
        return invalid("Regular-file-set package schema is unknown.");
    }
    if read_u16(reader)? != VERSION || read_u8(reader)? != REPRESENTATION_REGULAR_FILE_SET {
        return invalid("Regular-file-set package version or representation is unknown.");
    }
    let entry_count = usize::try_from(read_u32(reader)?).map_err(|_| {
        AppError::InvalidInput("Regular-file-set package entry count is invalid.".into())
    })?;
    if entry_count == 0 || entry_count > MAX_REGULAR_FILE_SET_ENTRIES {
        return invalid("Regular-file-set package entry count is invalid.");
    }
    let aggregate_digest = read_string(reader, MAX_DIGEST_BYTES)?;
    if aggregate_digest.len() != 64
        || !aggregate_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return invalid("Regular-file-set package aggregate digest is invalid.");
    }
    Ok(PackageHeader {
        entry_count,
        aggregate_digest,
        logical_byte_count: read_u64(reader)?,
    })
}

fn read_entry_header(reader: &mut impl Read) -> AppResult<EntryHeader> {
    let selector = read_string(reader, MAX_SELECTOR_BYTES)?;
    let byte_count = read_u64(reader)?;
    let content_digest = read_string(reader, MAX_DIGEST_BYTES)?;
    if content_digest.len() != 64 || !content_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return invalid("Regular-file-set package entry digest is invalid.");
    }
    Ok(EntryHeader {
        selector,
        byte_count,
        content_digest,
    })
}

fn stream_entry_payload(
    reader: &mut impl Read,
    output: &mut File,
    entry: &EntryHeader,
) -> AppResult<()> {
    let mut remaining = entry.byte_count;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; STREAM_BUFFER_BYTES];
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64)).expect("bounded buffer");
        let read = reader.read(&mut buffer[..wanted])?;
        if read == 0 {
            return invalid("Regular-file-set package payload is truncated.");
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }
    if hasher.finalize().to_hex().as_str() != entry.content_digest {
        return invalid("Regular-file-set package entry payload digest is invalid.");
    }
    Ok(())
}

fn read_u8(reader: &mut impl Read) -> AppResult<u8> {
    let mut value = [0; 1];
    reader.read_exact(&mut value)?;
    Ok(value[0])
}
fn read_u16(reader: &mut impl Read) -> AppResult<u16> {
    let mut value = [0; 2];
    reader.read_exact(&mut value)?;
    Ok(u16::from_be_bytes(value))
}
fn read_u32(reader: &mut impl Read) -> AppResult<u32> {
    let mut value = [0; 4];
    reader.read_exact(&mut value)?;
    Ok(u32::from_be_bytes(value))
}
fn read_u64(reader: &mut impl Read) -> AppResult<u64> {
    let mut value = [0; 8];
    reader.read_exact(&mut value)?;
    Ok(u64::from_be_bytes(value))
}

fn read_string(reader: &mut impl Read, maximum: usize) -> AppResult<String> {
    let length = usize::from(read_u16(reader)?);
    if length > maximum {
        return invalid("Regular-file-set package text field exceeds its limit.");
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::InvalidInput("Regular-file-set package text is not UTF-8.".into()))
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    fn fixture(label: &str) -> (PathBuf, PathBuf, RegularFileSetIdentity) {
        let base = std::env::temp_dir().join(format!("{label}-{}", Uuid::new_v4()));
        let tree = base.join("tree");
        fs::create_dir_all(tree.join("src")).unwrap();
        fs::write(tree.join("Cargo.toml"), b"[package]\nname = 'fixture'\n").unwrap();
        fs::write(tree.join("src/main.rs"), b"fn main() {}\n").unwrap();
        let identity =
            safe_file_identity::capture_regular_file_set_identity(&tree, MAX_FILE_SIZE_BYTES)
                .unwrap();
        (base, tree, identity)
    }

    #[test]
    fn package_is_deterministic_and_streams_the_same_logical_tree() {
        let (base, tree, identity) = fixture("pastey-rfs-package");
        let first = prepare_package(&tree, &base, &identity, &base.join("temp-a")).unwrap();
        let second = prepare_package(&tree, &base, &identity, &base.join("temp-b")).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        let receive_root = base.join("received");
        fs::create_dir(&receive_root).unwrap();
        let received =
            materialize_package(&first, &receive_root, &identity.digest, identity.byte_count)
                .unwrap();
        let observed =
            safe_file_identity::capture_regular_file_set_identity(&received, MAX_FILE_SIZE_BYTES)
                .unwrap();
        assert_eq!(observed.digest, identity.digest);
        assert_eq!(observed.byte_count, identity.byte_count);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn zero_byte_file_sets_are_streamed_and_materialized() {
        let base = std::env::temp_dir().join(format!("pastey-rfs-zero-{}", Uuid::new_v4()));
        let single = base.join("single");
        let multiple = base.join("multiple");
        fs::create_dir_all(&single).unwrap();
        fs::create_dir_all(multiple.join("nested")).unwrap();
        fs::write(single.join("empty"), b"").unwrap();
        fs::write(multiple.join("a"), b"").unwrap();
        fs::write(multiple.join("nested/b"), b"").unwrap();
        for (tree, label) in [(&single, "single"), (&multiple, "multiple")] {
            let identity = safe_file_identity::capture_regular_file_set_identity(tree, 0).unwrap();
            let package =
                prepare_package(tree, &base, &identity, &base.join(format!("send-{label}")))
                    .unwrap();
            let receive_root = base.join(format!("receive-{label}"));
            fs::create_dir(&receive_root).unwrap();
            let received =
                materialize_package(&package, &receive_root, &identity.digest, 0).unwrap();
            assert_eq!(
                safe_file_identity::capture_regular_file_set_identity(&received, 0)
                    .unwrap()
                    .digest,
                identity.digest
            );
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn streamed_parser_rejects_truncation_digest_and_trailing_data() {
        let (base, tree, identity) = fixture("pastey-rfs-corrupt");
        let package = prepare_package(&tree, &base, &identity, &base.join("temp")).unwrap();
        let mut bytes = fs::read(&package).unwrap();
        bytes.pop();
        fs::write(&package, &bytes).unwrap();
        assert!(materialize_package(
            &package,
            &base.join("truncated"),
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let package = prepare_package(&tree, &base, &identity, &base.join("temp2")).unwrap();
        let mut bytes = fs::read(&package).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&package, &bytes).unwrap();
        let digest_root = base.join("digest");
        fs::create_dir(&digest_root).unwrap();
        assert!(materialize_package(
            &package,
            &digest_root,
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let digest_namespace = digest_root.join("native-v2-file-sets");
        assert!(
            !digest_namespace.exists() || fs::read_dir(digest_namespace).unwrap().next().is_none()
        );
        let package = prepare_package(&tree, &base, &identity, &base.join("temp3")).unwrap();
        let mut bytes = fs::read(&package).unwrap();
        bytes.extend_from_slice(b"unexpected");
        fs::write(&package, &bytes).unwrap();
        let trailing_root = base.join("trailing");
        fs::create_dir(&trailing_root).unwrap();
        assert!(materialize_package(
            &package,
            &trailing_root,
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let trailing_namespace = trailing_root.join("native-v2-file-sets");
        assert!(
            !trailing_namespace.exists()
                || fs::read_dir(trailing_namespace).unwrap().next().is_none()
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn streamed_parser_rejects_a_declared_payload_larger_than_remaining_bytes() {
        let (base, tree, identity) = fixture("pastey-rfs-declared-size");
        let package = prepare_package(&tree, &base, &identity, &base.join("temp")).unwrap();
        let mut package_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&package)
            .unwrap();
        let _header = read_header(&mut package_file).unwrap();
        let selector_length = u64::from(read_u16(&mut package_file).unwrap());
        package_file
            .seek(SeekFrom::Current(selector_length as i64))
            .unwrap();
        package_file.write_all(&u64::MAX.to_be_bytes()).unwrap();
        package_file.sync_all().unwrap();
        drop(package_file);
        assert!(materialize_package(
            &package,
            &base.join("received"),
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let namespace = base.join("received/native-v2-file-sets");
        assert!(
            !namespace.exists() || fs::read_dir(namespace).unwrap().next().is_none(),
            "failed materialization must not retain a private tree"
        );
        let _ = fs::remove_dir_all(base);
    }
}
