# ui-bridge

Development tool: Pastey's real frontend in a browser, driving two real Hosts, without a native window.

- `src-tauri/src/collaboration/ui_bridge.rs` (an ignored test) starts the pair harness's two Hosts, "Mac" and "Windows". They are paired by code over a real Bridge, and Windows shares a folder with the Mac. Each Host serves the app's own Tauri commands through Tauri's IPC dispatch (`tauri::test`, mock runtime) on localhost: Mac on 17431, Windows on 17432. Codex and Claude Code are the installed ones, and turns the collaboration starts run at effort `low`.
- `vite.config.mjs` serves the frontend on 1430 with a shim that sends `invoke` to the bridge. `?bridge=17431` is the Mac, and `?bridge=17432` is Windows.

```bash
cd src-tauri && cargo build && cargo test --bin pastey collaboration_screens_over_a_browser_bridge -- --ignored --nocapture
```

```bash
npx vite --config tools/ui-bridge/vite.config.mjs
```

- The folder dialog returns what `POST http://127.0.0.1:<port>/pick {"path": "..."}` set last.
- Events are not delivered; the screens poll.
- Six commands that need a native window are left out: reveal, logs folder, clipboard and updates.
- Touch `src-tauri/target/ui-bridge/stop` to end the bridge.
- Folders and app data live under `src-tauri/target/ui-bridge/` and a temporary directory. The bridge never touches this Mac's own Pastey data.
