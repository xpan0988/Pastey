import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import {
  decodeTerminalOutputFrame,
  MAX_QUEUED_TERMINAL_INPUT_BYTES,
  MAX_TERMINAL_INPUT_FRAME_BYTES,
  OrderedTerminalInputWriter,
  TerminalInputBackpressureError,
  terminalDimensionsEqual,
  terminalInputBytes,
} from "../src/lib/developerTerminalFrontend";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<void>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

test("rapid terminal input uses one ordered writer and preserves several hundred bytes", async () => {
  const firstSend = deferred();
  const frames: number[][] = [];
  const errors: unknown[] = [];
  const writer = new OrderedTerminalInputWriter(async (frame) => {
    frames.push(frame);
    if (frames.length === 1) await firstSend.promise;
  }, (error) => errors.push(error));

  const expected = [...new TextEncoder().encode("rapid-input-".repeat(64))];
  expected.forEach((byte) => writer.enqueue([byte]));
  assert.equal(frames.length, 1, "only one invoke may be in flight");
  firstSend.resolve();
  await writer.whenIdle();

  assert.deepEqual(frames.flat(), expected);
  assert.ok(frames.every((frame) => frame.length <= MAX_TERMINAL_INPUT_FRAME_BYTES));
  assert.deepEqual(errors, []);
});

test("key repeat remains ordered without duplicate input", async () => {
  const frames: number[][] = [];
  const writer = new OrderedTerminalInputWriter(
    async (frame) => { frames.push(frame); },
    (error) => assert.fail(String(error)),
  );
  const repeated = [...terminalInputBytes("\u007f".repeat(400)), ...terminalInputBytes("x".repeat(400))];
  repeated.forEach((byte) => writer.enqueue([byte]));
  await writer.whenIdle();
  assert.deepEqual(frames.flat(), repeated);
});

test("allowed paste is chunked below the 8 KiB wire bound and remains ordered", async () => {
  const frames: number[][] = [];
  const writer = new OrderedTerminalInputWriter(
    async (frame) => { frames.push(frame); },
    (error) => assert.fail(String(error)),
  );
  const paste = [...new TextEncoder().encode("paste-line\n".repeat(2_000))];
  writer.enqueue(paste);
  await writer.whenIdle();
  assert.deepEqual(frames.flat(), paste);
  assert.ok(frames.length > 1);
  assert.ok(frames.every((frame) => frame.length <= MAX_TERMINAL_INPUT_FRAME_BYTES));
});

test("input queue backpressure is explicit and does not fabricate authority loss", () => {
  const blocked = deferred();
  const writer = new OrderedTerminalInputWriter(
    async () => blocked.promise,
    (error) => assert.fail(String(error)),
  );
  writer.enqueue([1]);
  writer.enqueue(new Array(MAX_QUEUED_TERMINAL_INPUT_BYTES).fill(2));
  assert.throws(() => writer.enqueue([3]), TerminalInputBackpressureError);
  writer.cancel();
  blocked.resolve();
});

test("close or Burn-style cancellation discards queued input without retry", async () => {
  const firstSend = deferred();
  const frames: number[][] = [];
  const writer = new OrderedTerminalInputWriter(async (frame) => {
    frames.push(frame);
    await firstSend.promise;
  }, (error) => assert.fail(String(error)));
  writer.enqueue([1]);
  writer.enqueue([2, 3, 4]);
  writer.cancel();
  firstSend.resolve();
  await writer.whenIdle();
  assert.deepEqual(frames, [[1]]);
  assert.equal(writer.pendingBytes(), 0);
});

test("a rejected send stops the writer and does not emit queued frames", async () => {
  const rejected = new Error("Developer Terminal sequence was rejected.");
  const frames: number[][] = [];
  const errors: unknown[] = [];
  const writer = new OrderedTerminalInputWriter(async (frame) => {
    frames.push(frame);
    throw rejected;
  }, (error) => errors.push(error));
  writer.enqueue([1]);
  writer.enqueue([2, 3]);
  await writer.whenIdle();
  writer.enqueue([4]);
  assert.deepEqual(frames, [[1]]);
  assert.deepEqual(errors, [rejected]);
});

test("terminal output events decode bounded binary frames", () => {
  const encoded = btoa(String.fromCharCode(0, 27, 255));
  assert.deepEqual([...decodeTerminalOutputFrame(encoded)!], [0, 27, 255]);
  assert.equal(decodeTerminalOutputFrame("not base64 !!!"), null);
  assert.equal(decodeTerminalOutputFrame(btoa("x".repeat(MAX_TERMINAL_INPUT_FRAME_BYTES + 1))), null);
});

test("terminal resize dimensions suppress identical remote resize reports", () => {
  assert.equal(terminalDimensionsEqual(null, { cols: 80, rows: 24 }), false);
  assert.equal(terminalDimensionsEqual({ cols: 80, rows: 24 }, { cols: 80, rows: 24 }), true);
  assert.equal(terminalDimensionsEqual({ cols: 80, rows: 24 }, { cols: 81, rows: 24 }), false);
  assert.equal(terminalDimensionsEqual({ cols: 80, rows: 24 }, { cols: 80, rows: 25 }), false);
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
  assert.match(component, /listen<DeveloperTerminalOutputEvent>\(DEVELOPER_TERMINAL_OUTPUT_EVENT/);
  assert.match(component, /terminal\.write\(bytes, \(\) => \{/);
  assert.match(component, /outputSequence <= lastOutputSequenceRef\.current/);
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
  const styles = readFileSync("src/styles.css", "utf8");

  assert.match(component, /new FitAddon\(\)/);
  assert.match(component, /terminal\.onResize\(\(\{ cols, rows \}\)/);
  assert.match(component, /RESIZE_DEBOUNCE_MS = 80/);
  assert.match(component, /terminalDimensionsEqual\(lastReportedResize, next\)/);
  assert.match(component, /resizeRef\.current\(next\.cols, next\.rows\)/);
  assert.match(component, /new ResizeObserver\(scheduleFit\)/);
  assert.match(component, /observer\?\.observe\(container\)/);
  assert.match(component, /observer\?\.observe\(container\.parentElement\)/);
  assert.match(component, /document\.fonts\?\.ready\.then\(scheduleFit\)/);
  assert.match(component, /settledFitFrame = window\.requestAnimationFrame\(fitAndRefresh\)/);
  assert.match(component, /terminal\.refresh\(0, terminal\.rows - 1\)/);
  assert.match(component, /terminal\.write\(bytes, \(\) => \{/);
  assert.match(component, /postOutputFitPending = false/);
  assert.match(component, /terminal\.write\(output, \(\) => fitRequestRef\.current\(\)\)/);
  assert.match(styles, /\.developer-terminal-xterm > \.xterm[\s\S]*padding: 12px/);
  assert.match(styles, /\.developer-terminal-xterm \{\s*height: 360px;\s*\}/);
});

test("Developer Terminal fit lifecycle is recreated for each opened session and disposed on close", () => {
  const component = readFileSync("src/components/DeveloperTerminalViewport.tsx", "utf8");
  assert.match(component, /\[environmentLabel, roomId, terminalSessionId\]/);
  assert.match(component, /observer\?\.disconnect\(\)/);
  assert.match(component, /window\.cancelAnimationFrame\(firstFitFrame\)/);
  assert.match(component, /window\.cancelAnimationFrame\(settledFitFrame\)/);
  assert.match(component, /fitRequestRef\.current = \(\) => \{\}/);
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
  assert.match(panel, /new OrderedTerminalInputWriter\(/);
  assert.match(panel, /terminalInputWriterRef\.current\?\.cancel\(\)/);
  assert.doesNotMatch(panel, /void sendDeveloperTerminalInput\(/);
  assert.doesNotMatch(panel, /function sendKey/);
  assert.doesNotMatch(panel, /terminalDisplay/);
});
