//! Executor-side decision-stream tools. Transport-agnostic: local callers and
//! the bridge (physical-control-v2) reach this one dispatcher, which runs on
//! the Host that owns the body; admission never leaves it. Pastey defines and
//! runs no brain loop: a brain is any authenticated tool caller.
use super::*;
use serde::{Deserialize, Serialize};

/// Read-only query of the binding's own view for a brain.
pub(crate) const OBSERVE_TOOL: &str = "observe";
/// Read-only query of what the approval still allows.
pub(crate) const BUDGET_TOOL: &str = "remaining_budget";

/// One call. Options arrive as caller text and are validated here, so a
/// malformed call is a recorded refusal, not a transport error.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DecisionToolCallV1 {
    Decide { option: String, duration_us: u64 },
    Observe,
    RemainingBudget,
}

/// What a caller learns. Admission is allowed or refused; `disposition` is the
/// binding's native reply to the write ("accepted", "refused" or "unknown").
/// Neither is a physical consequence: only the witness decides that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DecisionToolReplyV1 {
    Allowed { disposition: String },
    Refused { reason: String },
    Observation { view: CanonicalJsonV1 },
    Budget { actions: u64, execution_us: u64 },
}

/// Executor-side runtime of one stream: the binding lane, serialized
/// sampling (the timer and tool calls must not interleave observations), the
/// last brain activity for the idle lease, and the end latch. Process-local.
pub(crate) struct StreamRuntimeV1 {
    session: Arc<BodyControlSessionV1>,
    lane: Arc<dyn EnvironmentBinding>,
    sampling: tokio::sync::Mutex<()>,
    last_activity: AtomicU64,
    ended: AtomicBool,
}
/// Result of one executor timer tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamTickV1 {
    Continue(std::time::Duration),
    Ended,
}

/// One authenticated caller of one stream. Process-local: no serde, no clone,
/// never restored. Closing it closes the stream.
pub(crate) struct ToolSessionV1 {
    id: RequestId,
    caller: LabelV1,
    stream: Arc<StreamRuntimeV1>,
    open: AtomicBool,
    /// Ticks of the last released observation, for the declared rate.
    last_observation: parking_lot::Mutex<Option<u64>>,
}
impl ToolSessionV1 {
    pub(crate) fn id(&self) -> &RequestId {
        &self.id
    }
}

fn refused(reason: impl Into<String>) -> DecisionToolReplyV1 {
    DecisionToolReplyV1::Refused {
        reason: reason.into(),
    }
}
fn stream_scope(s: &BodyControlSessionV1) -> AppResult<&DecisionStreamScopeV1> {
    s.basis.scope().fields().stream.as_ref().ok_or_else(|| {
        crate::error::AppError::InvalidInput("Tools exist only for decision streams".into())
    })
}

impl PhysicalControlServiceV1 {
    /// Opens a tool session for a local stream. Optional: a same-Host brain
    /// may drive its body through the binding without Pastey; this path gives
    /// it Pastey's envelope and records. Remote callers open theirs through
    /// the bridge with verified peer ingress.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optional local path has no product caller yet"
        )
    )]
    pub(in crate::physical) fn open_tool_session(
        &mut self,
        ingress: &LocalCoreIngressV1,
        session: &Arc<BodyControlSessionV1>,
        lane: Arc<dyn EnvironmentBinding>,
        caller: LabelV1,
    ) -> AppResult<Arc<ToolSessionV1>> {
        self.validate_ingress(ingress)?;
        require(
            session.root().peer.is_none(),
            "A remote stream's tools are reached through its bridge",
        )?;
        let destination = self.runtime.host_ref().clone();
        self.open_tool_session_inner(session, lane, caller, &destination)
    }
    /// The stream's executor runtime, created once per installed session.
    pub(in crate::physical) fn stream_runtime(
        &mut self,
        session: &Arc<BodyControlSessionV1>,
        lane: Arc<dyn EnvironmentBinding>,
    ) -> AppResult<Arc<StreamRuntimeV1>> {
        stream_scope(session)?;
        if let Some(existing) = self.control.streams.get(session.id()) {
            return Ok(existing.clone());
        }
        self.validate_control_session(session, true)?;
        let (_, ticks) = self.clock.read()?;
        let stream = Arc::new(StreamRuntimeV1 {
            session: session.clone(),
            lane,
            sampling: tokio::sync::Mutex::new(()),
            last_activity: AtomicU64::new(ticks),
            ended: AtomicBool::new(false),
        });
        self.control
            .streams
            .insert(session.id().clone(), stream.clone());
        Ok(stream)
    }
    pub(in crate::physical) fn open_tool_session_inner(
        &mut self,
        session: &Arc<BodyControlSessionV1>,
        lane: Arc<dyn EnvironmentBinding>,
        caller: LabelV1,
        destination: &HostRef,
    ) -> AppResult<Arc<ToolSessionV1>> {
        require(
            stream_scope(session)?.observation.destination == *destination,
            "Observations are declared for another Host",
        )?;
        self.validate_control_session(session, true)?;
        lane.status()?;
        let stream = self.stream_runtime(session, lane)?;
        require(!stream.ended.load(Ordering::Acquire), "Stream ended")?;
        self.control
            .tool_sessions
            .retain(|_, t| t.open.load(Ordering::Acquire));
        require(
            self.control.tool_sessions.len() < 64,
            "Too many tool sessions",
        )?;
        let (_, ticks) = self.clock.read()?;
        stream.last_activity.fetch_max(ticks, Ordering::AcqRel);
        let ts = Arc::new(ToolSessionV1 {
            id: request_id()?,
            caller,
            stream,
            open: AtomicBool::new(true),
            last_observation: parking_lot::Mutex::new(None),
        });
        self.control.tool_sessions.insert(ts.id.clone(), ts.clone());
        Ok(ts)
    }
    pub(in crate::physical) fn tool_session(&self, id: &RequestId) -> Option<Arc<ToolSessionV1>> {
        self.control.tool_sessions.get(id).cloned()
    }
    /// Tool names: the approved options plus the two read-only queries.
    pub(in crate::physical) fn tool_names(&self, ts: &ToolSessionV1) -> Vec<String> {
        let mut names: Vec<String> = stream_scope(&ts.stream.session)
            .map(|s| s.options.iter().map(|o| o.as_str().to_owned()).collect())
            .unwrap_or_default();
        names.extend([OBSERVE_TOOL.to_owned(), BUDGET_TOOL.to_owned()]);
        names
    }
    fn tool_session_current(&self, ts: &ToolSessionV1) -> AppResult<()> {
        require(
            ts.open.load(Ordering::Acquire)
                && !ts.stream.ended.load(Ordering::Acquire)
                && self
                    .control
                    .tool_sessions
                    .get(&ts.id)
                    .is_some_and(|t| std::ptr::eq(t.as_ref(), ts)),
            "Tool session closed",
        )
    }

    /// Runs one tool call. Every `Decide` is a new proposal through Core
    /// admission and is recorded (proposer, allowed or refused, reason); a
    /// refused proposal never reaches the body. Completion and the stream's
    /// end are driven by `stream_tick`, never by a call.
    pub(in crate::physical) async fn call_tool(
        core: &Mutex<Self>,
        ts: &Arc<ToolSessionV1>,
        call: DecisionToolCallV1,
    ) -> AppResult<DecisionToolReplyV1> {
        let current = core.lock().tool_session_current(ts);
        if let Err(e) = current {
            // A closed session still answers, and a proposal on it is recorded.
            if let DecisionToolCallV1::Decide {
                option,
                duration_us,
            } = &call
            {
                let service = core.lock();
                let (now, _) = service.clock.read()?;
                service.store.record_refusal(
                    ts.stream.session.root().root_id(),
                    &ts.caller,
                    option,
                    *duration_us,
                    &e.message(),
                    now,
                )?;
            }
            return Ok(refused(e.message()));
        }
        let (_, ticks) = core.lock().clock.read()?;
        ts.stream.last_activity.fetch_max(ticks, Ordering::AcqRel);
        match call {
            DecisionToolCallV1::RemainingBudget => {
                let service = core.lock();
                let (actions, execution_us) = service
                    .store
                    .remaining_budget(ts.stream.session.root().root_id())?;
                Ok(DecisionToolReplyV1::Budget {
                    actions,
                    execution_us,
                })
            }
            DecisionToolCallV1::Observe => {
                let flow = stream_scope(&ts.stream.session)?.observation.clone();
                {
                    let mut last = ts.last_observation.lock();
                    if last.is_some_and(|l| ticks.saturating_sub(l) < flow.min_interval_us.get()) {
                        return Ok(refused("Observation rate ceiling exceeded"));
                    }
                    *last = Some(ticks);
                }
                match Self::sample(core, &ts.stream).await {
                    // Only declared fields leave the executor.
                    Ok(view) => Ok(DecisionToolReplyV1::Observation {
                        view: view.select(&flow.fields),
                    }),
                    Err(e) => Ok(refused(e.message())),
                }
            }
            DecisionToolCallV1::Decide {
                option,
                duration_us,
            } => Self::decide(core, ts, option, duration_us).await,
        }
    }
    /// Samples the binding and records the sample, one sample at a time per
    /// stream. A lost binding ends the stream: its body stops by the
    /// binding's own loss policy.
    async fn sample(
        core: &Mutex<Self>,
        stream: &Arc<StreamRuntimeV1>,
    ) -> AppResult<CanonicalJsonV1> {
        let _serial = stream.sampling.lock().await;
        let lane = stream.lane.clone();
        let sampled = tokio::task::spawn_blocking(move || lane.observe())
            .await
            .map_err(|_| crate::error::AppError::InvalidInput("Binding sample failed".into()))
            .and_then(|r| r);
        let recorded = sampled.and_then(|sample| {
            let view = sample.view.clone();
            let mut service = core.lock();
            let ingress = service.local_ingress()?;
            service.ingest_binding_sample(
                &ingress,
                &stream.session,
                stream.lane.as_ref(),
                sample,
                true,
            )?;
            Ok(view)
        });
        if recorded.is_err() {
            drop(_serial);
            Self::end_stream(core, stream).await;
        }
        recorded
    }
    /// Ends the stream once: its tool sessions close, authority closes and
    /// the fence is requested. The outcome stays whatever the witness last
    /// verified; otherwise it is uncertain.
    async fn end_stream(core: &Mutex<Self>, stream: &Arc<StreamRuntimeV1>) {
        if stream.ended.swap(true, Ordering::AcqRel) {
            return;
        }
        {
            let mut service = core.lock();
            service.control.tool_sessions.retain(|_, t| {
                if Arc::ptr_eq(&t.stream, stream) {
                    t.open.store(false, Ordering::Release);
                    false
                } else {
                    true
                }
            });
        }
        let _ = Self::revoke_control_session(core, &stream.session, stream.lane.as_ref()).await;
    }
    /// One executor timer tick: the stream's completion and end never depend
    /// on a brain calling. It samples, evaluates the latest dispatched
    /// decision and ends the stream on verified completion, a spent budget,
    /// an expired idle lease (treated like a crashed brain) or closed
    /// authority. The next tick comes after half the observation gap.
    pub(in crate::physical) async fn stream_tick(
        core: &Mutex<Self>,
        stream: &Arc<StreamRuntimeV1>,
    ) -> AppResult<StreamTickV1> {
        if stream.ended.load(Ordering::Acquire) {
            return Ok(StreamTickV1::Ended);
        }
        let scope = stream.session.basis.scope().fields().clone();
        let flow = stream_scope(&stream.session)?.clone();
        let period = std::time::Duration::from_micros(
            (scope.freshness.observation.max_gap_us.get() / 2).max(1),
        );
        let (live, idle) = {
            let mut service = core.lock();
            let (_, ticks) = service.clock.read()?;
            let live = service
                .validate_control_session(&stream.session, true)
                .is_ok();
            let idle = ticks.saturating_sub(stream.last_activity.load(Ordering::Acquire))
                > flow.idle_lease_us(&scope.execution);
            (live, idle)
        };
        if !live || idle {
            Self::end_stream(core, stream).await;
            return Ok(StreamTickV1::Ended);
        }
        if Self::sample(core, stream).await.is_err() {
            return Ok(StreamTickV1::Ended);
        }
        if core.lock().check_effect_bound(stream)? {
            Self::end_stream(core, stream).await;
            return Ok(StreamTickV1::Ended);
        }
        let (verified, spent) = {
            let mut service = core.lock();
            let verified = service.evaluate_stream(stream, flow.on_completion)?;
            let root = stream.session.root().root_id().clone();
            let (actions, time) = service.store.remaining_budget(&root)?;
            let (_, ticks) = service.clock.read()?;
            let busy = service.control.actions.values().any(|a| {
                a.audit.session == stream.session.audit.id
                    && a.valid.load(Ordering::Acquire)
                    && ticks < a.deadline
            });
            (verified, (actions == 0 || time == 0) && !busy)
        };
        if verified || spent {
            Self::end_stream(core, stream).await;
            return Ok(StreamTickV1::Ended);
        }
        Ok(StreamTickV1::Continue(period))
    }
    /// Drives `stream_tick` on its own period until the stream ends. The
    /// executor spawns this for every installed stream.
    pub(crate) async fn supervise_stream(core: &Mutex<Self>, stream: Arc<StreamRuntimeV1>) {
        loop {
            match Self::stream_tick(core, &stream).await {
                Ok(StreamTickV1::Continue(d)) => tokio::time::sleep(d).await,
                Ok(StreamTickV1::Ended) => break,
                Err(_) => {
                    Self::end_stream(core, &stream).await;
                    break;
                }
            }
        }
    }
    /// Checks a witnessed effect bound over the latest dispatched decision.
    /// An admitted Contradicted verdict (its window and evidence recomputed
    /// from the stored observations it cites) is recorded and closes the root
    /// with `effect_bound_violated`. Returns true on a violation.
    fn check_effect_bound(&mut self, stream: &StreamRuntimeV1) -> AppResult<bool> {
        let EffectBoundV1::Witnessed {
            predicate,
            required_witness,
        } = &stream_scope(&stream.session)?.effect_bound
        else {
            return Ok(false);
        };
        let root = stream.session.root().root_id().clone();
        let Some(action) = self.store.latest_dispatched_action(&root)? else {
            return Ok(false);
        };
        let Some(witness) = self.witnesses.get(&predicate.id).cloned() else {
            return Ok(false);
        };
        let (lineage, observations) = self.store.effect_bound_facts(&action)?;
        let predicate_digest = digest("pastey-physical-effect-bound-v1", predicate)?;
        let (now, _) = self.clock.read()?;
        let input = crate::physical::evidence::EffectBoundInputV1 {
            predicate,
            predicate_digest: &predicate_digest,
            lineage: &lineage,
            observations: &observations,
        };
        let Ok(verdict) = witness.effect_bound(&input) else {
            return Ok(false);
        };
        if verdict.result != crate::physical::evidence::WitnessResultV1::Contradicted {
            return Ok(false);
        }
        let cited: Vec<_> = verdict
            .observations
            .iter()
            .filter_map(|id| observations.iter().find(|o| &o.fact.id == id))
            .collect();
        let rebuilt = crate::physical::evidence::WitnessVerdictV1::over(
            &action,
            &predicate_digest,
            verdict.result,
            verdict.witness_class,
            verdict.reason.as_str(),
            &cited,
        )?;
        if rebuilt != verdict
            || cited.is_empty()
            || !verdict.witness_class.satisfies(*required_witness)
        {
            return Ok(false);
        }
        self.control.invalidate_root(&root);
        self.store.record_effect_violation(&root, &verdict, now)?;
        Ok(true)
    }
    /// Evaluates the latest dispatched decision. Returns true once the
    /// completion is verified (and, if the scope says so, accepted).
    fn evaluate_stream(
        &mut self,
        stream: &StreamRuntimeV1,
        on_completion: CompletionAcceptanceV1,
    ) -> AppResult<bool> {
        let root = stream.session.root().root_id().clone();
        if self.store.acceptance(&root)? != crate::physical::evidence::AcceptanceStateV1::Pending {
            return Ok(true);
        }
        let Some(action) = self.store.latest_dispatched_action(&root)? else {
            return Ok(false);
        };
        let ingress = self.local_ingress()?;
        // Not yet decidable (no terminal, missing evidence) is not an error of
        // the stream; the next tick evaluates again.
        let Ok(c) = self.evaluate_physical_consequence(&ingress, &action) else {
            return Ok(false);
        };
        if c.state != crate::physical::evidence::ConsequenceStateV1::Verified {
            return Ok(false);
        }
        if on_completion == CompletionAcceptanceV1::Automatic {
            self.decide_physical_acceptance(
                &ingress,
                &c.root,
                &c.attempt,
                &c.action,
                c.revision,
                &c.completion_digest,
                false,
            )?;
        }
        Ok(true)
    }
    async fn decide(
        core: &Mutex<Self>,
        ts: &Arc<ToolSessionV1>,
        option: String,
        duration_us: u64,
    ) -> AppResult<DecisionToolReplyV1> {
        let root = ts.stream.session.root().root_id().clone();
        let refuse = |reason: String| -> AppResult<DecisionToolReplyV1> {
            let service = core.lock();
            let (now, _) = service.clock.read()?;
            service
                .store
                .record_refusal(&root, &ts.caller, &option, duration_us, &reason, now)?;
            Ok(refused(reason))
        };
        let (label, duration) = match (
            LabelV1::try_from(option.clone()),
            PositiveMicros::try_from(duration_us),
        ) {
            (Ok(l), Ok(d)) => (l, d),
            (Err(_), _) => return refuse("Invalid option name".into()),
            (_, Err(_)) => return refuse("Invalid action duration".into()),
        };
        // A fresh trusted sample precedes every challenge.
        if let Err(e) = Self::sample(core, &ts.stream).await {
            return refuse(e.message().to_owned());
        }
        let admitted = {
            let mut service = core.lock();
            service
                .construct_session_grant(ts.stream.session.clone())
                .and_then(|g| service.admit_decision(&g, &ts.caller, &label, duration))
        };
        let action = match admitted {
            Ok(a) => a,
            Err(e) => return refuse(e.message().to_owned()),
        };
        // The write's native reply. A refusal or unknown reply closes the
        // stream inside Core (no retry, budget kept); the caller learns only
        // the disposition.
        let _ = Self::dispatch_admitted_action(core, &action, ts.stream.lane.as_ref()).await;
        let disposition = match core.lock().store.action_status(action.id())?.1.as_str() {
            "fake_accepted" => "accepted",
            "fake_refused" => "refused",
            _ => "unknown",
        };
        Ok(DecisionToolReplyV1::Allowed {
            disposition: disposition.into(),
        })
    }
    /// Ends a tool session and with it the stream: authority closes first,
    /// then the fence is requested. The outcome stays uncertain unless the
    /// witness already verified the completion contract.
    pub(in crate::physical) async fn close_tool_session(
        core: &Mutex<Self>,
        ts: &Arc<ToolSessionV1>,
    ) -> AppResult<()> {
        ts.open.store(false, Ordering::Release);
        Self::end_stream(core, &ts.stream).await;
        Ok(())
    }
    /// The records of a stream, in order.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the optional local path has no product caller yet"
        )
    )]
    pub(in crate::physical) fn decision_records(
        &self,
        ts: &ToolSessionV1,
    ) -> AppResult<Vec<crate::physical::store::DecisionRecordV1>> {
        self.store.decisions(ts.stream.session.root().root_id())
    }
}

#[cfg(test)]
pub(super) fn test_stream(ts: &ToolSessionV1) -> Arc<StreamRuntimeV1> {
    ts.stream.clone()
}
