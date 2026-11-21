// Serves Pastey's frontend with an IPC shim that sends `invoke` to the
// development bridge (`src-tauri/src/collaboration/ui_bridge.rs`). Open
// http://localhost:1430/?bridge=17431 (the Mac Host) or ?bridge=17432 (Windows).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const shim = `
(() => {
  const port = new URLSearchParams(location.search).get("bridge") || "17431";
  const base = "http://127.0.0.1:" + port;
  let next = 1;
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
    plugins: {},
    transformCallback(callback) { const id = next++; window["_" + id] = callback; return id; },
    unregisterCallback(id) { delete window["_" + id]; },
    convertFileSrc(path) { return path; },
    async invoke(cmd, args) {
      const response = await fetch(base + "/ipc", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ cmd, args: args ?? {} }) });
      const result = await response.json();
      if ("err" in result) throw result.err;
      return result.ok;
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__PASTEY_BRIDGE__ = base;
})();
`;

export default defineConfig({
  root: new URL("../..", import.meta.url).pathname,
  clearScreen: false,
  plugins: [react(), { name: "pastey-ipc-shim", transformIndexHtml: () => [{ tag: "script", children: shim, injectTo: "head-prepend" }] }],
  server: { port: 1430, strictPort: true },
});
