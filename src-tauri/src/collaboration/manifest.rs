//! File state without Git: a manifest of a member's folder at the start and
//! end of each of its turns (M3 of `docs/research/agent-collaboration-links.md`,
//! "File state without Git").
//!
//! A manifest records each file's relative path, size and modification time,
//! nothing else: no contents and no hashes. The difference between two
//! manifests gives the paths added, changed and removed, including changes
//! made through shell commands, which native events do not name.
//!
//! Bounds: at most `MAX_FILES` files and `MAX_SCAN_TIME` per scan. Beyond
//! either, the scan is "unavailable", never a partial claim. Version-control
//! and dependency folders are skipped by name (the user chose this on
//! 2026-11-21), so ordinary projects fit. Symbolic links are recorded as
//! entries, never followed.

use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Files one scan may record.
pub(crate) const MAX_FILES: usize = 50_000;
/// Time one scan may take.
pub(crate) const MAX_SCAN_TIME: Duration = Duration::from_secs(2);
/// Changed paths an entry keeps per kind; more are only counted.
pub(crate) const MAX_LISTED_PATHS: usize = 20;

/// Folders skipped by name, at any depth: version control, dependencies,
/// build output and caches.
const SKIPPED_FOLDERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    "node_modules",
    "bower_components",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    ".gradle",
    ".dart_tool",
    "Pods",
    "DerivedData",
    ".build",
    ".cache",
];

/// One file as a scan saw it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FileStampV1 {
    pub(crate) size: u64,
    /// Modification time, nanoseconds since the epoch (0 when unknown).
    pub(crate) modified: i128,
}

/// A folder's files by relative path (`/`-separated).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ManifestV1 {
    pub(crate) files: BTreeMap<String, FileStampV1>,
}

/// Why a scan gave no manifest.
pub(crate) type UnavailableV1 = String;

pub(crate) type ScanV1 = Result<ManifestV1, UnavailableV1>;

/// Scans `root` within the bounds.
pub(crate) fn scan(root: &Path) -> ScanV1 {
    scan_within(root, MAX_FILES, MAX_SCAN_TIME)
}

pub(crate) fn scan_within(root: &Path, max_files: usize, max_time: Duration) -> ScanV1 {
    let started = Instant::now();
    if !root.is_dir() {
        return Err("the folder is unavailable".into());
    }
    let mut manifest = ManifestV1::default();
    let mut pending = vec![(root.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        let Ok(listing) = fs::read_dir(&folder) else {
            if prefix.is_empty() {
                return Err("the folder could not be read".into());
            }
            // An unreadable subfolder is left out, as Git would.
            continue;
        };
        for item in listing.flatten() {
            if started.elapsed() > max_time {
                return Err(format!(
                    "scanning it took longer than {} s",
                    max_time.as_secs_f32()
                ));
            }
            let name = item.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            // Never follows a symbolic link.
            let Ok(metadata) = fs::symlink_metadata(item.path()) else {
                continue;
            };
            if metadata.is_dir() {
                if !SKIPPED_FOLDERS.contains(&name.as_str()) {
                    pending.push((item.path(), relative));
                }
                continue;
            }
            if manifest.files.len() >= max_files {
                return Err(format!("it has more than {max_files} files"));
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos() as i128);
            manifest.files.insert(
                relative,
                FileStampV1 {
                    size: metadata.len(),
                    modified,
                },
            );
        }
    }
    Ok(manifest)
}

/// Paths that differ between two manifests, each list at most
/// `MAX_LISTED_PATHS`, with full counts.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileChangesV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) removed: Vec<String>,
    /// Every differing path, including those not listed.
    pub(crate) total: u32,
}

impl FileChangesV1 {
    pub(crate) fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The listed paths, added first.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &String> {
        self.added.iter().chain(&self.changed).chain(&self.removed)
    }
}

pub(crate) fn diff(before: &ManifestV1, after: &ManifestV1) -> FileChangesV1 {
    let mut changes = FileChangesV1::default();
    let note = |list: &mut Vec<String>, path: &str, total: &mut u32| {
        *total += 1;
        if list.len() < MAX_LISTED_PATHS {
            list.push(path.to_owned());
        }
    };
    let mut total = 0;
    for (path, stamp) in &after.files {
        match before.files.get(path) {
            None => note(&mut changes.added, path, &mut total),
            Some(earlier) if earlier != stamp => note(&mut changes.changed, path, &mut total),
            Some(_) => {}
        }
    }
    for path in before.files.keys() {
        if !after.files.contains_key(path) {
            note(&mut changes.removed, path, &mut total);
        }
    }
    changes.total = total;
    changes
}

/// What the Journal keeps of a turn's file state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum FileStateV1 {
    /// Pastey compared its folder's manifests from the turn's start and end.
    Observed {
        #[serde(flatten)]
        changes: FileChangesV1,
    },
    /// No claim either way.
    Unavailable { reason: String },
}

/// A turn's file state from its start and end scans.
pub(crate) fn turn_file_state(start: Option<&ScanV1>, end: &ScanV1) -> FileStateV1 {
    match (start, end) {
        (Some(Ok(before)), Ok(after)) => FileStateV1::Observed {
            changes: diff(before, after),
        },
        (None, _) => FileStateV1::Unavailable {
            reason: "Pastey did not scan the folder when the turn started".into(),
        },
        (Some(Err(reason)), _) | (_, Err(reason)) => FileStateV1::Unavailable {
            reason: reason.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn folder(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("pastey-manifest-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.py"), "a").unwrap();
        fs::write(root.join("README.md"), "r").unwrap();
        root
    }

    #[test]
    fn a_shell_made_change_appears_in_the_difference() {
        let root = folder("shell");
        fs::create_dir_all(root.join("node_modules/x")).unwrap();
        fs::write(root.join("node_modules/x/index.js"), "x").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "h").unwrap();
        let before = scan(&root).unwrap();
        assert_eq!(before.files.keys().collect::<Vec<_>>(), vec!["README.md", "src/a.py"]);
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf more >> src/a.py && echo new > src/b.py && rm README.md && echo y > node_modules/x/y.js")
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        let after = scan(&root).unwrap();
        let changes = diff(&before, &after);
        assert_eq!(changes.added, vec!["src/b.py"]);
        assert_eq!(changes.changed, vec!["src/a.py"]);
        assert_eq!(changes.removed, vec!["README.md"]);
        assert_eq!(changes.total, 3);
        assert!(diff(&after, &after).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_over_the_bounds_has_no_manifest() {
        let root = folder("bounds");
        for index in 0..5 {
            fs::write(root.join(format!("f{index}")), "x").unwrap();
        }
        let over = scan_within(&root, 4, MAX_SCAN_TIME).unwrap_err();
        assert!(over.contains("more than 4 files"), "{over}");
        let slow = scan_within(&root, MAX_FILES, Duration::ZERO).unwrap_err();
        assert!(slow.contains("longer than"), "{slow}");
        assert!(scan(&root.join("missing")).is_err());
        let state = turn_file_state(Some(&Ok(ManifestV1::default())), &Err(over));
        assert!(matches!(state, FileStateV1::Unavailable { .. }));
        assert!(matches!(turn_file_state(None, &scan(&root)), FileStateV1::Unavailable { .. }));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_is_recorded_never_followed() {
        let root = folder("link");
        let outside = folder("outside");
        std::os::unix::fs::symlink(&outside, root.join("elsewhere")).unwrap();
        let manifest = scan(&root).unwrap();
        assert!(manifest.files.contains_key("elsewhere"));
        assert!(!manifest.files.keys().any(|path| path.starts_with("elsewhere/")));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }

    #[test]
    fn listed_paths_are_bounded_and_counted() {
        let before = ManifestV1::default();
        let mut after = ManifestV1::default();
        for index in 0..30 {
            after.files.insert(format!("f{index:02}"), FileStampV1 { size: 1, modified: 1 });
        }
        let changes = diff(&before, &after);
        assert_eq!(changes.added.len(), MAX_LISTED_PATHS);
        assert_eq!(changes.total, 30);
    }
}
