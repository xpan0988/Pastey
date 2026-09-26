//! Stage 3 audit data and transactions. No row-to-live-authority conversion.
use super::*;
use crate::host_identity::HostRef;
use crate::physical::{binding::BindingLedgerSnapshotV1, contracts::*};
use serde::Deserialize;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_core_schema (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1), version INTEGER NOT NULL CHECK(version=1)
) STRICT;
INSERT INTO physical_core_schema VALUES(1,1);
CREATE TABLE physical_reviews (
 review_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>=1),
 scope_digest TEXT NOT NULL CHECK(length(scope_digest)=64), scope_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('draft','reviewed','approved','rejected','expired')),
 state_revision INTEGER NOT NULL CHECK(state_revision>=1),
 approval_id TEXT UNIQUE, approval_principal TEXT, approved_at INTEGER, approval_expiry INTEGER,
 record_json TEXT NOT NULL,
 PRIMARY KEY(review_id,revision),
 CHECK((approval_id IS NULL AND approval_principal IS NULL AND approved_at IS NULL AND approval_expiry IS NULL AND state!='approved') OR
       (approval_id IS NOT NULL AND approval_principal IS NOT NULL AND approved_at>0 AND approval_expiry>approved_at AND state IN ('approved','expired')))
) STRICT;
CREATE TABLE physical_attempts (
 root_id TEXT NOT NULL, role TEXT NOT NULL CHECK(role='requester_executor'),
 attempt_id TEXT NOT NULL UNIQUE, approval_id TEXT NOT NULL UNIQUE REFERENCES physical_reviews(approval_id),
 review_id TEXT NOT NULL, review_revision INTEGER NOT NULL,
 scope_digest TEXT NOT NULL CHECK(length(scope_digest)=64), principal TEXT NOT NULL,
 requester TEXT NOT NULL, executor TEXT NOT NULL,
 environment_id TEXT NOT NULL REFERENCES physical_environments(environment_id),
 registration_digest TEXT NOT NULL CHECK(length(registration_digest)=64),
 binding_digest TEXT NOT NULL CHECK(length(binding_digest)=64),
 profile_digest TEXT NOT NULL CHECK(length(profile_digest)=64),
 qualification_id TEXT NOT NULL REFERENCES physical_qualifications(qualification_id),
 qualification_digest TEXT NOT NULL CHECK(length(qualification_digest)=64),
 policy_digest TEXT NOT NULL CHECK(length(policy_digest)=64), runtime_generation TEXT NOT NULL,
 created_at INTEGER NOT NULL CHECK(created_at>0), expires_at INTEGER NOT NULL CHECK(expires_at>created_at),
 audit_digest TEXT NOT NULL CHECK(length(audit_digest)=64), audit_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('open','closed')), revision INTEGER NOT NULL CHECK(revision>=1),
 close_reason TEXT CHECK(close_reason IN ('interrupted','shutdown','revoked','dependency_invalidated','superseded','expired')),
 PRIMARY KEY(root_id,role), FOREIGN KEY(review_id,review_revision) REFERENCES physical_reviews(review_id,revision),
 CHECK((state='open' AND revision=1 AND close_reason IS NULL) OR (state='closed' AND revision=2 AND close_reason IS NOT NULL))
) STRICT;
CREATE TRIGGER physical_review_immutable BEFORE UPDATE ON physical_reviews
WHEN NEW.review_id!=OLD.review_id OR NEW.revision!=OLD.revision OR NEW.scope_digest!=OLD.scope_digest
 OR NEW.scope_json!=OLD.scope_json OR NEW.state_revision!=OLD.state_revision+1
 OR (OLD.approval_id IS NOT NULL AND (NEW.approval_id IS NOT OLD.approval_id OR NEW.approval_principal IS NOT OLD.approval_principal OR NEW.approved_at IS NOT OLD.approved_at OR NEW.approval_expiry IS NOT OLD.approval_expiry))
 OR NOT ((OLD.state='draft' AND NEW.state IN ('reviewed','rejected','expired')) OR (OLD.state='reviewed' AND NEW.state IN ('approved','rejected','expired')) OR (OLD.state='approved' AND NEW.state='expired') OR (OLD.state='approved' AND NEW.state='approved'))
BEGIN SELECT RAISE(ABORT,'physical immutable review or invalid transition'); END;
CREATE TRIGGER physical_attempt_closed BEFORE UPDATE ON physical_attempts
WHEN OLD.state!='open' OR NEW.state!='closed' OR NEW.revision!=OLD.revision+1
 OR NEW.root_id!=OLD.root_id OR NEW.role!=OLD.role OR NEW.attempt_id!=OLD.attempt_id
 OR NEW.approval_id!=OLD.approval_id OR NEW.review_id!=OLD.review_id OR NEW.review_revision!=OLD.review_revision
 OR NEW.scope_digest!=OLD.scope_digest OR NEW.principal!=OLD.principal OR NEW.requester!=OLD.requester OR NEW.executor!=OLD.executor
 OR NEW.environment_id!=OLD.environment_id OR NEW.registration_digest!=OLD.registration_digest OR NEW.binding_digest!=OLD.binding_digest
 OR NEW.profile_digest!=OLD.profile_digest OR NEW.qualification_id!=OLD.qualification_id OR NEW.qualification_digest!=OLD.qualification_digest
 OR NEW.policy_digest!=OLD.policy_digest OR NEW.runtime_generation!=OLD.runtime_generation
 OR NEW.created_at!=OLD.created_at OR NEW.expires_at!=OLD.expires_at OR NEW.audit_digest!=OLD.audit_digest OR NEW.audit_json!=OLD.audit_json
BEGIN SELECT RAISE(ABORT,'physical attempt is immutable or already closed'); END;
CREATE TRIGGER physical_reviews_keep BEFORE DELETE ON physical_reviews
BEGIN SELECT RAISE(ABORT,'physical review history required'); END;
CREATE TRIGGER physical_attempts_keep BEFORE DELETE ON physical_attempts
BEGIN SELECT RAISE(ABORT,'physical attempt history required'); END;
"#;

// These are canonical, validated audit snapshots, never current ingress/runtime
// proofs. Core has no From/TryFrom constructor consuming this representation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct RootAuditV1 {
    pub(in crate::physical) version: VersionV1,
    pub(in crate::physical) root_id: RootId,
    pub(in crate::physical) attempt_id: AttemptId,
    pub(in crate::physical) review_id: ReviewId,
    pub(in crate::physical) review_revision: u64,
    pub(in crate::physical) approval: ApprovalCorrelationV1,
    pub(in crate::physical) scope_digest: DigestV1,
    pub(in crate::physical) principal: LabelV1,
    pub(in crate::physical) requester: HostRef,
    pub(in crate::physical) executor: HostRef,
    pub(in crate::physical) environment: EnvironmentRefV1,
    pub(in crate::physical) registration_digest: DigestV1,
    pub(in crate::physical) epochs: BTreeMap<DomainId, u64>,
    pub(in crate::physical) binding_digest: DigestV1,
    pub(in crate::physical) profile_digest: DigestV1,
    pub(in crate::physical) qualification_id: QualificationId,
    pub(in crate::physical) qualification_digest: DigestV1,
    pub(in crate::physical) policy_digest: DigestV1,
    pub(in crate::physical) runtime_generation: String,
    pub(in crate::physical) created_at: UnixMillis,
    pub(in crate::physical) expires_at: UnixMillis,
}
impl RootAuditV1 {
    fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-root-audit-v1", self)
    }
    pub(super) fn validate(&self, review: &PhysicalReviewRecordV1) -> AppResult<()> {
        let s = review.scope.fields();
        validate_host(&self.requester)?;
        validate_host(&self.executor)?;
        let generation = self
            .runtime_generation
            .strip_prefix("local-runtime:v1:")
            .unwrap_or("");
        let uuid = uuid::Uuid::parse_str(generation).ok();
        require(
            uuid.is_some_and(|id| id.get_version_num() == 4 && id.to_string() == generation),
            "Invalid runtime audit correlation",
        )?;
        require(
            self.review_id == review.review_id
                && self.review_revision == review.revision
                && Some(&self.approval) == review.approval.as_ref()
                && self.scope_digest == review.scope_digest
                && self.principal == s.principal
                && self.principal == self.approval.principal
                && self.requester == s.requester
                && self.executor == s.executor
                && self.requester == self.executor
                && self.environment == s.environment.environment
                && self.binding_digest == s.environment.digest()?
                && self.profile_digest == s.profile.digest()?
                && self.qualification_id == s.qualification.qualification_id
                && self.qualification_digest == s.qualification.digest()?
                && self.created_at >= self.approval.approved_at
                && self.created_at < self.expires_at
                && self.expires_at <= self.approval.expires_at
                && self.expires_at <= s.environment.offer_expiry
                && self.expires_at <= s.qualification.expires_at
                && self.epochs.len() == s.environment.domains().len()
                && self
                    .epochs
                    .values()
                    .all(|e| *e >= 1 && *e <= i64::MAX as u64)
                && s.environment
                    .domains()
                    .iter()
                    .all(|d| self.epochs.contains_key(*d)),
            "Root audit lineage mismatch",
        )
    }
}

pub(super) fn verify_version(conn: &Connection) -> AppResult<()> {
    let version: Vec<i64> = conn
        .prepare("SELECT version FROM physical_core_schema WHERE singleton=1")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    require(version == [1], "Incompatible physical Core ledger version")
}
fn load_review(
    conn: &Connection,
    id: &ReviewId,
    revision: u64,
) -> AppResult<PhysicalReviewRecordV1> {
    let raw: String = conn.query_row(
        "SELECT record_json FROM physical_reviews WHERE review_id=?1 AND revision=?2",
        params![text(id), checked_integer(revision)?],
        |r| r.get(0),
    )?;
    let r: PhysicalReviewRecordV1 = decode(&raw)?;
    r.validate()?;
    Ok(r)
}
pub(super) fn current_review(
    conn: &Connection,
    id: &ReviewId,
    revision: u64,
) -> AppResult<PhysicalReviewRecordV1> {
    let latest: i64 = conn.query_row(
        "SELECT max(revision) FROM physical_reviews WHERE review_id=?1",
        [text(id)],
        |r| r.get(0),
    )?;
    require(
        latest == checked_integer(revision)?,
        "Superseded physical review",
    )?;
    load_review(conn, id, revision)
}
fn insert_review(conn: &Connection, r: &PhysicalReviewRecordV1) -> AppResult<()> {
    r.validate()?;
    conn.execute("INSERT INTO physical_reviews(review_id,revision,scope_digest,scope_json,state,state_revision,record_json) VALUES(?1,?2,?3,?4,?5,1,?6)",params![text(&r.review_id),checked_integer(r.revision)?,text(&r.scope_digest),serde_json::to_string(&r.scope)?,tag(&r.state)?,serde_json::to_string(r)?])?;
    Ok(())
}
fn update_review(
    conn: &Connection,
    r: &PhysicalReviewRecordV1,
    expected: PhysicalReviewStateV1,
) -> AppResult<()> {
    r.validate()?;
    let a = r.approval.as_ref();
    let n=conn.execute("UPDATE physical_reviews SET state=?3,state_revision=state_revision+1,approval_id=?4,approval_principal=?5,approved_at=?6,approval_expiry=?7,record_json=?8 WHERE review_id=?1 AND revision=?2 AND state=?9",params![text(&r.review_id),checked_integer(r.revision)?,tag(&r.state)?,a.map(|a|text(&a.approval_id)),a.map(|a|text(&a.principal)),a.map(|a|a.approved_at.get() as i64),a.map(|a|a.expires_at.get() as i64),serde_json::to_string(r)?,tag(&expected)?])?;
    require(n == 1, "Concurrent/invalid physical review transition")
}
pub(super) fn dependencies(
    conn: &Connection,
    scope: &PhysicalReviewScopeV1,
    snapshot: &BindingLedgerSnapshotV1,
    now: UnixMillis,
) -> AppResult<()> {
    let s = scope.fields();
    s.validate()?;
    let reg = load_registration(conn, snapshot.environment(), true)?;
    require(
        reg.digest()? == *snapshot.registration_digest()
            && *snapshot.environment() == s.environment.environment
            && reg.host == s.executor
            && reg.revision == s.environment.registration_revision,
        "Current enrollment mismatch",
    )?;
    for (d, e) in snapshot.epochs() {
        let actual: i64 = conn.query_row(
            "SELECT epoch FROM physical_domains WHERE domain_id=?1",
            [text(d)],
            |r| r.get(0),
        )?;
        require(
            actual == checked_integer(*e)?,
            "Current domain epoch mismatch",
        )?;
    }
    require(
        snapshot.epochs().len() == s.environment.domains().len()
            && s.environment
                .domains()
                .iter()
                .all(|d| snapshot.epochs().contains_key(*d)),
        "Missing binding domain",
    )?;
    let (raw,withdrawal,env,registration):(String,i64,String,String)=conn.query_row("SELECT record_json,withdrawal_revision,environment_id,registration_digest FROM physical_qualifications WHERE qualification_id=?1",[text(&s.qualification.qualification_id)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let q: PhysicalQualificationV1 = decode(&raw)?;
    require(
        q == s.qualification
            && withdrawal == 0
            && now < q.expires_at
            && now < s.environment.offer_expiry
            && env == text(snapshot.environment())
            && registration == text(snapshot.registration_digest()),
        "Current qualification/binding mismatch",
    )?;
    q.validate_for(&s.profile, &s.environment)
}

impl PhysicalStoreV1 {
    pub(in crate::physical) fn create_review(
        &self,
        r: &PhysicalReviewRecordV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<()> {
        require(
            r.state == PhysicalReviewStateV1::Draft && r.revision == 1 && r.approval.is_none(),
            "Invalid initial review",
        )?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        dependencies(&tx, &r.scope, snapshot, now)?;
        insert_review(&tx, r)?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn review(
        &self,
        id: &ReviewId,
        revision: u64,
    ) -> AppResult<PhysicalReviewRecordV1> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let r = load_review(&tx, id, revision)?;
        tx.commit()?;
        Ok(r)
    }
    pub(in crate::physical) fn review_for_approval(
        &self,
        id: &ApprovalId,
    ) -> AppResult<PhysicalReviewRecordV1> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let (id, rev): (String, i64) = tx.query_row(
            "SELECT review_id,revision FROM physical_reviews WHERE approval_id=?1",
            [text(id)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let r = current_review(&tx, &ReviewId::try_from(id)?, rev as u64)?;
        tx.commit()?;
        Ok(r)
    }
    pub(in crate::physical) fn transition_review(
        &self,
        id: &ReviewId,
        revision: u64,
        digest: &DigestV1,
        next: PhysicalReviewStateV1,
        approval: Option<ApprovalCorrelationV1>,
    ) -> AppResult<PhysicalReviewRecordV1> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let mut r = current_review(&tx, id, revision)?;
        require(r.scope_digest == *digest, "Approval/review digest mismatch")?;
        let previous = r.state;
        require(
            matches!(
                (previous, next),
                (
                    PhysicalReviewStateV1::Draft,
                    PhysicalReviewStateV1::Reviewed
                        | PhysicalReviewStateV1::Rejected
                        | PhysicalReviewStateV1::Expired
                ) | (
                    PhysicalReviewStateV1::Reviewed,
                    PhysicalReviewStateV1::Approved
                        | PhysicalReviewStateV1::Rejected
                        | PhysicalReviewStateV1::Expired
                ) | (
                    PhysicalReviewStateV1::Approved,
                    PhysicalReviewStateV1::Expired
                )
            ),
            "Invalid review lifecycle",
        )?;
        if next == PhysicalReviewStateV1::Approved {
            let a = approval.ok_or_else(|| {
                crate::error::AppError::InvalidInput("Missing Core approval".into())
            })?;
            require(
                a.principal == r.scope.fields().principal,
                "Approval principal mismatch",
            )?;
            r.approval = Some(a);
        } else {
            require(approval.is_none(), "Unexpected approval replacement")?;
        }
        r.state = next;
        update_review(&tx, &r, previous)?;
        if matches!(
            next,
            PhysicalReviewStateV1::Expired | PhysicalReviewStateV1::Rejected
        ) {
            close_review(&tx, id, revision, "superseded")?;
        }
        tx.commit()?;
        Ok(r)
    }
    pub(in crate::physical) fn revise_review(
        &self,
        id: &ReviewId,
        expected: u64,
        scope: PhysicalReviewScopeV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<PhysicalReviewRecordV1> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let mut old = current_review(&tx, id, expected)?;
        dependencies(&tx, &scope, snapshot, now)?;
        if !matches!(
            old.state,
            PhysicalReviewStateV1::Rejected | PhysicalReviewStateV1::Expired
        ) {
            let state = old.state;
            old.state = PhysicalReviewStateV1::Expired;
            update_review(&tx, &old, state)?;
        }
        close_review(&tx, id, expected, "superseded")?;
        let revision = expected.checked_add(1).unwrap_or(u64::MAX);
        checked_integer(revision)?;
        let r = PhysicalReviewRecordV1 {
            version: VersionV1,
            review_id: id.clone(),
            revision,
            scope_digest: scope.digest()?,
            scope,
            state: PhysicalReviewStateV1::Draft,
            approval: None,
        };
        insert_review(&tx, &r)?;
        tx.commit()?;
        Ok(r)
    }
    /// Approval consumption and audit insertion are one immediate transaction.
    /// This returns no Root and cannot be used to rebuild one.
    pub(in crate::physical) fn originate_attempt(
        &self,
        a: &RootAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let r = current_review(&tx, &a.review_id, a.review_revision)?;
        a.validate(&r)?;
        require(
            r.state == PhysicalReviewStateV1::Approved
                && now >= a.approval.approved_at
                && now < a.expires_at
                && now < a.approval.expires_at
                && a.registration_digest == *snapshot.registration_digest()
                && a.epochs == *snapshot.epochs(),
            "Approval/start no longer current",
        )?;
        dependencies(&tx, &r.scope, snapshot, now)?;
        tx.execute("INSERT INTO physical_attempts(root_id,role,attempt_id,approval_id,review_id,review_revision,scope_digest,principal,requester,executor,environment_id,registration_digest,binding_digest,profile_digest,qualification_id,qualification_digest,policy_digest,runtime_generation,created_at,expires_at,audit_digest,audit_json,state,revision) VALUES(?1,'requester_executor',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,'open',1)",params![text(&a.root_id),text(&a.attempt_id),text(&a.approval.approval_id),text(&a.review_id),checked_integer(a.review_revision)?,text(&a.scope_digest),text(&a.principal),a.requester.as_str(),a.executor.as_str(),text(&a.environment),text(&a.registration_digest),text(&a.binding_digest),text(&a.profile_digest),text(&a.qualification_id),text(&a.qualification_digest),text(&a.policy_digest),a.runtime_generation,a.created_at.get() as i64,a.expires_at.get() as i64,text(&a.digest()?),serde_json::to_string(a)?])?;
        tx.execute("UPDATE physical_reviews SET state_revision=state_revision+1 WHERE review_id=?1 AND revision=?2 AND state='approved'",params![text(&a.review_id),checked_integer(a.review_revision)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn validate_attempt(
        &self,
        a: &RootAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        validate_attempt_in(&tx, a, snapshot, now)?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn close_attempt(&self, id: &RootId, reason: &str) -> AppResult<()> {
        require(valid_reason(reason), "Invalid Root close reason")?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        super::control_ledger::close_root(&tx, id)?;
        tx.execute("UPDATE physical_attempts SET state='closed',revision=2,close_reason=?2 WHERE root_id=?1 AND state='open'",params![text(id),reason])?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn close_open_attempts(&self, reason: &str) -> AppResult<()> {
        require(valid_reason(reason), "Invalid Root close reason")?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        super::control_ledger::recover(&tx, reason == "interrupted")?;
        tx.execute("UPDATE physical_attempts SET state='closed',revision=2,close_reason=?1 WHERE state='open'",[reason])?;
        tx.commit()?;
        Ok(())
    }
}
fn valid_reason(s: &str) -> bool {
    matches!(
        s,
        "interrupted"
            | "shutdown"
            | "revoked"
            | "dependency_invalidated"
            | "superseded"
            | "expired"
    )
}
fn close_review(c: &Connection, id: &ReviewId, rev: u64, reason: &str) -> AppResult<()> {
    let roots:Vec<String> = c.prepare("SELECT root_id FROM physical_attempts WHERE review_id=?1 AND review_revision=?2 AND state='open'")?.query_map(params![text(id),checked_integer(rev)?],|r|r.get(0))?.collect::<Result<_,_>>()?;
    for root in roots {
        super::control_ledger::close_root(c, &RootId::try_from(root)?)?;
    }
    c.execute("UPDATE physical_attempts SET state='closed',revision=2,close_reason=?3 WHERE review_id=?1 AND review_revision=?2 AND state='open'",params![text(id),checked_integer(rev)?,reason])?;
    Ok(())
}
pub(super) fn audit(conn: &Connection) -> AppResult<()> {
    verify_version(conn)?;
    for table in ["physical_reviews", "physical_attempts"] {
        require(
            !conn
                .prepare(&format!("PRAGMA foreign_key_check({table})"))?
                .exists([])?,
            "Physical Core foreign key violation",
        )?;
    }
    let mut stmt=conn.prepare("SELECT review_id,revision,scope_digest,scope_json,state,state_revision,approval_id,approval_principal,approved_at,approval_expiry,record_json FROM physical_reviews")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let r: PhysicalReviewRecordV1 = decode(&row.get::<_, String>(10)?)?;
        r.validate()?;
        let a = r.approval.as_ref();
        require(
            row.get::<_, String>(0)? == text(&r.review_id)
                && row.get::<_, i64>(1)? == checked_integer(r.revision)?
                && row.get::<_, String>(2)? == text(&r.scope_digest)
                && row.get::<_, String>(3)? == serde_json::to_string(&r.scope)?
                && row.get::<_, String>(4)? == tag(&r.state)?
                && row.get::<_, i64>(5)? >= 1
                && row.get::<_, Option<String>>(6)? == a.map(|a| text(&a.approval_id))
                && row.get::<_, Option<String>>(7)? == a.map(|a| text(&a.principal))
                && row.get::<_, Option<i64>>(8)? == a.map(|a| a.approved_at.get() as i64)
                && row.get::<_, Option<i64>>(9)? == a.map(|a| a.expires_at.get() as i64)
                && a.is_none_or(|a| a.principal == r.scope.fields().principal),
            "Review column/body mismatch",
        )?;
        let starts: i64 = conn.query_row(
            "SELECT count(*) FROM physical_attempts WHERE approval_id=?1",
            [a.map(|a| text(&a.approval_id))],
            |r| r.get(0),
        )?;
        let lifecycle_revision: i64 = row.get(5)?;
        let coherent = match r.state {
            PhysicalReviewStateV1::Draft => lifecycle_revision == 1 && starts == 0,
            PhysicalReviewStateV1::Reviewed => lifecycle_revision == 2 && starts == 0,
            PhysicalReviewStateV1::Approved => lifecycle_revision == 3 + starts,
            PhysicalReviewStateV1::Rejected => matches!(lifecycle_revision, 2 | 3) && starts == 0,
            PhysicalReviewStateV1::Expired if a.is_some() => lifecycle_revision == 4 + starts,
            PhysicalReviewStateV1::Expired => matches!(lifecycle_revision, 2 | 3) && starts == 0,
        };
        require(
            starts <= 1 && coherent,
            "Unprovable review/start CAS history",
        )?;
        let prior: i64 = conn.query_row(
            "SELECT count(*) FROM physical_reviews WHERE review_id=?1 AND revision<=?2",
            params![text(&r.review_id), checked_integer(r.revision)?],
            |r| r.get(0),
        )?;
        require(
            prior == checked_integer(r.revision)?,
            "Missing review revision history",
        )?;
        let newer: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM physical_reviews WHERE review_id=?1 AND revision>?2)",
            params![text(&r.review_id), checked_integer(r.revision)?],
            |r| r.get(0),
        )?;
        require(
            !newer
                || matches!(
                    r.state,
                    PhysicalReviewStateV1::Rejected | PhysicalReviewStateV1::Expired
                ),
            "Superseded review still active",
        )?;
    }
    let mut stmt=conn.prepare("SELECT root_id,attempt_id,approval_id,review_id,review_revision,scope_digest,principal,requester,executor,environment_id,registration_digest,binding_digest,profile_digest,qualification_id,qualification_digest,policy_digest,runtime_generation,created_at,expires_at,audit_digest,audit_json,state,revision,close_reason,role FROM physical_attempts")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let a: RootAuditV1 = decode(&row.get::<_, String>(20)?)?;
        a.validate(&load_review(conn, &a.review_id, a.review_revision)?)?;
        let strings = [
            text(&a.root_id),
            text(&a.attempt_id),
            text(&a.approval.approval_id),
            text(&a.review_id),
            text(&a.scope_digest),
            text(&a.principal),
            a.requester.as_str().into(),
            a.executor.as_str().into(),
            text(&a.environment),
            text(&a.registration_digest),
            text(&a.binding_digest),
            text(&a.profile_digest),
            text(&a.qualification_id),
            text(&a.qualification_digest),
            text(&a.policy_digest),
            a.runtime_generation.clone(),
            text(&a.digest()?),
        ];
        let columns = [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 19];
        for (i, s) in columns.into_iter().zip(strings) {
            require(
                row.get::<_, String>(i)? == s,
                "Attempt column/body mismatch",
            )?;
        }
        let state: String = row.get(21)?;
        let rev: i64 = row.get(22)?;
        let reason: Option<String> = row.get(23)?;
        require(
            row.get::<_, i64>(4)? == checked_integer(a.review_revision)?
                && row.get::<_, i64>(17)? == a.created_at.get() as i64
                && row.get::<_, i64>(18)? == a.expires_at.get() as i64
                && row.get::<_, String>(24)? == "requester_executor"
                && ((state == "open" && rev == 1 && reason.is_none())
                    || (state == "closed"
                        && rev == 2
                        && reason.as_deref().is_some_and(valid_reason))),
            "Invalid attempt closure",
        )?;
    }
    Ok(())
}

pub(super) fn validate_attempt_in(
    conn: &Connection,
    a: &RootAuditV1,
    snapshot: &BindingLedgerSnapshotV1,
    now: UnixMillis,
) -> AppResult<()> {
    let r = current_review(conn, &a.review_id, a.review_revision)?;
    a.validate(&r)?;
    let (raw,state):(String,String)=conn.query_row("SELECT audit_json,state FROM physical_attempts WHERE root_id=?1 AND role='requester_executor'",[text(&a.root_id)],|r|Ok((r.get(0)?,r.get(1)?)))?;
    require(
        raw == serde_json::to_string(a)?
            && state == "open"
            && r.state == PhysicalReviewStateV1::Approved
            && now >= a.created_at
            && now < a.expires_at
            && *snapshot.registration_digest() == a.registration_digest
            && (*snapshot.epochs() == a.epochs
                || super::control_ledger::owns_epochs(conn, a, snapshot)?),
        "Closed/stale physical Root audit",
    )?;
    dependencies(conn, &r.scope, snapshot, now)?;
    Ok(())
}
