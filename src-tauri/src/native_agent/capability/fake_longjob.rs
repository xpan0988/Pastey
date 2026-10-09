//! Test-only `fake.longjob.v1`: a mature Host capability whose HOW Pastey
//! does not know. It runs its own job loop with its own progress, may fail
//! at a step, honours cancellation, and reports a terminal output. Pastey
//! only sees its opaque input and output.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    ExclusivityKeyV1, NativeCapabilityAdapterV1, NativeInvocationOutcomeV1,
    NativeInvocationProgressV1, NativeInvocationRunV1, OpaqueCapabilityPayloadV1,
    PreparedInvocationV1, StartedInvocationV1,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::{
    NativeAgentCapabilityStateV1, NativeTurnOutcomeV1, GENERIC_NATIVE_INVOKE_PROTOCOLS,
};

pub(in crate::native_agent) const FAKE_LONGJOB_ID: &str = "fake.longjob.v1";
const MAX_STEPS: u32 = 1_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LongJobInputV1 {
    steps: u32,
    fail_at: Option<u32>,
    payload: Value,
    /// Optional resource the job holds exclusively while unresolved.
    lane: Option<String>,
}

fn parse(input: &OpaqueCapabilityPayloadV1) -> AppResult<LongJobInputV1> {
    let parsed: LongJobInputV1 = serde_json::from_value(input.value().clone())
        .map_err(|_| AppError::InvalidInput("fake.longjob input is invalid.".into()))?;
    if parsed.steps == 0 || parsed.steps > MAX_STEPS {
        return Err(AppError::InvalidInput(
            "fake.longjob steps are out of range.".into(),
        ));
    }
    Ok(parsed)
}

struct JobV1 {
    payload: Value,
    progress: u32,
    cancel: bool,
}

#[derive(Default)]
struct FakeStateV1 {
    jobs: HashMap<String, JobV1>,
    prepares: usize,
    starts: usize,
    hold: bool,
    complete_despite_cancel: bool,
    lose_observation: bool,
    observation_lost: bool,
    shutdowns: usize,
}

#[derive(Default)]
pub(in crate::native_agent) struct FakeLongJobAdapterV1 {
    state: Arc<(Mutex<FakeStateV1>, Condvar)>,
}

impl FakeLongJobAdapterV1 {
    fn with<T>(&self, change: impl FnOnce(&mut FakeStateV1) -> T) -> T {
        let (lock, wake) = &*self.state;
        let result = change(&mut lock.lock().unwrap());
        wake.notify_all();
        result
    }

    /// Jobs pause before their next step until `release`.
    pub(in crate::native_agent) fn hold(&self) {
        self.with(|state| state.hold = true);
    }

    pub(in crate::native_agent) fn release(&self) {
        self.with(|state| state.hold = false);
    }

    /// The native job finishes even after it was asked to stop.
    pub(in crate::native_agent) fn complete_despite_cancel(&self) {
        self.with(|state| state.complete_despite_cancel = true);
    }

    /// Outcomes are unobservable until `drop_observation`.
    pub(in crate::native_agent) fn lose_observation(&self) {
        self.with(|state| state.lose_observation = true);
    }

    pub(in crate::native_agent) fn drop_observation(&self) {
        self.with(|state| state.observation_lost = true);
    }

    /// How many inputs the capability was asked to prepare.
    pub(in crate::native_agent) fn prepares(&self) -> usize {
        self.with(|state| state.prepares)
    }

    /// How many invocations the capability itself accepted.
    pub(in crate::native_agent) fn starts(&self) -> usize {
        self.with(|state| state.starts)
    }

    /// The payload exactly as the capability received it.
    pub(in crate::native_agent) fn received_payload(&self, task_id: &str) -> Option<Value> {
        self.with(|state| state.jobs.get(task_id).map(|job| job.payload.clone()))
    }

    pub(in crate::native_agent) fn progress(&self, task_id: &str) -> Option<u32> {
        self.with(|state| state.jobs.get(task_id).map(|job| job.progress))
    }

    pub(in crate::native_agent) fn shutdowns(&self) -> usize {
        self.with(|state| state.shutdowns)
    }

    pub(in crate::native_agent) fn cancel_requested(&self, task_id: &str) -> bool {
        self.with(|state| state.jobs.get(task_id).is_some_and(|job| job.cancel))
    }
}

impl NativeCapabilityAdapterV1 for FakeLongJobAdapterV1 {
    fn capability_id(&self) -> &'static str {
        FAKE_LONGJOB_ID
    }

    fn display_name(&self) -> &'static str {
        "Long job"
    }

    fn availability(&self) -> NativeAgentCapabilityStateV1 {
        NativeAgentCapabilityStateV1::Available
    }

    fn supported_protocols(&self) -> &'static [&'static str] {
        &GENERIC_NATIVE_INVOKE_PROTOCOLS
    }

    fn prepare(&self, input: &OpaqueCapabilityPayloadV1) -> AppResult<PreparedInvocationV1> {
        self.with(|state| state.prepares += 1);
        let parsed = parse(input)?;
        Ok(PreparedInvocationV1 {
            identity_digest: input.digest(),
            exclusivity: parsed
                .lane
                .as_deref()
                .map(|lane| ExclusivityKeyV1::new(FAKE_LONGJOB_ID, lane)),
            label: "longjob".into(),
            host_workspace: None,
        })
    }

    fn start(
        &self,
        task_id: &str,
        input: &OpaqueCapabilityPayloadV1,
    ) -> AppResult<StartedInvocationV1> {
        let parsed = parse(input)?;
        self.with(|state| {
            state.starts += 1;
            state.jobs.insert(
                task_id.to_owned(),
                JobV1 {
                    payload: parsed.payload.clone(),
                    progress: 0,
                    cancel: false,
                },
            );
        });
        Ok(StartedInvocationV1 {
            session_reused: false,
            run: Box::new(LongJobRunV1 {
                state: self.state.clone(),
                task_id: task_id.to_owned(),
                steps: parsed.steps,
                fail_at: parsed.fail_at,
            }),
        })
    }

    fn cancel(&self, task_id: &str) -> AppResult<()> {
        self.with(|state| match state.jobs.get_mut(task_id) {
            Some(job) => {
                job.cancel = true;
                Ok(())
            }
            None => Err(AppError::InvalidInput(
                "fake.longjob job is unknown.".into(),
            )),
        })
    }

    fn release(&self, task_id: &str) {
        self.with(|state| {
            if let Some(job) = state.jobs.get_mut(task_id) {
                job.cancel = true;
            }
        });
    }

    fn shutdown(&self) {
        self.with(|state| {
            state.shutdowns += 1;
            for job in state.jobs.values_mut() {
                job.cancel = true;
            }
            state.hold = false;
            state.observation_lost = true;
        });
    }
}

struct LongJobRunV1 {
    state: Arc<(Mutex<FakeStateV1>, Condvar)>,
    task_id: String,
    steps: u32,
    fail_at: Option<u32>,
}

impl NativeInvocationRunV1 for LongJobRunV1 {
    fn run(&mut self, _progress: &dyn NativeInvocationProgressV1) -> NativeInvocationOutcomeV1 {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        for step in 1..=self.steps {
            while state.hold && !state.jobs[&self.task_id].cancel {
                state = wake
                    .wait_timeout(state, Duration::from_millis(10))
                    .unwrap()
                    .0;
            }
            let despite_cancel = state.complete_despite_cancel;
            let job = state.jobs.get_mut(&self.task_id).unwrap();
            if job.cancel && !despite_cancel {
                return outcome(NativeTurnOutcomeV1::Cancelled, None);
            }
            if self.fail_at == Some(step) {
                return outcome(NativeTurnOutcomeV1::Failed, None);
            }
            job.progress = step;
        }
        if state.lose_observation {
            return outcome(NativeTurnOutcomeV1::Unknown, None);
        }
        let job = &state.jobs[&self.task_id];
        let output = json!({ "payload": job.payload, "steps": job.progress });
        outcome(
            NativeTurnOutcomeV1::Completed,
            Some(OpaqueCapabilityPayloadV1::new(output)),
        )
    }

    fn wait_until_observation_lost(&self) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        while !state.observation_lost {
            state = wake
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
}

fn outcome(
    kind: NativeTurnOutcomeV1,
    output: Option<OpaqueCapabilityPayloadV1>,
) -> NativeInvocationOutcomeV1 {
    NativeInvocationOutcomeV1 {
        kind,
        summary: None,
        output,
    }
}
