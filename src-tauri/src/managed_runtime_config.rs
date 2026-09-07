//! Host-local managed runtime configuration.
//!
//! Runtime capability probes are factual observations only. This service is
//! the separate Host control-plane configuration seam that records an allowed
//! logical runtime identity, pins the exact discovered executable identity, and later
//! resolves it without accepting a path from a Plan, requester, PM, Worker,
//! or provider. It creates no execution authority; the existing exact binder,
//! admission, EffectEnvelope, and ExecutionWorld remain authoritative.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{
    capability_probe,
    error::{AppError, AppResult},
    managed_execution::ManagedProcessWorldSpecV1,
    storage::{self, AppPaths},
};

const EXECUTE_SELECTION_KEY: &str = "managed_execute";

pub(crate) struct ManagedRuntimeConfigServiceV1 {
    paths: AppPaths,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedRuntimeOptionV1 {
    pub(crate) runtime_id: String,
    pub(crate) available: bool,
    pub(crate) selected: bool,
    pub(crate) selection_ready: bool,
}

impl ManagedRuntimeConfigServiceV1 {
    pub(crate) fn new(paths: AppPaths) -> AppResult<Self> {
        let service = Self { paths };
        service.ensure_schema()?;
        Ok(service)
    }

    /// Host control-plane operation: allow one known, locally discovered
    /// runtime identity and select it for future managed Execute steps.
    /// Callers supply only the logical identity; executable paths are resolved
    /// inside the Host and never cross the command/protocol/model boundary.
    pub(crate) fn configure_discovered_for_managed_execute(
        &self,
        runtime_id: &str,
    ) -> AppResult<()> {
        let discovered =
            capability_probe::discover_managed_runtime(runtime_id)?.ok_or_else(|| {
                AppError::NotFound("Managed runtime is not installed on this Host.".into())
            })?;
        self.configure_for_managed_execute(runtime_id, discovered.process_world)
    }

    /// Resolves the selected logical runtime to its current exact Host-private
    /// executable. Replacement, removal, or selection substitution fails
    /// closed instead of falling back to PATH or another runtime.
    pub(crate) fn selected_for_managed_execute(
        &self,
    ) -> AppResult<Option<ManagedProcessWorldSpecV1>> {
        let selected = self
            .connection()?
            .query_row(
                "SELECT s.runtime_id, c.executable_identity_ref,
                        c.executable_path, c.scope_root
                 FROM managed_runtime_selection s
                 JOIN managed_runtime_configs c ON c.runtime_id = s.runtime_id
                 WHERE s.selection_key = ?1",
                [EXECUTE_SELECTION_KEY],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((runtime_id, expected_identity_ref, executable_path, scope_root)) = selected
        else {
            return Ok(None);
        };
        capability_probe::validate_managed_runtime_id(&runtime_id)?;
        let process_world =
            ManagedProcessWorldSpecV1::new(crate::managed_resources::ExecutableBindingSpecV1 {
                executable_path: PathBuf::from(executable_path),
                scope_root: PathBuf::from(scope_root),
            })?;
        capability_probe::validate_managed_runtime_executable(&process_world.executable)?;
        let current_identity_ref = process_world.validate_executable_identity()?;
        if current_identity_ref != expected_identity_ref {
            return invalid("Selected managed runtime executable identity changed.");
        }
        Ok(Some(process_world))
    }

    pub(crate) fn selected_managed_execute_runtime_id(&self) -> AppResult<Option<String>> {
        self.connection()?
            .query_row(
                "SELECT runtime_id FROM managed_runtime_selection WHERE selection_key = ?1",
                [EXECUTE_SELECTION_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Renderer-safe Host settings projection. It exposes logical identities
    /// and readiness only; physical paths and safe identity references stay in
    /// this service.
    pub(crate) fn list_managed_execute_options(&self) -> AppResult<Vec<ManagedRuntimeOptionV1>> {
        let selected = self.selected_managed_execute_runtime_id()?;
        let selection_ready = self
            .selected_for_managed_execute()
            .is_ok_and(|value| value.is_some());
        [
            capability_probe::MANAGED_PYTHON_RUNTIME_ID,
            capability_probe::MANAGED_NODE_RUNTIME_ID,
        ]
        .into_iter()
        .map(|runtime_id| {
            let is_selected = selected.as_deref() == Some(runtime_id);
            Ok(ManagedRuntimeOptionV1 {
                runtime_id: runtime_id.into(),
                available: capability_probe::discover_managed_runtime(runtime_id)?.is_some(),
                selected: is_selected,
                selection_ready: is_selected && selection_ready,
            })
        })
        .collect()
    }

    fn configure_for_managed_execute(
        &self,
        runtime_id: &str,
        process_world: ManagedProcessWorldSpecV1,
    ) -> AppResult<()> {
        capability_probe::validate_managed_runtime_id(runtime_id)?;
        capability_probe::validate_managed_runtime_executable(&process_world.executable)?;
        let identity_ref = process_world.validate_executable_identity()?;
        let executable_path = process_world
            .executable
            .executable_path
            .to_str()
            .ok_or_else(|| {
                AppError::InvalidInput("Managed runtime path is not valid UTF-8.".into())
            })?;
        let scope_root = process_world
            .executable
            .scope_root
            .to_str()
            .ok_or_else(|| {
                AppError::InvalidInput("Managed runtime scope is not valid UTF-8.".into())
            })?;
        let now = storage::now_ts();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO managed_runtime_configs
             (runtime_id, executable_identity_ref, executable_path, scope_root, configured_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(runtime_id) DO UPDATE SET
               executable_identity_ref = excluded.executable_identity_ref,
               executable_path = excluded.executable_path,
               scope_root = excluded.scope_root,
               configured_at = excluded.configured_at",
            params![runtime_id, identity_ref, executable_path, scope_root, now],
        )?;
        transaction.execute(
            "INSERT INTO managed_runtime_selection
             (selection_key, runtime_id, selected_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(selection_key) DO UPDATE SET
               runtime_id = excluded.runtime_id,
               selected_at = excluded.selected_at",
            params![EXECUTE_SELECTION_KEY, runtime_id, now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn configure_for_managed_execute_for_tests(
        &self,
        runtime_id: &str,
        process_world: ManagedProcessWorldSpecV1,
    ) -> AppResult<()> {
        self.configure_for_managed_execute(runtime_id, process_world)
    }

    fn ensure_schema(&self) -> AppResult<()> {
        self.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS managed_runtime_configs (
                runtime_id TEXT PRIMARY KEY,
                executable_identity_ref TEXT NOT NULL,
                executable_path TEXT NOT NULL,
                scope_root TEXT NOT NULL,
                configured_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS managed_runtime_selection (
                selection_key TEXT PRIMARY KEY CHECK(selection_key = 'managed_execute'),
                runtime_id TEXT NOT NULL,
                selected_at INTEGER NOT NULL,
                FOREIGN KEY(runtime_id) REFERENCES managed_runtime_configs(runtime_id)
                    ON DELETE CASCADE
            );",
        )?;
        Ok(())
    }

    fn connection(&self) -> AppResult<Connection> {
        let connection = Connection::open(&self.paths.db_path)?;
        connection.execute("PRAGMA foreign_keys = ON", [])?;
        Ok(connection)
    }
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_resources::ExecutableBindingSpecV1;

    struct TestRoot(PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn service() -> (TestRoot, ManagedRuntimeConfigServiceV1) {
        let root = TestRoot(std::env::temp_dir().join(format!(
            "pastey-managed-runtime-config-{}",
            uuid::Uuid::new_v4()
        )));
        let paths = AppPaths::new(root.0.clone(), root.0.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let service = ManagedRuntimeConfigServiceV1::new(paths).unwrap();
        (root, service)
    }

    fn test_process_world(root: &std::path::Path) -> ManagedProcessWorldSpecV1 {
        let executable_path = root.join("runtime-test-entrypoint");
        std::fs::write(&executable_path, b"runtime-v1").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&executable_path).unwrap().permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&executable_path, permissions).unwrap();
        }
        ManagedProcessWorldSpecV1::new(ExecutableBindingSpecV1 {
            executable_path,
            scope_root: root.to_path_buf(),
        })
        .unwrap()
    }

    #[test]
    fn logical_selection_resolves_only_the_pinned_host_private_identity() {
        let (root, service) = service();
        let process_world = test_process_world(&root.0);
        service
            .configure_for_managed_execute_for_tests(
                capability_probe::MANAGED_PYTHON_RUNTIME_ID,
                process_world.clone(),
            )
            .unwrap();

        let resolved = service
            .selected_for_managed_execute()
            .unwrap()
            .expect("selected runtime");
        assert_eq!(
            resolved.executable.executable_path,
            process_world.executable.executable_path
        );
        assert_eq!(
            service.selected_managed_execute_runtime_id().unwrap(),
            Some(capability_probe::MANAGED_PYTHON_RUNTIME_ID.into())
        );
        let restarted = ManagedRuntimeConfigServiceV1::new(service.paths.clone()).unwrap();
        assert!(restarted.selected_for_managed_execute().unwrap().is_some());
        let projection = serde_json::to_string(&restarted.list_managed_execute_options().unwrap())
            .expect("serialize renderer-safe runtime options");
        assert!(!projection.contains(root.0.to_str().unwrap()));
        assert!(!projection.contains("executablePath"));
        assert!(!projection.contains("identityRef"));

        std::fs::write(&process_world.executable.executable_path, b"runtime-v2").unwrap();
        assert!(restarted.selected_for_managed_execute().is_err());
    }

    #[test]
    fn unsupported_runtime_identity_cannot_be_configured() {
        let (root, service) = service();
        assert!(service
            .configure_for_managed_execute_for_tests("shell", test_process_world(&root.0))
            .is_err());
        assert_eq!(service.selected_managed_execute_runtime_id().unwrap(), None);
    }
}
