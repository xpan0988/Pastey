//! Pure checks and opt-in real patched robotd + FakeIo integration. Synthetic
//! enrollment/qualification in this fixture never becomes a release record.
use super::*;
use crate::physical::{core::gate_b, native_protocol as wire};
use std::collections::BTreeMap;

#[test]
fn native_protocol_has_no_live_authority_and_rejects_unknown_data() {
    let value = serde_json::json!({"kind":"status","protocol":wire::PROTOCOL,"root":{}});
    assert!(serde_json::from_value::<wire::Request>(value).is_err());
    assert!(serde_json::from_str::<wire::Request>("{\"kind\":\"restore_session\"}").is_err());
    assert!(!wire::bounded(&"x".repeat(161)));
}

#[cfg(unix)]
struct NativeDaemon {
    child: std::process::Child,
    directory: PathBuf,
    binding: EnvironmentBindingViewV1,
}
#[cfg(unix)]
impl NativeDaemon {
    fn launch() -> Self {
        let binary = std::env::var_os("PASTEY_GATE_B_ROBOTD")
            .expect("set PASTEY_GATE_B_ROBOTD to the patched pinned binary");
        let directory = std::env::temp_dir().join(format!("pgb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let mut binding = binding();
        let s = binding.subsystems.values().next().unwrap();
        let launch = wire::LaunchIdentity {
            environment: String::from(binding.environment.clone()),
            domain: String::from(s.domains[0].clone()),
            body: String::from(s.body_incarnation.clone()),
            body_ref: String::from(s.body.clone()),
            world: String::from(s.world_incarnation.clone().unwrap()),
        };
        let identity = directory.join("identity.json");
        std::fs::write(&identity, serde_json::to_vec(&launch).unwrap()).unwrap();
        let child = std::process::Command::new(binary)
            .args(["--fake", "--no-policy", "--socket"])
            .arg(directory.join("robot.sock"))
            .arg("--pastey-task-identity")
            .arg(identity)
            .env("DUCK_RUNTIME_ROOT", &directory)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut daemon = Self {
            child,
            directory: directory.clone(),
            binding: binding.clone(),
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !directory.join("robot.sock").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let socket = std::os::unix::net::UnixStream::connect(directory.join("robot.sock")).unwrap();
        let mut socket = std::io::BufReader::new(socket);
        use std::io::{BufRead, Write};
        let request = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"robot.task","params":{"kind":"status","protocol":wire::PROTOCOL}});
        writeln!(socket.get_mut(), "{request}").unwrap();
        let mut line = String::new();
        socket.read_line(&mut line).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        let receipt: wire::Receipt = serde_json::from_value(reply["result"].clone()).unwrap();
        binding
            .subsystems
            .values_mut()
            .next()
            .unwrap()
            .controller_incarnation = IncarnationId::try_from(receipt.identity.controller).unwrap();
        daemon.binding = binding;
        daemon
    }
    fn lane(&self, fixture: &ControlFixture) -> Arc<gate_b::GateBNativeLaneV1> {
        gate_b::GateBNativeLaneV1::connect_owned(
            std::os::unix::net::UnixStream::connect(self.directory.join("robot.sock")).unwrap(),
            &fixture.live,
            fixture.clock.clone(),
        )
        .unwrap()
    }
}
#[cfg(unix)]
impl Drop for NativeDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires the explicitly built pinned patched robotd; uses FakeIo, no MuJoCo/hardware qualification"]
async fn real_robotd_local_install_action_fence_and_restart_history() {
    let daemon = NativeDaemon::launch();
    let f = ControlFixture::configured_binding(true, daemon.binding.clone());
    let native = daemon.lane(&f);
    let adapter = microduck::MicroDuckAdapterV1::native_fence(native);
    let session = f.reserve();
    PhysicalControlServiceV1::install_control_session(&f.core, &session, &adapter)
        .await
        .unwrap();
    let (g, p) = f.challenged(&session);
    let action = f.admit(&g, p);
    PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &action, &adapter)
        .await
        .unwrap();
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_native_receipts WHERE kind='install'"),
        1
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_native_receipts WHERE kind='command'"),
        1
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_task_acceptance WHERE state='accepted'"),
        0,
        "native ACK cannot become L7 acceptance"
    );
    assert!(
        PhysicalControlServiceV1::dispatch_admitted_action(&f.core, &action, &adapter)
            .await
            .is_err()
    );
    assert!(
        PhysicalControlServiceV1::revoke_control_session(&f.core, &session, &adapter)
            .await
            .unwrap()
    );
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_native_receipts WHERE kind='fence'"),
        1
    );
    assert_eq!(f.session_state(), "quarantined");
    let mut restarted = PhysicalControlServiceV1::new(
        &f.paths,
        LocalRuntimeRef::fresh(host("executor")),
        f.clock.clone(),
    )
    .unwrap();
    assert!(lane::validate_session(&mut restarted, &session, true).is_err());
    assert!(lane::validate_session(&mut restarted, &session, false).is_err());
    assert_eq!(f.scalar("SELECT count(*) FROM physical_native_receipts"), 3);
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires the explicitly built pinned patched robotd; synthetic two-Host enrollment, real native FakeIo boundary"]
async fn real_robotd_remote_uses_the_same_executor_native_lane() {
    let daemon = NativeDaemon::launch();
    let fixture = ControlFixture::configured_binding(true, daemon.binding.clone());
    let native = daemon.lane(&fixture);
    let adapter = Arc::new(microduck::MicroDuckAdapterV1::native_fence(native));
    let mut pair = Pair::with_executor(fixture);
    let ingress = pair.b.core.lock().local_ingress().unwrap();
    pair.b
        .core
        .lock()
        .attach_product_environment(
            &ingress,
            ProductEnvironmentV1 {
                binding: pair.b.live.clone(),
                adapter: adapter.clone(),
                run: None,
            },
        )
        .unwrap();
    let (start, session) = pair.start().await;
    let (g, p) = pair.b.challenged(&session);
    let action = pair.b.admit(&g, p);
    PhysicalControlServiceV1::dispatch_admitted_action(&pair.b.core, &action, adapter.as_ref())
        .await
        .unwrap();
    let (_, message) = pair
        .a
        .physical_product(
            &pair.ab,
            PhysicalProductRequestV1::Cancel {
                start: start.semantic_id,
            },
        )
        .unwrap();
    let (reply, work) = pair.deliver_b(message.unwrap()).unwrap();
    PhysicalControlServiceV1::perform_physical_work(&pair.b.core, work.unwrap())
        .await
        .unwrap();
    pair.deliver_a(reply.unwrap());
    assert_eq!(
        pair.b
            .scalar("SELECT count(*) FROM physical_attempts WHERE role='executor_remote'"),
        1
    );
    assert_eq!(pair.b.scalar("SELECT count(*) FROM physical_sessions WHERE install_evidence='native_fence' AND fence_ack='native_fence'"),1);
    assert_eq!(
        pair.b
            .scalar("SELECT count(*) FROM physical_native_receipts WHERE kind='command'"),
        1
    );
    assert_eq!(
        pair.b
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert!(PhysicalControlServiceV1::dispatch_admitted_action(
        &pair.b.core,
        &action,
        adapter.as_ref()
    )
    .await
    .is_err());
}

#[test]
fn historical_native_receipts_require_exact_inner_and_outer_correlation() {
    let b = binding();
    let body = b.subsystems.values().next().unwrap();
    let identity = wire::Identity {
        environment: String::from(b.environment.clone()),
        domain: String::from(body.domains[0].clone()),
        body: String::from(body.body_incarnation.clone()),
        body_ref: String::from(body.body.clone()),
        world: String::from(body.world_incarnation.clone().unwrap()),
        controller: String::from(body.controller_incarnation.clone()),
    };
    let session = SessionId::try_from(id("physical-session")).unwrap();
    let request = RequestId::try_from(id("physical-request")).unwrap();
    let epochs = BTreeMap::from([(body.domains[0].clone(), 1)]);
    let install = wire::Install {
        protocol: wire::PROTOCOL.into(),
        profile: wire::PROFILE.into(),
        identity: identity.clone(),
        domain: identity.domain.clone(),
        session: String::from(session.clone()),
        epoch: 1,
        request: String::from(request.clone()),
        lease_deadline_us: 1000,
    };
    let receipt = wire::Receipt {
        protocol: wire::PROTOCOL.into(),
        profile: wire::PROFILE.into(),
        identity,
        native_us: 10,
        request: String::from(request.clone()),
        accepted: true,
        reason: "installed".into(),
        installed: Some(install),
        action: None,
        sequence: 0,
        high_water_epoch: 1,
        fenced: false,
        consumed_sequence: 0,
    };
    let check = |r: &wire::Receipt| {
        gate_b::validate_historical_receipt(r, &b, &session, &epochs, &request, false)
    };
    check(&receipt).unwrap();
    for field in 0..7 {
        let mut bad = receipt.clone();
        match field {
            0 => bad.installed = None,
            1 => bad.request = "other".into(),
            2 => bad.installed.as_mut().unwrap().identity.controller = "old".into(),
            3 => bad.installed.as_mut().unwrap().profile = "other".into(),
            4 => bad.high_water_epoch = 2,
            5 => bad.fenced = true,
            _ => bad.accepted = false,
        }
        assert!(check(&bad).is_err());
    }
}

#[tokio::test]
async fn exact_stage7_migration_preserves_remote_lineage_and_consumed_budget() {
    let mut pair = Pair::new();
    let (_, session) = pair.start().await;
    let (grant, proposal) = pair.b.challenged(&session);
    let action = pair.b.admit(&grant, proposal);
    PhysicalControlServiceV1::dispatch_admitted_action(
        &pair.b.core,
        &action,
        &FakeLane::new(vec![Reply::Success]),
    )
    .await
    .unwrap();
    pair.b.core.lock().close().unwrap();
    let audits: Vec<(String, String)> = pair
        .b
        .sql()
        .prepare("SELECT session_id,audit_json FROM physical_sessions ORDER BY session_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let semantic = pair
        .b
        .scalar("SELECT count(*) FROM physical_semantic_messages");
    crate::physical::store::test_restore_stage7_schema(&pair.b.paths).unwrap();
    storage::init_database(&pair.b.paths).unwrap();
    let _restarted = PhysicalControlServiceV1::new(
        &pair.b.paths,
        LocalRuntimeRef::fresh(host("executor")),
        pair.b.clock.clone(),
    )
    .unwrap();
    let migrated: Vec<(String, String)> = pair
        .b
        .sql()
        .prepare("SELECT session_id,audit_json FROM physical_sessions ORDER BY session_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(audits, migrated);
    assert_eq!(
        semantic,
        pair.b
            .scalar("SELECT count(*) FROM physical_semantic_messages")
    );
    assert_eq!(
        pair.b
            .scalar("SELECT consumed_us FROM physical_control_budgets"),
        1_000_000
    );
    assert_eq!(
        pair.b
            .scalar("SELECT count(*) FROM physical_native_receipts"),
        0
    );
    assert_eq!(
        pair.b.scalar("SELECT version FROM physical_native_schema"),
        1
    );
}
