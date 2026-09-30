//! Step C: decision streams through the executor-side tool dispatcher.
use super::*;
use crate::physical::core::{DecisionToolCallV1, DecisionToolReplyV1, StreamTickV1, ToolSessionV1};
use crate::physical::protocol::ToolOutcomeV1;

/// A fake binding sampling at the fixture clock's current time.
fn lane_for(f: &ControlFixture) -> Arc<lane::DescribedLane> {
    let b = binding();
    let mut enrollment = fake::enrollment(&b);
    let registration = fake::record(&mut enrollment).clone();
    let fingerprint = b.implementation_fingerprint.clone();
    let clock = f.clock.clone();
    Arc::new(lane::DescribedLane::new(
        move || crate::physical::binding::BindingDescriptionV1 {
            registration: registration.clone(),
            provenance_digest: digest_value(),
            conditions_digest: digest_value(),
            implementation_fingerprint: fingerprint.clone(),
        },
        move |_| clock.ticks.load(Ordering::SeqCst),
        0,
    ))
}
async fn open(
    f: &ControlFixture,
    described: &Arc<lane::DescribedLane>,
) -> (Arc<BodyControlSessionV1>, Arc<ToolSessionV1>) {
    let s = f.reserve();
    PhysicalControlServiceV1::install_control_session(&f.core, &s, described.as_ref())
        .await
        .unwrap();
    let lane: Arc<dyn crate::physical::core::EnvironmentBinding> = described.clone();
    let mut core = f.core.lock();
    let i = core.local_ingress().unwrap();
    let ts = core
        .open_tool_session(&i, &s, lane, caller("brain.rules"))
        .unwrap();
    (s, ts)
}
/// The active session of a stream fixture (there is exactly one).
fn ts_session(f: &ControlFixture) -> Arc<BodyControlSessionV1> {
    lane::only_session(&f.core.lock())
}
fn caller(name: &str) -> LabelV1 {
    LabelV1::try_from(name.to_owned()).unwrap()
}
fn decide(option: &str, ms: u64) -> DecisionToolCallV1 {
    DecisionToolCallV1::Decide {
        option: option.into(),
        duration_us: ms * 1000,
    }
}
async fn call(
    f: &ControlFixture,
    ts: &Arc<ToolSessionV1>,
    c: DecisionToolCallV1,
) -> DecisionToolReplyV1 {
    PhysicalControlServiceV1::call_tool(&f.core, ts, c)
        .await
        .unwrap()
}
fn allowed(r: &DecisionToolReplyV1) -> bool {
    matches!(r, DecisionToolReplyV1::Allowed { .. })
}
/// Lets time pass while the brain keeps observing every 100 ms, as a live
/// brain would; consecutive samples must stay under the observation gap.
async fn observe_for(f: &ControlFixture, ts: &Arc<ToolSessionV1>, ms: u64) {
    for _ in 0..ms / 100 {
        advance(f, 100);
        call(f, ts, DecisionToolCallV1::Observe).await;
    }
}
fn advance(f: &ControlFixture, ms: u64) {
    let wall = f.clock.wall.load(Ordering::SeqCst) + ms;
    let ticks = f.clock.ticks.load(Ordering::SeqCst) + ms * 1000;
    f.clock.set(wall, ticks);
}

#[tokio::test]
async fn stream_tools_are_the_approved_options_and_two_read_only_queries() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    let mut tools = f.core.lock().tool_names(&ts);
    tools.sort();
    assert_eq!(
        tools,
        [
            "forward",
            "observe",
            "remaining_budget",
            "stop",
            "turn_left",
            "turn_right"
        ]
    );
    // The observation is the binding's own view, passed through untouched.
    assert!(matches!(
        call(&f, &ts, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Observation { .. }
    ));
    assert_eq!(
        call(&f, &ts, DecisionToolCallV1::RemainingBudget).await,
        DecisionToolReplyV1::Budget {
            actions: 6,
            execution_us: 1_200_000
        }
    );
}

#[tokio::test]
async fn each_decision_is_admitted_recorded_rate_limited_and_replaces_the_last() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    let first = call(&f, &ts, decide("forward", 300)).await;
    assert_eq!(
        first,
        DecisionToolReplyV1::Allowed {
            disposition: "accepted".into()
        }
    );
    // Faster than the approved decision rate.
    let early = call(&f, &ts, decide("turn_left", 300)).await;
    assert!(matches!(&early, DecisionToolReplyV1::Refused { reason } if reason.contains("rate")));
    observe_for(&f, &ts, 200).await;
    assert!(allowed(&call(&f, &ts, decide("turn_left", 300)).await));
    // The replacement closed the first action in the same transaction.
    let rows: Vec<(i64, String)> = f
        .sql()
        .prepare("SELECT decision_sequence,state FROM physical_actions ORDER BY decision_sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(rows, [(1, "closed".into()), (2, "open".into())]);
    let records = f.core.lock().decision_records(&ts).unwrap();
    let summary: Vec<(u64, &str, &str, bool, bool)> = records
        .iter()
        .map(|r| {
            (
                r.sequence,
                r.proposer.as_str(),
                r.option.as_str(),
                r.allowed,
                r.action.is_some(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (1, "brain.rules", "forward", true, true),
            (2, "brain.rules", "turn_left", false, false),
            (3, "brain.rules", "turn_left", true, true),
        ]
    );
    assert_eq!(
        call(&f, &ts, DecisionToolCallV1::RemainingBudget).await,
        DecisionToolReplyV1::Budget {
            actions: 4,
            execution_us: 600_000
        }
    );
    // The strict ledger audit replays multi-action stream rows on restart.
    f.core.lock().close().unwrap();
    PhysicalControlServiceV1::new(
        &f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.clock.clone(),
        witnesses(),
    )
    .unwrap();
}

#[tokio::test]
async fn out_of_envelope_proposals_are_refused_recorded_and_never_reach_the_body() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    let before = described.lane.calls.load(Ordering::SeqCst);
    for (option, ms) in [
        ("sprint", 100),     // declared, not approved
        ("teleport", 100),   // not declared
        ("forward", 501),    // longer than one action may run
        ("forward", 0),      // no duration
        ("no spaces!", 100), // not an option name
    ] {
        let r = call(&f, &ts, decide(option, ms)).await;
        assert!(
            matches!(r, DecisionToolReplyV1::Refused { .. }),
            "{option} {ms}"
        );
    }
    // Installation was the lane's only call: no write reached the body.
    assert_eq!(described.lane.calls.load(Ordering::SeqCst), before);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 0);
    let records = f.core.lock().decision_records(&ts).unwrap();
    assert_eq!(records.len(), 5);
    assert!(records
        .iter()
        .all(|r| !r.allowed && r.action.is_none() && !r.reason.is_empty()));
}

#[tokio::test]
async fn the_cumulative_budget_holds_across_decisions() {
    // 0.9 s in total: three 300 ms decisions fit, a fourth does not, well
    // inside the root lifetime so only the cumulative ceiling can refuse it.
    let f = ControlFixture::stream(900_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    for _ in 0..3 {
        assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
        observe_for(&f, &ts, 200).await;
    }
    let over = call(&f, &ts, decide("forward", 300)).await;
    assert!(
        matches!(&over, DecisionToolReplyV1::Refused { reason } if reason.contains("budget")),
        "{over:?}"
    );
    assert_eq!(
        f.scalar("SELECT reserved_us FROM physical_control_budgets"),
        900_000
    );
    // Two actions in total: the third decision is refused by count.
    let f = ControlFixture::stream(1_200_000, 2);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    for expect in [true, true, false] {
        assert_eq!(allowed(&call(&f, &ts, decide("stop", 100)).await), expect);
        observe_for(&f, &ts, 200).await;
    }
}

#[tokio::test]
async fn closing_a_tool_session_ends_the_stream_uncertain_and_nothing_resumes() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 400)).await));
    PhysicalControlServiceV1::close_tool_session(&f.core, &ts)
        .await
        .unwrap();
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_sessions WHERE fence_ack IS NOT NULL"),
        1
    );
    assert_eq!(f.session_state(), "quarantined");
    let status = core_fake::store(&f.core.lock())
        .physical_status(&lane::session_audit(&s).root)
        .unwrap();
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert_ne!(status.acceptance, AcceptanceStateV1::Accepted);
    // The closed session and any new one are refused; the refusal is recorded.
    advance(&f, 200);
    assert!(!allowed(&call(&f, &ts, decide("forward", 100)).await));
    let lane: Arc<dyn crate::physical::core::EnvironmentBinding> = described.clone();
    let mut core = f.core.lock();
    let i = core.local_ingress().unwrap();
    assert!(core
        .open_tool_session(&i, &s, lane, caller("brain.next"))
        .is_err());
    let records = core.decision_records(&ts).unwrap();
    assert!(!records.last().unwrap().allowed);
    drop(core);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 1);
}

#[tokio::test]
async fn a_lost_binding_ends_the_stream_before_any_further_write() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 400)).await));
    described.fail_after.store(1, Ordering::SeqCst);
    advance(&f, 200);
    let lost = call(&f, &ts, decide("forward", 400)).await;
    assert!(matches!(lost, DecisionToolReplyV1::Refused { .. }));
    assert_eq!(f.session_state(), "quarantined");
    assert_eq!(f.scalar("SELECT count(*) FROM physical_actions"), 1);
    assert!(!allowed(&call(&f, &ts, decide("stop", 100)).await));
}

#[tokio::test]
async fn an_unmet_start_predicate_prevents_installation() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    described.start_ready.store(false, Ordering::SeqCst);
    let s = f.reserve();
    assert!(
        PhysicalControlServiceV1::install_control_session(&f.core, &s, described.as_ref())
            .await
            .is_err()
    );
    // The predicate is read-only: nothing was installed on the device.
    assert_eq!(described.lane.calls.load(Ordering::SeqCst), 0);
    assert_ne!(f.session_state(), "active");
}

#[tokio::test]
async fn observations_carry_only_declared_fields_at_the_declared_rate() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    let DecisionToolReplyV1::Observation { view } =
        call(&f, &ts, DecisionToolCallV1::Observe).await
    else {
        panic!("no observation");
    };
    // The binding's view also holds "secret" and "pose.y"; neither is declared.
    assert_eq!(
        serde_json::to_value(&view).unwrap(),
        json!({"pose": {"x": 1}, "sample": 1})
    );
    // Faster than the declared observation rate is refused.
    assert!(matches!(
        call(&f, &ts, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Refused { reason } if reason.contains("rate")
    ));
    advance(&f, 100);
    assert!(matches!(
        call(&f, &ts, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Observation { .. }
    ));
    // A tool session opens only for the declared destination Host.
    let lane: Arc<dyn crate::physical::core::EnvironmentBinding> = described.clone();
    let s = ts_session(&f);
    assert!(f
        .core
        .lock()
        .open_tool_session_inner(&s, lane, caller("brain.elsewhere"), &host("elsewhere"))
        .is_err());
}

/// Runs executor timer ticks every 100 ms of simulated time.
async fn tick_for(f: &ControlFixture, ts: &Arc<ToolSessionV1>, ms: u64) -> StreamTickV1 {
    let stream = lane::stream_of(ts);
    let mut last = StreamTickV1::Continue(std::time::Duration::ZERO);
    for _ in 0..ms / 100 {
        advance(f, 100);
        last = PhysicalControlServiceV1::stream_tick(&f.core, &stream)
            .await
            .unwrap();
        if last == StreamTickV1::Ended {
            break;
        }
    }
    last
}

#[tokio::test]
async fn the_timer_ends_an_idle_stream_like_a_crashed_brain() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
    // The timer keeps sampling on its own; the brain never calls again. The
    // idle lease is one action plus one decision interval (700 ms).
    assert!(matches!(
        tick_for(&f, &ts, 600).await,
        StreamTickV1::Continue(_)
    ));
    assert_eq!(f.session_state(), "active");
    assert_eq!(tick_for(&f, &ts, 200).await, StreamTickV1::Ended);
    assert_eq!(f.session_state(), "quarantined");
    let status = core_fake::store(&f.core.lock())
        .physical_status(&lane::session_audit(&s).root)
        .unwrap();
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert!(!allowed(&call(&f, &ts, decide("stop", 100)).await));
}

#[tokio::test]
async fn the_timer_ends_a_stream_whose_budget_is_spent() {
    // One action in total: once it ran out, nothing is left to authorize.
    let f = ControlFixture::stream(1_200_000, 1);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
    // While the action runs the stream stays open.
    assert!(matches!(
        tick_for(&f, &ts, 200).await,
        StreamTickV1::Continue(_)
    ));
    assert_eq!(tick_for(&f, &ts, 200).await, StreamTickV1::Ended);
    assert_eq!(f.session_state(), "quarantined");
}

#[tokio::test]
async fn the_supervisor_stops_once_the_stream_has_ended() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    let core = f.core.clone();
    let stream = lane::stream_of(&ts);
    let supervisor =
        tokio::spawn(
            async move { PhysicalControlServiceV1::supervise_stream(&core, stream).await },
        );
    PhysicalControlServiceV1::close_tool_session(&f.core, &ts)
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), supervisor)
        .await
        .expect("supervisor still running")
        .unwrap();
}

#[tokio::test]
async fn exact_scopes_have_no_decision_tools() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let lane: Arc<dyn crate::physical::core::EnvironmentBinding> = Arc::new(FakeLane::new(vec![]));
    let mut core = f.core.lock();
    let i = core.local_ingress().unwrap();
    assert!(core
        .open_tool_session(&i, &s, lane, caller("brain"))
        .is_err());
}

#[tokio::test]
async fn remote_tools_run_on_the_executor_and_die_with_the_bridge() {
    let executor = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&executor);
    {
        let mut core = executor.core.lock();
        let i = core.local_ingress().unwrap();
        core.attach_product_environment(
            &i,
            ProductEnvironmentV1 {
                binding: executor.live.clone(),
                adapter: described.clone(),
            },
        )
        .unwrap();
    }
    let mut p = Pair::with_executor(executor);
    let (start, _) = p.start().await;
    // The requester relays; the executor opens the session and answers.
    let request = |p: &mut Pair, r: PhysicalProductRequestV1| {
        let (view, m) = p.a.physical_product(&p.ab, r).unwrap();
        (view.tool_request.unwrap(), m.unwrap())
    };
    let (open_id, m) = request(
        &mut p,
        PhysicalProductRequestV1::ToolOpen {
            start: start.semantic_id.clone(),
            caller: caller("brain.remote"),
        },
    );
    let (reply, work) = p.deliver_b(m).unwrap();
    assert!(work.is_none());
    p.deliver_a(reply.unwrap());
    let read = |p: &mut Pair, id: &RequestId| {
        p.a.physical_product(
            &p.ab,
            PhysicalProductRequestV1::ToolResult {
                request: id.clone(),
            },
        )
        .unwrap()
        .0
        .tool
        .unwrap()
    };
    let ToolOutcomeV1::Opened {
        tool_session,
        tools,
    } = read(&mut p, &open_id)
    else {
        panic!("tool session not opened");
    };
    assert!(tools.contains(&"forward".into()) && !tools.contains(&"sprint".into()));
    // A call runs as executor work; its reply travels back over the bridge.
    let (call_id, m) = request(
        &mut p,
        PhysicalProductRequestV1::ToolCall {
            start: start.semantic_id.clone(),
            tool_session: tool_session.clone(),
            call: decide("forward", 400),
        },
    );
    let (reply, work) = p.deliver_b(m.clone()).unwrap();
    assert!(reply.is_none());
    let reply = PhysicalControlServiceV1::perform_physical_work(&p.b.core, work.unwrap())
        .await
        .unwrap()
        .reply
        .unwrap();
    p.deliver_a(reply);
    assert_eq!(
        read(&mut p, &call_id),
        ToolOutcomeV1::Reply {
            reply: DecisionToolReplyV1::Allowed {
                disposition: "accepted".into()
            }
        }
    );
    // An observation crosses the bridge with the declared fields only.
    advance(&p.b, 100);
    let (obs_id, om) = request(
        &mut p,
        PhysicalProductRequestV1::ToolCall {
            start: start.semantic_id.clone(),
            tool_session: tool_session.clone(),
            call: DecisionToolCallV1::Observe,
        },
    );
    let (_, work) = p.deliver_b(om).unwrap();
    let reply = PhysicalControlServiceV1::perform_physical_work(&p.b.core, work.unwrap())
        .await
        .unwrap()
        .reply
        .unwrap();
    p.deliver_a(reply);
    let ToolOutcomeV1::Reply {
        reply: DecisionToolReplyV1::Observation { view },
    } = read(&mut p, &obs_id)
    else {
        panic!("no observation relayed");
    };
    let view = serde_json::to_value(&view).unwrap();
    assert!(view.get("secret").is_none() && view["pose"].get("y").is_none());
    // A replayed call runs nothing again.
    let (replayed, work) = p.deliver_b(m).unwrap();
    assert!(work.is_none());
    assert!(matches!(
        replayed.unwrap().operation,
        PhysicalOperationV1::ToolResult {
            outcome: ToolOutcomeV1::Failed { .. },
            ..
        }
    ));
    // Losing the bridge closes the stream: uncertain, nothing continues.
    p.b.core
        .lock()
        .invalidate_physical_bridge("bridge")
        .unwrap();
    let status = p.query(&start.semantic_id);
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    let ts = p.b.core.lock().tool_session(&tool_session).unwrap();
    advance(&p.b, 200);
    assert!(!allowed(
        &PhysicalControlServiceV1::call_tool(&p.b.core, &ts, decide("forward", 100))
            .await
            .unwrap()
    ));
    p.assert_single(1);
}
