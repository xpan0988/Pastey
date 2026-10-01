//! Step C: decision streams through the executor-side tool dispatcher.
use super::*;
use crate::physical::core::{
    next_tick_deadline, DecisionToolCallV1, DecisionToolReplyV1, StreamTickV1, ToolSessionV1,
};
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
            disposition: crate::physical::store::ActionDispositionV1::Accepted
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
    // The brain commits with its first call; losing it then ends the stream.
    call(&f, &ts, DecisionToolCallV1::RemainingBudget).await;
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

fn open_another(
    f: &ControlFixture,
    described: &Arc<lane::DescribedLane>,
    name: &str,
) -> crate::error::AppResult<Arc<ToolSessionV1>> {
    let lane: Arc<dyn crate::physical::core::EnvironmentBinding> = described.clone();
    let s = ts_session(f);
    let mut core = f.core.lock();
    let i = core.local_ingress().unwrap();
    core.open_tool_session(&i, &s, lane, caller(name))
}
/// The stream as the executor holds it: authority, installation, records
/// and the idle-lease clock.
fn untouched(
    f: &ControlFixture,
    s: &Arc<BodyControlSessionV1>,
    ts: &Arc<ToolSessionV1>,
    idle_clock: u64,
) {
    let status = core_fake::store(&f.core.lock())
        .physical_status(&lane::session_audit(s).root)
        .unwrap();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Open);
    assert_eq!(f.session_state(), "active");
    assert_eq!(f.scalar("SELECT count(*) FROM physical_decisions"), 0);
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_sessions WHERE fence_ack IS NOT NULL"),
        0
    );
    assert_eq!(lane::last_activity(&lane::stream_of(ts)), idle_clock);
}

#[tokio::test]
async fn closing_an_unused_tool_session_releases_only_its_reservation() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, probe) = open(&f, &described).await;
    let idle_clock = lane::last_activity(&lane::stream_of(&probe));
    // Neither opening nor closing an unused session is brain activity.
    advance(&f, 100);
    assert_eq!(
        PhysicalControlServiceV1::close_tool_session(&f.core, &probe)
            .await
            .unwrap(),
        crate::physical::core::ToolCloseV1::Released
    );
    untouched(&f, &s, &probe, idle_clock);
    // The stream takes a new session; the released one stays closed and
    // cannot commit or disturb it.
    let brain = open_another(&f, &described, "brain.next").unwrap();
    assert!(matches!(
        call(&f, &probe, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Refused { reason } if reason.contains("closed")
    ));
    assert!(matches!(
        call(&f, &brain, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Observation { .. }
    ));
    assert!(lane::last_activity(&lane::stream_of(&brain)) > idle_clock);
}

#[tokio::test]
async fn a_stream_has_one_tool_session_and_the_first_call_commits_it() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, first) = open(&f, &described).await;
    // A second session is refused while the first is reserved; the refusal
    // leaves the first as it was.
    let second = open_another(&f, &described, "brain.second");
    assert!(second.is_err_and(|e| e.message().contains("already open")));
    assert!(allowed(&call(&f, &first, decide("forward", 300)).await));
    // Committed: no other brain may attach, now or after it is gone.
    let third = open_another(&f, &described, "brain.third");
    assert!(third.is_err_and(|e| e.message().contains("already drives")));
    assert_eq!(
        PhysicalControlServiceV1::close_tool_session(&f.core, &first)
            .await
            .unwrap(),
        crate::physical::core::ToolCloseV1::Ended
    );
    assert_eq!(f.session_state(), "quarantined");
    let status = core_fake::store(&f.core.lock())
        .physical_status(&lane::session_audit(&s).root)
        .unwrap();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert!(open_another(&f, &described, "brain.fourth").is_err());
}

#[tokio::test]
async fn an_unused_reservation_expires_without_touching_the_stream() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, stale) = open(&f, &described).await;
    let idle_clock = lane::last_activity(&lane::stream_of(&stale));
    // Never longer than half the stream's idle lease (350 ms here). The timer
    // is not run, so only the reservation can expire.
    advance(&f, 400);
    let next = open_another(&f, &described, "brain.next").unwrap();
    untouched(&f, &s, &next, idle_clock);
    // The expired session cannot commit; its replacement can.
    assert!(matches!(
        call(&f, &stale, DecisionToolCallV1::RemainingBudget).await,
        DecisionToolReplyV1::Refused { .. }
    ));
    assert!(matches!(
        call(&f, &next, DecisionToolCallV1::RemainingBudget).await,
        DecisionToolReplyV1::Budget { .. }
    ));
    // A reservation that expires before its first call is refused too.
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, late) = open(&f, &described).await;
    let idle_clock = lane::last_activity(&lane::stream_of(&late));
    advance(&f, 400);
    assert!(matches!(
        call(&f, &late, DecisionToolCallV1::Observe).await,
        DecisionToolReplyV1::Refused { reason } if reason.contains("expired")
    ));
    untouched(&f, &s, &late, idle_clock);
    open_another(&f, &described, "brain.next").unwrap();
}

/// A timer on the fixture clock: sleeping moves simulated time to the
/// deadline at once.
struct FixtureTimer(Arc<ControlFixture>);
impl crate::physical::core::TickTimerV1 for FixtureTimer {
    fn now_us(&self) -> u64 {
        self.0.clock.ticks.load(Ordering::SeqCst)
    }
    fn sleep_until(
        &self,
        deadline_us: u64,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let now = self.now_us();
        if deadline_us > now {
            advance(&self.0, (deadline_us - now) / 1000);
        }
        Box::pin(std::future::ready(()))
    }
}
/// Supervises a fresh stream whose every sample takes `work_us` of simulated
/// time, until the stream ends. Returns the capture times of the
/// supervisor's samples and the observation gap limit.
async fn supervised_captures(work_us: u64) -> (Arc<ControlFixture>, Vec<u64>, u64) {
    let f = Arc::new(ControlFixture::stream(1_200_000, 6));
    let b = binding();
    let mut enrollment = fake::enrollment(&b);
    let registration = fake::record(&mut enrollment).clone();
    let fingerprint = b.implementation_fingerprint.clone();
    let captures = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let (clock, seen) = (f.clock.clone(), captures.clone());
    let described = Arc::new(lane::DescribedLane::new(
        move || crate::physical::binding::BindingDescriptionV1 {
            registration: registration.clone(),
            provenance_digest: digest_value(),
            conditions_digest: digest_value(),
            implementation_fingerprint: fingerprint.clone(),
        },
        // The sample is captured once its work is done.
        move |_| {
            let wall = clock.wall.load(Ordering::SeqCst) + work_us / 1000;
            let ticks = clock.ticks.load(Ordering::SeqCst) + work_us;
            clock.set(wall, ticks);
            seen.lock().push(ticks);
            ticks
        },
        0,
    ));
    let (_, ts) = open(&f, &described).await;
    captures.lock().clear();
    let max_gap = f.scope.fields().freshness.observation.max_gap_us.get();
    let stream = lane::stream_of(&ts);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        PhysicalControlServiceV1::supervise_stream_on(&f.core, stream, &FixtureTimer(f.clone())),
    )
    .await
    .expect("supervisor did not stop");
    let captured = captures.lock().clone();
    (f, captured, max_gap)
}
fn gaps(captures: &[u64]) -> Vec<u64> {
    captures.windows(2).map(|w| w[1] - w[0]).collect()
}

#[tokio::test]
async fn ticks_run_on_deadlines_so_tick_work_does_not_widen_the_sample_gap() {
    // Work takes three quarters of the period (150 of 200 ms on a 400 ms gap
    // limit, 75 of 100 ms here): samples stay one period apart, not
    // period + work.
    let period = ControlFixture::stream(1_200_000, 6)
        .scope
        .fields()
        .freshness
        .observation
        .max_gap_us
        .get()
        / 2;
    let (f, captures, _) = supervised_captures(period * 3 / 4).await;
    assert!(captures.len() >= 3, "{captures:?}");
    assert!(gaps(&captures).iter().all(|g| *g == period), "{captures:?}");
    // The stream ran until its idle lease (no brain ever called), not a gap.
    assert_eq!(f.session_state(), "quarantined");
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_attempts WHERE close_reason='revoked'"),
        1
    );
}

#[tokio::test]
async fn an_overrun_runs_the_next_tick_at_once_without_a_catch_up_burst() {
    // Work longer than the period but inside the gap limit: each tick
    // starts as soon as the last one finished, and no ticks are replayed.
    let max_gap = ControlFixture::stream(1_200_000, 6)
        .scope
        .fields()
        .freshness
        .observation
        .max_gap_us
        .get();
    let work = max_gap * 3 / 4;
    let (_, captures, _) = supervised_captures(work).await;
    assert!(captures.len() >= 2, "{captures:?}");
    assert!(gaps(&captures).iter().all(|g| *g == work), "{captures:?}");
    assert_eq!(next_tick_deadline(0, 200, 1_000), 1_000);
    assert_eq!(next_tick_deadline(1_000, 200, 1_050), 1_200);
}

#[tokio::test]
async fn tick_work_beyond_the_gap_limit_still_fails_closed() {
    let max_gap = ControlFixture::stream(1_200_000, 6)
        .scope
        .fields()
        .freshness
        .observation
        .max_gap_us
        .get();
    let (f, captures, _) = supervised_captures(max_gap * 3 / 2).await;
    // The second sample comes too late: the stream ends at once.
    assert_eq!(captures.len(), 2, "{captures:?}");
    assert_eq!(f.session_state(), "quarantined");
    let root = lane::session_audit(&ts_session(&f)).root;
    let status = core_fake::store(&f.core.lock())
        .physical_status(&root)
        .unwrap();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
}

/// Records one qualified observation of the latest action's lineage with
/// the given measured progress.
fn observe_progress(f: &ControlFixture, progress: f64) {
    let mut core = f.core.lock();
    let store = core_fake::store(&core);
    let root = f
        .sql()
        .query_row("SELECT root_id FROM physical_attempts", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    let action = store
        .latest_dispatched_action(&RootId::try_from(root).unwrap())
        .unwrap()
        .unwrap();
    let lineage = store.evidence_lineage(&action).unwrap();
    let qualification = f.scope.fields().qualification.qualification_id.clone();
    let seq =
        f.scalar("SELECT count(*) FROM physical_evidence WHERE kind='observation'") as u64 + 1;
    let ticks = f.clock.ticks.load(Ordering::SeqCst);
    let fact = PhysicalObservationV1 {
        lineage,
        id: ObservationId::try_from(format!("physical-observation:v1:{}", uuid::Uuid::new_v4()))
            .unwrap(),
        sequence: seq,
        capture_us: 1_000_000 + ticks,
        gap_us: 0,
        measurements: fx::MeasurementV1 {
            frame: evidence::label("world"),
            progress: Some(Finite::try_from(progress).unwrap()),
            drift: None,
            rate: None,
            spin: None,
            uncertainty: Some(NonNegative::try_from(0.0001).unwrap()),
            intact: Some(true),
        }
        .encode()
        .unwrap(),
    };
    let i = core.local_ingress().unwrap();
    core.record_physical_observation(&i, producer::qualified_observation(fact, qualification))
        .unwrap();
}

/// Appends one qualified observation of `action` with sequence `seq`
/// through Core, and returns how long the append took.
fn append_observation(f: &ControlFixture, action: &ActionId, seq: u64) -> std::time::Duration {
    advance(f, 1);
    let mut core = f.core.lock();
    let lineage = core_fake::store(&core).evidence_lineage(action).unwrap();
    let qualification = f.scope.fields().qualification.qualification_id.clone();
    let ticks = f.clock.ticks.load(Ordering::SeqCst);
    let fact = PhysicalObservationV1 {
        lineage,
        id: ObservationId::try_from(format!("physical-observation:v1:{}", uuid::Uuid::new_v4()))
            .unwrap(),
        sequence: seq,
        capture_us: 1_000_000 + ticks,
        gap_us: 0,
        measurements: fx::MeasurementV1 {
            frame: evidence::label("world"),
            progress: Some(Finite::try_from(0.1).unwrap()),
            drift: None,
            rate: None,
            spin: None,
            uncertainty: Some(NonNegative::try_from(0.0001).unwrap()),
            intact: Some(true),
        }
        .encode()
        .unwrap(),
    };
    let i = core.local_ingress().unwrap();
    let proof = producer::qualified_observation(fact, qualification);
    let started = std::time::Instant::now();
    core.record_physical_observation(&i, proof).unwrap();
    started.elapsed()
}
fn dispatched_action(f: &ControlFixture) -> ActionId {
    let root: String = f
        .sql()
        .query_row("SELECT root_id FROM physical_attempts", [], |r| r.get(0))
        .unwrap();
    core_fake::store(&f.core.lock())
        .latest_dispatched_action(&RootId::try_from(root).unwrap())
        .unwrap()
        .unwrap()
}
fn median(mut d: Vec<std::time::Duration>) -> std::time::Duration {
    d.sort();
    d[d.len() / 2]
}

#[tokio::test]
async fn appended_evidence_passes_the_full_audit_and_a_forged_append_is_refused() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
    let action = dispatched_action(&f);
    for seq in 1..=40 {
        append_observation(&f, &action, seq);
    }
    // Rows accepted one at a time at the head pass the full audit, which
    // checks every row against every predecessor (forced by another
    // connection's commit).
    f.sql()
        .execute_batch("CREATE TABLE unrelated(x); INSERT INTO unrelated VALUES(1);")
        .unwrap();
    let root = lane::session_audit(&s).root;
    core_fake::store(&f.core.lock())
        .physical_status(&root)
        .unwrap();
    // An append whose columns disagree with its body is refused at commit,
    // as the group audit would refuse it, and nothing is written.
    let before = f.scalar("SELECT count(*) FROM physical_evidence");
    let core = f.core.lock();
    let store = core_fake::store(&core);
    let mut c = store.connection().unwrap();
    let tx = c.transaction().unwrap();
    tx.execute(
        "INSERT INTO physical_evidence SELECT 'forged-'||id,action_id,kind,sequence+1000,revision+1,capture_us,receipt_us,source_digest,ordered,qualified,digest,record_json FROM physical_evidence ORDER BY revision DESC LIMIT 1",
        [],
    )
    .unwrap();
    assert!(store.commit(tx).is_err());
    drop(c);
    drop(core);
    assert_eq!(f.scalar("SELECT count(*) FROM physical_evidence"), before);
}

#[tokio::test]
async fn appending_evidence_costs_the_same_with_a_long_history() {
    // Before, each append re-audited every evidence and consequence row of
    // its Root. Now its cost does not grow with the history before it.
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (_, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
    let action = dispatched_action(&f);
    let mut early = Vec::new();
    let mut late = Vec::new();
    for seq in 1..=2_000 {
        let took = append_observation(&f, &action, seq);
        if (81..=100).contains(&seq) {
            early.push(took);
        }
        if seq > 1_980 {
            late.push(took);
        }
    }
    let (early, late) = (median(early), median(late));
    assert!(
        late < early * 3,
        "append cost grew from {early:?} at ~100 rows to {late:?} at ~2000"
    );
}

#[tokio::test]
async fn a_witnessed_effect_bound_violation_ends_the_stream_and_is_recorded() {
    let f = ControlFixture::stream(1_200_000, 6);
    let described = lane_for(&f);
    let (s, ts) = open(&f, &described).await;
    assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
    // Inside the bound (progress 0.1 of at most 0.2): the stream goes on.
    observe_progress(&f, 0.1);
    assert!(matches!(
        tick_for(&f, &ts, 100).await,
        StreamTickV1::Continue(_)
    ));
    // Outside it: the witness contradicts the bound and Core ends the stream.
    observe_progress(&f, 0.5);
    assert_eq!(tick_for(&f, &ts, 100).await, StreamTickV1::Ended);
    assert_eq!(
        f.scalar(
            "SELECT count(*) FROM physical_attempts WHERE close_reason='effect_bound_violated'"
        ),
        1
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_effect_bound_violations"),
        1
    );
    assert_eq!(f.session_state(), "quarantined");
    let status = core_fake::store(&f.core.lock())
        .physical_status(&lane::session_audit(&s).root)
        .unwrap();
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert!(!allowed(&call(&f, &ts, decide("stop", 100)).await));
}

#[tokio::test]
async fn a_bound_this_host_cannot_witness_must_be_marked_intent_only() {
    let f = ControlFixture::stream(1_200_000, 6);
    // This Host has no witness for the extent bound.
    let mut core = f.core.lock();
    core_fake::set_witnesses(
        &mut core,
        crate::physical::evidence::WitnessRegistryV1::default().with(
            fx::id(fx::COMPLETION_PREDICATE),
            witnesses()
                .get(&fx::id(fx::COMPLETION_PREDICATE))
                .unwrap()
                .clone(),
        ),
    );
    let i = core.local_ingress().unwrap();
    assert!(core.draft_review(&i, &f.live, f.scope.clone()).is_err());
    let mut fields = f.scope.fields().clone();
    fields.stream.effect_bound = EffectBoundV1::IntentOnly;
    core.draft_review(
        &i,
        &f.live,
        PhysicalReviewScopeV1::try_from(fields).unwrap(),
    )
    .unwrap();
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
                disposition: crate::physical::store::ActionDispositionV1::Accepted
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

#[tokio::test]
async fn remote_tool_sessions_release_refuse_stale_ids_and_stay_single() {
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
    let start = start.semantic_id.clone();
    // One request over the bridge, run as the executor would, read back.
    async fn relay(p: &mut Pair, r: PhysicalProductRequestV1) -> ToolOutcomeV1 {
        let (view, m) = p.a.physical_product(&p.ab, r).unwrap();
        let id = view.tool_request.unwrap();
        let (reply, work) = p.deliver_b(m.unwrap()).unwrap();
        let reply = match work {
            Some(work) => {
                PhysicalControlServiceV1::perform_physical_work(&p.b.core, work)
                    .await
                    .unwrap()
                    .reply
            }
            None => reply,
        };
        p.deliver_a(reply.unwrap());
        p.a.physical_product(&p.ab, PhysicalProductRequestV1::ToolResult { request: id })
            .unwrap()
            .0
            .tool
            .unwrap()
    }
    let open = |name: &str| PhysicalProductRequestV1::ToolOpen {
        start: start.clone(),
        caller: caller(name),
    };
    let ToolOutcomeV1::Opened {
        tool_session: probe,
        ..
    } = relay(&mut p, open("brain.probe")).await
    else {
        panic!("not opened");
    };
    // A second open while one is reserved is refused.
    assert!(matches!(
        relay(&mut p, open("brain.second")).await,
        ToolOutcomeV1::Failed { reason } if reason.contains("already open")
    ));
    // Closing the unused session releases it; the stream is unaffected.
    assert_eq!(
        relay(
            &mut p,
            PhysicalProductRequestV1::ToolClose {
                start: start.clone(),
                tool_session: probe.clone()
            }
        )
        .await,
        ToolOutcomeV1::Released
    );
    let ToolOutcomeV1::Opened {
        tool_session: brain,
        ..
    } = relay(&mut p, open("brain.real")).await
    else {
        panic!("not reopened");
    };
    // A call on the released session (not the executor's current one) is
    // refused, and the current session is not disturbed.
    assert!(matches!(
        relay(
            &mut p,
            PhysicalProductRequestV1::ToolCall {
                start: start.clone(),
                tool_session: probe.clone(),
                call: decide("forward", 300)
            }
        )
        .await,
        ToolOutcomeV1::Failed { .. }
    ));
    assert!(matches!(
        relay(
            &mut p,
            PhysicalProductRequestV1::ToolCall {
                start: start.clone(),
                tool_session: brain.clone(),
                call: decide("forward", 300)
            }
        )
        .await,
        ToolOutcomeV1::Reply { reply } if allowed(&reply)
    ));
    // After the commit, closing ends the stream and no brain attaches again.
    assert_eq!(
        relay(
            &mut p,
            PhysicalProductRequestV1::ToolClose {
                start: start.clone(),
                tool_session: brain
            }
        )
        .await,
        ToolOutcomeV1::Closed
    );
    let status = p.query(&start);
    assert_eq!(status.authority, PhysicalAuthorityStateV1::Closed);
    assert_eq!(status.consequence, ConsequenceStateV1::OutcomeUnknown);
    assert!(matches!(
        relay(&mut p, open("brain.late")).await,
        ToolOutcomeV1::Failed { .. }
    ));
    p.assert_single(1);
}

#[path = "physical_demo_tests.rs"]
mod physical_demo;

/// Adversarial appends: every forged evidence or consequence row is offered
/// twice, once alone (validated by the append path) and once with another
/// physical-table write in the same transaction (validated by the group
/// audit, as before the append path existed). Both must reach the same
/// result.
mod append_path {
    use super::*;
    use crate::physical::evidence::{
        ConsequenceStateV1, ObservationRecordV1, PhysicalConsequenceV1,
    };

    /// One `physical_evidence` row, as columns plus its decoded body.
    #[derive(Clone)]
    struct EvidenceRow {
        id: String,
        action: String,
        kind: String,
        sequence: i64,
        revision: i64,
        capture: i64,
        receipt: i64,
        source: String,
        ordered: bool,
        qualified: bool,
        digest: Option<String>,
        body: ObservationRecordV1,
    }
    fn last_observation(f: &ControlFixture) -> EvidenceRow {
        f.sql()
            .query_row(
                "SELECT id,action_id,kind,sequence,revision,capture_us,receipt_us,source_digest,ordered,qualified,digest,record_json FROM physical_evidence WHERE kind='observation' ORDER BY revision DESC LIMIT 1",
                [],
                |r| {
                    Ok(EvidenceRow {
                        id: r.get(0)?,
                        action: r.get(1)?,
                        kind: r.get(2)?,
                        sequence: r.get(3)?,
                        revision: r.get(4)?,
                        capture: r.get(5)?,
                        receipt: r.get(6)?,
                        source: r.get(7)?,
                        ordered: r.get(8)?,
                        qualified: r.get(9)?,
                        digest: Some(r.get(10)?),
                        body: serde_json::from_str(&r.get::<_, String>(11)?).unwrap(),
                    })
                },
            )
            .unwrap()
    }
    /// A correct next observation: new identity, the next sequence and
    /// revision, a later capture, its digest recomputed.
    fn next_valid(f: &ControlFixture) -> EvidenceRow {
        let mut row = last_observation(f);
        let head: i64 = f
            .sql()
            .query_row(
                "SELECT max(revision) FROM physical_evidence WHERE action_id=?1",
                [&row.action],
                |r| r.get(0),
            )
            .unwrap();
        let id =
            ObservationId::try_from(format!("physical-observation:v1:{}", uuid::Uuid::new_v4()))
                .unwrap();
        let (seq, capture) = (row.sequence as u64 + 1, row.capture as u64 + 1_000);
        row.body.fact.id = id.clone();
        row.body.fact.sequence = seq;
        row.body.fact.capture_us = capture;
        row.body.receipt_us = capture;
        if let Some(p) = row.body.producer.as_mut() {
            p.local_sequence = seq;
            p.receipt_us = capture;
        }
        row.body.ordered = true;
        row.id = String::from(id);
        row.sequence = seq as i64;
        row.revision = head + 1;
        row.capture = capture as i64;
        row.receipt = capture as i64;
        row.ordered = true;
        row.digest = None;
        row
    }
    fn insert(tx: &rusqlite::Transaction<'_>, row: &EvidenceRow) -> rusqlite::Result<usize> {
        let digest = row.digest.clone().unwrap_or_else(|| {
            String::from(
                crate::physical::values::digest("pastey-physical-evidence-record-v1", &row.body)
                    .unwrap(),
            )
        });
        tx.execute(
            "INSERT INTO physical_evidence VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                row.id,
                row.action,
                row.kind,
                row.sequence,
                row.revision,
                row.capture,
                row.receipt,
                row.source,
                row.ordered,
                row.qualified,
                digest,
                serde_json::to_string(&row.body).unwrap()
            ],
        )
    }
    /// Offers one transaction: `write` plus, if `mixed`, a write to another
    /// physical table of the same Root. The ledger's one connection and its
    /// commit validation are used, as Core's own writes are.
    fn offer(
        f: &ControlFixture,
        mixed: bool,
        write: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<usize>,
    ) -> Result<(), String> {
        let core = f.core.lock();
        let store = core_fake::store(&core);
        let mut c = store.connection().unwrap();
        let tx = c.transaction().unwrap();
        write(&tx).map_err(|e| format!("insert refused: {e}"))?;
        if mixed {
            tx.execute(
                "UPDATE physical_control_budgets SET revision=revision+1",
                [],
            )
            .map_err(|e| format!("mixed write refused: {e}"))?;
        }
        store.commit(tx).map_err(|e| e.message().to_owned())
    }
    /// Both validations of the same forged evidence row, which must agree.
    fn both(f: &ControlFixture, row: &EvidenceRow) -> (Result<(), String>, Result<(), String>) {
        let alone = offer(f, false, |tx| insert(tx, row));
        let mixed = offer(f, true, |tx| insert(tx, row));
        (alone, mixed)
    }
    fn count(f: &ControlFixture) -> i64 {
        f.scalar("SELECT count(*) FROM physical_evidence")
    }
    async fn fixture() -> ControlFixture {
        let f = ControlFixture::stream(1_200_000, 6);
        let described = lane_for(&f);
        let (_, ts) = open(&f, &described).await;
        assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
        let action = dispatched_action(&f);
        for seq in 1..=5 {
            append_observation(&f, &action, seq);
        }
        f
    }

    #[tokio::test]
    async fn a_valid_append_is_accepted_alone_and_mixed() {
        let f = fixture().await;
        let before = count(&f);
        assert_eq!(offer(&f, false, |tx| insert(tx, &next_valid(&f))), Ok(()));
        assert_eq!(offer(&f, true, |tx| insert(tx, &next_valid(&f))), Ok(()));
        assert_eq!(count(&f), before + 2);
    }

    #[tokio::test]
    async fn an_out_of_order_append_is_refused_alike() {
        let f = fixture().await;
        let before = count(&f);
        let last = last_observation(&f);
        // Claims to be ordered, but captured no later than its predecessor.
        let mut row = next_valid(&f);
        row.capture = last.capture;
        row.receipt = last.capture;
        row.body.fact.capture_us = last.capture as u64;
        row.body.receipt_us = last.capture as u64;
        if let Some(p) = row.body.producer.as_mut() {
            p.receipt_us = last.capture as u64;
        }
        let (alone, mixed) = both(&f, &row);
        assert!(alone.is_err(), "{alone:?}");
        assert_eq!(alone, mixed);
        // Later in sequence and capture, yet claims not to be ordered.
        let mut row = next_valid(&f);
        row.ordered = false;
        row.body.ordered = false;
        let (alone, mixed) = both(&f, &row);
        assert!(alone.is_err(), "{alone:?}");
        assert_eq!(alone, mixed);
        assert_eq!(count(&f), before);
    }

    #[tokio::test]
    async fn a_duplicate_or_skipped_revision_or_sequence_is_refused_alike() {
        let f = fixture().await;
        let before = count(&f);
        let mut duplicate_revision = next_valid(&f);
        duplicate_revision.revision -= 1;
        let mut duplicate_sequence = next_valid(&f);
        duplicate_sequence.sequence -= 1;
        duplicate_sequence.body.fact.sequence -= 1;
        if let Some(p) = duplicate_sequence.body.producer.as_mut() {
            p.local_sequence -= 1;
        }
        let mut skipped = next_valid(&f);
        skipped.revision += 1;
        for row in [&duplicate_revision, &duplicate_sequence, &skipped] {
            let (alone, mixed) = both(&f, row);
            assert!(alone.is_err(), "{alone:?}");
            assert_eq!(alone, mixed);
        }
        assert_eq!(count(&f), before);
    }

    #[tokio::test]
    async fn a_forged_body_or_column_is_refused_alike() {
        let f = fixture().await;
        let before = count(&f);
        // A column that disagrees with the body.
        let mut column = next_valid(&f);
        column.capture += 1;
        // A body changed after its digest was taken.
        let mut stale_digest = next_valid(&f);
        let digest = crate::physical::values::digest(
            "pastey-physical-evidence-record-v1",
            &stale_digest.body,
        )
        .unwrap();
        stale_digest.digest = Some(String::from(digest));
        stale_digest.body.fact.gap_us += 1;
        // An unqualified body claiming to be qualified.
        let mut qualified = next_valid(&f);
        qualified.body.qualified = false;
        for row in [&column, &stale_digest, &qualified] {
            let (alone, mixed) = both(&f, row);
            assert!(alone.is_err(), "{alone:?}");
            assert_eq!(alone, mixed);
        }
        assert_eq!(count(&f), before);
    }

    #[tokio::test]
    async fn a_wrong_lineage_or_source_is_refused_alike() {
        let f = fixture().await;
        let before = count(&f);
        let mut lineage = next_valid(&f);
        lineage.body.fact.lineage.session =
            SessionId::try_from(format!("physical-session:v1:{}", uuid::Uuid::new_v4())).unwrap();
        let mut source = next_valid(&f);
        source.source = "0".repeat(64);
        let mut action = next_valid(&f);
        action.body.fact.lineage.action =
            ActionId::try_from(format!("physical-action:v1:{}", uuid::Uuid::new_v4())).unwrap();
        for row in [&lineage, &source, &action] {
            let (alone, mixed) = both(&f, row);
            assert!(alone.is_err(), "{alone:?}");
            assert_eq!(alone, mixed);
        }
        assert_eq!(count(&f), before);
    }

    #[tokio::test]
    async fn a_forged_consequence_is_refused_alike() {
        let f = fixture().await;
        let action = dispatched_action(&f);
        {
            let mut core = f.core.lock();
            let i = core.local_ingress().unwrap();
            core.evaluate_physical_consequence(&i, &action).unwrap();
        }
        let (rev, raw): (i64, String) = f
            .sql()
            .query_row(
                "SELECT revision,record_json FROM physical_consequences ORDER BY revision DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let mut x: PhysicalConsequenceV1 = serde_json::from_str(&raw).unwrap();
        assert_ne!(x.state, ConsequenceStateV1::Verified);
        x.revision = rev as u64 + 1;
        x.state = ConsequenceStateV1::Verified;
        let digest = String::from(
            crate::physical::values::digest("pastey-physical-consequence-v1", &x).unwrap(),
        );
        let insert = |tx: &rusqlite::Transaction<'_>| {
            tx.execute(
                "INSERT INTO physical_consequences VALUES(?1,?2,?3,?4,?5,?6,?7)",
                rusqlite::params![
                    String::from(x.action.clone()),
                    x.revision as i64,
                    x.evidence_revision as i64,
                    "verified",
                    String::from(x.completion_digest.clone()),
                    digest,
                    serde_json::to_string(&x).unwrap()
                ],
            )
        };
        let alone = offer(&f, false, insert);
        let mixed = offer(&f, true, insert);
        assert!(alone.is_err(), "{alone:?}");
        assert_eq!(alone, mixed);
        assert_eq!(
            f.scalar("SELECT count(*) FROM physical_consequences WHERE state='verified'"),
            0
        );
    }

    /// Neither validation has a rule about when an action's evidence ends:
    /// a valid append after the Root has closed and the stream has fenced is
    /// accepted by both alike. (An evidence window would be a new rule.)
    #[tokio::test]
    async fn an_append_after_the_stream_ended_is_judged_alike() {
        let f = ControlFixture::stream(1_200_000, 6);
        let described = lane_for(&f);
        let (s, ts) = open(&f, &described).await;
        assert!(allowed(&call(&f, &ts, decide("forward", 300)).await));
        let action = dispatched_action(&f);
        for seq in 1..=3 {
            append_observation(&f, &action, seq);
        }
        PhysicalControlServiceV1::close_tool_session(&f.core, &ts)
            .await
            .unwrap();
        assert_eq!(f.session_state(), "quarantined");
        let root = lane::session_audit(&s).root;
        assert_eq!(
            core_fake::store(&f.core.lock())
                .physical_status(&root)
                .unwrap()
                .authority,
            PhysicalAuthorityStateV1::Closed
        );
        let before = count(&f);
        assert_eq!(offer(&f, false, |tx| insert(tx, &next_valid(&f))), Ok(()));
        assert_eq!(offer(&f, true, |tx| insert(tx, &next_valid(&f))), Ok(()));
        assert_eq!(count(&f), before + 2);
    }

    /// Another connection's commit makes the next transaction audit the
    /// whole ledger before it appends. Appends through Core then still pass,
    /// a forged one is still refused alike, and the whole ledger still audits.
    #[tokio::test]
    async fn a_foreign_commit_forces_the_full_audit_and_the_result_is_unchanged() {
        let f = fixture().await;
        let action = dispatched_action(&f);
        f.sql()
            .execute_batch("CREATE TABLE unrelated(x); INSERT INTO unrelated VALUES(1);")
            .unwrap();
        append_observation(&f, &action, 6);
        f.sql()
            .execute("INSERT INTO unrelated VALUES(2)", [])
            .unwrap();
        let mut forged = next_valid(&f);
        forged.capture += 1;
        let (alone, mixed) = both(&f, &forged);
        assert!(alone.is_err(), "{alone:?}");
        assert_eq!(alone, mixed);
        f.sql()
            .execute("INSERT INTO unrelated VALUES(3)", [])
            .unwrap();
        let root: String = f
            .sql()
            .query_row("SELECT root_id FROM physical_attempts", [], |r| r.get(0))
            .unwrap();
        core_fake::store(&f.core.lock())
            .physical_status(&RootId::try_from(root).unwrap())
            .unwrap();
    }
}
