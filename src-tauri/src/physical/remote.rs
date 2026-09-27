//! Remote ingress joins the existing Core/local execution path; no wire permit.
use super::*;
use crate::physical::{binding::EnvironmentBindingV1, protocol::*, store::RemoteRootLineageV2};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub(in crate::physical) struct ProductEnvironmentV1 {
    pub binding: Arc<EnvironmentBindingV1>,
    pub adapter: Arc<dyn PhysicalEnvironmentAdapterV1>,
    pub run: Option<Arc<microduck::GateARunV1>>,
}
#[derive(Default)]
pub(super) struct RemoteControlV1 {
    pub(super) environment: Option<ProductEnvironmentV1>,
    executions: BTreeMap<RequestId, Arc<BodyControlSessionV1>>,
    peers: Vec<Arc<VerifiedPeerCoreIngressV1>>,
}
pub(crate) struct PhysicalWorkV1(pub(in crate::physical) PhysicalWorkKindV1);
pub(in crate::physical) enum PhysicalWorkKindV1 {
    Install {
        start: RequestId,
        session: Arc<BodyControlSessionV1>,
        adapter: Arc<dyn PhysicalEnvironmentAdapterV1>,
        run: Option<Arc<microduck::GateARunV1>>,
    },
    Cancel {
        session: Arc<BodyControlSessionV1>,
        adapter: Arc<dyn PhysicalEnvironmentAdapterV1>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PhysicalProductRequestV1 {
    Discover,
    Compose {
        offer_digest: DigestV1,
    },
    Approve {
        review_id: ReviewId,
        scope_digest: DigestV1,
    },
    Start {
        review_id: ReviewId,
        scope_digest: DigestV1,
    },
    Status {
        start: RequestId,
    },
    Cancel {
        start: RequestId,
    },
    Reconcile {
        start: RequestId,
    },
    Snapshot,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhysicalOfferViewV1 {
    pub scope_digest: DigestV1,
    pub scope: PhysicalReviewScopeV1,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PhysicalProductViewV1 {
    pub offers: Vec<PhysicalOfferViewV1>,
    pub review: Option<PhysicalReviewRecordV1>,
    pub start: Option<RequestId>,
    pub status: Option<PhysicalStatusV1>,
    pub delivery_pending: bool,
}

impl PhysicalControlServiceV1 {
    /// This consumes only the sealed producer from authenticated Room Control.
    /// Neither a DTO nor a renderer-supplied session can call that producer.
    pub(crate) fn verified_peer_ingress(
        &mut self,
        p: crate::room_control::AuthenticatedPhysicalPeerV1,
    ) -> AppResult<Arc<VerifiedPeerCoreIngressV1>> {
        let (runtime, binding, current) = p.into_parts();
        runtime.validate_current(&self.runtime)?;
        current()?;
        // Retire the prior ingress and close its tasks before accepting a new route.
        let replaced = self
            .remote
            .peers
            .iter()
            .filter(|p| p.binding.peer_host_ref == binding.peer_host_ref && p.binding != binding)
            .cloned()
            .collect::<Vec<_>>();
        for p in replaced {
            self.invalidate_physical_peer(&p)?;
        }
        if let Some(old) = self
            .remote
            .peers
            .iter()
            .find(|p| p.binding == binding && p.authenticated.load(Ordering::Acquire))
        {
            return Ok(old.clone());
        }
        let proof = Arc::new(VerifiedPeerCoreIngressV1 {
            runtime,
            binding,
            authenticated: Arc::new(AtomicBool::new(true)),
            current: Some(current),
        });
        self.remote
            .peers
            .retain(|p| p.authenticated.load(Ordering::Acquire));
        require(
            self.remote.peers.len() < 64,
            "Too many physical peer proofs",
        )?;
        self.remote.peers.push(proof.clone());
        Ok(proof)
    }
    fn invalidate_physical_peer(&mut self, peer: &VerifiedPeerCoreIngressV1) -> AppResult<()> {
        peer.authenticated.store(false, Ordering::Release);
        let sessions = self
            .remote
            .executions
            .values()
            .filter(|s| {
                s.root()
                    .peer
                    .as_ref()
                    .is_some_and(|p| p.binding == peer.binding)
            })
            .cloned()
            .collect::<Vec<_>>();
        for s in sessions {
            self.close_root(s.root())?;
        }
        Ok(())
    }
    pub(crate) fn invalidate_physical_bridge(&mut self, bridge: &str) -> AppResult<()> {
        let peers = self
            .remote
            .peers
            .iter()
            .filter(|p| p.binding.bridge_id == bridge)
            .cloned()
            .collect::<Vec<_>>();
        // Close RAM for all peers first, even if durable closure subsequently fails.
        for p in &peers {
            p.authenticated.store(false, Ordering::Release);
        }
        let mut error = None;
        for p in peers {
            if let Err(e) = self.invalidate_physical_peer(&p) {
                error = Some(e);
            }
        }
        if let Some(e) = error {
            Err(e)
        } else {
            Ok(())
        }
    }
    pub(in crate::physical) fn attach_product_environment(
        &mut self,
        ingress: &LocalCoreIngressV1,
        environment: ProductEnvironmentV1,
    ) -> AppResult<()> {
        self.validate_ingress(ingress)?;
        self.binding.validate_current(&environment.binding)?;
        let ceiling = self
            .policy
            .as_ref()
            .ok_or_else(|| {
                crate::error::AppError::InvalidInput("Physical policy unavailable".into())
            })?
            .ceiling
            .clone();
        self.current_scope(&ceiling, &environment.binding)?;
        self.remote.environment = Some(environment);
        Ok(())
    }
    fn validate_peer_message(
        &mut self,
        p: &VerifiedPeerCoreIngressV1,
        m: &PhysicalMessageV1,
    ) -> AppResult<()> {
        m.validate()?;
        let (now, _) = self.binding.now()?;
        let (sender, receiver) = if m.operation.is_response() {
            (&m.executor, &m.requester)
        } else {
            (&m.requester, &m.executor)
        };
        p.validate(&self.runtime, &p.binding, sender, receiver, now)?;
        require(
            m.session_pair == p.binding.session_pair_ref,
            "Old-session physical packet",
        )
    }
    pub(crate) fn receive_physical(
        &mut self,
        p: Arc<VerifiedPeerCoreIngressV1>,
        m: PhysicalMessageV1,
    ) -> AppResult<(Option<PhysicalMessageV1>, Option<PhysicalWorkV1>)> {
        self.validate_peer_message(&p, &m)?;
        if m.operation.is_response() {
            let fresh = self.store.claim_semantic(&p.binding.peer_host_ref, &m)?;
            if !fresh {
                return Ok((None, None));
            }
        }
        if let PhysicalOperationV1::Environments { offers } = &m.operation {
            require(
                offers.iter().all(|s| {
                    s.fields().executor == m.executor && s.fields().requester == m.requester
                }),
                "Foreign physical offers",
            )?;
            let c = self.store.connection()?;
            c.execute("INSERT INTO physical_remote_offers VALUES(?1,?2,?3) ON CONFLICT(peer) DO UPDATE SET session_pair=excluded.session_pair,offers_json=excluded.offers_json",rusqlite::params![m.executor.as_str(),m.session_pair,serde_json::to_string(offers)?])?;
            return Ok((None, None));
        }
        if let PhysicalOperationV1::Status { start, status } = &m.operation {
            let original = self.store.semantic_message(&m.executor, start)?;
            require(
                matches!(original.operation, PhysicalOperationV1::Start { .. })
                    && original.requester == m.requester
                    && original.executor == m.executor,
                "Uncorrelated physical result",
            )?;
            if let Some(old) = self.store.semantic_result(&m.executor, start)? {
                require(
                    !matches!(
                        old.acceptance,
                        crate::physical::evidence::AcceptanceStateV1::Accepted
                            | crate::physical::evidence::AcceptanceStateV1::Rejected
                            | crate::physical::evidence::AcceptanceStateV1::Cancelled
                    ) || old.acceptance == status.acceptance,
                    "Late status cannot reverse terminal task acceptance",
                )?;
                require(
                    old.root.is_none() || old.root == status.root,
                    "Changed physical result lineage",
                )?;
            }
            // Only a historical projection is updated. It cannot construct authority.
            self.store
                .save_semantic_result(&m.executor, start, status)?;
            return Ok((None, None));
        }
        let fresh = self.store.claim_semantic(&m.requester, &m)?;
        let mut work = None;
        let operation = match &m.operation {
            PhysicalOperationV1::Discover => {
                let mut offers = Vec::new();
                if let Some(e) = &self.remote.environment {
                    let binding = e.binding.clone();
                    let mut f = self
                        .policy
                        .as_ref()
                        .ok_or_else(|| {
                            crate::error::AppError::InvalidInput(
                                "Physical policy unavailable".into(),
                            )
                        })?
                        .ceiling
                        .fields()
                        .clone();
                    f.requester = m.requester.clone();
                    let scope = PhysicalReviewScopeV1::try_from(f)?;
                    self.current_scope(&scope, &binding)?;
                    offers.push(scope);
                }
                PhysicalOperationV1::Environments { offers }
            }
            PhysicalOperationV1::Start { review } => {
                require(
                    review.scope.fields().requester == m.requester
                        && review.scope.fields().executor == m.executor,
                    "Remote review Host mismatch",
                )?;
                if fresh && !self.cancel_requested(&m.requester, &m.semantic_id, &m.session_pair)? {
                    let environment = self.remote.environment.as_ref().ok_or_else(|| {
                        crate::error::AppError::InvalidInput(
                            "No configured physical environment".into(),
                        )
                    })?;
                    let binding = environment.binding.clone();
                    let adapter = environment.adapter.clone();
                    let run = environment.run.clone();
                    self.current_scope(&review.scope, &binding)?;
                    let snapshot = self.binding.ledger_snapshot(&binding)?;
                    let (now, _) = self.binding.now()?;
                    self.store.import_approved_review(review, &snapshot, now)?;
                    let lineage = RemoteRootLineageV2 {
                        version: 2,
                        binding: p.binding.clone(),
                        semantic_id: m.semantic_id.clone(),
                        semantic_digest: m.digest()?,
                    };
                    let root = Arc::new(self.start_exact_action_inner(
                        &review.approval.as_ref().unwrap().approval_id,
                        binding,
                        Some(p.clone()),
                        Some(lineage),
                    )?);
                    let basis = Arc::new(
                        self.construct_grant_basis(
                            &root,
                            review.scope.clone(),
                            review
                                .scope
                                .fields()
                                .qualification
                                .required_enforcement_class,
                        )?,
                    );
                    let session = self.reserve_control_session(root, basis)?;
                    self.remote
                        .executions
                        .insert(m.semantic_id.clone(), session.clone());
                    work = Some(PhysicalWorkKindV1::Install {
                        start: m.semantic_id.clone(),
                        session,
                        adapter,
                        run,
                    });
                }
                PhysicalOperationV1::Status {
                    start: m.semantic_id.clone(),
                    status: self.remote_status(&m.requester, &m.semantic_id)?,
                }
            }
            PhysicalOperationV1::StatusQuery { start }
            | PhysicalOperationV1::Cancel { start }
            | PhysicalOperationV1::Reconcile { start } => {
                let original = self.store.semantic_message(&m.requester, start);
                if let Ok(original) = original {
                    require(
                        original.requester == m.requester
                            && original.executor == m.executor
                            && matches!(original.operation, PhysicalOperationV1::Start { .. }),
                        "Old-session/unrelated physical operation",
                    )?;
                } else {
                    require(
                        matches!(m.operation, PhysicalOperationV1::Cancel { .. })
                            || self.cancel_requested(&m.requester, start, &m.session_pair)?,
                        "Unknown physical Start correlation",
                    )?;
                }
                if fresh {
                    let status = self.remote_status(&m.requester, start)?;
                    if matches!(m.operation, PhysicalOperationV1::Cancel { .. }) {
                        if let Some(s) = self.remote.executions.get(start).cloned() {
                            self.close_root(s.root())?; // close task authority before a stop await
                            if let Some(e) = &self.remote.environment {
                                work = Some(PhysicalWorkKindV1::Cancel {
                                    session: s,
                                    adapter: e.adapter.clone(),
                                });
                            }
                        } else if let Some(root) = status.root {
                            self.store.close_attempt(&root, "revoked")?;
                        }
                    } else if matches!(m.operation, PhysicalOperationV1::Reconcile { .. }) {
                        if let Some(action) = status.action {
                            let i = self.local_ingress()?;
                            self.reconcile_physical_action(&i, &action, false)?;
                        }
                    }
                }
                PhysicalOperationV1::Status {
                    start: start.clone(),
                    status: self.remote_status(&m.requester, start)?,
                }
            }
            _ => {
                return Err(crate::error::AppError::InvalidInput(
                    "Unsupported physical direction".into(),
                ))
            }
        };
        let response = PhysicalMessageV1 {
            protocol: PROTOCOL.into(),
            semantic_id: request_id()?,
            session_pair: m.session_pair,
            requester: m.requester,
            executor: m.executor,
            operation,
        };
        Ok((Some(response), work.map(PhysicalWorkV1)))
    }
    fn remote_status(&self, peer: &HostRef, start: &RequestId) -> AppResult<PhysicalStatusV1> {
        if let Some(session) = self.remote.executions.get(start) {
            return self.store.physical_status(session.root().root_id());
        }
        let c = self.store.connection()?;
        let root:Option<String>=c.query_row("SELECT root_id FROM physical_attempts WHERE role='executor_remote' AND requester=?1 AND json_extract(audit_json,'$.remoteLineage.semanticId')=?2",rusqlite::params![peer.as_str(),String::from(start.clone())],|r|r.get(0)).optional()?;
        if let Some(root) = root {
            self.store.physical_status(&RootId::try_from(root)?)
        } else {
            let mut status = PhysicalStatusV1::pending();
            if self.cancel_requested(peer, start, "")? {
                status.authority = PhysicalAuthorityStateV1::Closed;
                status.acceptance = crate::physical::evidence::AcceptanceStateV1::Cancelled;
            }
            Ok(status)
        }
    }
    fn cancel_requested(&self, peer: &HostRef, start: &RequestId, pair: &str) -> AppResult<bool> {
        let c = self.store.connection()?;
        Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM physical_semantic_messages WHERE peer=?1 AND json_extract(message_json,'$.operation.kind')='cancel' AND json_extract(message_json,'$.operation.start')=?2 AND (?3='' OR session_pair=?3))",rusqlite::params![peer.as_str(),String::from(start.clone()),pair],|r|r.get(0))?)
    }
    pub(crate) fn physical_product(
        &mut self,
        b: &HostSessionBinding,
        request: PhysicalProductRequestV1,
    ) -> AppResult<(PhysicalProductViewV1, Option<PhysicalMessageV1>)> {
        require(
            b.local_host_ref == *self.runtime.host_ref(),
            "Foreign local physical requester",
        )?;
        let (now, _) = self.binding.now()?;
        require(
            b.expires_at > (now.get() / 1000) as i64,
            "Expired physical product route",
        )?;
        let mut view = self.product_snapshot(b)?;
        let mut operation = None;
        match request {
            PhysicalProductRequestV1::Snapshot => {}
            PhysicalProductRequestV1::Discover => operation = Some(PhysicalOperationV1::Discover),
            PhysicalProductRequestV1::Compose { offer_digest } => {
                let scope = view
                    .offers
                    .iter()
                    .find(|s| s.scope_digest == offer_digest)
                    .map(|s| s.scope.clone())
                    .ok_or_else(|| {
                        crate::error::AppError::InvalidInput("Stale/missing physical offer".into())
                    })?;
                require(
                    now < scope.fields().environment.offer_expiry
                        && now < scope.fields().qualification.expires_at,
                    "Stale physical review data",
                )?;
                let r = PhysicalReviewRecordV1 {
                    version: VersionV1,
                    review_id: ReviewId::try_from(format!(
                        "physical-review:v1:{}",
                        uuid::Uuid::new_v4()
                    ))?,
                    revision: 1,
                    scope_digest: scope.digest()?,
                    scope,
                    state: PhysicalReviewStateV1::Reviewed,
                    approval: None,
                };
                self.store.save_remote_review(&r, &b.session_pair_ref)?;
                view.review = Some(r);
                view.start = None;
                view.status = None;
                view.delivery_pending = false;
            }
            PhysicalProductRequestV1::Approve {
                review_id,
                scope_digest,
            } => {
                let mut r = self.store.remote_review(&review_id, &b.session_pair_ref)?;
                require(
                    r.state == PhysicalReviewStateV1::Reviewed
                        && r.scope_digest == scope_digest
                        && r.scope.fields().requester == b.local_host_ref
                        && r.scope.fields().executor == b.peer_host_ref,
                    "Invalid exact physical approval",
                )?;
                let expiry = now
                    .get()
                    .saturating_add(30_000)
                    .min(r.scope.fields().environment.offer_expiry.get())
                    .min(r.scope.fields().qualification.expires_at.get());
                require(now.get() < expiry, "Stale physical approval")?;
                r.approval = Some(ApprovalCorrelationV1 {
                    approval_id: ApprovalId::try_from(format!(
                        "physical-approval:v1:{}",
                        uuid::Uuid::new_v4()
                    ))?,
                    review_id: r.review_id.clone(),
                    review_revision: r.revision,
                    scope_digest: r.scope_digest.clone(),
                    principal: r.scope.fields().principal.clone(),
                    approved_at: now,
                    expires_at: UnixMillis::try_from(expiry)?,
                });
                r.state = PhysicalReviewStateV1::Approved;
                self.store.save_remote_review(&r, &b.session_pair_ref)?;
                view.review = Some(r);
            }
            PhysicalProductRequestV1::Start {
                review_id,
                scope_digest,
            } => {
                let r = self.store.remote_review(&review_id, &b.session_pair_ref)?;
                require(
                    r.scope_digest == scope_digest
                        && r.state == PhysicalReviewStateV1::Approved
                        && r.approval.as_ref().is_some_and(|a| now < a.expires_at),
                    "Stale/unapproved physical Start",
                )?;
                let c = self.store.connection()?;
                let old:Option<String>=c.query_row("SELECT message_json FROM physical_semantic_messages WHERE peer=?1 AND json_extract(message_json,'$.operation.review.reviewId')=?2",rusqlite::params![b.peer_host_ref.as_str(),String::from(review_id)],|r|r.get(0)).optional()?;
                if let Some(raw) = old {
                    let m: PhysicalMessageV1 = serde_json::from_str(&raw)?;
                    require(
                        m.session_pair == b.session_pair_ref,
                        "Original Start route invalidated",
                    )?;
                    require(
                        !self.cancel_requested(
                            &b.peer_host_ref,
                            &m.semantic_id,
                            &b.session_pair_ref,
                        )?,
                        "Physical Start has been cancelled locally",
                    )?;
                    view.start = Some(m.semantic_id.clone());
                    view.delivery_pending = true;
                    return Ok((view, Some(m)));
                }
                operation = Some(PhysicalOperationV1::Start { review: r });
            }
            PhysicalProductRequestV1::Status { start } => {
                operation = Some(PhysicalOperationV1::StatusQuery { start })
            }
            PhysicalProductRequestV1::Cancel { start } => {
                operation = Some(PhysicalOperationV1::Cancel { start })
            }
            PhysicalProductRequestV1::Reconcile { start } => {
                operation = Some(PhysicalOperationV1::Reconcile { start })
            }
        }
        if let Some(
            PhysicalOperationV1::StatusQuery { start }
            | PhysicalOperationV1::Cancel { start }
            | PhysicalOperationV1::Reconcile { start },
        ) = &operation
        {
            let original = self.store.semantic_message(&b.peer_host_ref, start)?;
            require(
                original.requester == b.local_host_ref
                    && original.executor == b.peer_host_ref
                    && matches!(original.operation, PhysicalOperationV1::Start { .. }),
                "Stale/unrelated physical product correlation",
            )?;
        }
        let message = operation.map(|operation| PhysicalMessageV1 {
            protocol: PROTOCOL.into(),
            semantic_id: request_id().expect("uuid"),
            session_pair: b.session_pair_ref.clone(),
            requester: b.local_host_ref.clone(),
            executor: b.peer_host_ref.clone(),
            operation,
        });
        if let Some(m) = &message {
            self.store.claim_semantic(&b.peer_host_ref, m)?;
            view.delivery_pending = true;
            if matches!(m.operation, PhysicalOperationV1::Start { .. }) {
                view.start = Some(m.semantic_id.clone());
                view.status = Some(PhysicalStatusV1::pending());
            }
        }
        Ok((view, message))
    }
    fn product_snapshot(&self, b: &HostSessionBinding) -> AppResult<PhysicalProductViewV1> {
        let c = self.store.connection()?;
        let offers: Option<String> = c
            .query_row(
                "SELECT offers_json FROM physical_remote_offers WHERE peer=?1 AND session_pair=?2",
                rusqlite::params![b.peer_host_ref.as_str(), b.session_pair_ref],
                |r| r.get(0),
            )
            .optional()?;
        let review:Option<String>=c.query_row("SELECT record_json FROM physical_remote_reviews WHERE session_pair=?1 AND json_extract(record_json,'$.scope.executor')=?2 ORDER BY rowid DESC LIMIT 1",rusqlite::params![b.session_pair_ref,b.peer_host_ref.as_str()],|r|r.get(0)).optional()?;
        let review = review
            .map(|r| serde_json::from_str::<PhysicalReviewRecordV1>(&r))
            .transpose()?;
        let review_id = review.as_ref().map(|r| String::from(r.review_id.clone()));
        let start:Option<String>=c.query_row("SELECT semantic_id FROM physical_semantic_messages WHERE peer=?1 AND json_extract(message_json,'$.operation.kind')='start' AND (?2 IS NULL OR json_extract(message_json,'$.operation.review.reviewId')=?2) ORDER BY rowid DESC LIMIT 1",rusqlite::params![b.peer_host_ref.as_str(),review_id],|r|r.get(0)).optional()?;
        let start = start.map(RequestId::try_from).transpose()?;
        let status = if let Some(id) = &start {
            self.store.semantic_result(&b.peer_host_ref, id)?
        } else {
            None
        };
        Ok(PhysicalProductViewV1 {
            offers: offers
                .map(|r| serde_json::from_str::<Vec<PhysicalReviewScopeV1>>(&r))
                .transpose()?
                .unwrap_or_default()
                .into_iter()
                .map(|scope| {
                    Ok(PhysicalOfferViewV1 {
                        scope_digest: scope.digest()?,
                        scope,
                    })
                })
                .collect::<AppResult<Vec<_>>>()?,
            review,
            delivery_pending: start.is_some() && status.is_none(),
            start,
            status,
        })
    }
}
use rusqlite::OptionalExtension;

impl PhysicalControlServiceV1 {
    pub(crate) async fn perform_physical_work(
        core: &Mutex<Self>,
        work: PhysicalWorkV1,
    ) -> AppResult<()> {
        match work.0 {
            PhysicalWorkKindV1::Cancel { session, adapter } => {
                Self::revoke_control_session(core, &session, adapter.as_ref()).await?;
            }
            PhysicalWorkKindV1::Install {
                session,
                adapter,
                run,
                ..
            } => {
                Self::install_control_session(core, &session, adapter.as_ref()).await?;
                if let Some(run) = run {
                    let prepared = async {
                        let fact = {
                            let run = run.clone();
                            tokio::task::spawn_blocking(move || run.poll_control())
                                .await
                                .map_err(|_| {
                                    crate::error::AppError::InvalidInput(
                                        "Physical observation task failed".into(),
                                    )
                                })??
                        };
                        let action = {
                            let mut service = core.lock();
                            let i = service.local_ingress()?;
                            service.ingest_gate_a(&i, &session, &run, fact, true)?;
                            service.admit_reference_action(session.clone())?
                        };
                        Ok::<_, crate::error::AppError>(action)
                    }
                    .await;
                    let action = match prepared {
                        Ok(action) => action,
                        Err(error) => {
                            let _ = Self::revoke_control_session(core, &session, adapter.as_ref())
                                .await;
                            return Err(error);
                        }
                    };
                    let native = microduck::MicroDuckAdapterV1::new(run.clone());
                    Self::run_gate_a_reference(core, &session, &action, run.clone(), &native)
                        .await?;
                    // Same Stage 5 evaluator and L7 acceptance; no transport ACK
                    // or installation/write/stop ACK can become completion.
                    let settling_deadline = {
                        let service = core.lock();
                        let (_, ticks) = service.clock.read()?;
                        ticks
                            .checked_add(
                                crate::physical::evidence::completion(session.root().scope())
                                    .settling_timeout_us
                                    .get(),
                            )
                            .ok_or_else(|| {
                                crate::error::AppError::InvalidInput(
                                    "Physical settling deadline overflow".into(),
                                )
                            })?
                    };
                    loop {
                        if core.lock().clock.read()?.1 >= settling_deadline {
                            break;
                        }
                        let fact = {
                            let run = run.clone();
                            tokio::task::spawn_blocking(move || run.poll_control())
                                .await
                                .map_err(|_| {
                                    crate::error::AppError::InvalidInput(
                                        "Physical observation task failed".into(),
                                    )
                                })??
                        };
                        let mut service = core.lock();
                        let i = service.local_ingress()?;
                        if let Some(peer) = &session.root().peer {
                            let (now, _) = service.binding.now()?;
                            if let Err(error) = peer.validate(
                                &service.runtime,
                                &peer.binding,
                                &session.root().audit.requester,
                                &session.root().audit.executor,
                                now,
                            ) {
                                service.close_root(session.root())?;
                                return Err(error);
                            }
                        }
                        service.ingest_gate_a(&i, &session, &run, fact, false)?;
                        let c = service.evaluate_physical_consequence(&i, action.id())?;
                        if c.state == crate::physical::evidence::ConsequenceStateV1::Verified {
                            service.decide_physical_acceptance(
                                &i,
                                &c.root,
                                &c.attempt,
                                &c.action,
                                c.revision,
                                &c.completion_digest,
                                false,
                            )?;
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn request_id() -> AppResult<RequestId> {
    RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
}
