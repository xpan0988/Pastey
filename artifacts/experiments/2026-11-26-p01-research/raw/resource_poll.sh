#!/bin/zsh
# Observer: every 30 s, record agent processes (cpu, rss, elapsed) and the collaboration record.
OUT=${0:a:h}/resources.log; D="$HOME/Library/Application Support/dev.localfirst.pastey/db.sqlite"
while true; do
  echo "=== $(date +%H:%M:%S)" >> $OUT
  ps -axo pid,ppid,etime,%cpu,rss,command | grep -E 'claude|codex|python3' | grep -v -E 'grep|resource_poll|Claude.app|claude-501|\.claude/local' | cut -c1-220 >> $OUT
  sqlite3 -readonly "$D" "select record_json from collaborations where id='collaboration:00a9f8a3-8035-4c27-aca6-5cffad6eb5c4'" > ${0:a:h}/record-latest.json
  sleep 30
done
