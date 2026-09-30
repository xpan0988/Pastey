//! Physical demo acceptance: "walk from the living room to the bedroom".
//!
//! Specification: tests/physical_demo/README.md. One human approval authorizes
//! a finite decision stream; a brain (any tool caller) decides continuously;
//! the body must end up in the bedroom as judged by the binding's witness.
//! Pastey defines no brain loop: the brains below are test code that only
//! calls the tool surface Core exposes for the approved decision stream.
//!
//! Ignored until Step C (DecisionStream) and Step D (reference binding) land.
//! Run with `cargo test physical_demo -- --ignored`. Each test fails at the
//! first missing capability with `physical demo: missing capability ...`.
//! Implementing a capability replaces its `missing` stub below; the test
//! bodies and assertions are the acceptance criteria and must not weaken.
use super::descriptor::InvocationModeV1;
use serde_json::{json, Value};

/// Panics with the capability the demo needs next and the step that adds it.
fn missing<T>(capability: &str, step: &str) -> T {
    panic!("physical demo: missing capability `{capability}` (Step {step})")
}

/// Real probe: a binding can declare a decision-stream capability.
fn decision_stream_mode() -> InvocationModeV1 {
    serde_json::from_value(json!("decision_stream"))
        .unwrap_or_else(|_| missing("InvocationModeV1::DecisionStream", "C"))
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
#[expect(dead_code, reason = "constructed by the Step C tool surface")]
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
#[expect(dead_code, reason = "read by the Step C tool surface")]
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

/// One running demo: the reference binding's simulated flat (living room,
/// door, bedroom, walls), a Host Core with that binding attached, and one
/// approved decision stream for the walk.
struct DemoV1;
impl DemoV1 {
    /// Body starts in the living room; one approval is granted.
    fn living_room_to_bedroom() -> Self {
        let _ = decision_stream_mode();
        missing(
            "2D kinematic reference binding (two rooms, a door, walls; SimulationOracle witness; local timeout self-stop)",
            "D",
        )
    }
    /// The walk and a dispenser, each with its own binding and approval,
    /// attached to one Core instance.
    fn two_bodies_one_core() -> (Self, Self) {
        let _ = decision_stream_mode();
        missing(
            "second binding: a dispenser (own options, payload schema and observation format)",
            "D",
        )
    }
    /// Identity of the Core instance serving this demo.
    fn core_identity(&self) -> String {
        missing("Core instance identity for attached bindings", "C")
    }
    fn approval_count(&self) -> u64 {
        missing("decision-stream approval ledger query", "C")
    }
    fn approval_digest(&self) -> String {
        missing("decision-stream approval scope digest", "C")
    }
    /// Tool names exposed to a caller: the approved options plus the
    /// observation and remaining-budget queries.
    fn tool_names(&self, _caller: &str) -> Vec<String> {
        missing("decision-stream tool surface", "C")
    }
    fn observe(&self, _caller: &str) -> Value {
        missing("read-only observation query tool", "C")
    }
    fn remaining(&self, _caller: &str) -> RemainingBudgetV1 {
        missing("remaining-budget query tool", "C")
    }
    fn call(&self, _caller: &str, _call: &DecisionCallV1) -> ToolResultV1 {
        missing(
            "decision tool call through executor-side Core admission",
            "C",
        )
    }
    /// Lets simulated time pass without any tool call.
    fn wait_ms(&self, _ms: u64) {
        missing("reference binding simulated clock", "D")
    }
    fn revoke(&self) {
        missing("decision-stream revocation", "C")
    }
    fn disconnect_bridge(&self) {
        missing("decision stream behind the bridge route", "C")
    }
    /// The caller's tool session ends (the brain process died mid-action).
    fn brain_crashed(&self, _caller: &str) {
        missing("tool-session loss closes the decision stream", "C")
    }
    /// A fresh route/brain after a loss; must not resume anything.
    fn recover(&self) {
        missing("decision-stream recovery after loss", "C")
    }
    fn consequence(&self) -> ConsequenceV1 {
        missing("witness verdict for the stream's completion contract", "D")
    }
    fn records(&self) -> Vec<StepRecordV1> {
        missing("per-step proposer/admission/body records", "C")
    }
    fn truth(&self) -> BodyTruthV1 {
        missing("reference binding simulator ground truth", "D")
    }
    fn consumed(&self) -> (u64, u64) {
        missing("cumulative decision-stream budget rows", "C")
    }
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

#[tokio::test]
#[ignore = "physical demo: needs DecisionStream (Step C) and the reference binding (Step D)"]
async fn physical_demo_1_brains_are_replaceable_under_one_approval() {
    let brains: Vec<Box<dyn BrainV1>> = vec![Box::new(RuleBrainV1), Box::new(MockLlmBrainV1)];
    let mut digests = Vec::new();
    for mut brain in brains {
        let demo = DemoV1::living_room_to_bedroom();
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

#[tokio::test]
#[ignore = "physical demo: needs DecisionStream (Step C) and the reference binding (Step D)"]
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

#[tokio::test]
#[ignore = "physical demo: needs DecisionStream (Step C) and the reference binding (Step D)"]
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

#[tokio::test]
#[ignore = "physical demo: needs DecisionStream (Step C) and the reference binding (Step D)"]
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

#[tokio::test]
#[ignore = "physical demo: needs DecisionStream (Step C) and the reference bindings (Step D)"]
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
