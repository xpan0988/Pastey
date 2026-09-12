//! Host-private framing for moving one managed regular-file-set through the
//! existing single-file encrypted Room transfer. This is deliberately not a
//! managed artifact, Plan primitive, or archive abstraction.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
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

#[derive(Debug)]
struct PackageEntry {
    selector: String,
    byte_count: u64,
    content_digest: String,
    bytes: Vec<u8>,
}

/// Writes a bounded, deterministic transport-private package. The returned
/// path is under Pastey's temporary root and must be removed by the caller.
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
            let bytes = safe_file_identity::read_source_if_identity_matches(
                &root.join(selector),
                scope_root,
                identity,
                MAX_FILE_SIZE_BYTES,
            )?;
            write_entry(
                &mut package,
                selector,
                identity.byte_count,
                &identity.digest,
                &bytes,
            )?;
        }
        package.sync_all()?;
        let package_bytes = package.metadata()?.len();
        if package_bytes > MAX_FILE_SIZE_BYTES {
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

/// Parses a received package into a fresh app-owned tree. It validates every
/// framing claim and then relies on the canonical scanner for final physical
/// identity before a caller can register any Transfer receipt.
pub(crate) fn materialize_package(
    package_path: &Path,
    temp_dir: &Path,
    expected_digest: &str,
    expected_bytes: u64,
) -> AppResult<PathBuf> {
    let entries = parse_package(package_path)?;
    let parsed_total = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.byte_count).ok_or_else(|| {
            AppError::InvalidInput("Regular-file-set package byte quota overflowed.".into())
        })
    })?;
    let declared_digest =
        safe_file_identity::regular_file_set_digest_from_entries(entries.iter().map(|entry| {
            (
                entry.selector.as_str(),
                entry.content_digest.as_str(),
                entry.byte_count,
            )
        }))?;
    if declared_digest != expected_digest || parsed_total != expected_bytes {
        return invalid("Regular-file-set package does not match the authored logical identity.");
    }

    let root = temp_dir
        .join("native-v2-file-sets")
        .join(Uuid::new_v4().to_string())
        .join("tree");
    let result = (|| {
        fs::create_dir_all(&root)?;
        for entry in &entries {
            materialize_entry(&root, entry)?;
        }
        let observed =
            safe_file_identity::capture_regular_file_set_identity(&root, MAX_FILE_SIZE_BYTES)?;
        if observed.digest != expected_digest
            || observed.byte_count != expected_bytes
            || observed.files.len() != entries.len()
            || observed.files.keys().collect::<BTreeSet<_>>()
                != entries
                    .iter()
                    .map(|entry| &entry.selector)
                    .collect::<BTreeSet<_>>()
        {
            return invalid("Materialized regular-file-set does not match its package identity.");
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(root.parent().expect("tree has a parent"));
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

fn write_entry(
    package: &mut File,
    selector: &str,
    byte_count: u64,
    digest: &str,
    bytes: &[u8],
) -> AppResult<()> {
    if bytes.len() as u64 != byte_count || blake3::hash(bytes).to_hex().as_str() != digest {
        return invalid("Regular-file-set source changed while Transfer packaging.");
    }
    write_string(package, selector, MAX_SELECTOR_BYTES)?;
    package.write_all(&byte_count.to_be_bytes())?;
    write_string(package, digest, MAX_DIGEST_BYTES)?;
    package.write_all(bytes)?;
    Ok(())
}

fn write_string(package: &mut File, value: &str, maximum: usize) -> AppResult<()> {
    if value.len() > maximum {
        return invalid("Regular-file-set package text field exceeds its limit.");
    }
    package.write_all(
        &u16::try_from(value.len())
            .map_err(|_| {
                AppError::InvalidInput("Regular-file-set package text field overflowed.".into())
            })?
            .to_be_bytes(),
    )?;
    package.write_all(value.as_bytes())?;
    Ok(())
}

fn parse_package(path: &Path) -> AppResult<Vec<PackageEntry>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE_BYTES {
        return invalid("Regular-file-set package is unavailable or exceeds its limit.");
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).map_err(|_| {
        AppError::InvalidInput("Regular-file-set package is too large for this platform.".into())
    })?);
    File::open(path)?.read_to_end(&mut bytes)?;
    let mut reader = Reader::new(&bytes);
    if reader.take_exact(MAGIC.len())? != MAGIC {
        return invalid("Regular-file-set package schema is unknown.");
    }
    if reader.u16()? != VERSION || reader.u8()? != REPRESENTATION_REGULAR_FILE_SET {
        return invalid("Regular-file-set package version or representation is unknown.");
    }
    let count = usize::try_from(reader.u32()?).map_err(|_| {
        AppError::InvalidInput("Regular-file-set package entry count is invalid.".into())
    })?;
    if count == 0 || count > MAX_REGULAR_FILE_SET_ENTRIES {
        return invalid("Regular-file-set package entry count is invalid.");
    }
    let aggregate_digest = reader.string(MAX_DIGEST_BYTES)?;
    let total_bytes = reader.u64()?;
    let mut entries = Vec::with_capacity(count);
    let mut seen = BTreeSet::new();
    let mut observed_total = 0_u64;
    for _ in 0..count {
        let selector = reader.string(MAX_SELECTOR_BYTES)?;
        safe_file_identity::validate_managed_selector(&selector)?;
        if selector == "." || !seen.insert(selector.clone()) {
            return invalid("Regular-file-set package selectors are duplicated or invalid.");
        }
        let byte_count = reader.u64()?;
        let digest = reader.string(MAX_DIGEST_BYTES)?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return invalid("Regular-file-set package entry digest is invalid.");
        }
        observed_total = observed_total.checked_add(byte_count).ok_or_else(|| {
            AppError::InvalidInput("Regular-file-set package byte quota overflowed.".into())
        })?;
        if observed_total > MAX_FILE_SIZE_BYTES {
            return invalid("Regular-file-set package logical bytes exceed their limit.");
        }
        let payload = reader
            .take_exact(usize::try_from(byte_count).map_err(|_| {
                AppError::InvalidInput(
                    "Regular-file-set package entry is too large for this platform.".into(),
                )
            })?)?
            .to_vec();
        if blake3::hash(&payload).to_hex().as_str() != digest {
            return invalid("Regular-file-set package entry payload digest is invalid.");
        }
        entries.push(PackageEntry {
            selector,
            byte_count,
            content_digest: digest,
            bytes: payload,
        });
    }
    if !reader.is_empty() || observed_total != total_bytes {
        return invalid("Regular-file-set package contains trailing or inconsistent data.");
    }
    let observed_digest =
        safe_file_identity::regular_file_set_digest_from_entries(entries.iter().map(|entry| {
            (
                entry.selector.as_str(),
                entry.content_digest.as_str(),
                entry.byte_count,
            )
        }))?;
    if aggregate_digest != observed_digest {
        return invalid("Regular-file-set package aggregate digest is invalid.");
    }
    Ok(entries)
}

fn materialize_entry(root: &Path, entry: &PackageEntry) -> AppResult<()> {
    safe_file_identity::validate_managed_selector(&entry.selector)?;
    let mut destination = root.to_path_buf();
    for component in Path::new(&entry.selector).components() {
        let Component::Normal(component) = component else {
            return invalid("Regular-file-set package selector is invalid.");
        };
        destination.push(component);
    }
    let parent = destination.parent().ok_or_else(|| {
        AppError::InvalidInput("Regular-file-set materialization parent is unavailable.".into())
    })?;
    fs::create_dir_all(parent)?;
    let components = Path::new(&entry.selector).components().collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for component in &components[..components.len().saturating_sub(1)] {
        if let Component::Normal(name) = component {
            current.push(name);
            let metadata = fs::symlink_metadata(&current)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return invalid(
                    "Regular-file-set materialization encountered an unsafe directory.",
                );
            }
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    file.write_all(&entry.bytes)?;
    file.sync_all()?;
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take_exact(&mut self, count: usize) -> AppResult<&'a [u8]> {
        let end = self.position.checked_add(count).ok_or_else(|| {
            AppError::InvalidInput("Regular-file-set package framing overflowed.".into())
        })?;
        let result = self.bytes.get(self.position..end).ok_or_else(|| {
            AppError::InvalidInput("Regular-file-set package is truncated.".into())
        })?;
        self.position = end;
        Ok(result)
    }

    fn u8(&mut self) -> AppResult<u8> {
        Ok(self.take_exact(1)?[0])
    }

    fn u16(&mut self) -> AppResult<u16> {
        let bytes: [u8; 2] = self.take_exact(2)?.try_into().expect("exact length");
        Ok(u16::from_be_bytes(bytes))
    }

    fn u32(&mut self) -> AppResult<u32> {
        let bytes: [u8; 4] = self.take_exact(4)?.try_into().expect("exact length");
        Ok(u32::from_be_bytes(bytes))
    }

    fn u64(&mut self) -> AppResult<u64> {
        let bytes: [u8; 8] = self.take_exact(8)?.try_into().expect("exact length");
        Ok(u64::from_be_bytes(bytes))
    }

    fn string(&mut self, maximum: usize) -> AppResult<String> {
        let length = usize::from(self.u16()?);
        if length > maximum {
            return invalid("Regular-file-set package text field exceeds its limit.");
        }
        String::from_utf8(self.take_exact(length)?.to_vec()).map_err(|_| {
            AppError::InvalidInput("Regular-file-set package text is not UTF-8.".into())
        })
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn package_is_deterministic_and_materializes_the_same_logical_tree() {
        let (base, tree, identity) = fixture("pastey-rfs-package");
        let first = prepare_package(&tree, &base, &identity, &base.join("temp-a")).unwrap();
        let second = prepare_package(&tree, &base, &identity, &base.join("temp-b")).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        let materialized = materialize_package(
            &first,
            &base.join("received"),
            &identity.digest,
            identity.byte_count,
        )
        .unwrap();
        let received = safe_file_identity::capture_regular_file_set_identity(
            &materialized,
            MAX_FILE_SIZE_BYTES,
        )
        .unwrap();
        assert_eq!(received.digest, identity.digest);
        assert_eq!(received.byte_count, identity.byte_count);
        assert_eq!(
            received.files.keys().collect::<Vec<_>>(),
            identity.files.keys().collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn package_rejects_digest_mutation_duplicate_selector_and_trailing_data() {
        let (base, tree, identity) = fixture("pastey-rfs-corrupt");
        let package = prepare_package(&tree, &base, &identity, &base.join("temp")).unwrap();
        let mut bytes = fs::read(&package).unwrap();
        let digest_offset = MAGIC.len() + 2 + 1 + 4 + 2;
        bytes[digest_offset] ^= 1;
        fs::write(&package, &bytes).unwrap();
        assert!(materialize_package(
            &package,
            &base.join("received"),
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let package = prepare_package(&tree, &base, &identity, &base.join("temp2")).unwrap();
        let mut bytes = fs::read(&package).unwrap();
        bytes.extend_from_slice(b"unexpected");
        fs::write(&package, &bytes).unwrap();
        assert!(materialize_package(
            &package,
            &base.join("received2"),
            &identity.digest,
            identity.byte_count
        )
        .is_err());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn package_rejects_duplicate_or_unsafe_selectors_truncation_and_excess_entries() {
        let (base, tree, identity) = fixture("pastey-rfs-framing");
        let cargo = fs::read(tree.join("Cargo.toml")).unwrap();
        let cargo_digest = blake3::hash(&cargo).to_hex().to_string();
        let duplicate = base.join("duplicate");
        let mut file = File::create(&duplicate).unwrap();
        write_header(&mut file, &identity).unwrap();
        write_entry(
            &mut file,
            "Cargo.toml",
            cargo.len() as u64,
            &cargo_digest,
            &cargo,
        )
        .unwrap();
        write_entry(
            &mut file,
            "Cargo.toml",
            cargo.len() as u64,
            &cargo_digest,
            &cargo,
        )
        .unwrap();
        assert!(parse_package(&duplicate).is_err());

        let unsafe_selector = base.join("unsafe-selector");
        let mut file = File::create(&unsafe_selector).unwrap();
        write_header(&mut file, &identity).unwrap();
        write_entry(
            &mut file,
            "../escape",
            cargo.len() as u64,
            &cargo_digest,
            &cargo,
        )
        .unwrap();
        assert!(parse_package(&unsafe_selector).is_err());

        let truncated = prepare_package(&tree, &base, &identity, &base.join("temp3")).unwrap();
        let mut bytes = fs::read(&truncated).unwrap();
        bytes.pop();
        fs::write(&truncated, bytes).unwrap();
        assert!(parse_package(&truncated).is_err());

        let excessive = base.join("excessive");
        let mut file = File::create(&excessive).unwrap();
        file.write_all(MAGIC).unwrap();
        file.write_all(&VERSION.to_be_bytes()).unwrap();
        file.write_all(&[REPRESENTATION_REGULAR_FILE_SET]).unwrap();
        file.write_all(&((MAX_REGULAR_FILE_SET_ENTRIES as u32) + 1).to_be_bytes())
            .unwrap();
        assert!(parse_package(&excessive).is_err());
        let _ = fs::remove_dir_all(base);
    }
}
