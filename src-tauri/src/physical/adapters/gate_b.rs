//! Executor-local NativeFence mode. No qualification producer or auto-promotion.
use super::*;
use crate::error::{AppError, AppResult};
use crate::physical::binding::{BindingClockV1, EnvironmentBindingV1};
use crate::physical::{native_protocol as wire, values::*};
use std::sync::atomic::AtomicBool;

pub(in crate::physical) struct NativeEnforcementReceiptV1 {
    // Only this native lane can produce the trusted evidence wrapper.
    raw: wire::Receipt,
}
impl NativeEnforcementReceiptV1 {
    pub(in crate::physical) fn raw(&self) -> &wire::Receipt {
        &self.raw
    }
    pub(in crate::physical) fn validate(
        &self,
        binding: &EnvironmentBindingViewV1,
        session: &SessionId,
        epochs: &BTreeMap<DomainId, u64>,
        request: &RequestId,
        fence: bool,
    ) -> AppResult<()> {
        validate_receipt(&self.raw, binding)?;
        let installed = self
            .raw
            .installed
            .as_ref()
            .ok_or_else(|| invalid("Missing native installed descriptor"))?;
        require(
            installed.session == String::from(session.clone())
                && epochs.len() == 1
                && epochs.iter().all(|(d, e)| {
                    installed.domain == String::from(d.clone())
                        && if fence {
                            self.raw.high_water_epoch == *e && *e > installed.epoch
                        } else {
                            installed.epoch == *e && self.raw.high_water_epoch == *e
                        }
                })
                && self.raw.request == String::from(request.clone())
                && self.raw.accepted
                && self.raw.fenced == fence,
            "Native enforcement receipt mismatch",
        )
    }
}
pub(in crate::physical) fn validate_historical_receipt(
    raw: &wire::Receipt,
    binding: &EnvironmentBindingViewV1,
    session: &SessionId,
    epochs: &BTreeMap<DomainId, u64>,
    request: &RequestId,
    fence: bool,
) -> AppResult<()> {
    NativeEnforcementReceiptV1 { raw: raw.clone() }
        .validate(binding, session, epochs, request, fence)
}
pub(in crate::physical) struct NativeCommandReceiptV1 {
    raw: wire::Receipt,
}
impl NativeCommandReceiptV1 {
    pub(in crate::physical) fn raw(&self) -> &wire::Receipt {
        &self.raw
    }
    pub(in crate::physical) fn validate(
        &self,
        b: &EnvironmentBindingViewV1,
        a: &ActionAuditV1,
        op: &RequestId,
        accepted: bool,
    ) -> AppResult<()> {
        validate_command_receipt(&self.raw, b, a, op, accepted)
    }
}
pub(in crate::physical) fn validate_command_receipt(
    r: &wire::Receipt,
    b: &EnvironmentBindingViewV1,
    a: &ActionAuditV1,
    op: &RequestId,
    accepted: bool,
) -> AppResult<()> {
    validate_receipt(r, b)?;
    let action = r
        .action
        .as_ref()
        .ok_or_else(|| invalid("Native command lacks admitted action"))?;
    require(
        r.installed.as_ref() == Some(&action.install)
            && action.install.session == String::from(a.session.clone())
            && a.epochs.len() == 1
            && a.epochs.iter().all(|(d, e)| {
                action.install.domain == String::from(d.clone()) && action.install.epoch == *e
            })
            && action.action == String::from(a.proposal.action_id.clone())
            && action.payload_digest == String::from(a.proposal.payload_digest.clone())
            && action.deadline_us <= action.install.lease_deadline_us
            && r.request == String::from(op.clone())
            && r.accepted == accepted
            && (!accepted || (!r.fenced && r.sequence > 0)),
        "Native command receipt mismatch",
    )
}
fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}
fn identity(binding: &EnvironmentBindingViewV1) -> AppResult<wire::Identity> {
    require(
        binding.evidence_class == EvidenceClassV1::Simulation
            && binding.subsystems.len() == 1
            && binding.domains().len() == 1,
        "NativeFence v1 requires one isolated simulation body/domain",
    )?;
    let s = binding.subsystems.values().next().unwrap();
    Ok(wire::Identity {
        environment: String::from(binding.environment.clone()),
        domain: String::from((*binding.domains().first().unwrap()).clone()),
        body: String::from(s.body_incarnation.clone()),
        body_ref: String::from(s.body.clone()),
        world: String::from(
            s.world_incarnation
                .clone()
                .ok_or_else(|| invalid("Missing native world incarnation"))?,
        ),
        controller: String::from(s.controller_incarnation.clone()),
    })
}
fn validate_receipt(r: &wire::Receipt, b: &EnvironmentBindingViewV1) -> AppResult<()> {
    require(
        r.protocol == wire::PROTOCOL
            && r.profile == wire::PROFILE
            && r.identity == identity(b)?
            && r.native_us > 0
            && r.installed.as_ref().is_none_or(|i| {
                i.protocol == wire::PROTOCOL
                    && i.profile == wire::PROFILE
                    && i.identity == r.identity
                    && i.domain == r.identity.domain
                    && i.epoch > 0
                    && i.epoch <= i64::MAX as u64
                    && i.lease_deadline_us > 0
            }),
        "Native identity/protocol receipt mismatch",
    )
}
struct Lane {
    #[cfg(unix)]
    socket: Option<std::os::unix::net::UnixStream>,
    sequence: u64,
    installed: Option<wire::Install>,
    action: Option<(wire::Action, u64)>,
    requests: u64,
}
pub(in crate::physical) struct GateBNativeLaneV1 {
    binding: EnvironmentBindingViewV1,
    clock: Arc<dyn BindingClockV1>,
    live: AtomicBool,
    lane: Mutex<Lane>,
}
impl GateBNativeLaneV1 {
    /// Internal enrollment seam: consumes a live executor-local binding and a
    /// private owned socket, never a renderer/wire DTO. Qualification remains a
    /// separate Stage 9 gate. No automatic reconnect or session recovery.
    #[cfg(unix)]
    pub(in crate::physical) fn connect_owned(
        socket: std::os::unix::net::UnixStream,
        binding: &EnvironmentBindingV1,
        clock: Arc<dyn BindingClockV1>,
    ) -> AppResult<Arc<Self>> {
        socket.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
        socket.set_write_timeout(Some(std::time::Duration::from_millis(200)))?;
        let b = binding.view().clone();
        identity(&b)?;
        let run = Arc::new(Self {
            binding: b,
            clock,
            live: AtomicBool::new(true),
            lane: Mutex::new(Lane {
                socket: Some(socket),
                sequence: 0,
                installed: None,
                action: None,
                requests: 0,
            }),
        });
        let mut lane = run.lane.lock();
        let r = run.rpc(
            &mut lane,
            wire::Request::Status {
                protocol: wire::PROTOCOL.into(),
            },
        )?;
        require(
            r.accepted && r.fenced,
            "Native lane already owned; fresh enrollment required",
        )?;
        drop(lane);
        Ok(run)
    }
    fn rpc(&self, lane: &mut Lane, request: wire::Request) -> AppResult<wire::Receipt> {
        require(
            self.live.load(Ordering::Acquire),
            "Native connection lost; no implicit reconnect",
        )?;
        #[cfg(not(unix))]
        {
            let _ = (lane, request);
            Err(invalid("Native MicroDuck requires local Unix transport"))
        }
        #[cfg(unix)]
        {
            use std::io::{Read, Write};
            let result = (|| -> AppResult<wire::Receipt> {
                lane.requests = lane
                    .requests
                    .checked_add(1)
                    .ok_or_else(|| invalid("Native request counter overflow"))?;
                let id = lane.requests;
                let mut bytes = serde_json::to_vec(
                    &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"robot.task","params":request}),
                )?;
                require(bytes.len() < 16 * 1024, "Oversized native task request")?;
                bytes.push(b'\n');
                let socket = lane
                    .socket
                    .as_mut()
                    .ok_or_else(|| invalid("Native socket absent"))?;
                socket.write_all(&bytes)?;
                let mut response = Vec::new();
                loop {
                    let mut b = [0u8; 1];
                    socket.read_exact(&mut b)?;
                    if b[0] == b'\n' {
                        break;
                    }
                    require(response.len() < 16 * 1024, "Oversized native task receipt")?;
                    response.push(b[0]);
                }
                let reply: serde_json::Value = serde_json::from_slice(&response)?;
                require(
                    reply["jsonrpc"] == "2.0" && reply["id"] == id && reply.get("error").is_none(),
                    "Uncorrelated native response",
                )?;
                let receipt: wire::Receipt = serde_json::from_value(reply["result"].clone())?;
                validate_receipt(&receipt, &self.binding)?;
                Ok(receipt)
            })();
            if result.is_err() {
                self.live.store(false, Ordering::Release);
                if let Some(socket) = &lane.socket {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
            }
            result
        }
    }
    fn clock_anchor(&self, lane: &mut Lane) -> AppResult<(u64, u64)> {
        let start = self.clock.read()?.1;
        let r = self.rpc(
            lane,
            wire::Request::Status {
                protocol: wire::PROTOCOL.into(),
            },
        )?;
        let end = self.clock.read()?.1;
        require(
            r.accepted && end >= start && end - start <= 20_000,
            "Unbounded native clock exchange",
        )?;
        // Native sample precedes local receipt: add only the remaining Core
        // lifetime measured AFTER receipt. Network/queue delay shortens authority.
        Ok((r.native_us, end))
    }
    fn native_deadline(native: u64, local: u64, deadline: u64, max: u64) -> AppResult<u64> {
        require(
            deadline > local && deadline - local <= max,
            "Unbounded/expired native authority projection",
        )?;
        native
            .checked_add(deadline - local)
            .ok_or_else(|| invalid("Native deadline overflow"))
    }
    pub(in crate::physical) async fn install(
        self: &Arc<Self>,
        view: NativeSessionInstallViewV1,
    ) -> AppResult<Option<SessionEnforcementEvidenceV1>> {
        let run = self.clone();
        tokio::task::spawn_blocking(move || {
            require(
                view.required == SessionEnforcementClassV1::NativeFence
                    && view.validity.allows()
                    && view.binding == run.binding
                    && view.epochs.len() == 1,
                "Invalid NativeFence installation view",
            )?;
            let mut lane = run.lane.lock();
            require(
                lane.installed
                    .as_ref()
                    .is_none_or(|old| old.session != String::from(view.session.clone())),
                "Native session already had an install decision",
            )?;
            let (native, local) = run.clock_anchor(&mut lane)?;
            let (domain, epoch) = view.epochs.iter().next().unwrap();
            let install = wire::Install {
                protocol: wire::PROTOCOL.into(),
                profile: wire::PROFILE.into(),
                identity: identity(&run.binding)?,
                domain: String::from(domain.clone()),
                session: String::from(view.session.clone()),
                epoch: *epoch,
                request: String::from(view.request.clone()),
                lease_deadline_us: Self::native_deadline(
                    native,
                    local,
                    view.validity.deadline,
                    wire::MAX_LEASE_US,
                )?,
            };
            let raw = run.rpc(
                &mut lane,
                wire::Request::Install {
                    descriptor: install.clone(),
                },
            )?;
            require(
                raw.installed.as_ref() == Some(&install),
                "Native install ACK descriptor mismatch",
            )?;
            let evidence = NativeEnforcementReceiptV1 { raw };
            evidence.validate(
                &view.binding,
                &view.session,
                &view.epochs,
                &view.request,
                false,
            )?;
            lane.installed = Some(install);
            lane.action = None;
            lane.sequence = 0;
            require(view.validity.allows(), "Late native installation")?;
            Ok(Some(SessionEnforcementEvidenceV1 {
                session: view.session,
                epochs: view.epochs,
                request: view.request,
                class: SessionEnforcementClassV1::NativeFence,
                native: Some(evidence),
            }))
        })
        .await
        .map_err(|_| invalid("Native installation task failed"))?
    }
    pub(in crate::physical) async fn write(
        self: &Arc<Self>,
        view: AdmittedActionReadViewV1,
        refresh: bool,
    ) -> AppResult<Option<AdapterWriteReceiptV1>> {
        let run = self.clone();
        tokio::task::spawn_blocking(move || {
            require(
                view.binding == run.binding && view.validity.allows(),
                "Invalid native action view",
            )?;
            let payload = super::microduck_capability::VelocityV1::from_intent(&view.payload)?;
            require(
                payload.is(wire::REFERENCE_FORWARD_MPS, 0.0, 0.0),
                "Native profile admits only exact reference velocity",
            )?;
            let mut lane = run.lane.lock();
            let install = lane
                .installed
                .clone()
                .ok_or_else(|| invalid("Native session not installed"))?;
            require(
                install.session == String::from(view.session.clone())
                    && view.epochs.len() == 1
                    && view.epochs.iter().all(|(d, e)| {
                        install.domain == String::from(d.clone()) && install.epoch == *e
                    }),
                "Changed native session",
            )?;
            if refresh {
                require(
                    lane.action.as_ref().is_some_and(|(a, d)| {
                        a.action == String::from(view.action.clone())
                            && a.payload_digest == String::from(view.payload_digest.clone())
                            && *d == view.deadline
                    }),
                    "Changed native refresh/action deadline",
                )?;
            } else {
                require(lane.action.is_none(), "Native action already originated")?;
                let (native, local) = run.clock_anchor(&mut lane)?;
                let action = wire::Action {
                    install: install.clone(),
                    action: String::from(view.action.clone()),
                    payload_digest: String::from(view.payload_digest.clone()),
                    deadline_us: Self::native_deadline(
                        native,
                        local,
                        view.deadline,
                        wire::MAX_ACTION_US,
                    )?
                    .min(install.lease_deadline_us),
                };
                let raw = run.rpc(
                    &mut lane,
                    wire::Request::Admit {
                        descriptor: action.clone(),
                    },
                )?;
                require(
                    raw.accepted && raw.action.as_ref() == Some(&action) && !raw.fenced,
                    "Native action admission unavailable",
                )?;
                lane.action = Some((action, view.deadline));
            }
            require(view.validity.allows(), "Revoked at native write boundary")?;
            lane.sequence = lane
                .sequence
                .checked_add(1)
                .ok_or_else(|| invalid("Native sequence exhausted"))?;
            let action = lane.action.as_ref().unwrap().0.clone();
            let sequence = lane.sequence;
            let raw = run.rpc(
                &mut lane,
                wire::Request::Move {
                    descriptor: wire::Move {
                        action: action.clone(),
                        request: String::from(view.request.clone()),
                        sequence,
                        twist: wire::REFERENCE_TWIST,
                    },
                },
            )?;
            require(
                raw.request == String::from(view.request.clone())
                    && raw.installed.as_ref() == Some(&install)
                    && raw.action.as_ref() == Some(&action)
                    && (!raw.accepted || (!raw.fenced && raw.sequence == sequence))
                    && view.validity.allows(),
                "Uncorrelated/late native command receipt",
            )?;
            Ok(Some(AdapterWriteReceiptV1 {
                session: view.session,
                epochs: view.epochs,
                request: view.request,
                action: view.action,
                payload_digest: view.payload_digest,
                accepted: raw.accepted,
                native: Some(NativeCommandReceiptV1 { raw }),
            }))
        })
        .await
        .map_err(|_| invalid("Native write task failed"))?
    }
    pub(in crate::physical) async fn fence(
        self: &Arc<Self>,
        view: NativeFenceRequestViewV1,
    ) -> AppResult<Option<SessionEnforcementEvidenceV1>> {
        let run = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut lane = run.lane.lock();
            let install = lane
                .installed
                .clone()
                .ok_or_else(|| invalid("No native installed session"))?;
            require(
                install.session == String::from(view.audit.session.clone())
                    && view.audit.epochs.len() == 1
                    && view
                        .audit
                        .epochs
                        .contains_key(&DomainId::try_from(install.domain.clone())?),
                "Foreign native fence",
            )?;
            let next_epoch = *view.audit.epochs.values().next().unwrap();
            let raw = run.rpc(
                &mut lane,
                wire::Request::Fence {
                    descriptor: wire::Fence {
                        install,
                        next_epoch,
                        request: String::from(view.audit.request.clone()),
                    },
                },
            )?;
            let evidence = NativeEnforcementReceiptV1 { raw };
            evidence.validate(
                &run.binding,
                &view.audit.session,
                &view.audit.epochs,
                &view.audit.request,
                true,
            )?;
            Ok(Some(SessionEnforcementEvidenceV1 {
                session: view.audit.session,
                epochs: view.audit.epochs,
                request: view.audit.request,
                class: SessionEnforcementClassV1::NativeFence,
                native: Some(evidence),
            }))
        })
        .await
        .map_err(|_| invalid("Native fence task failed"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_deadline_projection_shortens_and_never_renews_authority() {
        assert_eq!(
            GateBNativeLaneV1::native_deadline(1000, 200, 500, 1000).unwrap(),
            1300
        );
        assert_eq!(
            GateBNativeLaneV1::native_deadline(1000, 250, 500, 1000).unwrap(),
            1250
        );
        assert!(GateBNativeLaneV1::native_deadline(1000, 500, 500, 1000).is_err());
        assert!(GateBNativeLaneV1::native_deadline(1000, 0, 1001, 1000).is_err());
        assert!(GateBNativeLaneV1::native_deadline(u64::MAX, 0, 1, 1000).is_err());
    }
}
