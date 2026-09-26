//! L2-L6 durable control facts in the existing physical store. No live handles.
use super::*;
use crate::physical::{binding::BindingLedgerSnapshotV1, contracts::*, core::validate_narrowing};
use serde::Deserialize;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_control_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_control_schema VALUES(1,1);
CREATE TABLE physical_sessions(
 session_id TEXT PRIMARY KEY,root_id TEXT NOT NULL UNIQUE,role TEXT NOT NULL CHECK(role='requester_executor'),
 installation_id TEXT NOT NULL UNIQUE,binding_digest TEXT NOT NULL,profile_digest TEXT NOT NULL,qualification_digest TEXT NOT NULL,
 scope_digest TEXT NOT NULL,required_enforcement TEXT NOT NULL CHECK(required_enforcement IN ('adapter_isolation_only','native_fence')),
 lease_expiry INTEGER NOT NULL CHECK(lease_expiry>0),audit_digest TEXT NOT NULL,audit_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('installing','active','quarantined')),revision INTEGER NOT NULL CHECK(revision>=1),
 install_evidence TEXT CHECK(install_evidence='adapter_isolation_only'),fence_json TEXT,fence_ack TEXT CHECK(fence_ack='adapter_isolation_only'),
 FOREIGN KEY(root_id,role) REFERENCES physical_attempts(root_id,role),
 CHECK(state!='active' OR install_evidence IS NOT NULL),CHECK(state!='quarantined' OR fence_json IS NOT NULL),CHECK(fence_ack IS NULL OR state='quarantined')
) STRICT;
CREATE TABLE physical_domain_reservations(domain_id TEXT PRIMARY KEY REFERENCES physical_domains(domain_id),session_id TEXT NOT NULL REFERENCES physical_sessions(session_id),epoch INTEGER NOT NULL CHECK(epoch>0),state TEXT NOT NULL CHECK(state IN ('held','quarantined'))) STRICT;
CREATE TABLE physical_control_budgets(root_id TEXT PRIMARY KEY,role TEXT NOT NULL CHECK(role='requester_executor'),ceiling_us INTEGER NOT NULL CHECK(ceiling_us>0),ceiling_count INTEGER NOT NULL CHECK(ceiling_count=1),reserved_us INTEGER NOT NULL DEFAULT 0 CHECK(reserved_us>=0 AND reserved_us<=ceiling_us),reserved_count INTEGER NOT NULL DEFAULT 0 CHECK(reserved_count BETWEEN 0 AND ceiling_count),consumed_us INTEGER NOT NULL DEFAULT 0 CHECK(consumed_us>=0 AND consumed_us<=reserved_us),revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>=1),FOREIGN KEY(root_id,role) REFERENCES physical_attempts(root_id,role)) STRICT;
CREATE TABLE physical_actions(
 action_id TEXT PRIMARY KEY,root_id TEXT NOT NULL UNIQUE,session_id TEXT NOT NULL UNIQUE REFERENCES physical_sessions(session_id),grant_id TEXT NOT NULL UNIQUE,
 decision_sequence INTEGER NOT NULL CHECK(decision_sequence=1),payload_digest TEXT NOT NULL,proposal_digest TEXT NOT NULL,
 challenge_id TEXT NOT NULL UNIQUE,requested_us INTEGER NOT NULL CHECK(requested_us>0),reserved_us INTEGER NOT NULL CHECK(reserved_us>=requested_us),
 expires_at INTEGER NOT NULL CHECK(expires_at>0),audit_digest TEXT NOT NULL,audit_json TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('open','closed')),revision INTEGER NOT NULL CHECK(revision>=1),
 dispatch_intent INTEGER NOT NULL DEFAULT 0 CHECK(dispatch_intent IN (0,1)),
 disposition TEXT NOT NULL CHECK(disposition IN ('not_sent','dispatch_unknown','fake_accepted','fake_refused')),
 apply_result TEXT CHECK(apply_result IN ('accepted','refused')),operation_id TEXT,refresh_sequence INTEGER NOT NULL DEFAULT 0 CHECK(refresh_sequence>=0),
 UNIQUE(root_id,decision_sequence),CHECK(dispatch_intent=1 OR (disposition='not_sent' AND apply_result IS NULL AND refresh_sequence=0)),
 CHECK(disposition!='fake_accepted' OR apply_result='accepted'),CHECK(disposition!='fake_refused' OR apply_result='refused')
) STRICT;
CREATE TRIGGER physical_session_monotonic BEFORE UPDATE ON physical_sessions
WHEN OLD.state='quarantined' AND (NEW.state!=OLD.state OR NEW.fence_json IS NOT OLD.fence_json)
 OR NEW.revision!=OLD.revision+1 OR NEW.audit_json!=OLD.audit_json OR NEW.audit_digest!=OLD.audit_digest
 OR NEW.session_id!=OLD.session_id OR NEW.root_id!=OLD.root_id OR NEW.role!=OLD.role OR NEW.installation_id!=OLD.installation_id
 OR NEW.binding_digest!=OLD.binding_digest OR NEW.profile_digest!=OLD.profile_digest OR NEW.qualification_digest!=OLD.qualification_digest
 OR NEW.scope_digest!=OLD.scope_digest OR NEW.required_enforcement!=OLD.required_enforcement OR NEW.lease_expiry!=OLD.lease_expiry
 OR (OLD.state='active' AND NEW.state='installing') OR (OLD.install_evidence IS NOT NULL AND NEW.install_evidence IS NOT OLD.install_evidence)
 OR (OLD.fence_ack IS NOT NULL AND NEW.fence_ack IS NOT OLD.fence_ack)
BEGIN SELECT RAISE(ABORT,'physical session regression');END;
CREATE TRIGGER physical_reservation_monotonic BEFORE UPDATE ON physical_domain_reservations
WHEN OLD.state='quarantined' OR NEW.state!='quarantined' OR NEW.domain_id!=OLD.domain_id OR NEW.session_id!=OLD.session_id OR NEW.epoch!=OLD.epoch
BEGIN SELECT RAISE(ABORT,'physical domain holder regression');END;
CREATE TRIGGER physical_budget_monotonic BEFORE UPDATE ON physical_control_budgets
WHEN NEW.root_id!=OLD.root_id OR NEW.role!=OLD.role OR NEW.ceiling_us!=OLD.ceiling_us OR NEW.ceiling_count!=OLD.ceiling_count
 OR NEW.revision!=OLD.revision+1 OR NEW.consumed_us<OLD.consumed_us OR (OLD.consumed_us>0 AND (NEW.reserved_us<OLD.reserved_us OR NEW.reserved_count<OLD.reserved_count))
BEGIN SELECT RAISE(ABORT,'physical budget regression');END;
CREATE TRIGGER physical_action_monotonic BEFORE UPDATE ON physical_actions
WHEN OLD.state='closed' OR NEW.revision!=OLD.revision+1 OR NEW.audit_json!=OLD.audit_json OR NEW.audit_digest!=OLD.audit_digest
 OR NEW.action_id!=OLD.action_id OR NEW.root_id!=OLD.root_id OR NEW.session_id!=OLD.session_id OR NEW.grant_id!=OLD.grant_id
 OR NEW.decision_sequence!=OLD.decision_sequence OR NEW.payload_digest!=OLD.payload_digest OR NEW.proposal_digest!=OLD.proposal_digest OR NEW.challenge_id!=OLD.challenge_id
 OR NEW.requested_us!=OLD.requested_us OR NEW.reserved_us!=OLD.reserved_us OR NEW.expires_at!=OLD.expires_at
 OR NEW.dispatch_intent<OLD.dispatch_intent OR NEW.refresh_sequence<OLD.refresh_sequence
 OR (OLD.apply_result IS NOT NULL AND NEW.apply_result IS NOT OLD.apply_result)
BEGIN SELECT RAISE(ABORT,'physical action regression');END;
CREATE TRIGGER physical_sessions_keep BEFORE DELETE ON physical_sessions BEGIN SELECT RAISE(ABORT,'physical session history required');END;
CREATE TRIGGER physical_reservations_keep BEFORE DELETE ON physical_domain_reservations BEGIN SELECT RAISE(ABORT,'physical domain denial required');END;
CREATE TRIGGER physical_budgets_keep BEFORE DELETE ON physical_control_budgets BEGIN SELECT RAISE(ABORT,'physical budget history required');END;
CREATE TRIGGER physical_actions_keep BEFORE DELETE ON physical_actions BEGIN SELECT RAISE(ABORT,'physical action history required');END;
"#;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct SessionAuditV1 {
    pub(in crate::physical) version: VersionV1,
    pub(in crate::physical) id: SessionId,
    pub(in crate::physical) root: RootId,
    pub(in crate::physical) installation: RequestId,
    pub(in crate::physical) binding_digest: DigestV1,
    pub(in crate::physical) profile_digest: DigestV1,
    pub(in crate::physical) qualification_digest: DigestV1,
    pub(in crate::physical) scope: PhysicalReviewScopeV1,
    pub(in crate::physical) enforcement: SessionEnforcementClassV1,
    pub(in crate::physical) previous: BTreeMap<DomainId, u64>,
    pub(in crate::physical) epochs: BTreeMap<DomainId, u64>,
    pub(in crate::physical) lease_expiry: UnixMillis,
}
impl SessionAuditV1 {
    fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-session-audit-v1", self)
    }
    fn validate(&self, a: &RootAuditV1, r: &PhysicalReviewRecordV1) -> AppResult<()> {
        validate_narrowing(&r.scope, &self.scope)?;
        let s = self.scope.fields();
        require(
            self.root == a.root_id
                && self.binding_digest == a.binding_digest
                && self.profile_digest == a.profile_digest
                && self.qualification_digest == a.qualification_digest
                && self.previous == a.epochs
                && self.epochs.len() == self.previous.len()
                && self.epochs.iter().all(|(d, e)| {
                    self.previous
                        .get(d)
                        .is_some_and(|old| *e > *old && *e <= i64::MAX as u64)
                })
                && self.enforcement.meets(s.profile.required_enforcement_class)
                && self
                    .enforcement
                    .meets(s.qualification.required_enforcement_class)
                && s.qualification
                    .required_enforcement_class
                    .meets(self.enforcement)
                && self.lease_expiry <= a.expires_at
                && self.lease_expiry > a.created_at,
            "Session lineage mismatch",
        )
    }
}
/// Sealed L2 receipt: only this transaction module constructs it. No serde.
pub(in crate::physical) struct ReservationReceiptV1 {
    session: SessionAuditV1,
}
impl ReservationReceiptV1 {
    pub(in crate::physical) fn session(&self) -> &SessionAuditV1 {
        &self.session
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct ActionAuditV1 {
    pub(in crate::physical) version: VersionV1,
    pub(in crate::physical) grant: GrantId,
    pub(in crate::physical) session: SessionId,
    pub(in crate::physical) root: RootId,
    pub(in crate::physical) proposal: PhysicalActionProposalV1,
    pub(in crate::physical) epochs: BTreeMap<DomainId, u64>,
    pub(in crate::physical) reserved_us: u64,
    pub(in crate::physical) expires_at: UnixMillis,
    pub(in crate::physical) completion_digest: DigestV1,
    pub(in crate::physical) loss_digest: DigestV1,
}
impl ActionAuditV1 {
    fn digest(&self) -> AppResult<DigestV1> {
        digest("pastey-physical-action-audit-v1", self)
    }
    fn validate(&self, s: &SessionAuditV1, a: &RootAuditV1) -> AppResult<()> {
        self.proposal.validate_scope(&s.scope)?;
        require(
            self.root == s.root
                && self.session == s.id
                && self.epochs == s.epochs
                && self.proposal.attempt_id == a.attempt_id
                && self.proposal.decision_sequence == 1
                && self.reserved_us == s.scope.fields().execution.action_duration_us.get()
                && self.reserved_us <= s.scope.fields().execution.total_execution_us.get()
                && self.expires_at <= s.lease_expiry
                && self.expires_at > a.created_at
                && self.completion_digest
                    == digest(
                        "pastey-physical-completion-v1",
                        &s.scope.fields().completion,
                    )?
                && self.loss_digest == digest("pastey-physical-loss-v1", &s.scope.fields().loss)?,
            "Action audit lineage mismatch",
        )
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::physical) struct FenceAuditV1 {
    pub(in crate::physical) session: SessionId,
    pub(in crate::physical) request: RequestId,
    pub(in crate::physical) epochs: BTreeMap<DomainId, u64>,
}
pub(super) fn verify_version(c: &Connection) -> AppResult<()> {
    let v: Vec<i64> = c
        .prepare("SELECT version FROM physical_control_schema WHERE singleton=1")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    require(v == [1], "Incompatible control ledger version")
}
fn root(c: &Connection, id: &RootId) -> AppResult<RootAuditV1> {
    let raw: String = c.query_row(
        "SELECT audit_json FROM physical_attempts WHERE root_id=?1 AND role='requester_executor'",
        [text(id)],
        |r| r.get(0),
    )?;
    decode(&raw)
}
fn session(c: &Connection, id: &SessionId) -> AppResult<SessionAuditV1> {
    let raw: String = c.query_row(
        "SELECT audit_json FROM physical_sessions WHERE session_id=?1",
        [text(id)],
        |r| r.get(0),
    )?;
    decode(&raw)
}
fn action(c: &Connection, id: &ActionId) -> AppResult<ActionAuditV1> {
    let raw: String = c.query_row(
        "SELECT audit_json FROM physical_actions WHERE action_id=?1",
        [text(id)],
        |r| r.get(0),
    )?;
    decode(&raw)
}
pub(super) fn owns_epochs(
    c: &Connection,
    a: &RootAuditV1,
    snapshot: &BindingLedgerSnapshotV1,
) -> AppResult<bool> {
    let raw:Option<String>=c.query_row("SELECT audit_json FROM physical_sessions WHERE root_id=?1 AND state IN ('installing','active')",[text(&a.root_id)],|r|r.get(0)).optional()?;
    match raw {
        None => Ok(false),
        Some(raw) => {
            let s: SessionAuditV1 = decode(&raw)?;
            Ok(s.root == a.root_id && s.previous == a.epochs && s.epochs == *snapshot.epochs())
        }
    }
}
fn current_session(
    c: &Connection,
    a: &RootAuditV1,
    s: &SessionAuditV1,
    snapshot: &BindingLedgerSnapshotV1,
    now: UnixMillis,
    active: bool,
) -> AppResult<()> {
    super::core_ledger::validate_attempt_in(c, a, snapshot, now)?;
    let (raw, state): (String, String) = c.query_row(
        "SELECT audit_json,state FROM physical_sessions WHERE session_id=?1",
        [text(&s.id)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    require(
        raw == serde_json::to_string(s)?
            && s.epochs == *snapshot.epochs()
            && now < s.lease_expiry
            && if active {
                state == "active"
            } else {
                state == "installing"
            },
        "Session reservation no longer current",
    )?;
    for (d, e) in &s.epochs {
        let held:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM physical_domain_reservations WHERE domain_id=?1 AND session_id=?2 AND epoch=?3 AND state='held')",params![text(d),text(&s.id),checked_integer(*e)?],|r|r.get(0))?;
        require(held, "Lost domain reservation")?;
    }
    Ok(())
}
impl PhysicalStoreV1 {
    pub(in crate::physical) fn reserve_session(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<ReservationReceiptV1> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        super::core_ledger::validate_attempt_in(&tx, a, snapshot, now)?;
        let r = super::core_ledger::current_review(&tx, &a.review_id, a.review_revision)?;
        s.validate(a, &r)?;
        require(
            s.previous == *snapshot.epochs() && now < s.lease_expiry,
            "Stale session reservation",
        )?;
        tx.execute("INSERT INTO physical_sessions(session_id,root_id,role,installation_id,binding_digest,profile_digest,qualification_digest,scope_digest,required_enforcement,lease_expiry,audit_digest,audit_json,state,revision) VALUES(?1,?2,'requester_executor',?3,?4,?5,?6,?7,?8,?9,?10,?11,'installing',1)",params![text(&s.id),text(&s.root),text(&s.installation),text(&s.binding_digest),text(&s.profile_digest),text(&s.qualification_digest),text(&s.scope.digest()?),tag(&s.enforcement)?,s.lease_expiry.get() as i64,text(&s.digest()?),serde_json::to_string(s)?])?;
        for (d, e) in &s.epochs {
            let n = tx.execute(
                "UPDATE physical_domains SET epoch=?2 WHERE domain_id=?1 AND epoch=?3",
                params![
                    text(d),
                    checked_integer(*e)?,
                    checked_integer(s.previous[d])?
                ],
            )?;
            require(n == 1, "Concurrent/stale domain epoch")?;
            tx.execute(
                "INSERT INTO physical_domain_reservations VALUES(?1,?2,?3,'held')",
                params![text(d), text(&s.id), checked_integer(*e)?],
            )?;
        }
        tx.execute("INSERT INTO physical_control_budgets(root_id,role,ceiling_us,ceiling_count) VALUES(?1,'requester_executor',?2,1)",params![text(&s.root),checked_integer(s.scope.fields().execution.total_execution_us.get())?])?;
        super::audit(&tx)?;
        tx.commit()?;
        Ok(ReservationReceiptV1 { session: s.clone() })
    }
    pub(in crate::physical) fn validate_session(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
        active: bool,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        current_session(&tx, a, s, snapshot, now, active)?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn activate_session(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
        request: &RequestId,
        epochs: &BTreeMap<DomainId, u64>,
        evidence: SessionEnforcementClassV1,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        current_session(&tx, a, s, snapshot, now, false)?;
        require(
            s.installation == *request
                && s.epochs == *epochs
                && evidence == SessionEnforcementClassV1::AdapterIsolationOnly
                && evidence.meets(s.enforcement),
            "Invalid installation evidence",
        )?;
        let n=tx.execute("UPDATE physical_sessions SET state='active',install_evidence='adapter_isolation_only',revision=revision+1 WHERE session_id=?1 AND state='installing'",[text(&s.id)])?;
        require(n == 1, "Stale installation response")?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn admit_action(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        x: &ActionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        current_session(&tx, a, s, snapshot, now, true)?;
        x.validate(s, a)?;
        require(now < x.expires_at, "Action lifetime exhausted")?;
        let n=tx.execute("UPDATE physical_control_budgets SET reserved_us=reserved_us+?2,reserved_count=reserved_count+1,revision=revision+1 WHERE root_id=?1 AND reserved_count=0 AND reserved_us+?2<=ceiling_us AND consumed_us=0",params![text(&a.root_id),checked_integer(x.reserved_us)?])?;
        require(n == 1, "Insufficient cumulative budget")?;
        tx.execute("INSERT INTO physical_actions(action_id,root_id,session_id,grant_id,decision_sequence,payload_digest,proposal_digest,challenge_id,requested_us,reserved_us,expires_at,audit_digest,audit_json,state,revision,disposition) VALUES(?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?10,?11,?12,'open',1,'not_sent')",params![text(&x.proposal.action_id),text(&x.root),text(&x.session),text(&x.grant),text(&x.proposal.payload_digest),text(&digest("pastey-physical-proposal-v1",&x.proposal)?),text(&x.proposal.challenge_id),checked_integer(x.proposal.requested_duration_us.get())?,checked_integer(x.reserved_us)?,x.expires_at.get() as i64,text(&x.digest()?),serde_json::to_string(x)?])?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn prepare_write(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        x: &ActionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
        op: &RequestId,
        refresh: bool,
    ) -> AppResult<()> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        current_session(&tx, a, s, snapshot, now, true)?;
        require(
            action(&tx, &x.proposal.action_id)? == *x,
            "Action immutable snapshot mismatch",
        )?;
        require(now < x.expires_at, "Action expired")?;
        let n = if refresh {
            tx.execute("UPDATE physical_actions SET operation_id=?2,refresh_sequence=refresh_sequence+1,revision=revision+1 WHERE action_id=?1 AND state='open' AND disposition='fake_accepted' AND operation_id IS NULL",params![text(&x.proposal.action_id),text(op)])?
        } else {
            tx.execute("UPDATE physical_actions SET dispatch_intent=1,disposition='dispatch_unknown',operation_id=?2,revision=revision+1 WHERE action_id=?1 AND state='open' AND dispatch_intent=0 AND operation_id IS NULL",params![text(&x.proposal.action_id),text(op)])?
        };
        require(n == 1, "Duplicate/uncertain/in-flight dispatch")?;
        if !refresh {
            tx.execute("UPDATE physical_control_budgets SET consumed_us=reserved_us,revision=revision+1 WHERE root_id=?1",[text(&x.root)])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn finish_write(
        &self,
        a: &RootAuditV1,
        s: &SessionAuditV1,
        x: &ActionAuditV1,
        snapshot: &BindingLedgerSnapshotV1,
        now: UnixMillis,
        op: &RequestId,
        result: Option<bool>,
        refresh: bool,
    ) -> AppResult<bool> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        current_session(&tx, a, s, snapshot, now, true)?;
        require(
            action(&tx, &x.proposal.action_id)? == *x && now < x.expires_at,
            "Closed/mismatched action callback",
        )?;
        let disposition = if refresh {
            if result == Some(true) {
                "fake_accepted"
            } else {
                "dispatch_unknown"
            }
        } else {
            match result {
                Some(true) => "fake_accepted",
                Some(false) => "fake_refused",
                None => "dispatch_unknown",
            }
        };
        let n=tx.execute("UPDATE physical_actions SET disposition=?3,apply_result=COALESCE(apply_result,?4),operation_id=NULL,revision=revision+1 WHERE action_id=?1 AND state='open' AND operation_id=?2",params![text(&x.proposal.action_id),text(op),disposition,if refresh{None}else{result.map(|b|if b{"accepted"}else{"refused"})}])?;
        tx.commit()?;
        Ok(n == 1)
    }
    pub(in crate::physical) fn action_status(
        &self,
        id: &ActionId,
    ) -> AppResult<(String, String, u64, u64)> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let v=tx.query_row("SELECT a.state,a.disposition,b.reserved_us,b.consumed_us FROM physical_actions a JOIN physical_control_budgets b USING(root_id) WHERE action_id=?1",[text(id)],|r|Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)? as u64,r.get::<_,i64>(3)? as u64)))?;
        tx.commit()?;
        Ok(v)
    }
    pub(in crate::physical) fn fence_request(&self, id: &SessionId) -> AppResult<FenceAuditV1> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let raw: String = tx.query_row(
            "SELECT fence_json FROM physical_sessions WHERE session_id=?1 AND state='quarantined'",
            [text(id)],
            |r| r.get(0),
        )?;
        let f = decode(&raw)?;
        tx.commit()?;
        Ok(f)
    }
    pub(in crate::physical) fn acknowledge_fence(&self, f: &FenceAuditV1) -> AppResult<bool> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let n=tx.execute("UPDATE physical_sessions SET fence_ack='adapter_isolation_only',revision=revision+1 WHERE session_id=?1 AND state='quarantined' AND fence_json=?2 AND fence_ack IS NULL",params![text(&f.session),serde_json::to_string(f)?])?;
        tx.commit()?;
        Ok(n == 1)
    }
}
pub(super) fn close_root(c: &Connection, id: &RootId) -> AppResult<()> {
    let raw: Option<String> = c
        .query_row(
            "SELECT audit_json FROM physical_sessions WHERE root_id=?1 AND state!='quarantined'",
            [text(id)],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = raw {
        let s: SessionAuditV1 = decode(&raw)?;
        let mut epochs = BTreeMap::new();
        for d in s.epochs.keys() {
            let old: i64 = c.query_row(
                "SELECT epoch FROM physical_domains WHERE domain_id=?1",
                [text(d)],
                |r| r.get(0),
            )?;
            let next = checked_integer((old as u64).checked_add(1).unwrap_or(u64::MAX))?;
            c.execute(
                "UPDATE physical_domains SET epoch=?2 WHERE domain_id=?1",
                params![text(d), next],
            )?;
            epochs.insert(d.clone(), next as u64);
        }
        let fence = FenceAuditV1 {
            session: s.id.clone(),
            request: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))?,
            epochs,
        };
        c.execute("UPDATE physical_sessions SET state='quarantined',fence_json=?2,revision=revision+1 WHERE session_id=?1 AND state!='quarantined'",params![text(&s.id),serde_json::to_string(&fence)?])?;
        c.execute("UPDATE physical_domain_reservations SET state='quarantined' WHERE session_id=?1 AND state='held'",[text(&s.id)])?;
        // Only rows that provably never reached dispatch intent release reservation.
        c.execute("UPDATE physical_control_budgets SET reserved_us=0,reserved_count=0,revision=revision+1 WHERE root_id=?1 AND consumed_us=0",[text(id)])?;
        c.execute("UPDATE physical_actions SET state='closed',operation_id=NULL,revision=revision+1 WHERE root_id=?1 AND state='open'",[text(id)])?;
    }
    Ok(())
}
pub(super) fn recover(c: &Connection, restart: bool) -> AppResult<()> {
    if restart {
        c.execute("UPDATE physical_actions SET disposition='dispatch_unknown',operation_id=NULL,revision=revision+1 WHERE state='open' AND dispatch_intent=1 AND disposition!='fake_refused'",[])?;
    }
    let ids: Vec<String> = c
        .prepare("SELECT root_id FROM physical_sessions WHERE state!='quarantined'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        close_root(c, &RootId::try_from(id)?)?;
    }
    Ok(())
}
pub(super) fn audit(c: &Connection) -> AppResult<()> {
    verify_version(c)?;
    for table in [
        "physical_sessions",
        "physical_domain_reservations",
        "physical_control_budgets",
        "physical_actions",
    ] {
        require(
            !c.prepare(&format!("PRAGMA foreign_key_check({table})"))?
                .exists([])?,
            "Control ledger foreign key violation",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_sessions")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let s: SessionAuditV1 = decode(&row.get::<_, String>("audit_json")?)?;
        let a = root(c, &s.root)?;
        let r = super::core_ledger::current_review(c, &a.review_id, a.review_revision).or_else(
            |_| {
                let raw: String = c.query_row(
                    "SELECT record_json FROM physical_reviews WHERE review_id=?1 AND revision=?2",
                    params![text(&a.review_id), checked_integer(a.review_revision)?],
                    |r| r.get(0),
                )?;
                decode(&raw)
            },
        )?;
        s.validate(&a, &r)?;
        for (col, value) in [
            ("session_id", text(&s.id)),
            ("root_id", text(&s.root)),
            ("role", "requester_executor".into()),
            ("installation_id", text(&s.installation)),
            ("binding_digest", text(&s.binding_digest)),
            ("profile_digest", text(&s.profile_digest)),
            ("qualification_digest", text(&s.qualification_digest)),
            ("scope_digest", text(&s.scope.digest()?)),
            ("required_enforcement", tag(&s.enforcement)?),
            ("audit_digest", text(&s.digest()?)),
        ] {
            require(
                row.get::<_, String>(col)? == value,
                "Session column/body mismatch",
            )?;
        }
        require(
            row.get::<_, i64>("lease_expiry")? == s.lease_expiry.get() as i64,
            "Session expiry mismatch",
        )?;
        let actual: BTreeMap<String, i64> = c
            .prepare(
                "SELECT domain_id,epoch FROM physical_domain_reservations WHERE session_id=?1",
            )?
            .query_map([text(&s.id)], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        require(
            actual == s.epochs.iter().map(|(d, e)| (text(d), *e as i64)).collect(),
            "Domain reservation mismatch",
        )?;
        let state: String = row.get("state")?;
        let fence: Option<String> = row.get("fence_json")?;
        if state == "quarantined" {
            let f: FenceAuditV1 = decode(&fence.ok_or_else(|| {
                crate::error::AppError::InvalidInput("Missing fence intent".into())
            })?)?;
            require(
                f.session == s.id
                    && f.epochs.len() == s.epochs.len()
                    && f.epochs.iter().all(|(d, e)| {
                        s.epochs
                            .get(d)
                            .is_some_and(|old| *e > *old && *e <= i64::MAX as u64)
                    }),
                "Fence lineage mismatch",
            )?;
        } else {
            require(fence.is_none(), "Live session has fence intent")?;
        }
        if state == "active" {
            require(
                s.enforcement == SessionEnforcementClassV1::AdapterIsolationOnly,
                "Isolation cannot activate NativeFence",
            )?;
        }
        for (domain, reserved_epoch) in &s.epochs {
            let (phase, high_water): (String, i64) = c.query_row(
                "SELECT r.state,d.epoch FROM physical_domain_reservations r JOIN physical_domains d USING(domain_id) WHERE r.domain_id=?1 AND r.session_id=?2",
                params![text(domain), text(&s.id)], |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            require(
                phase
                    == if state == "quarantined" {
                        "quarantined"
                    } else {
                        "held"
                    }
                    && high_water >= checked_integer(*reserved_epoch)?,
                "Domain state/high-water mismatch",
            )?;
            if state == "quarantined" {
                let f: FenceAuditV1 = decode(&row.get::<_, String>("fence_json")?)?;
                require(
                    high_water >= checked_integer(f.epochs[domain])?,
                    "Fence high-water rollback",
                )?;
            }
        }
        let has_budget: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM physical_control_budgets WHERE root_id=?1)",
            [text(&s.root)],
            |r| r.get(0),
        )?;
        require(has_budget, "Missing session budget ledger")?;
        let root_state: String = c.query_row(
            "SELECT state FROM physical_attempts WHERE root_id=?1 AND role='requester_executor'",
            [text(&s.root)],
            |r| r.get(0),
        )?;
        require(
            root_state == "open" || state == "quarantined",
            "Closed Root has live session",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_actions")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let x: ActionAuditV1 = decode(&row.get::<_, String>("audit_json")?)?;
        let s = session(c, &x.session)?;
        let a = root(c, &x.root)?;
        x.validate(&s, &a)?;
        for (col, value) in [
            ("action_id", text(&x.proposal.action_id)),
            ("root_id", text(&x.root)),
            ("session_id", text(&x.session)),
            ("grant_id", text(&x.grant)),
            ("payload_digest", text(&x.proposal.payload_digest)),
            (
                "proposal_digest",
                text(&digest("pastey-physical-proposal-v1", &x.proposal)?),
            ),
            ("challenge_id", text(&x.proposal.challenge_id)),
            ("audit_digest", text(&x.digest()?)),
        ] {
            require(
                row.get::<_, String>(col)? == value,
                "Action column/body mismatch",
            )?;
        }
        require(
            row.get::<_, i64>("decision_sequence")? == 1
                && row.get::<_, i64>("requested_us")?
                    == checked_integer(x.proposal.requested_duration_us.get())?
                && row.get::<_, i64>("reserved_us")? == checked_integer(x.reserved_us)?
                && row.get::<_, i64>("expires_at")? == x.expires_at.get() as i64,
            "Action budget/deadline mismatch",
        )?;
        if let Some(op) = row.get::<_, Option<String>>("operation_id")? {
            RequestId::try_from(op)?;
        }
        let session_state: String = c.query_row(
            "SELECT state FROM physical_sessions WHERE session_id=?1",
            [text(&s.id)],
            |r| r.get(0),
        )?;
        let state: String = row.get("state")?;
        require(
            state == "closed" || session_state == "active",
            "Live action without active session",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_control_budgets")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: RootId = RootId::try_from(row.get::<_, String>("root_id")?)?;
        let raw: String = c.query_row(
            "SELECT audit_json FROM physical_sessions WHERE root_id=?1",
            [text(&id)],
            |r| r.get(0),
        )?;
        let s: SessionAuditV1 = decode(&raw)?;
        require(
            row.get::<_, i64>("ceiling_us")?
                == checked_integer(s.scope.fields().execution.total_execution_us.get())?
                && row.get::<_, i64>("ceiling_count")? == 1,
            "Budget ceiling mismatch",
        )?;
        let values: Option<(i64, i64, String)> = c
            .query_row(
                "SELECT reserved_us,dispatch_intent,state FROM physical_actions WHERE root_id=?1",
                [text(&id)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (reserved, count, consumed) = match values {
            None => (0, 0, 0),
            Some((_n, 0, state)) if state == "closed" => (0, 0, 0),
            Some((n, d, _)) => (n, 1, if d == 1 { n } else { 0 }),
        };
        require(
            row.get::<_, i64>("reserved_us")? == reserved
                && row.get::<_, i64>("reserved_count")? == count
                && row.get::<_, i64>("consumed_us")? == consumed,
            "Budget reservation/dispatch mismatch",
        )?;
    }
    Ok(())
}
