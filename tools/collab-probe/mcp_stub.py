"""A stand-in for Pastey's per-run `send` tool: a stdio MCP server whose one
tool appends its arguments to a spool file and answers at once.

Research only (see probe.py). It records the names, never the values, of
the CLAUDE_* and CODEX_* environment variables it was started with, so the
probe can tell what identity a native Agent gives an MCP server.

    python3 -I mcp_stub.py <spool-file>
"""

import json
import os
import sys

SPOOL = sys.argv[1]

TOOL = {
    "name": "send",
    "description": (
        "Send a message to an Agent session this session is linked with. "
        "Pastey delivers it when your turn ends."
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "to": {"type": "string", "description": "The linked peer's alias."},
            "kind": {"type": "string", "enum": ["request", "info"]},
            "text": {"type": "string"},
        },
        "required": ["text"],
    },
    # It writes to the spool: not read-only, but nothing is destroyed.
    "annotations": {
        "readOnlyHint": False,
        "destructiveHint": False,
        "idempotentHint": False,
        "openWorldHint": False,
    },
}


def record(entry):
    with open(SPOOL, "a", encoding="utf-8") as spool:
        spool.write(json.dumps(entry) + "\n")


def reply(message_id, result=None, error=None):
    payload = {"jsonrpc": "2.0", "id": message_id}
    if error is not None:
        payload["error"] = error
    else:
        payload["result"] = result
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


record({
    "event": "started",
    "envNames": sorted(
        name for name in os.environ if name.startswith(("CLAUDE", "CODEX"))
    ),
})

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method = message.get("method")
    message_id = message.get("id")
    if message_id is None:
        continue  # a notification
    if method == "initialize":
        params = message.get("params") or {}
        reply(message_id, {
            "protocolVersion": params.get("protocolVersion", "2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pastey-probe", "version": "0"},
        })
    elif method == "tools/list":
        reply(message_id, {"tools": [TOOL]})
    elif method == "tools/call":
        params = message.get("params") or {}
        record({"event": "call", "tool": params.get("name"),
                "arguments": params.get("arguments") or {}})
        reply(message_id, {"content": [
            {"type": "text", "text": "Queued. Pastey delivers it when your turn ends."}
        ]})
    elif method == "ping":
        reply(message_id, {})
    else:
        reply(message_id, error={"code": -32601, "message": f"unknown method {method}"})
