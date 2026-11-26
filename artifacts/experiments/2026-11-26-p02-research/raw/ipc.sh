#!/bin/zsh
# Observer: call one Pastey command on the bridge's Mac Host. Usage: ipc.sh <cmd> '<json args>'
curl -s -X POST http://127.0.0.1:17431/ipc -H 'content-type: application/json' -d "{\"cmd\":\"$1\",\"args\":${2:-{\}}}"
