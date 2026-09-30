//! Internal Core evidence ingress and L7. No live authority is made from facts.
use super::*;
use crate::physical::evidence::*;

/// Only this Core child can construct a terminal decision. No serde or DTO/row
/// conversion. The ledger rechecks its exact stored consequence in the CAS.
pub(in crate::physical) struct CoreAcceptanceDecisionV1 {
    root: RootId,
    attempt: AttemptId,
    action: ActionId,
    revision: u64,
    completion: DigestV1,
    reject: bool,
    now_us: u64,
}
impl CoreAcceptanceDecisionV1 {
    pub(in crate::physical) fn fields(
        &self,
    ) -> (&RootId, &AttemptId, &ActionId, u64, &DigestV1, bool, u64) {
        (
            &self.root,
            &self.attempt,
            &self.action,
            self.revision,
            &self.completion,
            self.reject,
            self.now_us,
        )
    }
}
impl PhysicalControlServiceV1 {
    fn evidence_now(&mut self) -> AppResult<u64> {
        let (now, _) = match self.binding.now() {
            Ok(now) => now,
            Err(error) => {
                self.invalidate_live();
                let _ = self.store.close_open_attempts("dependency_invalidated");
                return Err(error);
            }
        };
        if let Some(time) = now
            .get()
            .checked_mul(1000)
            .filter(|v| *v <= i64::MAX as u64)
        {
            Ok(time)
        } else {
            self.invalidate_live();
            let _ = self.store.close_open_attempts("dependency_invalidated");
            Err(crate::error::AppError::InvalidInput(
                "Evidence clock overflow".into(),
            ))
        }
    }
    fn evidence_continuity(&mut self, fact: &EvidenceLineageV1) -> AppResult<()> {
        let expected = self.store.evidence_lineage(&fact.action)?;
        require(
            expected.environment == fact.environment
                && expected.root == fact.root
                && expected.attempt == fact.attempt
                && expected.session == fact.session,
            "Foreign evidence lineage",
        )?;
        if expected != *fact {
            self.invalidate_environment(&expected.environment);
            // Close private proof first, before fallible ledger closure. A trusted
            // reset is denial evidence, never a new enrollment or incarnation.
            self.binding
                .invalidate_evidence_continuity(&expected.environment)?;
            self.store
                .close_environment_attempts(&expected.environment, "dependency_invalidated")?;
        }
        Ok(())
    }
    pub(in crate::physical) fn record_physical_observation(
        &mut self,
        ingress: &LocalCoreIngressV1,
        proof: TrustedObservationV1,
    ) -> AppResult<bool> {
        self.validate_ingress(ingress)?;
        require(
            self.store.evidence_host(&proof.fact().lineage.action)? == *self.runtime.host_ref(),
            "Wrong Core evidence Host",
        )?;
        let now = self.evidence_now()?;
        self.evidence_continuity(&proof.fact().lineage)?;
        self.store.record_observation(&proof, now)
    }
    pub(in crate::physical) fn record_physical_disposition(
        &mut self,
        ingress: &LocalCoreIngressV1,
        proof: TrustedDispositionV1,
    ) -> AppResult<bool> {
        self.validate_ingress(ingress)?;
        require(
            self.store.evidence_host(&proof.fact().lineage.action)? == *self.runtime.host_ref(),
            "Wrong Core evidence Host",
        )?;
        let now = self.evidence_now()?;
        self.evidence_continuity(&proof.fact().lineage)?;
        self.store.record_disposition(&proof, now)
    }
    pub(in crate::physical) fn evaluate_physical_consequence(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ActionId,
    ) -> AppResult<PhysicalConsequenceV1> {
        self.validate_ingress(ingress)?;
        require(
            self.store.evidence_host(id)? == *self.runtime.host_ref(),
            "Wrong Core evidence Host",
        )?;
        let now = self.evidence_now()?;
        self.store.evaluate_consequence(id, now, &self.witnesses)
    }
    pub(in crate::physical) fn decide_physical_acceptance(
        &mut self,
        ingress: &LocalCoreIngressV1,
        root: &RootId,
        attempt: &AttemptId,
        id: &ActionId,
        revision: u64,
        completion: &DigestV1,
        reject: bool,
    ) -> AppResult<AcceptanceStateV1> {
        self.validate_ingress(ingress)?;
        require(
            self.store.evidence_host(id)? == *self.runtime.host_ref(),
            "Wrong Core evidence Host",
        )?;
        let l = self.store.evidence_lineage(id)?;
        require(
            l.root == *root && l.attempt == *attempt,
            "Foreign acceptance lineage",
        )?;
        // Close RAM first even if storage subsequently fails. Terminal decisions
        // never preserve permission to refresh the task. Durable closure and task
        // terminal CAS commit together; cancellation uses the same terminal row.
        self.control.invalidate_root(root);
        if let Some(r) = self.roots.remove(root) {
            r.valid.store(false, Ordering::Release);
        }
        let now_us = self.evidence_now()?;
        let proof = CoreAcceptanceDecisionV1 {
            root: root.clone(),
            attempt: attempt.clone(),
            action: id.clone(),
            revision,
            completion: completion.clone(),
            reject,
            now_us,
        };
        self.store.commit_acceptance(&proof, &self.witnesses)
    }
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no production binding is attached; the reference bindings are test-only"
        )
    )]
    pub(in crate::physical) fn configure_physical_handover(
        &mut self,
        ingress: &LocalCoreIngressV1,
        proof: TrustedHandoverPolicyV1,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        self.store.configure_handover(&proof)
    }
    pub(in crate::physical) fn reconcile_physical_action(
        &mut self,
        ingress: &LocalCoreIngressV1,
        id: &ActionId,
        request_handover: bool,
    ) -> AppResult<PhysicalReconciliationV1> {
        self.validate_ingress(ingress)?;
        require(
            self.store.evidence_host(id)? == *self.runtime.host_ref(),
            "Wrong Core evidence Host",
        )?;
        let now = self.evidence_now()?;
        self.store
            .reconcile(id, now, request_handover, &self.witnesses)
    }
}
