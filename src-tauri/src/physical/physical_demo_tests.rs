//! Physical demo acceptance: "walk from the living room to the bedroom".
//!
//! Specification: tests/physical_demo/README.md. One human approval authorizes
//! a finite decision stream; a brain (any tool caller) decides continuously;
//! the body must end up in the bedroom as judged by the binding's witness.
//! Pastey defines no brain loop: the brains below are test code that only
//! calls the tool surface Core exposes for the approved decision stream.
//!
//! The harness below replaced the original `missing(...)` stubs. The flat
//! and the dispenser are the reference bindings in `physical/bindings`; the
//! walk's brains run on another Host and reach the executor over the bridge
//! (physical-control-v2); the dispenser's brain is on the executor Host and
//! uses the optional local path. Simulated time is the Hosts' shared test
//! clock; `wait_ms` runs each installed stream's executor timer at its own
//! period, as `supervise_stream` does on a live Host. The test bodies and
//! assertions are the acceptance criteria and must not weaken.
use super::*;
use crate::physical::bindings::{
    dev::EnvelopeV1,
    dispenser::DispenserBodyV1,
    flat::FlatBodyV1,
    sim::{self, SimBindingV1, SimBodyV1, SimTruthV1},
};
use crate::physical::core::StreamRuntimeV1;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn label(value: &str) -> LabelV1 {
    LabelV1::try_from(value.to_owned()).unwrap()
}

// The approved envelope for the walk. The option names are the reference
// binding's; Core knows only names and payload digests.
const APPROVED_OPTIONS: [&str; 4] = ["forward", "turn_left", "turn_right", "stop"];
/// Declared by the binding but not approved for the walk.
const UNAPPROVED_OPTION: &str = "sprint";
const MAX_DECISIONS_PER_SECOND: u64 = 5;
const MAX_ACTION_MS: u64 = 500;
const MAX_TOTAL_EXECUTION_MS: u64 = 30_000;
const MAX_ACTIONS: u64 = 60;

// Criterion 5: a second, deliberately different body. A dispenser filling a
// cup: its own option names, payload schema (millilitres, not motion) and
// observation format, under the same DecisionStream and the same Core.
const DISPENSER_APPROVED: [&str; 2] = ["pour_small", "idle"];
const DISPENSER_UNAPPROVED: &str = "pour_large";
const DISPENSER_MAX_ACTION_MS: u64 = 1_000;
const DISPENSER_MAX_TOTAL_EXECUTION_MS: u64 = 5_000;
const DISPENSER_MAX_ACTIONS: u64 = 10;

/// Result of one tool call: admission only, never a physical consequence.
#[derive(Clone, Debug, PartialEq)]
enum ToolResultV1 {
    Allowed { disposition: String },
    Refused { reason: String },
}
impl ToolResultV1 {
    fn allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}

/// A brain's request. `option` is a name from the tool list.
#[derive(Clone, Debug)]
struct DecisionCallV1 {
    option: String,
    duration_ms: u64,
}

/// Remaining approved budget as reported by the budget query tool.
#[derive(Clone, Debug, PartialEq)]
struct RemainingBudgetV1 {
    actions: u64,
    execution_ms: u64,
}

/// One ledger step, recorded in three separate parts.
#[derive(Clone, Debug)]
struct StepRecordV1 {
    /// Who proposed: the tool caller identity.
    proposer: String,
    /// Who allowed or refused: Core's admission decision.
    admission: ToolResultV1,
    /// What the body did: the binding's disposition and observations for the
    /// admitted action, or nothing for a refused proposal.
    body: Option<Value>,
}

/// The witness's verdict on the completion contract ("in the bedroom").
#[derive(Clone, Debug, PartialEq)]
enum ConsequenceV1 {
    Verified,
    Uncertain,
}

/// The simulator's ground truth, for checking what the body actually did.
/// Tests may read it; Pastey never does.
#[derive(Clone, Debug)]
struct BodyTruthV1 {
    room: String,
    stopped: bool,
    executed_options: Vec<String>,
    executed_ms: u64,
}

/// Freshness shared by both bodies' scopes. The executor timer runs every
/// half observation gap.
const MAX_GAP_US: u64 = 400_000;
const TICK_US: u64 = MAX_GAP_US / 2;

/// Runs a Core future to completion from the synchronous harness API.
fn block<F: std::future::Future>(f: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(f))
}

const WALK: EnvelopeV1 = EnvelopeV1 {
    options: &["forward", "stop", "turn_left", "turn_right"],
    fields: &["/headingToGoal", "/room"],
    min_decision_interval_us: 1_000_000 / MAX_DECISIONS_PER_SECOND,
    action_us: MAX_ACTION_MS * 1000,
    total_us: MAX_TOTAL_EXECUTION_MS * 1000,
    count: MAX_ACTIONS as u32,
    lease_us: 60_000_000,
    idle_lease_us: MAX_ACTION_MS * 1000 + 1_000_000 / MAX_DECISIONS_PER_SECOND,
    approval_lifetime_us: 30_000_000,
    root_lifetime_us: 120_000_000,
    max_gap_us: MAX_GAP_US,
};
const POUR: EnvelopeV1 = EnvelopeV1 {
    options: &["idle", "pour_small"],
    fields: &["/cupMl", "/flowing"],
    min_decision_interval_us: 500_000,
    action_us: DISPENSER_MAX_ACTION_MS * 1000,
    total_us: DISPENSER_MAX_TOTAL_EXECUTION_MS * 1000,
    count: DISPENSER_MAX_ACTIONS as u32,
    lease_us: 60_000_000,
    idle_lease_us: DISPENSER_MAX_ACTION_MS * 1000 + 500_000,
    approval_lifetime_us: 30_000_000,
    root_lifetime_us: 120_000_000,
    max_gap_us: MAX_GAP_US,
};

/// Attaches one simulated environment the way a Host does (see
/// `bindings::dev::attach`).
fn attach<B: SimBodyV1>(
    core: &mut PhysicalControlServiceV1,
    lane: &Arc<SimBindingV1<B>>,
    requester: &HostRef,
    envelope: &EnvelopeV1,
) -> (Arc<EnvironmentBindingV1>, PhysicalReviewScopeV1) {
    crate::physical::bindings::dev::attach(core, lane, requester, envelope).unwrap()
}

/// The executor Host: one Core with the reference bindings attached, and the
/// executor timers of every installed stream.
struct ExecutorV1 {
    paths: AppPaths,
    clock: Arc<Clock>,
    core: Arc<Mutex<PhysicalControlServiceV1>>,
    timers: Mutex<Vec<Arc<StreamRuntimeV1>>>,
    flat: Arc<SimBindingV1<FlatBodyV1>>,
    flat_scope: PhysicalReviewScopeV1,
    cup: Option<(
        Arc<SimBindingV1<DispenserBodyV1>>,
        Arc<EnvironmentBindingV1>,
        PhysicalReviewScopeV1,
    )>,
    /// The last walk's session, handed over before the next walk starts.
    last_walk: Mutex<Option<Arc<BodyControlSessionV1>>>,
}
impl Drop for ExecutorV1 {
    fn drop(&mut self) {
        let _ = self.core.lock().close();
        let _ = std::fs::remove_dir_all(&self.paths.app_data_dir);
    }
}
impl ExecutorV1 {
    /// Starts the executor Host: Core with its bindings' witnesses, the flat
    /// (offered over the bridge) and, with `with_cup`, the dispenser.
    fn launch(with_cup: bool) -> Arc<Self> {
        Self::launch_walk(with_cup, &WALK)
    }
    /// As `launch`, offering the walk under `walk`.
    fn launch_walk(with_cup: bool, walk: &EnvelopeV1) -> Arc<Self> {
        let dir =
            std::env::temp_dir().join(format!("pastey-physical-demo-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(dir.clone(), dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let clock = Arc::new(Clock::new());
        let mut registry = sim::witnesses::<FlatBodyV1>().unwrap();
        for (id, witness) in sim::witnesses::<DispenserBodyV1>().unwrap().entries() {
            registry = registry.with(id.clone(), witness.clone());
        }
        let mut core = PhysicalControlServiceV1::new(
            &paths,
            LocalRuntimeRef::fresh(host("executor")),
            clock.clone(),
            registry,
        )
        .unwrap();
        let flat =
            Arc::new(SimBindingV1::<FlatBodyV1>::launch(&host("executor"), clock.clone()).unwrap());
        let (flat_live, flat_scope) = attach(&mut core, &flat, &host("requester"), walk);
        let cup = with_cup.then(|| {
            let cup = Arc::new(
                SimBindingV1::<DispenserBodyV1>::launch(&host("executor"), clock.clone()).unwrap(),
            );
            let (live, scope) = attach(&mut core, &cup, &host("executor"), &POUR);
            (cup, live, scope)
        });
        // The walk is what this Host offers over the bridge. Qualifying an
        // environment makes it the offered one, so this comes last.
        let i = core.local_ingress().unwrap();
        core.attach_product_environment(
            &i,
            ProductEnvironmentV1 {
                binding: flat_live,
                adapter: flat.clone(),
            },
        )
        .unwrap();
        Arc::new(Self {
            paths,
            clock,
            core: Arc::new(Mutex::new(core)),
            timers: Mutex::new(Vec::new()),
            flat,
            flat_scope,
            cup,
            last_walk: Mutex::new(None),
        })
    }
    /// Lets simulated time pass; every stream's timer ticks on its period.
    fn wait_ms(&self, ms: u64) {
        let mut left = ms * 1000;
        while left > 0 {
            let step = left.min(TICK_US);
            left -= step;
            let wall = self.clock.wall.load(Ordering::SeqCst) + step / 1000;
            let ticks = self.clock.ticks.load(Ordering::SeqCst) + step;
            self.clock.set(wall, ticks);
            let timers = self.timers.lock().clone();
            for stream in timers {
                let tick = block(PhysicalControlServiceV1::stream_tick(&self.core, &stream));
                if !matches!(tick, Ok(StreamTickV1::Continue(_))) {
                    self.timers.lock().retain(|t| !Arc::ptr_eq(t, &stream));
                }
            }
        }
    }
    fn running(&self, stream: &Arc<StreamRuntimeV1>) -> bool {
        self.timers.lock().iter().any(|t| Arc::ptr_eq(t, stream))
    }
    /// A walk for a brain on a new requester Host, under a new approval of
    /// the same offered scope. A previous walk's body is handed over first.
    fn walk(self: &Arc<Self>) -> DemoV1 {
        let previous = self.last_walk.lock().take();
        if let Some(session) = previous {
            self.hand_over(&session);
        }
        let mut pair = RequesterV1::new(self);
        // The person approves on the requester Host.
        let start_message = pair.approve();
        let PhysicalOperationV1::Start { review } = &start_message.operation else {
            panic!("not a Start");
        };
        DemoV1 {
            executor: self.clone(),
            body: BodyV1::Flat(self.flat.clone()),
            approval_digest: review.scope_digest.clone(),
            path: Mutex::new(PathV1::Bridge {
                pair,
                start_message,
                tools: BTreeMap::new(),
            }),
            session: Mutex::new(None),
        }
    }
    /// The dispenser, approved on the executor Host for a local brain.
    fn pour(self: &Arc<Self>) -> DemoV1 {
        let (cup, live, scope) = self.cup.clone().expect("launched without the dispenser");
        let mut c = self.core.lock();
        let i = c.local_ingress().unwrap();
        let r = c.draft_review(&i, &live, scope.clone()).unwrap();
        c.seal_review(&i, &r.review_id, r.revision, &r.scope_digest)
            .unwrap();
        let (now, _) = self.clock.read().unwrap();
        let a = c
            .approve_review(
                &i,
                &r.review_id,
                r.revision,
                &r.scope_digest,
                label("operator"),
                // The longest approval the scope allows.
                UnixMillis::try_from(
                    now.get() + scope.fields().stream.approval_lifetime_us.get() / 1000,
                )
                .unwrap(),
            )
            .unwrap();
        drop(c);
        DemoV1 {
            executor: self.clone(),
            body: BodyV1::Cup(cup.clone()),
            approval_digest: r.scope_digest,
            path: Mutex::new(PathV1::Local {
                live,
                scope,
                approval: a.approval_id,
                lane: cup,
                tools: BTreeMap::new(),
            }),
            session: Mutex::new(None),
        }
    }
    /// Safe handover of an ended walk. The fenced session keeps its domain
    /// quarantined; the Host observes the fenced body (sealed evidence only),
    /// and the witness must see it at rest before reconciliation releases
    /// the domain. Then a person carries the body back to the living room.
    fn hand_over(&self, session: &Arc<BodyControlSessionV1>) {
        let audit = lane::session_audit(session);
        let policy = HandoverPredicateV1 {
            version: VersionV2,
            session: audit.id.clone(),
            qualification_digest: audit.qualification_digest.clone(),
            predicate: FlatBodyV1::handover_contract().unwrap(),
            required_witness: WitnessClassV1::SimulationOracle,
            dwell_us: micros(200_000),
            freshness: self.flat_scope.fields().freshness.observation.clone(),
        };
        {
            let mut c = self.core.lock();
            let i = c.local_ingress().unwrap();
            c.configure_physical_handover(&i, producer::handover(policy))
                .unwrap();
        }
        let lane: &dyn crate::physical::core::EnvironmentBinding = self.flat.as_ref();
        for _ in 0..3 {
            self.wait_ms(TICK_US / 1000);
            let sample = lane.observe().unwrap();
            let mut c = self.core.lock();
            let i = c.local_ingress().unwrap();
            c.ingest_binding_sample(&i, session, lane, sample, false)
                .unwrap();
        }
        let mut c = self.core.lock();
        let i = c.local_ingress().unwrap();
        let action = core_fake::store(&c)
            .latest_dispatched_action(&audit.root)
            .unwrap()
            .unwrap();
        c.evaluate_physical_consequence(&i, &action).unwrap();
        let reconciled = c.reconcile_physical_action(&i, &action, true).unwrap();
        assert!(reconciled.holder_released, "{reconciled:?}");
        drop(c);
        self.flat.carry(FlatBodyV1::place_at_start).unwrap();
    }
}

/// The brain's Host: its own Core, where the person approves, joined to the
/// executor by one bridge route. It only relays.
struct RequesterV1 {
    a_paths: AppPaths,
    a: PhysicalControlServiceV1,
    b: Arc<Mutex<PhysicalControlServiceV1>>,
    ab: HostSessionBinding,
    ba: HostSessionBinding,
    route: Arc<AtomicBool>,
}
impl Drop for RequesterV1 {
    fn drop(&mut self) {
        let _ = self.a.close();
        let _ = std::fs::remove_dir_all(&self.a_paths.app_data_dir);
    }
}
impl RequesterV1 {
    fn new(executor: &ExecutorV1) -> Self {
        let a_paths = AppPaths::new(
            std::env::temp_dir().join(format!("pastey-physical-demo-a-{}", uuid::Uuid::new_v4())),
            std::path::PathBuf::new(),
        );
        a_paths.ensure_directories().unwrap();
        storage::init_database(&a_paths).unwrap();
        let a = PhysicalControlServiceV1::new(
            &a_paths,
            LocalRuntimeRef::fresh(host("requester")),
            executor.clock.clone(),
            witnesses(),
        )
        .unwrap();
        let binding = |bridge: &str, local: &str, peer: &str, l: &str, p: &str, route: &str| {
            HostSessionBinding::new(bridge, host(local), host(peer), l, p, route, 2000).unwrap()
        };
        Self {
            a_paths,
            a,
            b: executor.core.clone(),
            ab: binding("bridge", "requester", "executor", "a", "b", "route-b"),
            ba: binding("bridge", "executor", "requester", "b", "a", "route-a"),
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
        let mut b = self.b.lock();
        let proof = self.proof(&mut b, self.ba.clone());
        b.receive_physical(proof, m)
    }
    fn deliver_a(&mut self, m: PhysicalMessageV1) {
        let proof = {
            let route = self.route.clone();
            AuthenticatedPhysicalPeerV1::fake(
                core_fake::runtime(&self.a),
                self.ab.clone(),
                Arc::new(move || {
                    crate::physical::require(route.load(Ordering::Acquire), "Route invalidated")
                }),
            )
        };
        let proof = self.a.verified_peer_ingress(proof).unwrap();
        self.a.receive_physical(proof, m).unwrap();
    }
    fn product(
        &mut self,
        request: PhysicalProductRequestV1,
    ) -> (PhysicalProductViewV1, Option<PhysicalMessageV1>) {
        self.a.physical_product(&self.ab, request).unwrap()
    }
    /// Discovery, review and approval on this Host; returns the Start.
    fn approve(&mut self) -> PhysicalMessageV1 {
        let (_, m) = self.product(PhysicalProductRequestV1::Discover);
        let (reply, _) = self.deliver_b(m.unwrap()).unwrap();
        self.deliver_a(reply.unwrap());
        let (view, _) = self.product(PhysicalProductRequestV1::Snapshot);
        assert_eq!(view.offers.len(), 1);
        let (view, _) = self.product(PhysicalProductRequestV1::Compose {
            offer_digest: view.offers[0].scope_digest.clone(),
        });
        let r = view.review.unwrap();
        let (view, _) = self.product(PhysicalProductRequestV1::Approve {
            review_id: r.review_id,
            scope_digest: r.scope_digest,
        });
        let r = view.review.unwrap();
        let (_, m) = self.product(PhysicalProductRequestV1::Start {
            review_id: r.review_id,
            scope_digest: r.scope_digest,
        });
        m.unwrap()
    }
}

/// The body under test: its binding, so the harness can read ground truth.
enum BodyV1 {
    Flat(Arc<SimBindingV1<FlatBodyV1>>),
    Cup(Arc<SimBindingV1<DispenserBodyV1>>),
}
impl BodyV1 {
    fn truth(&self) -> SimTruthV1 {
        match self {
            Self::Flat(b) => b.truth(),
            Self::Cup(b) => b.truth(),
        }
        .unwrap()
    }
}

/// How brains reach the stream.
enum PathV1 {
    /// Brains on the requester Host; every tool request crosses the bridge.
    Bridge {
        pair: RequesterV1,
        start_message: PhysicalMessageV1,
        tools: BTreeMap<String, (RequestId, Vec<String>)>,
    },
    /// Brains on the executor Host, through the optional local tool path.
    Local {
        live: Arc<EnvironmentBindingV1>,
        scope: PhysicalReviewScopeV1,
        approval: ApprovalId,
        lane: Arc<dyn crate::physical::core::EnvironmentBinding>,
        tools: BTreeMap<String, Arc<ToolSessionV1>>,
    },
}

/// One running demo: a reference body attached to an executor Core, and one
/// approved decision stream for it. The stream starts on first tool use; a
/// started stream whose brain stays idle ends by its idle lease.
struct DemoV1 {
    executor: Arc<ExecutorV1>,
    body: BodyV1,
    approval_digest: DigestV1,
    path: Mutex<PathV1>,
    session: Mutex<Option<(Arc<BodyControlSessionV1>, Arc<StreamRuntimeV1>)>>,
}
impl DemoV1 {
    /// Body starts in the living room; one approval is granted.
    fn living_room_to_bedroom() -> Self {
        ExecutorV1::launch(false).walk()
    }
    /// The walk and a dispenser, each with its own binding and approval,
    /// attached to one Core instance.
    fn two_bodies_one_core() -> (Self, Self) {
        let executor = ExecutorV1::launch(true);
        (executor.walk(), executor.pour())
    }
    /// Starts the approved stream once: the executor installs the session and
    /// its timer begins.
    fn started(&self) -> Arc<BodyControlSessionV1> {
        if let Some((s, _)) = self.session.lock().as_ref() {
            return s.clone();
        }
        let core = &self.executor.core;
        let (session, stream) = match &mut *self.path.lock() {
            PathV1::Bridge {
                pair,
                start_message,
                ..
            } => {
                let (reply, work) = pair.deliver_b(start_message.clone()).unwrap();
                let work = work.unwrap();
                let PhysicalWorkKindV1::Install { session, .. } = &work.0 else {
                    panic!("not an installation");
                };
                let session = session.clone();
                pair.deliver_a(reply.unwrap());
                let done =
                    block(PhysicalControlServiceV1::perform_physical_work(core, work)).unwrap();
                *self.executor.last_walk.lock() = Some(session.clone());
                (session, done.stream.unwrap())
            }
            PathV1::Local {
                live,
                scope,
                approval,
                lane,
                ..
            } => {
                let session = {
                    let mut c = core.lock();
                    let i = c.local_ingress().unwrap();
                    let root = Arc::new(c.start_approved_root(&i, approval, live.clone()).unwrap());
                    let basis = Arc::new(
                        c.construct_grant_basis(
                            &root,
                            scope.clone(),
                            SessionEnforcementClassV1::AdapterIsolationOnly,
                        )
                        .unwrap(),
                    );
                    c.reserve_control_session(root, basis).unwrap()
                };
                block(PhysicalControlServiceV1::install_control_session(
                    core,
                    &session,
                    lane.as_ref(),
                ))
                .unwrap();
                let stream = core.lock().stream_runtime(&session, lane.clone()).unwrap();
                (session, stream)
            }
        };
        self.executor.timers.lock().push(stream.clone());
        *self.session.lock() = Some((session.clone(), stream));
        session
    }
    fn root(&self) -> RootId {
        lane::session_audit(&self.started()).root
    }
    fn status(&self) -> PhysicalStatusV1 {
        let root = self.root();
        core_fake::store(&self.executor.core.lock())
            .physical_status(&root)
            .unwrap()
    }
    /// One tool request from `caller`. The caller's tool session opens on its
    /// first request; a refusal to open is the caller's refusal.
    fn tool(&self, caller: &str, call: Option<DecisionToolCallV1>) -> Result<Tool, String> {
        self.started();
        let core = &self.executor.core;
        match &mut *self.path.lock() {
            PathV1::Bridge {
                pair,
                start_message,
                tools,
            } => {
                let start = start_message.semantic_id.clone();
                if !tools.contains_key(caller) {
                    let (view, m) = pair
                        .a
                        .physical_product(
                            &pair.ab,
                            PhysicalProductRequestV1::ToolOpen {
                                start: start.clone(),
                                caller: label(caller),
                            },
                        )
                        .map_err(|e| e.message().to_owned())?;
                    let id = view.tool_request.unwrap();
                    let (reply, _) = pair
                        .deliver_b(m.unwrap())
                        .map_err(|e| e.message().to_owned())?;
                    pair.deliver_a(reply.unwrap());
                    match read_tool(pair, &id) {
                        ToolOutcomeV1::Opened {
                            tool_session,
                            tools: names,
                        } => {
                            tools.insert(caller.to_owned(), (tool_session, names));
                        }
                        other => return Err(format!("{other:?}")),
                    }
                }
                let (tool_session, names) = tools[caller].clone();
                let Some(call) = call else {
                    return Ok(Tool::Names(names));
                };
                let (view, m) = pair
                    .a
                    .physical_product(
                        &pair.ab,
                        PhysicalProductRequestV1::ToolCall {
                            start,
                            tool_session,
                            call,
                        },
                    )
                    .map_err(|e| e.message().to_owned())?;
                let id = view.tool_request.unwrap();
                let (reply, work) = pair
                    .deliver_b(m.unwrap())
                    .map_err(|e| e.message().to_owned())?;
                let reply = match work {
                    Some(work) => {
                        block(PhysicalControlServiceV1::perform_physical_work(core, work))
                            .unwrap()
                            .reply
                    }
                    None => reply,
                };
                pair.deliver_a(reply.unwrap());
                match read_tool(pair, &id) {
                    ToolOutcomeV1::Reply { reply } => Ok(Tool::Reply(reply)),
                    ToolOutcomeV1::Failed { reason } => Err(reason),
                    other => Err(format!("{other:?}")),
                }
            }
            PathV1::Local { lane, tools, .. } => {
                if !tools.contains_key(caller) {
                    let session = self.started();
                    let mut c = core.lock();
                    let i = c.local_ingress().unwrap();
                    let ts = c
                        .open_tool_session(&i, &session, lane.clone(), label(caller))
                        .map_err(|e| e.message().to_owned())?;
                    tools.insert(caller.to_owned(), ts);
                }
                let ts = tools[caller].clone();
                let Some(call) = call else {
                    return Ok(Tool::Names(core.lock().tool_names(&ts)));
                };
                block(PhysicalControlServiceV1::call_tool(core, &ts, call))
                    .map(Tool::Reply)
                    .map_err(|e| e.message().to_owned())
            }
        }
    }

    /// Identity of the Core instance serving this demo.
    fn core_identity(&self) -> String {
        format!("{:p}", Arc::as_ptr(&self.executor.core))
    }
    /// Approvals granted where the person approved (the brain's Host for the
    /// walk, the executor for the local dispenser).
    fn approval_count(&self) -> u64 {
        let count: i64 = match &*self.path.lock() {
            PathV1::Bridge { pair, .. } => rusqlite::Connection::open(&pair.a_paths.db_path)
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM physical_remote_reviews \
                     WHERE json_extract(record_json,'$.approval') IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap(),
            PathV1::Local { .. } => self
                .executor_sql()
                .query_row(
                    "SELECT count(*) FROM physical_reviews WHERE approval_id IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap(),
        };
        count as u64
    }
    fn approval_digest(&self) -> String {
        String::from(self.approval_digest.clone())
    }
    /// Tool names exposed to a caller: the approved options plus the
    /// observation and remaining-budget queries.
    fn tool_names(&self, caller: &str) -> Vec<String> {
        match self.tool(caller, None) {
            Ok(Tool::Names(names)) => names,
            other => panic!("no tool list: {other:?}"),
        }
    }
    fn observe(&self, caller: &str) -> Value {
        match self.tool(caller, Some(DecisionToolCallV1::Observe)) {
            Ok(Tool::Reply(DecisionToolReplyV1::Observation { view })) => {
                serde_json::to_value(view).unwrap()
            }
            _ => Value::Null,
        }
    }
    fn remaining(&self, caller: &str) -> RemainingBudgetV1 {
        match self.tool(caller, Some(DecisionToolCallV1::RemainingBudget)) {
            Ok(Tool::Reply(DecisionToolReplyV1::Budget {
                actions,
                execution_us,
            })) => RemainingBudgetV1 {
                actions,
                execution_ms: execution_us / 1000,
            },
            other => panic!("no budget: {other:?}"),
        }
    }
    fn call(&self, caller: &str, call: &DecisionCallV1) -> ToolResultV1 {
        let decide = DecisionToolCallV1::Decide {
            option: call.option.clone(),
            duration_us: call.duration_ms * 1000,
        };
        match self.tool(caller, Some(decide)) {
            Ok(Tool::Reply(DecisionToolReplyV1::Allowed { disposition })) => {
                ToolResultV1::Allowed {
                    disposition: serde_json::to_value(disposition)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_owned(),
                }
            }
            Ok(Tool::Reply(DecisionToolReplyV1::Refused { reason })) | Err(reason) => {
                ToolResultV1::Refused { reason }
            }
            Ok(other) => panic!("not a decision reply: {other:?}"),
        }
    }
    /// Lets simulated time pass without any tool call.
    fn wait_ms(&self, ms: u64) {
        self.executor.wait_ms(ms);
    }
    fn revoke(&self) {
        self.started();
        let core = &self.executor.core;
        match &mut *self.path.lock() {
            PathV1::Bridge {
                pair,
                start_message,
                ..
            } => {
                let (_, m) = pair
                    .a
                    .physical_product(
                        &pair.ab,
                        PhysicalProductRequestV1::Cancel {
                            start: start_message.semantic_id.clone(),
                        },
                    )
                    .unwrap();
                let (reply, work) = pair.deliver_b(m.unwrap()).unwrap();
                if let Some(work) = work {
                    block(PhysicalControlServiceV1::perform_physical_work(core, work)).unwrap();
                }
                pair.deliver_a(reply.unwrap());
            }
            PathV1::Local { lane, .. } => {
                let session = self.started();
                block(PhysicalControlServiceV1::revoke_control_session(
                    core,
                    &session,
                    lane.as_ref(),
                ))
                .unwrap();
            }
        }
    }
    fn disconnect_bridge(&self) {
        let PathV1::Bridge { pair, .. } = &mut *self.path.lock() else {
            panic!("a local stream has no bridge");
        };
        pair.route.store(false, Ordering::Release);
        self.executor
            .core
            .lock()
            .invalidate_physical_bridge("bridge")
            .unwrap();
    }
    /// The caller's tool session ends (the brain process died mid-action).
    /// A dead process sends nothing: the executor only sees it go idle.
    fn brain_crashed(&self, caller: &str) {
        match &mut *self.path.lock() {
            PathV1::Bridge { tools, .. } => {
                tools.remove(caller);
            }
            PathV1::Local { tools, .. } => {
                tools.remove(caller);
            }
        }
    }
    /// A fresh route/brain after a loss; must not resume anything.
    fn recover(&self) {
        if let PathV1::Bridge { pair, .. } = &mut *self.path.lock() {
            if !pair.route.load(Ordering::Acquire) {
                pair.ab = HostSessionBinding::new(
                    "bridge-2",
                    host("requester"),
                    host("executor"),
                    "a2",
                    "b2",
                    "route-b2",
                    2000,
                )
                .unwrap();
                pair.ba = HostSessionBinding::new(
                    "bridge-2",
                    host("executor"),
                    host("requester"),
                    "b2",
                    "a2",
                    "route-a2",
                    2000,
                )
                .unwrap();
                pair.route.store(true, Ordering::Release);
            }
        }
    }
    /// The executor's record of the stream's consequence, once its timer has
    /// run to the end of the stream (bounded).
    fn consequence(&self) -> ConsequenceV1 {
        self.started();
        let stream = self.session.lock().as_ref().unwrap().1.clone();
        for _ in 0..(5_000_000 / TICK_US) {
            if !self.executor.running(&stream) {
                break;
            }
            self.wait_ms(TICK_US / 1000);
        }
        match self.status().consequence {
            ConsequenceStateV1::Verified => ConsequenceV1::Verified,
            ConsequenceStateV1::OutcomeUnknown
            | ConsequenceStateV1::Partial
            | ConsequenceStateV1::Unobserved => ConsequenceV1::Uncertain,
            ConsequenceStateV1::Contradicted => panic!("the witness contradicted the task"),
        }
    }
    fn executor_sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.executor.paths.db_path).unwrap()
    }
    fn records(&self) -> Vec<StepRecordV1> {
        let root = self.root();
        let sql = self.executor_sql();
        core_fake::store(&self.executor.core.lock())
            .decisions(&root)
            .unwrap()
            .into_iter()
            .map(|r| {
                let body = r.action.as_ref().map(|a| {
                    let (disposition, observations): (String, i64) = sql
                        .query_row(
                            "SELECT a.disposition,(SELECT count(*) FROM physical_evidence e WHERE e.action_id=a.action_id AND e.kind='observation') FROM physical_actions a WHERE a.action_id=?1",
                            [String::from(a.clone())],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .unwrap();
                    json!({"disposition": disposition, "observations": observations})
                });
                StepRecordV1 {
                    proposer: r.proposer,
                    admission: match &body {
                        Some(b) if r.allowed => ToolResultV1::Allowed {
                            disposition: b["disposition"].as_str().unwrap().to_owned(),
                        },
                        _ => ToolResultV1::Refused { reason: r.reason },
                    },
                    body,
                }
            })
            .collect()
    }
    fn sim_truth(&self) -> SimTruthV1 {
        self.body.truth()
    }
    fn truth(&self) -> BodyTruthV1 {
        let t = self.sim_truth();
        BodyTruthV1 {
            room: t.view["room"].as_str().unwrap_or_default().to_owned(),
            stopped: !t.moving && !t.action_running,
            executed_options: t.executed,
            executed_ms: t.executed_us / 1000,
        }
    }
    /// The Host re-installs this environment's executor policy.
    fn reconfigure_policy(&self) -> crate::error::AppResult<()> {
        let PathV1::Local { live, scope, .. } = &*self.path.lock() else {
            panic!("only the local stream's policy is reconfigured here");
        };
        let mut c = self.executor.core.lock();
        let i = c.local_ingress().unwrap();
        c.configure_executor_policy(
            &i,
            live,
            scope.clone(),
            SessionEnforcementClassV1::AdapterIsolationOnly,
            micros(120_000_000),
        )
    }
    fn consumed(&self) -> (u64, u64) {
        let (count, us): (i64, i64) = self
            .executor_sql()
            .query_row(
                "SELECT reserved_count,reserved_us FROM physical_control_budgets WHERE root_id=?1",
                [String::from(self.root())],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        (count as u64, us as u64 / 1000)
    }
}
#[derive(Debug)]
enum Tool {
    Names(Vec<String>),
    Reply(DecisionToolReplyV1),
}
fn read_tool(pair: &mut RequesterV1, id: &RequestId) -> ToolOutcomeV1 {
    pair.a
        .physical_product(
            &pair.ab,
            PhysicalProductRequestV1::ToolResult {
                request: id.clone(),
            },
        )
        .unwrap()
        .0
        .tool
        .unwrap()
}

/// A brain is any tool caller. It sees tool names and observations and returns
/// its next call, or `None` when it thinks it is done.
trait BrainV1 {
    fn id(&self) -> &str;
    fn next(&mut self, tools: &[String], observation: &Value) -> Option<DecisionCallV1>;
}

/// Rule controller written against the reference binding's observation schema.
struct RuleBrainV1;
impl BrainV1 for RuleBrainV1 {
    fn id(&self) -> &str {
        "brain:rules"
    }
    fn next(&mut self, _tools: &[String], observation: &Value) -> Option<DecisionCallV1> {
        if observation["room"] == "bedroom" {
            return None;
        }
        let option = match observation["headingToGoal"].as_str() {
            Some("left") => "turn_left",
            Some("right") => "turn_right",
            _ => "forward",
        };
        Some(DecisionCallV1 {
            option: option.into(),
            duration_ms: 400,
        })
    }
}

/// A mock LLM agent: it receives a prompt with the tool list and the
/// observation and answers with a JSON tool call, as a model would.
struct MockLlmBrainV1;
impl MockLlmBrainV1 {
    fn complete(&self, prompt: &str) -> String {
        let observation: Value = serde_json::from_str(
            prompt
                .split_once("OBSERVATION:")
                .map(|(_, o)| o.trim())
                .unwrap_or("{}"),
        )
        .unwrap_or(Value::Null);
        if observation["room"] == "bedroom" {
            return r#"{"done": true}"#.into();
        }
        let tool = match observation["headingToGoal"].as_str() {
            Some("left") => "turn_left",
            Some("right") => "turn_right",
            _ => "forward",
        };
        json!({"tool": tool, "durationMs": 300}).to_string()
    }
}
impl BrainV1 for MockLlmBrainV1 {
    fn id(&self) -> &str {
        "brain:llm-mock"
    }
    fn next(&mut self, tools: &[String], observation: &Value) -> Option<DecisionCallV1> {
        let prompt = format!("TOOLS: {}\nOBSERVATION: {observation}", tools.join(", "));
        let reply: Value = serde_json::from_str(&self.complete(&prompt)).ok()?;
        Some(DecisionCallV1 {
            option: reply["tool"].as_str()?.into(),
            duration_ms: reply["durationMs"].as_u64()?,
        })
    }
}

/// Drives one brain until it stops, runs out of approved budget or is
/// refused because the stream closed. Returns (allowed, refused) counts.
fn drive(demo: &DemoV1, brain: &mut dyn BrainV1) -> (u64, u64) {
    let (mut allowed, mut refused) = (0, 0);
    for _ in 0..(MAX_ACTIONS * 2) {
        let tools = demo.tool_names(brain.id());
        let observation = demo.observe(brain.id());
        let Some(call) = brain.next(&tools, &observation) else {
            break;
        };
        if demo.call(brain.id(), &call).allowed() {
            allowed += 1;
        } else {
            refused += 1;
        }
        demo.wait_ms(1000 / MAX_DECISIONS_PER_SECOND);
    }
    (allowed, refused)
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_1_brains_are_replaceable_under_one_approval() {
    let brains: Vec<Box<dyn BrainV1>> = vec![Box::new(RuleBrainV1), Box::new(MockLlmBrainV1)];
    let mut digests = Vec::new();
    // One executor Host offers the same scope to each brain in turn.
    let executor = ExecutorV1::launch(false);
    for mut brain in brains {
        let demo = executor.walk();
        // Same approval shape for every brain; nothing brain-specific in Pastey.
        digests.push(demo.approval_digest());
        let mut tools = demo.tool_names(brain.id());
        tools.sort();
        let mut expected: Vec<String> = APPROVED_OPTIONS
            .iter()
            .map(|o| o.to_string())
            .chain(["observe".into(), "remaining_budget".into()])
            .collect();
        expected.sort();
        assert_eq!(tools, expected, "{}", brain.id());
        let (allowed, _) = drive(&demo, brain.as_mut());
        assert!(allowed > 1, "{} decided continuously", brain.id());
        assert_eq!(demo.approval_count(), 1, "{}", brain.id());
        assert_eq!(
            demo.consequence(),
            ConsequenceV1::Verified,
            "{}",
            brain.id()
        );
        assert_eq!(demo.truth().room, "bedroom", "{}", brain.id());
    }
    assert!(digests.windows(2).all(|d| d[0] == d[1]));
}

/// Calls outside the envelope, then floods it.
struct BadBrainV1 {
    calls: u64,
}
impl BrainV1 for BadBrainV1 {
    fn id(&self) -> &str {
        "brain:bad"
    }
    fn next(&mut self, _tools: &[String], _observation: &Value) -> Option<DecisionCallV1> {
        self.calls += 1;
        let call = match self.calls % 4 {
            0 => DecisionCallV1 {
                option: UNAPPROVED_OPTION.into(),
                duration_ms: 100,
            },
            1 => DecisionCallV1 {
                option: "forward".into(),
                duration_ms: MAX_ACTION_MS * 10,
            },
            2 => DecisionCallV1 {
                option: "teleport".into(),
                duration_ms: 100,
            },
            _ => DecisionCallV1 {
                option: "forward".into(),
                duration_ms: MAX_ACTION_MS,
            },
        };
        (self.calls <= MAX_ACTIONS * 4).then_some(call)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_2_a_bad_brain_cannot_leave_the_envelope() {
    let demo = DemoV1::living_room_to_bedroom();
    let bad = "brain:bad";
    // Out-of-envelope proposals are refused, one by one.
    for call in [
        DecisionCallV1 {
            option: UNAPPROVED_OPTION.into(),
            duration_ms: 100,
        },
        DecisionCallV1 {
            option: "teleport".into(),
            duration_ms: 100,
        },
        DecisionCallV1 {
            option: "forward".into(),
            duration_ms: MAX_ACTION_MS + 1,
        },
        DecisionCallV1 {
            option: "forward".into(),
            duration_ms: 0,
        },
    ] {
        assert!(!demo.call(bad, &call).allowed(), "{call:?}");
    }
    // Faster than the approved decision rate: the burst beyond it is refused.
    let burst: Vec<_> = (0..MAX_DECISIONS_PER_SECOND * 3)
        .map(|_| {
            demo.call(
                bad,
                &DecisionCallV1 {
                    option: "forward".into(),
                    duration_ms: 100,
                },
            )
        })
        .collect();
    assert!(burst.iter().filter(|r| r.allowed()).count() as u64 <= MAX_DECISIONS_PER_SECOND);
    // Flooding until the budget is gone never crosses the approval.
    drive(&demo, &mut BadBrainV1 { calls: 0 });
    let (actions, execution_ms) = demo.consumed();
    assert!(actions <= MAX_ACTIONS && execution_ms <= MAX_TOTAL_EXECUTION_MS);
    assert_eq!(
        demo.remaining(bad),
        RemainingBudgetV1 {
            actions: MAX_ACTIONS - actions,
            execution_ms: MAX_TOTAL_EXECUTION_MS - execution_ms,
        }
    );
    // What the body actually did stays inside the approval.
    let truth = demo.truth();
    assert!(truth
        .executed_options
        .iter()
        .all(|o| APPROVED_OPTIONS.contains(&o.as_str())));
    assert!(truth.executed_ms <= MAX_TOTAL_EXECUTION_MS);
    assert!(truth.executed_options.len() as u64 <= MAX_ACTIONS);
    // Every refusal is on record with its reason; nothing refused reached the body.
    for r in demo.records() {
        if let ToolResultV1::Refused { reason } = &r.admission {
            assert!(!reason.is_empty());
            assert!(r.body.is_none());
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_3_revoke_disconnect_and_crash_stop_the_body_and_never_resume() {
    for loss in ["revoke", "disconnect_bridge", "brain_crash"] {
        let demo = DemoV1::living_room_to_bedroom();
        let brain = "brain:rules";
        let forward = DecisionCallV1 {
            option: "forward".into(),
            duration_ms: MAX_ACTION_MS,
        };
        assert!(demo.call(brain, &forward).allowed(), "{loss}");
        match loss {
            "revoke" => demo.revoke(),
            "disconnect_bridge" => demo.disconnect_bridge(),
            _ => demo.brain_crashed(brain),
        }
        // The binding's declared loss policy (local timeout self-stop) must
        // stop the body within the action's own bound, without Pastey.
        demo.wait_ms(MAX_ACTION_MS * 2);
        assert!(
            demo.truth().stopped,
            "{loss}: body stopped by its loss policy"
        );
        // Authority closed mid-action: Pastey records the outcome as
        // uncertain, never as arrival or as failure.
        assert_eq!(demo.consequence(), ConsequenceV1::Uncertain, "{loss}");
        // Recovery never resumes: no queued decision, no automatic call.
        let executed = demo.truth().executed_options.len();
        demo.recover();
        demo.wait_ms(MAX_ACTION_MS * 4);
        assert_eq!(demo.truth().executed_options.len(), executed, "{loss}");
        assert!(
            !demo.call(brain, &forward).allowed(),
            "{loss}: needs a new approval"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_4_proposer_admission_and_body_are_recorded_apart_and_the_witness_judges_arrival(
) {
    let demo = DemoV1::living_room_to_bedroom();
    let mut brain = RuleBrainV1;
    // One refused proposal from a second caller, then the rule brain's walk.
    assert!(!demo
        .call(
            "brain:other",
            &DecisionCallV1 {
                option: UNAPPROVED_OPTION.into(),
                duration_ms: 100,
            },
        )
        .allowed());
    let (allowed, refused) = drive(&demo, &mut brain);
    // The rule brain stays inside the envelope: it is never refused.
    assert_eq!(refused, 0);
    let records = demo.records();
    assert_eq!(records.len() as u64, allowed + refused + 1);
    for r in &records {
        assert!(!r.proposer.is_empty());
        match &r.admission {
            // An allowed step carries the body's own record, separately.
            ToolResultV1::Allowed { disposition } => {
                assert!(!disposition.is_empty());
                assert!(r.body.is_some(), "allowed step has a body record");
            }
            ToolResultV1::Refused { .. } => assert!(r.body.is_none()),
        }
    }
    assert!(records.iter().any(|r| r.proposer == "brain:other"));
    assert!(records.iter().any(|r| r.proposer == "brain:rules"));
    // Arrival is the witness's verdict, not an ACK and not the brain's claim.
    assert_eq!(demo.consequence(), ConsequenceV1::Verified);
    assert_eq!(demo.truth().room, "bedroom");
}

#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_5_bodies_are_replaceable_under_the_same_core() {
    let (walk, pour) = DemoV1::two_bodies_one_core();
    // Same Core instance, same DecisionStream machinery; nothing body-specific.
    assert_eq!(walk.core_identity(), pour.core_identity());
    let caller = "brain:rules";
    // Option-subset approval: only the approved options become tools.
    let mut tools = pour.tool_names(caller);
    tools.sort();
    let mut expected: Vec<String> = DISPENSER_APPROVED
        .iter()
        .map(|o| o.to_string())
        .chain(["observe".into(), "remaining_budget".into()])
        .collect();
    expected.sort();
    assert_eq!(tools, expected);
    // The dispenser's observation is its own format, not the walk's.
    let observation = pour.observe(caller);
    assert!(observation.get("room").is_none());
    // Out-of-envelope proposals are refused.
    for call in [
        DecisionCallV1 {
            option: DISPENSER_UNAPPROVED.into(),
            duration_ms: 100,
        },
        DecisionCallV1 {
            option: "forward".into(),
            duration_ms: 100,
        },
        DecisionCallV1 {
            option: "pour_small".into(),
            duration_ms: DISPENSER_MAX_ACTION_MS + 1,
        },
    ] {
        assert!(!pour.call(caller, &call).allowed(), "{call:?}");
    }
    // Cumulative budget holds across decisions.
    let pour_small = DecisionCallV1 {
        option: "pour_small".into(),
        duration_ms: DISPENSER_MAX_ACTION_MS,
    };
    for _ in 0..(DISPENSER_MAX_ACTIONS * 2) {
        pour.call(caller, &pour_small);
        pour.wait_ms(DISPENSER_MAX_ACTION_MS);
    }
    let (actions, execution_ms) = pour.consumed();
    assert!(actions <= DISPENSER_MAX_ACTIONS);
    assert!(execution_ms <= DISPENSER_MAX_TOTAL_EXECUTION_MS);
    let truth = pour.truth();
    assert!(truth
        .executed_options
        .iter()
        .all(|o| DISPENSER_APPROVED.contains(&o.as_str())));
    // Revocation mid-stream on a fresh pair: uncertain, and nothing resumes.
    let (walk, pour) = DemoV1::two_bodies_one_core();
    assert!(pour.call(caller, &pour_small).allowed());
    pour.revoke();
    pour.wait_ms(DISPENSER_MAX_ACTION_MS * 2);
    assert!(pour.truth().stopped);
    assert_eq!(pour.consequence(), ConsequenceV1::Uncertain);
    let executed = pour.truth().executed_options.len();
    pour.recover();
    pour.wait_ms(DISPENSER_MAX_ACTION_MS * 2);
    assert_eq!(pour.truth().executed_options.len(), executed);
    assert!(!pour.call(caller, &pour_small).allowed());
    // The walk under the same Core is untouched by the dispenser's revocation.
    assert!(walk
        .call(
            caller,
            &DecisionCallV1 {
                option: "forward".into(),
                duration_ms: MAX_ACTION_MS,
            },
        )
        .allowed());
}

/// The end of a verified stream, in order: the witness verifies arrival, Core
/// accepts the task, the stream ends with a fence, and after the fence no
/// action is still executing on the body. Nothing runs afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn physical_demo_verified_acceptance_then_fence_leaves_no_action_executing() {
    let demo = DemoV1::living_room_to_bedroom();
    let (allowed, _) = drive(&demo, &mut RuleBrainV1);
    assert!(allowed > 1);
    // The witness verified the completion contract from stored observations...
    assert_eq!(demo.consequence(), ConsequenceV1::Verified);
    let status = demo.status();
    // ...Core accepted the task on that verdict (the scope says automatic)...
    assert_eq!(status.acceptance, AcceptanceStateV1::Accepted);
    // ...and the stream ended: authority closed, the session fenced.
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert!(status.quarantined);
    assert_eq!(
        demo.executor_sql()
            .query_row(
                "SELECT count(*) FROM physical_sessions WHERE fence_ack IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    // The body applied the fence and no admitted action is still running.
    let truth = demo.sim_truth();
    assert_eq!(truth.fences, 1);
    assert!(!truth.action_running && !truth.moving);
    assert_eq!(truth.view["room"], "bedroom");
    // Nothing executes afterwards, and a further decision is refused.
    let executed = truth.executed.len();
    demo.wait_ms(2_000);
    let later = demo.sim_truth();
    assert_eq!(later.executed.len(), executed);
    assert!(!later.action_running && !later.moving);
    assert!(!demo
        .call(
            "brain:rules",
            &DecisionCallV1 {
                option: "forward".into(),
                duration_ms: MAX_ACTION_MS,
            },
        )
        .allowed());
    assert_eq!(demo.sim_truth().executed.len(), executed);
}

/// Executor policy is per environment: re-installing the dispenser's policy
/// closes the dispenser's stream and leaves the walk's authority open.
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_change_closes_only_its_own_environment() {
    let (walk, pour) = DemoV1::two_bodies_one_core();
    let forward = DecisionCallV1 {
        option: "forward".into(),
        duration_ms: MAX_ACTION_MS,
    };
    let pour_small = DecisionCallV1 {
        option: "pour_small".into(),
        duration_ms: DISPENSER_MAX_ACTION_MS,
    };
    assert!(walk.call("brain:rules", &forward).allowed());
    assert!(pour.call("brain:rules", &pour_small).allowed());
    // The change closes the dispenser's Root before anything else. Closing
    // moved its domain epochs, so the new policy itself waits for the Host to
    // resolve the environment again; that part is not what this test checks.
    let _ = pour.reconfigure_policy();
    assert_eq!(pour.status().authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(walk.status().authority, PhysicalAuthorityStateV1::Open);
    walk.wait_ms(1000 / MAX_DECISIONS_PER_SECOND);
    assert!(!pour.call("brain:rules", &pour_small).allowed());
    assert!(walk.call("brain:rules", &forward).allowed());
}

/// The development switch attaches a reference body the way a Host does and
/// offers it over the bridge. It fails closed: an unknown body is refused,
/// and a Core started without the body's witnesses cannot qualify it.
#[test]
fn the_development_switch_offers_a_reference_body_and_fails_closed() {
    use crate::physical::bindings::dev::ReferenceBodyV1;
    assert!(ReferenceBodyV1::parse("robot").is_err());
    for (body, registered) in [
        (ReferenceBodyV1::Flat, true),
        (ReferenceBodyV1::Flat, false),
    ] {
        let dir =
            std::env::temp_dir().join(format!("pastey-physical-dev-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths::new(dir.clone(), dir.join("logs"));
        paths.ensure_directories().unwrap();
        storage::init_database(&paths).unwrap();
        let clock = Arc::new(Clock::new());
        let registry = if registered {
            body.witnesses().unwrap()
        } else {
            crate::physical::evidence::WitnessRegistryV1::default()
        };
        let mut core = PhysicalControlServiceV1::new(
            &paths,
            LocalRuntimeRef::fresh(host("executor")),
            clock.clone(),
            registry,
        )
        .unwrap();
        let attached = body.attach(&mut core, &host("executor"), clock);
        assert_eq!(attached.is_ok(), registered, "{attached:?}");
        assert_eq!(core_fake::has_product_environment(&core), registered);
        let _ = core.close();
        drop(core);
        let _ = std::fs::remove_dir_all(dir);
    }
    assert!(ReferenceBodyV1::Cup.witnesses().is_ok());
}

#[path = "mcp_tests.rs"]
mod mcp;
