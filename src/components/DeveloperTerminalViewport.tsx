import { FitAddon } from "@xterm/addon-fit";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef, useState } from "react";

import {
  decodeTerminalOutputFrame,
  DEVELOPER_TERMINAL_OUTPUT_EVENT,
  type DeveloperTerminalOutputEvent,
  terminalInputBytes,
} from "../lib/developerTerminalFrontend";

const TERMINAL_SCROLLBACK_LINES = 5_000;
const RESIZE_DEBOUNCE_MS = 80;

export function DeveloperTerminalViewport({
  roomId,
  terminalSessionId,
  environmentLabel,
  output,
  outputSequence,
  onInput,
  onResize,
}: {
  roomId: string;
  terminalSessionId: string;
  environmentLabel?: string | null;
  output: string;
  outputSequence: number;
  onInput: (bytes: number[]) => void;
  onResize: (cols: number, rows: number) => void;
}) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const inputRef = useRef(onInput);
  const resizeRef = useRef(onResize);
  const lastOutputSequenceRef = useRef(0);
  const focusedRef = useRef(false);
  const [focused, setFocused] = useState(false);

  inputRef.current = onInput;
  resizeRef.current = onResize;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: "block",
      fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
      fontSize: 13,
      lineHeight: 1.2,
      scrollback: TERMINAL_SCROLLBACK_LINES,
      scrollOnUserInput: true,
      theme: {
        background: "#0d1420",
        foreground: "#e6edf7",
        cursor: "#f4f7fb",
        cursorAccent: "#0d1420",
        selectionBackground: "#315680",
      },
      windowsPty: environmentLabel === "PowerShell" ? { backend: "conpty" } : undefined,
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);
    terminalRef.current = terminal;
    lastOutputSequenceRef.current = 0;

    const textarea = terminal.textarea;
    const setTerminalFocus = (next: boolean) => {
      focusedRef.current = next;
      setFocused(next);
    };
    const handleFocus = () => setTerminalFocus(true);
    const handleBlur = () => setTerminalFocus(false);
    textarea?.addEventListener("focus", handleFocus);
    textarea?.addEventListener("blur", handleBlur);

    const dataSubscription = terminal.onData((data) => {
      if (!focusedRef.current) return;
      inputRef.current(terminalInputBytes(data));
    });

    let unlistenOutput: (() => void) | undefined;
    let outputListenerCancelled = false;
    void listen<DeveloperTerminalOutputEvent>(DEVELOPER_TERMINAL_OUTPUT_EVENT, (event) => {
      const frame = event.payload;
      if (
        frame.roomId !== roomId
        || frame.terminalSessionId !== terminalSessionId
        || frame.sequence !== lastOutputSequenceRef.current + 1
      ) return;
      const bytes = decodeTerminalOutputFrame(frame.dataBase64);
      if (!bytes) return;
      terminal.write(bytes);
      lastOutputSequenceRef.current = frame.sequence;
    }).then((unlisten) => {
      if (outputListenerCancelled) unlisten();
      else unlistenOutput = unlisten;
    });

    let resizeTimer: number | undefined;
    const resizeSubscription = terminal.onResize(({ cols, rows }) => {
      if (resizeTimer !== undefined) window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        resizeRef.current(cols, rows);
      }, RESIZE_DEBOUNCE_MS);
    });

    let animationFrame = 0;
    const fit = () => {
      window.cancelAnimationFrame(animationFrame);
      animationFrame = window.requestAnimationFrame(() => {
        if (container.isConnected) fitAddon.fit();
      });
    };
    const observer = typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(fit);
    observer?.observe(container);
    fit();
    window.requestAnimationFrame(() => terminal.focus());

    return () => {
      observer?.disconnect();
      window.cancelAnimationFrame(animationFrame);
      if (resizeTimer !== undefined) window.clearTimeout(resizeTimer);
      textarea?.removeEventListener("focus", handleFocus);
      textarea?.removeEventListener("blur", handleBlur);
      dataSubscription.dispose();
      resizeSubscription.dispose();
      outputListenerCancelled = true;
      unlistenOutput?.();
      terminal.dispose();
      terminalRef.current = null;
      lastOutputSequenceRef.current = 0;
      focusedRef.current = false;
    };
  }, [environmentLabel, roomId, terminalSessionId]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) return;
    if (outputSequence <= lastOutputSequenceRef.current) return;
    terminal.reset();
    terminal.write(output);
    lastOutputSequenceRef.current = outputSequence;
  }, [output, outputSequence, terminalSessionId]);

  return (
    <div className={`developer-terminal-viewport${focused ? " focused" : ""}`}>
      <div
        ref={containerRef}
        className="developer-terminal-xterm"
        role="application"
        aria-label="Remote developer terminal"
        onMouseDown={() => terminalRef.current?.focus()}
      />
      {output.length === 0 ? (
        <p className="developer-terminal-startup-hint">Connected. If no prompt appears, type a command and press Enter.</p>
      ) : null}
      {!focused ? (
        <p className="developer-terminal-focus-hint">Click the terminal to type on the remote Host.</p>
      ) : null}
    </div>
  );
}

export default DeveloperTerminalViewport;
