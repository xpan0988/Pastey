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
    host_runtime::HostRuntime as AppState,
    logging, storage,
    transfer::BridgePeerTransferEndpoint,
    transfer_tuning,
};

const DEFAULT_DURATION_SECS: u64 = 5;
const MAX_DURATION_SECS: u64 = 15;
const RAW_CHUNK_BYTES: usize = 256 * 1024;
const PIPELINE_CHUNK_BYTES: usize = 256 * 1024;
const BENCHMARK_KEY: [u8; 32] = [42; 32];
/// The shortest interval the peak is measured over.
const PEAK_SAMPLE: Duration = Duration::from_millis(500);

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
    let mut spans = Vec::new();
    let mut total_bytes = 0u64;
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
        let issued = started_at.elapsed();
        stream.write_all(&len.to_be_bytes()).await?;
        stream.write_all(&payload).await?;
        let plaintext_bytes = plaintext_len_for_payload(mode, &payload)?;
        total_bytes = total_bytes.saturating_add(plaintext_bytes);
        spans.push(ChunkSpan {
            issued,
            acknowledged: started_at.elapsed(),
            bytes: plaintext_bytes,
        });
        chunk_index = chunk_index.saturating_add(1);
    }

    stream.shutdown().await?;
    let _received_bytes = receiver
        .await
        .map_err(|_| AppError::Network("Loopback benchmark interrupted.".into()))??;
    let elapsed = started_at.elapsed();
    let duration_ms = elapsed.as_millis().max(1) as u64;
    let peak_mbps = peak_mbps(&spans, elapsed);
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
    let peer_key = room
        .peer_transport_public_key
        .as_deref()
        .ok_or_else(|| AppError::NotFound("Peer is not connected.".into()))?;
    let signer = crate::bridge_auth::PeerRequestSigner::for_room(&state, &room_id, peer_key)?;
    run_peer_link_benchmark_at(
        signer,
        format!("http://{peer_host}:{peer_port}/rooms/{room_id}/diagnostics"),
        Some(room_id),
        room.peer_device_name,
        mode,
        duration_seconds,
        window_size,
        sender_cpu_hint,
    )
    .await
}

pub(crate) async fn run_peer_link_benchmark_for_endpoint(
    signer: crate::bridge_auth::PeerRequestSigner,
    endpoint: BridgePeerTransferEndpoint,
    bridge_id: String,
    mode: BenchmarkMode,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    sender_cpu_hint: Option<String>,
) -> AppResult<LinkBenchmarkResult> {
    run_peer_link_benchmark_at(
        signer,
        format!(
            "http://{}:{}/rooms/{bridge_id}/diagnostics",
            endpoint.host, endpoint.port
        ),
        Some(bridge_id),
        None,
        mode,
        duration_seconds,
        window_size,
        sender_cpu_hint,
    )
    .await
}

async fn run_peer_link_benchmark_at(
    signer: crate::bridge_auth::PeerRequestSigner,
    base_url: String,
    peer_id: Option<String>,
    peer_name: Option<String>,
    mode: BenchmarkMode,
    duration_seconds: Option<u64>,
    window_size: Option<usize>,
    sender_cpu_hint: Option<String>,
) -> AppResult<LinkBenchmarkResult> {
    let client = reqwest::Client::new();
    let latency_ms = estimate_peer_latency(&client, &signer, &base_url)
        .await
        .ok();
    let duration = benchmark_duration(duration_seconds);
    let window = effective_window(window_size);
    let url = match mode {
        BenchmarkMode::RawMemory => format!("{base_url}/benchmark/raw"),
        BenchmarkMode::PasteyPipeline => format!("{base_url}/benchmark/pipeline"),
    };
    let started_at = Instant::now();
    let mut spans = Vec::new();
    let mut total_bytes = 0u64;
    let mut failed_chunks = 0u64;
    let mut chunk_index = 0u64;
    let mut in_flight = FuturesUnordered::new();
    let mut receiver_cpu_hint = None;
    let mut record = |result: AppResult<(u64, Option<String>, Duration)>| match result {
        Ok((bytes, hint, issued)) => {
            total_bytes = total_bytes.saturating_add(bytes);
            spans.push(ChunkSpan {
                issued,
                acknowledged: started_at.elapsed(),
                bytes,
            });
            if receiver_cpu_hint.is_none() {
                receiver_cpu_hint = hint;
            }
        }
        Err(_) => failed_chunks = failed_chunks.saturating_add(1),
    };

    // Peer diagnostics use the trusted room's LAN endpoint and discard payloads
    // in memory. They are a network baseline, not a real file transfer path.
    // Chunks are issued only while `duration` lasts; the run then waits for
    // those in flight, so on a slow link it lasts as long as they take.
    while started_at.elapsed() < duration {
        while in_flight.len() < window && started_at.elapsed() < duration {
            let payload = benchmark_payload(mode, chunk_index)?;
            let plaintext_bytes = plaintext_len_for_payload(mode, &payload)?;
            let client = client.clone();
            let url = url.clone();
            let (request, _) = signer.apply(client.post(url), benchmark_route(mode), &payload);
            let issued = started_at.elapsed();
            in_flight.push(async move {
                let response = request.body(payload).send().await?;
                if !response.status().is_success() {
                    return Err(AppError::Network("Peer benchmark chunk rejected.".into()));
                }
                let ack = response.json::<BenchmarkDiscardResponse>().await?;
                Ok::<_, AppError>((plaintext_bytes, ack.receiver_cpu_hint, issued))
            });
            chunk_index = chunk_index.saturating_add(1);
        }

        let Some(result) = in_flight.next().await else {
            break;
        };
        record(result);
    }

    while let Some(result) = in_flight.next().await {
        record(result);
    }

    let elapsed = started_at.elapsed();
    let duration_ms = elapsed.as_millis().max(1) as u64;
    let peak_mbps = peak_mbps(&spans, elapsed);
    let average_mbps = mbps(total_bytes, Duration::from_millis(duration_ms));
    logging::write_transfer_line(&format!(
        "[pastey diagnostics] event=link_benchmark_summary bridge_id={} mode={} window={window} {} failed_chunks={failed_chunks} total_bytes={total_bytes} duration_ms={duration_ms} latency_ms={} average_MBps={average_mbps:.2} peak_MBps={peak_mbps:.2}",
        peer_id.as_deref().unwrap_or("unknown"),
        benchmark_route(mode),
        ChunkTiming::log_fields(&spans),
        latency_ms.map(|latency| format!("{latency:.1}")).unwrap_or_else(|| "unknown".into()),
    ));

    Ok(LinkBenchmarkResult {
        peer_id,
        peer_name,
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

async fn estimate_peer_latency(
    client: &reqwest::Client,
    signer: &crate::bridge_auth::PeerRequestSigner,
    base_url: &str,
) -> AppResult<f64> {
    let started = Instant::now();
    let (request, _) = signer.apply(client.post(format!("{base_url}/ping")), "ping", b"");
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(AppError::Network("Peer benchmark ping rejected.".into()));
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

/// The authenticated route name of one benchmark mode.
pub(crate) fn benchmark_route(mode: BenchmarkMode) -> &'static str {
    match mode {
        BenchmarkMode::RawMemory => "benchmark/raw",
        BenchmarkMode::PasteyPipeline => "benchmark/pipeline",
    }
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

/// One chunk the receiver acknowledged: when it was issued and when its
/// acknowledgement arrived, from the start of the run, and its bytes.
#[derive(Clone, Copy, Debug)]
struct ChunkSpan {
    issued: Duration,
    acknowledged: Duration,
    bytes: u64,
}

/// The highest rate over equal consecutive intervals of the run, each at
/// least `PEAK_SAMPLE` long (a run shorter than that is one interval).
///
/// The sender sees a chunk only twice, when it issues it and when the
/// receiver acknowledges it, so its bytes are counted spread evenly over
/// that span. Counting them at the acknowledgement instead turns chunks that
/// shared a slow link and finished together into a burst that never
/// happened. The intervals split the same bytes over the same run, so the
/// peak is never below the average.
fn peak_mbps(spans: &[ChunkSpan], run: Duration) -> f64 {
    let run = run.as_secs_f64();
    if run <= 0.0 {
        return 0.0;
    }
    let intervals = ((run / PEAK_SAMPLE.as_secs_f64()).floor() as usize).max(1);
    let width = run / intervals as f64;
    let mut bytes = vec![0.0f64; intervals];
    for span in spans {
        let start = span.issued.as_secs_f64().clamp(0.0, run);
        let end = span.acknowledged.as_secs_f64().clamp(start, run);
        let first = ((start / width) as usize).min(intervals - 1);
        if end <= start {
            bytes[first] += span.bytes as f64;
            continue;
        }
        let per_second = span.bytes as f64 / (end - start);
        let last = ((end / width).ceil() as usize).clamp(first + 1, intervals);
        for (index, interval) in bytes.iter_mut().enumerate().take(last).skip(first) {
            let overlap = ((index + 1) as f64 * width).min(end) - (index as f64 * width).max(start);
            if overlap > 0.0 {
                *interval += per_second * overlap;
            }
        }
    }
    bytes
        .into_iter()
        .map(|interval| interval / 1_000_000.0 / width)
        .fold(0.0, f64::max)
}

/// When acknowledgements arrived and how long chunks took, for the
/// diagnostics log, with no content. It shows whether the run cycled through
/// many short chunks or only ever had its first window in flight (every
/// chunk taking about the whole run). Acknowledgements alone cannot say
/// whether such a link was slow throughout or stalled and then caught up;
/// the path counters logged beside a Bridge benchmark can.
#[derive(Debug, PartialEq)]
struct ChunkTiming {
    chunks: usize,
    first_ack_ms: u128,
    last_ack_ms: u128,
    shortest_chunk_ms: u128,
    longest_chunk_ms: u128,
}

impl ChunkTiming {
    fn of(spans: &[ChunkSpan]) -> Option<Self> {
        let lifetime = |span: &ChunkSpan| span.acknowledged.saturating_sub(span.issued).as_millis();
        Some(Self {
            chunks: spans.len(),
            first_ack_ms: spans
                .iter()
                .map(|span| span.acknowledged)
                .min()?
                .as_millis(),
            last_ack_ms: spans
                .iter()
                .map(|span| span.acknowledged)
                .max()?
                .as_millis(),
            shortest_chunk_ms: spans.iter().map(lifetime).min()?,
            longest_chunk_ms: spans.iter().map(lifetime).max()?,
        })
    }

    fn log_fields(spans: &[ChunkSpan]) -> String {
        match Self::of(spans) {
            Some(timing) => format!(
                "chunks={} first_ack_ms={} last_ack_ms={} shortest_chunk_ms={} longest_chunk_ms={}",
                timing.chunks,
                timing.first_ack_ms,
                timing.last_ack_ms,
                timing.shortest_chunk_ms,
                timing.longest_chunk_ms
            ),
            None => "chunks=0".into(),
        }
    }
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

    fn span(issued_ms: u64, acknowledged_ms: u64, bytes: u64) -> ChunkSpan {
        ChunkSpan {
            issued: Duration::from_millis(issued_ms),
            acknowledged: Duration::from_millis(acknowledged_ms),
            bytes,
        }
    }

    /// The 27 October 2026 Bridge measurement (Mac on an iPhone hotspot,
    /// Windows at home, direct path): one window of eight 256 KiB chunks,
    /// all issued at once, all acknowledged between 10.67 s and 10.80 s.
    fn hotspot_run() -> (Vec<ChunkSpan>, Duration) {
        let chunk = PIPELINE_CHUNK_BYTES as u64;
        let mut spans = vec![span(0, 10_670, chunk)];
        spans.extend((1..8).map(|index| span(0, 10_670 + index * 18, chunk)));
        (spans, Duration::from_millis(10_800))
    }

    /// The sampling this replaced: bytes counted when acknowledged, a
    /// sample closed on the first acknowledgement 500 ms after the last,
    /// and whatever followed the last sample divided by its own length.
    fn acknowledgement_sampled_peak(spans: &[ChunkSpan], run: Duration) -> f64 {
        let mut last_sample_at = Duration::ZERO;
        let mut sample_bytes = 0u64;
        let mut peak: f64 = 0.0;
        for span in spans {
            sample_bytes += span.bytes;
            if span.acknowledged - last_sample_at >= PEAK_SAMPLE {
                peak = peak.max(mbps(sample_bytes, span.acknowledged - last_sample_at));
                sample_bytes = 0;
                last_sample_at = span.acknowledged;
            }
        }
        peak.max(mbps(sample_bytes, run - last_sample_at))
    }

    #[test]
    fn chunks_that_finish_together_on_a_slow_link_are_not_a_peak() {
        let (spans, run) = hotspot_run();
        let total = spans.iter().map(|span| span.bytes).sum::<u64>();
        let average = mbps(total, run);
        assert_eq!(round_metric(average), 0.2);
        // What Bridge details showed: 14.1 MB/s from 7 chunks over 130 ms.
        assert_eq!(
            round_metric(acknowledgement_sampled_peak(&spans, run)),
            14.1
        );

        let peak = peak_mbps(&spans, run);
        assert!(peak >= average, "peak {peak} below average {average}");
        assert!(peak < average * 1.1, "peak {peak} for average {average}");
        assert_eq!(
            quality_label(average, Some(51.5)),
            crate::diagnostics::LinkQuality::Poor
        );

        // Every chunk took about the whole run: only the first window was
        // ever in flight. Whether the path was slow throughout or stalled
        // is for the path counters logged beside it.
        let timing = ChunkTiming::of(&spans).unwrap();
        assert_eq!(timing.chunks, 8);
        assert_eq!((timing.first_ack_ms, timing.last_ack_ms), (10_670, 10_796));
        assert!(timing.shortest_chunk_ms >= 10_000);
    }

    #[test]
    fn a_burst_seen_chunk_by_chunk_is_still_the_peak() {
        let chunk = PIPELINE_CHUNK_BYTES as u64;
        // One chunk stalls for 4.5 s, then forty finish 12.5 ms apart.
        let mut spans = vec![span(0, 4_500, chunk)];
        spans.extend((0..40u64).map(|index| span(4_500 + index * 12, 4_512 + index * 12, chunk)));
        let run = Duration::from_millis(5_000);
        let average = mbps(41 * chunk, run);
        let peak = peak_mbps(&spans, run);
        assert!(peak > 8.0 * average, "peak {peak} for average {average}");
        assert!(peak < 25.0, "peak {peak} beyond what the chunks show");
    }

    #[test]
    fn a_steady_link_peaks_at_its_average_and_a_short_run_is_one_interval() {
        let chunk = PIPELINE_CHUNK_BYTES as u64;
        // Eight in flight, each taking 40 ms, for two seconds.
        let spans = (0..400u64)
            .map(|index| span(index / 8 * 40, index / 8 * 40 + 40, chunk))
            .collect::<Vec<_>>();
        let run = Duration::from_millis(2_000);
        let average = mbps(400 * chunk, run);
        let peak = peak_mbps(&spans, run);
        assert!(
            peak >= average && peak < average * 1.05,
            "{peak} vs {average}"
        );

        let short = [span(0, 120, chunk), span(10, 300, chunk)];
        let run = Duration::from_millis(300);
        assert!((peak_mbps(&short, run) - mbps(2 * chunk, run)).abs() < 1e-9);
        assert_eq!(peak_mbps(&[], Duration::from_secs(1)), 0.0);
    }

    #[test]
    fn pipeline_payload_encrypts_then_discards_in_memory() {
        let payload = benchmark_payload(BenchmarkMode::PasteyPipeline, 7).unwrap();
        let received = discard_benchmark_payload(BenchmarkMode::PasteyPipeline, &payload).unwrap();

        assert_eq!(received, PIPELINE_CHUNK_BYTES as u64);
    }
}
