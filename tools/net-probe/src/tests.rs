//! Deterministic checks of the probe itself, against iroh's in-process test
//! relay on loopback. Loopback is not cross-NAT: these prove the tool
//! measures and gates correctly, not how real networks behave.

use std::time::Duration;

use iroh::{test_utils::run_relay_server, RelayMap, SecretKey};

use super::*;

fn options(map: &RelayMap, ip: bool) -> NetOptions {
    NetOptions {
        relay: Relay::Map(map.clone()),
        dns: false,
        ip,
        portmapper: false,
        insecure_relay_tls: true,
    }
}

async fn listener(map: &RelayMap, ip: bool, secret: SecretKey, allow: Allowlist) -> Endpoint {
    let endpoint = bind_endpoint(&options(map, ip), secret, Some(allow))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), endpoint.online())
        .await
        .expect("listener reaches the test relay");
    let info = HostInfo::local(&endpoint, "test-listener");
    tokio::spawn(serve(endpoint.clone(), info));
    endpoint
}

async fn dialer(map: &RelayMap, secret: SecretKey) -> Endpoint {
    bind_endpoint(&options(map, true), secret, None)
        .await
        .unwrap()
}

#[test]
fn a_ticket_round_trips_and_rejects_foreign_text() {
    let addr = EndpointAddr::new(SecretKey::generate().public())
        .with_ip_addr("192.0.2.1:4433".parse().unwrap());
    let ticket = encode_ticket(&addr).unwrap();
    assert_eq!(decode_ticket(&ticket).unwrap(), addr);
    assert!(decode_ticket("pastey").is_err());
}

#[tokio::test]
async fn a_loopback_peer_goes_direct_and_carries_bulk_and_http_intact() {
    let (map, _url, _relay) = run_relay_server().await.unwrap();
    let dialer_key = SecretKey::generate();
    let listen = listener(
        &map,
        true,
        SecretKey::generate(),
        Allowlist::only([dialer_key.public()]),
    )
    .await;
    let dial_ep = dialer(&map, dialer_key).await;
    let plan = DialPlan {
        bulk_mib: 16,
        http: true,
        direct_wait: Duration::from_secs(10),
        ..DialPlan::default()
    };
    let report = dial(&dial_ep, listen.addr(), &plan, "test", &options(&map, true)).await;
    assert!(report.connect.ok, "{:?}", report.connect.error);
    assert!(
        report.connect.time_to_direct_ms.is_some(),
        "{:?}",
        report.connect
    );
    assert_eq!(report.connect.final_path, "direct");
    let bulk = report.bulk.unwrap();
    assert!(bulk.digest_ok);
    assert_eq!(bulk.bytes, 16 << 20);
    assert!(bulk.control_samples > 0);
    assert_eq!(bulk.control_lost, 0);
    let http = report.http.unwrap();
    assert!(http.ok, "{:?}", http.error);
    assert!(
        http.peer_identity_matches,
        "router saw {:?}",
        http.server_saw_peer
    );
    assert!(http.chunk_digests_ok);
    // A handler that needs the socket address cannot run over QUIC as is.
    assert_eq!(http.connect_info_route_status, Some(500));
    assert_eq!(report.peer.unwrap().net_desc, "test-listener");
    // One JSON record per run.
    assert!(serde_json::to_string(&dial_ep.id()).is_ok());
}

#[tokio::test]
async fn a_relay_only_peer_is_reported_as_relay_and_never_direct() {
    let (map, _url, _relay) = run_relay_server().await.unwrap();
    let dialer_key = SecretKey::generate();
    let listen = listener(
        &map,
        false,
        SecretKey::generate(),
        Allowlist::only([dialer_key.public()]),
    )
    .await;
    let dial_ep = dialer(&map, dialer_key).await;
    let plan = DialPlan {
        bulk_mib: 4,
        direct_wait: Duration::from_secs(3),
        ..DialPlan::default()
    };
    let report = dial(&dial_ep, listen.addr(), &plan, "test", &options(&map, true)).await;
    assert!(report.connect.ok, "{:?}", report.connect.error);
    assert_eq!(report.connect.time_to_direct_ms, None);
    assert_eq!(report.connect.final_path, "relay");
    let bulk = report.bulk.unwrap();
    assert!(bulk.digest_ok);
    assert_eq!(bulk.direct_share, 0.0);
    assert!(bulk.relay_tx_bytes >= 4 << 20, "{bulk:?}");
}

#[tokio::test]
async fn the_allowlist_refuses_an_unlisted_key_after_the_handshake() {
    let (map, _url, _relay) = run_relay_server().await.unwrap();
    let listen = listener(
        &map,
        true,
        SecretKey::generate(),
        Allowlist::only([SecretKey::generate().public()]),
    )
    .await;
    let stranger = dialer(&map, SecretKey::generate()).await;
    let report = dial(
        &stranger,
        listen.addr(),
        &DialPlan::default(),
        "test",
        &options(&map, true),
    )
    .await;
    assert!(!report.connect.ok);
    assert!(report.connect.rejected, "{:?}", report.connect);
    assert!(report.peer.is_none(), "no stream was served");
}

#[tokio::test]
async fn several_hosts_are_served_at_once_and_only_listed_ones() {
    let (map, _url, _relay) = run_relay_server().await.unwrap();
    let keys = (0..3).map(|_| SecretKey::generate()).collect::<Vec<_>>();
    let listen = listener(
        &map,
        true,
        SecretKey::generate(),
        Allowlist::only(keys.iter().map(SecretKey::public)),
    )
    .await;
    let addr = listen.addr();
    let mut runs = Vec::new();
    for key in keys {
        let map = map.clone();
        let addr = addr.clone();
        runs.push(tokio::spawn(async move {
            let ep = dialer(&map, key).await;
            let plan = DialPlan {
                bulk_mib: 4,
                direct_wait: Duration::from_secs(5),
                ..DialPlan::default()
            };
            dial(&ep, addr, &plan, "test", &options(&map, true)).await
        }));
    }
    let stranger = dialer(&map, SecretKey::generate()).await;
    let refused = dial(
        &stranger,
        addr,
        &DialPlan::default(),
        "test",
        &options(&map, true),
    )
    .await;
    for run in runs {
        let report = run.await.unwrap();
        assert!(report.connect.ok, "{:?}", report.connect.error);
        assert!(report.bulk.unwrap().digest_ok);
    }
    assert!(!refused.connect.ok);
}

#[tokio::test]
async fn a_restarted_peer_is_recovered_by_redialling_its_key() {
    let (map, _url, _relay) = run_relay_server().await.unwrap();
    let dialer_key = SecretKey::generate();
    let listener_key = SecretKey::generate();
    let allow = Allowlist::only([dialer_key.public()]);
    let first = listener(&map, true, listener_key.clone(), allow.clone()).await;
    let addr = first.addr();
    let dial_ep = dialer(&map, dialer_key).await;

    let restart = {
        let map = map.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            first.close().await;
            tokio::time::sleep(Duration::from_secs(2)).await;
            // Same key, new port: only the relay URL in the ticket still
            // points at it.
            listener(&map, true, listener_key, allow).await
        })
    };
    let plan = DialPlan {
        direct_wait: Duration::from_secs(2),
        watch: Duration::from_secs(16),
        probe_interval: Duration::from_millis(500),
        probe_timeout: Duration::from_secs(1),
        ..DialPlan::default()
    };
    let report = dial(&dial_ep, addr, &plan, "test", &options(&map, true)).await;
    let _second = restart.await.unwrap();
    let watch = report.watch.unwrap();
    assert!(!watch.outages.is_empty(), "{watch:?}");
    assert!(watch.redials >= 1, "{watch:?}");
    assert_eq!(
        watch.outages[0].recovered_by.as_deref(),
        Some("redial"),
        "{watch:?}"
    );
    assert!(watch.ended_connected, "{watch:?}");
}
