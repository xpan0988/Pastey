# Codex Stop probe (Windows)

Checks, outside Pastey, whether Codex's own `turn/interrupt` ends the command a turn runs. It drives `codex app-server --stdio` the way Pastey does (`initialize`, `thread/start`, `turn/start`) and asks for `ping -n 120 127.0.0.1`. Once the `ping` runs, it sends `turn/interrupt` and prints the command's processes, with their owner and ancestry, each time that set changes.

```
python -I probe.py                         # interrupt only
python -I probe.py --after=shutdown        # then close the app-server's stdin
python -I probe.py --job --after=terminate # app-server in a Job Object; then terminate the job
```

`--job` starts the app-server suspended inside a kill-on-close Job Object that permits no breakaway, as Pastey does, and reports whether each of the command's processes is in it. `--codex=PATH` picks another launcher. Each run uses the signed-in Codex and its quota (one short turn). See [development](../../docs/development.md#automated-local-and-physical-evidence) for Windows Stop evidence.
