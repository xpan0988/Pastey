# P01 setup log (observer: Claude Code Opus 5.5, outside the collaboration)

Times local (+11:00), 2026-11-26.

| Time | Basis | Event |
|---|---|---|
| ~16:08 | observer (earlier session) | Debug bundle built at Pastey a2ce090 (`raw/pastey-build.log`); verifier hashes recorded (`verifier-hashes-pre.txt`). |
| 16:17 | observer | Workspaces: independent `git clone --no-hardlinks` of `~/research-playgrounds/qcc-stage-playground` into `p01/ws/claude` and `p01/ws/codex`, branch `p01/<alias>` at 572572f (8fe93fe + .gitignore-only commit); `origin` removed so nothing can be pushed back to the frozen playground. selftest 33/33 in each; L3 incumbent full-tier PASS 953 T / 71 Q in codex clone. Hashes: `hashes-pre-*.txt`. |
| 16:17:44 | observer | Pastey bundle launched detached, clean env (HOME, USER, LOGNAME, SHELL, LANG, TMPDIR, PATH incl. Python 3.14 framework bin). Log `raw/pastey-app.log`. Pastey DB had 0 collaborations, no execution default, no preferences. |
| 16:20:54 | pastey | Collaboration `p01-research` created (id `collaboration:00a9f8a3-8035-4c27-aca6-5cffad6eb5c4`); members codex (Codex, ws/codex), claude (Claude Code, ws/claude), role lines empty. Folder picker needed full-screen control (native Open panel). |
| 16:21 | observer | Removed OPERATOR.md from both workspaces (commit 97c9b59 on each p01 branch): the playground marks it "not for participating agents" and it contains organisational suggestions. |
| 16:23 | pastey/user-authorised | Model/effort per member: codex gpt-6.2-sol / low; claude claude-sonnet-5-5 / low. Collaboration execution = "Unattended engineering" preset (collaboration scope only, not user default), consent ticked under the P01 authorisation. No preference profile. Codex network: off (no folder grant). |
| 16:24 | pastey | Grants: 7 automatic turns within 2 h per member (first turn is user-message-triggered and uses no grant: 2 automatic planning + 5 execution). |
| 16:24 | pastey | Readiness re-check: both members ready (folder, installed, signed in, trust, model/effort configured, execution configured). Plan together first: on. |
| 16:25:14 / 16:26 | pastey | Identical kickoff message (1179 chars) to codex, then claude, while Preparing. |
| 16:26:11 | pastey | Start collaboration: released; both members' first (planning) turns start together. Hard end 17:26. |
- 16:37 observer ended the collaboration after >5 min quiet (Journal kept).
