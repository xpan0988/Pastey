//! Isolated iroh 1.3 experiment for Pastey NAT-traversal research.
//! Not part of Pastey. Two endpoints in one process on one machine, so a
//! "direct" path here is a local path, not cross-NAT evidence. It measures:
//! path observation (direct vs relay), an application-level direct-only
//! gate, peer pinning by EndpointId, payload integrity and stream throughput.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use iroh::{endpoint::presets, Endpoint, EndpointId};


const ALPN: &[u8] = b"pastey/connectivity-poc/0";

#[derive(Debug)]
struct PathSnapshot {
    selected_ip: bool,
    selected_relay: bool,
    paths: Vec<String>,
}

fn snapshot(conn: &iroh::endpoint::Connection) -> PathSnapshot {
    let list = conn.paths();
    let mut selected_ip = false;
    let mut selected_relay = false;
    let mut paths = Vec::new();
    for path in list.iter() {
        if path.is_selected() {
            selected_ip |= path.is_ip();
            selected_relay |= path.is_relay();
        }
        let stats = path.stats();
        paths.push(format!(
            "{}{} rtt={:?} tx_bytes={} tx_datagrams={}",
            if path.is_relay() { "relay" } else if path.is_ip() { "ip" } else { "other" },
            if path.is_selected() { "*" } else { "" },
            path.rtt(),
            stats.udp_tx.bytes,
            stats.udp_tx.datagrams
        ));
    }
    PathSnapshot { selected_ip, selected_relay, paths }
}

/// Pastey's rule: application bytes only on a selected direct (IP) path.
async fn wait_for_direct(conn: &iroh::endpoint::Connection, limit: Duration) -> Result<Duration> {
    let start = Instant::now();
    loop {
        let snap = snapshot(conn);
        if snap.selected_ip {
            return Ok(start.elapsed());
        }
        if start.elapsed() > limit {
            bail!("no direct path within {limit:?}; paths: {:?}", snap.paths);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn acceptor(ep: Endpoint, pinned: EndpointId, payload_len: usize) -> Result<String> {
    let incoming = ep.accept().await.ok_or_else(|| anyhow!("endpoint closed"))?;
    let conn = incoming.await?;
    // Pinning: a valid iroh key is not enough; it must be the paired peer.
    if conn.remote_id() != pinned {
        conn.close(1u32.into(), b"not a paired Pastey device");
        return Ok("refused unpinned peer".into());
    }
    let (mut send, mut recv) = conn.accept_bi().await?;
    let mut header = [0u8; 8];
    recv.read_exact(&mut header).await?;
    let expected = u64::from_be_bytes(header) as usize;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut got = 0usize;
    while got < expected {
        let n = recv.read(&mut buf).await?.ok_or_else(|| anyhow!("stream ended early"))?;
        hasher.update(&buf[..n]);
        got += n;
    }
    send.write_all(hasher.finalize().as_bytes()).await?;
    send.finish()?;
    conn.closed().await;
    let _ = payload_len;
    Ok(format!("received {got} bytes"))
}

async fn dial_and_send(
    dialer: &Endpoint,
    target: iroh::EndpointAddr,
    payload: &[u8],
    gate: Duration,
) -> Result<String> {
    let t0 = Instant::now();
    let conn = dialer.connect(target, ALPN).await?;
    let handshake = t0.elapsed();
    let first = snapshot(&conn);
    let direct_after = match wait_for_direct(&conn, gate).await {
        Ok(after) => after,
        Err(error) => {
            let report = format!(
                "handshake {handshake:?}; first paths {:?}; GATE: {error}; application payload withheld (0 bytes sent)",
                first.paths
            );
            conn.close(0u32.into(), b"no direct path");
            return Ok(report);
        }
    };
    let (mut send, mut recv) = conn.open_bi().await?;
    let start = Instant::now();
    send.write_all(&(payload.len() as u64).to_be_bytes()).await?;
    let mut relay_selected_during_send = false;
    for chunk in payload.chunks(256 * 1024) {
        send.write_all(chunk).await?;
        relay_selected_during_send |= snapshot(&conn).selected_relay;
    }
    send.finish()?;
    let mut digest = [0u8; 32];
    recv.read_exact(&mut digest).await?;
    let elapsed = start.elapsed();
    let intact = digest == *blake3::hash(payload).as_bytes();
    let final_paths = snapshot(&conn).paths;
    conn.close(0u32.into(), b"done");
    Ok(format!(
        "handshake {handshake:?}; first paths {:?}; direct selected after {direct_after:?}; \
         sent {} bytes in {elapsed:?} ({:.1} MiB/s); digest intact: {intact}; \
         relay ever selected during send: {relay_selected_during_send}; final paths {final_paths:?}",
        first.paths,
        payload.len(),
        payload.len() as f64 / 1048576.0 / elapsed.as_secs_f64()
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let scenario = std::env::args().nth(1).unwrap_or_default();
    let mb: usize = std::env::args().nth(2).and_then(|v| v.parse().ok()).unwrap_or(64);
    let payload: Vec<u8> = (0..mb * 1048576).map(|i| (i * 31 % 251) as u8).collect();

    let dialer = Endpoint::builder(presets::N0).bind().await?;
    let acceptor_builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]);
    let acceptor_ep = match scenario.as_str() {
        // Reachable only through n0's public relay: no IP transport at all.
        "relay-only" => acceptor_builder.clear_ip_transports().bind().await?,
        _ => acceptor_builder.bind().await?,
    };
    let online = Instant::now();
    tokio::time::timeout(Duration::from_secs(15), acceptor_ep.online()).await?;
    tokio::time::timeout(Duration::from_secs(15), dialer.online()).await?;
    println!("[{scenario}] both endpoints online (home relay reachable) in {:?}", online.elapsed());
    let addr = acceptor_ep.addr();
    println!("[{scenario}] acceptor addr: relay={:?} direct_addrs={}", addr.relay_urls().collect::<Vec<_>>(), addr.ip_addrs().count());

    let pinned = match scenario.as_str() {
        "unpinned" => iroh::SecretKey::generate().public(),
        _ => dialer.id(),
    };
    let accept_task = tokio::spawn(acceptor(acceptor_ep.clone(), pinned, payload.len()));
    let report = dial_and_send(&dialer, addr, &payload, Duration::from_secs(8)).await;
    println!("[{scenario}] dialer: {}", match &report { Ok(r) => r.clone(), Err(e) => format!("error: {e:#}") });
    match tokio::time::timeout(Duration::from_secs(10), accept_task).await {
        Ok(Ok(Ok(r))) => println!("[{scenario}] acceptor: {r}"),
        Ok(Ok(Err(e))) => println!("[{scenario}] acceptor: error: {e:#}"),
        _ => println!("[{scenario}] acceptor: no result"),
    }
    dialer.close().await;
    acceptor_ep.close().await;
    Ok(())
}
