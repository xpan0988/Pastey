//! Durable Host-owned Worker provider configuration.
//!
//! Non-secret metadata is stored in SQLite. Credentials are stored separately,
//! authenticated-encrypted with Pastey's existing Host master key, and are
//! materialized only into an immutable process-local binding. None of these
//! types are Plan authority, effect authority, or Worker-visible handles.

#![allow(dead_code)] // The private Phase 6 coordinator seam is not product attached.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
};

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    crypto,
    error::{AppError, AppResult},
    storage::{self, AppPaths},
    worker_provider::ConfiguredWorkerProviderConfigV1,
};

pub(crate) const WORKER_PROVIDER_CONFIG_VERSION: &str = "pastey-worker-provider-config-v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderConfigRefV1 {
    pub(crate) provider_id: String,
    pub(crate) generation: u64,
    pub(crate) config_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderSelectionV1 {
    pub(crate) config_ref: WorkerProviderConfigRefV1,
    pub(crate) model: String,
}

pub(crate) struct WorkerProviderConfigWriteV1 {
    pub(crate) provider_id: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) api_key: String,
    pub(crate) timeout_millis: u64,
    pub(crate) max_output_tokens: u32,
}

impl fmt::Debug for WorkerProviderConfigWriteV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerProviderConfigWriteV1")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &"[redacted]")
            .field("timeout_millis", &self.timeout_millis)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

pub(crate) struct WorkerProviderConfigUpdateV1 {
    pub(crate) expected_ref: WorkerProviderConfigRefV1,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) replacement_api_key: Option<String>,
    pub(crate) timeout_millis: u64,
    pub(crate) max_output_tokens: u32,
}

impl fmt::Debug for WorkerProviderConfigUpdateV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerProviderConfigUpdateV1")
            .field("expected_ref", &self.expected_ref)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field(
                "replacement_api_key",
                &self.replacement_api_key.as_ref().map(|_| "[redacted]"),
            )
            .field("timeout_millis", &self.timeout_millis)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkerProviderHealthStateV1 {
    Unknown,
    Healthy,
    Unhealthy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerProviderMetadataV1 {
    pub(crate) config_ref: WorkerProviderConfigRefV1,
    pub(crate) model: String,
    pub(crate) available: bool,
    pub(crate) health: WorkerProviderHealthStateV1,
    pub(crate) last_health_check_at: Option<i64>,
    pub(crate) updated_at: i64,
}

/// Immutable for one run. Updates create a new generation and cannot mutate
/// this value. Explicit deletion flips only the revocation token so an active
/// or pending request fails closed without substituting another provider.
pub(crate) struct ResolvedWorkerProviderBindingV1 {
    pub(crate) config_ref: WorkerProviderConfigRefV1,
    pub(crate) provider_config: ConfiguredWorkerProviderConfigV1,
    revoked: Arc<AtomicBool>,
}

impl fmt::Debug for ResolvedWorkerProviderBindingV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedWorkerProviderBindingV1")
            .field("config_ref", &self.config_ref)
            .field("provider_config", &self.provider_config)
            .field("revoked", &self.is_revoked())
            .finish()
    }
}

impl ResolvedWorkerProviderBindingV1 {
    pub(crate) fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire)
    }

    pub(crate) fn revocation_token(&self) -> Arc<AtomicBool> {
        self.revoked.clone()
    }
}

pub(crate) struct WorkerProviderConfigServiceV1 {
    paths: AppPaths,
    master_key: [u8; 32],
    active_bindings: Mutex<HashMap<String, Vec<Weak<AtomicBool>>>>,
}

impl WorkerProviderConfigServiceV1 {
    pub(crate) fn new(paths: AppPaths, master_key: [u8; 32]) -> AppResult<Self> {
        let service = Self {
            paths,
            master_key,
            active_bindings: Mutex::new(HashMap::new()),
        };
        service.ensure_schema()?;
        Ok(service)
    }

    pub(crate) fn create(
        &self,
        write: WorkerProviderConfigWriteV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        let validated = validate_write(write)?;
        let generation = 1;
        let config_ref = config_ref(
            &validated.provider_id,
            generation,
            &validated.base_url,
            &validated.model,
            validated.timeout_millis,
            validated.max_output_tokens,
        );
        let (credential, nonce) =
            crypto::wrap_bytes(validated.api_key.as_bytes(), &self.master_key)?;
        let now = storage::now_ts();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO worker_provider_configs
             (provider_id, generation, config_digest, base_url, model, timeout_millis,
              max_output_tokens, health, last_health_check_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'unknown', NULL, ?8)",
            params![
                validated.provider_id,
                generation,
                config_ref.config_digest,
                validated.base_url,
                validated.model,
                validated.timeout_millis,
                validated.max_output_tokens,
                now,
            ],
        )?;
        transaction.execute(
            "INSERT INTO worker_provider_credentials
             (provider_id, generation, wrapped_credential, credential_nonce)
             VALUES (?1, ?2, ?3, ?4)",
            params![config_ref.provider_id, generation, credential, nonce],
        )?;
        transaction.commit()?;
        self.metadata(&config_ref)
    }

    pub(crate) fn update(
        &self,
        update: WorkerProviderConfigUpdateV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        let current = self.load_row(&update.expected_ref)?;
        let credential = if let Some(replacement) = update.replacement_api_key {
            replacement
        } else {
            self.decrypt_credential(&current)?
        };
        let validated = validate_write(WorkerProviderConfigWriteV1 {
            provider_id: update.expected_ref.provider_id.clone(),
            base_url: update.base_url,
            model: update.model,
            api_key: credential.clone(),
            timeout_millis: update.timeout_millis,
            max_output_tokens: update.max_output_tokens,
        })?;
        let generation = current.generation.checked_add(1).ok_or_else(|| {
            AppError::InvalidInput("Worker provider generation is exhausted.".into())
        })?;
        let next_ref = config_ref(
            &validated.provider_id,
            generation,
            &validated.base_url,
            &validated.model,
            validated.timeout_millis,
            validated.max_output_tokens,
        );
        let (wrapped, nonce) = crypto::wrap_bytes(credential.as_bytes(), &self.master_key)?;
        let now = storage::now_ts();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "UPDATE worker_provider_configs SET generation = ?1, config_digest = ?2,
             base_url = ?3, model = ?4, timeout_millis = ?5, max_output_tokens = ?6,
             health = 'unknown', last_health_check_at = NULL, updated_at = ?7
             WHERE provider_id = ?8 AND generation = ?9 AND config_digest = ?10",
            params![
                generation,
                next_ref.config_digest,
                validated.base_url,
                validated.model,
                validated.timeout_millis,
                validated.max_output_tokens,
                now,
                next_ref.provider_id,
                current.generation,
                current.config_digest,
            ],
        )?;
        if changed != 1 {
            return invalid("Worker provider configuration changed during update.");
        }
        transaction.execute(
            "UPDATE worker_provider_credentials SET generation = ?1,
             wrapped_credential = ?2, credential_nonce = ?3 WHERE provider_id = ?4",
            params![generation, wrapped, nonce, next_ref.provider_id],
        )?;
        transaction.commit()?;
        self.metadata(&next_ref)
    }

    pub(crate) fn delete(&self, expected: &WorkerProviderConfigRefV1) -> AppResult<()> {
        self.load_row(expected)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM worker_provider_selection
             WHERE provider_id = ?1 AND generation = ?2 AND config_digest = ?3",
            params![
                expected.provider_id,
                expected.generation,
                expected.config_digest
            ],
        )?;
        let deleted = transaction.execute(
            "DELETE FROM worker_provider_configs
             WHERE provider_id = ?1 AND generation = ?2 AND config_digest = ?3",
            params![
                expected.provider_id,
                expected.generation,
                expected.config_digest
            ],
        )?;
        if deleted != 1 {
            return invalid("Worker provider configuration changed during deletion.");
        }
        transaction.commit()?;
        if let Some(tokens) = self.active_bindings.lock().remove(&expected.provider_id) {
            for token in tokens.into_iter().filter_map(|token| token.upgrade()) {
                token.store(true, Ordering::Release);
            }
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        selection: &WorkerProviderSelectionV1,
    ) -> AppResult<ResolvedWorkerProviderBindingV1> {
        let row = self.load_row(&selection.config_ref)?;
        if selection.model != row.model {
            return invalid("Worker provider model selection is stale or substituted.");
        }
        let api_key = self.decrypt_credential(&row)?;
        let provider_config = ConfiguredWorkerProviderConfigV1::new(
            row.provider_id.clone(),
            row.base_url,
            row.model,
            api_key,
            row.timeout_millis,
            row.max_output_tokens,
        )?;
        let revoked = Arc::new(AtomicBool::new(false));
        let mut active = self.active_bindings.lock();
        let tokens = active.entry(row.provider_id).or_default();
        tokens.retain(|token| token.strong_count() > 0);
        tokens.push(Arc::downgrade(&revoked));
        Ok(ResolvedWorkerProviderBindingV1 {
            config_ref: selection.config_ref.clone(),
            provider_config,
            revoked,
        })
    }

    /// Host control-plane choice for managed Worker runs. Resolving first
    /// proves the exact generation/model exists without exposing credentials.
    pub(crate) fn select_for_managed_workers(
        &self,
        selection: &WorkerProviderSelectionV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        drop(self.resolve(selection)?);
        self.connection()?.execute(
            "INSERT INTO worker_provider_selection
             (selection_key, provider_id, generation, config_digest, model, selected_at)
             VALUES ('managed_worker', ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(selection_key) DO UPDATE SET provider_id = excluded.provider_id,
             generation = excluded.generation, config_digest = excluded.config_digest,
             model = excluded.model, selected_at = excluded.selected_at",
            params![
                selection.config_ref.provider_id,
                selection.config_ref.generation,
                selection.config_ref.config_digest,
                selection.model,
                storage::now_ts(),
            ],
        )?;
        self.metadata(&selection.config_ref)
    }

    pub(crate) fn selected_for_managed_workers(&self) -> AppResult<WorkerProviderSelectionV1> {
        let selection = self
            .connection()?
            .query_row(
                "SELECT provider_id, generation, config_digest, model
                 FROM worker_provider_selection WHERE selection_key = 'managed_worker'",
                [],
                |row| {
                    Ok(WorkerProviderSelectionV1 {
                        config_ref: WorkerProviderConfigRefV1 {
                            provider_id: row.get(0)?,
                            generation: row.get(1)?,
                            config_digest: row.get(2)?,
                        },
                        model: row.get(3)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::NotFound("Managed Worker provider selection is unavailable.".into())
            })?;
        self.load_row(&selection.config_ref)?;
        Ok(selection)
    }

    pub(crate) fn list_metadata(&self) -> AppResult<Vec<WorkerProviderMetadataV1>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT provider_id, generation, config_digest, model, health,
             last_health_check_at, updated_at FROM worker_provider_configs
             ORDER BY provider_id",
        )?;
        let metadata = statement
            .query_map([], metadata_from_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into);
        metadata
    }

    pub(crate) fn record_health(
        &self,
        expected: &WorkerProviderConfigRefV1,
        health: WorkerProviderHealthStateV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        let state = match health {
            WorkerProviderHealthStateV1::Unknown => "unknown",
            WorkerProviderHealthStateV1::Healthy => "healthy",
            WorkerProviderHealthStateV1::Unhealthy => "unhealthy",
        };
        let connection = self.connection()?;
        let changed = connection.execute(
            "UPDATE worker_provider_configs SET health = ?1, last_health_check_at = ?2
             WHERE provider_id = ?3 AND generation = ?4 AND config_digest = ?5",
            params![
                state,
                storage::now_ts(),
                expected.provider_id,
                expected.generation,
                expected.config_digest,
            ],
        )?;
        if changed != 1 {
            return invalid("Worker provider configuration reference is stale.");
        }
        self.metadata(expected)
    }

    fn metadata(
        &self,
        expected: &WorkerProviderConfigRefV1,
    ) -> AppResult<WorkerProviderMetadataV1> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT provider_id, generation, config_digest, model, health,
                 last_health_check_at, updated_at FROM worker_provider_configs
                 WHERE provider_id = ?1 AND generation = ?2 AND config_digest = ?3",
                params![
                    expected.provider_id,
                    expected.generation,
                    expected.config_digest
                ],
                metadata_from_row,
            )
            .optional()?
            .ok_or_else(|| {
                AppError::NotFound("Worker provider configuration is unavailable.".into())
            })
    }

    fn load_row(&self, expected: &WorkerProviderConfigRefV1) -> AppResult<StoredProviderRowV1> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT c.provider_id, c.generation, c.config_digest, c.base_url, c.model,
                 c.timeout_millis, c.max_output_tokens, k.wrapped_credential,
                 k.credential_nonce FROM worker_provider_configs c
                 JOIN worker_provider_credentials k ON k.provider_id = c.provider_id
                 AND k.generation = c.generation
                 WHERE c.provider_id = ?1 AND c.generation = ?2 AND c.config_digest = ?3",
                params![
                    expected.provider_id,
                    expected.generation,
                    expected.config_digest
                ],
                |row| {
                    Ok(StoredProviderRowV1 {
                        provider_id: row.get(0)?,
                        generation: row.get(1)?,
                        config_digest: row.get(2)?,
                        base_url: row.get(3)?,
                        model: row.get(4)?,
                        timeout_millis: row.get(5)?,
                        max_output_tokens: row.get(6)?,
                        wrapped_credential: row.get(7)?,
                        credential_nonce: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                AppError::NotFound("Worker provider configuration is unavailable or stale.".into())
            })
    }

    fn decrypt_credential(&self, row: &StoredProviderRowV1) -> AppResult<String> {
        String::from_utf8(crypto::unwrap_bytes(
            &row.wrapped_credential,
            &row.credential_nonce,
            &self.master_key,
        )?)
        .map_err(Into::into)
    }

    fn ensure_schema(&self) -> AppResult<()> {
        self.connection()?.execute_batch(
            "CREATE TABLE IF NOT EXISTS worker_provider_configs (
                provider_id TEXT PRIMARY KEY,
                generation INTEGER NOT NULL,
                config_digest TEXT NOT NULL,
                base_url TEXT NOT NULL,
                model TEXT NOT NULL,
                timeout_millis INTEGER NOT NULL,
                max_output_tokens INTEGER NOT NULL,
                health TEXT NOT NULL,
                last_health_check_at INTEGER,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS worker_provider_credentials (
                provider_id TEXT PRIMARY KEY,
                generation INTEGER NOT NULL,
                wrapped_credential TEXT NOT NULL,
                credential_nonce TEXT NOT NULL,
                FOREIGN KEY(provider_id) REFERENCES worker_provider_configs(provider_id)
                    ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS worker_provider_selection (
                selection_key TEXT PRIMARY KEY CHECK(selection_key = 'managed_worker'),
                provider_id TEXT NOT NULL,
                generation INTEGER NOT NULL,
                config_digest TEXT NOT NULL,
                model TEXT NOT NULL,
                selected_at INTEGER NOT NULL
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

struct StoredProviderRowV1 {
    provider_id: String,
    generation: u64,
    config_digest: String,
    base_url: String,
    model: String,
    timeout_millis: u64,
    max_output_tokens: u32,
    wrapped_credential: String,
    credential_nonce: String,
}

fn validate_write(write: WorkerProviderConfigWriteV1) -> AppResult<WorkerProviderConfigWriteV1> {
    let validated = ConfiguredWorkerProviderConfigV1::new(
        write.provider_id.clone(),
        write.base_url.clone(),
        write.model.clone(),
        write.api_key.clone(),
        write.timeout_millis,
        write.max_output_tokens,
    )?;
    Ok(WorkerProviderConfigWriteV1 {
        provider_id: validated.provider_id,
        base_url: validated.base_url,
        model: validated.model,
        api_key: write.api_key,
        timeout_millis: validated.timeout_millis,
        max_output_tokens: validated.max_output_tokens,
    })
}

fn config_ref(
    provider_id: &str,
    generation: u64,
    base_url: &str,
    model: &str,
    timeout_millis: u64,
    max_output_tokens: u32,
) -> WorkerProviderConfigRefV1 {
    let canonical = serde_json::to_vec(&(
        WORKER_PROVIDER_CONFIG_VERSION,
        provider_id,
        generation,
        base_url,
        model,
        timeout_millis,
        max_output_tokens,
    ))
    .expect("Worker provider config reference serialization");
    WorkerProviderConfigRefV1 {
        provider_id: provider_id.into(),
        generation,
        config_digest: format!(
            "pastey-worker-provider-config-v1:{}",
            blake3::hash(&canonical).to_hex()
        ),
    }
}

fn metadata_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerProviderMetadataV1> {
    let health: String = row.get(4)?;
    Ok(WorkerProviderMetadataV1 {
        config_ref: WorkerProviderConfigRefV1 {
            provider_id: row.get(0)?,
            generation: row.get(1)?,
            config_digest: row.get(2)?,
        },
        model: row.get(3)?,
        available: true,
        health: match health.as_str() {
            "healthy" => WorkerProviderHealthStateV1::Healthy,
            "unhealthy" => WorkerProviderHealthStateV1::Unhealthy,
            _ => WorkerProviderHealthStateV1::Unknown,
        },
        last_health_check_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn invalid<T>(message: &str) -> AppResult<T> {
    Err(AppError::InvalidInput(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        worker_harness::WorkerProviderErrorKindV1,
        worker_provider::OpenAICompatibleStreamingWorkerProviderV1,
    };

    struct TestRoot(std::path::PathBuf);

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn service() -> (TestRoot, WorkerProviderConfigServiceV1, [u8; 32]) {
        let root = TestRoot(std::env::temp_dir().join(format!(
            "pastey_worker_provider_config_{}",
            uuid::Uuid::new_v4()
        )));
        std::fs::create_dir_all(&root.0).unwrap();
        let paths = AppPaths::new(root.0.clone(), root.0.join("logs"));
        paths.ensure_directories().unwrap();
        let key = crypto::random_key();
        let service = WorkerProviderConfigServiceV1::new(paths, key).unwrap();
        (root, service, key)
    }

    fn write(model: &str, secret: &str) -> WorkerProviderConfigWriteV1 {
        WorkerProviderConfigWriteV1 {
            provider_id: "primary".into(),
            base_url: "https://api.example.test/v1/".into(),
            model: model.into(),
            api_key: secret.into(),
            timeout_millis: 10_000,
            max_output_tokens: 512,
        }
    }

    #[test]
    fn persistence_load_update_delete_and_restart_are_generation_bound() {
        let (_root, service, key) = service();
        let created = service.create(write("model-a", "secret-a")).unwrap();
        let paths = service.paths.clone();
        let restarted = WorkerProviderConfigServiceV1::new(paths, key).unwrap();
        assert_eq!(restarted.list_metadata().unwrap(), vec![created.clone()]);

        let updated = restarted
            .update(WorkerProviderConfigUpdateV1 {
                expected_ref: created.config_ref.clone(),
                base_url: "https://api.example.test/v1".into(),
                model: "model-b".into(),
                replacement_api_key: Some("secret-b".into()),
                timeout_millis: 20_000,
                max_output_tokens: 1024,
            })
            .unwrap();
        assert_eq!(updated.config_ref.generation, 2);
        assert!(restarted
            .resolve(&WorkerProviderSelectionV1 {
                config_ref: created.config_ref,
                model: "model-a".into(),
            })
            .is_err());
        restarted.delete(&updated.config_ref).unwrap();
        assert!(restarted.list_metadata().unwrap().is_empty());
    }

    #[test]
    fn managed_selection_persists_but_stale_update_and_delete_fail_closed() {
        let (_root, service, key) = service();
        let created = service.create(write("model-a", "secret-a")).unwrap();
        let selection = WorkerProviderSelectionV1 {
            config_ref: created.config_ref.clone(),
            model: created.model.clone(),
        };
        service.select_for_managed_workers(&selection).unwrap();
        let restarted = WorkerProviderConfigServiceV1::new(service.paths.clone(), key).unwrap();
        assert_eq!(restarted.selected_for_managed_workers().unwrap(), selection);

        let updated = restarted
            .update(WorkerProviderConfigUpdateV1 {
                expected_ref: created.config_ref,
                base_url: "https://api.example.test/v1".into(),
                model: "model-b".into(),
                replacement_api_key: None,
                timeout_millis: 10_000,
                max_output_tokens: 512,
            })
            .unwrap();
        assert!(restarted.selected_for_managed_workers().is_err());
        let replacement = WorkerProviderSelectionV1 {
            config_ref: updated.config_ref.clone(),
            model: updated.model,
        };
        restarted.select_for_managed_workers(&replacement).unwrap();
        restarted.delete(&updated.config_ref).unwrap();
        assert!(restarted.selected_for_managed_workers().is_err());
    }

    #[test]
    fn resolved_binding_is_immutable_across_update_and_revoked_by_delete() {
        let (_root, service, _) = service();
        let created = service.create(write("model-a", "secret-a")).unwrap();
        let binding = service
            .resolve(&WorkerProviderSelectionV1 {
                config_ref: created.config_ref.clone(),
                model: "model-a".into(),
            })
            .unwrap();
        let updated = service
            .update(WorkerProviderConfigUpdateV1 {
                expected_ref: created.config_ref,
                base_url: "https://other.example.test/v1".into(),
                model: "model-b".into(),
                replacement_api_key: Some("secret-b".into()),
                timeout_millis: 12_000,
                max_output_tokens: 600,
            })
            .unwrap();
        assert_eq!(binding.provider_config.model, "model-a");
        assert!(!binding.is_revoked());
        let provider = OpenAICompatibleStreamingWorkerProviderV1::from_binding(binding).unwrap();
        service.delete(&updated.config_ref).unwrap();
        assert_eq!(
            provider.health_probe().unwrap_err().kind,
            WorkerProviderErrorKindV1::ProviderRevoked
        );
        assert!(service
            .resolve(&WorkerProviderSelectionV1 {
                config_ref: updated.config_ref,
                model: "model-b".into(),
            })
            .is_err());
    }

    #[test]
    fn credentials_are_encrypted_and_absent_from_metadata_debug_and_errors() {
        let (_root, service, _) = service();
        let secret = "credential-must-not-leak";
        let write = write("model-a", secret);
        assert!(!format!("{write:?}").contains(secret));
        let created = service.create(write).unwrap();
        let database = std::fs::read(&service.paths.db_path).unwrap();
        assert!(!database
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        let metadata = serde_json::to_string(&service.list_metadata().unwrap()).unwrap();
        assert!(!metadata.contains(secret));
        assert!(!format!(
            "{:?}",
            service
                .resolve(&WorkerProviderSelectionV1 {
                    config_ref: created.config_ref,
                    model: "wrong".into(),
                })
                .unwrap_err()
        )
        .contains(secret));
    }

    #[test]
    fn invalid_config_model_and_stale_references_fail_closed() {
        let (_root, service, _) = service();
        assert!(service
            .create(WorkerProviderConfigWriteV1 {
                base_url: "http://insecure.test".into(),
                ..write("model", "secret")
            })
            .is_err());
        let created = service.create(write("model", "secret")).unwrap();
        assert!(service
            .resolve(&WorkerProviderSelectionV1 {
                config_ref: created.config_ref.clone(),
                model: "substituted".into(),
            })
            .is_err());
        let mut stale = created.config_ref;
        stale.generation += 1;
        assert!(service
            .resolve(&WorkerProviderSelectionV1 {
                config_ref: stale,
                model: "model".into(),
            })
            .is_err());
    }

    #[test]
    fn metadata_health_projection_is_bounded_and_non_secret() {
        let (_root, service, _) = service();
        let created = service.create(write("model", "secret")).unwrap();
        let healthy = service
            .record_health(&created.config_ref, WorkerProviderHealthStateV1::Healthy)
            .unwrap();
        assert_eq!(healthy.health, WorkerProviderHealthStateV1::Healthy);
        let json = serde_json::to_string(&healthy).unwrap();
        assert!(!json.contains("base_url"));
        assert!(!json.contains("credential"));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("EffectRequest"));
        assert!(!json.contains("SecretHandle"));
        assert!(!json.contains("NetworkGrant"));
    }

    #[test]
    fn provider_control_plane_has_no_direct_product_registration() {
        let main_source = include_str!("main.rs");
        let invoke_registration = main_source
            .split(".invoke_handler(tauri::generate_handler![")
            .nth(1)
            .and_then(|source| source.split("])").next())
            .expect("Tauri invoke registration remains source-visible");
        assert!(!invoke_registration.contains("worker"));
        assert!(!invoke_registration.contains("provider"));
        assert!(!include_str!("commands.rs").contains("run_v2_worker_with_provider_selection"));
        let room_control = include_str!("room_control.rs");
        assert!(room_control.contains("accept_live_v2_managed_attempt"));
        assert!(!room_control.contains("select_for_managed_workers"));
        assert!(!room_control.contains("delete_worker_provider_config"));
    }
}
