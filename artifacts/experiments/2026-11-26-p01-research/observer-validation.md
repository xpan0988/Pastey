# Observer validation during P01 (read-only to participant folders)

## 16:29 codex retained-input stage
Fresh clone of qcc-stage-playground at 572572f in observer scratch; copied only `work/codex/retained_input.py` from ws/codex (commit a2bde2b).
`python3 work/codex/retained_input.py work/codex/rv.kmx` → SHA-256 c9f31ceed8b683ac6c977dddff2a52617b91ca31c2cb4ed4595a150301c39008 (matches codex's claim).
`python3 verify.py work/codex/rv.kmx --tier full` → PASS VERIFIED exhaustive, L=3, Toffolis=230, qubits=63, HMR=202, 1.457 s.
Protected files in ws/codex unchanged vs 97c9b59 (`git diff --stat` touches only work/codex/).

## 16:33 all claimed results (fresh clone of 572572f, generators copied from ws/*/work, prior .kmx deleted)
| Claim | Generator (run in clone) | SHA-256 | verify.py --tier full |
|---|---|---|---|
| codex 99 T / 65 Q (headline) | `work/codex/live_norm.py --compact --in-place out` (cf9b1e4) | f222cb3b657da128d50faac397d1ac70c42f8f2f5464a08fdab9dd8633c6594e | PASS exhaustive, T=99 Q=65 HMR=99 |
| codex 99 T / 71 Q | `work/codex/live_norm.py out` (c29f6a2) | 698fc3b92be406a66f5c2efcce687d0a7964fd36b9e37d8eabae49deb9e54b80 | PASS, T=99 Q=71 |
| claude 198 T / 61 Q | `work/claude/codex_retained_input.py out` (codex's 230/63 generator + `from inv2 import Gen2 as Gen`) | 7f584f38c6bf0f278a345fb4f8f2afb5ec62818ef1015901927329cda3a3b7ff | PASS, T=198 Q=61 HMR=166 |
| codex 230 T / 63 Q | `work/codex/retained_input.py out` (a2bde2b) | c9f31ceed8b683ac6c977dddff2a52617b91ca31c2cb4ed4595a150301c39008 | PASS, T=230 Q=63 |
| claude 873 T / 69 Q | `work/claude/stage2.py 3` | 5ff71a2d946f7ba918e53213d0041094137c989c9441b5f08fa1d90548f1bce9 | PASS, T=873 Q=69 |
| claude GF(256) inv component 36 T | `work/claude/inv2.py` | b3950862ef1b291bff550d1fbab927e4797cfd0414b2835e90672dfcd515f53e | check_component PASS exhaustive 16 bits, T=36 Q=29 |
selftest in the clone: 33/33. All hashes match the participants' claims.
Caveat: claude's 198/61 generator exists only as an edited copy of codex's file in claude's untracked work dir (no commit, no ledger entry). Claude's work is all uncommitted.
