//! Host-private durable facts. Rows and epoch operations never convey permission.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

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
    deadline_reached, ActionAuditV1, ActionDispositionV1, CallbackRecordV1, DecisionRecordV1,
    FenceAuditV1, ReservationReceiptV1, SessionAuditV1, WriteCallbackV1,
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
pub(super) const LEDGER_FORMAT: i64 = 6;
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
    ledger: Arc<LedgerV1>,
}

/// One ledger file as this process uses it: every physical store on the file
/// shares this one connection, whose hooks keep the write journal.
///
/// Audit model. The first transaction audits the whole ledger (startup).
/// From then on the ledger is trusted at the `PRAGMA data_version` it was
/// audited at; that version changes only when another connection or process
/// commits, and then the next transaction audits the whole ledger again
/// (fail-closed). This connection's own transactions validate, before they
/// commit, exactly the rows they wrote and the groups those rows belong to
/// (a Root with its sessions, actions, decisions, budgets and evidence; a
/// review; a domain), trusting unchanged audited rows by their stored
/// digests. A commit whose writes were not validated is refused by the
/// commit hook.
struct LedgerV1 {
    path: PathBuf,
    /// The file the connection opened. A deleted or replaced file is never
    /// read through the old connection.
    file: FileIdentityV1,
    /// The files as this connection last left them.
    stamp: parking_lot::Mutex<FileStampV1>,
    connection: parking_lot::Mutex<Connection>,
    /// The thread holding `connection`: re-entering it from the same thread
    /// is a bug and fails at once instead of waiting forever.
    owner: parking_lot::Mutex<Option<std::thread::ThreadId>>,
    journal: Arc<parking_lot::Mutex<JournalV1>>,
}
/// The ledger connection, held for one transaction or query.
pub(in crate::physical) struct LedgerGuardV1<'a> {
    connection: parking_lot::MutexGuard<'a, Connection>,
    owner: &'a parking_lot::Mutex<Option<std::thread::ThreadId>>,
}
impl Drop for LedgerGuardV1<'_> {
    fn drop(&mut self) {
        *self.owner.lock() = None;
    }
}
impl std::ops::Deref for LedgerGuardV1<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}
impl std::ops::DerefMut for LedgerGuardV1<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}
#[derive(Default)]
struct JournalV1 {
    /// Rows the open transaction inserted or updated, by table and rowid.
    rows: Vec<(String, i64)>,
    deleted: bool,
    /// The data version of the last fully audited or validated state.
    trusted: Option<i64>,
    /// The evidence and consequence history validated at `trusted`. It exists
    /// only together with `trusted` and is dropped with it.
    history: Option<Arc<HistoryBaselineV1>>,
    /// Tests: compare every bounded group validation with the full one.
    #[cfg(test)]
    no_oracle: bool,
}
impl JournalV1 {
    fn pending(&self) -> bool {
        !self.rows.is_empty() || self.deleted
    }
    /// Forgets the trusted state: the next transaction audits in full.
    fn distrust(&mut self) {
        self.trusted = None;
        self.history = None;
    }
}

/// Bounded Root-group validation (docs/physical.md, "Ledger validation").
///
/// Evidence and consequence rows are validated once, when they are appended:
/// by the full audit, or by `validate_appended` at the head of their action
/// under a trusted snapshot. Triggers refuse every later change to them, and
/// every input their checks read (the action, attempt and review rows that
/// derive their lineage) is immutable too; producer qualifications live in
/// ungrouped tables whose writes audit in full. A change by any other
/// connection or process moves `PRAGMA data_version` (or the file stamp) and
/// makes the next transaction audit in full. So a write that changes no
/// history row does not need to replay history that is still exactly the
/// validated history. That history is recorded here: for each Root, each
/// action's evidence and consequence heads at the trusted data version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct HistoryBaselineV1 {
    /// The data version at which these heads were validated; equal to the
    /// journal's `trusted` while the baseline holds.
    version: i64,
    roots: BTreeMap<String, BTreeMap<String, HistoryHeadV1>>,
}
/// One action's highest evidence and consequence revisions (0 for none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct HistoryHeadV1 {
    evidence: i64,
    consequences: i64,
}
impl HistoryBaselineV1 {
    /// Adopts newly validated heads of the listed actions.
    fn learn(&mut self, heads: BTreeMap<String, BTreeMap<String, HistoryHeadV1>>) {
        for (root, actions) in heads {
            self.roots.entry(root).or_default().extend(actions);
        }
    }
    /// Whether every listed action's history is exactly validated history:
    /// none at all, or the heads this baseline holds for it.
    fn covers(&self, heads: &BTreeMap<String, BTreeMap<String, HistoryHeadV1>>) -> bool {
        heads.iter().all(|(root, actions)| {
            actions.iter().all(|(action, head)| {
                *head == HistoryHeadV1::default()
                    || self.roots.get(root).and_then(|r| r.get(action)) == Some(head)
            })
        })
    }
}
/// The current history heads of every action, or of the actions in the
/// audit scope, by Root. Indexed lookups only: (action_id, revision) is
/// unique in both history tables.
fn history_heads(
    c: &Connection,
    scope: AuditScopeV1,
) -> AppResult<BTreeMap<String, BTreeMap<String, HistoryHeadV1>>> {
    let mut stmt = c.prepare(&format!(
        "SELECT a.root_id,a.action_id,
         COALESCE((SELECT max(revision) FROM physical_evidence e WHERE e.action_id=a.action_id),0),
         COALESCE((SELECT max(revision) FROM physical_consequences q WHERE q.action_id=a.action_id),0)
         FROM physical_actions a WHERE {}",
        scope.filter("a.action_id", "action")
    ))?;
    let mut heads = BTreeMap::<String, BTreeMap<String, HistoryHeadV1>>::new();
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        heads.entry(r.get(0)?).or_default().insert(
            r.get(1)?,
            HistoryHeadV1 {
                evidence: r.get(2)?,
                consequences: r.get(3)?,
            },
        );
    }
    Ok(heads)
}

/// What one write may touch. Every store write declares its kind; a write may
/// skip replaying history only if its kind declares no history table and it
/// wrote no history row. Debug builds refuse a declared write that touches a
/// table its kind does not declare.
pub(super) struct WriteKindV1 {
    pub(super) name: &'static str,
    pub(super) tables: &'static [&'static str],
}
impl WriteKindV1 {
    /// Declared, and declares no evidence or consequence table.
    fn preserves_history(&self) -> bool {
        !self.tables.is_empty()
            && !self
                .tables
                .iter()
                .any(|t| evidence_ledger::APPEND_ONLY.contains(t))
    }
    #[cfg(debug_assertions)]
    fn check(&self, rows: &[(String, i64)]) -> AppResult<()> {
        match rows
            .iter()
            .find(|(t, _)| !self.tables.contains(&t.as_str()))
        {
            Some((table, _)) => Err(AppError::InvalidInput(format!(
                "Undeclared physical ledger write: {table} in {}",
                self.name
            ))),
            None => Ok(()),
        }
    }
}
/// A transaction that declares nothing (one that only reads, or a test's own
/// write): its writes, if any, take the full validation and never skip any.
const UNDECLARED: WriteKindV1 = WriteKindV1 {
    name: "undeclared",
    tables: &[],
};
/// The tables each write kind touches (see `WriteKindV1`). A Root's closure
/// (`control_ledger::close_root`) writes actions, budgets, reservations,
/// domains and sessions.
mod kinds {
    use super::WriteKindV1;
    pub(super) const ACKNOWLEDGE_FENCE: WriteKindV1 = WriteKindV1 {
        name: "acknowledge_fence",
        tables: &["physical_sessions"],
    };
    pub(super) const ACTIVATE_SESSION: WriteKindV1 = WriteKindV1 {
        name: "activate_session",
        tables: &["physical_sessions"],
    };
    pub(super) const ADMIT_ACTION: WriteKindV1 = WriteKindV1 {
        name: "admit_action",
        tables: &[
            "physical_actions",
            "physical_control_budgets",
            "physical_decisions",
        ],
    };
    pub(super) const ADVANCE_EPOCHS: WriteKindV1 = WriteKindV1 {
        name: "advance_epochs",
        tables: &["physical_domains"],
    };
    pub(super) const CLAIM_SEMANTIC: WriteKindV1 = WriteKindV1 {
        name: "claim_semantic",
        tables: &["physical_semantic_messages"],
    };
    pub(super) const CLOSE_ATTEMPT: WriteKindV1 = WriteKindV1 {
        name: "close_attempt",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const CLOSE_ENVIRONMENT_ATTEMPTS: WriteKindV1 = WriteKindV1 {
        name: "close_environment_attempts",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const CLOSE_OPEN_ATTEMPTS: WriteKindV1 = WriteKindV1 {
        name: "close_open_attempts",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const COMMIT_ACCEPTANCE: WriteKindV1 = WriteKindV1 {
        name: "commit_acceptance",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const CONFIGURE_HANDOVER: WriteKindV1 = WriteKindV1 {
        name: "configure_handover",
        tables: &["physical_handover_policies"],
    };
    pub(super) const CREATE_REVIEW: WriteKindV1 = WriteKindV1 {
        name: "create_review",
        tables: &["physical_reviews"],
    };
    pub(super) const ENROLL: WriteKindV1 = WriteKindV1 {
        name: "enroll",
        tables: &[
            "physical_aliases",
            "physical_domains",
            "physical_environment_domains",
            "physical_environments",
            "physical_qualifications",
        ],
    };
    pub(super) const EVALUATE_CONSEQUENCE: WriteKindV1 = WriteKindV1 {
        name: "evaluate_consequence",
        tables: &["physical_consequences"],
    };
    pub(super) const FINISH_WRITE: WriteKindV1 = WriteKindV1 {
        name: "finish_write",
        tables: &["physical_actions", "physical_write_callbacks"],
    };
    pub(super) const IMPORT_APPROVED_REVIEW: WriteKindV1 = WriteKindV1 {
        name: "import_approved_review",
        tables: &["physical_reviews"],
    };
    pub(super) const INVALIDATE_QUALIFICATIONS: WriteKindV1 = WriteKindV1 {
        name: "invalidate_qualifications",
        tables: &["physical_qualifications"],
    };
    pub(super) const ORIGINATE_ATTEMPT: WriteKindV1 = WriteKindV1 {
        name: "originate_attempt",
        tables: &[
            "physical_attempts",
            "physical_reviews",
            "physical_task_acceptance",
        ],
    };
    pub(super) const PREPARE_WRITE: WriteKindV1 = WriteKindV1 {
        name: "prepare_write",
        tables: &["physical_actions", "physical_control_budgets"],
    };
    pub(super) const RECONCILE: WriteKindV1 = WriteKindV1 {
        name: "reconcile",
        tables: &[
            "physical_domain_reservations",
            "physical_handover_verdicts",
            "physical_handovers",
            "physical_reconciliations",
        ],
    };
    pub(super) const RECORD_DISPOSITION: WriteKindV1 = WriteKindV1 {
        name: "record_disposition",
        tables: &["physical_evidence"],
    };
    pub(super) const RECORD_EFFECT_VIOLATION: WriteKindV1 = WriteKindV1 {
        name: "record_effect_violation",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_effect_bound_violations",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const RECORD_OBSERVATION: WriteKindV1 = WriteKindV1 {
        name: "record_observation",
        tables: &["physical_evidence"],
    };
    pub(super) const RECORD_QUALIFICATION: WriteKindV1 = WriteKindV1 {
        name: "record_qualification_inner",
        tables: &["physical_qualifications"],
    };
    pub(super) const RECORD_REFUSAL: WriteKindV1 = WriteKindV1 {
        name: "record_refusal",
        tables: &["physical_decisions"],
    };
    pub(super) const RESERVE_SESSION: WriteKindV1 = WriteKindV1 {
        name: "reserve_session",
        tables: &[
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_sessions",
        ],
    };
    pub(super) const RETIRE: WriteKindV1 = WriteKindV1 {
        name: "retire",
        tables: &[
            "physical_domains",
            "physical_environments",
            "physical_qualifications",
        ],
    };
    pub(super) const REVISE_REVIEW: WriteKindV1 = WriteKindV1 {
        name: "revise_review",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_reviews",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const SAVE_REMOTE_OFFERS: WriteKindV1 = WriteKindV1 {
        name: "save_remote_offers",
        tables: &["physical_remote_offers"],
    };
    pub(super) const SAVE_REMOTE_REVIEW: WriteKindV1 = WriteKindV1 {
        name: "save_remote_review",
        tables: &["physical_remote_reviews"],
    };
    pub(super) const SAVE_SEMANTIC_RESULT: WriteKindV1 = WriteKindV1 {
        name: "save_semantic_result",
        tables: &["physical_semantic_messages"],
    };
    pub(super) const TRANSITION_REVIEW: WriteKindV1 = WriteKindV1 {
        name: "transition_review",
        tables: &[
            "physical_actions",
            "physical_attempts",
            "physical_control_budgets",
            "physical_domain_reservations",
            "physical_domains",
            "physical_reviews",
            "physical_sessions",
            "physical_task_acceptance",
        ],
    };
    pub(super) const WITHDRAW_QUALIFICATION: WriteKindV1 = WriteKindV1 {
        name: "withdraw_qualification",
        tables: &["physical_qualifications"],
    };
    /// Tests: a write of evidence together with a Root's budget row (a
    /// history change that is not a bounded append).
    #[cfg(test)]
    pub(super) const TEST_HISTORY_AND_BUDGET: WriteKindV1 = WriteKindV1 {
        name: "test_history_and_budget",
        tables: &["physical_control_budgets", "physical_evidence"],
    };
    #[cfg(test)]
    pub(super) const ALL: &[&WriteKindV1] = &[
        &ACKNOWLEDGE_FENCE,
        &ACTIVATE_SESSION,
        &ADMIT_ACTION,
        &ADVANCE_EPOCHS,
        &CLAIM_SEMANTIC,
        &CLOSE_ATTEMPT,
        &CLOSE_ENVIRONMENT_ATTEMPTS,
        &CLOSE_OPEN_ATTEMPTS,
        &COMMIT_ACCEPTANCE,
        &CONFIGURE_HANDOVER,
        &CREATE_REVIEW,
        &ENROLL,
        &EVALUATE_CONSEQUENCE,
        &FINISH_WRITE,
        &IMPORT_APPROVED_REVIEW,
        &INVALIDATE_QUALIFICATIONS,
        &ORIGINATE_ATTEMPT,
        &PREPARE_WRITE,
        &RECONCILE,
        &RECORD_DISPOSITION,
        &RECORD_EFFECT_VIOLATION,
        &RECORD_OBSERVATION,
        &RECORD_QUALIFICATION,
        &RECORD_REFUSAL,
        &RESERVE_SESSION,
        &RETIRE,
        &REVISE_REVIEW,
        &SAVE_REMOTE_OFFERS,
        &SAVE_REMOTE_REVIEW,
        &SAVE_SEMANTIC_RESULT,
        &TRANSITION_REVIEW,
        &WITHDRAW_QUALIFICATION,
        &TEST_HISTORY_AND_BUDGET,
    ];
}
/// What the Root-group validation of one commit adopts once it commits.
enum LearnedV1 {
    Nothing,
    /// Everything was audited in full: the whole baseline.
    All(BTreeMap<String, BTreeMap<String, HistoryHeadV1>>),
    /// These Roots' history was validated or confirmed unchanged.
    Roots(BTreeMap<String, BTreeMap<String, HistoryHeadV1>>),
}
/// Which file a path names: device and inode where the platform has them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentityV1(u64, u64);
impl FileIdentityV1 {
    fn of(path: &std::path::Path) -> AppResult<Self> {
        let m = std::fs::metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self(m.dev(), m.ino()))
        }
        #[cfg(not(unix))]
        {
            let created = m
                .created()?
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| AppError::InvalidInput("Physical ledger file time".into()))?;
            Ok(Self(created.as_secs(), u64::from(created.subsec_nanos())))
        }
    }
}
type LedgersV1 = parking_lot::Mutex<BTreeMap<PathBuf, std::sync::Weak<LedgerV1>>>;
/// Ledgers open in this process, by file.
fn ledgers() -> &'static LedgersV1 {
    static LEDGERS: std::sync::OnceLock<LedgersV1> = std::sync::OnceLock::new();
    LEDGERS.get_or_init(Default::default)
}
/// When the ledger's files last changed: the database, its WAL and its
/// rollback journal. A change this connection did not make is either another
/// connection's commit or an edit that bypassed SQLite; both reopen the
/// connection (no cached page survives) and audit the whole ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStampV1(Vec<Option<(std::time::SystemTime, u64)>>);
impl FileStampV1 {
    fn of(path: &std::path::Path) -> AppResult<Self> {
        let mut stamps = Vec::new();
        for suffix in ["", "-wal", "-journal"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            stamps.push(match std::fs::metadata(std::path::PathBuf::from(name)) {
                Ok(m) => Some((m.modified()?, m.len())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            });
        }
        Ok(Self(stamps))
    }
}
impl LedgerV1 {
    fn open(path: &std::path::Path) -> AppResult<Arc<Self>> {
        let mut open = ledgers().lock();
        if let Some(ledger) = open.get(path).and_then(|l| l.upgrade()) {
            if ledger.current().is_ok() {
                return Ok(ledger);
            }
        }
        let journal = Arc::new(parking_lot::Mutex::new(JournalV1::default()));
        let connection = connect(path, &journal)?;
        let ledger = Arc::new(Self {
            path: path.to_owned(),
            file: FileIdentityV1::of(path)?,
            stamp: parking_lot::Mutex::new(FileStampV1::of(path)?),
            connection: parking_lot::Mutex::new(connection),
            owner: parking_lot::Mutex::new(None),
            journal,
        });
        open.retain(|_, l| l.strong_count() > 0);
        open.insert(path.to_owned(), Arc::downgrade(&ledger));
        Ok(ledger)
    }
    /// The path still names the file this connection opened.
    fn current(&self) -> AppResult<()> {
        require(
            FileIdentityV1::of(&self.path).is_ok_and(|f| f == self.file),
            "Physical ledger file missing or replaced",
        )
    }
}
/// The ledger connection with its journal hooks and scope table.
fn connect(
    path: &std::path::Path,
    journal: &Arc<parking_lot::Mutex<JournalV1>>,
) -> AppResult<Connection> {
    let connection = configured_connection(path)?;
    connection.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS physical_audit_scope(kind TEXT NOT NULL,key TEXT NOT NULL,PRIMARY KEY(kind,key))",
    )?;
    let written = journal.clone();
    connection.update_hook(Some(
        move |action: rusqlite::hooks::Action, db: &str, table: &str, rowid: i64| {
            if db != "main" {
                return;
            }
            let mut j = written.lock();
            match action {
                rusqlite::hooks::Action::SQLITE_INSERT | rusqlite::hooks::Action::SQLITE_UPDATE => {
                    j.rows.push((table.into(), rowid))
                }
                _ => j.deleted = true,
            }
        },
    ));
    // A commit that wrote without passing validation becomes a rollback;
    // `commit` drains the journal just before committing.
    let committing = journal.clone();
    connection.commit_hook(Some(move || committing.lock().pending()));
    let rolled_back = journal.clone();
    connection.rollback_hook(Some(move || {
        let mut j = rolled_back.lock();
        j.rows.clear();
        j.deleted = false;
    }));
    Ok(connection)
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
        audit_facts(&tx, AuditScopeV1::Full)?;
        tx.execute_batch(core_ledger::SCHEMA)?;
    }
    let stage3 = Connection::open_in_memory()?;
    stage3.execute_batch(SCHEMA)?;
    stage3.execute_batch(core_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage3)? {
        verify_base_version(&tx)?;
        core_ledger::verify_version(&tx)?;
        audit_facts(&tx, AuditScopeV1::Full)?;
        core_ledger::audit(&tx, AuditScopeV1::Full)?;
        tx.execute_batch(control_ledger::SCHEMA)?;
    }
    let stage4 = Connection::open_in_memory()?;
    stage4.execute_batch(SCHEMA)?;
    stage4.execute_batch(core_ledger::SCHEMA)?;
    stage4.execute_batch(control_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage4)? {
        audit_facts(&tx, AuditScopeV1::Full)?;
        core_ledger::audit(&tx, AuditScopeV1::Full)?;
        control_ledger::audit(&tx, AuditScopeV1::Full)?;
        tx.execute_batch(evidence_ledger::SCHEMA)?;
    }
    let stage6 = Connection::open_in_memory()?;
    stage6.execute_batch(SCHEMA)?;
    stage6.execute_batch(core_ledger::SCHEMA)?;
    stage6.execute_batch(control_ledger::SCHEMA)?;
    stage6.execute_batch(evidence_ledger::SCHEMA)?;
    if schema_objects(&tx)? == schema_objects(&stage6)? {
        audit_facts(&tx, AuditScopeV1::Full)?;
        core_ledger::audit(&tx, AuditScopeV1::Full)?;
        control_ledger::audit(&tx, AuditScopeV1::Full)?;
        evidence_ledger::audit(&tx, AuditScopeV1::Full)?;
        remote_ledger::migrate(&tx)?;
    }
    let stage7 = Connection::open_in_memory()?;
    stage7.execute_batch(&remote_ledger::stage7_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage7)? {
        audit_facts(&tx, AuditScopeV1::Full)?;
        core_ledger::audit(&tx, AuditScopeV1::Full)?;
        control_ledger::audit(&tx, AuditScopeV1::Full)?;
        evidence_ledger::audit(&tx, AuditScopeV1::Full)?;
        remote_ledger::audit(&tx, AuditScopeV1::Full)?;
        remote_ledger::migrate(&tx)?;
    }
    let stage8 = Connection::open_in_memory()?;
    stage8.execute_batch(&remote_ledger::stage8_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage8)? {
        audit_facts(&tx, AuditScopeV1::Full)?;
        core_ledger::audit(&tx, AuditScopeV1::Full)?;
        control_ledger::audit(&tx, AuditScopeV1::Full)?;
        evidence_ledger::audit(&tx, AuditScopeV1::Full)?;
        remote_ledger::audit(&tx, AuditScopeV1::Full)?;
        native_ledger::audit(&tx, AuditScopeV1::Full)?;
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
    // Additive: action callbacks kept as history. Older ledgers recorded
    // none (a result after the deadline or after the action closed stayed
    // dispatch_unknown), so there is no history to backfill.
    let stage10 = Connection::open_in_memory()?;
    stage10.execute_batch(&remote_ledger::stage10_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage10)? {
        tx.execute_batch(control_ledger::ACTION_CALLBACK_SCHEMA)?;
    }
    // Additive: write callbacks judged on the executor's monotonic clock.
    // The wall-clock callback table keeps its rows (audited as written) and
    // takes no new ones.
    let stage10_callbacks = Connection::open_in_memory()?;
    stage10_callbacks.execute_batch(&remote_ledger::stage10_callbacks_ddl())?;
    if schema_objects(&tx)? == schema_objects(&stage10_callbacks)? {
        tx.execute_batch(control_ledger::WRITE_CALLBACK_SCHEMA)?;
    }
    verify_schema(&tx)?;
    stamp_ledger_format(&tx)?;
    audit(&tx, AuditScopeV1::Full)?;
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
            ledger: LedgerV1::open(&paths.db_path)?,
        };
        {
            let mut conn = store.connection()?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            store.audit(&tx)?;
            store.commit(tx)?;
        }
        Ok(store)
    }
    /// Establishes that this transaction's snapshot is trusted: unchanged
    /// since the last full audit or validated write on this connection, or
    /// else fully audited now.
    fn audit(&self, tx: &Connection) -> AppResult<()> {
        // Starts the snapshot of a deferred transaction; the data version is
        // then that snapshot's. An immediate one holds the write lock.
        tx.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        let version: i64 = tx.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        {
            let mut j = self.ledger.journal.lock();
            if j.trusted == Some(version) {
                return Ok(());
            }
            j.distrust();
        }
        let trace_started = std::time::Instant::now(); // TEMP-TRACE
        tx.execute("DELETE FROM temp.physical_audit_scope", [])?;
        #[cfg(test)]
        validation_stats::record("trust_audit", "full");
        audit(tx, AuditScopeV1::Full)?;
        crate::physical::temp_trace::FULL_AUDITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
        crate::physical::temp_trace::FULL_AUDIT_US.fetch_add(trace_started.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
        // The history baseline is established by, and only by, a full audit
        // (or by validations under the trust it starts).
        let heads = history_heads(tx, AuditScopeV1::Full)?;
        let mut j = self.ledger.journal.lock();
        j.trusted = Some(version);
        j.history = Some(Arc::new(HistoryBaselineV1 {
            version,
            roots: heads,
        }));
        Ok(())
    }
    /// Commits a transaction of no declared kind: whatever it wrote is
    /// validated without skipping anything.
    pub(super) fn commit(&self, tx: rusqlite::Transaction<'_>) -> AppResult<()> {
        self.commit_as(tx, &UNDECLARED)
    }
    /// Validates exactly what this transaction wrote, then commits it.
    pub(super) fn commit_as(
        &self,
        tx: rusqlite::Transaction<'_>,
        kind: &WriteKindV1,
    ) -> AppResult<()> {
        crate::physical::temp_trace::PHYSICAL_COMMITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
        let (rows, deleted, trusted, history) = {
            let mut j = self.ledger.journal.lock();
            (
                std::mem::take(&mut j.rows),
                std::mem::take(&mut j.deleted),
                j.trusted,
                j.history.clone(),
            )
        };
        #[cfg(debug_assertions)]
        if !kind.tables.is_empty() {
            kind.check(&rows)?;
        }
        let learned = if deleted {
            tx.execute("DELETE FROM temp.physical_audit_scope", [])?;
            audit(&tx, AuditScopeV1::Full)?;
            #[cfg(test)]
            validation_stats::record(kind.name, "full");
            LearnedV1::All(history_heads(&tx, AuditScopeV1::Full)?)
        } else if !rows.is_empty() {
            let trust = TrustV1 {
                trusted,
                history: history.as_deref(),
                #[cfg(test)]
                oracle: !self.ledger.journal.lock().no_oracle,
            };
            validate_written(&tx, &rows, kind, &trust)?
        } else {
            LearnedV1::Nothing
        };
        drop(history);
        tx.commit()?;
        *self.ledger.stamp.lock() = FileStampV1::of(&self.ledger.path)?;
        // Own commits leave this connection's data version unchanged, so the
        // baseline stays at `trusted`; it is extended only if nothing
        // distrusted the ledger meanwhile.
        let mut j = self.ledger.journal.lock();
        let trusted = j.trusted;
        if let (Some(b), Some(v)) = (j.history.as_mut(), trusted) {
            // The transaction's own reference is gone: this updates in place.
            let b = Arc::make_mut(b);
            if b.version == v {
                match learned {
                    LearnedV1::Nothing => {}
                    LearnedV1::All(heads) => b.roots = heads,
                    LearnedV1::Roots(heads) => b.learn(heads),
                }
            }
        }
        Ok(())
    }
    /// The ledger's one connection, for one transaction at a time. A journal
    /// left by a transaction that neither committed nor rolled back makes the
    /// next audit a full one.
    pub(in crate::physical) fn connection(&self) -> AppResult<LedgerGuardV1<'_>> {
        let me = std::thread::current().id();
        require(
            *self.ledger.owner.lock() != Some(me),
            "Physical ledger connection re-entered",
        )?;
        let connection = self.ledger.connection.lock();
        *self.ledger.owner.lock() = Some(me);
        let mut conn = LedgerGuardV1 {
            connection,
            owner: &self.ledger.owner,
        };
        self.ledger.current()?;
        let stamp = FileStampV1::of(&self.ledger.path)?;
        if stamp != *self.ledger.stamp.lock() {
            crate::physical::temp_trace::RECONNECTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // TEMP-TRACE
            *conn = connect(&self.ledger.path, &self.ledger.journal)?;
            let mut j = self.ledger.journal.lock();
            j.distrust();
            j.rows.clear();
            j.deleted = false;
            *self.ledger.stamp.lock() = stamp;
        }
        {
            let mut j = self.ledger.journal.lock();
            if j.pending() {
                j.distrust();
                j.rows.clear();
                j.deleted = false;
            }
        }
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
        self.audit(&tx)?;
        let value = load_registration(&tx, id, true)?;
        self.commit(tx)?;
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
        self.audit(&tx)?;
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
        self.audit(&tx)?;
        self.commit_as(tx, &kinds::ENROLL)?;
        Ok(())
    }
    pub(super) fn retire(&self, id: &EnvironmentRefV1, expected: u64) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.audit(&tx)?;
        let old = load_registration(&tx, id, true)?;
        require(old.revision == expected, "Retirement revision mismatch")?;
        bump_environment_domains(&tx, id)?;
        withdraw_environment(&tx, id)?;
        tx.execute("UPDATE physical_environments SET retired=1,denial_revision=?2 WHERE environment_id=?1 AND retired=0 AND revision=?3",params![text(id),checked_integer(expected.checked_add(1).unwrap_or(u64::MAX))?,checked_integer(expected)?])?;
        self.commit_as(tx, &kinds::RETIRE)?;
        Ok(())
    }
    pub(super) fn epochs(
        &self,
        domains: impl Iterator<Item = DomainId>,
    ) -> AppResult<BTreeMap<DomainId, u64>> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        self.audit(&tx)?;
        let mut result = BTreeMap::new();
        for domain in domains {
            let epoch: i64 = tx.query_row(
                "SELECT epoch FROM physical_domains WHERE domain_id=?1",
                [text(&domain)],
                |r| r.get(0),
            )?;
            result.insert(domain, epoch as u64);
        }
        self.commit(tx)?;
        Ok(result)
    }
    /// Atomic CAS across canonical domains, ledger bookkeeping only. Domains
    /// remain quarantined; no holder, task, native receipt or permit is created.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no production binding is attached; the reference bindings are test-only"
        )
    )]
    pub(super) fn advance_epochs(&self, expected: &BTreeMap<DomainId, u64>) -> AppResult<()> {
        require(
            !expected.is_empty() && expected.len() <= 16,
            "Invalid domain ledger operation",
        )?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.audit(&tx)?;
        for (domain, epoch) in expected {
            let next = checked_integer(epoch.checked_add(1).unwrap_or(u64::MAX))?;
            let n = tx.execute(
                "UPDATE physical_domains SET epoch=?2 WHERE domain_id=?1 AND epoch=?3",
                params![text(domain), next, checked_integer(*epoch)?],
            )?;
            require(n == 1, "Stale/conflicting physical domain ledger operation")?;
        }
        self.commit_as(tx, &kinds::ADVANCE_EPOCHS)?;
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
        self.audit(&tx)?;
        require(
            load_registration(&tx, environment, true)?.digest()? == *registration_digest,
            "Qualification enrollment mismatch",
        )?;
        // Strict insert: identities are immutable, including expiry/evidence. A
        // new qualification requires a new ID; withdrawal cannot be overwritten.
        tx.execute("INSERT INTO physical_qualifications(qualification_id,environment_id,revision,registration_digest,profile_digest,binding_digest,evidence_class,enforcement_class,evidence_digest,conditions_digest,provenance_digest,record_digest,record_json,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![text(&q.qualification_id),text(environment),checked_integer(q.revision)?,text(registration_digest),text(&q.profile_digest),text(&q.binding_digest),tag(&q.evidence_class)?,tag(&q.required_enforcement_class)?,text(&q.evidence_digest),text(&q.conditions_digest),text(provenance),text(&q.digest()?),serde_json::to_string(q)?,q.expires_at.get() as i64])?;
        self.commit_as(tx, &kinds::RECORD_QUALIFICATION)?;
        Ok(())
    }
    pub(super) fn qualification(
        &self,
        id: &QualificationId,
        now: UnixMillis,
    ) -> AppResult<PhysicalQualificationV1> {
        let mut conn = self.connection()?;
        let tx = conn.transaction()?;
        self.audit(&tx)?;
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
        self.commit(tx)?;
        Ok(q)
    }
    pub(super) fn withdraw(&self, id: &QualificationId, revision: u64) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.audit(&tx)?;
        let n = tx.execute("UPDATE physical_qualifications SET withdrawal_revision=?2 WHERE qualification_id=?1 AND revision < ?2 AND withdrawal_revision < ?2",params![text(id),checked_integer(revision)?])?;
        require(n == 1, "Missing/stale qualification withdrawal")?;
        self.commit_as(tx, &kinds::WITHDRAW_QUALIFICATION)?;
        Ok(())
    }
    pub(super) fn invalidate_qualifications(
        &self,
        environment: &EnvironmentRefV1,
    ) -> AppResult<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.audit(&tx)?;
        load_registration(&tx, environment, true)?;
        withdraw_environment(&tx, environment)?;
        self.commit_as(tx, &kinds::INVALIDATE_QUALIFICATIONS)?;
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
/// Which rows an audit pass covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuditScopeV1 {
    /// Every row, plus page integrity and foreign keys.
    Full,
    /// The groups named in `temp.physical_audit_scope` by kind: `root`,
    /// `action`, `session`, `review`, `domain`, `semantic`, `remote_review`
    /// and `offer`. SQLite enforces foreign keys on this connection's own
    /// writes; the full audit rechecks them.
    Touched,
    /// As `Touched`, for a write that changed no evidence or consequence row
    /// while every in-scope action's history is exactly the validated
    /// baseline: those rows are not replayed; every other check runs.
    TouchedValidatedHistory,
}
impl AuditScopeV1 {
    pub(super) fn full(self) -> bool {
        self == Self::Full
    }
    /// Whether evidence and consequence rows in scope are replayed.
    pub(super) fn replays_history(self) -> bool {
        self != Self::TouchedValidatedHistory
    }
    /// A condition restricting `column` to this scope's keys of `kind`.
    pub(super) fn filter(self, column: &str, kind: &str) -> String {
        match self {
            Self::Full => "1".into(),
            Self::Touched | Self::TouchedValidatedHistory => format!(
                "{column} IN (SELECT key FROM temp.physical_audit_scope WHERE kind='{kind}')"
            ),
        }
    }
}
/// Ledger tables whose rows belong to a group, and the key expressions that
/// name the group. A write to any other physical table (enrollment,
/// qualification, schema and format markers, retired tables) makes the
/// validation a full audit.
const GROUPED_TABLES: &[(&str, &[(&str, &str)])] = &[
    (
        "physical_attempts",
        &[("root", "root_id"), ("review", "review_id")],
    ),
    ("physical_reviews", &[("review", "review_id")]),
    ("physical_sessions", &[("root", "root_id")]),
    (
        "physical_domain_reservations",
        &[("session", "session_id"), ("domain", "domain_id")],
    ),
    ("physical_domains", &[("domain", "domain_id")]),
    ("physical_control_budgets", &[("root", "root_id")]),
    ("physical_actions", &[("root", "root_id")]),
    ("physical_action_callbacks", &[("action", "action_id")]),
    ("physical_write_callbacks", &[("action", "action_id")]),
    ("physical_decisions", &[("root", "root_id")]),
    ("physical_evidence", &[("action", "action_id")]),
    ("physical_consequences", &[("action", "action_id")]),
    ("physical_reconciliations", &[("action", "action_id")]),
    ("physical_task_acceptance", &[("root", "root_id")]),
    ("physical_handover_policies", &[("session", "session_id")]),
    ("physical_handovers", &[("session", "session_id")]),
    ("physical_handover_verdicts", &[("session", "session_id")]),
    ("physical_effect_bound_violations", &[("root", "root_id")]),
    (
        "physical_semantic_messages",
        &[("semantic", "peer||' '||semantic_id")],
    ),
    ("physical_remote_reviews", &[("remote_review", "review_id")]),
    ("physical_remote_offers", &[("offer", "peer")]),
];
/// The trust a commit validates under: the journal as the transaction found it.
struct TrustV1<'a> {
    trusted: Option<i64>,
    history: Option<&'a HistoryBaselineV1>,
    /// Tests: also run the full group audit and require the same outcome.
    #[cfg(test)]
    oracle: bool,
}
/// Validates the rows a transaction wrote and every group they belong to,
/// before it commits. Every check the full audit makes on those groups runs
/// unchanged; rows outside them were audited and have not changed. History
/// rows of the groups are replayed unless the write changed none of them and
/// all of them are the validated baseline (see `HistoryBaselineV1`).
fn validate_written(
    tx: &Connection,
    rows: &[(String, i64)],
    kind: &WriteKindV1,
    trust: &TrustV1<'_>,
) -> AppResult<LearnedV1> {
    tx.execute("DELETE FROM temp.physical_audit_scope", [])?;
    // Appends to the immutable evidence tables at the head of their actions
    // are validated row by row against their audited predecessors, which is
    // what the group audit concludes about them (see `validate_appended`).
    // The checks every transaction runs still run, with no group in scope.
    if !rows.is_empty()
        && rows
            .iter()
            .all(|(table, _)| evidence_ledger::APPEND_ONLY.contains(&table.as_str()))
        && evidence_ledger::appended_at_head(tx, rows)?
    {
        audit(tx, AuditScopeV1::Touched)?;
        evidence_ledger::validate_appended(tx, rows)?;
        return Ok(LearnedV1::Roots(appended_heads(tx, rows)?));
    }
    for (table, rowid) in rows {
        let Some((_, keys)) = GROUPED_TABLES.iter().find(|(t, _)| t == table) else {
            tx.execute("DELETE FROM temp.physical_audit_scope", [])?;
            audit(tx, AuditScopeV1::Full)?;
            #[cfg(test)]
            validation_stats::record(kind.name, "full");
            return Ok(LearnedV1::All(history_heads(tx, AuditScopeV1::Full)?));
        };
        for (kind, key) in *keys {
            tx.execute(
                &format!("INSERT OR IGNORE INTO temp.physical_audit_scope SELECT '{kind}',{key} FROM {table} WHERE rowid=?1"),
                [rowid],
            )?;
        }
    }
    // Groups reach their Roots, and Roots their members: a check on one
    // member of a Root reads the others.
    tx.execute_batch(
        "INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'root',root_id FROM physical_sessions WHERE session_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='session');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'root',root_id FROM physical_actions WHERE action_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='action');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'root',s.root_id FROM physical_domain_reservations r JOIN physical_sessions s USING(session_id) WHERE r.domain_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='domain');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'root',root_id FROM physical_attempts WHERE review_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='review');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'root',root_id FROM physical_attempts WHERE role='executor_remote' AND requester||' '||json_extract(audit_json,'$.remoteLineage.semanticId') IN (SELECT key FROM temp.physical_audit_scope WHERE kind='semantic');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'review',review_id FROM physical_attempts WHERE root_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='root');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'session',session_id FROM physical_sessions WHERE root_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='root');
         INSERT OR IGNORE INTO temp.physical_audit_scope SELECT 'action',action_id FROM physical_actions WHERE root_id IN (SELECT key FROM temp.physical_audit_scope WHERE kind='root');",
    )?;
    // Every in-scope action's history heads, read now under this transaction's
    // lock: the history the groups would replay.
    let heads = history_heads(tx, AuditScopeV1::Touched)?;
    let bounded = kind.preserves_history()
        && rows
            .iter()
            .all(|(table, _)| !evidence_ledger::APPEND_ONLY.contains(&table.as_str()))
        && trust
            .history
            .is_some_and(|b| trust.trusted == Some(b.version) && b.covers(&heads))
        // Re-read at commit: no other connection has committed since the
        // trust (and the baseline with it) was established.
        && trust.trusted == Some(data_version(tx)?);
    if !bounded {
        #[cfg(test)]
        validation_stats::record(kind.name, "replayed");
        audit(tx, AuditScopeV1::Touched)?;
        return Ok(LearnedV1::Roots(heads));
    }
    #[cfg(test)]
    validation_stats::record(kind.name, "bounded");
    let result = audit(tx, AuditScopeV1::TouchedValidatedHistory);
    #[cfg(test)]
    if trust.oracle {
        let oracle = audit(tx, AuditScopeV1::Touched);
        assert_eq!(
            result.as_ref().map_err(|e| e.message().to_owned()),
            oracle.as_ref().map_err(|e| e.message().to_owned()),
            "bounded Root-group validation diverged from the group audit ({})",
            kind.name
        );
    }
    result?;
    Ok(LearnedV1::Roots(heads))
}
/// This connection's `PRAGMA data_version`: it changes when any other
/// connection or process commits to the file.
fn data_version(c: &Connection) -> AppResult<i64> {
    Ok(c.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}
/// The heads, by Root, of the actions whose history these appended rows extend.
fn appended_heads(
    c: &Connection,
    rows: &[(String, i64)],
) -> AppResult<BTreeMap<String, BTreeMap<String, HistoryHeadV1>>> {
    let mut heads = BTreeMap::<String, BTreeMap<String, HistoryHeadV1>>::new();
    for (table, rowid) in rows {
        let (root, action, evidence, consequences): (String, String, i64, i64) = c.query_row(
            &format!(
                "SELECT a.root_id,a.action_id,
                 COALESCE((SELECT max(revision) FROM physical_evidence e WHERE e.action_id=a.action_id),0),
                 COALESCE((SELECT max(revision) FROM physical_consequences q WHERE q.action_id=a.action_id),0)
                 FROM physical_actions a WHERE a.action_id=(SELECT action_id FROM {table} WHERE rowid=?1)"
            ),
            [rowid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        heads.entry(root).or_default().insert(
            action,
            HistoryHeadV1 {
                evidence,
                consequences,
            },
        );
    }
    Ok(heads)
}

/// Tests: which validation each commit took, and how many history rows the
/// audits on this thread read.
#[cfg(test)]
pub(in crate::physical) mod validation_stats {
    use std::cell::RefCell;
    #[derive(Clone, Debug, Default)]
    pub(in crate::physical) struct StatsV1 {
        /// (write kind, "bounded" | "replayed" | "full").
        pub validations: Vec<(&'static str, &'static str)>,
        /// Evidence and consequence rows decoded by audits.
        pub history_rows: u64,
    }
    thread_local! {
        static STATS: RefCell<StatsV1> = RefCell::default();
    }
    pub(super) fn record(kind: &'static str, path: &'static str) {
        STATS.with(|s| s.borrow_mut().validations.push((kind, path)));
        // A whole-suite tally across threads, when asked for.
        if let Ok(file) = std::env::var("PASTEY_TEST_LEDGER_VALIDATION_LOG") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
            {
                // One write per line, so lines from parallel tests never mix.
                let _ = f.write_all(format!("{kind}\t{path}\n").as_bytes());
            }
        }
    }
    pub(in crate::physical) fn history_row() {
        STATS.with(|s| s.borrow_mut().history_rows += 1);
    }
    /// This thread's tally since the last call.
    pub(in crate::physical) fn take() -> StatsV1 {
        STATS.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}
/// The ledger audit over `scope`. A migration's connection has no scope
/// table and always audits in full.
fn audit(conn: &Connection, scope: AuditScopeV1) -> AppResult<()> {
    require(
        ledger_format(conn)? == Some(LEDGER_FORMAT),
        "Physical ledger format mismatch",
    )?;
    let domains = !scope.full()
        && conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM temp.physical_audit_scope WHERE kind='domain')",
            [],
            |r| r.get::<_, bool>(0),
        )?;
    if scope.full() || domains {
        audit_facts(conn, scope)?;
    }
    core_ledger::audit(conn, scope)?;
    control_ledger::audit(conn, scope)?;
    evidence_ledger::audit(conn, scope)?;
    remote_ledger::audit(conn, scope)?;
    native_ledger::audit(conn, scope)?;
    qualification_ledger::audit(conn)
}
/// Enrollment, qualification, alias and domain rows: few, audited whole.
fn audit_facts(conn: &Connection, scope: AuditScopeV1) -> AppResult<()> {
    if scope.full() {
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
pub(super) fn test_full_audit(conn: &Connection) -> AppResult<()> {
    audit(conn, AuditScopeV1::Full)
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

/// Tests: the trusted history baseline, and writes as a declared kind.
#[cfg(test)]
impl PhysicalStoreV1 {
    /// The baseline as (data version, Root -> action -> (evidence head,
    /// consequence head)), if the ledger is trusted.
    pub(in crate::physical) fn test_history_baseline(
        &self,
    ) -> Option<(i64, BTreeMap<String, BTreeMap<String, (i64, i64)>>)> {
        let j = self.ledger.journal.lock();
        j.history.as_ref().map(|b| {
            (
                b.version,
                b.roots
                    .iter()
                    .map(|(root, actions)| {
                        let heads = actions
                            .iter()
                            .map(|(a, h)| (a.clone(), (h.evidence, h.consequences)))
                            .collect();
                        (root.clone(), heads)
                    })
                    .collect(),
            )
        })
    }
    /// Replaces one action's validated heads (a baseline that no longer
    /// matches the history).
    pub(in crate::physical) fn test_set_history_head(
        &self,
        root: &str,
        action: &str,
        heads: (i64, i64),
    ) {
        let mut j = self.ledger.journal.lock();
        let b = Arc::make_mut(j.history.as_mut().expect("a trusted baseline"));
        b.roots.entry(root.to_owned()).or_default().insert(
            action.to_owned(),
            HistoryHeadV1 {
                evidence: heads.0,
                consequences: heads.1,
            },
        );
    }
    /// Stops comparing bounded validations with the full group audit (for
    /// measuring the bounded work alone).
    pub(in crate::physical) fn test_without_oracle(&self) {
        self.ledger.journal.lock().no_oracle = true;
    }
    /// Runs `write` in one immediate transaction and commits it as the
    /// declared kind named `kind`.
    pub(in crate::physical) fn test_write_as(
        &self,
        kind: &str,
        write: impl FnOnce(&Connection) -> rusqlite::Result<()>,
    ) -> AppResult<()> {
        let kind = kinds::ALL
            .iter()
            .find(|k| k.name == kind)
            .unwrap_or_else(|| panic!("no write kind {kind}"));
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        self.audit(&tx)?;
        write(&tx)?;
        self.commit_as(tx, kind)
    }
}
