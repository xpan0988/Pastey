use std::{sync::Arc, time::Duration};

use futures_util::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::Instant,
};

use crate::{
    chunk_frame::{decode_binary_chunk_frame, encode_binary_chunk_frame, BinaryChunkFrame},
    crypto,
    diagnostics::{quality_label, BenchmarkMode, LinkBenchmarkResult},
    error::{AppError, AppResult},
    storage, transfer_tuning, AppState,
};

const DEFAULT_DURATION_SECS: u64 = 5;
const MAX_DURATION_SECS: u64 = 15;
const RAW_CHUNK_BYTES: usize = 256 * 1024;
const PIPELINE_CHUNK_BYTES: usize = 256 * 1024;
const BENCHMARK_KEY: [u8; 32] = [42; 32];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkDiscardResponse {
    pub received_bytes: u64,
    pub receiver_cpu_hint: Option<String>,
}

pub async fn run_loopback_benchmark(
    mode: BenchmarkMode,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    sender_cpu_hint: Option<String>,
) -> AppResult<LinkBenchmarkResult> {
    let duration = benchmark_duration(duration_seconds);
    let chunk = benchmark_payload(mode, 0)?;
    let latency_ms = estimate_loopback_latency().await.ok();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let receiver_mode = mode;
    let receiver = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        receive_loopback_stream(stream, receiver_mode).await
    });

    let mut stream = TcpStream::connect(address).await?;
    let started_at = Instant::now();
    let mut last_sample_at = started_at;
    let mut sample_bytes = 0u64;
    let mut total_bytes = 0u64;
    let mut peak_mbps: f64 = 0.0;
    let mut chunk_index = 0u64;

    // Loopback diagnostics stay on localhost. They measure local memory/socket
    // and optional Pastey framing overhead, not Wi-Fi, Ethernet, ISP, or disk.
    while started_at.elapsed() < duration {
        let payload = if mode == BenchmarkMode::PasteyPipeline {
            benchmark_payload(mode, chunk_index)?
        } else {
            chunk.clone()
        };
        let len = u32::try_from(payload.len())
            .map_err(|_| AppError::InvalidInput("Benchmark payload too large.".into()))?;
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&payload).await?;
        let plaintext_bytes = plaintext_len_for_payload(mode, &payload)?;
        total_bytes = total_bytes.saturating_add(plaintext_bytes);
        sample_bytes = sample_bytes.saturating_add(plaintext_bytes);
        chunk_index = chunk_index.saturating_add(1);

        if last_sample_at.elapsed() >= Duration::from_millis(500) {
            peak_mbps = peak_mbps.max(mbps(sample_bytes, last_sample_at.elapsed()));
            sample_bytes = 0;
            last_sample_at = Instant::now();
        }
    }

    stream.shutdown().await?;
    let _received_bytes = receiver
        .await
        .map_err(|_| AppError::Network("Loopback benchmark interrupted.".into()))??;
    let duration_ms = started_at.elapsed().as_millis().max(1) as u64;
    peak_mbps = peak_mbps.max(mbps(sample_bytes, last_sample_at.elapsed()));
    let average_mbps = mbps(total_bytes, Duration::from_millis(duration_ms));

    Ok(LinkBenchmarkResult {
        peer_id: Some("loopback".into()),
        peer_name: Some("This device".into()),
        average_MBps: round_metric(average_mbps),
        peak_MBps: round_metric(peak_mbps),
        latency_ms: latency_ms.map(round_metric),
        duration_ms,
        total_bytes,
        effective_window_size: Some(effective_window(window_size)),
        sender_cpu_hint,
        receiver_cpu_hint: None,
        failed_chunks: 0,
        duplicate_chunks: 0,
        benchmark_mode: mode,
        link_quality: quality_label(average_mbps, latency_ms),
        timestamp: storage::now_ts(),
    })
}

pub async fn run_peer_link_benchmark(
    state: Arc<AppState>,
    room_id: String,
    mode: BenchmarkMode,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    sender_cpu_hint: Option<String>,
) -> AppResult<LinkBenchmarkResult> {
    let room = storage::get_room_by_id(&state.paths, &room_id)?;
    let (peer_host, peer_port) = room
        .peer_host
        .zip(room.peer_port)
        .ok_or_else(|| AppError::NotFound("Peer is not connected.".into()))?;
    let base_url = format!("http://{peer_host}:{peer_port}/rooms/{room_id}/diagnostics");
    let client = reqwest::Client::new();
    let latency_ms = estimate_peer_latency(&client, &base_url).await.ok();
    let duration = benchmark_duration(duration_seconds);
    let window = effective_window(window_size);
    let url = match mode {
        BenchmarkMode::RawMemory => format!("{base_url}/benchmark/raw"),
        BenchmarkMode::PasteyPipeline => format!("{base_url}/benchmark/pipeline"),
    };
    let started_at = Instant::now();
    let mut last_sample_at = started_at;
    let mut sample_bytes = 0u64;
    let mut total_bytes = 0u64;
    let mut peak_mbps: f64 = 0.0;
    let mut failed_chunks = 0u64;
    let mut chunk_index = 0u64;
    let mut in_flight = FuturesUnordered::new();
    let mut receiver_cpu_hint = None;

    // Peer diagnostics use the trusted room's LAN endpoint and discard payloads
    // in memory. They are a network baseline, not a real file transfer path.
    while started_at.elapsed() < duration {
        while in_flight.len() < window && started_at.elapsed() < duration {
            let payload = benchmark_payload(mode, chunk_index)?;
            let plaintext_bytes = plaintext_len_for_payload(mode, &payload)?;
            let client = client.clone();
            let url = url.clone();
            in_flight.push(async move {
                let response = client.post(url).body(payload).send().await?;
                if !response.status().is_success() {
                    return Err(AppError::Network("Peer benchmark chunk rejected.".into()));
                }
                let ack = response.json::<BenchmarkDiscardResponse>().await?;
                Ok::<_, AppError>((plaintext_bytes, ack.receiver_cpu_hint))
            });
            chunk_index = chunk_index.saturating_add(1);
        }

        let Some(result) = in_flight.next().await else {
            break;
        };
        match result {
            Ok((bytes, hint)) => {
                total_bytes = total_bytes.saturating_add(bytes);
                sample_bytes = sample_bytes.saturating_add(bytes);
                if receiver_cpu_hint.is_none() {
                    receiver_cpu_hint = hint;
                }
            }
            Err(_) => {
                failed_chunks = failed_chunks.saturating_add(1);
            }
        }

        if last_sample_at.elapsed() >= Duration::from_millis(500) {
            peak_mbps = peak_mbps.max(mbps(sample_bytes, last_sample_at.elapsed()));
            sample_bytes = 0;
            last_sample_at = Instant::now();
        }
    }

    while let Some(result) = in_flight.next().await {
        match result {
            Ok((bytes, hint)) => {
                total_bytes = total_bytes.saturating_add(bytes);
                sample_bytes = sample_bytes.saturating_add(bytes);
                if receiver_cpu_hint.is_none() {
                    receiver_cpu_hint = hint;
                }
            }
            Err(_) => failed_chunks = failed_chunks.saturating_add(1),
        }
    }

    let duration_ms = started_at.elapsed().as_millis().max(1) as u64;
    peak_mbps = peak_mbps.max(mbps(sample_bytes, last_sample_at.elapsed()));
    let average_mbps = mbps(total_bytes, Duration::from_millis(duration_ms));

    Ok(LinkBenchmarkResult {
        peer_id: Some(room_id),
        peer_name: room.peer_device_name,
        average_MBps: round_metric(average_mbps),
        peak_MBps: round_metric(peak_mbps),
        latency_ms: latency_ms.map(round_metric),
        duration_ms,
        total_bytes,
        effective_window_size: Some(window),
        sender_cpu_hint,
        receiver_cpu_hint,
        failed_chunks,
        duplicate_chunks: 0,
        benchmark_mode: mode,
        link_quality: quality_label(average_mbps, latency_ms),
        timestamp: storage::now_ts(),
    })
}

pub fn discard_benchmark_payload(mode: BenchmarkMode, body: &[u8]) -> AppResult<u64> {
    match mode {
        BenchmarkMode::RawMemory => Ok(body.len() as u64),
        BenchmarkMode::PasteyPipeline => {
            let frame = decode_binary_chunk_frame(body).map_err(|error| {
                AppError::InvalidInput(format!("Invalid benchmark frame: {}", error.as_str()))
            })?;
            let plaintext = crypto::decrypt_bytes(&frame.ciphertext, &BENCHMARK_KEY, &frame.nonce)?;
            if plaintext.len() != frame.plaintext_size as usize {
                return Err(AppError::InvalidInput(
                    "Benchmark plaintext size mismatch.".into(),
                ));
            }
            Ok(plaintext.len() as u64)
        }
    }
}

pub fn cpu_hint() -> Option<String> {
    std::thread::available_parallelism()
        .ok()
        .map(|count| format!("{} logical cores", count.get()))
}

async fn receive_loopback_stream(mut stream: TcpStream, mode: BenchmarkMode) -> AppResult<u64> {
    let mut total = 0u64;
    loop {
        let mut len_bytes = [0u8; 4];
        match stream.read_exact(&mut len_bytes).await {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        let len = u32::from_be_bytes(len_bytes) as usize;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).await?;
        total = total.saturating_add(discard_benchmark_payload(mode, &payload)?);
    }
    Ok(total)
}

async fn estimate_loopback_latency() -> AppResult<f64> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let receiver = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await?;
        stream.write_all(&byte).await?;
        Ok::<_, std::io::Error>(())
    });
    let started = Instant::now();
    let mut stream = TcpStream::connect(address).await?;
    stream.write_all(&[1]).await?;
    let mut byte = [0u8; 1];
    stream.read_exact(&mut byte).await?;
    stream.shutdown().await?;
    receiver
        .await
        .map_err(|_| AppError::Network("Loopback benchmark ping interrupted.".into()))??;
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

async fn estimate_peer_latency(client: &reqwest::Client, base_url: &str) -> AppResult<f64> {
    let started = Instant::now();
    let response = client.post(format!("{base_url}/ping")).send().await?;
    if !response.status().is_success() {
        return Err(AppError::Network("Peer benchmark ping rejected.".into()));
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn benchmark_payload(mode: BenchmarkMode, chunk_index: u64) -> AppResult<Vec<u8>> {
    match mode {
        BenchmarkMode::RawMemory => Ok(vec![0x5a; RAW_CHUNK_BYTES]),
        BenchmarkMode::PasteyPipeline => {
            let plaintext = vec![0xa5; PIPELINE_CHUNK_BYTES];
            let (ciphertext, nonce) = crypto::encrypt_bytes(&plaintext, &BENCHMARK_KEY)?;
            encode_binary_chunk_frame(&BinaryChunkFrame {
                chunk_index,
                nonce,
                ciphertext,
                plaintext_size: plaintext.len() as u32,
                is_final: false,
            })
            .map_err(|error| {
                AppError::InvalidInput(format!("Invalid benchmark frame: {}", error.as_str()))
            })
        }
    }
}

fn plaintext_len_for_payload(mode: BenchmarkMode, payload: &[u8]) -> AppResult<u64> {
    match mode {
        BenchmarkMode::RawMemory => Ok(payload.len() as u64),
        BenchmarkMode::PasteyPipeline => {
            let frame = decode_binary_chunk_frame(payload).map_err(|error| {
                AppError::InvalidInput(format!("Invalid benchmark frame: {}", error.as_str()))
            })?;
            Ok(frame.plaintext_size as u64)
        }
    }
}

fn benchmark_duration(duration_seconds: Option<u64>) -> Duration {
    Duration::from_secs(
        duration_seconds
            .unwrap_or(DEFAULT_DURATION_SECS)
            .clamp(1, MAX_DURATION_SECS),
    )
}

fn effective_window(window_size: Option<usize>) -> usize {
    transfer_tuning::normalize_transfer_window_override(window_size)
        .unwrap_or(transfer_tuning::DEFAULT_BINARY_V1_WINDOW)
}

fn mbps(bytes: u64, duration: Duration) -> f64 {
    let seconds = duration.as_secs_f64();
    if seconds <= 0.0 {
        0.0
    } else {
        bytes as f64 / 1_000_000.0 / seconds
    }
}

fn round_metric(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn raw_memory_discard_does_not_write_files() {
        let temp_dir = std::env::temp_dir().join(format!(
            "pastey_loopback_benchmark_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&temp_dir).unwrap();
        let before = fs::read_dir(&temp_dir).unwrap().count();

        let received =
            discard_benchmark_payload(BenchmarkMode::RawMemory, &vec![0x5a; RAW_CHUNK_BYTES])
                .unwrap();

        let after = fs::read_dir(&temp_dir).unwrap().count();
        assert_eq!(before, after);
        assert_eq!(received, RAW_CHUNK_BYTES as u64);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn pipeline_payload_encrypts_then_discards_in_memory() {
        let payload = benchmark_payload(BenchmarkMode::PasteyPipeline, 7).unwrap();
        let received = discard_benchmark_payload(BenchmarkMode::PasteyPipeline, &payload).unwrap();

        assert_eq!(received, PIPELINE_CHUNK_BYTES as u64);
    }
}
