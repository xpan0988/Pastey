//! Narrow native task guard. IPC queues metadata; the control thread consumes it
//! under the same lock that serializes fencing through Safety::apply.
use duck_ipc_proto::task_authority::*;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

pub struct Authority {
    enabled: AtomicBool,
    // Independent of state contention. First failure permanently closes this launch.
    loss: AtomicU8,
    #[cfg(test)]
    controller_failure: AtomicBool,
    state: Mutex<State>,
    #[cfg(test)]
    pause: Mutex<
        Option<(
            bool,
            std::sync::mpsc::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}
pub struct State {
    identity: Identity,
    high_water: u64,
    installed: Option<Install>,
    owner: u64,
    action: Option<Action>,
    pending: Option<Move>,
    received_us: u64,
    sequence: u64,
    consumed: u64,
    fenced: bool,
    reason: String,
    fence: Option<Fence>,
    invalidated: bool,
}
impl Default for Authority {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            pause: Mutex::new(None),
            enabled: AtomicBool::new(false),
            loss: AtomicU8::new(0),
            #[cfg(test)]
            controller_failure: AtomicBool::new(false),
            state: Mutex::new(State::new(Identity {
                environment: "disabled".into(),
                domain: "disabled".into(),
                body: "disabled".into(),
                body_ref: "disabled".into(),
                world: "disabled".into(),
                controller: "disabled".into(),
            })),
        }
    }
}
impl State {
    fn new(identity: Identity) -> Self {
        Self {
            identity,
            high_water: 0,
            installed: None,
            owner: 0,
            action: None,
            pending: None,
            received_us: 0,
            sequence: 0,
            consumed: 0,
            fenced: true,
            reason: "not_installed".into(),
            fence: None,
            invalidated: false,
        }
    }
    pub fn invalidate(&mut self, reason: &str) {
        self.close(reason);
        self.invalidated = true;
    }
    fn close(&mut self, reason: &str) {
        self.fenced = true;
        self.pending = None;
        self.reason = reason.into();
    }
    fn expire(&mut self, now: u64) {
        if !self.fenced
            && self
                .installed
                .as_ref()
                .is_some_and(|i| now >= i.lease_deadline_us)
        {
            self.close("session_expired");
        } else if !self.fenced && self.action.as_ref().is_some_and(|a| now >= a.deadline_us) {
            self.close("action_expired");
        } else if !self.fenced
            && self.pending.is_some()
            && now.saturating_sub(self.received_us) >= REFRESH_LOSS_US
        {
            self.close("refresh_lost");
        }
    }
    pub fn valid(&mut self, now: u64) -> bool {
        self.expire(now);
        !self.fenced
            && self.pending.as_ref().is_some_and(|m| {
                self.action.as_ref() == Some(&m.action)
                    && self.installed.as_ref() == Some(&m.action.install)
            })
    }
    pub fn twist(&mut self, now: u64) -> [f64; 3] {
        if self.valid(now) {
            self.consumed = self.sequence;
            self.reason = "consumed".into();
            self.pending.as_ref().unwrap().twist
        } else {
            [0.0; 3]
        }
    }
    fn receipt(&self, now: u64, request: &str, accepted: bool, reason: &str) -> Receipt {
        Receipt {
            protocol: PROTOCOL.into(),
            profile: PROFILE.into(),
            identity: self.identity.clone(),
            native_us: now,
            request: request.into(),
            accepted,
            reason: reason.into(),
            installed: self.installed.clone(),
            action: self.action.clone(),
            sequence: self.sequence,
            high_water_epoch: self.high_water,
            fenced: self.fenced,
            consumed_sequence: self.consumed,
        }
    }
    fn current(&self, i: &Install, owner: u64) -> bool {
        !self.fenced
            && self.owner == owner
            && self.installed.as_ref() == Some(i)
            && self.identity == i.identity
            && self.high_water == i.epoch
    }
    fn install(&mut self, i: &Install, owner: u64, now: u64) -> Result<(), &'static str> {
        if self.invalidated {
            return Err("native_reset_requires_fresh_launch");
        }
        if i.protocol != PROTOCOL
            || i.profile != PROFILE
            || !i.identity.validate()
            || i.identity != self.identity
            || i.domain != self.identity.domain
            || !bounded(&i.domain)
            || !bounded(&i.session)
            || !bounded(&i.request)
            || i.epoch == 0
            || i.epoch > i64::MAX as u64
        {
            return Err("incompatible_identity_protocol_or_scope");
        }
        if self.current(i, owner) {
            return Ok(());
        }
        if i.epoch <= self.high_water {
            return Err("stale_epoch");
        }
        // No foreign connection takeover. A closed connection cannot reconnect/resume.
        if !self.fenced && self.owner != owner {
            return Err("foreign_owner");
        }
        if self
            .installed
            .as_ref()
            .is_some_and(|old| old.domain != i.domain)
        {
            return Err("foreign_domain");
        }
        if i.lease_deadline_us <= now || i.lease_deadline_us - now > MAX_LEASE_US {
            return Err("unbounded_or_expired_lease");
        }
        self.high_water = i.epoch;
        self.installed = Some(i.clone());
        self.owner = owner;
        self.action = None;
        self.pending = None;
        self.sequence = 0;
        self.consumed = 0;
        self.fence = None;
        self.fenced = false;
        self.reason = "installed".into();
        Ok(())
    }
    fn admit(&mut self, a: &Action, owner: u64, now: u64) -> Result<(), &'static str> {
        if !self.current(&a.install, owner) {
            return Err("invalid_session");
        }
        if !bounded(&a.action)
            || a.payload_digest.len() != 64
            || !a
                .payload_digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("invalid_action_digest");
        }
        if let Some(old) = &self.action {
            return if old == a {
                Ok(())
            } else {
                Err("changed_action")
            };
        }
        if a.deadline_us <= now
            || a.deadline_us - now > MAX_ACTION_US
            || a.deadline_us > a.install.lease_deadline_us
        {
            return Err("unbounded_or_expired_action");
        }
        self.action = Some(a.clone());
        self.reason = "admitted".into();
        Ok(())
    }
    fn queue(&mut self, m: &Move, owner: u64, now: u64) -> Result<(), &'static str> {
        if !self.current(&m.action.install, owner) || self.action.as_ref() != Some(&m.action) {
            return Err("invalid_action_session");
        }
        if m.twist != REFERENCE_TWIST
            || !bounded(&m.request)
            || m.sequence == 0
            || m.sequence <= self.sequence
        {
            return Err("changed_payload_or_stale_sequence");
        }
        self.pending = Some(m.clone());
        self.sequence = m.sequence;
        self.received_us = now;
        self.reason = "queued".into();
        Ok(())
    }
    fn fence(&mut self, f: &Fence, owner: u64) -> Result<(), &'static str> {
        if self.fence.as_ref() == Some(f) && self.owner == owner {
            return Ok(());
        }
        if self.installed.as_ref() != Some(&f.install)
            || self.owner != owner
            || f.install.identity != self.identity
            || f.next_epoch <= self.high_water
            || f.next_epoch > i64::MAX as u64
            || !bounded(&f.request)
        {
            return Err("stale_or_foreign_fence");
        }
        self.high_water = f.next_epoch;
        self.close("fenced");
        self.fence = Some(f.clone());
        Ok(())
    }
}
impl Authority {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub fn configure(&self, identity: Identity) -> Result<(), &'static str> {
        if !identity.validate() || self.enabled() || self.loss.load(Ordering::Acquire) != 0 {
            return Err("invalid_native_configuration");
        }
        *self.state.lock().map_err(|_| "poisoned_native_guard")? = State::new(identity);
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    pub fn request_live(&self, r: &Request, owner: u64) -> Receipt {
        self.request_with_clock(r, owner, || duck_ipc_proto::clock::monotonic_ns() / 1000)
    }
    #[cfg(test)]
    pub fn request(&self, r: &Request, owner: u64, now: u64) -> Receipt {
        self.request_with_clock(r, owner, || now)
    }
    fn request_with_clock(&self, r: &Request, owner: u64, clock: impl Fn() -> u64) -> Receipt {
        let mut s = match self.state.lock() {
            Ok(s) => s,
            Err(poison) => {
                let mut s = poison.into_inner();
                s.invalidate("poisoned_native_guard");
                return s.receipt(clock(), "error", false, "poisoned_native_guard");
            }
        };
        let now = clock(); // sample only AFTER acquiring the native application/fence lock
        self.fold_loss(&mut s);
        s.expire(now);
        let request = match r {
            Request::Install { descriptor } => descriptor.request.as_str(),
            Request::Admit { descriptor } => descriptor.install.request.as_str(),
            Request::Move { descriptor } => descriptor.request.as_str(),
            Request::Fence { descriptor } => descriptor.request.as_str(),
            Request::Status { .. } => "status",
        };
        let mut result = if !self.enabled() {
            Err("native_task_mode_disabled")
        } else {
            match r {
                Request::Status { protocol } => {
                    if protocol == PROTOCOL {
                        Ok(())
                    } else {
                        Err("unsupported_protocol")
                    }
                }
                Request::Install { descriptor } => s.install(descriptor, owner, now),
                Request::Admit { descriptor } => s.admit(descriptor, owner, now),
                Request::Move { descriptor } => s.queue(descriptor, owner, now),
                Request::Fence { descriptor } => s.fence(descriptor, owner),
            }
        };
        // Invalid commands from the current owner close its task window. A
        // foreign connection cannot revoke somebody else's valid authority.
        if result.is_err() && matches!(r, Request::Move { .. }) && s.owner == owner {
            s.close("invalid_task_command");
        }
        // A loss may have been published while this request held the mutex.
        if self.fold_loss(&mut s) && !matches!(r, Request::Status { .. }) {
            result = Err("native_reset_requires_fresh_launch");
        }
        s.receipt(
            now,
            request,
            result.is_ok(),
            result.err().unwrap_or(&s.reason),
        )
    }
    pub fn disconnect(&self, owner: u64) {
        if let Ok(mut s) = self.state.lock() {
            if s.owner == owner {
                s.close("connection_lost");
            }
        }
    }
    fn fold_loss(&self, state: &mut State) -> bool {
        let reason = match self.loss.load(Ordering::Acquire) {
            0 => return false,
            1 => "native_controller_loss",
            2 => "native_write_loss",
            3 => "native_io_loss",
            _ => "native_launch_loss",
        };
        state.invalidate(reason);
        true
    }
    /// Nonblocking even when the caller holds the consumption guard or another
    /// thread owns it. No request, epoch or reconfiguration clears this latch.
    pub fn native_loss(&self, reason: &str) {
        let code = match reason {
            "native_controller_loss" => 1,
            "native_write_loss" => 2,
            "native_io_loss" => 3,
            _ => 4,
        };
        let _ = self
            .loss
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
        if let Ok(mut s) = self.state.try_lock() {
            self.fold_loss(&mut s);
        }
    }
    #[cfg(test)]
    pub fn inject_controller_failure(&self) {
        self.controller_failure.store(true, Ordering::Release);
    }
    #[cfg(test)]
    pub fn take_controller_failure(&self) -> bool {
        self.controller_failure.swap(false, Ordering::AcqRel)
    }
    pub fn can_provision(&self) -> bool {
        self.loss.load(Ordering::Acquire) == 0
            && self
                .state
                .lock()
                .is_ok_and(|s| s.high_water == 0 && s.installed.is_none() && !s.invalidated)
    }
    /// The loop never waits for IPC. A contended/poisoned lock means zero task
    /// twist and discarding this tick's motion targets. Held through native apply.
    #[cfg(test)]
    pub fn pause_at(&self, apply: bool) {
        let mut hook = self.pause.lock().unwrap();
        if hook.as_ref().is_some_and(|h| h.0 == apply) {
            let (_, entered, release) = hook.take().unwrap();
            drop(hook);
            entered.send(()).unwrap();
            release
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        }
    }
    #[cfg(test)]
    pub fn inject_pause(
        &self,
        apply: bool,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        *self.pause.lock().unwrap() = Some((apply, tx, resume));
        (rx, release)
    }
    pub fn consume(&self) -> Option<MutexGuard<'_, State>> {
        let mut state = self.state.try_lock().ok()?;
        self.fold_loss(&mut state);
        Some(state)
    }
}

/// A fresh unpredictable controller incarnation on every process boot; no
/// durable task rows are restored. Failure to get OS randomness aborts launch.
pub fn controller_incarnation() -> std::io::Result<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "incarnation:v1:{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}
pub struct Connection<'a> {
    pub authority: &'a Authority,
    pub owner: u64,
}
impl Drop for Connection<'_> {
    fn drop(&mut self) {
        self.authority.disconnect(self.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_clears_pinned_standing_threshold_early_under_native_ema() {
        let threshold = duck_control::policy::DEFAULT_STANDING_THRESHOLD;
        let control = robotd_params::Control::default();
        assert_eq!(threshold, 0.05);
        assert_eq!(control.cmd_alpha, 0.2);
        assert_eq!(control.hz, 50);
        assert_eq!(MAX_ACTION_US, 1_000_000);
        assert_eq!(REFRESH_LOSS_US, 200_000);
        assert_eq!(MAX_LEASE_US, 3_000_000);
        assert!(REFERENCE_FORWARD_MPS >= threshold * 1.5);
        assert!(REFERENCE_FORWARD_MPS < 0.1);
        assert_eq!(REFERENCE_TWIST, [REFERENCE_FORWARD_MPS, 0.0, 0.0]);
        let ticks = MAX_ACTION_US * u64::from(control.hz) / 1_000_000;
        let (mut old, mut reference, mut first_walk_tick, mut integral) = (0., 0., None, 0.);
        for tick in 1..=ticks {
            // Exercise the pinned daemon's actual smoother, not another EMA.
            crate::slew(&mut old, 0.05, control.cmd_alpha);
            crate::slew(&mut reference, REFERENCE_FORWARD_MPS, control.cmd_alpha);
            assert!(old <= threshold, "old payload must always select Stand");
            if reference > threshold {
                first_walk_tick.get_or_insert(tick);
            }
            integral += reference / f64::from(control.hz);
        }
        assert_eq!(first_walk_tick, Some(5));
        let crossing_us = first_walk_tick.unwrap() * 1_000_000 / u64::from(control.hz);
        assert_eq!(crossing_us, 100_000);
        assert!(MAX_ACTION_US - crossing_us >= 900_000);
        // Command integral only: PPO/measured displacement still needs the real gate.
        assert!((integral - 0.0736).abs() < 1e-6);
        assert!((0.01..=0.1).contains(&integral));
    }

    fn fixture() -> (Authority, Install, Action, Move, Fence) {
        let n = Authority::default();
        let identity = Identity {
            environment: "environment".into(),
            domain: "body-motion".into(),
            body: "body-generation".into(),
            body_ref: "body-ref".into(),
            world: "world-generation".into(),
            controller: "controller-generation".into(),
        };
        n.configure(identity.clone()).unwrap();
        let i = Install {
            protocol: PROTOCOL.into(),
            profile: PROFILE.into(),
            identity,
            domain: "body-motion".into(),
            session: "session-1".into(),
            epoch: 1,
            request: "install-1".into(),
            lease_deadline_us: 2_000_000,
        };
        let a = Action {
            install: i.clone(),
            action: "action-1".into(),
            payload_digest: "a".repeat(64),
            deadline_us: 1_000_000,
        };
        let m = Move {
            action: a.clone(),
            request: "move-1".into(),
            sequence: 1,
            twist: REFERENCE_TWIST,
        };
        let f = Fence {
            install: i.clone(),
            next_epoch: 2,
            request: "fence-1".into(),
        };
        (n, i, a, m, f)
    }
    fn installed(n: &Authority, i: &Install, a: &Action) {
        assert!(
            n.request(
                &Request::Install {
                    descriptor: i.clone()
                },
                1,
                10
            )
            .accepted
        );
        assert!(
            n.request(
                &Request::Admit {
                    descriptor: a.clone()
                },
                1,
                20
            )
            .accepted
        );
    }
    fn queued(n: &Authority, m: &Move) {
        assert!(
            n.request(
                &Request::Move {
                    descriptor: m.clone()
                },
                1,
                30
            )
            .accepted
        );
    }
    #[test]
    fn first_and_duplicate_exact_install_are_idempotent() {
        let (n, i, _, _, _) = fixture();
        let a = n.request(
            &Request::Install {
                descriptor: i.clone(),
            },
            1,
            10,
        );
        let b = n.request(
            &Request::Install {
                descriptor: i.clone(),
            },
            1,
            100,
        );
        assert!(a.accepted && b.accepted);
        assert_eq!(a.installed, b.installed);
        assert_eq!(b.installed.unwrap().lease_deadline_us, 2_000_000);
    }
    #[test]
    fn stale_epoch_foreign_connection_and_domain_are_rejected() {
        let (n, i, a, _, _) = fixture();
        installed(&n, &i, &a);
        let mut new = i.clone();
        new.epoch = 3;
        new.session = "session-2".into();
        assert!(
            !n.request(
                &Request::Install {
                    descriptor: new.clone()
                },
                2,
                40
            )
            .accepted
        );
        new.domain = "other-body".into();
        assert!(
            !n.request(&Request::Install { descriptor: new }, 1, 40)
                .accepted
        );
        let mut old = i;
        old.request = "changed".into();
        assert!(
            !n.request(&Request::Install { descriptor: old }, 1, 40)
                .accepted
        );
    }
    #[test]
    fn newer_same_owner_install_supersedes_old_commands() {
        let (n, i, a, m, f) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        let mut next = i.clone();
        next.epoch = 3;
        next.session = "session-2".into();
        assert!(
            n.request(
                &Request::Install {
                    descriptor: next.clone()
                },
                1,
                40
            )
            .accepted
        );
        assert!(!n.request(&Request::Move { descriptor: m }, 1, 50).accepted);
        assert!(!n.request(&Request::Fence { descriptor: f }, 1, 50).accepted);
        assert_eq!(
            n.request(
                &Request::Status {
                    protocol: PROTOCOL.into()
                },
                1,
                50
            )
            .installed,
            Some(next)
        );
    }
    #[test]
    fn wrong_identity_protocol_profile_and_bounds_reject_install() {
        for field in 0..8 {
            let (n, mut i, _, _, _) = fixture();
            match field {
                0 => i.identity.body = "replacement".into(),
                1 => i.identity.controller = "replacement".into(),
                2 => i.identity.world = "replacement".into(),
                3 => i.protocol = "v2".into(),
                4 => i.profile = "skills".into(),
                5 => i.lease_deadline_us = 4_000_000,
                6 => i.lease_deadline_us = 10,
                _ => i.identity.environment = "other".into(),
            }
            assert!(
                !n.request(&Request::Install { descriptor: i }, 1, 10)
                    .accepted
            );
        }
    }
    #[test]
    fn exact_action_refresh_keeps_original_deadline() {
        let (n, i, a, mut m, _) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        assert_eq!(n.consume().unwrap().twist(40), REFERENCE_TWIST);
        m.sequence = 2;
        m.request = "refresh".into();
        assert!(n.request(&Request::Move { descriptor: m }, 1, 50).accepted);
        assert_eq!(
            n.request(
                &Request::Status {
                    protocol: PROTOCOL.into()
                },
                1,
                60
            )
            .action,
            Some(a)
        );
    }
    #[test]
    fn changed_payload_action_session_epoch_deadline_and_sequence_reject() {
        for field in 0..8 {
            let (n, i, a, mut m, _) = fixture();
            installed(&n, &i, &a);
            queued(&n, &m);
            m.sequence = 2;
            match field {
                0 => m.twist = [0.1, 0.0, 0.0],
                1 => m.action.payload_digest = "b".repeat(64),
                2 => m.action.action = "foreign".into(),
                3 => m.action.install.session = "foreign".into(),
                4 => m.action.install.epoch = 2,
                5 => m.action.deadline_us += 1,
                6 => m.sequence = 1,
                _ => m.action.install.identity.controller = "old".into(),
            }
            assert!(!n.request(&Request::Move { descriptor: m }, 1, 50).accepted);
        }
    }
    #[test]
    fn exact_reference_payload_rejects_old_velocity_and_any_changed_component() {
        let x = REFERENCE_FORWARD_MPS;
        for twist in [
            [0.05, 0., 0.],
            [f64::from_bits(x.to_bits() - 1), 0., 0.],
            [f64::from_bits(x.to_bits() + 1), 0., 0.],
            [-x, 0., 0.],
            [x, f64::MIN_POSITIVE, 0.],
            [x, -f64::MIN_POSITIVE, 0.],
            [x, 0., f64::MIN_POSITIVE],
            [x, 0., -f64::MIN_POSITIVE],
            [f64::NAN, 0., 0.],
            [x, f64::NAN, 0.],
            [x, 0., f64::NAN],
            [f64::INFINITY, 0., 0.],
            [x, f64::INFINITY, 0.],
            [x, 0., f64::INFINITY],
        ] {
            let (n, i, a, mut m, _) = fixture();
            installed(&n, &i, &a);
            queued(&n, &m);
            m.twist = twist;
            m.sequence = 2;
            let r = n.request(&Request::Move { descriptor: m }, 1, 50);
            assert!(!r.accepted, "{twist:?}");
            assert_eq!(r.reason, "changed_payload_or_stale_sequence");
            assert!(r.fenced);
            assert_eq!(r.sequence, 1);
            assert_eq!(n.consume().unwrap().twist(60), [0.; 3]);
        }
    }
    #[test]
    fn no_move_before_native_action_admission() {
        let (n, i, _, m, _) = fixture();
        assert!(
            n.request(&Request::Install { descriptor: i }, 1, 10)
                .accepted
        );
        assert!(!n.request(&Request::Move { descriptor: m }, 1, 20).accepted);
    }
    #[test]
    fn action_and_session_expiry_reject_without_pastey() {
        for action in [true, false] {
            let (n, i, a, m, _) = fixture();
            installed(&n, &i, &a);
            queued(&n, &m);
            let now = if action {
                a.deadline_us
            } else {
                i.lease_deadline_us
            };
            assert_eq!(n.consume().unwrap().twist(now), [0.0; 3]);
            assert!(!n.request(&Request::Move { descriptor: m }, 1, now).accepted);
        }
    }
    #[test]
    fn refresh_loss_closes_and_cannot_resume_same_action() {
        let (n, i, a, mut m, _) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        assert_eq!(n.consume().unwrap().twist(30 + REFRESH_LOSS_US), [0.0; 3]);
        m.sequence += 1;
        assert!(
            !n.request(&Request::Move { descriptor: m }, 1, 30 + REFRESH_LOSS_US)
                .accepted
        );
    }
    #[test]
    fn received_command_fenced_before_consumption_never_reaches_controller() {
        let (n, i, a, m, f) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        assert!(n.request(&Request::Fence { descriptor: f }, 1, 40).accepted);
        let mut consume = n.consume().unwrap();
        assert_eq!(consume.twist(50), [0.0; 3]);
        assert_eq!(consume.consumed, 0);
    }
    #[test]
    fn fence_then_delayed_command_and_duplicate_fence_remain_closed() {
        let (n, i, a, m, f) = fixture();
        installed(&n, &i, &a);
        let old = n.request(
            &Request::Move {
                descriptor: m.clone(),
            },
            1,
            30,
        );
        assert!(old.accepted);
        for _ in 0..2 {
            assert!(
                n.request(
                    &Request::Fence {
                        descriptor: f.clone()
                    },
                    1,
                    40
                )
                .accepted
            );
        }
        assert!(!n.request(&Request::Move { descriptor: m }, 1, 50).accepted);
        // A delayed ACK is inert data: it cannot modify native state.
        assert!(old.accepted);
        assert_eq!(n.consume().unwrap().twist(50), [0.0; 3]);
    }
    #[test]
    fn fence_serializes_through_actual_apply_critical_section() {
        let (n, i, a, m, f) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        let n = std::sync::Arc::new(n);
        let mut apply = n.consume().unwrap();
        assert_eq!(apply.twist(40), REFERENCE_TWIST);
        let (start_tx, start_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let peer = n.clone();
        let thread = std::thread::spawn(move || {
            start_tx.send(()).unwrap();
            done_tx
                .send(peer.request(&Request::Fence { descriptor: f }, 1, 50))
                .unwrap();
        });
        start_rx.recv().unwrap();
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_millis(10))
                .is_err()
        );
        drop(apply);
        assert!(done_rx.recv().unwrap().accepted);
        thread.join().unwrap();
        assert_eq!(n.consume().unwrap().twist(60), [0.0; 3]);
    }
    #[test]
    fn disconnect_and_reconnect_cannot_resume_or_reinstall_old_epoch() {
        let (n, i, a, m, _) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        n.disconnect(1);
        assert_eq!(n.consume().unwrap().twist(40), [0.0; 3]);
        assert!(!n.request(&Request::Move { descriptor: m }, 2, 40).accepted);
        assert!(
            !n.request(&Request::Install { descriptor: i }, 2, 40)
                .accepted
        );
    }
    #[test]
    fn body_controller_io_loss_requires_fresh_launch() {
        let (n, mut i, a, m, _) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        n.native_loss("controller_replaced");
        i.epoch = 3;
        assert!(
            !n.request(&Request::Install { descriptor: i }, 1, 40)
                .accepted
        );
        assert_eq!(n.consume().unwrap().twist(50), [0.0; 3]);
    }
    #[test]
    fn restart_has_new_incarnation_and_no_buffer_or_authority() {
        let (n, i, a, m, _) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        let fresh = Authority::default();
        let mut identity = i.identity.clone();
        identity.controller = controller_incarnation().unwrap();
        fresh.configure(identity).unwrap();
        assert!(
            !fresh
                .request(&Request::Move { descriptor: m }, 1, 40)
                .accepted
        );
        assert!(
            !fresh
                .request(&Request::Install { descriptor: i }, 1, 40)
                .accepted
        );
        assert_eq!(fresh.consume().unwrap().twist(50), [0.0; 3]);
    }
    #[test]
    fn unknown_fields_and_variants_do_not_parse() {
        let (_, i, _, _, _) = fixture();
        let mut v = serde_json::to_value(Request::Install { descriptor: i }).unwrap();
        v["descriptor"]["nativePermit"] = serde_json::json!({});
        assert!(serde_json::from_value::<Request>(v).is_err());
        assert!(serde_json::from_str::<Request>("{\"kind\":\"toggle\"}").is_err());
    }
    #[test]
    fn fence_racing_refresh_always_leaves_old_action_closed() {
        let (n, i, a, mut m, f) = fixture();
        installed(&n, &i, &a);
        queued(&n, &m);
        m.sequence = 2;
        let n = std::sync::Arc::new(n);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let peer = n.clone();
        let b = barrier.clone();
        let refresh = std::thread::spawn(move || {
            b.wait();
            peer.request(&Request::Move { descriptor: m }, 1, 40)
        });
        let peer = n.clone();
        let b = barrier.clone();
        let fence = std::thread::spawn(move || {
            b.wait();
            peer.request(&Request::Fence { descriptor: f }, 1, 40)
        });
        barrier.wait();
        let _ = refresh.join().unwrap();
        assert!(fence.join().unwrap().accepted);
        assert_eq!(n.consume().unwrap().twist(50), [0.0; 3]);
    }
    #[test]
    fn partial_install_and_fence_never_mutate_authority() {
        let (n, i, a, m, f) = fixture();
        let encoded = serde_json::to_vec(&Request::Install {
            descriptor: i.clone(),
        })
        .unwrap();
        assert!(serde_json::from_slice::<Request>(&encoded[..encoded.len() - 1]).is_err());
        assert!(
            n.request(
                &Request::Status {
                    protocol: PROTOCOL.into()
                },
                1,
                10
            )
            .installed
            .is_none()
        );
        installed(&n, &i, &a);
        queued(&n, &m);
        let encoded = serde_json::to_vec(&Request::Fence { descriptor: f }).unwrap();
        assert!(serde_json::from_slice::<Request>(&encoded[..encoded.len() - 1]).is_err());
        assert_eq!(n.consume().unwrap().twist(40), REFERENCE_TWIST);
        // Transport loss while a partial request is buffered closes authority.
        n.disconnect(1);
        assert_eq!(n.consume().unwrap().twist(50), [0.0; 3]);
    }
}
