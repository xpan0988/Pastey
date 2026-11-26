# P02 observer notes
- 17:26:48 claude's first plan block refused by Pastey: "task A is over its bounds, or claims a path outside its folder" (attention). claude did not re-propose; it accepted codex's r1 in its next turn (17:27:04). Plan r1 adopted 35 s after release.
- 17:28:19 claude reported `done` with artifact `p02/claude@work-tree-uncommitted`: Pastey recorded "not a commit id; not checked".
- 17:28:19 False claim conflict: "claude changed files another member's task claims: results/ledger.jsonl (claimed by codex's task schedule)". Each member has its own clone, so the two ledgers are different files. R5 compares claimed paths literally across members and ignores whose folder they are in.
- 17:30:14 codex asked claude to send the multiplier source through Pastey. claude answered at 17:30:24 with the 24 terms inline (1,224 B, no fenced code, delivered whole), so the CF-1 fix was not exercised live. This time the note told Agents how to quote code.
- 17:31:19 codex's 96 T / 67 Q (2c8959a) uses its own 24-AND multiplier (`work/codex/mul24.py`, "polynomial interpolation over GF(4)"). It differs from claude's file, and the codex rollout shows no read of ws/claude. It is an independent construction, made before claude's inline message reached it.
- claude asked codex to "copy work/claude/*.py into your folder" (handoff by filesystem); codex kept to Pastey messages.
