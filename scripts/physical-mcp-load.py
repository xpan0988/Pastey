#!/usr/bin/env python3
"""Scripted (no LLM) MCP load client for a Physical decision stream.

Speaks MCP over stdio to `pastey --physical-mcp <grant>` and issues tool calls
at a fixed cadence: `observe` and `remaining_budget` (neither consumes action
budget), with a short approved action every N calls. Reports outcomes,
refusal reasons, rate-limit rejections and latencies.

With --brain-log/--executor-log (the Hosts' stderr while TEMP-TRACE is
compiled in), it also reports Room Control events received per tool call on
each Host, by kind, from the `rc_events={...}` totals before and after.

Example:
  python3 -B scripts/physical-mcp-load.py \
      --binary src-tauri/target/debug/pastey --grant /path/to/grant.json \
      --calls 150 --interval 1.5 --action-every 10 --option turn_left \
      --brain-log brain.log --executor-log executor.log
"""
import argparse
import json
import re
import statistics
import subprocess
import sys
import time

RATE_MARKERS = ("rate_limited", "rate limit", "flow-control", "rate exceeded")


def rc_events(path):
    """The last `rc_events={...}` totals in a Host's stderr log, or None."""
    if not path:
        return None
    last = None
    try:
        with open(path, encoding="utf-8", errors="replace") as log:
            for line in log:
                match = re.search(r"rc_events=(\{[^}]*\})", line)
                if match:
                    last = match.group(1)
    except FileNotFoundError:
        return None
    return json.loads(last) if last else {}


class Client:
    def __init__(self, command):
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1
        )
        self.next_id = 1

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method, params=None):
        request_id = self.next_id
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}})
        while True:
            line = self.process.stdout.readline()
            if not line:
                raise RuntimeError("MCP server closed the connection")
            response = json.loads(line)
            if response.get("id") == request_id:
                return response

    def close(self):
        try:
            self.process.stdin.close()
            self.process.wait(timeout=10)
        except Exception:
            self.process.kill()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", required=True)
    parser.add_argument("--grant", required=True)
    parser.add_argument("--calls", type=int, default=150)
    parser.add_argument("--interval", type=float, default=1.5, help="seconds between calls")
    parser.add_argument("--action-every", type=int, default=10, help="0 disables actions")
    parser.add_argument("--option", default="turn_left")
    parser.add_argument("--duration-ms", type=int, default=200)
    parser.add_argument("--brain-log")
    parser.add_argument("--executor-log")
    args = parser.parse_args()

    before = {"brain": rc_events(args.brain_log), "executor": rc_events(args.executor_log)}
    client = Client([args.binary, "--physical-mcp", args.grant])
    init = client.request(
        "initialize",
        {"protocolVersion": "2025-06-18", "capabilities": {},
         "clientInfo": {"name": "load-client", "version": "1"}},
    )
    if "error" in init:
        sys.exit(f"initialize failed: {init['error']}")
    client.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    tools = [t["name"] for t in client.request("tools/list")["result"]["tools"]]
    if args.action_every and args.option not in tools:
        sys.exit(f"option {args.option!r} is not an approved tool: {tools}")

    results = []
    started = time.monotonic()
    for n in range(args.calls):
        due = started + n * args.interval
        time.sleep(max(0.0, due - time.monotonic()))
        if args.action_every and n % args.action_every == args.action_every - 1:
            name, arguments = args.option, {"durationMs": args.duration_ms}
        else:
            name, arguments = ("observe" if n % 2 == 0 else "remaining_budget"), {}
        t0 = time.monotonic()
        response = client.request("tools/call", {"name": name, "arguments": arguments})
        latency = time.monotonic() - t0
        if "error" in response:
            outcome, reason = "mcp_error", response["error"].get("message", "")
        else:
            body = response["result"].get("structuredContent") or {}
            outcome = body.get("result", "ok")
            if outcome in ("observation", "budget"):
                outcome = "ok"
            reason = body.get("reason", "")
            if outcome == "allowed" and body.get("disposition") != "accepted":
                reason = f"disposition={body.get('disposition')}"
        results.append({"n": n, "tool": name, "outcome": outcome, "reason": reason,
                        "latency_ms": round(latency * 1000, 1)})
        print(json.dumps(results[-1]), flush=True)
        if "Stream ended" in reason or "closed" in reason.lower():
            print("stream ended; stopping", flush=True)
            break
    client.close()

    latencies = [r["latency_ms"] for r in results]
    reasons = {}
    for r in results:
        if r["reason"]:
            reasons[r["reason"]] = reasons.get(r["reason"], 0) + 1
    by_tool = {}
    for r in results:
        key = f"{r['tool']}:{r['outcome']}"
        by_tool[key] = by_tool.get(key, 0) + 1
    summary = {
        "calls": len(results),
        "duration_s": round(time.monotonic() - started, 1),
        "by_tool_and_outcome": by_tool,
        "reasons": reasons,
        "rate_limit_rejections": sum(
            1 for r in results if any(m in r["reason"].lower() for m in RATE_MARKERS)),
        "latency_ms": {
            "p50": statistics.median(latencies) if latencies else None,
            "p95": sorted(latencies)[int(len(latencies) * 0.95) - 1] if latencies else None,
            "max": max(latencies) if latencies else None,
        },
    }
    after = {"brain": rc_events(args.brain_log), "executor": rc_events(args.executor_log)}
    per_call = {}
    for host in ("brain", "executor"):
        if before[host] is not None and after[host] is not None and results:
            per_call[host] = {
                kind: round((count - before[host].get(kind, 0)) / len(results), 2)
                for kind, count in sorted(after[host].items())
                if count != before[host].get(kind, 0)
            }
    if per_call:
        summary["room_control_events_per_call"] = per_call
    print(json.dumps(summary, indent=2))
    sys.exit(1 if summary["rate_limit_rejections"] else 0)


if __name__ == "__main__":
    main()
