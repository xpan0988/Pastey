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

/// One authenticated caller of one stream. Process-local: no serde, no clone,
/// never restored. Closing it closes the stream.
pub(crate) struct ToolSessionV1 {
    id: RequestId,
    caller: LabelV1,
    session: Arc<BodyControlSessionV1>,
    lane: Arc<dyn EnvironmentBinding>,
    open: AtomicBool,
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
        self.open_tool_session_inner(session, lane, caller)
    }
    pub(in crate::physical) fn open_tool_session_inner(
        &mut self,
        session: &Arc<BodyControlSessionV1>,
        lane: Arc<dyn EnvironmentBinding>,
        caller: LabelV1,
    ) -> AppResult<Arc<ToolSessionV1>> {
        require(
            session.basis.scope().fields().mode == PhysicalScopeModeV1::DecisionStream,
            "Tools exist only for decision streams",
        )?;
        self.validate_control_session(session, true)?;
        lane.status()?;
        self.control
            .tool_sessions
            .retain(|_, t| t.open.load(Ordering::Acquire));
        require(
            self.control.tool_sessions.len() < 64,
            "Too many tool sessions",
        )?;
        let ts = Arc::new(ToolSessionV1 {
            id: request_id()?,
            caller,
            session: session.clone(),
            lane,
            open: AtomicBool::new(true),
        });
        self.control.tool_sessions.insert(ts.id.clone(), ts.clone());
        Ok(ts)
    }
    pub(in crate::physical) fn tool_session(&self, id: &RequestId) -> Option<Arc<ToolSessionV1>> {
        self.control.tool_sessions.get(id).cloned()
    }
    /// Tool names: the approved options plus the two read-only queries.
    pub(in crate::physical) fn tool_names(&self, ts: &ToolSessionV1) -> Vec<String> {
        let mut names: Vec<String> = ts
            .session
            .basis
            .scope()
            .fields()
            .stream
            .as_ref()
            .map(|s| s.options.iter().map(|o| o.as_str().to_owned()).collect())
            .unwrap_or_default();
        names.extend([OBSERVE_TOOL.to_owned(), BUDGET_TOOL.to_owned()]);
        names
    }
    fn tool_session_current(&self, ts: &ToolSessionV1) -> AppResult<()> {
        require(
            ts.open.load(Ordering::Acquire)
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
    /// refused proposal never reaches the body.
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
                    ts.session.root().root_id(),
                    &ts.caller,
                    option,
                    *duration_us,
                    &e.message(),
                    now,
                )?;
            }
            return Ok(refused(e.message()));
        }
        match call {
            DecisionToolCallV1::RemainingBudget => {
                let service = core.lock();
                let (actions, execution_us) = service
                    .store
                    .remaining_budget(ts.session.root().root_id())?;
                Ok(DecisionToolReplyV1::Budget {
                    actions,
                    execution_us,
                })
            }
            DecisionToolCallV1::Observe => match Self::sample(core, ts).await {
                Ok(view) => Ok(DecisionToolReplyV1::Observation { view }),
                Err(e) => Ok(refused(e.message())),
            },
            DecisionToolCallV1::Decide {
                option,
                duration_us,
            } => Self::decide(core, ts, option, duration_us).await,
        }
    }
    /// Samples the binding and records the sample. A lost binding closes the
    /// stream: its body stops by the binding's own loss policy.
    async fn sample(core: &Mutex<Self>, ts: &Arc<ToolSessionV1>) -> AppResult<CanonicalJsonV1> {
        let lane = ts.lane.clone();
        let sampled = tokio::task::spawn_blocking(move || lane.observe())
            .await
            .map_err(|_| crate::error::AppError::InvalidInput("Binding sample failed".into()))
            .and_then(|r| r);
        let recorded = sampled.and_then(|sample| {
            let view = sample.view.clone();
            let mut service = core.lock();
            let ingress = service.local_ingress()?;
            service.ingest_binding_sample(&ingress, &ts.session, ts.lane.as_ref(), sample, true)?;
            Ok(view)
        });
        if recorded.is_err() {
            ts.open.store(false, Ordering::Release);
            let _ = Self::revoke_control_session(core, &ts.session, ts.lane.as_ref()).await;
            return recorded;
        }
        // The stream's termination condition is its completion contract. Once
        // the witness verifies it, Core accepts the task and ends the stream.
        let terminated = core.lock().evaluate_stream(ts);
        if terminated.unwrap_or(false) {
            let _ = Self::revoke_control_session(core, &ts.session, ts.lane.as_ref()).await;
        }
        recorded
    }
    /// Evaluates the latest dispatched decision. Returns true once the task
    /// was accepted by this or an earlier evaluation.
    fn evaluate_stream(&mut self, ts: &ToolSessionV1) -> AppResult<bool> {
        let root = ts.session.root().root_id().clone();
        if self.store.acceptance(&root)? != crate::physical::evidence::AcceptanceStateV1::Pending {
            return Ok(true);
        }
        let Some(action) = self.store.latest_dispatched_action(&root)? else {
            return Ok(false);
        };
        let ingress = self.local_ingress()?;
        // Not yet decidable (no terminal, missing evidence) is not an error of
        // the stream; the next sample evaluates again.
        let Ok(c) = self.evaluate_physical_consequence(&ingress, &action) else {
            return Ok(false);
        };
        if c.state != crate::physical::evidence::ConsequenceStateV1::Verified {
            return Ok(false);
        }
        self.decide_physical_acceptance(
            &ingress,
            &c.root,
            &c.attempt,
            &c.action,
            c.revision,
            &c.completion_digest,
            false,
        )?;
        Ok(true)
    }
    async fn decide(
        core: &Mutex<Self>,
        ts: &Arc<ToolSessionV1>,
        option: String,
        duration_us: u64,
    ) -> AppResult<DecisionToolReplyV1> {
        let root = ts.session.root().root_id().clone();
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
        if let Err(e) = Self::sample(core, ts).await {
            return refuse(e.message().to_owned());
        }
        let admitted = {
            let mut service = core.lock();
            service
                .construct_session_grant(ts.session.clone())
                .and_then(|g| service.admit_decision(&g, &ts.caller, &label, duration))
        };
        let action = match admitted {
            Ok(a) => a,
            Err(e) => return refuse(e.message().to_owned()),
        };
        // The write's native reply. A refusal or unknown reply closes the
        // stream inside Core (no retry, budget kept); the caller learns only
        // the disposition.
        let _ = Self::dispatch_admitted_action(core, &action, ts.lane.as_ref()).await;
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
    ) -> AppResult<bool> {
        ts.open.store(false, Ordering::Release);
        core.lock().control.tool_sessions.remove(&ts.id);
        Self::revoke_control_session(core, &ts.session, ts.lane.as_ref()).await
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
        self.store.decisions(ts.session.root().root_id())
    }
}
