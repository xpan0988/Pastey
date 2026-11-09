r"""Probe: does Codex's turn/interrupt end the command its turn runs, on Windows?

Drives `codex app-server --stdio` as Pastey does (initialize, thread/start,
turn/start), waits until a `ping -n 120` command is running, sends
turn/interrupt, and samples the process tree. Optional modes:

  --job      start the app-server suspended inside a Job Object (kill on close)
             and report whether the command's processes are in that job.
  --codex=P  the launcher to run (default: %APPDATA%\npm\codex.cmd)
  --after=X  what to do once turn/completed arrives:
             wait      (default) just keep sampling
             shutdown  close app-server stdin (Pastey's shutdown)
             terminate TerminateJobObject (needs --job)
"""
import ctypes, ctypes.wintypes as wt, json, os, subprocess, sys, tempfile, threading, time, queue

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
ntdll = ctypes.WinDLL("ntdll")

TH32CS_SNAPPROCESS = 0x2
class PROCESSENTRY32W(ctypes.Structure):
    _fields_ = [("dwSize", wt.DWORD), ("cntUsage", wt.DWORD), ("th32ProcessID", wt.DWORD),
                ("th32DefaultHeapID", ctypes.c_void_p), ("th32ModuleID", wt.DWORD),
                ("cntThreads", wt.DWORD), ("th32ParentProcessID", wt.DWORD),
                ("pcPriClassBase", ctypes.c_long), ("dwFlags", wt.DWORD),
                ("szExeFile", ctypes.c_wchar * 260)]
k32.CreateToolhelp32Snapshot.restype = wt.HANDLE
k32.OpenProcess.restype = wt.HANDLE
k32.CreateJobObjectW.restype = wt.HANDLE

def snapshot():
    h = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    e = PROCESSENTRY32W(); e.dwSize = ctypes.sizeof(e)
    procs = {}
    ok = k32.Process32FirstW(h, ctypes.byref(e))
    while ok:
        procs[e.th32ProcessID] = (e.th32ParentProcessID, e.szExeFile)
        ok = k32.Process32NextW(h, ctypes.byref(e))
    k32.CloseHandle(h)
    return procs

def descendants(procs, root):
    out, frontier = set(), {root}
    while frontier:
        nxt = {p for p, (pp, _) in procs.items() if pp in frontier and p not in out and p != root}
        out |= nxt; frontier = nxt
    return out

INTERESTING = ("ping", "powershell", "codex-command-runner", "conhost", "pwsh", "cmd")

def interesting(procs):
    return {p: n for p, (_, n) in procs.items() if n.lower().startswith(INTERESTING[:3])}

def in_job(pid, job):
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    h = k32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not h:
        return f"open failed ({ctypes.get_last_error()})"
    r = wt.BOOL()
    ok = k32.IsProcessInJob(wt.HANDLE(h), wt.HANDLE(job), ctypes.byref(r))
    k32.CloseHandle(h)
    return bool(r.value) if ok else f"query failed ({ctypes.get_last_error()})"

def owner_of(pids):
    if not pids:
        return {}
    ids = ",".join(str(p) for p in pids)
    ps = ("Get-CimInstance Win32_Process | Where-Object { @(" + ids + ") -contains $_.ProcessId } | "
          "ForEach-Object { $o = Invoke-CimMethod -InputObject $_ -MethodName GetOwner; "
          "'{0} {1} {2}\\{3} {4}' -f $_.ProcessId, $_.ParentProcessId, $o.Domain, $o.User, $_.Name }")
    out = subprocess.run(["powershell.exe", "-NoProfile", "-Command", ps], capture_output=True, text=True)
    return out.stdout.strip()

def main():
    args = sys.argv[1:]
    use_job = "--job" in args
    after = next((a.split("=", 1)[1] for a in args if a.startswith("--after=")), "wait")
    workspace = tempfile.mkdtemp(prefix="codex-stop-probe-")
    t0 = time.monotonic()
    log = lambda *a: print(f"[{time.monotonic()-t0:7.2f}]", *a, flush=True)

    CREATE_SUSPENDED = 0x4
    flags = CREATE_SUSPENDED if use_job else 0
    codex = next((a.split("=", 1)[1] for a in args if a.startswith("--codex=")),
                 os.path.expandvars(r"%APPDATA%\npm\codex.cmd"))
    child = subprocess.Popen([codex, "app-server", "--stdio"], cwd=workspace,
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, creationflags=flags)
    job = None
    if use_job:
        job = k32.CreateJobObjectW(None, None)
        class BASIC(ctypes.Structure):
            _fields_ = [("a", ctypes.c_int64), ("b", ctypes.c_int64), ("LimitFlags", wt.DWORD),
                        ("c", ctypes.c_size_t), ("d", ctypes.c_size_t), ("e", wt.DWORD),
                        ("f", ctypes.c_size_t), ("g", wt.DWORD), ("h", wt.DWORD)]
        class IO(ctypes.Structure):
            _fields_ = [("x", ctypes.c_uint64 * 6)]
        class EXT(ctypes.Structure):
            _fields_ = [("Basic", BASIC), ("Io", IO), ("p", ctypes.c_size_t), ("j", ctypes.c_size_t),
                        ("pp", ctypes.c_size_t), ("pj", ctypes.c_size_t)]
        info = EXT(); info.Basic.LimitFlags = 0x2000  # KILL_ON_JOB_CLOSE, no breakaway
        assert k32.SetInformationJobObject(wt.HANDLE(job), 9, ctypes.byref(info), ctypes.sizeof(info))
        assert k32.AssignProcessToJobObject(wt.HANDLE(job), wt.HANDLE(int(child._handle))), ctypes.get_last_error()
        ntdll.NtResumeProcess(wt.HANDLE(int(child._handle)))
        log("app-server launcher in job, resumed")
    msgs = queue.Queue()
    def reader():
        for line in child.stdout:
            try:
                msgs.put(json.loads(line))
            except Exception:
                pass
        msgs.put(None)
    threading.Thread(target=reader, daemon=True).start()
    def send(v):
        child.stdin.write((json.dumps(v) + "\n").encode()); child.stdin.flush()
    def wait_for(pred, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                m = msgs.get(timeout=0.2)
            except queue.Empty:
                continue
            if m is None:
                raise SystemExit("app-server stdout closed")
            if pred(m):
                return m
        return None

    send({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "PasteyProbe", "version": "0"}, "capabilities": {}}})
    init = wait_for(lambda m: m.get("id") == 1, 60)
    log("initialize:", (init or {}).get("result", {}).get("userAgent"))
    send({"method": "initialized", "params": {}})
    send({"id": 2, "method": "thread/start", "params": {"cwd": workspace}})
    th = wait_for(lambda m: m.get("id") == 2, 60)
    thread_id = th["result"]["thread"]["id"]
    log("thread", thread_id)
    send({"id": 3, "method": "turn/start", "params": {"threadId": thread_id, "input": [{"type": "text", "text":
        "Run exactly this one shell command and wait for it to finish, then reply done: ping -n 120 127.0.0.1"}]}})
    ack = wait_for(lambda m: m.get("id") == 3, 60)
    turn_id = ack["result"]["turn"]["id"]
    log("turn", turn_id)
    started = wait_for(lambda m: m.get("method") == "item/started"
                       and m["params"]["item"].get("type") == "commandExecution", 180)
    log("command started:", json.dumps(started["params"]["item"].get("command"))[:200])
    # Let the command's processes appear.
    pings = {}
    end = time.monotonic() + 20
    while time.monotonic() < end and not pings:
        procs = snapshot()
        pings = {p: n for p, (_, n) in procs.items() if n.lower() == "ping.exe"}
        time.sleep(0.1)
    procs = snapshot()
    app_tree = descendants(procs, child.pid)
    log("app-server tree:", sorted((p, procs[p][1]) for p in app_tree))
    watch = {}
    for pid in pings:
        chain, p = [], pid
        while p in procs and len(chain) < 12:
            chain.append((p, procs[p][1])); p = procs[p][0]
        log("ping ancestry:", chain, "in app-server tree:", pid in app_tree)
        for q, n in chain:
            if n.lower() in ("ping.exe", "powershell.exe", "codex-command-runner.exe", "conhost.exe") or n.lower().startswith("codex-command-runner"):
                watch[q] = n
    log("owners:\n" + owner_of(list(watch)))
    if job:
        for pid, n in watch.items():
            log("in Pastey-style job:", pid, n, in_job(pid, job))
    time.sleep(3)
    t_int = time.monotonic()
    send({"id": 4, "method": "turn/interrupt", "params": {"threadId": thread_id, "turnId": turn_id}})
    log("turn/interrupt sent")
    completed = None
    acted = False
    alive_last = None
    while time.monotonic() - t_int < 150:
        try:
            while True:
                m = msgs.get_nowait()
                if m is None:
                    log("app-server stdout closed"); break
                if m.get("method") == "turn/completed":
                    completed = time.monotonic() - t_int
                    log(f"turn/completed +{completed:.2f}s status={m['params']['turn'].get('status')}")
                elif m.get("id") == 4:
                    log(f"interrupt reply +{time.monotonic()-t_int:.2f}s", json.dumps(m)[:200])
                elif m.get("method") in ("item/completed",) and m["params"]["item"].get("type") == "commandExecution":
                    it = m["params"]["item"]
                    log(f"command item completed +{time.monotonic()-t_int:.2f}s status={it.get('status')} exit={it.get('exitCode')}")
        except queue.Empty:
            pass
        if completed is not None and not acted and after != "wait":
            acted = True
            time.sleep(0.5)
            if after == "shutdown":
                log("closing app-server stdin"); child.stdin.close()
            elif after == "terminate":
                log("TerminateJobObject"); k32.TerminateJobObject(wt.HANDLE(job), 1)
        procs = snapshot()
        alive = {p: n for p, n in watch.items() if p in procs and procs[p][1] == n}
        key = tuple(sorted(alive.items()))
        if key != alive_last:
            log(f"+{time.monotonic()-t_int:6.2f}s alive: {sorted(alive.values()) or 'none'}  app-server alive: {child.poll() is None}")
            alive_last = key
        if not alive and completed is not None:
            break
        time.sleep(0.05)
    log("done; closing")
    try:
        child.stdin.close()
    except Exception:
        pass
    child.wait(10)

main()
