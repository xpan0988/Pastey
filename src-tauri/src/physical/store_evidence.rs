//! Append-only evidence/history and one terminal task decision in the existing ledger.
use super::*;
use crate::physical::{contracts::*, evidence::*};

pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_evidence_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_evidence_schema VALUES(1,1);
-- Retain all holder history; only unreleased holders exclude another session.
DROP TRIGGER physical_reservation_monotonic;
DROP TRIGGER physical_reservations_keep;
ALTER TABLE physical_domain_reservations RENAME TO physical_stage4_reservations;
CREATE TABLE physical_domain_reservations(domain_id TEXT NOT NULL REFERENCES physical_domains(domain_id),session_id TEXT NOT NULL REFERENCES physical_sessions(session_id),epoch INTEGER NOT NULL CHECK(epoch>0),state TEXT NOT NULL CHECK(state IN ('held','quarantined','released')),PRIMARY KEY(domain_id,session_id)) STRICT;
INSERT INTO physical_domain_reservations SELECT * FROM physical_stage4_reservations;
DROP TABLE physical_stage4_reservations;
CREATE UNIQUE INDEX physical_current_holder ON physical_domain_reservations(domain_id) WHERE state!='released';
CREATE TRIGGER physical_reservation_monotonic BEFORE UPDATE ON physical_domain_reservations
WHEN NEW.domain_id!=OLD.domain_id OR NEW.session_id!=OLD.session_id OR NEW.epoch!=OLD.epoch
 OR NOT ((OLD.state='held' AND NEW.state='quarantined') OR (OLD.state='quarantined' AND NEW.state='released' AND EXISTS(SELECT 1 FROM physical_handovers WHERE session_id=OLD.session_id)))
BEGIN SELECT RAISE(ABORT,'physical domain holder regression');END;
CREATE TRIGGER physical_reservations_keep BEFORE DELETE ON physical_domain_reservations BEGIN SELECT RAISE(ABORT,'physical domain denial required');END;
CREATE TABLE physical_evidence(
 id TEXT PRIMARY KEY,action_id TEXT NOT NULL REFERENCES physical_actions(action_id),
 kind TEXT NOT NULL CHECK(kind IN ('observation','disposition')),sequence INTEGER NOT NULL CHECK(sequence>0),
 revision INTEGER NOT NULL CHECK(revision>0),capture_us INTEGER NOT NULL CHECK(capture_us>0),receipt_us INTEGER NOT NULL CHECK(receipt_us>=capture_us),
 source_digest TEXT NOT NULL CHECK(length(source_digest)=64),ordered INTEGER NOT NULL CHECK(ordered IN (0,1)),qualified INTEGER NOT NULL CHECK(qualified IN (0,1)),
 digest TEXT NOT NULL CHECK(length(digest)=64),record_json TEXT NOT NULL,
 UNIQUE(action_id,revision),UNIQUE(action_id,kind,source_digest,sequence)
) STRICT;
CREATE TABLE physical_consequences(action_id TEXT NOT NULL REFERENCES physical_actions(action_id),revision INTEGER NOT NULL CHECK(revision>0),evidence_revision INTEGER NOT NULL CHECK(evidence_revision>=0),state TEXT NOT NULL CHECK(state IN ('unobserved','partial','verified','contradicted','outcome_unknown')),completion_digest TEXT NOT NULL CHECK(length(completion_digest)=64),digest TEXT NOT NULL CHECK(length(digest)=64),record_json TEXT NOT NULL,PRIMARY KEY(action_id,revision)) STRICT;
CREATE TABLE physical_reconciliations(action_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),consequence_revision INTEGER NOT NULL,state TEXT NOT NULL CHECK(state IN ('needed','observing','resolved','still_unknown','intervention_required')),digest TEXT NOT NULL CHECK(length(digest)=64),record_json TEXT NOT NULL,PRIMARY KEY(action_id,revision),FOREIGN KEY(action_id,consequence_revision) REFERENCES physical_consequences(action_id,revision)) STRICT;
CREATE TABLE physical_task_acceptance(root_id TEXT PRIMARY KEY,role TEXT NOT NULL CHECK(role='requester_executor'),state TEXT NOT NULL CHECK(state IN ('pending','accepted','rejected','cancelled')),revision INTEGER NOT NULL CHECK(revision IN (1,2)),action_id TEXT,consequence_revision INTEGER,FOREIGN KEY(root_id,role) REFERENCES physical_attempts(root_id,role),FOREIGN KEY(action_id,consequence_revision) REFERENCES physical_consequences(action_id,revision),CHECK((state='pending' AND revision=1 AND action_id IS NULL AND consequence_revision IS NULL) OR (state='cancelled' AND revision=2 AND action_id IS NULL AND consequence_revision IS NULL) OR (state IN ('accepted','rejected') AND revision=2 AND action_id IS NOT NULL AND consequence_revision IS NOT NULL))) STRICT;
INSERT INTO physical_task_acceptance SELECT root_id,role,CASE WHEN state='closed' AND close_reason NOT IN ('interrupted','shutdown') THEN 'cancelled' ELSE 'pending' END,CASE WHEN state='closed' AND close_reason NOT IN ('interrupted','shutdown') THEN 2 ELSE 1 END,NULL,NULL FROM physical_attempts;
CREATE TABLE physical_handover_policies(session_id TEXT PRIMARY KEY REFERENCES physical_sessions(session_id),digest TEXT NOT NULL CHECK(length(digest)=64),record_json TEXT NOT NULL) STRICT;
CREATE TABLE physical_handovers(session_id TEXT PRIMARY KEY REFERENCES physical_sessions(session_id),action_id TEXT NOT NULL REFERENCES physical_actions(action_id),evidence_revision INTEGER NOT NULL CHECK(evidence_revision>0),verified_us INTEGER NOT NULL CHECK(verified_us>0),policy_digest TEXT NOT NULL CHECK(length(policy_digest)=64),lineage_digest TEXT NOT NULL CHECK(length(lineage_digest)=64)) STRICT;
CREATE TRIGGER physical_acceptance_terminal BEFORE UPDATE ON physical_task_acceptance
WHEN OLD.state!='pending' OR NEW.state='pending' OR NEW.revision!=2 OR NEW.root_id!=OLD.root_id OR NEW.role!=OLD.role
BEGIN SELECT RAISE(ABORT,'physical terminal acceptance');END;
CREATE TRIGGER physical_acceptance_keep BEFORE DELETE ON physical_task_acceptance BEGIN SELECT RAISE(ABORT,'physical acceptance history required');END;
CREATE TRIGGER physical_evidence_immutable BEFORE UPDATE ON physical_evidence BEGIN SELECT RAISE(ABORT,'physical immutable evidence');END;
CREATE TRIGGER physical_evidence_keep BEFORE DELETE ON physical_evidence BEGIN SELECT RAISE(ABORT,'physical evidence history required');END;
CREATE TRIGGER physical_consequence_immutable BEFORE UPDATE ON physical_consequences BEGIN SELECT RAISE(ABORT,'physical immutable consequence');END;
CREATE TRIGGER physical_consequence_keep BEFORE DELETE ON physical_consequences BEGIN SELECT RAISE(ABORT,'physical consequence history required');END;
CREATE TRIGGER physical_reconciliation_immutable BEFORE UPDATE ON physical_reconciliations BEGIN SELECT RAISE(ABORT,'physical immutable reconciliation');END;
CREATE TRIGGER physical_reconciliation_keep BEFORE DELETE ON physical_reconciliations BEGIN SELECT RAISE(ABORT,'physical reconciliation history required');END;
CREATE TRIGGER physical_handover_policy_immutable BEFORE UPDATE ON physical_handover_policies BEGIN SELECT RAISE(ABORT,'physical immutable handover policy');END;
CREATE TRIGGER physical_handover_policy_keep BEFORE DELETE ON physical_handover_policies BEGIN SELECT RAISE(ABORT,'physical handover policy required');END;
CREATE TRIGGER physical_handover_immutable BEFORE UPDATE ON physical_handovers BEGIN SELECT RAISE(ABORT,'physical immutable handover');END;
CREATE TRIGGER physical_handover_keep BEFORE DELETE ON physical_handovers BEGIN SELECT RAISE(ABORT,'physical handover history required');END;
"#;

pub(super) fn lineage(
    c: &Connection,
    id: &ActionId,
) -> AppResult<(EvidenceLineageV1, PhysicalReviewScopeV1)> {
    // Existing action/root/review only. No upsert and no conversion into authority.
    let x = super::control_ledger::action(c, id)?;
    let a = super::control_ledger::root(c, &x.root)?;
    let raw: String = c.query_row(
        "SELECT scope_json FROM physical_reviews WHERE review_id=?1 AND revision=?2",
        params![text(&a.review_id), checked_integer(a.review_revision)?],
        |r| r.get(0),
    )?;
    let scope: PhysicalReviewScopeV1 = decode(&raw)?;
    require(
        x.completion_digest == digest("pastey-physical-completion-v1", &scope.fields().completion)?,
        "Original completion mismatch",
    )?;
    let s = scope.fields();
    let sub = &s.environment.subsystems[&s.profile.subsystem];
    let l = EvidenceLineageV1 {
        version: VersionV1,
        environment: a.environment,
        root: a.root_id,
        attempt: a.attempt_id,
        session: x.session,
        action: id.clone(),
        source: s.profile.subsystem.clone(),
        controller: sub.controller_incarnation.clone(),
        body: sub.body.clone(),
        body_incarnation: sub.body_incarnation.clone(),
        world: sub.world_incarnation.clone(),
        frame: completion(&scope).frame.clone(),
        schema: label("microduck.displacement-measured.v1"),
        evidence_class: s.environment.evidence_class,
        witness: completion(&scope).witness,
        qualification_digest: s.qualification.digest()?,
        origin: digest("pastey-physical-measured-origin-v1", &(id, &a.scope_digest))?,
    };
    Ok((l, scope))
}
fn producer_qualification(
    c: &Connection,
    scope: &PhysicalReviewScopeV1,
    id: &QualificationId,
) -> AppResult<PhysicalQualificationV1> {
    let raw: String = c.query_row(
        "SELECT record_json FROM physical_qualifications WHERE qualification_id=?1",
        [text(id)],
        |r| r.get(0),
    )?;
    let q: PhysicalQualificationV1 = decode(&raw)?;
    let original = &scope.fields().qualification;
    require(
        q.profile_digest == original.profile_digest
            && q.conditions_digest == original.conditions_digest
            && q.evidence_class == original.evidence_class
            && q.required_enforcement_class
                .meets(original.required_enforcement_class),
        "Producer qualification weakens original requirements",
    )?;
    Ok(q)
}
fn qualified(
    c: &Connection,
    l: &EvidenceLineageV1,
    scope: &PhysicalReviewScopeV1,
    id: &QualificationId,
    capture: u64,
) -> AppResult<bool> {
    let q = producer_qualification(c, scope, id)?;
    let (retired, reg): (i64, String) = c.query_row(
        "SELECT retired,registration_digest FROM physical_environments WHERE environment_id=?1",
        [text(&l.environment)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let a = super::control_ledger::root(c, &l.root)?;
    let (withdrawal,env,qualification_reg):(i64,String,String)=c.query_row("SELECT withdrawal_revision,environment_id,registration_digest FROM physical_qualifications WHERE qualification_id=?1",[text(id)],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    Ok(retired == 0
        && reg == text(&a.registration_digest)
        && env == text(&l.environment)
        && qualification_reg == reg
        && withdrawal == 0
        && capture >= a.created_at.get() * 1000
        && capture < q.expires_at.get() * 1000)
}
fn source(l: &EvidenceLineageV1) -> AppResult<DigestV1> {
    digest("pastey-physical-evidence-source-v1", l)
}
fn facts(
    c: &Connection,
    id: &ActionId,
    revision: u64,
) -> AppResult<(Vec<ObservationRecordV1>, Vec<DispositionRecordV1>)> {
    let mut os = Vec::new();
    let mut ds = Vec::new();
    let mut stmt=c.prepare("SELECT kind,record_json FROM physical_evidence WHERE action_id=?1 AND revision<=?2 ORDER BY revision")?;
    let mut rows = stmt.query(params![text(id), checked_integer(revision)?])?;
    while let Some(r) = rows.next()? {
        let raw: String = r.get(1)?;
        match r.get::<_, String>(0)?.as_str() {
            "observation" => os.push(decode(&raw)?),
            _ => ds.push(decode(&raw)?),
        }
    }
    Ok((os, ds))
}
fn head(c: &Connection, id: &ActionId) -> AppResult<u64> {
    Ok(c.query_row(
        "SELECT COALESCE(max(revision),0) FROM physical_evidence WHERE action_id=?1",
        [text(id)],
        |r| r.get::<_, i64>(0),
    )? as u64)
}
fn consequence(c: &Connection, id: &ActionId) -> AppResult<Option<PhysicalConsequenceV1>> {
    c.query_row("SELECT record_json FROM physical_consequences WHERE action_id=?1 ORDER BY revision DESC LIMIT 1",[text(id)],|r|r.get::<_,String>(0)).optional()?.map(|r|decode(&r)).transpose()
}
pub(super) fn released(c: &Connection, id: &SessionId) -> AppResult<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM physical_handovers WHERE session_id=?1)",
        [text(id)],
        |r| r.get(0),
    )?)
}
pub(super) fn cancel(c: &Connection, id: &RootId) -> AppResult<()> {
    c.execute("UPDATE physical_task_acceptance SET state='cancelled',revision=2 WHERE root_id=?1 AND state='pending'",[text(id)])?;
    Ok(())
}
pub(super) fn originate(c: &Connection, id: &RootId) -> AppResult<()> {
    c.execute("INSERT INTO physical_task_acceptance VALUES(?1,'requester_executor','pending',1,NULL,NULL)",[text(id)])?;
    Ok(())
}
impl PhysicalStoreV1 {
    pub(in crate::physical) fn evidence_host(
        &self,
        id: &ActionId,
    ) -> AppResult<crate::host_identity::HostRef> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let (_, scope) = lineage(&tx, id)?;
        let host = scope.fields().executor.clone();
        tx.commit()?;
        Ok(host)
    }
    pub(in crate::physical) fn evidence_lineage(
        &self,
        id: &ActionId,
    ) -> AppResult<EvidenceLineageV1> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let (l, _) = lineage(&tx, id)?;
        tx.commit()?;
        Ok(l)
    }
    pub(in crate::physical) fn record_observation(
        &self,
        proof: &TrustedObservationV1,
        receipt_us: u64,
    ) -> AppResult<bool> {
        let f = proof.fact();
        f.validate()?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let (l, s) = lineage(&tx, &f.lineage.action)?;
        // Foreign root/session/action cannot even attach historical facts. Source
        // reset/class/frame mismatch may be recorded but denies evaluation continuity.
        correlate(&l, &f.lineage)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT record_json FROM physical_evidence WHERE id=?1",
                [text(&f.id)],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = prior {
            let old: ObservationRecordV1 = decode(&raw)?;
            require(old.fact == *f, "Observation identity conflict")?;
            return Ok(false);
        }
        require(
            receipt_us >= f.capture_us && receipt_us <= i64::MAX as u64,
            "Unprovable observation receipt clock",
        )?;
        let qid = proof
            .qualification()
            .unwrap_or(&s.fields().qualification.qualification_id);
        let producer_q = producer_qualification(&tx, &s, qid)?;
        let src = source(&f.lineage)?;
        let ordered = ordered(
            &tx,
            &f.lineage.action,
            "observation",
            &src,
            f.sequence,
            f.capture_us,
        )?;
        let record = ObservationRecordV1 {
            gate_a: proof.gate_a_provenance().cloned(),
            fact: f.clone(),
            receipt_us,
            receipt: fresh_request()?,
            ordered,
            producer_qualification: qid.clone(),
            producer_qualification_digest: producer_q.digest()?,
            qualified: qualified(&tx, &l, &s, qid, f.capture_us)?
                && receipt_us - f.capture_us <= completion(&s).observation.max_age_us.get(),
        };
        insert_fact(
            &tx,
            &text(&f.id),
            &f.lineage.action,
            "observation",
            f.sequence,
            f.capture_us,
            receipt_us,
            &src,
            ordered,
            record.qualified,
            &record,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub(in crate::physical) fn record_disposition(
        &self,
        proof: &TrustedDispositionV1,
        receipt_us: u64,
    ) -> AppResult<bool> {
        let f = proof.fact();
        f.validate()?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let (l, s) = lineage(&tx, &f.lineage.action)?;
        correlate(&l, &f.lineage)?;
        let prior: Option<String> = tx
            .query_row(
                "SELECT record_json FROM physical_evidence WHERE id=?1",
                [text(&f.id)],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(raw) = prior {
            let old: DispositionRecordV1 = decode(&raw)?;
            require(old.fact == *f, "Disposition identity conflict")?;
            return Ok(false);
        }
        require(
            receipt_us >= f.capture_us && receipt_us <= i64::MAX as u64,
            "Unprovable disposition receipt clock",
        )?;
        let qid = proof
            .qualification()
            .unwrap_or(&s.fields().qualification.qualification_id);
        let producer_q = producer_qualification(&tx, &s, qid)?;
        let src = source(&f.lineage)?;
        let ordered = ordered(
            &tx,
            &f.lineage.action,
            "disposition",
            &src,
            f.sequence,
            f.capture_us,
        )?;
        let record = DispositionRecordV1 {
            fact: f.clone(),
            receipt_us,
            receipt: fresh_request()?,
            ordered,
            producer_qualification: qid.clone(),
            producer_qualification_digest: producer_q.digest()?,
            qualified: qualified(&tx, &l, &s, qid, f.capture_us)?
                && receipt_us - f.capture_us <= completion(&s).observation.max_age_us.get(),
        };
        insert_fact(
            &tx,
            &text(&f.id),
            &f.lineage.action,
            "disposition",
            f.sequence,
            f.capture_us,
            receipt_us,
            &src,
            ordered,
            record.qualified,
            &record,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub(in crate::physical) fn evaluate_consequence(
        &self,
        id: &ActionId,
        now_us: u64,
    ) -> AppResult<PhysicalConsequenceV1> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let (l, s) = lineage(&tx, id)?;
        let rev = head(&tx, id)?;
        let (os, ds) = facts(&tx, id, rev)?;
        let old = consequence(&tx, id)?;
        let (state, reason, gap, uncertainty) = evaluate(&s, &l, &os, &ds, now_us);
        let x = PhysicalConsequenceV1 {
            version: VersionV1,
            root: l.root,
            attempt: l.attempt,
            action: id.clone(),
            completion_digest: digest("pastey-physical-completion-v1", &s.fields().completion)?,
            evaluator: label("microduck.displacement-settled.v1"),
            evaluator_version: VersionV1,
            evidence_class: l.evidence_class,
            witness: l.witness,
            revision: old.as_ref().map_or(1, |o| o.revision + 1),
            evidence_revision: rev,
            evaluated_us: now_us,
            observations: os.iter().map(|o| o.fact.id.clone()).collect(),
            dispositions: ds.iter().map(|d| d.fact.id.clone()).collect(),
            max_gap_us: gap,
            max_uncertainty_m: uncertainty,
            state,
            reason: label(reason),
        };
        if let Some(old) = old {
            if old.evidence_revision == rev && old.state == state && old.reason == x.reason {
                return Ok(old);
            }
        }
        tx.execute(
            "INSERT INTO physical_consequences VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                text(id),
                checked_integer(x.revision)?,
                checked_integer(rev)?,
                tag(&state)?,
                text(&x.completion_digest),
                text(&digest("pastey-physical-consequence-v1", &x)?),
                serde_json::to_string(&x)?
            ],
        )?;
        tx.commit()?;
        Ok(x)
    }
    // Called only by the Core-owned evidence child module, never an evidence DTO.
    pub(in crate::physical) fn commit_acceptance(
        &self,
        proof: &crate::physical::core::CoreAcceptanceDecisionV1,
    ) -> AppResult<AcceptanceStateV1> {
        let (root, attempt, id, revision, completion_digest, reject, now_us) = proof.fields();
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let (l, s) = lineage(&tx, id)?;
        require(
            l.root == *root
                && l.attempt == *attempt
                && digest("pastey-physical-completion-v1", &s.fields().completion)?
                    == *completion_digest,
            "Acceptance lineage mismatch",
        )?;
        let x = consequence(&tx, id)?
            .ok_or_else(|| crate::error::AppError::InvalidInput("Missing consequence".into()))?;
        require(
            x.revision == revision
                && x.completion_digest == *completion_digest
                && x.evidence_revision == head(&tx, id)?,
            "Stale acceptance consequence",
        )?;
        let terminal = acceptance(&tx, root)?;
        if terminal != AcceptanceStateV1::Pending {
            return Ok(terminal);
        }
        let sent: bool = tx.query_row(
            "SELECT dispatch_intent=1 FROM physical_actions WHERE action_id=?1",
            [text(id)],
            |r| r.get(0),
        )?;
        require(sent, "Acceptance lacks durable dispatch intent")?;
        let (os, ds) = facts(&tx, id, x.evidence_revision)?;
        require(
            os.iter()
                .rev()
                .find(|o| o.ordered && o.fact.lineage == l)
                .is_some_and(|o| {
                    qualified(&tx, &l, &s, &o.producer_qualification, now_us).unwrap_or(false)
                }),
            "Acceptance qualification/enrollment no longer provable",
        )?;
        let (current, _, _, _) = evaluate(&s, &l, &os, &ds, now_us);
        require(
            if reject {
                x.state == ConsequenceStateV1::Contradicted && current == x.state
            } else {
                x.state == ConsequenceStateV1::Verified && current == x.state
            },
            "Acceptance requires exact current consequence",
        )?;
        let desired = if reject {
            AcceptanceStateV1::Rejected
        } else {
            AcceptanceStateV1::Accepted
        };
        tx.execute("UPDATE physical_task_acceptance SET state=?2,revision=2,action_id=?3,consequence_revision=?4 WHERE root_id=?1 AND state='pending'",params![text(root),tag(&desired)?,text(id),checked_integer(revision)?])?;
        let state = acceptance(&tx, root)?;
        super::control_ledger::close_root(&tx, root)?;
        tx.execute("UPDATE physical_attempts SET state='closed',revision=2,close_reason='revoked' WHERE root_id=?1 AND state='open'",[text(root)])?;
        tx.commit()?;
        Ok(state)
    }
    pub(in crate::physical) fn acceptance(&self, id: &RootId) -> AppResult<AcceptanceStateV1> {
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        super::audit(&tx)?;
        let v = acceptance(&tx, id)?;
        tx.commit()?;
        Ok(v)
    }
    pub(in crate::physical) fn configure_handover(
        &self,
        proof: &TrustedHandoverPolicyV1,
    ) -> AppResult<()> {
        let p = proof.predicate();
        p.freshness.validate()?;
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let s = super::control_ledger::session(&tx, &p.session)?;
        require(
            p.qualification_digest == s.qualification_digest
                && p.frame == completion(&s.scope).frame
                && p.freshness.max_age_us <= s.scope.fields().freshness.observation.max_age_us
                && p.freshness.max_gap_us <= s.scope.fields().freshness.observation.max_gap_us,
            "Handover policy lineage mismatch",
        )?;
        tx.execute(
            "INSERT INTO physical_handover_policies VALUES(?1,?2,?3)",
            params![
                text(&p.session),
                text(&digest("pastey-physical-handover-policy-v1", p)?),
                serde_json::to_string(p)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(in crate::physical) fn reconcile(
        &self,
        id: &ActionId,
        now_us: u64,
        request_handover: bool,
    ) -> AppResult<PhysicalReconciliationV1> {
        let mut c = self.connection()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::audit(&tx)?;
        let (l, s) = lineage(&tx, id)?;
        let x = consequence(&tx, id)?.ok_or_else(|| {
            crate::error::AppError::InvalidInput("Evaluate before reconciliation".into())
        })?;
        require(
            x.evidence_revision == head(&tx, id)?,
            "Evaluate new evidence before reconciliation",
        )?;
        let (os, ds) = facts(&tx, id, x.evidence_revision)?;
        let phase: String = tx.query_row(
            "SELECT state FROM physical_sessions WHERE session_id=?1",
            [text(&l.session)],
            |r| r.get(0),
        )?;
        let ack: bool = tx.query_row(
            "SELECT fence_ack IS NOT NULL FROM physical_sessions WHERE session_id=?1",
            [text(&l.session)],
            |r| r.get(0),
        )?;
        if request_handover && phase == "quarantined" && !released(&tx, &l.session)? {
            let raw: Option<String> = tx
                .query_row(
                    "SELECT record_json FROM physical_handover_policies WHERE session_id=?1",
                    [text(&l.session)],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(raw) = raw {
                let p: HandoverPredicateV1 = decode(&raw)?;
                // Fenced disposition from the qualified producer anchors the
                // safe trace. Adapter fence ACK alone cannot supply this fact.
                let after = ds
                    .iter()
                    .filter(|d| {
                        d.ordered
                            && d.qualified
                            && d.fact.lineage == l
                            && d.fact.disposition == DispositionV1::Fenced
                            && fence_matches(&tx, &l.session, &d.fact.fence_request)
                                .unwrap_or(false)
                    })
                    .map(|d| d.fact.capture_us)
                    .max();
                let current = load_registration(&tx, &l.environment, false)?;
                let continuous = current
                    .subsystems
                    .get(&s.fields().profile.subsystem)
                    .is_some_and(|sub| {
                        sub.controller_incarnation == l.controller
                            && sub.body == l.body
                            && sub.body_incarnation == l.body_incarnation
                            && sub.world_incarnation == l.world
                    });
                if continuous
                    && os
                        .iter()
                        .rev()
                        .find(|o| o.ordered && o.fact.lineage == l)
                        .is_some_and(|o| {
                            qualified(&tx, &l, &s, &o.producer_qualification, now_us)
                                .unwrap_or(false)
                        })
                    && after.is_some_and(|after| safe_handover(&p, &l, &os, after, now_us))
                {
                    tx.execute(
                        "INSERT INTO physical_handovers VALUES(?1,?2,?3,?4,?5,?6)",
                        params![
                            text(&l.session),
                            text(id),
                            checked_integer(x.evidence_revision)?,
                            checked_integer(now_us)?,
                            text(&digest("pastey-physical-handover-policy-v1", &p)?),
                            text(&source(&l)?)
                        ],
                    )?;
                    tx.execute("UPDATE physical_domain_reservations SET state='released' WHERE session_id=?1 AND state='quarantined'",[text(&l.session)])?;
                }
            }
        }
        let is_released = released(&tx, &l.session)?;
        let current = evaluate(&s, &l, &os, &ds, now_us).0;
        let (retired, registration): (i64, String) = tx.query_row(
            "SELECT retired,registration_digest FROM physical_environments WHERE environment_id=?1",
            [text(&l.environment)],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let original = super::control_ledger::root(&tx, &l.root)?;
        let continuous = retired == 0 && registration == text(&original.registration_digest);
        let (state, reason) = if !continuous {
            (
                ReconciliationStateV1::InterventionRequired,
                "identity_unproved",
            )
        } else if is_released {
            (ReconciliationStateV1::Resolved, "verified_handover")
        } else if current == ConsequenceStateV1::Contradicted {
            (
                ReconciliationStateV1::InterventionRequired,
                "contract_contradicted",
            )
        } else if phase == "quarantined" {
            (ReconciliationStateV1::StillUnknown, "handover_unproved")
        } else if current == ConsequenceStateV1::Verified {
            (ReconciliationStateV1::Resolved, "completion_verified")
        } else if current == ConsequenceStateV1::Partial {
            (ReconciliationStateV1::Observing, "partial_evidence")
        } else {
            (ReconciliationStateV1::StillUnknown, "outcome_unproved")
        };
        let old:Option<String>=tx.query_row("SELECT record_json FROM physical_reconciliations WHERE action_id=?1 ORDER BY revision DESC LIMIT 1",[text(id)],|r|r.get(0)).optional()?;
        let old: Option<PhysicalReconciliationV1> = old.map(|r| decode(&r)).transpose()?;
        let r = PhysicalReconciliationV1 {
            version: VersionV1,
            action: id.clone(),
            revision: old.as_ref().map_or(1, |r| r.revision + 1),
            consequence_revision: x.revision,
            state,
            reason: label(reason),
            holder_released: is_released,
            fence_acknowledged: ack,
        };
        if let Some(old) = old {
            if old.consequence_revision == r.consequence_revision
                && old.state == r.state
                && old.holder_released == is_released
                && old.fence_acknowledged == ack
            {
                return Ok(old);
            }
        }
        tx.execute(
            "INSERT INTO physical_reconciliations VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                text(id),
                checked_integer(r.revision)?,
                checked_integer(x.revision)?,
                tag(&state)?,
                text(&digest("pastey-physical-reconciliation-v1", &r)?),
                serde_json::to_string(&r)?
            ],
        )?;
        super::audit(&tx)?;
        tx.commit()?;
        Ok(r)
    }
}
fn acceptance(c: &Connection, id: &RootId) -> AppResult<AcceptanceStateV1> {
    let raw: String = c.query_row(
        "SELECT state FROM physical_task_acceptance WHERE root_id=?1",
        [text(id)],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&serde_json::to_string(&raw)?)?)
}
fn correlate(expected: &EvidenceLineageV1, actual: &EvidenceLineageV1) -> AppResult<()> {
    require(
        expected.environment == actual.environment
            && expected.root == actual.root
            && expected.attempt == actual.attempt
            && expected.session == actual.session
            && expected.action == actual.action,
        "Foreign historical evidence lineage",
    )
}
fn fresh_request() -> AppResult<RequestId> {
    RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
}
fn ordered(
    c: &Connection,
    id: &ActionId,
    kind: &str,
    src: &DigestV1,
    sequence: u64,
    capture: u64,
) -> AppResult<bool> {
    let (seq,time):(i64,i64)=c.query_row("SELECT COALESCE(max(sequence),0),COALESCE(max(capture_us),0) FROM physical_evidence WHERE action_id=?1 AND kind=?2 AND source_digest=?3 AND ordered=1",params![text(id),kind,text(src)],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok(sequence > seq as u64 && capture > time as u64)
}
#[allow(clippy::too_many_arguments)]
fn insert_fact(
    c: &Connection,
    id: &str,
    action: &ActionId,
    kind: &str,
    sequence: u64,
    capture: u64,
    receipt: u64,
    src: &DigestV1,
    ordered: bool,
    qualified: bool,
    record: &impl Serialize,
) -> AppResult<()> {
    let revision = head(c, action)?.checked_add(1).unwrap_or(u64::MAX);
    c.execute(
        "INSERT INTO physical_evidence VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            id,
            text(action),
            kind,
            checked_integer(sequence)?,
            checked_integer(revision)?,
            checked_integer(capture)?,
            checked_integer(receipt)?,
            text(src),
            ordered,
            qualified,
            text(&digest("pastey-physical-evidence-record-v1", record)?),
            serde_json::to_string(record)?
        ],
    )?;
    Ok(())
}

pub(super) fn audit(c: &Connection) -> AppResult<()> {
    let v: Vec<i64> = c
        .prepare("SELECT version FROM physical_evidence_schema WHERE singleton=1")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    require(v == [1], "Incompatible evidence ledger")?;
    for table in [
        "physical_evidence",
        "physical_consequences",
        "physical_reconciliations",
        "physical_task_acceptance",
        "physical_handover_policies",
        "physical_handovers",
    ] {
        require(
            !c.prepare(&format!("PRAGMA foreign_key_check({table})"))?
                .exists([])?,
            "Evidence foreign key violation",
        )?;
    }
    let missing:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM physical_attempts a LEFT JOIN physical_task_acceptance t USING(root_id) WHERE t.root_id IS NULL)",[],|r|r.get(0))?;
    require(!missing, "Missing task terminal state")?;
    let mut stmt = c.prepare("SELECT * FROM physical_evidence ORDER BY action_id,revision")?;
    let mut rows = stmt.query([])?;
    let mut revisions = BTreeMap::<String, u64>::new();
    let mut sources = BTreeMap::<(String, String, String), (u64, u64)>::new();
    while let Some(r) = rows.next()? {
        let id: String = r.get("id")?;
        let action: String = r.get("action_id")?;
        let kind: String = r.get("kind")?;
        let raw: String = r.get("record_json")?;
        let (l, fid, seq, capture, receipt, ordered, qualified, hash) = if kind == "observation" {
            let o: ObservationRecordV1 = decode(&raw)?;
            o.fact.validate()?;
            if let Some(p) = &o.gate_a {
                p.validate()?;
                require(
                    p.receipt_us <= o.receipt_us
                        && p.local_sequence == o.fact.sequence
                        && p.sample.daemon == o.fact.lineage.controller
                        && p.sample.body == o.fact.lineage.body_incarnation
                        && Some(p.sample.world.clone()) == o.fact.lineage.world,
                    "Gate A provenance/lineage mismatch",
                )?;
            }
            let (_, scope) = lineage(c, &o.fact.lineage.action)?;
            require(
                producer_qualification(c, &scope, &o.producer_qualification)?.digest()?
                    == o.producer_qualification_digest,
                "Observation producer qualification mismatch",
            )?;
            (
                o.fact.lineage.clone(),
                text(&o.fact.id),
                o.fact.sequence,
                o.fact.capture_us,
                o.receipt_us,
                o.ordered,
                o.qualified,
                digest("pastey-physical-evidence-record-v1", &o)?,
            )
        } else {
            let d: DispositionRecordV1 = decode(&raw)?;
            d.fact.validate()?;
            let (_, scope) = lineage(c, &d.fact.lineage.action)?;
            require(
                producer_qualification(c, &scope, &d.producer_qualification)?.digest()?
                    == d.producer_qualification_digest,
                "Disposition producer qualification mismatch",
            )?;
            (
                d.fact.lineage.clone(),
                text(&d.fact.id),
                d.fact.sequence,
                d.fact.capture_us,
                d.receipt_us,
                d.ordered,
                d.qualified,
                digest("pastey-physical-evidence-record-v1", &d)?,
            )
        };
        let (expected, _) = lineage(c, &l.action)?;
        correlate(&expected, &l)?;
        let src = text(&source(&l)?);
        let old = sources
            .entry((action.clone(), kind, src.clone()))
            .or_insert((0, 0));
        require(
            ordered == (seq > old.0 && capture > old.1),
            "Evidence source order mismatch",
        )?;
        if ordered {
            *old = (seq, capture);
        }
        let revision = revisions.entry(action.clone()).or_insert(0);
        *revision += 1;
        require(
            id == fid
                && action == text(&l.action)
                && r.get::<_, i64>("sequence")? == checked_integer(seq)?
                && r.get::<_, i64>("revision")? == checked_integer(*revision)?
                && r.get::<_, i64>("capture_us")? == checked_integer(capture)?
                && r.get::<_, i64>("receipt_us")? == checked_integer(receipt)?
                && receipt >= capture
                && r.get::<_, String>("source_digest")? == src
                && r.get::<_, bool>("ordered")? == ordered
                && r.get::<_, bool>("qualified")? == qualified
                && r.get::<_, String>("digest")? == text(&hash),
            "Evidence column/body mismatch",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_consequences ORDER BY action_id,revision")?;
    let mut rows = stmt.query([])?;
    let mut revisions = BTreeMap::<ActionId, u64>::new();
    while let Some(r) = rows.next()? {
        let x: PhysicalConsequenceV1 = decode(&r.get::<_, String>("record_json")?)?;
        let (l, s) = lineage(c, &x.action)?;
        let (os, ds) = facts(c, &x.action, x.evidence_revision)?;
        let (state, reason, gap, u) = evaluate(&s, &l, &os, &ds, x.evaluated_us);
        let rev = revisions.entry(x.action.clone()).or_insert(0);
        *rev += 1;
        require(
            x.revision == *rev
                && x.evidence_revision <= head(c, &x.action)?
                && x.root == l.root
                && x.attempt == l.attempt
                && x.completion_digest
                    == digest("pastey-physical-completion-v1", &s.fields().completion)?
                && x.evaluator == label("microduck.displacement-settled.v1")
                && x.evidence_class == l.evidence_class
                && x.witness == l.witness
                && x.observations == os.iter().map(|o| o.fact.id.clone()).collect::<Vec<_>>()
                && x.dispositions == ds.iter().map(|d| d.fact.id.clone()).collect::<Vec<_>>()
                && x.state == state
                && x.reason == label(reason)
                && x.max_gap_us == gap
                && x.max_uncertainty_m == u
                && r.get::<_, String>("action_id")? == text(&x.action)
                && r.get::<_, i64>("revision")? == checked_integer(x.revision)?
                && r.get::<_, i64>("evidence_revision")? == checked_integer(x.evidence_revision)?
                && r.get::<_, String>("state")? == tag(&state)?
                && r.get::<_, String>("completion_digest")? == text(&x.completion_digest)
                && r.get::<_, String>("digest")?
                    == text(&digest("pastey-physical-consequence-v1", &x)?),
            "Unprovable consequence",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_task_acceptance")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let state: String = r.get("state")?;
        if state != "pending" {
            let closed: bool = c.query_row(
                "SELECT state='closed' FROM physical_attempts WHERE root_id=?1",
                [r.get::<_, String>("root_id")?],
                |r| r.get(0),
            )?;
            require(closed, "Terminal task has open control authority")?;
        }
        if matches!(state.as_str(), "accepted" | "rejected") {
            let raw: String = c.query_row(
                "SELECT record_json FROM physical_consequences WHERE action_id=?1 AND revision=?2",
                params![
                    r.get::<_, String>("action_id")?,
                    r.get::<_, i64>("consequence_revision")?
                ],
                |r| r.get(0),
            )?;
            let x: PhysicalConsequenceV1 = decode(&raw)?;
            let sent: bool = c.query_row(
                "SELECT dispatch_intent=1 FROM physical_actions WHERE action_id=?1",
                [text(&x.action)],
                |r| r.get(0),
            )?;
            require(sent, "Terminal decision lacks dispatch audit")?;
            require(
                text(&x.root) == r.get::<_, String>("root_id")?
                    && x.state
                        == if state == "accepted" {
                            ConsequenceStateV1::Verified
                        } else {
                            ConsequenceStateV1::Contradicted
                        },
                "Acceptance without exact consequence",
            )?;
        }
    }
    let mut stmt = c.prepare("SELECT * FROM physical_handover_policies")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let p: HandoverPredicateV1 = decode(&r.get::<_, String>("record_json")?)?;
        p.freshness.validate()?;
        let s = super::control_ledger::session(c, &p.session)?;
        require(
            r.get::<_, String>("session_id")? == text(&p.session)
                && p.qualification_digest == s.qualification_digest
                && p.frame == completion(&s.scope).frame
                && p.freshness.max_age_us <= s.scope.fields().freshness.observation.max_age_us
                && p.freshness.max_gap_us <= s.scope.fields().freshness.observation.max_gap_us
                && r.get::<_, String>("digest")?
                    == text(&digest("pastey-physical-handover-policy-v1", &p)?),
            "Handover policy mismatch",
        )?;
    }
    let mut stmt = c.prepare("SELECT * FROM physical_handovers")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let id = ActionId::try_from(r.get::<_, String>("action_id")?)?;
        let (l, _) = lineage(c, &id)?;
        let raw: String = c.query_row(
            "SELECT record_json FROM physical_handover_policies WHERE session_id=?1",
            [text(&l.session)],
            |r| r.get(0),
        )?;
        let p: HandoverPredicateV1 = decode(&raw)?;
        let rev = r.get::<_, i64>("evidence_revision")? as u64;
        let (os, ds) = facts(c, &id, rev)?;
        let after = ds
            .iter()
            .filter(|d| {
                d.ordered
                    && d.qualified
                    && d.fact.lineage == l
                    && d.fact.disposition == DispositionV1::Fenced
                    && fence_matches(c, &l.session, &d.fact.fence_request).unwrap_or(false)
            })
            .map(|d| d.fact.capture_us)
            .max();
        require(
            r.get::<_, String>("session_id")? == text(&l.session)
                && rev <= head(c, &id)?
                && r.get::<_, String>("policy_digest")?
                    == text(&digest("pastey-physical-handover-policy-v1", &p)?)
                && r.get::<_, String>("lineage_digest")? == text(&source(&l)?)
                && after.is_some_and(|after| {
                    safe_handover(
                        &p,
                        &l,
                        &os,
                        after,
                        r.get::<_, i64>("verified_us").unwrap_or(0) as u64,
                    )
                }),
            "Unprovable handover",
        )?;
        let bad:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM physical_domain_reservations WHERE session_id=?1 AND state!='released')",[text(&l.session)],|r|r.get(0))?;
        require(!bad, "Incomplete handover release")?;
    }
    let mut stmt =
        c.prepare("SELECT * FROM physical_reconciliations ORDER BY action_id,revision")?;
    let mut rows = stmt.query([])?;
    let mut revisions = BTreeMap::<ActionId, u64>::new();
    while let Some(r) = rows.next()? {
        let x: PhysicalReconciliationV1 = decode(&r.get::<_, String>("record_json")?)?;
        let rev = revisions.entry(x.action.clone()).or_insert(0);
        *rev += 1;
        let (l, _) = lineage(c, &x.action)?;
        require(
            !x.holder_released || released(c, &l.session)?,
            "Reconciliation release without handover",
        )?;
        let raw: String = c.query_row(
            "SELECT record_json FROM physical_consequences WHERE action_id=?1 AND revision=?2",
            params![text(&x.action), checked_integer(x.consequence_revision)?],
            |r| r.get(0),
        )?;
        let finding: PhysicalConsequenceV1 = decode(&raw)?;
        require(
            match x.state {
                ReconciliationStateV1::Resolved => {
                    (x.reason == label("verified_handover") && x.holder_released)
                        || (x.reason == label("completion_verified")
                            && finding.state == ConsequenceStateV1::Verified)
                }
                ReconciliationStateV1::InterventionRequired => {
                    (finding.state == ConsequenceStateV1::Contradicted
                        && x.reason == label("contract_contradicted"))
                        || x.reason == label("identity_unproved")
                }
                ReconciliationStateV1::Observing => {
                    finding.state == ConsequenceStateV1::Partial
                        && x.reason == label("partial_evidence")
                }
                ReconciliationStateV1::StillUnknown => matches!(
                    text(&x.reason).as_str(),
                    "handover_unproved" | "outcome_unproved"
                ),
                ReconciliationStateV1::Needed => false,
            },
            "Unprovable reconciliation finding",
        )?;
        require(
            x.revision == *rev
                && r.get::<_, String>("action_id")? == text(&x.action)
                && r.get::<_, i64>("revision")? == checked_integer(x.revision)?
                && r.get::<_, i64>("consequence_revision")?
                    == checked_integer(x.consequence_revision)?
                && r.get::<_, String>("state")? == tag(&x.state)?
                && r.get::<_, String>("digest")?
                    == text(&digest("pastey-physical-reconciliation-v1", &x)?),
            "Reconciliation column/body mismatch",
        )?;
    }
    Ok(())
}

fn fence_matches(
    c: &Connection,
    session: &SessionId,
    request: &Option<RequestId>,
) -> AppResult<bool> {
    let raw: Option<String> = c.query_row(
        "SELECT fence_json FROM physical_sessions WHERE session_id=?1",
        [text(session)],
        |r| r.get(0),
    )?;
    match raw {
        Some(raw) => {
            let f: FenceAuditV1 = decode(&raw)?;
            Ok(Some(f.request) == *request)
        }
        None => Ok(false),
    }
}
