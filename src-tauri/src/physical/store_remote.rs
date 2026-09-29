//! Stage 7 schema extension and durable semantic correlation. No authority restoration.
use super::*;
use crate::physical::{contracts::*, protocol::*};
use serde::Deserialize;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_remote_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_remote_schema VALUES(1,1);
CREATE TABLE physical_semantic_messages(peer TEXT NOT NULL,semantic_id TEXT NOT NULL,digest TEXT NOT NULL CHECK(length(digest)=64),session_pair TEXT NOT NULL,message_json TEXT NOT NULL,result_json TEXT,PRIMARY KEY(peer,semantic_id)) STRICT;
CREATE TRIGGER physical_semantic_identity BEFORE UPDATE ON physical_semantic_messages
WHEN NEW.peer!=OLD.peer OR NEW.semantic_id!=OLD.semantic_id OR NEW.digest!=OLD.digest OR NEW.session_pair!=OLD.session_pair OR NEW.message_json!=OLD.message_json
BEGIN SELECT RAISE(ABORT,'physical semantic identity immutable'); END;
CREATE TRIGGER physical_semantic_keep BEFORE DELETE ON physical_semantic_messages BEGIN SELECT RAISE(ABORT,'physical replay history required');END;
CREATE TABLE physical_remote_reviews(review_id TEXT PRIMARY KEY,record_json TEXT NOT NULL,session_pair TEXT NOT NULL) STRICT;
CREATE TABLE physical_remote_offers(peer TEXT PRIMARY KEY,session_pair TEXT NOT NULL,offers_json TEXT NOT NULL) STRICT;
"#;

/// Old local audit encodings stay byte-identical. New remote Roots explicitly
/// carry version 2 lineage and use executor_remote rows throughout the ledger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct RemoteRootLineageV2 {
    pub version: u8,
    pub binding: crate::host_identity::HostSessionBinding,
    pub semantic_id: RequestId,
    pub semantic_digest: DigestV1,
}

pub(super) fn stage7_ddl() -> String {
    [
        super::SCHEMA,
        super::core_ledger::SCHEMA,
        super::control_ledger::SCHEMA,
        super::evidence_ledger::SCHEMA,
    ]
    .join("\n")
    .replace(
        "CHECK(role='requester_executor')",
        "CHECK(role IN ('requester_executor','executor_remote'))",
    ) + SCHEMA
}
/// Called only for an exactly recognized, audited Stage 6 or 7 schema, with foreign
/// keys disabled outside the transaction. Copy original values, including old
/// audit JSON/digests; no historical row is reclassified or reconstructed.
pub(super) fn migrate(c: &Connection) -> AppResult<()> {
    rebuild(c, &current_ddl())
}
pub(super) fn rebuild(c: &Connection, ddl: &str) -> AppResult<()> {
    use rusqlite::types::Value;
    // The ledger format marker is not part of any DDL stage and is kept as is.
    let names: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'physical_*' AND name!=?1 ORDER BY name")?
        .query_map([super::LEDGER_META_TABLE], |r| r.get(0))?.collect::<Result<_,_>>()?;
    let mut saved = Vec::new();
    for name in &names {
        let mut stmt = c.prepare(&format!("SELECT * FROM {name}"))?;
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                (0..columns)
                    .map(|i| r.get::<_, Value>(i))
                    .collect::<Result<Vec<_>, _>>()
            })?
            .collect::<Result<Vec<_>, _>>()?;
        saved.push((name.clone(), columns, rows));
    }
    let triggers: Vec<String> = c
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='trigger' AND tbl_name GLOB 'physical_*' AND tbl_name!=?1",
        )?
        .query_map([super::LEDGER_META_TABLE], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for t in triggers {
        c.execute_batch(&format!("DROP TRIGGER {t}"))?;
    }
    for name in &names {
        c.execute_batch(&format!("DROP TABLE {name}"))?;
    }
    // These DDL templates include initial version rows and one legacy acceptance
    // population statement. Empty attempts make that population a no-op.
    c.execute_batch(ddl)?;
    for (name, columns, rows) in saved {
        if name.ends_with("_schema") {
            continue;
        } // checked versions are identical
        let exists: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [&name],
            |r| r.get(0),
        )?;
        if !exists {
            require(
                rows.is_empty(),
                "Cannot drop populated history during schema rebuild",
            )?;
            continue;
        }
        let placeholders = vec!["?"; columns].join(",");
        let mut stmt = c.prepare(&format!("INSERT INTO {name} VALUES({placeholders})"))?;
        for row in rows {
            stmt.execute(rusqlite::params_from_iter(row))?;
        }
    }
    Ok(())
}

impl PhysicalStoreV1 {
    pub(in crate::physical) fn claim_semantic(
        &self,
        peer: &crate::host_identity::HostRef,
        m: &PhysicalMessageV1,
    ) -> AppResult<bool> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<(String,String)> = tx.query_row("SELECT digest,session_pair FROM physical_semantic_messages WHERE peer=?1 AND semantic_id=?2",
            params![peer.as_str(),text(&m.semantic_id)], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((d, s)) = old {
            require(
                d == text(&m.digest()?) && s == m.session_pair,
                "Changed-digest or old-session physical replay",
            )?;
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO physical_semantic_messages VALUES(?1,?2,?3,?4,?5,NULL)",
            params![
                peer.as_str(),
                text(&m.semantic_id),
                text(&m.digest()?),
                m.session_pair,
                serde_json::to_string(m)?
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub(in crate::physical) fn semantic_message(
        &self,
        peer: &crate::host_identity::HostRef,
        id: &RequestId,
    ) -> AppResult<PhysicalMessageV1> {
        let c = self.connection()?;
        let raw: String = c.query_row(
            "SELECT message_json FROM physical_semantic_messages WHERE peer=?1 AND semantic_id=?2",
            params![peer.as_str(), text(id)],
            |r| r.get(0),
        )?;
        decode(&raw)
    }
    pub(in crate::physical) fn semantic_result(
        &self,
        peer: &crate::host_identity::HostRef,
        id: &RequestId,
    ) -> AppResult<Option<PhysicalStatusV1>> {
        let c = self.connection()?;
        let raw: Option<String> = c.query_row(
            "SELECT result_json FROM physical_semantic_messages WHERE peer=?1 AND semantic_id=?2",
            params![peer.as_str(), text(id)],
            |r| r.get(0),
        )?;
        raw.map(|r| decode(&r)).transpose()
    }
    pub(in crate::physical) fn save_semantic_result(
        &self,
        peer: &crate::host_identity::HostRef,
        id: &RequestId,
        s: &PhysicalStatusV1,
    ) -> AppResult<()> {
        let c = self.connection()?;
        require(c.execute("UPDATE physical_semantic_messages SET result_json=?3 WHERE peer=?1 AND semantic_id=?2",params![peer.as_str(),text(id),serde_json::to_string(s)?])? == 1, "Unknown physical result correlation")
    }
    pub(in crate::physical) fn save_remote_review(
        &self,
        r: &PhysicalReviewRecordV1,
        pair: &str,
    ) -> AppResult<()> {
        r.validate()?;
        let c = self.connection()?;
        c.execute("INSERT INTO physical_remote_reviews VALUES(?1,?2,?3) ON CONFLICT(review_id) DO UPDATE SET record_json=excluded.record_json WHERE physical_remote_reviews.session_pair=excluded.session_pair",params![text(&r.review_id),serde_json::to_string(r)?,pair])?;
        Ok(())
    }
    pub(in crate::physical) fn remote_review(
        &self,
        id: &ReviewId,
        pair: &str,
    ) -> AppResult<PhysicalReviewRecordV1> {
        let c = self.connection()?;
        let raw: String = c.query_row("SELECT record_json FROM physical_remote_reviews WHERE review_id=?1 AND session_pair=?2",params![text(id),pair],|r|r.get(0))?;
        decode(&raw)
    }
    pub(in crate::physical) fn import_approved_review(
        &self,
        r: &PhysicalReviewRecordV1,
        snapshot: &crate::physical::binding::BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::core_ledger::dependencies(&tx, &r.scope, snapshot, now)?;
        require(
            r.state == PhysicalReviewStateV1::Approved && r.revision == 1,
            "Unsupported remote review lineage",
        )?;
        let a = r
            .approval
            .as_ref()
            .ok_or_else(|| crate::error::AppError::InvalidInput("Missing approval".into()))?;
        require(
            now >= a.approved_at && now < a.expires_at,
            "Stale remote review",
        )?;
        tx.execute("INSERT INTO physical_reviews(review_id,revision,scope_digest,scope_json,state,state_revision,approval_id,approval_principal,approved_at,approval_expiry,record_json) VALUES(?1,1,?2,?3,'approved',3,?4,?5,?6,?7,?8)", params![text(&r.review_id),text(&r.scope_digest),serde_json::to_string(&r.scope)?,text(&a.approval_id),text(&a.principal),a.approved_at.get() as i64,a.expires_at.get() as i64,serde_json::to_string(r)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn physical_status(
        &self,
        root: &RootId,
    ) -> AppResult<PhysicalStatusV1> {
        let c = self.connection()?;
        let mut s = PhysicalStatusV1::pending();
        let state: String = c.query_row(
            "SELECT state FROM physical_attempts WHERE root_id=?1",
            [text(root)],
            |r| r.get(0),
        )?;
        s.root = Some(root.clone());
        s.authority = if state == "open" {
            PhysicalAuthorityStateV1::Open
        } else {
            PhysicalAuthorityStateV1::Closed
        };
        let session: Option<(String,String,bool)> = c.query_row("SELECT session_id,state,(fence_ack IS NOT NULL) FROM physical_sessions WHERE root_id=?1",[text(root)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, state, acked)) = session {
            s.session = Some(SessionId::try_from(id)?);
            s.installation = if state == "active" {
                PhysicalInstallationStateV1::Active
            } else if state == "quarantined" {
                PhysicalInstallationStateV1::Quarantined
            } else {
                PhysicalInstallationStateV1::Pending
            };
            s.quarantined = state == "quarantined";
            s.enforcement_pending = state == "installing" || (s.quarantined && !acked);
        }
        let action: Option<(String, String)> = c
            .query_row(
                "SELECT action_id,disposition FROM physical_actions WHERE root_id=?1",
                [text(root)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((id, d)) = action {
            let id = ActionId::try_from(id)?;
            s.action = Some(id.clone());
            s.dispatch = match d.as_str() {
                "not_sent" => PhysicalDispatchStateV1::NotSent,
                "dispatch_intent" => PhysicalDispatchStateV1::IntentCommitted,
                "fake_accepted" => PhysicalDispatchStateV1::Acknowledged,
                "fake_refused" => PhysicalDispatchStateV1::Refused,
                _ => PhysicalDispatchStateV1::Unknown,
            };
            if let Some(raw) = c.query_row("SELECT record_json FROM physical_consequences WHERE action_id=?1 ORDER BY revision DESC LIMIT 1",[text(&id)],|r|r.get::<_,String>(0)).optional()? {
                let consequence: crate::physical::evidence::PhysicalConsequenceV1=decode(&raw)?;
                s.consequence=consequence.state;
            }
            let reconciled: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM physical_reconciliations WHERE action_id=?1)",
                [text(&id)],
                |r| r.get(0),
            )?;
            if reconciled {
                s.reconciliation = PhysicalReconciliationStateV1::Recorded;
            }
        }
        let acceptance: String = c.query_row(
            "SELECT state FROM physical_task_acceptance WHERE root_id=?1",
            [text(root)],
            |r| r.get(0),
        )?;
        s.acceptance = serde_json::from_value(serde_json::Value::String(acceptance))?;
        Ok(s)
    }
}

pub(super) fn audit(c: &Connection) -> AppResult<()> {
    let version: i64 = c.query_row(
        "SELECT version FROM physical_remote_schema WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    require(version == 1, "Unknown physical remote schema")?;
    let mut stmt=c.prepare("SELECT peer,semantic_id,digest,session_pair,message_json,result_json FROM physical_semantic_messages")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let m: PhysicalMessageV1 = decode(&r.get::<_, String>(4)?)?;
        let peer: String = r.get(0)?;
        require(
            (peer == m.requester.as_str() || peer == m.executor.as_str())
                && r.get::<_, String>(1)? == text(&m.semantic_id)
                && r.get::<_, String>(2)? == text(&m.digest()?)
                && r.get::<_, String>(3)? == m.session_pair,
            "Corrupt physical semantic correlation",
        )?;
        if let Some(raw) = r.get::<_, Option<String>>(5)? {
            let status: PhysicalStatusV1 = decode(&raw)?;
            status.validate()?;
            require(
                matches!(m.operation, PhysicalOperationV1::Start { .. }),
                "Physical result on non-Start correlation",
            )?;
        }
    }
    let mut stmt = c.prepare("SELECT record_json FROM physical_remote_reviews")?;
    for raw in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let review: PhysicalReviewRecordV1 = decode(&raw?)?;
        review.validate()?;
    }
    let mut stmt = c.prepare("SELECT offers_json FROM physical_remote_offers")?;
    for raw in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let offers: Vec<PhysicalReviewScopeV1> = decode(&raw?)?;
        require(offers.len() <= 8, "Oversized cached physical offers")?;
    }
    let mut stmt =
        c.prepare("SELECT audit_json FROM physical_attempts WHERE role='executor_remote'")?;
    for raw in stmt.query_map([], |r| r.get::<_, String>(0))? {
        let root: RootAuditV1 = decode(&raw?)?;
        let lineage = root.remote_lineage.as_ref().ok_or_else(|| {
            crate::error::AppError::InvalidInput("Missing remote Root lineage".into())
        })?;
        let raw: String = c.query_row(
            "SELECT message_json FROM physical_semantic_messages WHERE peer=?1 AND semantic_id=?2",
            params![root.requester.as_str(), text(&lineage.semantic_id)],
            |r| r.get(0),
        )?;
        let m: PhysicalMessageV1 = decode(&raw)?;
        let PhysicalOperationV1::Start { review } = &m.operation else {
            return Err(crate::error::AppError::InvalidInput(
                "Remote Root without Start".into(),
            ));
        };
        require(
            m.digest()? == lineage.semantic_digest
                && m.session_pair == lineage.binding.session_pair_ref
                && review.review_id == root.review_id
                && review.scope_digest == root.scope_digest
                && review.approval.as_ref() == Some(&root.approval),
            "Remote Root semantic lineage mismatch",
        )?;
    }
    Ok(())
}

pub(super) fn stage8_ddl() -> String {
    stage7_ddl()
        .replace(
            "CHECK(install_evidence='adapter_isolation_only')",
            "CHECK(install_evidence IN ('adapter_isolation_only','native_fence'))",
        )
        .replace(
            "CHECK(fence_ack='adapter_isolation_only')",
            "CHECK(fence_ack IN ('adapter_isolation_only','native_fence'))",
        )
        + super::native_ledger::SCHEMA
}

pub(super) fn current_ddl() -> String {
    stage8_ddl() + super::qualification_ledger::SCHEMA
}
