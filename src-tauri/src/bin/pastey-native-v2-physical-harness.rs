#![allow(dead_code)]

//! Physical Mac requester <-> Windows Host acceptance driver for native-v2.
//!
//! This is deliberately a headless adapter over the production HostRuntime and
//! its normal Bridge lifecycle. It neither supplies authority nor emulates a
//! peer: each process owns one real Host and uses the ordinary Room Control and
//! encrypted Transfer paths. Its report reader is read-only and derives PASS
//! only after it combines the requester Core database with the Windows Host
//! database.

#[path = "../bridge_lifecycle.rs"]
mod bridge_lifecycle;
#[path = "../bridge_plan.rs"]
mod bridge_plan;
#[path = "../bridge_plan_v2.rs"]
mod bridge_plan_v2;
#[path = "../capability_probe.rs"]
mod capability_probe;
#[path = "../chunk_frame.rs"]
mod chunk_frame;
#[path = "../cleanup.rs"]
mod cleanup;
#[path = "../commands.rs"]
mod commands;
#[path = "../config.rs"]
mod config;
#[path = "../crypto.rs"]
mod crypto;
#[path = "../dev_tools.rs"]
mod dev_tools;
#[path = "../developer_terminal.rs"]
mod developer_terminal;
#[path = "../device_profile.rs"]
mod device_profile;
#[path = "../diagnostics.rs"]
mod diagnostics;
#[path = "../discovery.rs"]
mod discovery;
#[path = "../effect_authority.rs"]
mod effect_authority;
#[path = "../error.rs"]
mod error;
#[path = "../execution_backend.rs"]
mod execution_backend;
#[path = "../execution_world.rs"]
mod execution_world;
#[path = "../file_candidates.rs"]
mod file_candidates;
#[path = "../host_admission.rs"]
mod host_admission;
#[path = "../host_identity.rs"]
mod host_identity;
#[path = "../host_runtime.rs"]
mod host_runtime;
#[path = "../link_benchmark.rs"]
mod link_benchmark;
#[path = "../logging.rs"]
mod logging;
#[path = "../managed_execution.rs"]
mod managed_execution;
#[path = "../managed_objects.rs"]
mod managed_objects;
#[path = "../managed_resources.rs"]
mod managed_resources;
#[path = "../managed_worker_coordinator.rs"]
mod managed_worker_coordinator;
#[path = "../managed_workspace.rs"]
mod managed_workspace;
#[path = "../models.rs"]
mod models;
#[path = "../native_v2_orchestration.rs"]
mod native_v2_orchestration;
#[path = "../natural_v2.rs"]
mod natural_v2;
#[path = "../network_broker.rs"]
mod network_broker;
#[path = "../object_refs.rs"]
mod object_refs;
#[path = "../peer_capabilities.rs"]
mod peer_capabilities;
#[path = "../room_control.rs"]
mod room_control;
#[path = "../safe_file_identity.rs"]
mod safe_file_identity;
#[path = "../storage.rs"]
mod storage;
#[path = "../transfer.rs"]
mod transfer;
#[path = "../transfer_orchestration.rs"]
mod transfer_orchestration;
#[path = "../transfer_tuning.rs"]
mod transfer_tuning;
#[cfg(windows)]
#[path = "../windows_codex_backend.rs"]
mod windows_codex_backend;
#[cfg(any(windows, test))]
#[path = "../windows_verifier_diagnostics.rs"]
mod windows_verifier_diagnostics;
#[path = "../worker_harness.rs"]
mod worker_harness;
#[path = "../worker_provider.rs"]
mod worker_provider;
#[path = "../worker_provider_config.rs"]
mod worker_provider_config;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use rusqlite::{types::ValueRef, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Map, Value};
use sha2::{Digest as _, Sha256};

use crate::{
    error::{AppError, AppResult},
    host_runtime::{HostEvent, HostEventSink, HostRuntime, RuntimeTask, RuntimeTaskSpawner},
    native_v2_orchestration::{
        NativeV2ComposeRequestV1, NativeV2ObjectRevisionDtoV1, NativeV2ProductStateV1,
        NativeV2StepDraftV1,
    },
    storage::AppPaths,
};

const USAGE: &str = "usage:\n  pastey-native-v2-physical-harness host --app-data-dir DIR --bridge-id ID\n  pastey-native-v2-physical-harness run --profile a|b --app-data-dir DIR --bridge-id ID --remote-host-ref REF --run-id ID --report-dir DIR [--product-executable PATH]\n  pastey-native-v2-physical-harness collect --profile a|b --role requester|windows-host --app-data-dir DIR --attempt-id ID --report-dir DIR [--product-executable PATH]\n  pastey-native-v2-physical-harness verify --profile a|b --requester-report FILE --windows-report FILE --output-dir DIR";
const EVIDENCE_SCHEMA: &str = "pastey-native-v2-physical-evidence-v1";
const HOST_LAUNCH_IDENTITY_SCHEMA: &str = "pastey-native-v2-physical-host-launch-v1";
const HOST_LAUNCH_IDENTITY_FILE: &str = "native-v2-physical-host-launch.json";

struct SilentEvents;
impl HostEventSink for SilentEvents {
    fn emit(&self, _event: HostEvent) -> AppResult<()> {
        Ok(())
    }
}

struct TokioSpawner;
impl RuntimeTaskSpawner for TokioSpawner {
    fn spawn(&self, task: RuntimeTask) {
        tokio::spawn(task);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Profile {
    A,
    B,
}

impl Profile {
    fn parse(value: &str) -> AppResult<Self> {
        match value {
            "a" | "A" => Ok(Self::A),
            "b" | "B" => Ok(Self::B),
            _ => Err(invalid("profile must be a or b")),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
    fn expected_steps(self) -> i64 {
        match self {
            Self::A => 2,
            Self::B => 3,
        }
    }
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}

fn args() -> AppResult<(String, BTreeMap<String, String>)> {
    let mut values = std::env::args().skip(1);
    let command = values.next().ok_or_else(|| invalid(USAGE))?;
    let mut out = BTreeMap::new();
    while let Some(key) = values.next() {
        if !key.starts_with("--") {
            return Err(invalid(USAGE));
        }
        let value = values.next().ok_or_else(|| invalid(USAGE))?;
        out.insert(key.trim_start_matches("--").to_string(), value);
    }
    Ok((command, out))
}

fn required<'a>(args: &'a BTreeMap<String, String>, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(&format!("missing --{key}")))
}

fn paths(app_data_dir: &str) -> AppResult<AppPaths> {
    let root = PathBuf::from(app_data_dir);
    if !root.is_absolute() {
        return Err(invalid("--app-data-dir must be absolute"));
    }
    let paths = AppPaths::new(root.clone(), root.join("logs"));
    paths.ensure_directories()?;
    Ok(paths)
}

async fn start_host(paths: AppPaths) -> AppResult<Arc<HostRuntime>> {
    let runtime = HostRuntime::initialize(
        paths,
        "CommandOrControl+Shift+V",
        Arc::new(SilentEvents),
        Arc::new(TokioSpawner),
    )?;
    let lifecycle = runtime.clone();
    runtime.spawn(async move {
        bridge_lifecycle::start(lifecycle).await;
    });
    // bootstrap_room_servers runs at the beginning of lifecycle startup.
    tokio::time::sleep(Duration::from_millis(350)).await;
    Ok(runtime)
}

async fn wait_for_exact_connected_peer(
    runtime: &HostRuntime,
    bridge_id: &str,
    remote_host_ref: &str,
) -> AppResult<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let matches = storage::list_bridge_peer_endpoints(&runtime.paths, bridge_id)?
            .into_iter()
            .filter(|peer| {
                peer.logical_host_ref.as_deref() == Some(remote_host_ref)
                    && peer.liveness == models::BridgePeerLiveness::Connected
            })
            .count();
        if matches == 1 {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(invalid(
                "the exact remote HostRef did not establish one current connected Bridge session",
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn run_token(value: &str) -> AppResult<String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(invalid(
            "--run-id must be 1-64 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(value.into())
}

fn request(
    profile: Profile,
    runtime: &HostRuntime,
    bridge_id: &str,
    remote: &str,
    run_id: &str,
) -> AppResult<NativeV2ComposeRequestV1> {
    let requester = runtime.local_host_ref.as_str().to_string();
    if requester == remote {
        return Err(invalid(
            "--remote-host-ref must name the other physical Host",
        ));
    }
    let object = NativeV2ObjectRevisionDtoV1 {
        logical_object_id: format!("physical-native-v2-{run_id}"),
        revision: 1,
    };
    let mut steps = vec![
        NativeV2StepDraftV1::Search {
            step_id: "search-mac".into(),
            depends_on: vec![],
            host_ref: requester.clone(),
            output: object.clone(),
            query: format!("pastey-native-v2-{run_id}.txt"),
            safe_scope_labels: vec!["pastey_shared".into()],
        },
        NativeV2StepDraftV1::Transfer {
            step_id: "transfer-mac-windows".into(),
            depends_on: vec!["search-mac".into()],
            source_host_ref: requester.clone(),
            destination_host_ref: remote.into(),
            input: object.clone(),
            output: object.clone(),
        },
    ];
    if profile == Profile::B {
        steps.push(NativeV2StepDraftV1::Execute {
            step_id: "execute-windows".into(),
            depends_on: vec!["transfer-mac-windows".into()],
            host_ref: remote.into(),
            target: object.clone(),
            execution_intent: "Return the deterministic physical-acceptance result digest.".into(),
        });
    }
    Ok(NativeV2ComposeRequestV1 {
        plan_id: format!("physical-native-v2-plan-{run_id}"),
        revision_id: format!("physical-native-v2-revision-{run_id}"),
        revision_number: 1,
        bridge_id: bridge_id.into(),
        requester_host_ref: requester.clone(),
        participant_host_refs: vec![requester, remote.into()],
        roots: vec![],
        original_user_goal:
            "Physical native-v2 acceptance: exact Search then explicit Transfer to Windows.".into(),
        expected_outcome: if profile == Profile::A {
            "One exact Search result is received by Windows and Core completes.".into()
        } else {
            "Windows Execute records one result digest without a successor managed object.".into()
        },
        steps,
    })
}

fn prepare_search_fixture(paths: &AppPaths, run_id: &str) -> AppResult<()> {
    let shared = paths.app_data_dir.join("shared");
    fs::create_dir_all(&shared)?;
    let file = shared.join(format!("pastey-native-v2-{run_id}.txt"));
    if file.exists() {
        return Err(invalid(
            "physical fixture already exists; choose a fresh --run-id",
        ));
    }
    fs::write(
        file,
        format!("pastey native-v2 physical fixture {run_id}\n"),
    )?;
    Ok(())
}

async fn run_requester(profile: Profile, args: &BTreeMap<String, String>) -> AppResult<()> {
    if std::env::consts::OS != "macos" {
        return Err(invalid(
            "run is the macOS requester HostRuntime for this physical harness",
        ));
    }
    let launch_identity = git_identity();
    let paths = paths(required(args, "app-data-dir")?)?;
    let bridge_id = required(args, "bridge-id")?;
    let remote = required(args, "remote-host-ref")?;
    let run_id = run_token(required(args, "run-id")?)?;
    let report_dir = PathBuf::from(required(args, "report-dir")?);
    let runtime = start_host(paths.clone()).await?;
    wait_for_exact_connected_peer(&runtime, bridge_id, remote).await?;
    prepare_search_fixture(&paths, &run_id)?;
    let request = request(profile, &runtime, bridge_id, remote, &run_id)?;
    let revision_id = request.revision_id.clone();
    let approval_id = format!("physical-native-v2-approval-{run_id}");
    let attempt_id = format!("physical-native-v2-attempt-{run_id}");
    runtime.compose_native_v2_product_plan(request, storage::now_ts())?;
    runtime.approve_native_v2_product_plan(
        &revision_id,
        &approval_id,
        storage::now_ts() + 900,
        storage::now_ts(),
    )?;
    runtime
        .start_native_v2_product_attempt(
            &approval_id,
            &attempt_id,
            storage::now_ts() + 900,
            storage::now_ts(),
        )
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let terminal = loop {
        let status = runtime.native_v2_product_status(&revision_id)?;
        if matches!(
            status.state,
            NativeV2ProductStateV1::Completed
                | NativeV2ProductStateV1::Failed
                | NativeV2ProductStateV1::Interrupted
                | NativeV2ProductStateV1::Cancelled
        ) {
            break status;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = runtime
                .cancel_native_v2_product_attempt(&attempt_id, storage::now_ts())
                .await;
            return Err(invalid(
                "physical attempt did not reach a terminal Core state within 90 seconds",
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    write_report(
        profile,
        "requester",
        &paths,
        &attempt_id,
        &report_dir,
        args.get("product-executable").map(String::as_str),
        &launch_identity,
    )?;
    runtime.shutdown_all();
    if terminal.state == NativeV2ProductStateV1::Completed {
        Ok(())
    } else {
        Err(invalid(
            "physical attempt reached a non-completed Core terminal state; evidence was emitted",
        ))
    }
}

fn database(paths: &AppPaths) -> AppResult<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Ok(Connection::open_with_flags(&paths.db_path, flags)?)
}

fn column(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => json!(v),
        ValueRef::Real(v) => json!(v),
        ValueRef::Text(v) => json!(String::from_utf8_lossy(v).to_string()),
        ValueRef::Blob(_) => json!("[redacted-binary]"),
    }
}

fn rows(conn: &Connection, query: &str, values: &[&str]) -> AppResult<Vec<Value>> {
    let mut statement = conn.prepare(query)?;
    let names = statement
        .column_names()
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    let result = statement
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            let mut object = Map::new();
            for (index, name) in names.iter().enumerate() {
                object.insert(name.clone(), column(row.get_ref(index)?));
            }
            Ok(Value::Object(object))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

fn scalar_i64(conn: &Connection, query: &str, values: &[&str]) -> AppResult<i64> {
    Ok(
        conn.query_row(query, rusqlite::params_from_iter(values.iter()), |row| {
            row.get(0)
        })?,
    )
}

fn scalar_text(conn: &Connection, query: &str, values: &[&str]) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(query, rusqlite::params_from_iter(values.iter()), |row| {
            row.get(0)
        })
        .optional()?)
}

fn git_identity() -> Value {
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|commit| !commit.is_empty())
        .unwrap_or_else(|| "unavailable".into());
    let worktree = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            if out.stdout.is_empty() {
                "clean"
            } else {
                "dirty"
            }
        })
        .unwrap_or("unavailable");
    json!({"commit": commit, "worktree": worktree, "capturedAt": storage::now_ts()})
}

fn host_launch_identity_path(paths: &AppPaths) -> PathBuf {
    paths.app_data_dir.join(HOST_LAUNCH_IDENTITY_FILE)
}

fn write_host_launch_identity(paths: &AppPaths, git: &Value) -> AppResult<()> {
    let record = json!({
        "schemaVersion": HOST_LAUNCH_IDENTITY_SCHEMA,
        "role": "windows-host",
        "machine": {"os": std::env::consts::OS},
        "git": git,
    });
    fs::write(
        host_launch_identity_path(paths),
        serde_json::to_vec_pretty(&record)?,
    )?;
    Ok(())
}

fn read_host_launch_identity(paths: &AppPaths) -> AppResult<Value> {
    let record: Value = serde_json::from_slice(&fs::read(host_launch_identity_path(paths))?)?;
    if record["schemaVersion"] != HOST_LAUNCH_IDENTITY_SCHEMA
        || record["role"] != "windows-host"
        || record["machine"]["os"] != "windows"
    {
        return Err(invalid(
            "Windows Host launch identity is absent or does not describe this physical harness",
        ));
    }
    Ok(record["git"].clone())
}

fn product_identity(path: Option<&str>) -> Value {
    let Some(path) = path else {
        return json!({"status":"not_supplied"});
    };
    match fs::read(path) {
        Ok(bytes) => {
            json!({"status":"observed", "path": Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("product"), "bytes": bytes.len(), "sha256": format!("{:x}", Sha256::digest(&bytes))})
        }
        Err(_) => json!({"status":"unavailable"}),
    }
}

fn write_report(
    profile: Profile,
    role: &str,
    paths: &AppPaths,
    attempt_id: &str,
    report_dir: &Path,
    product: Option<&str>,
    launch_identity: &Value,
) -> AppResult<PathBuf> {
    let conn = database(paths)?;
    let config: Value = serde_json::from_slice(&fs::read(&paths.config_path)?)?;
    let revision_id = scalar_text(&conn, "SELECT revision_id FROM native_v2_product_attempts WHERE attempt_id = ?1 UNION SELECT revision_id FROM native_v2_receiver_attempts WHERE attempt_id = ?1 LIMIT 1", &[attempt_id])?.ok_or_else(|| invalid("attempt is absent from authoritative native-v2 tables"))?;
    let revision_hash = scalar_text(&conn, "SELECT revision_hash FROM native_v2_product_attempts WHERE attempt_id = ?1 UNION SELECT revision_hash FROM native_v2_receiver_attempts WHERE attempt_id = ?1 LIMIT 1", &[attempt_id])?.unwrap_or_default();
    let plan_state = scalar_text(
        &conn,
        "SELECT state FROM native_v2_product_attempts WHERE attempt_id = ?1",
        &[attempt_id],
    )?;
    let report = json!({
        "schemaVersion": EVIDENCE_SCHEMA,
        "profile": profile.label(), "role": role,
        "machine": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH, "git": launch_identity, "harnessVersion": env!("CARGO_PKG_VERSION"), "product": product_identity(product)},
        "identity": {"localHostRef": host_identity::HostRef::from_device_id(config.get("deviceId").and_then(Value::as_str).unwrap_or_default()).map(|host| host.as_str().to_string()).unwrap_or_else(|_| "unavailable".into()), "bridgePeers": rows(&conn, "SELECT room_id AS bridgeId, peer_session_id AS peerSessionId, logical_host_ref AS hostRef, liveness FROM bridge_peers WHERE liveness = 'connected' ORDER BY room_id, peer_session_id", &[])?},
        "attempt": {"attemptId": attempt_id, "revisionId": revision_id, "revisionHash": revision_hash, "finalPlanState": plan_state, "productRecords": rows(&conn, "SELECT attempt_id AS attemptId, approval_id AS approvalId, revision_id AS revisionId, revision_hash AS revisionHash, state, failure_code AS failureCode, expires_at AS expiresAt FROM native_v2_product_attempts WHERE attempt_id = ?1", &[attempt_id])?, "receiverRecords": rows(&conn, "SELECT attempt_id AS attemptId, revision_id AS revisionId, revision_hash AS revisionHash, approval_id AS approvalId, requester_participant_ref AS requesterParticipantRef, target_participant_ref AS targetParticipantRef, session_binding_ref AS sessionBindingRef, state, failure_code AS failureCode FROM native_v2_receiver_attempts WHERE attempt_id = ?1", &[attempt_id])?},
        "readinessAndAdmission": rows(&conn, "SELECT participant_ref AS participantRef, host_ref AS hostRef, peer_route_ref AS peerRouteRef, session_binding_ref AS sessionBindingRef, readiness_state AS readinessState, admission_state AS admissionState, readiness_code AS readinessCode, admission_ref AS admissionRef FROM native_v2_product_hosts WHERE attempt_id = ?1 ORDER BY participant_ref", &[attempt_id])?,
        "steps": rows(&conn, "SELECT step_id AS stepId, operation, state, completion_ref AS completionRef FROM native_v2_product_steps WHERE attempt_id = ?1 ORDER BY rowid", &[attempt_id])?,
        "stepCommits": rows(&conn, "SELECT step_id AS stepId, operation, host_ref AS hostRef, completion_ref AS completionRef, state, committed_at AS committedAt FROM native_v2_step_commits WHERE attempt_id = ?1 ORDER BY step_id", &[attempt_id])?,
        "externalDispatches": rows(&conn, "SELECT step_id AS stepId, operation, state, transfer_id AS transferId, failure_code AS failureCode FROM native_v2_external_dispatches WHERE attempt_id = ?1 ORDER BY step_id", &[attempt_id])?,
        "transferReceipts": rows(&conn, "SELECT step_id AS stepId, revision_id AS revisionId, revision_hash AS revisionHash, logical_object_id AS logicalObjectId, object_revision AS objectRevision, content_digest AS contentDigest, destination_host_ref AS destinationHostRef, binding_ref AS bindingRef, received_at AS receivedAt FROM native_v2_transfer_receipts WHERE attempt_id = ?1", &[attempt_id])?,
        "managed": {"claims": rows(&conn, "SELECT step_id AS stepId, operation, state, evidence_head AS evidenceHead FROM bridge_plan_v2_managed_step_claims WHERE attempt_id = ?1", &[attempt_id])?, "executeResults": rows(&conn, "SELECT step_id AS stepId, result_ref AS resultRef, host_ref AS hostRef, input_revision AS inputRevision, result_digest AS resultDigest, evidence_head AS evidenceHead FROM bridge_plan_v2_execute_results WHERE attempt_id = ?1", &[attempt_id])?, "successorLineageCount": scalar_i64(&conn, "SELECT COUNT(*) FROM bridge_plan_v2_transform_results WHERE attempt_id = ?1", &[attempt_id])?},
        "counts": {"stepCommitCount": scalar_i64(&conn, "SELECT COUNT(*) FROM native_v2_step_commits WHERE attempt_id = ?1", &[attempt_id])?, "uniqueCommittedSteps": scalar_i64(&conn, "SELECT COUNT(DISTINCT step_id) FROM native_v2_step_commits WHERE attempt_id = ?1", &[attempt_id])?, "externalDispatchCount": scalar_i64(&conn, "SELECT COUNT(*) FROM native_v2_external_dispatches WHERE attempt_id = ?1", &[attempt_id])?, "uniqueExternalDispatches": scalar_i64(&conn, "SELECT COUNT(DISTINCT step_id) FROM native_v2_external_dispatches WHERE attempt_id = ?1", &[attempt_id])?, "receiptCount": scalar_i64(&conn, "SELECT COUNT(*) FROM native_v2_transfer_receipts WHERE attempt_id = ?1", &[attempt_id])?, "integrity": scalar_text(&conn, "PRAGMA integrity_check", &[])?},
        "note": "This is an authoritative SQLite snapshot. Individual snapshots never claim PASS; verify combines requester Core completion and Windows receipt/effect evidence."
    });
    fs::create_dir_all(report_dir)?;
    let json_path = report_dir.join(format!("native-v2-physical-{}-{}.json", role, attempt_id));
    fs::write(&json_path, serde_json::to_vec_pretty(&report)?)?;
    let text_path = report_dir.join(format!("native-v2-physical-{}-{}.txt", role, attempt_id));
    fs::write(text_path, format!("Native-v2 physical evidence\nprofile: {}\nrole: {}\nos: {}\ngit commit: {}\ngit worktree at launch: {}\nattempt: {}\nrevision: {}\nrequester state: {}\ncommits: {}\nreceipts: {}\nSQLite integrity: {}\nJSON: {}\n", profile.label(), role, report["machine"]["os"], report["machine"]["git"]["commit"], report["machine"]["git"]["worktree"], attempt_id, report["attempt"]["revisionId"], report["attempt"]["finalPlanState"], report["counts"]["stepCommitCount"], report["counts"]["receiptCount"], report["counts"]["integrity"], json_path.display()))?;
    println!("EVIDENCE_JSON={}", json_path.display());
    Ok(json_path)
}

fn same(value: &Value, path: &[&str], expected: &Value) -> bool {
    path.iter().fold(value, |current, key| &current[*key]) == expected
}

fn nonempty_string(value: &Value) -> bool {
    value.as_str().is_some_and(|text| !text.trim().is_empty())
}

fn clean_source_identity(report: &Value) -> bool {
    nonempty_string(&report["machine"]["git"]["commit"])
        && report["machine"]["git"]["commit"] != "unavailable"
        && report["machine"]["git"]["worktree"] == "clean"
}

fn verify(profile: Profile, args: &BTreeMap<String, String>) -> AppResult<()> {
    let requester: Value = serde_json::from_slice(&fs::read(required(args, "requester-report")?)?)?;
    let windows: Value = serde_json::from_slice(&fs::read(required(args, "windows-report")?)?)?;
    let mut failures = Vec::new();
    let expected = profile.expected_steps();
    if requester["schemaVersion"] != EVIDENCE_SCHEMA || windows["schemaVersion"] != EVIDENCE_SCHEMA
    {
        failures.push("report schemaVersion is not the exact physical evidence schema");
    }
    if requester["profile"] != profile.label() || windows["profile"] != profile.label() {
        failures.push("report profiles do not equal the requested profile");
    }
    if requester["role"] != "requester" || windows["role"] != "windows-host" {
        failures.push("report roles are not requester and windows-host");
    }
    if requester["machine"]["os"] != "macos" || windows["machine"]["os"] != "windows" {
        failures.push("report operating systems are not macos requester and windows host");
    }
    if !clean_source_identity(&requester) || !clean_source_identity(&windows) {
        failures
            .push("a source-built report lacks a clean, present git identity captured at launch");
    }
    if requester["machine"]["git"]["commit"] != windows["machine"]["git"]["commit"] {
        failures.push("source-built reports do not name the same git commit");
    }
    if !nonempty_string(&requester["identity"]["localHostRef"])
        || !nonempty_string(&windows["identity"]["localHostRef"])
        || requester["identity"]["localHostRef"] == windows["identity"]["localHostRef"]
    {
        failures.push("requester and Windows local HostRef values are absent or not distinct");
    }
    if !same(
        &requester,
        &["attempt", "finalPlanState"],
        &json!("completed"),
    ) {
        failures.push("requester Core Plan is not completed");
    }
    if requester["attempt"]["attemptId"] != windows["attempt"]["attemptId"]
        || requester["attempt"]["revisionId"] != windows["attempt"]["revisionId"]
        || requester["attempt"]["revisionHash"] != windows["attempt"]["revisionHash"]
    {
        failures.push("reports do not name one exact attempt/revision/hash");
    }
    if requester["counts"]["stepCommitCount"].as_i64() != Some(expected)
        || requester["counts"]["uniqueCommittedSteps"].as_i64() != Some(expected)
    {
        failures.push("authoritative Core step commits are missing or duplicated");
    }
    if requester["readinessAndAdmission"]
        .as_array()
        .is_none_or(|hosts| {
            hosts.len() != 2
                || hosts.iter().any(|host| {
                    host["readinessState"] != "ready" || host["admissionState"] != "committed"
                })
                || !hosts
                    .iter()
                    .any(|host| host["hostRef"] == requester["identity"]["localHostRef"])
                || !hosts
                    .iter()
                    .any(|host| host["hostRef"] == windows["identity"]["localHostRef"])
                || hosts
                    .iter()
                    .map(|host| host["hostRef"].as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != 2
        })
    {
        failures.push("requester Core does not show exactly the two expected ready, committed participant Hosts");
    }
    if windows["counts"]["receiptCount"].as_i64() != Some(1) {
        failures.push("Windows exact destination receipt is absent or duplicated");
    }
    if requester["counts"]["externalDispatchCount"].as_i64() != Some(2)
        || requester["counts"]["uniqueExternalDispatches"].as_i64() != Some(2)
    {
        failures.push("Search/Transfer dispatch count is missing or duplicated");
    }
    if windows["transferReceipts"]
        .as_array()
        .is_none_or(|receipts| {
            receipts.len() != 1
                || receipts[0]["stepId"] != "transfer-mac-windows"
                || receipts[0]["revisionId"] != requester["attempt"]["revisionId"]
                || receipts[0]["revisionHash"] != requester["attempt"]["revisionHash"]
                || receipts[0]["destinationHostRef"] != windows["identity"]["localHostRef"]
                || !nonempty_string(&receipts[0]["contentDigest"])
        })
    {
        failures.push("Windows receipt does not prove transfer-mac-windows with a digest for this exact revision and destination HostRef");
    }
    if profile == Profile::B {
        if windows["managed"]["executeResults"]
            .as_array()
            .is_none_or(|items| items.len() != 1 || !nonempty_string(&items[0]["resultDigest"]))
        {
            failures.push("Windows authoritative Execute result is absent or duplicated");
        }
        if windows["managed"]["successorLineageCount"].as_i64() != Some(0) {
            failures.push("Execute created ManagedObject successor lineage");
        }
    }
    if requester["counts"]["integrity"] != "ok" || windows["counts"]["integrity"] != "ok" {
        failures.push("an authoritative SQLite integrity check did not return ok");
    }
    let outcome = if failures.is_empty() { "PASS" } else { "FAIL" };
    let result = json!({"schemaVersion":"pastey-native-v2-physical-verdict-v1", "profile":profile.label(), "outcome":outcome, "failures":failures, "requesterReport":required(args,"requester-report")?, "windowsReport":required(args,"windows-report")?});
    let dir = PathBuf::from(required(args, "output-dir")?);
    fs::create_dir_all(&dir)?;
    let json_path = dir.join(format!(
        "native-v2-physical-profile-{}-verdict.json",
        profile.label().to_lowercase()
    ));
    fs::write(&json_path, serde_json::to_vec_pretty(&result)?)?;
    fs::write(
        dir.join(format!(
            "native-v2-physical-profile-{}-verdict.txt",
            profile.label().to_lowercase()
        )),
        format!(
            "Native-v2 physical Profile {} verdict: {}\n{}\n",
            profile.label(),
            outcome,
            result["failures"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )?;
    println!("NATIVE_V2_PHYSICAL_PROFILE_{}_{}", profile.label(), outcome);
    println!("VERDICT_JSON={}", json_path.display());
    if outcome == "PASS" {
        Ok(())
    } else {
        Err(invalid(
            "physical evidence does not satisfy the authoritative acceptance gate",
        ))
    }
}

#[tokio::main]
async fn main() {
    let result = async {
        let (command, args) = args()?;
        match command.as_str() {
            "host" => {
                if std::env::consts::OS != "windows" {
                    return Err(invalid(
                        "host is reserved for the persistent Windows Host; macOS run owns the requester HostRuntime",
                    ));
                }
                let launch_identity = git_identity();
                let paths = paths(required(&args, "app-data-dir")?)?;
                write_host_launch_identity(&paths, &launch_identity)?;
                let runtime = start_host(paths).await?;
                let bridge_id = required(&args, "bridge-id")?;
                if storage::get_room_by_id(&runtime.paths, bridge_id).is_err() {
                    return Err(invalid("Bridge is not present in this Host database"));
                }
                println!(
                    "PHYSICAL_HOST_READY git_commit={} host_ref={} bridge_id={}",
                    launch_identity["commit"].as_str().unwrap_or("unavailable"),
                    runtime.local_host_ref.as_str(),
                    bridge_id
                );
                std::future::pending::<()>().await;
                runtime.shutdown_all();
                Ok(())
            }
            "run" => run_requester(Profile::parse(required(&args, "profile")?)?, &args).await,
            "collect" => {
                let profile = Profile::parse(required(&args, "profile")?)?;
                let role = required(&args, "role")?;
                let paths = paths(required(&args, "app-data-dir")?)?;
                let launch_identity = if role == "windows-host" {
                    if std::env::consts::OS != "windows" {
                        return Err(invalid("windows-host evidence must be collected on Windows"));
                    }
                    read_host_launch_identity(&paths)?
                } else {
                    git_identity()
                };
                write_report(
                    profile,
                    role,
                    &paths,
                    required(&args, "attempt-id")?,
                    Path::new(required(&args, "report-dir")?),
                    args.get("product-executable").map(String::as_str),
                    &launch_identity,
                )
                .map(|_| ())
            }
            "verify" => verify(Profile::parse(required(&args, "profile")?)?, &args),
            _ => Err(invalid(USAGE)),
        }
    }
    .await;
    if let Err(error) = result {
        eprintln!("ERROR: {}", error.message());
        std::process::exit(1);
    }
}
