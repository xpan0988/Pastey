#!/usr/bin/env python3
"""Observer: poll the P02 collaboration through the bridge's IPC (get_collaboration); append new/changed entries and print one line each."""
import json, os, sys, time, datetime, urllib.request
C = "collaboration:cdf86f40-d2ef-4745-8195-eb8857272ffa"
H = os.path.dirname(os.path.abspath(__file__))
OUT, STATE = os.path.join(H, "journal.jsonl"), os.path.join(H, ".journal_seen.json")
seen = json.load(open(STATE)) if os.path.exists(STATE) else {}
def get():
    req = urllib.request.Request("http://127.0.0.1:17431/ipc", data=json.dumps({"cmd": "get_collaboration", "args": {"collaborationId": C}}).encode(), headers={"content-type": "application/json"})
    return json.load(urllib.request.urlopen(req, timeout=20))["ok"]
def line(e):
    b = e.get("body", {}); t = b.get("type"); st = e.get("status") or {}
    ts = datetime.datetime.fromtimestamp(e["at"]).strftime("%H:%M:%S")
    if t == "message": s = f'{b.get("from")}->{b.get("to")} hop{b.get("hop")} [{st.get("state")}] {b.get("subject","")[:90]}'
    elif t == "turn": s = f'turn {e.get("member")} {b.get("state")} auto={b.get("automatic")} files={b.get("files",{}).get("total")}'
    elif t == "summary": s = f'summary {e.get("member")}: ' + json.dumps(b.get("summary"))[:150]
    else: s = f'{t} {e.get("member")} ' + json.dumps({k: v for k, v in b.items() if k != "type"})[:200]
    return f'#{e["seq"]} {ts} {s}'
while True:
    try:
        v = get(); json.dump(v, open(os.path.join(H, "view-latest.json"), "w"))
        out = []
        for e in v["entries"]:
            js = json.dumps(e, sort_keys=True)
            if seen.get(str(e["seq"])) != js:
                seen[str(e["seq"])] = js
                open(OUT, "a").write(js + "\n"); out.append(line(e))
        if out:
            json.dump(seen, open(STATE, "w")); print("\n".join(out), flush=True)
    except Exception as ex:
        print("poll error:", ex, flush=True)
    if sys.argv[1:] == ["once"]: break
    time.sleep(10)
