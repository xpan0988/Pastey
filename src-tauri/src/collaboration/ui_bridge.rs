//! A development bridge, not a test of behaviour: two real Hosts (the pair
//! harness's Mac and Windows, with the installed Codex and Claude Code)
//! serve their real Tauri commands on localhost, through Tauri's own IPC
//! dispatch (`tauri::test`), so the app's frontend in a browser can drive
//! them. It checks the Collaboration screens against real backends without
//! a native window to click.
//!
//! Run it, then open the frontend with the IPC shim (`tools/ui-bridge`):
//!
//!     cargo test --bin pastey collaboration_screens_over_a_browser_bridge -- --ignored --nocapture
//!     npx vite --config tools/ui-bridge/vite.config.mjs
//!
//! Mac: port 17431, Windows: 17432. `POST /ipc {cmd, args}` answers
//! `{ok}` or `{err}`; `POST /pick {path}` sets what the folder dialog
//! returns next. It stops when `target/ui-bridge/stop` exists, or after an
//! hour. Collaboration turns run at effort `low`.

use std::{path::PathBuf, sync::Arc};

use axum::{extract::State as AxumState, routing::post, Json, Router};
use serde_json::{json, Value};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};

use crate::host_runtime::HostRuntime;
use crate::room_control::runtime_pair_tests::{RuntimePairV1, ROOM};
use crate::storage;

#[derive(Clone)]
struct BridgeV1 {
    webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    pick: Arc<parking_lot::Mutex<Option<String>>>,
}

async fn ipc(AxumState(bridge): AxumState<BridgeV1>, Json(request): Json<Value>) -> Json<Value> {
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or_default().to_owned();
    let args = request.get("args").cloned().unwrap_or(json!({}));
    // The frontend's plugins: events are not delivered (screens poll), the
    // folder dialog answers what /pick set.
    if cmd.starts_with("plugin:event|") {
        return Json(json!({ "ok": 0 }));
    }
    if cmd == "plugin:dialog|open" {
        return Json(json!({ "ok": bridge.pick.lock().take() }));
    }
    let response = tokio::task::spawn_blocking(move || {
        tauri::test::get_ipc_response(
            &bridge.webview,
            tauri::webview::InvokeRequest {
                cmd,
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "http://tauri.localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
    })
    .await
    .unwrap();
    Json(match response {
        Ok(body) => json!({ "ok": body.deserialize::<Value>().unwrap_or(Value::Null) }),
        Err(error) => json!({ "err": error }),
    })
}

async fn pick(AxumState(bridge): AxumState<BridgeV1>, Json(request): Json<Value>) -> Json<Value> {
    *bridge.pick.lock() = request.get("path").and_then(Value::as_str).map(str::to_owned);
    Json(json!({ "ok": true }))
}

fn mock_app(runtime: Arc<HostRuntime>) -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        .manage(runtime)
        // Every command of the app except the six that need a native
        // window (reveal, logs folder, clipboard, updates).
        .invoke_handler(tauri::generate_handler![
            crate::commands::create_room,
            crate::commands::open_bridge_pairing,
            crate::commands::repair_bridge,
            crate::commands::list_known_devices,
            crate::commands::forget_known_device,
            crate::commands::join_room,
            crate::commands::join_room_at,
            crate::commands::get_device_identity,
            crate::commands::rotate_device_identity,
            crate::commands::present_device_identity,
            crate::commands::list_seen_peer_devices,
            crate::commands::trust_peer_device,
            crate::commands::list_trusted_devices,
            crate::commands::revoke_trusted_device,
            crate::commands::get_bridge_full_access,
            crate::commands::list_full_access,
            crate::commands::grant_full_access,
            crate::commands::revoke_full_access,
            crate::commands::remove_full_access_folder,
            crate::commands::get_bridge_peer_address,
            crate::commands::set_bridge_peer_address,
            crate::commands::list_nearby_devices,
            crate::commands::request_nearby_join,
            crate::commands::complete_nearby_join,
            crate::commands::accept_nearby_join,
            crate::commands::reject_nearby_join,
            crate::commands::pending_join_requests,
            crate::commands::mark_join_prompt_rendered,
            crate::commands::list_rooms,
            crate::commands::get_room,
            crate::commands::pair_bridge_peer,
            crate::commands::revoke_bridge_peer_pairing,
            crate::commands::mark_bridge_peer_pairing_rotation_required,
            crate::commands::list_room_items,
            crate::commands::send_text_to_room,
            crate::commands::send_file_to_room,
            crate::commands::create_composed_file_bridge_plan,
            crate::commands::create_direct_file_transfer_bridge_plan,
            crate::commands::refresh_selected_peer_capabilities,
            crate::commands::compose_natural_v2_candidate,
            crate::commands::compose_native_v2_plan,
            crate::commands::approve_native_v2_plan,
            crate::commands::start_native_v2_plan_attempt,
            crate::commands::get_native_v2_plan_status,
            crate::commands::cancel_native_v2_plan_attempt,
            crate::commands::list_native_agent_capabilities,
            crate::commands::discover_native_sessions,
            crate::commands::read_native_agent_defaults,
            crate::commands::read_native_model_catalogs,
            crate::commands::read_remote_native_model_catalogs,
            crate::commands::inspect_native_session,
            crate::commands::continue_native_session,
            crate::commands::get_internet_connectivity,
            crate::commands::set_internet_connectivity,
            crate::commands::get_bridge_internet_status,
            crate::commands::continue_remote_native_session,
            crate::commands::discover_remote_native_sessions,
            crate::commands::inspect_remote_native_session,
            crate::commands::list_session_read_grants,
            crate::commands::list_bridge_workspace_shares,
            crate::commands::share_bridge_workspace,
            crate::commands::update_workspace_share,
            crate::commands::stop_sharing_workspace,
            crate::commands::list_remote_shared_workspaces,
            crate::commands::open_remote_folder,
            crate::commands::list_authorized_search_folders,
            crate::commands::authorize_search_folder,
            crate::commands::revoke_authorized_search_folder,
            crate::commands::list_remote_native_agent_capabilities,
            crate::commands::start_native_codex_task,
            crate::commands::start_native_claude_code_task,
            crate::commands::list_collaborations,
            crate::commands::get_collaboration,
            crate::commands::create_collaboration,
            crate::commands::set_collaboration_state,
            crate::commands::delete_collaboration,
            crate::commands::send_collaboration_message,
            crate::commands::approve_collaboration_messages,
            crate::commands::dismiss_collaboration_entry,
            crate::commands::resume_collaboration_member,
            crate::commands::collaboration_needs_you_count,
            crate::commands::set_collaboration_grant,
            crate::commands::withdraw_collaboration_grant,
            crate::commands::add_collaboration_member,
            crate::commands::remove_collaboration_member,
            crate::commands::set_collaboration_goal,
            crate::commands::set_collaboration_role,
            crate::commands::decide_collaboration_summary,
            crate::commands::invite_collaboration_member,
            crate::commands::list_collaboration_invitations,
            crate::commands::answer_collaboration_invitation,
            crate::commands::get_native_agent_task_session,
            crate::commands::get_native_agent_task_status,
            crate::commands::cancel_native_agent_task,
            crate::commands::list_native_invocation_reviews,
            crate::commands::accept_native_invocation_review,
            crate::commands::deny_native_invocation_review,
            crate::commands::start_remote_native_codex_task,
            crate::commands::start_remote_native_capability_task,
            crate::commands::start_remote_native_claude_code_task,
            crate::commands::cancel_remote_native_agent_task,
            crate::commands::respond_local_native_decision,
            crate::commands::stop_collaboration_member,
            crate::commands::set_collaboration_member_options,
            crate::commands::list_execution_presets,
            crate::commands::get_execution_default,
            crate::commands::set_execution_default,
            crate::commands::set_collaboration_execution,
            crate::commands::check_collaboration_readiness,
            crate::commands::release_collaboration,
            crate::commands::respond_remote_native_decision,
            crate::commands::stop_bridge_native_agent_task,
            crate::commands::propose_remote_native_codex_workspace_movement,
            crate::commands::approve_remote_native_codex_workspace_movement,
            crate::commands::get_native_agent_workspace_movement_status,
            crate::commands::get_native_agent_recovery_projection,
            crate::commands::discard_native_agent_conflict_result,
            crate::commands::retry_native_agent_workspace_result_return,
            crate::commands::reconcile_remote_native_agent_task,
            crate::commands::list_bridge_plan_workspace,
            crate::commands::approve_bridge_plan,
            crate::commands::withdraw_bridge_plan_revision,
            crate::commands::bind_bridge_plan_to_session,
            crate::commands::start_bridge_plan_attempt,
            crate::commands::select_bridge_plan_search_candidate,
            crate::commands::get_room_control_session_context,
            crate::commands::list_received_room_control_events,
            crate::commands::get_room_control_diagnostics,
            crate::commands::enter_developer_mode,
            crate::commands::get_developer_terminal_workspace,
            crate::commands::request_developer_terminal,
            crate::commands::accept_developer_terminal,
            crate::commands::close_hosted_developer_terminal,
            crate::commands::deny_developer_terminal,
            crate::commands::send_developer_terminal_input,
            crate::commands::resize_developer_terminal,
            crate::commands::close_developer_terminal,
            crate::commands::cancel_transfer,
            crate::commands::update_transfer_window,
            crate::commands::write_temp_file,
            crate::commands::get_file_transfer_metadata,
            crate::commands::delete_temp_file,
            crate::commands::burn_room,
            crate::commands::get_config,
            crate::commands::list_managed_runtime_options,
            crate::commands::select_managed_execute_runtime,
            crate::commands::get_managed_worker_provider_settings,
            crate::commands::create_managed_worker_provider,
            crate::commands::update_managed_worker_provider,
            crate::commands::delete_managed_worker_provider,
            crate::commands::select_managed_worker_provider,
            crate::commands::check_managed_worker_provider_health,
            crate::commands::get_device_profile,
            crate::commands::get_device_capabilities,
            crate::commands::confirm_capability_acquisition,
            crate::commands::get_bridge_node_list_projection,
            crate::commands::run_bridge_device_diagnostics,
            crate::commands::run_bridge_device_self_check,
            crate::commands::run_loopback_benchmark,
            crate::commands::run_peer_link_benchmark,
            crate::commands::get_last_benchmark_results,
            crate::commands::update_config,
            crate::commands::log_frontend_diagnostic
        ])
        .build(mock_context(noop_assets()))
        .unwrap()
}

async fn serve(runtime: Arc<HostRuntime>, port: u16) -> tauri::App<tauri::test::MockRuntime> {
    let app = mock_app(runtime);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();
    let bridge = BridgeV1 {
        webview,
        pick: Arc::default(),
    };
    let router = Router::new()
        .route("/ipc", post(ipc))
        .route("/pick", post(pick))
        .with_state(bridge)
        .layer(axum::middleware::from_fn(cors));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    app
}

async fn cors(request: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let preflight = request.method() == axum::http::Method::OPTIONS;
    let mut response = if preflight {
        axum::response::Response::new(axum::body::Body::empty())
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert("access-control-allow-origin", "*".parse().unwrap());
    headers.insert("access-control-allow-headers", "content-type".parse().unwrap());
    headers.insert("access-control-allow-methods", "POST, OPTIONS".parse().unwrap());
    response
}

fn folder(name: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-bridge").join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path.canonicalize().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a development bridge for the frontend; uses the installed Codex and Claude Code when turns run"]
async fn collaboration_screens_over_a_browser_bridge() {
    *super::TURN_OPTIONS_FOR_TESTS.lock().unwrap() = Some(crate::native_agent::NativeSessionOptionsV1 {
        model: None,
        effort: Some("low".into()),
    });
    let pair = RuntimePairV1::new().await;
    let key = |runtime: &HostRuntime| {
        let master_key = crate::config::master_key(&runtime.config.read()).unwrap();
        crate::device_identity::load_or_create(&runtime.paths, &master_key).unwrap().public_key()
    };
    storage::pin_bridge_peer(&pair.windows.runtime.paths, ROOM, pair.mac.host_ref().as_str(), &key(&pair.mac.runtime)).unwrap();
    storage::pin_bridge_peer(&pair.mac.runtime.paths, ROOM, pair.windows.host_ref().as_str(), &key(&pair.windows.runtime)).unwrap();
    let api = folder("mac-api");
    std::fs::write(api.join("orders.py"), "# GET /v2/orders now takes ?cursor= and returns nextCursor; ?offset= gets 400.\ndef list_orders(cursor=None, limit=50):\n    ...\n").unwrap();
    let docs = folder("mac-docs");
    std::fs::write(docs.join("orders.md"), "# GET /v2/orders\n\nPaginate with `offset`.\n").unwrap();
    let web = folder("windows-web");
    std::fs::write(web.join("OrdersTable.js"), "export async function loadPage(page) {\n  return fetch(`/v2/orders?offset=${page * 50}`);\n}\n").unwrap();
    // Windows shares web with the Mac, Sessions and Agent requests on.
    crate::commands::present_device_identity_over(
        pair.mac.runtime.clone(),
        pair.mac.runtime.resolve_current_remote_host_session(ROOM, &pair.windows.host_ref()).await.unwrap().binding(),
    )
    .await
    .unwrap();
    let seen = crate::host_runtime::current_verified_peer_devices(&pair.windows.runtime);
    let mac = seen.iter().find(|device| device.binding.bridge_id == ROOM).unwrap();
    crate::device_trust::trust_seen_device(&pair.windows.runtime.paths, &seen, &mac.fingerprint, "Mac-Studio").unwrap();
    crate::session_read_access::share_with_bridge(
        &pair.windows.runtime.paths,
        &pair.windows.host_ref(),
        &seen,
        &crate::session_read_access::ShareWorkspaceV1 {
            room_id: ROOM.into(),
            device_fingerprint: Some(mac.fingerprint.clone()),
            workspace_path: web.to_string_lossy().into_owned(),
            duration_hours: 24,
            read_sessions: true,
            share_excerpts: false,
            agent_requests: true,
        },
        storage::now_ts(),
    )
    .unwrap();
    // The installed Claude Code, which test builds otherwise leave out.
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/pastey");
    assert!(helper.is_file(), "build `pastey` first: cargo build");
    for runtime in [&pair.mac.runtime, &pair.windows.runtime] {
        runtime.native_agents.lock().add_installed_claude_code_conversation_for_test(&helper);
    }
    super::start_scheduler(&pair.mac.runtime);
    super::start_scheduler(&pair.windows.runtime);
    let _mac_app = serve(pair.mac.runtime.clone(), 17431).await;
    let _windows_app = serve(pair.windows.runtime.clone(), 17432).await;
    eprintln!("bridge: mac 17431, windows 17432; folders {} {} {}", api.display(), docs.display(), web.display());
    let stop = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-bridge/stop");
    let _ = std::fs::remove_file(&stop);
    for _ in 0..3600 {
        if stop.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
