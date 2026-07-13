use std::{
    fs,
    path::{Component, Path},
};

use crate::{
    error::{AppError, AppResult},
    logging,
};

use super::staging::{existing_staging_parent, is_valid_staging_id, StagedSnapshot};

/// Deletes one Stage 1 snapshot. It is intentionally non-authoritative: this
/// function cannot consume consent, change an operation state, or resume work.
pub(crate) fn cleanup_staged_snapshot(snapshot: &StagedSnapshot) -> AppResult<()> {
    let Some(parent) = existing_staging_parent(&snapshot.app_data_dir)? else {
        return Ok(());
    };
    let expected = parent.join(&snapshot.staging_id);
    if !is_valid_staging_id(&snapshot.staging_id) || snapshot.root != expected {
        return Err(AppError::InvalidInput(
            "Invalid Transform staging cleanup handle.".into(),
        ));
    }
    let result = remove_tree_without_following_symlinks(&expected);
    if let Err(error) = &result {
        logging::write_error_line(&format!(
            "[pastey:transform-staging] event=cleanup_failed staging_id={} error={}",
            snapshot.staging_id,
            error.message()
        ));
    }
    result
}

/// Removes only well-formed stale staging directories beneath Pastey's fixed
/// app-data parent. Orphans are never treated as resumable work.
pub(crate) fn cleanup_orphaned_transform_staging(app_data_dir: &Path) -> AppResult<usize> {
    let Some(parent) = existing_staging_parent(app_data_dir)? else {
        return Ok(0);
    };
    let mut removed = 0;
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            logging::write_error_line(
                "[pastey:transform-staging] event=orphan_cleanup_rejected reason=non_utf8_name",
            );
            continue;
        };
        if !is_valid_staging_id(name) {
            logging::write_error_line("[pastey:transform-staging] event=orphan_cleanup_rejected reason=invalid_staging_id");
            continue;
        }
        let path = parent.join(name);
        match remove_tree_without_following_symlinks(&path) {
            Ok(()) => removed += 1,
            Err(error) => logging::write_error_line(&format!(
                "[pastey:transform-staging] event=orphan_cleanup_failed staging_id={name} error={}",
                error.message()
            )),
        }
    }
    Ok(removed)
}

pub(super) fn cleanup_partial_staging_root(parent: &Path, root: &Path) -> AppResult<()> {
    let canonical_parent = fs::canonicalize(parent)?;
    if root.parent() != Some(canonical_parent.as_path())
        || !root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_valid_staging_id)
    {
        return Err(AppError::InvalidInput(
            "Invalid partial Transform staging root.".into(),
        ));
    }
    remove_tree_without_following_symlinks(root)
}

fn remove_tree_without_following_symlinks(path: &Path) -> AppResult<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        return Err(AppError::InvalidInput(
            "Transform staging cleanup refused a symlink.".into(),
        ));
    }
    if metadata.is_file() {
        fs::remove_file(path)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(AppError::InvalidInput(
            "Transform staging cleanup refused a special file.".into(),
        ));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child_name = entry.file_name();
        if child_name.is_empty()
            || Path::new(&child_name)
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(AppError::InvalidInput(
                "Transform staging cleanup found an invalid child name.".into(),
            ));
        }
        remove_tree_without_following_symlinks(&entry.path())?;
    }
    fs::remove_dir(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;
    use crate::transform_sandbox::{
        profile::DETERMINISTIC_STAGED_INPUT_TEST,
        staging::{canonical_staging_parent, prepare_staged_snapshot},
    };

    fn root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pastey_transform_cleanup_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn snapshot(root: &Path) -> StagedSnapshot {
        let source_root = root.join("source");
        fs::create_dir(&source_root).unwrap();
        let source = source_root.join("candidate");
        fs::write(&source, b"snapshot").unwrap();
        let identity =
            super::super::staging::capture_source_identity(&source, &source_root, 1024 * 1024)
                .unwrap();
        prepare_staged_snapshot(
            root,
            &source,
            &source_root,
            &identity,
            DETERMINISTIC_STAGED_INPUT_TEST,
        )
        .unwrap()
    }

    #[test]
    fn cleanup_is_idempotent_and_never_uses_snapshot_as_authority() {
        let root = root();
        let snapshot = snapshot(&root);
        assert!(snapshot.root.exists());
        cleanup_staged_snapshot(&snapshot).unwrap();
        cleanup_staged_snapshot(&snapshot).unwrap();
        assert!(!snapshot.root.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn orphan_cleanup_removes_only_valid_directories_and_refuses_symlinks() {
        let root = root();
        let snapshot = snapshot(&root);
        let parent = canonical_staging_parent(&root).unwrap();
        let invalid = parent.join("not-a-transform-stage");
        fs::create_dir(&invalid).unwrap();
        assert_eq!(cleanup_orphaned_transform_staging(&root).unwrap(), 1);
        assert!(!snapshot.root.exists());
        assert!(invalid.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = root.join("outside");
            fs::create_dir(&target).unwrap();
            let link = parent.join(format!("transform-stage-{}", uuid::Uuid::new_v4()));
            symlink(&target, &link).unwrap();
            assert_eq!(cleanup_orphaned_transform_staging(&root).unwrap(), 0);
            assert!(target.exists());
            assert!(link.exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_refuses_a_handle_that_escapes_the_fixed_parent() {
        let root = root();
        let snapshot = snapshot(&root);
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        let escaped = StagedSnapshot {
            root: outside.clone(),
            ..snapshot.clone()
        };
        assert!(cleanup_staged_snapshot(&escaped).is_err());
        assert!(outside.exists());
        cleanup_staged_snapshot(&snapshot).unwrap();
        let _ = fs::remove_dir_all(root);
    }
}
