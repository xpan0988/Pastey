//! Two independent Host runtimes/stores; fake native and authenticated route oracle.
use super::*;
use crate::physical::protocol::*;
use crate::room_control::AuthenticatedPhysicalPeerV1;
use std::sync::atomic::AtomicBool;

struct Pair {
    a_paths: AppPaths,
    a: PhysicalControlServiceV1,
    b: ControlFixture,
    ab: HostSessionBinding,
    ba: HostSessionBinding,
    route: Arc<AtomicBool>,
}
impl Pair {
    fn new() -> Self {
        let b = ControlFixture::new();
        let a_paths = AppPaths::new(
            std::env::temp_dir().join(format!("physical-a-{}", uuid::Uuid::new_v4())),
            PathBuf::new(),
        );
        a_paths.ensure_directories().unwrap();
        storage::init_database(&a_paths).unwrap();
        let a = PhysicalControlServiceV1::new(
            &a_paths,
            LocalRuntimeRef::fresh(host("requester")),
            b.clock.clone(),
        )
        .unwrap();
        let ab = HostSessionBinding::new(
            "bridge",
            host("requester"),
            host("executor"),
            "a",
            "b",
            "route-b",
            2000,
        )
        .unwrap();
        let ba = HostSessionBinding::new(
            "bridge",
            host("executor"),
            host("requester"),
            "b",
            "a",
            "route-a",
            2000,
        )
        .unwrap();
        let i = b.core.lock().local_ingress().unwrap();
        b.core
            .lock()
            .attach_product_environment(
                &i,
                ProductEnvironmentV1 {
                    binding: b.live.clone(),
                    adapter: Arc::new(FakeLane::new(vec![])),
                    run: None,
                },
            )
            .unwrap();
        Self {
            a_paths,
            a,
            b,
            ab,
            ba,
            route: Arc::new(AtomicBool::new(true)),
        }
    }
    fn proof(
        &self,
        core: &mut PhysicalControlServiceV1,
        binding: HostSessionBinding,
    ) -> Arc<VerifiedPeerCoreIngressV1> {
        let route = self.route.clone();
        let current = Arc::new(move || {
            crate::physical::require(route.load(Ordering::Acquire), "Route invalidated")
        });
        let proof = AuthenticatedPhysicalPeerV1::fake(core_fake::runtime(core), binding, current);
        core.verified_peer_ingress(proof).unwrap()
    }
    fn deliver_b(
        &self,
        m: PhysicalMessageV1,
    ) -> crate::error::AppResult<(Option<PhysicalMessageV1>, Option<PhysicalWorkV1>)> {
        let mut b = self.b.core.lock();
        let proof = self.proof(&mut b, self.ba.clone());
        b.receive_physical(proof, m)
    }
    fn deliver_a(&mut self, m: PhysicalMessageV1) {
        let route = self.route.clone();
        let proof = AuthenticatedPhysicalPeerV1::fake(
            core_fake::runtime(&self.a),
            self.ab.clone(),
            Arc::new(move || {
                crate::physical::require(route.load(Ordering::Acquire), "Route invalidated")
            }),
        );
        let proof = self.a.verified_peer_ingress(proof).unwrap();
        self.a.receive_physical(proof, m).unwrap();
    }
    fn approve(&mut self) -> PhysicalMessageV1 {
        let (_, m) = self
            .a
            .physical_product(&self.ab, PhysicalProductRequestV1::Discover)
            .unwrap();
        let (reply, work) = self.deliver_b(m.unwrap()).unwrap();
        assert!(work.is_none());
        self.deliver_a(reply.unwrap());
        let (view, _) = self
            .a
            .physical_product(&self.ab, PhysicalProductRequestV1::Snapshot)
            .unwrap();
        assert_eq!(view.offers.len(), 1);
        let (view, _) = self
            .a
            .physical_product(
                &self.ab,
                PhysicalProductRequestV1::Compose {
                    offer_digest: view.offers[0].scope_digest.clone(),
                },
            )
            .unwrap();
        let r = view.review.unwrap();
        assert_eq!(r.state, PhysicalReviewStateV1::Reviewed);
        assert!(view.start.is_none());
        let (saved, _) = self
            .a
            .physical_product(&self.ab, PhysicalProductRequestV1::Snapshot)
            .unwrap();
        assert!(saved.start.is_none());
        assert!(self
            .a
            .physical_product(
                &self.ab,
                PhysicalProductRequestV1::Start {
                    review_id: r.review_id.clone(),
                    scope_digest: r.scope_digest.clone()
                }
            )
            .is_err());
        let (view, _) = self
            .a
            .physical_product(
                &self.ab,
                PhysicalProductRequestV1::Approve {
                    review_id: r.review_id,
                    scope_digest: r.scope_digest,
                },
            )
            .unwrap();
        let r = view.review.unwrap();
        let (_, m) = self
            .a
            .physical_product(
                &self.ab,
                PhysicalProductRequestV1::Start {
                    review_id: r.review_id,
                    scope_digest: r.scope_digest,
                },
            )
            .unwrap();
        m.unwrap()
    }
    async fn start(&mut self) -> (PhysicalMessageV1, Arc<BodyControlSessionV1>) {
        let m = self.approve();
        let (reply, work) = self.deliver_b(m.clone()).unwrap();
        let work = work.unwrap();
        let session = match &work.0 {
            PhysicalWorkKindV1::Install { session, .. } => session.clone(),
            _ => panic!(),
        };
        self.deliver_a(reply.unwrap());
        PhysicalControlServiceV1::perform_physical_work(&self.b.core, work)
            .await
            .unwrap();
        (m, session)
    }
    fn query(&mut self, start: &RequestId) -> PhysicalStatusV1 {
        let (_, m) = self
            .a
            .physical_product(
                &self.ab,
                PhysicalProductRequestV1::Status {
                    start: start.clone(),
                },
            )
            .unwrap();
        let (reply, work) = self.deliver_b(m.unwrap()).unwrap();
        assert!(work.is_none());
        self.deliver_a(reply.unwrap());
        self.a
            .physical_product(&self.ab, PhysicalProductRequestV1::Snapshot)
            .unwrap()
            .0
            .status
            .unwrap()
    }
    fn assert_single(&self, actions: i64) {
        for table in [
            "physical_attempts",
            "physical_sessions",
            "physical_control_budgets",
        ] {
            assert_eq!(self.b.scalar(&format!("SELECT count(*) FROM {table}")), 1);
        }
        assert_eq!(
            self.b.scalar("SELECT count(*) FROM physical_actions"),
            actions
        );
    }
}

#[test]
fn a_new_exact_review_has_its_own_product_start_correlation() {
    let mut pair = Pair::new();
    let first = pair.approve();
    let second = pair.approve();
    assert_ne!(first.semantic_id, second.semantic_id);
    let (view, _) = pair
        .a
        .physical_product(&pair.ab, PhysicalProductRequestV1::Snapshot)
        .unwrap();
    assert_eq!(view.start, Some(second.semantic_id));
    assert_eq!(
        core_fake::store(&pair.a)
            .semantic_message(&pair.ab.peer_host_ref, &first.semantic_id)
            .unwrap(),
        first
    );
}
use std::path::PathBuf;
impl Drop for Pair {
    fn drop(&mut self) {
        let _ = self.a.close();
        let _ = std::fs::remove_dir_all(&self.a_paths.app_data_dir);
    }
}

#[tokio::test]
async fn remote_discovery_exact_approval_install_retry_and_changed_digest() {
    let mut p = Pair::new();
    let (m, _) = p.start().await;
    let status = p.query(&m.semantic_id);
    assert_eq!(status.installation, PhysicalInstallationStateV1::Active);
    assert_eq!(status.acceptance, AcceptanceStateV1::Pending);
    for _ in 0..3 {
        assert!(p.deliver_b(m.clone()).unwrap().1.is_none());
    }
    p.assert_single(0);
    assert_eq!(
        p.b.scalar("SELECT count(*) FROM physical_attempts WHERE role='executor_remote'"),
        1
    );
    let mut changed = m.clone();
    changed.operation = PhysicalOperationV1::Cancel {
        start: m.semantic_id.clone(),
    };
    assert!(p.deliver_b(changed).is_err());
    p.assert_single(0);
}
#[tokio::test]
async fn lost_start_reply_and_requester_restart_reuse_same_semantic_start() {
    let mut p = Pair::new();
    let m = p.approve();
    let (_, work) = p.deliver_b(m.clone()).unwrap();
    PhysicalControlServiceV1::perform_physical_work(&p.b.core, work.unwrap())
        .await
        .unwrap();
    let runtime = LocalRuntimeRef::fresh(host("requester"));
    p.a = PhysicalControlServiceV1::new(&p.a_paths, runtime, p.b.clock.clone()).unwrap();
    let PhysicalOperationV1::Start { review } = &m.operation else {
        panic!()
    };
    let (_, retry) =
        p.a.physical_product(
            &p.ab,
            PhysicalProductRequestV1::Start {
                review_id: review.review_id.clone(),
                scope_digest: review.scope_digest.clone(),
            },
        )
        .unwrap();
    assert_eq!(retry.unwrap(), m);
    assert!(p.deliver_b(m.clone()).unwrap().1.is_none());
    assert_eq!(
        p.query(&m.semantic_id).installation,
        PhysicalInstallationStateV1::Active
    );
    p.assert_single(0);
}
#[tokio::test]
async fn executor_restart_consumed_start_is_history_only() {
    let mut p = Pair::new();
    let (m, _) = p.start().await;
    let mut restarted = PhysicalControlServiceV1::new(
        &p.b.paths,
        LocalRuntimeRef::fresh(host("executor")),
        p.b.clock.clone(),
    )
    .unwrap();
    let proof = p.proof(&mut restarted, p.ba.clone());
    let (reply, work) = restarted.receive_physical(proof, m.clone()).unwrap();
    assert!(work.is_none());
    let PhysicalOperationV1::Status { status, .. } = reply.unwrap().operation else {
        panic!()
    };
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert!(status.quarantined);
    p.assert_single(0);
}
#[tokio::test]
async fn route_replacement_burn_and_stale_packet_cannot_restore_authority() {
    let mut p = Pair::new();
    let (m, s) = p.start().await;
    let (g, proposal) = p.b.challenged(&s);
    let action = p.b.admit(&g, proposal);
    p.route.store(false, Ordering::Release);
    assert!(PhysicalControlServiceV1::dispatch_admitted_action(
        &p.b.core,
        &action,
        &FakeLane::new(vec![])
    )
    .await
    .is_err());
    p.b.core
        .lock()
        .invalidate_physical_bridge("bridge")
        .unwrap();
    p.route.store(true, Ordering::Release);
    p.ba = HostSessionBinding::new(
        "bridge",
        host("executor"),
        host("requester"),
        "b2",
        "a2",
        "new-route",
        2000,
    )
    .unwrap();
    assert!(p.deliver_b(m).is_err());
    p.assert_single(1);
    assert_eq!(p.b.session_state(), "quarantined");
}
#[tokio::test]
async fn remote_cancel_unknown_delivery_and_stop_ack_never_means_rest() {
    let mut p = Pair::new();
    let (m, _) = p.start().await;
    let (view, cancel) =
        p.a.physical_product(
            &p.ab,
            PhysicalProductRequestV1::Cancel {
                start: m.semantic_id.clone(),
            },
        )
        .unwrap();
    assert!(view.delivery_pending); // not delivered: executor still open
    assert_eq!(
        p.b.scalar("SELECT count(*) FROM physical_attempts WHERE state='open'"),
        1
    );
    let (_, work) = p.deliver_b(cancel.unwrap()).unwrap();
    PhysicalControlServiceV1::perform_physical_work(&p.b.core, work.unwrap())
        .await
        .unwrap();
    let status = p.query(&m.semantic_id);
    assert_eq!(status.acceptance, AcceptanceStateV1::Cancelled);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert!(status.quarantined);
    p.assert_single(0);
}
#[tokio::test]
async fn remote_action_result_l7_reconciliation_and_lost_reply_have_local_parity() {
    let mut p = Pair::new();
    let (m, s) = p.start().await;
    let action = {
        let mut core = p.b.core.lock();
        core.record_control_observation(&s, lane::observation(&s, 0, 0))
            .unwrap();
        core.admit_reference_action(s.clone()).unwrap()
    };
    PhysicalControlServiceV1::dispatch_admitted_action(&p.b.core, &action, &FakeLane::new(vec![]))
        .await
        .unwrap();
    assert!(PhysicalControlServiceV1::dispatch_admitted_action(
        &p.b.core,
        &action,
        &FakeLane::new(vec![])
    )
    .await
    .is_err());
    let lineage = core_fake::store(&p.b.core.lock())
        .evidence_lineage(action.id())
        .unwrap();
    // Reuse the Stage 5 fixture's production evaluator/acceptance and exact trace.
    let local = EvidenceFixture::new().await;
    local.trace(|_| {});
    let local_result = local.evaluate();
    local.decide(&local_result, false).unwrap();
    for (seq, time, kind) in [
        (1, 1_000_000, DispositionV1::Accepted),
        (2, 1_100_000, DispositionV1::Terminal),
    ] {
        p.b.clock.set(time / 1000, time - 1_000_000);
        let mut core = p.b.core.lock();
        let i = core.local_ingress().unwrap();
        core.record_physical_disposition(
            &i,
            producer::disposition(PhysicalActionDispositionV1 {
                lineage: lineage.clone(),
                id: RequestId::try_from(format!("physical-request:v1:{}", uuid::Uuid::new_v4()))
                    .unwrap(),
                sequence: seq,
                capture_us: time,
                disposition: kind,
                fence_request: None,
            }),
        )
        .unwrap();
    }
    for i in 0..=5 {
        let time = 1_110_000 + i * 100_000;
        p.b.clock.set(time / 1000, time - 1_000_000);
        let mut core = p.b.core.lock();
        let ingress = core.local_ingress().unwrap();
        let mut o = local.observation(i + 1, time);
        o.lineage = lineage.clone();
        core.record_physical_observation(&ingress, producer::observation(o))
            .unwrap();
    }
    {
        let mut core = p.b.core.lock();
        let i = core.local_ingress().unwrap();
        let x = core.evaluate_physical_consequence(&i, action.id()).unwrap();
        assert_eq!(x.state, local_result.state);
        assert_eq!(
            core.decide_physical_acceptance(
                &i,
                &x.root,
                &x.attempt,
                &x.action,
                x.revision,
                &x.completion_digest,
                false
            )
            .unwrap(),
            AcceptanceStateV1::Accepted
        );
    }
    // No result was delivered during execution; status query repairs observation.
    let status = p.query(&m.semantic_id);
    assert_eq!(status.acceptance, AcceptanceStateV1::Accepted);
    assert_eq!(status.consequence, ConsequenceStateV1::Verified);
    let (_, r) =
        p.a.physical_product(
            &p.ab,
            PhysicalProductRequestV1::Reconcile {
                start: m.semantic_id.clone(),
            },
        )
        .unwrap();
    let (reply, work) = p.deliver_b(r.unwrap()).unwrap();
    assert!(work.is_none());
    p.deliver_a(reply.unwrap());
    assert_eq!(
        p.query(&m.semantic_id).reconciliation,
        PhysicalReconciliationStateV1::Recorded
    );
    assert!(p.deliver_b(m).unwrap().1.is_none());
    p.assert_single(1);
    assert_eq!(
        p.b.scalar("SELECT sum(dispatch_intent) FROM physical_actions"),
        1
    );
    assert_eq!(
        p.b.scalar("SELECT reserved_count FROM physical_control_budgets"),
        1
    );
}
#[test]
fn protocol_versions_variants_bounds_and_no_authority_deserialization() {
    let mut p = Pair::new();
    let m = p.approve();
    let value = serde_json::to_value(&m).unwrap();
    for edit in 0..4 {
        let mut changed = value.clone();
        match edit {
            0 => changed["protocol"] = json!("physical-control-v2"),
            1 => changed["operation"]["kind"] = json!("enable"),
            2 => changed["sessionPair"] = json!("x".repeat(1000)),
            _ => changed["root"] = json!({}),
        };
        assert!(serde_json::from_value::<PhysicalMessageV1>(changed).is_err());
    }
    let mut q = crate::peer_capabilities::local_projection("b".into(), 1000);
    assert!(q.require_physical_protocol().is_err());
    let _ = &mut q;
}
#[test]
fn old_local_history_is_preserved_with_explicit_remote_schema() {
    let f = ControlFixture::new();
    let _ = f.root_basis();
    let sql = f.sql();
    let raw: String = sql
        .query_row("SELECT audit_json FROM physical_attempts", [], |r| r.get(0))
        .unwrap();
    assert!(!raw.contains("remoteLineage"));
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_attempts WHERE role='requester_executor'"),
        1
    );
    assert_eq!(f.scalar("SELECT version FROM physical_remote_schema"), 1);
}

#[tokio::test]
async fn cancel_before_start_is_durable_and_blocks_late_start_without_a_root() {
    let mut p = Pair::new();
    let m = p.approve();
    let (_, cancel) =
        p.a.physical_product(
            &p.ab,
            PhysicalProductRequestV1::Cancel {
                start: m.semantic_id.clone(),
            },
        )
        .unwrap();
    let (_, work) = p.deliver_b(cancel.unwrap()).unwrap();
    assert!(work.is_none());
    let (reply, work) = p.deliver_b(m.clone()).unwrap();
    assert!(work.is_none());
    p.deliver_a(reply.unwrap());
    assert_eq!(p.b.scalar("SELECT count(*) FROM physical_attempts"), 0);
    assert_eq!(
        p.query(&m.semantic_id).acceptance,
        AcceptanceStateV1::Cancelled
    );
    let PhysicalOperationV1::Start { review } = m.operation else {
        panic!()
    };
    assert!(p
        .a
        .physical_product(
            &p.ab,
            PhysicalProductRequestV1::Start {
                review_id: review.review_id,
                scope_digest: review.scope_digest
            }
        )
        .is_err());
}

#[tokio::test]
async fn fresh_route_can_reconcile_history_but_cannot_replay_old_start() {
    let mut p = Pair::new();
    let (m, _) = p.start().await;
    p.b.core
        .lock()
        .invalidate_physical_bridge("bridge")
        .unwrap();
    p.ab = HostSessionBinding::new(
        "bridge",
        host("requester"),
        host("executor"),
        "a2",
        "b2",
        "route-b2",
        2000,
    )
    .unwrap();
    p.ba = HostSessionBinding::new(
        "bridge",
        host("executor"),
        host("requester"),
        "b2",
        "a2",
        "route-a2",
        2000,
    )
    .unwrap();
    assert!(p.deliver_b(m.clone()).is_err());
    let status = p.query(&m.semantic_id);
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.acceptance, AcceptanceStateV1::Cancelled);
    p.assert_single(0);
}

#[tokio::test]
async fn lost_install_ack_cannot_retry_install_or_create_another_session() {
    let mut p = Pair::new();
    let m = p.approve();
    {
        let mut core = p.b.core.lock();
        let i = core.local_ingress().unwrap();
        core.attach_product_environment(
            &i,
            ProductEnvironmentV1 {
                binding: p.b.live.clone(),
                adapter: Arc::new(FakeLane::new(vec![Reply::Lost])),
                run: None,
            },
        )
        .unwrap();
    }
    let (_, work) = p.deliver_b(m.clone()).unwrap();
    assert!(
        PhysicalControlServiceV1::perform_physical_work(&p.b.core, work.unwrap())
            .await
            .is_err()
    );
    assert!(p.deliver_b(m.clone()).unwrap().1.is_none());
    let status = p.query(&m.semantic_id);
    assert_eq!(
        status.installation,
        PhysicalInstallationStateV1::Quarantined
    );
    assert_ne!(status.acceptance, AcceptanceStateV1::Accepted);
    p.assert_single(0);
}
#[tokio::test]
async fn lost_action_ack_and_lost_result_keep_one_dispatch_and_reserved_budget() {
    let mut p = Pair::new();
    let (m, s) = p.start().await;
    let action = {
        let mut core = p.b.core.lock();
        core.record_control_observation(&s, lane::observation(&s, 0, 0))
            .unwrap();
        core.admit_reference_action(s.clone()).unwrap()
    };
    assert!(PhysicalControlServiceV1::dispatch_admitted_action(
        &p.b.core,
        &action,
        &FakeLane::new(vec![Reply::Lost])
    )
    .await
    .is_err());
    assert!(p.deliver_b(m.clone()).unwrap().1.is_none());
    let status = p.query(&m.semantic_id);
    assert_ne!(status.acceptance, AcceptanceStateV1::Accepted);
    assert_eq!(status.dispatch, PhysicalDispatchStateV1::Unknown);
    p.assert_single(1);
    assert_eq!(
        p.b.scalar("SELECT sum(dispatch_intent) FROM physical_actions"),
        1
    );
    assert_eq!(
        p.b.scalar("SELECT reserved_count FROM physical_control_budgets"),
        1
    );
}
