#!/usr/bin/env python3
"""Prints wan-bench results (examples/wan_bench.rs) as Markdown tables.

    python3 -I tools/net-probe/wan-bench-summarize.py results/wan-bench-emulated.jsonl

Every row is an emulated loopback run, never WAN evidence.
"""
import json
import sys
from collections import OrderedDict


def cell(value):
    if value is None:
        return "–"
    if isinstance(value, bool):
        return "yes" if value else "NO"
    return str(value)


def main():
    runs = []
    for path in sys.argv[1:]:
        with open(path) as handle:
            runs += [json.loads(line) for line in handle if line.strip()]
    groups = OrderedDict()
    for run in sorted(runs, key=lambda r: r["label"]):
        groups.setdefault(run["label"].split("-")[0], []).append(run)
    head = [
        "label", "path", "work", "RTT ms", "link KiB/s", "loss", "MiB",
        "streams/window", "chunk KiB", "ok", "s", "MiB/s", "2nd-half MiB/s",
        "measured RTT", "chunk p50/max s", "retries/timeouts",
        "probe p50/max ms", "probe fails (worst run)", "longest stall s", "note",
    ]
    for group, rows in groups.items():
        print(f"\n### {group}\n")
        print("| " + " | ".join(head) + " |")
        print("|" + "---|" * len(head))
        for r in rows:
            pastey = r["work"] == "pastey"
            chunks, probes, path = r["chunks"], r["probes"], r.get("path") or {}
            notes = []
            if r.get("rwnd_kib"):
                notes.append(f"stream window {r['rwnd_kib']} KiB")
            if r.get("adaptive"):
                a = r["adaptive"]
                notes.append(f"adaptive window (lowest {a['lowest']}, last {a['last']})")
            if r.get("cc", "cubic") != "cubic":
                notes.append(f"cc {r['cc']}")
            if r.get("outage"):
                o = r["outage"]
                notes.append(f"{o['kind']} {o['lasts_s']:g} s at {o['at_s']:g} s")
            if probes.get("session_invalidated_at_s") is not None:
                notes.append(f"session invalidated at {probes['session_invalidated_at_s']} s")
            if probes.get("longest_gap_between_arrivals_s") is not None:
                notes.append(
                    f"{probes['arrived_at_receiver']} pings reached the receiver, "
                    f"longest gap {probes['longest_gap_between_arrivals_s']} s"
                )
            if r.get("carrier_dials", 1) > 1:
                notes.append(f"{r['carrier_dials'] - 1} redial(s)")
            if r.get("error"):
                notes.append(r["error"])
            print("| " + " | ".join(cell(v) for v in [
                r["label"], r["mode"], r["work"], r["rtt_ms"],
                r["rate_kib"] or "unlimited", r["loss"] or 0, r["mib"],
                r["window"] if pastey else r["streams"],
                r["chunk_kib"] if pastey else None,
                r["ok"], r["seconds"], r["mib_per_s"], r["second_half_mib_per_s"],
                path.get("rtt_ms"),
                f"{cell(chunks['p50_s'])}/{cell(chunks['max_s'])}" if pastey else None,
                f"{chunks['retries']}/{chunks['timeouts']}" if pastey else None,
                f"{cell(probes['p50_ms'])}/{cell(probes['max_ms'])}" if pastey else None,
                f"{probes['failed']} ({probes['worst_failure_run']})" if pastey else None,
                r["longest_no_progress_s"], "; ".join(notes),
            ]) + " |")


if __name__ == "__main__":
    main()
