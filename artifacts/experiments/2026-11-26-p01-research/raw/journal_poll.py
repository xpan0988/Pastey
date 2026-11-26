#!/usr/bin/env python3
"""Observer tool: read-only poll of the Pastey Journal; appends new/changed entries to journal.jsonl and prints one line each."""
import json, os, sqlite3, sys, time, datetime
DB = os.path.expanduser("~/Library/Application Support/dev.localfirst.pastey/db.sqlite")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "journal.jsonl")
STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".journal_seen.json")
seen = json.load(open(STATE)) if os.path.exists(STATE) else {}
def line(e):
    b = e.get("body", {}); t = b.get("type"); st = e.get("status") or {}
    ts = datetime.datetime.fromtimestamp(e["at"]).strftime("%H:%M:%S")
    if t == "message":
        s = f'{b.get("from")}->{b.get("to")} hop{b.get("hop")} [{st.get("state")}] {b.get("subject","")[:90]}'
    elif t == "turn":
        f = b.get("files", {})
        s = f'turn {e.get("member")} {b.get("state")} code={b.get("code")} auto={b.get("automatic")} files={f.get("total")}'
    elif t == "summary":
        s = f'summary {e.get("member")}: ' + "; ".join(f"{k}={str(v)[:60]}" for k, v in b.items() if k != "type")
    else:
        s = f'{t} {e.get("member")} ' + json.dumps({k: v for k, v in b.items() if k != "type"})[:160]
    return f'#{e["seq"]} {ts} {s}'
while True:
    try:
        con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True, timeout=5)
        rows = con.execute("select seq, entry_json from collaboration_entries where collaboration_id='collaboration:00a9f8a3-8035-4c27-aca6-5cffad6eb5c4' order by seq").fetchall()
        con.close()
        out = []
        for seq, js in rows:
            if seen.get(str(seq)) != js:
                seen[str(seq)] = js
                with open(OUT, "a") as f: f.write(js + "\n")
                out.append(line(json.loads(js)))
        if out:
            json.dump(seen, open(STATE, "w"))
            print("\n".join(out), flush=True)
    except Exception as ex:
        print("poll error:", ex, flush=True)
    if len(sys.argv) > 1 and sys.argv[1] == "once": break
    time.sleep(15)
