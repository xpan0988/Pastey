# P03 observer validation (fresh clone of 572572f; generators copied from ws/*/work; prior .kmx deleted)
| Claim | Command | SHA-256 prefix | verify --tier full |
|---|---|---|---|
| codex 141 T / 59 Q (new low-Q point) | `work/codex/retained.py out --retain split-phase` | 1254355d54d8 | PASS T=141 Q=59 |
| codex 115 / 71 | `retained.py out --retain split` | eb0b36b6870e | PASS |
| codex 123 / 67 | `retained.py out --retain split-partial` | 4f8bbf265e1a | PASS |
| codex 123 / 71 | `retained.py out --retain partial-first` | a43e21addf30 | PASS |
| codex 141 / 63 | `retained.py out --retain phase-first` | 6974d577f85e | PASS |
| codex 115 / 75 | `retained.py out --retain both` | 548ca9a608e4 | PASS |
| codex 159 / 63 | `retained.py out --retain second` | 77dac768583b | PASS |
| codex 230 / 63 | `work/codex/direct.py` | d445dd1b20f5 | PASS |
| claude 203 / 63 | `work/claude/gen_phase_direct.py` | f24db08c24ed | PASS |
| claude 167 / 63 | `work/claude/gen_phase_direct2.py` | 38d514fdca68 | PASS |
All SHA-256s equal the ledger entries; selftest 33/33; protected files unchanged in both clones; frozen playground clean; no REFUSED (no L=4 attempt) in either transcript.
