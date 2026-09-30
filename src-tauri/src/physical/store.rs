//! Host-private durable facts. Rows and epoch operations never convey permission.
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{de::DeserializeOwned, Serialize};

use super::{
    binding::EnvironmentRegistrationV1, contracts::PhysicalQualificationV1, require, values::*,
};
use crate::{
    error::{AppError, AppResult},
    storage::AppPaths,
};

#[path = "store_control.rs"]
mod control_ledger;
#[path = "store_core.rs"]
mod core_ledger;
#[path = "store_evidence.rs"]
mod evidence_ledger;
pub(super) use control_ledger::{
    ActionAuditV1, ActionDispositionV1, DecisionRecordV1, FenceAuditV1, ReservationReceiptV1,
    SessionAuditV1,
};
pub(super) use core_ledger::RootAuditV1;
#[path = "store_remote.rs"]
mod remote_ledger;
pub(super) use remote_ledger::RemoteRootLineageV2;
#[path = "store_native.rs"]
mod native_ledger;
#[path = "store_qualification.rs"]
mod qualification_ledger;

// Dedicated versioning; neither SQLite user_version nor other Pastey tables are repurposed.
const SCHEMA: &str = r#"
CREATE TABLE physical_schema (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    version INTEGER NOT NULL CHECK(version = 1)
) STRICT;
INSERT INTO physical_schema VALUES (1, 1);
CREATE TABLE physical_domains (
    domain_id TEXT PRIMARY KEY,
    resource_key TEXT NOT NULL UNIQUE,
    epoch INTEGER NOT NULL CHECK(epoch >= 1),
    quarantined INTEGER NOT NULL CHECK(quarantined = 1)
) STRICT;
CREATE TABLE physical_aliases (
    alias TEXT PRIMARY KEY,
    domain_id TEXT NOT NULL REFERENCES physical_domains(domain_id)
) STRICT;
CREATE TABLE physical_environments (
    environment_id TEXT PRIMARY KEY,
    host_ref TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision >= 1),
    configuration_ref TEXT NOT NULL,
    configuration_digest TEXT NOT NULL CHECK(length(configuration_digest) = 64),
    registration_digest TEXT NOT NULL CHECK(length(registration_digest) = 64),
    record_json TEXT NOT NULL,
    retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    denial_revision INTEGER NOT NULL DEFAULT 0 CHECK(denial_revision = 0 OR denial_revision > revision)
) STRICT;
CREATE TABLE physical_environment_domains (
    environment_id TEXT NOT NULL REFERENCES physical_environments(environment_id),
    domain_id TEXT NOT NULL REFERENCES physical_domains(domain_id),
    PRIMARY KEY(environment_id, domain_id)
) STRICT;
CREATE TABLE physical_qualifications (
    qualification_id TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES physical_environments(environment_id),
    revision INTEGER NOT NULL CHECK(revision >= 1),
    registration_digest TEXT NOT NULL CHECK(length(registration_digest) = 64),
    profile_digest TEXT NOT NULL CHECK(length(profile_digest) = 64),
    binding_digest TEXT NOT NULL CHECK(length(binding_digest) = 64),
    evidence_class TEXT NOT NULL CHECK(evidence_class IN ('simulation','hardware')),
    enforcement_class TEXT NOT NULL CHECK(enforcement_class IN ('adapter_isolation_only','native_fence')),
    evidence_digest TEXT NOT NULL CHECK(length(evidence_digest) = 64),
    conditions_digest TEXT NOT NULL CHECK(length(conditions_digest) = 64),
    provenance_digest TEXT NOT NULL CHECK(length(provenance_digest) = 64),
    record_digest TEXT NOT NULL CHECK(length(record_digest) = 64),
    record_json TEXT NOT NULL,
    expires_at INTEGER NOT NULL CHECK(expires_at > 0),
    withdrawal_revision INTEGER NOT NULL DEFAULT 0 CHECK(withdrawal_revision = 0 OR withdrawal_revision > revision)
) STRICT;
CREATE TRIGGER physical_epoch_monotonic BEFORE UPDATE ON physical_domains
WHEN NEW.domain_id != OLD.domain_id OR NEW.resource_key != OLD.resource_key OR NEW.epoch <= OLD.epoch
BEGIN SELECT RAISE(ABORT, 'physical domain identity/epoch regression'); END;
CREATE TRIGGER physical_environment_monotonic BEFORE UPDATE ON physical_environments
WHEN OLD.retired = 1 OR NEW.environment_id != OLD.environment_id OR NEW.host_ref != OLD.host_ref
 OR NEW.revision < OLD.revision OR (NEW.retired = 0 AND NEW.revision != OLD.revision + 1)
BEGIN SELECT RAISE(ABORT, 'physical environment regression'); END;
CREATE TRIGGER physical_qualification_monotonic BEFORE UPDATE ON physical_qualifications
WHEN NEW.withdrawal_revision <= OLD.withdrawal_revision OR NEW.record_json != OLD.record_json
 OR NEW.record_digest != OLD.record_digest OR NEW.qualification_id != OLD.qualification_id
BEGIN SELECT RAISE(ABORT, 'physical qualification regression'); END;
CREATE TRIGGER physical_alias_immutable BEFORE UPDATE ON physical_aliases
BEGIN SELECT RAISE(ABORT, 'physical alias reassignment'); END;
CREATE TRIGGER physical_domains_keep BEFORE DELETE ON physical_domains
BEGIN SELECT RAISE(ABORT, 'physical domain tombstone required'); END;
CREATE TRIGGER physical_aliases_keep BEFORE DELETE ON physical_aliases
BEGIN SELECT RAISE(ABORT, 'physical alias tombstone required'); END;
CREATE TRIGGER physical_environments_keep BEFORE DELETE ON physical_environments
BEGIN SELECT RAISE(ABORT, 'physical environment tombstone required'); END;
CREATE TRIGGER physical_qualifications_keep BEFORE DELETE ON physical_qualifications
BEGIN SELECT RAISE(ABORT, 'physical qualification tombstone required'); END;
CREATE TRIGGER physical_environment_domains_keep BEFORE DELETE ON physical_environment_domains
BEGIN SELECT RAISE(ABORT, 'physical domain membership required'); END;
"#;

/// Physical ledger *content* format, independent of the DDL stage versions.
/// Bump it whenever a persisted record body (scope, intent, evidence or
/// qualification JSON) changes incompatibly. Record bodies are never migrated:
/// a ledger holding rows in an older format fails closed until reset, which
/// keeps only the identity/epoch/environment tables in RETAINED_TABLES.
/// The marker table is orthogonal to the DDL stages: stage recognition ignores
/// it, stage rebuilds leave it untouched and it is verified on its own.
pub(super) const LEDGER_FORMAT: i64 = 5;
pub(super) const LEDGER_META_TABLE: &str = "physical_ledger_meta";
const LEDGER_META: &str = "CREATE TABLE physical_ledger_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format_version INTEGER NOT NULL CHECK(format_version>=1)) STRICT;";
// Per-stage `*_schema` singletons are version markers, not content.
const RETAINED_TABLES: [&str; 6] = [
    "physical_schema",
    "physical_ledger_meta",
    "physical_domains",
    "physical_aliases",
    "physical_environments",
    "physical_environment_domains",
];
fn ledger_meta_objects(conn: &Connection) -> AppResult<Vec<(String, Option<String>)>> {
    Ok(conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE tbl_name=?1 ORDER BY name")?
        .query_map([LEDGER_META_TABLE], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?)
}
fn ledger_format(conn: &Connection) -> AppResult<Option<i64>> {
    if ledger_meta_objects(conn)?.is_empty() {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT format_version FROM physical_ledger_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}
/// Runs before any stage audit decodes a record body. Detection is by format
/// marker and row presence only; Core carries no decoder for older formats.
fn ledger_format_gate(conn: &Connection) -> AppResult<()> {
    let found = ledger_format(conn)?;
    if found == Some(LEDGER_FORMAT) {
        return Ok(());
    }
    require(
        found.is_none_or(|v| v < LEDGER_FORMAT),
        "Unknown newer physical ledger format",
    )?;
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'physical_*' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for table in tables
        .iter()
        .filter(|t| !RETAINED_TABLES.contains(&t.as_str()) && !t.ends_with("_schema"))
    {
        let rows: i64 =
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        if rows > 0 {
            return Err(AppError::InvalidInput(format!(
                "legacy physical ledger (older format); reset required: format {} holds rows in {table}, \
                 expected format {LEDGER_FORMAT} (see docs/development.md)",
                found.map_or("none".to_owned(), |v| v.to_string())
            )));
        }
    }
    Ok(())
}
/// Only reachable after the gate proved no content rows exist in an older format.
fn stamp_ledger_format(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "INSERT INTO physical_ledger_meta(singleton,format_version) VALUES(1,?1) \
         ON CONFLICT(singleton) DO UPDATE SET format_version=excluded.format_version",
        [LEDGER_FORMAT],
    )?;
    Ok(())
}

fn text<T: Clone + Into<String>>(value: &T) -> String {
    value.clone().into()
}
fn tag(value: &impl Serialize) -> AppResult<String> {
    Ok(serde_json::from_str(&serde_json::to_string(value)?)?)
}
fn checked_integer(value: u64) -> AppResult<i64> {
    require(value <= i64::MAX as u64, "Physical ledger counter overflow")?;
    Ok(value as i64)
}
fn decode<T: DeserializeOwned + Serialize>(raw: &str) -> AppResult<T> {
    let value: T = serde_json::from_str(raw)?;
    require(
        serde_json::to_string(&value)? == raw,
        "Noncanonical physical record",
    )?;
    Ok(value)
}

/// Path-only module-local store. Opening it does not restore any live binding.
pub(super) struct PhysicalStoreV1 {
    path: PathBuf,
}

/// Called only through the existing database startup owner. A partial or unknown
/// schema is rejected, never repaired by CREATE IF NOT EXISTS/default counters.
pub(crate) fn initialize(paths: &AppPaths) -> AppResult<()> {
    let mut conn = configured_connection(&paths.db_path)?;
    // Schema migration only: suspend FK checks outside the transaction, then
    // verify every reference before committing and restore enforcement.
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name GLOB 'physical_*'",
        [],
        |r| r.get(0),
    )?;
    if count == 0 {
        tx.execute_batch(SCHEMA)?;
    }
    ledger_format_gate(&tx)?;
    if ledger_meta_objects(&tx)?.is_empty() {
        tx.execute_batch(LEDGER_META)?;
    }
    // Only a complete, exactly recognized Stage 2 schema may gain the Core extension.
    let base = Connection::open_in_memory()?;
    base.execute_batch(SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&base)? {
        verify_base_version(&tx)?;
        audit_facts(&tx)?;
        tx.execute_batch(core_ledger::SCHEMA)?;
    }
    let stage3 = Connection::open_in_memory()?;
    stage3.execute_batch(SCHEMA)?;
    stage3.execute_batch(core_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage3)? {
        verify_base_version(&tx)?;
        core_ledger::verify_version(&tx)?;
        audit_facts(&tx)?;
        core_ledger::audit(&tx)?;
        tx.execute_batch(control_ledger::SCHEMA)?;
    }
    let stage4 = Connection::open_in_memory()?;
    stage4.execute_batch(SCHEMA)?;
    stage4.execute_batch(core_ledger::SCHEMA)?;
    stage4.execute_batch(control_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage4)? {
        audit_facts(&tx)?;
        core_ledger::audit(&tx)?;
        control_ledger::audit(&tx)?;
        tx.execute_batch(evidence_ledger::SCHEMA)?;
    }
    let stage6 = Connection::open_in_memory()?;
    stage6.execute_batch(SCHEMA)?;
    stage6.execute_batch(core_ledger::SCHEMA)?;
    stage6.execute_batch(control_ledger::SCHEMA)?;
    stage6.execute_batch(evidence_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage6)? {
        audit_facts(&tx)?;
        core_ledger::audit(&tx)?;
        control_ledger::audit(&tx)?;
        evidence_ledger::audit(&tx)?;
        remote_ledger::migrate(&tx)?;
    }
    let stage7 = Connection::open_in_memory()?;
    stage7.execute_batch(&remote_ledger::stage7_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage7)? {
        audit_facts(&tx)?;
        core_ledger::audit(&tx)?;
        control_ledger::audit(&tx)?;
        evidence_ledger::audit(&tx)?;
        remote_ledger::audit(&tx)?;
        remote_ledger::migrate(&tx)?;
    }
    let stage8 = Connection::open_in_memory()?;
    stage8.execute_batch(&remote_ledger::stage8_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage8)? {
        audit_facts(&tx)?;
        core_ledger::audit(&tx)?;
        control_ledger::audit(&tx)?;
        evidence_ledger::audit(&tx)?;
        remote_ledger::audit(&tx)?;
        native_ledger::audit(&tx)?;
        tx.execute_batch(qualification_ledger::SCHEMA)?;
    }
    // Additive: handover verdicts. The format gate admits no older-format
    // handovers, so there is no history to backfill.
    let stage9 = Connection::open_in_memory()?;
    stage9.execute_batch(&remote_ledger::stage9_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage9)? {
        tx.execute_batch(evidence_ledger::HANDOVER_VERDICT_SCHEMA)?;
    }
    // Decision streams: several actions per root and the decision record. The
    // relaxed constraints admit every existing row, which the rebuild copies.
    let stage9_full = Connection::open_in_memory()?;
    stage9_full.execute_batch(&remote_ledger::stage9_full_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage9_full)? {
        remote_ledger::rebuild(&tx, &remote_ledger::current_ddl())?;
    }
    verify_schema(&tx)?;
    stamp_ledger_format(&tx)?;
    audit(&tx)?;
    let broken: bool = tx.prepare("PRAGMA foreign_key_check")?.exists([])?;
    require(!broken, "Physical migration foreign key mismatch")?;
    tx.commit()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(())
}

fn configured_connection(path: &std::path::Path) -> AppResult<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    // Connection-local only. Do not change persistent journal_mode for shared DB.
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=EXTRA; PRAGMA fullfsync=ON;")?;
    verify_durability(&conn)?;
    Ok(conn)
}
fn verify_durability(conn: &Connection) -> AppResult<()> {
    let foreign_keys: i64 = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
    let fullfsync: i64 = conn.query_row("PRAGMA fullfsync", [], |r| r.get(0))?;
    let journal: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    require(
        foreign_keys == 1
            && sync == 3
            && fullfsync == 1
            && matches!(journal.as_str(), "delete" | "truncate" | "persist" | "wal"),
        "Unsupported physical SQLite durability settings",
    )
}
fn schema_objects(conn: &Connection) -> AppResult<Vec<(String, Option<String>)>> {
    Ok(conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE (name GLOB 'physical_*' OR tbl_name GLOB 'physical_*') AND tbl_name!=?1 ORDER BY name")?
        .query_map([LEDGER_META_TABLE], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?)
}
fn expected_schema() -> AppResult<&'static Vec<(String, Option<String>)>> {
    // Cache only the immutable compiled DDL template, never database facts or
    // validation results. Each connection still reads and compares its own schema.
    static EXPECTED: std::sync::OnceLock<Vec<(String, Option<String>)>> =
        std::sync::OnceLock::new();
    if EXPECTED.get().is_none() {
        let expected = Connection::open_in_memory()?;
        expected.execute_batch(&remote_ledger::current_ddl())?;
        let _ = EXPECTED.set(schema_objects(&expected)?);
    }
    Ok(EXPECTED.get().expect("compiled schema initialized"))
}
fn expected_ledger_meta() -> AppResult<&'static Vec<(String, Option<String>)>> {
    static EXPECTED: std::sync::OnceLock<Vec<(String, Option<String>)>> =
        std::sync::OnceLock::new();
    if EXPECTED.get().is_none() {
        let expected = Connection::open_in_memory()?;
        expected.execute_batch(LEDGER_META)?;
        let _ = EXPECTED.set(ledger_meta_objects(&expected)?);
    }
    Ok(EXPECTED.get().expect("compiled ledger marker initialized"))
}
fn verify_schema(conn: &Connection) -> AppResult<()> {
    require(
        &schema_objects(conn)? == expected_schema()?,
        "Incompatible physical ledger schema",
    )?;
    require(
        &ledger_meta_objects(conn)? == expected_ledger_meta()?,
        "Incompatible physical ledger format marker",
    )?;
    control_ledger::verify_version(conn)?;
    core_ledger::verify_version(conn)?;
    verify_base_version(conn)
}
fn verify_base_version(conn: &Connection) -> AppResult<()> {
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM physical_schema WHERE singleton=1")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    require(
        versions == [1],
        "Missing/incompatible physical ledger version",
    )
}

impl PhysicalStoreV1 {
    pub(super) fn open(paths: &AppPaths) -> AppResult<Self> {
        let store = Self {
            path: paths.db_path.clone(),
        };
        let mut conn = store.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        tx.commit()?;
        Ok(store)
    }
    pub(in crate::physical) fn connection(&self) -> AppResult<Connection> {
        let conn = configured_connection(&self.path)?;
        verify_schema(&conn)?;
        Ok(conn)
    }
    // All reads used for trust inspect a coherent snapshot, including indexed
    // identity/body correlation. Stored JSON alone is never accepted as truth.
    pub(super) fn registration(
        &self,
        id: &EnvironmentRefV1,
    ) -> AppResult<EnvironmentRegistrationV1> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        audit(&tx)?;
        let value = load_registration(&tx, id, true)?;
        tx.commit()?;
        Ok(value)
    }
    pub(super) fn enroll(
        &self,
        record: &EnvironmentRegistrationV1,
        expected_revision: Option<u64>,
    ) -> AppResult<()> {
        record.validate()?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        match expected_revision {
            None => {
                require(
                    record.revision == 1,
                    "Initial enrollment revision must be one",
                )?;
                tx.execute("INSERT INTO physical_environments(environment_id,host_ref,revision,configuration_ref,configuration_digest,registration_digest,record_json) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![text(&record.environment),record.host.as_str(),checked_integer(record.revision)?,text(&record.configuration_ref),text(&record.configuration_digest),text(&record.digest()?),serde_json::to_string(record)?])?;
            }
            Some(expected) => {
                let old = load_registration(&tx, &record.environment, true)?;
                require(
                    old.revision == expected
                        && record.revision == expected.checked_add(1).unwrap_or(0)
                        && old.host == record.host
                        && old.resources == record.resources
                        && old.aliases == record.aliases,
                    "Enrollment revision/Host/domain ownership mismatch",
                )?;
                bump_environment_domains(&tx, &record.environment)?;
                withdraw_environment(&tx, &record.environment)?;
                let n = tx.execute("UPDATE physical_environments SET revision=?2, configuration_ref=?3, configuration_digest=?4,registration_digest=?5,record_json=?6 WHERE environment_id=?1 AND retired=0 AND revision=?7", params![text(&record.environment),checked_integer(record.revision)?,text(&record.configuration_ref),text(&record.configuration_digest),text(&record.digest()?),serde_json::to_string(record)?,checked_integer(expected)?])?;
                require(n == 1, "Enrollment changed concurrently")?;
            }
        }
        for (domain, resource) in &record.resources {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT resource_key FROM physical_domains WHERE domain_id=?1",
                    [text(domain)],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(existing) => require(
                    existing == text(resource),
                    "Canonical domain identity mismatch",
                )?,
                None => {
                    tx.execute(
                        "INSERT INTO physical_domains VALUES (?1,?2,1,1)",
                        params![text(domain), text(resource)],
                    )?;
                }
            }
            tx.execute(
                "INSERT INTO physical_environment_domains VALUES (?1,?2) ON CONFLICT DO NOTHING",
                params![text(&record.environment), text(domain)],
            )?;
        }
        for (alias, domain) in &record.aliases {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT domain_id FROM physical_aliases WHERE alias=?1",
                    [text(alias)],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(existing) => require(
                    existing == text(domain),
                    "Conflicting physical alias ownership",
                )?,
                None => {
                    tx.execute(
                        "INSERT INTO physical_aliases VALUES (?1,?2)",
                        params![text(alias), text(domain)],
                    )?;
                }
            }
        }
        audit(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn retire(&self, id: &EnvironmentRefV1, expected: u64) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        let old = load_registration(&tx, id, true)?;
        require(old.revision == expected, "Retirement revision mismatch")?;
        bump_environment_domains(&tx, id)?;
        withdraw_environment(&tx, id)?;
        tx.execute("UPDATE physical_environments SET retired=1,denial_revision=?2 WHERE environment_id=?1 AND retired=0 AND revision=?3",params![text(id),checked_integer(expected.checked_add(1).unwrap_or(u64::MAX))?,checked_integer(expected)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn epochs(
        &self,
        domains: impl Iterator<Item = DomainId>,
    ) -> AppResult<BTreeMap<DomainId, u64>> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        audit(&tx)?;
        let mut result = BTreeMap::new();
        for domain in domains {
            let epoch: i64 = tx.query_row(
                "SELECT epoch FROM physical_domains WHERE domain_id=?1",
                [text(&domain)],
                |r| r.get(0),
            )?;
            result.insert(domain, epoch as u64);
        }
        tx.commit()?;
        Ok(result)
    }
    /// Atomic CAS across canonical domains, ledger bookkeeping only. Domains
    /// remain quarantined; no holder, task, native receipt or permit is created.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "reachable only once a production binding is attached (Step D)"
        )
    )]
    pub(super) fn advance_epochs(&self, expected: &BTreeMap<DomainId, u64>) -> AppResult<()> {
        require(
            !expected.is_empty() && expected.len() <= 16,
            "Invalid domain ledger operation",
        )?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        for (domain, epoch) in expected {
            let next = checked_integer(epoch.checked_add(1).unwrap_or(u64::MAX))?;
            let n = tx.execute(
                "UPDATE physical_domains SET epoch=?2 WHERE domain_id=?1 AND epoch=?3",
                params![text(domain), next, checked_integer(*epoch)?],
            )?;
            require(n == 1, "Stale/conflicting physical domain ledger operation")?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(super) fn record_qualification(
        &self,
        environment: &EnvironmentRefV1,
        registration_digest: &DigestV1,
        q: &PhysicalQualificationV1,
        provenance: &DigestV1,
    ) -> AppResult<()> {
        self.record_qualification_inner(environment, registration_digest, q, provenance)
    }
    fn record_qualification_inner(
        &self,
        environment: &EnvironmentRefV1,
        registration_digest: &DigestV1,
        q: &PhysicalQualificationV1,
        provenance: &DigestV1,
    ) -> AppResult<()> {
        q.validate()?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        require(
            load_registration(&tx, environment, true)?.digest()? == *registration_digest,
            "Qualification enrollment mismatch",
        )?;
        // Strict insert: identities are immutable, including expiry/evidence. A
        // new qualification requires a new ID; withdrawal cannot be overwritten.
        tx.execute("INSERT INTO physical_qualifications(qualification_id,environment_id,revision,registration_digest,profile_digest,binding_digest,evidence_class,enforcement_class,evidence_digest,conditions_digest,provenance_digest,record_digest,record_json,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![text(&q.qualification_id),text(environment),checked_integer(q.revision)?,text(registration_digest),text(&q.profile_digest),text(&q.binding_digest),tag(&q.evidence_class)?,tag(&q.required_enforcement_class)?,text(&q.evidence_digest),text(&q.conditions_digest),text(provenance),text(&q.digest()?),serde_json::to_string(q)?,q.expires_at.get() as i64])?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn qualification(
        &self,
        id: &QualificationId,
        now: UnixMillis,
    ) -> AppResult<PhysicalQualificationV1> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        audit(&tx)?;
        let (raw,withdrawal,env,reg): (String,i64,String,String) = tx.query_row("SELECT record_json,withdrawal_revision,environment_id,registration_digest FROM physical_qualifications WHERE qualification_id=?1",[text(id)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let q: PhysicalQualificationV1 = decode(&raw)?;
        require(
            withdrawal == 0 && now < q.expires_at,
            "Qualification withdrawn or expired",
        )?;
        require(
            text(&load_registration(&tx, &EnvironmentRefV1::try_from(env)?, true)?.digest()?)
                == reg,
            "Qualification dependency invalidated",
        )?;
        tx.commit()?;
        Ok(q)
    }
    pub(super) fn withdraw(&self, id: &QualificationId, revision: u64) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        let n = tx.execute("UPDATE physical_qualifications SET withdrawal_revision=?2 WHERE qualification_id=?1 AND revision < ?2 AND withdrawal_revision < ?2",params![text(id),checked_integer(revision)?])?;
        require(n == 1, "Missing/stale qualification withdrawal")?;
        tx.commit()?;
        Ok(())
    }
    pub(super) fn invalidate_qualifications(
        &self,
        environment: &EnvironmentRefV1,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        audit(&tx)?;
        load_registration(&tx, environment, true)?;
        withdraw_environment(&tx, environment)?;
        tx.commit()?;
        Ok(())
    }
}
fn withdraw_environment(conn: &Connection, id: &EnvironmentRefV1) -> AppResult<()> {
    conn.execute("UPDATE physical_qualifications SET withdrawal_revision=revision+1 WHERE environment_id=?1 AND withdrawal_revision=0",[text(id)])?;
    Ok(())
}
fn bump_environment_domains(conn: &Connection, id: &EnvironmentRefV1) -> AppResult<()> {
    conn.execute("UPDATE physical_domains SET epoch=epoch+1 WHERE domain_id IN (SELECT domain_id FROM physical_environment_domains WHERE environment_id=?1)",[text(id)])?;
    Ok(())
}
fn load_registration(
    conn: &Connection,
    id: &EnvironmentRefV1,
    active: bool,
) -> AppResult<EnvironmentRegistrationV1> {
    let (raw, retired): (String, i64) = conn.query_row(
        "SELECT record_json,retired FROM physical_environments WHERE environment_id=?1",
        [text(id)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    require(!active || retired == 0, "Physical environment retired")?;
    decode(&raw)
}
fn audit(conn: &Connection) -> AppResult<()> {
    require(
        ledger_format(conn)? == Some(LEDGER_FORMAT),
        "Physical ledger format mismatch",
    )?;
    audit_facts(conn)?;
    core_ledger::audit(conn)?;
    control_ledger::audit(conn)?;
    evidence_ledger::audit(conn)?;
    remote_ledger::audit(conn)?;
    native_ledger::audit(conn)?;
    qualification_ledger::audit(conn)
}
fn audit_facts(conn: &Connection) -> AppResult<()> {
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    require(integrity == "ok", "Corrupt physical ledger")?;
    // Validate this module's relationships, not legacy rows owned elsewhere.
    for table in [
        "physical_aliases",
        "physical_environment_domains",
        "physical_qualifications",
    ] {
        require(
            !conn
                .prepare(&format!("PRAGMA foreign_key_check({table})"))?
                .exists([])?,
            "Physical ledger foreign key violation",
        )?;
    }
    let mut rows = conn.prepare("SELECT environment_id,host_ref,revision,configuration_ref,configuration_digest,registration_digest,record_json,retired,denial_revision FROM physical_environments")?;
    let mut query = rows.query([])?;
    while let Some(row) = query.next()? {
        let raw: String = row.get(6)?;
        let r: EnvironmentRegistrationV1 = decode(&raw)?;
        r.validate()?;
        require(
            row.get::<_, String>(0)? == text(&r.environment)
                && row.get::<_, String>(1)? == r.host.as_str()
                && row.get::<_, i64>(2)? == checked_integer(r.revision)?
                && row.get::<_, String>(3)? == text(&r.configuration_ref)
                && row.get::<_, String>(4)? == text(&r.configuration_digest)
                && row.get::<_, String>(5)? == text(&r.digest()?),
            "Physical registration column/body mismatch",
        )?;
        require(
            matches!((row.get::<_, i64>(7)?, row.get::<_, i64>(8)?), (0, 0))
                || (row.get::<_, i64>(7)? == 1
                    && row.get::<_, i64>(8)? > checked_integer(r.revision)?),
            "Invalid environment tombstone",
        )?;
        let actual: BTreeMap<String,String> = conn.prepare("SELECT d.domain_id,d.resource_key FROM physical_environment_domains m JOIN physical_domains d USING(domain_id) WHERE m.environment_id=?1 ORDER BY d.domain_id")?.query_map([text(&r.environment)],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<Result<_,_>>()?;
        require(
            actual
                == r.resources
                    .iter()
                    .map(|(d, k)| (text(d), text(k)))
                    .collect(),
            "Physical domain ownership mismatch",
        )?;
        for (alias, domain) in &r.aliases {
            let actual: String = conn.query_row(
                "SELECT domain_id FROM physical_aliases WHERE alias=?1",
                [text(alias)],
                |r| r.get(0),
            )?;
            require(actual == text(domain), "Physical alias ownership mismatch")?;
        }
    }
    let mut rows = conn.prepare("SELECT qualification_id,revision,profile_digest,binding_digest,evidence_class,enforcement_class,evidence_digest,conditions_digest,record_digest,record_json,expires_at,withdrawal_revision,provenance_digest,registration_digest,environment_id FROM physical_qualifications")?;
    let mut query = rows.query([])?;
    while let Some(row) = query.next()? {
        let q: PhysicalQualificationV1 = decode(&row.get::<_, String>(9)?)?;
        q.validate()?;
        require(
            row.get::<_, String>(0)? == text(&q.qualification_id)
                && row.get::<_, i64>(1)? == checked_integer(q.revision)?
                && row.get::<_, String>(2)? == text(&q.profile_digest)
                && row.get::<_, String>(3)? == text(&q.binding_digest)
                && row.get::<_, String>(4)? == tag(&q.evidence_class)?
                && row.get::<_, String>(5)? == tag(&q.required_enforcement_class)?
                && row.get::<_, String>(6)? == text(&q.evidence_digest)
                && row.get::<_, String>(7)? == text(&q.conditions_digest)
                && row.get::<_, String>(8)? == text(&q.digest()?)
                && row.get::<_, i64>(10)? == q.expires_at.get() as i64,
            "Physical qualification column/body mismatch",
        )?;
        let withdrawal: i64 = row.get(11)?;
        require(
            withdrawal == 0 || withdrawal > checked_integer(q.revision)?,
            "Invalid qualification withdrawal",
        )?;
        DigestV1::try_from(row.get::<_, String>(12)?)?;
        let reg = DigestV1::try_from(row.get::<_, String>(13)?)?;
        let env = EnvironmentRefV1::try_from(row.get::<_, String>(14)?)?;
        let current = load_registration(conn, &env, false)?;
        require(
            withdrawal != 0 || current.digest()? == reg,
            "Unwithdrawn qualification has changed enrollment",
        )?;
    }
    let mut rows = conn.prepare("SELECT alias,domain_id FROM physical_aliases")?;
    let mut query = rows.query([])?;
    while let Some(row) = query.next()? {
        LabelV1::try_from(row.get::<_, String>(0)?)?;
        DomainId::try_from(row.get::<_, String>(1)?)?;
    }
    let mut rows =
        conn.prepare("SELECT domain_id,resource_key,epoch,quarantined FROM physical_domains")?;
    let mut query = rows.query([])?;
    while let Some(row) = query.next()? {
        DomainId::try_from(row.get::<_, String>(0)?)?;
        LabelV1::try_from(row.get::<_, String>(1)?)?;
        require(
            row.get::<_, i64>(2)? >= 1 && row.get::<_, i64>(3)? == 1,
            "Unprovable physical domain continuity",
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn test_connection(paths: &AppPaths) -> AppResult<Connection> {
    configured_connection(&paths.db_path)
}

#[cfg(test)]
pub(super) fn test_verify_durability(conn: &Connection) -> AppResult<()> {
    verify_durability(conn)
}

#[cfg(test)]
pub(super) fn test_stage4_reservation_schema() -> AppResult<Vec<String>> {
    let c = Connection::open_in_memory()?;
    c.execute_batch(SCHEMA)?;
    c.execute_batch(core_ledger::SCHEMA)?;
    c.execute_batch(control_ledger::SCHEMA)?;
    let mut stmt=c.prepare("SELECT sql FROM sqlite_master WHERE name IN ('physical_domain_reservations','physical_reservation_monotonic','physical_reservations_keep') ORDER BY CASE WHEN type='table' THEN 0 ELSE 1 END,name")?;
    let rows = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

#[cfg(test)]
pub(super) fn test_restore_stage6_schema(paths: &AppPaths) -> AppResult<()> {
    let mut c = configured_connection(&paths.db_path)?;
    c.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ddl = [
        SCHEMA,
        core_ledger::SCHEMA,
        control_ledger::SCHEMA,
        evidence_ledger::SCHEMA,
    ]
    .join("\n");
    // Earlier stages had no decision records.
    tx.execute_batch("DROP TABLE physical_decisions")?;
    remote_ledger::rebuild(&tx, &ddl)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
pub(super) fn test_restore_stage7_schema(paths: &AppPaths) -> AppResult<()> {
    let mut c = configured_connection(&paths.db_path)?;
    c.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("DROP TABLE physical_decisions")?;
    remote_ledger::rebuild(&tx, &remote_ledger::stage7_ddl())?;
    tx.commit()?;
    Ok(())
}
