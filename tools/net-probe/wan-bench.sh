#!/bin/sh
# Runs the wan-bench matrix (examples/wan_bench.rs) and appends one JSON line
# per run to the file named by $1 (default results/wan-bench-emulated.jsonl).
#
# Everything here is an emulation on loopback: two iroh endpoints in one
# process behind a user-space latency/rate/loss shaper. It shows how the
# software behaves under given conditions; it is not WAN evidence.
#
#   cargo build --release --example wan_bench --manifest-path tools/net-probe/Cargo.toml
#   sh tools/net-probe/wan-bench.sh
#   python3 -I tools/net-probe/wan-bench-summarize.py tools/net-probe/results/wan-bench-emulated.jsonl
#
# Set WAN_GROUPS to run a subset, e.g. WAN_GROUPS="A F".
set -eu
cd "$(dirname "$0")"
BIN=target/release/examples/wan_bench
OUT=${1:-results/wan-bench-emulated.jsonl}
WAN_GROUPS=${WAN_GROUPS:-"A B C D E F G H I J K"}
mkdir -p "$(dirname "$OUT")"

run() { "$BIN" "$@" >>"$OUT" || echo "run failed: $*" >&2; }
want() { case " $WAN_GROUPS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# A. Raw QUIC, one stream, unlimited link: latency alone, direct and relay.
if want A; then
  for mode in direct relay; do
    for rtt in 0 50 100 200 350; do
      run label=A-raw-1stream mode=$mode work=raw rtt_ms=$rtt mib=128 max_s=120
    done
  done
fi

# B. 350 ms: one stream, eight streams, and one stream with an 8 MiB window.
if want B; then
  for mode in direct relay; do
    run label=B-raw-8streams mode=$mode work=raw rtt_ms=350 streams=8 mib=256 max_s=120
    run label=B-raw-1stream-rwnd8MiB mode=$mode work=raw rtt_ms=350 rwnd_kib=8192 mib=256 max_s=120
  done
fi

# C. The shape of the earlier public-relay probe: 8 MiB only.
if want C; then
  run label=C-raw-8MiB-sample mode=relay work=raw rtt_ms=350 mib=8 max_s=60
  run label=C-raw-8MiB-sample mode=relay work=raw rtt_ms=350 rate_kib=1126 mib=8 max_s=60
  run label=C-raw-64MiB-sample mode=relay work=raw rtt_ms=350 rate_kib=1126 mib=64 max_s=120
fi

# D. Pastey-shaped transfer at 350 ms, unlimited link: window 1, 8, 16.
if want D; then
  for mode in direct relay; do
    for window in 1 8 16; do
      run label=D-pastey-window mode=$mode work=pastey rtt_ms=350 window=$window mib=256 max_s=300
    done
  done
fi

# E. A 1.1 MiB/s link at 350 ms: raw against Pastey-shaped.
if want E; then
  run label=E-1.1MiB-link mode=relay work=raw rtt_ms=350 rate_kib=1126 streams=1 mib=64 max_s=200 &
  run label=E-1.1MiB-link mode=relay work=raw rtt_ms=350 rate_kib=1126 streams=8 mib=64 max_s=200 &
  run label=E-1.1MiB-link mode=relay work=pastey rtt_ms=350 rate_kib=1126 mib=64 max_s=300 &
  run label=E-1.1MiB-link mode=direct work=pastey rtt_ms=350 rate_kib=1126 mib=64 max_s=300 &
  wait
fi

# F. A slow uplink (hotspot-like, 150 ms): Pastey's defaults, then smaller
#    windows and chunks, which need no protocol change.
if want F; then
  for rate in 384 256 128; do
    run label=F-slow-default mode=relay work=pastey rtt_ms=150 rate_kib=$rate mib=32 max_s=900 &
  done
  run label=F-slow-default mode=direct work=pastey rtt_ms=150 rate_kib=256 mib=32 max_s=900 &
  run label=F-slow-window2 mode=relay work=pastey rtt_ms=150 rate_kib=256 window=2 mib=32 max_s=900 &
  run label=F-slow-window2 mode=relay work=pastey rtt_ms=150 rate_kib=128 window=2 mib=32 max_s=900 &
  run label=F-slow-chunk512K mode=relay work=pastey rtt_ms=150 rate_kib=256 chunk_kib=512 mib=32 max_s=900 &
  run label=F-slow-chunk512K mode=relay work=pastey rtt_ms=150 rate_kib=128 chunk_kib=512 mib=32 max_s=900 &
  run label=F-slow-raw mode=relay work=raw rtt_ms=150 rate_kib=256 mib=32 max_s=900 &
  wait
fi

# G. File sizes at 350 ms, unlimited link.
if want G; then
  for mode in direct relay; do
    for mib in 1 16 64 512; do
      run label=G-size mode=$mode work=pastey rtt_ms=350 mib=$mib max_s=300
      run label=G-size mode=$mode work=raw rtt_ms=350 mib=$mib max_s=300
    done
  done
fi

# H. Loss on the direct path (100 ms).
if want H; then
  for loss in 0.001 0.01; do
    run label=H-loss mode=direct work=raw rtt_ms=100 loss=$loss mib=64 max_s=300 &
    run label=H-loss mode=direct work=raw rtt_ms=100 loss=$loss streams=8 mib=64 max_s=300 &
    run label=H-loss mode=direct work=pastey rtt_ms=100 loss=$loss mib=64 max_s=300 &
  done
  wait
fi

# I. Outages during a Pastey-shaped transfer (100 ms, 2 MiB/s, 128 MiB).
if want I; then
  for mode in direct relay; do
    for lasts in 5 20 40; do
      run label=I-outage mode=$mode work=pastey rtt_ms=100 rate_kib=2048 mib=128 \
        outage_at_s=15 outage_s=$lasts outage_kind=stall max_s=600 &
    done
  done
  run label=I-outage mode=relay work=pastey rtt_ms=100 rate_kib=2048 mib=128 \
    outage_at_s=15 outage_s=5 outage_kind=kill max_s=600 &
  run label=I-outage-raw mode=direct work=raw rtt_ms=100 rate_kib=2048 mib=128 \
    outage_at_s=15 outage_s=20 outage_kind=stall max_s=600 &
  run label=I-outage-raw mode=direct work=raw rtt_ms=100 rate_kib=2048 mib=128 \
    outage_at_s=15 outage_s=40 outage_kind=stall max_s=600 &
  wait
fi

# J. BBRv3 instead of iroh's default Cubic, on the cases Cubic handled badly:
#    random loss (H), a slow relay path (F) and a 1.1 MiB/s relay path (E).
if want J; then
  run label=J-bbr3-loss mode=direct work=raw rtt_ms=100 loss=0.001 cc=bbr3 mib=64 max_s=300 &
  run label=J-bbr3-loss mode=direct work=raw rtt_ms=100 loss=0.01 cc=bbr3 mib=64 max_s=300 &
  run label=J-bbr3-loss mode=direct work=pastey rtt_ms=100 loss=0.01 cc=bbr3 mib=64 max_s=300 &
  wait
  run label=J-bbr3-slow mode=relay work=raw rtt_ms=150 rate_kib=256 cc=bbr3 mib=32 max_s=900 &
  run label=J-bbr3-slow mode=relay work=pastey rtt_ms=150 rate_kib=256 cc=bbr3 mib=32 max_s=900 &
  run label=J-bbr3-slow mode=relay work=pastey rtt_ms=150 rate_kib=256 window=2 cc=bbr3 mib=32 max_s=900 &
  run label=J-bbr3-slow mode=direct work=pastey rtt_ms=150 rate_kib=256 window=2 cc=bbr3 mib=32 max_s=900 &
  run label=J-bbr3-1.1MiB mode=relay work=pastey rtt_ms=350 rate_kib=1126 cc=bbr3 mib=64 max_s=300 &
  wait
  run label=J-bbr3-unlimited mode=direct work=pastey rtt_ms=350 cc=bbr3 mib=256 max_s=300
  run label=J-bbr3-unlimited mode=relay work=pastey rtt_ms=350 cc=bbr3 mib=256 max_s=300
  run label=J-bbr3-unlimited mode=direct work=raw rtt_ms=0 cc=bbr3 mib=128 max_s=120
fi

# K. Pastey's tunnel window (adaptive=1) against the fixed window 8:
#    slow links (F), a 1.1 MiB/s link (E), and unlimited links where
#    starting small must not cost much (D, G).
if want K; then
  for rate in 384 256 128 64; do
    run label=K-adaptive-slow mode=relay work=pastey rtt_ms=150 rate_kib=$rate adaptive=1 mib=32 max_s=900 &
  done
  run label=K-adaptive-slow mode=direct work=pastey rtt_ms=150 rate_kib=256 adaptive=1 mib=32 max_s=900 &
  run label=K-adaptive-slow mode=direct work=pastey rtt_ms=150 rate_kib=64 adaptive=1 mib=32 max_s=900 &
  run label=K-adaptive-1.1MiB mode=relay work=pastey rtt_ms=350 rate_kib=1126 adaptive=1 mib=64 max_s=300 &
  run label=K-adaptive-1.1MiB mode=direct work=pastey rtt_ms=350 rate_kib=1126 adaptive=1 mib=64 max_s=300 &
  wait
  for mode in direct relay; do
    for mib in 16 64 256; do
      run label=K-adaptive-unlimited mode=$mode work=pastey rtt_ms=350 adaptive=1 mib=$mib max_s=300
    done
    run label=K-adaptive-unlimited mode=$mode work=pastey rtt_ms=50 adaptive=1 mib=256 max_s=300
    run label=K-fixed-unlimited mode=$mode work=pastey rtt_ms=50 mib=256 max_s=300
  done
fi
