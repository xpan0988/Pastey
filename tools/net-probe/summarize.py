"""Summarize net-probe JSON lines into the Stage 2 matrix.

Usage: python3 -I summarize.py results/*.jsonl

Prints one row per matrix cell and lists the cells with no runs. Runs whose
`single_machine` is true are reported separately: they are never cross-NAT
evidence.
"""

import json
import statistics
import sys

CELLS = {
    "C1": "Mac <-> Windows, same Wi-Fi (LAN baseline, Windows firewall)",
    "C2": "home Wi-Fi <-> phone hotspot (CGNAT)",
    "C3": "home <-> university or corporate network",
    "C4": "hotspot <-> hotspot (two carriers)",
    "C5": "Sydney <-> mainland China",
    "C6": "UDP blocked on one side",
    "R1": "network switch during watch (Wi-Fi -> hotspot)",
    "R2": "sleep and wake during watch",
    "R3": "peer restart during watch",
    "M1": "two dialers on different networks into one listener",
}


def median(values):
    values = [value for value in values if value is not None]
    return round(statistics.median(values), 1) if values else None


def load(paths):
    runs = []
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if line.startswith("{"):
                    runs.append(json.loads(line))
    return runs


def row(cell, runs):
    connected = [run for run in runs if run["connect"]["ok"]]
    direct = [run for run in connected if run["connect"].get("time_to_direct_ms") is not None]
    bulk = [run["bulk"] for run in connected if run.get("bulk")]
    http = [run["http"] for run in connected if run.get("http")]
    watches = [run["watch"] for run in connected if run.get("watch")]
    outages = [outage for watch in watches for outage in watch["outages"]]
    pairs = sorted({
        f'{run["local"]["os"]}:{run["local"]["net_desc"]} -> '
        f'{(run.get("peer") or {}).get("os", "?")}:{(run.get("peer") or {}).get("net_desc", "?")}'
        for run in runs
    })
    return {
        "cell": cell,
        "runs": len(runs),
        "connected": len(connected),
        "rejected": sum(1 for run in runs if run["connect"].get("rejected")),
        "direct": len(direct),
        "direct_rate": round(len(direct) / len(connected), 2) if connected else None,
        "median_time_to_direct_ms": median([run["connect"]["time_to_direct_ms"] for run in direct]),
        "final_relay": sum(1 for run in connected if run["connect"].get("final_path") == "relay"),
        "median_mib_per_s": median([item.get("mib_per_s") for item in bulk]),
        "digest_failures": sum(1 for item in bulk if not item.get("digest_ok")),
        "median_relay_share_of_bulk_bytes": median([
            item["relay_tx_bytes"] / max(1, item["relay_tx_bytes"] + item["ip_tx_bytes"])
            for item in bulk
        ]),
        "median_control_p99_ms": median([item.get("control_p99_ms") for item in bulk]),
        "http_ok": sum(1 for item in http if item.get("ok")),
        "http_runs": len(http),
        "watch_runs": len(watches),
        "outages": len(outages),
        "recovered_same_connection": sum(1 for o in outages if o.get("recovered_by") == "same_connection"),
        "recovered_by_redial": sum(1 for o in outages if o.get("recovered_by") == "redial"),
        "unrecovered": sum(1 for o in outages if o.get("end_ms") is None),
        "median_outage_ms": median([o.get("duration_ms") for o in outages]),
        "pairs": pairs,
    }


def main():
    runs = load(sys.argv[1:])
    physical = [run for run in runs if not run.get("single_machine")]
    single = [run for run in runs if run.get("single_machine")]
    by_cell = {}
    for run in physical:
        by_cell.setdefault(run.get("cell") or "unlabelled", []).append(run)
    print(json.dumps({
        "physical_cells": [row(cell, by_cell[cell]) for cell in sorted(by_cell)],
        "untested_cells": {cell: text for cell, text in CELLS.items() if cell not in by_cell},
        "single_machine_not_cross_nat": [
            row(desc, [run for run in single if run["local"]["net_desc"] == desc])
            for desc in sorted({run["local"]["net_desc"] for run in single})
        ],
    }, indent=1))


if __name__ == "__main__":
    main()
