import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  terminalInputBytes,
  terminalOutputUpdate,
} from "../src/lib/developerTerminalFrontend";

test("terminal output snapshots append through the emulator without duplicating history", () => {
  assert.deepEqual(terminalOutputUpdate("", "prompt> "), {
    kind: "append",
    data: "prompt> ",
  });
  assert.deepEqual(terminalOutputUpdate("prompt> ", "prompt> echo hello\r\nhello\r\n"), {
    kind: "append",
    data: "echo hello\r\nhello\r\n",
  });
  assert.deepEqual(terminalOutputUpdate("same", "same"), {
    kind: "none",
    data: "",
  });
  assert.deepEqual(terminalOutputUpdate("trimmed-old-buffer", "new-bounded-buffer"), {
    kind: "reset",
    data: "new-bounded-buffer",
  });
});

test("xterm input data is UTF-8 encoded without manual key remapping", () => {
  for (const sequence of [
    "\u007f",
    "\u001b[3~",
    "\u001b[A",
    "\u001b[B",
    "\u001b[C",
    "\u001b[D",
    "\u001b[H",
    "\u001b[F",
    "\r",
    "\t",
    "\u0003",
    "\u0004",
    "\u000c",
    "printable text",
    "中文",
  ]) {
    assert.deepEqual(terminalInputBytes(sequence), [...new TextEncoder().encode(sequence)]);
  }
});

test("Developer Terminal delegates VT rendering input and cursor behavior to xterm", () => {
  const component = readFileSync("src/components/DeveloperTerminalViewport.tsx", "utf8");

  assert.match(component, /new Terminal\(\{/);
  assert.match(component, /cursorBlink: true/);
  assert.match(component, /terminal\.onData\(\(data\)/);
  assert.match(component, /inputRef\.current\(terminalInputBytes\(data\)\)/);
  assert.match(component, /terminal\.write\(update\.data\)/);
  assert.match(component, /windowsPty: environmentLabel === "PowerShell" \? \{ backend: "conpty" \}/);
  assert.doesNotMatch(component, /onKeyDown=/);
});

test("Developer Terminal focus is explicit and uses the emulator cursor", () => {
  const component = readFileSync("src/components/DeveloperTerminalViewport.tsx", "utf8");

  assert.match(component, /onMouseDown=\{\(\) => terminalRef\.current\?\.focus\(\)\}/);
  assert.match(component, /window\.requestAnimationFrame\(\(\) => terminal\.focus\(\)\)/);
  assert.match(component, /Click the terminal to type on the remote Host\./);
  assert.match(component, /If no prompt appears, type a command and press Enter\./);
  assert.match(component, /if \(!focusedRef\.current\) return/);
});

test("Developer Terminal resize uses FitAddon and a bounded debounce", () => {
  const component = readFileSync("src/components/DeveloperTerminalViewport.tsx", "utf8");

  assert.match(component, /new FitAddon\(\)/);
  assert.match(component, /terminal\.onResize\(\(\{ cols, rows \}\)/);
  assert.match(component, /RESIZE_DEBOUNCE_MS = 80/);
  assert.match(component, /resizeRef\.current\(cols, rows\)/);
});

test("active Developer Terminal UI hides fresh-request controls and identifies the Host", () => {
  const pages = readFileSync("src/pages/BridgeProductPages.tsx", "utf8");
  const panel = pages.slice(
    pages.indexOf("function DeveloperModePanel"),
    pages.indexOf("function BridgePlanReceiverPanel"),
  );

  assert.match(panel, /uiSession && !currentController/);
  assert.match(panel, /Connected Host: \{controlledHostName\}/);
  assert.match(panel, /Shell: \{currentController\.environmentLabel/);
  assert.match(panel, /Status: \{currentController\.state/);
  assert.match(panel, /<DeveloperTerminalViewport/);
  assert.doesNotMatch(panel, /function sendKey/);
  assert.doesNotMatch(panel, /terminalDisplay/);
});
