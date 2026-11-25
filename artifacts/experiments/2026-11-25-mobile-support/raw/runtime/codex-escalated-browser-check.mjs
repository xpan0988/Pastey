import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp, readFile, rm} from 'node:fs/promises';
import {createServer} from 'node:http';
import {join} from 'node:path';

const fixture = String.raw`<script>
const room = {
  id: "room", status: "active", created_at: 1, expires_at: 9999999999,
  local_role: "joined", auto_burn_after_expiry: false, peer_connected: true,
  pairing_state: "paired",
  peers: [{ peerSessionId: "route-desktop", hostRef: "host:desktop", displayName: "Studio",
    accepted: true, sessionVerified: true, currentSessionOnly: true, connected: true,
    liveness: "connected", joinMethod: "manual_code" }]
};
let rooms = [], items = [];
window.__TAURI_INTERNALS__ = {
  transformCallback() {return 1;}, unregisterCallback() {},
  async invoke(cmd, args) {
    switch (cmd) {
      case "list_rooms": return rooms;
      case "join_room_at":
        if (args.code !== "12345678" || args.address !== "192.168.1.24") throw new Error("Bad pairing input");
        rooms = [room];
        return room;
      case "list_room_items": return items;
      case "send_text_to_room": {
        if (args.bridgeRoute.target.peerSessionId !== "route-desktop") throw new Error("Wrong text route");
        const text = {id:"sent", room_id:"room", created_at:2, direction:"outgoing", status:"sent",
          payload_type:"text", item_kind:"text", text:args.text, size_bytes:args.text.length};
        items = [text];
        return text;
      }
      case "get_bridge_peer_address": return "192.168.1.24";
      case "set_bridge_peer_address": return args.address;
      case "list_native_agent_capabilities": return [];
      case "get_native_agent_recovery_projection": return null;
      case "list_remote_native_agent_capabilities": return [];
      case "list_remote_shared_workspaces": return {outcome: "ok", scopes: [
        {scopeRef: "scope:0123456789abcdef", label: "Projects", readSessions: true, agentRequests: true}
      ]};
      case "discover_remote_native_sessions": return { outcome: "ok", sessions: [], scopes: [] };
      case "burn_room": rooms = []; return true;
      default: return null;
    }
  }
};
const waitFor = async (match) => {
  for (let i=0; i<100; i++) {
    const value = match();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 30));
  }
  throw new Error("Timed out waiting for UI");
};
const byText = label => [...document.querySelectorAll("button")].find(b => b.textContent.trim() === label);
const setValue = (input, value) => {
  Object.getOwnPropertyDescriptor(input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype :
    HTMLInputElement.prototype, "value").set.call(input, value);
  input.dispatchEvent(new Event("input", {bubbles:true}));
};
window.addEventListener("load", async () => {
  try {
    (await waitFor(() => byText("Pair desktop"))).click();
    (await waitFor(() => byText("Pair Bridge"))).click();
    await waitFor(() => document.body.textContent.includes("Enter the other device"));
    setValue(document.querySelector("#mobile-address"), "192.168.1.24");
    setValue(document.querySelector("#mobile-code"), "1234 5678");
    byText("Pair Bridge").click();
    await waitFor(() => document.querySelector("h1")?.textContent === "Studio");
    const composer = await waitFor(() => document.querySelector("textarea[aria-label=Message]"));
    setValue(composer, "Hello from phone");
    (await waitFor(() => {const button = document.querySelector("button[aria-label=Send]"); return button && !button.disabled && button;})).click();
    await waitFor(() => document.querySelector(".msg-bubble")?.textContent === "Hello from phone");
    (await waitFor(() => byText("Agents"))).click();
    await waitFor(() => document.querySelector(".nav-place-name")?.textContent === "Projects");
    if (document.body.scrollWidth > innerWidth || document.querySelector(".mobile-shell").scrollWidth > innerWidth) throw new Error("Horizontal overflow");
    document.querySelector("button[aria-label='Bridge settings']").click();
    (await waitFor(() => byText("Burn Bridge…"))).click();
    (await waitFor(() => byText("Burn Bridge"))).click();
    await waitFor(() => document.querySelector("h1")?.textContent === "Pastey");
    document.body.dataset.mobileCheck = "pass";
  } catch (error) {
    document.body.dataset.mobileCheck = "fail " + error.message;
  }
});
</script>`;

const dist = join(process.cwd(), 'dist');
const server = createServer(async (request, response) => {
  try {
    const path = new URL(request.url, 'http://localhost').pathname;
    const body = await readFile(join(dist, path), path.endsWith('.html') ? 'utf8' : undefined);
    response.setHeader('Content-Type', path.endsWith('.html') ? 'text/html' : path.endsWith('.js') ? 'text/javascript' : 'text/css');
    response.end(path.endsWith('.html') ? body.replace('</head>', fixture + '</head>') : body);
  } catch { response.writeHead(404).end(); }
});
await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve));
const profile = await mkdtemp('/private/tmp/pastey-headless-');
try {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', '--disable-gpu', '--disable-background-networking', '--no-first-run',
    '--window-size=390,844', '--virtual-time-budget=8000', '--dump-dom',
    '--user-data-dir=' + profile, 'http://127.0.0.1:' + server.address().port + '/mobile.html',
  ]);
  let output = '', stderr = '';
  chrome.stdout.on('data', chunk => output += chunk);
  chrome.stderr.on('data', chunk => stderr += chunk);
  const code = await new Promise((resolve, reject) => {
    chrome.once('error', reject);
    chrome.once('close', resolve);
  });
  assert.equal(code, 0, stderr);
  assert.match(output, /<body[^>]+data-mobile-check="pass"/, output.split('<body').at(-1).slice(0, 4500));
  console.log('Mobile renderer smoke check passed at phone width: pair, send, shared workspace, Burn.');
} finally {
  server.close();
  await rm(profile, {recursive:true, force:true});
}
