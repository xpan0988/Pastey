# P02 observer validation (fresh clone of 572572f; generators copied from ws/*/work; prior .kmx deleted)
| Claim | Command | SHA-256 prefix | verify --tier full |
|---|---|---|---|
| codex 96 T / 67 Q (best) | `closed_form.py --split-inverse --compact-norm --mul24 out` (2c8959a) | 860c072519ff | PASS T=96 Q=67 |
| codex 99 / 67 | `closed_form.py --split-inverse --compact-norm out` | b91a6cfe088a | PASS |
| codex 123 / 67 | `closed_form.py --split-inverse --recycle-t6 out` | 2cec1f4f5848 | PASS |
| codex 115 / 71 | `closed_form.py --split-inverse out` | 5a7907733c55 | PASS |
| claude 816 / 69 | `work/claude/gen_stage24_inv36.py` | 871d8e246a50 | PASS |
| claude 896 / 71 | `work/claude/gen_stage24.py` | d2ec56c510f3 | PASS |
All SHA-256s equal the ledger entries; selftest 33/33. The 96/67 stage was also reproduced byte-identically by codex using claude's 24 terms received through Pastey (ledger note "Claude multiplier terms received via Pastey").
