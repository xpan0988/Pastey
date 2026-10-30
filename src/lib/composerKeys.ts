/** What a key press in a message composer does. Pure, so every browser's
 * event order can be tested without a DOM.
 *
 * An input method (Chinese, Japanese, Korean, …) uses Enter to confirm its
 * candidates; that Enter must never send. Browsers report it differently:
 * Chromium and Firefox (Windows' WebView2) set `isComposing`; WebKit (the
 * macOS webview) can deliver `compositionend` before the confirming
 * keydown, which then arrives with `isComposing` false but `keyCode` 229
 * (WebKit bug 165004). So the composer also tracks composition itself, and
 * treats an Enter within a few milliseconds of `compositionend` as part of
 * it. Each check alone misses a browser; together they cover all three. */

/** What plain Enter does. Shift+Enter always adds a line and Cmd/Ctrl+Enter
 * always sends, whichever is chosen. */
export type EnterKeyMode = "send" | "newline";

export interface ComposerKeyEvent {
  key: string;
  shiftKey: boolean;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  isComposing: boolean;
  /** Deprecated but the only signal WebKit gives: 229 while an IME owns it. */
  keyCode: number;
  repeat: boolean;
  timeStamp: number;
}

export interface CompositionState {
  /** Between `compositionstart` and `compositionend`. */
  composing: boolean;
  /** `timeStamp` of the last `compositionend`, if any. */
  endedAt: number | null;
}

/** An Enter this close to `compositionend` (in event time, either order)
 * belongs to the composition. */
export const COMPOSITION_GRACE_MS = 50;

/** `default`: leave the key to the textarea (or the input method), which
 * adds a line or confirms a candidate. `send`: send the message.
 * `ignore`: swallow the key and do nothing. */
export type ComposerKeyAction = "default" | "send" | "ignore";

export function composerKeyAction(event: ComposerKeyEvent, composition: CompositionState, mode: EnterKeyMode): ComposerKeyAction {
  if (event.key !== "Enter") return "default";
  if (enterBelongsToComposition(event, composition)) return "default";
  if (event.shiftKey || event.altKey) return "default";
  if (event.metaKey || event.ctrlKey) return event.repeat ? "ignore" : "send";
  if (mode === "newline") return "default";
  // Holding Enter must not send once per repeat.
  return event.repeat ? "ignore" : "send";
}

export function enterBelongsToComposition(event: Pick<ComposerKeyEvent, "isComposing" | "keyCode" | "timeStamp">, composition: CompositionState): boolean {
  if (event.isComposing || event.keyCode === 229 || composition.composing) return true;
  return composition.endedAt !== null && Math.abs(event.timeStamp - composition.endedAt) < COMPOSITION_GRACE_MS;
}

const ENTER_KEY = "pastey:enter-key-v1";
const CHANGE_EVENT = "pastey:enter-key-change";

/** A touch-first device, where the keyboard's Return is for writing and
 * sending is the button's job. */
function touchFirst(): boolean {
  try { return window.matchMedia("(pointer: coarse)").matches && !window.matchMedia("(hover: hover)").matches; }
  catch { return false; }
}

/** The renderer's Enter preference: a per-device convenience kept in this
 * device's renderer storage, like Appearance. It never reaches a peer. */
export function readEnterKeyMode(): EnterKeyMode {
  try {
    const value = window.localStorage.getItem(ENTER_KEY);
    if (value === "send" || value === "newline") return value;
  } catch { /* storage is a convenience only */ }
  return touchFirst() ? "newline" : "send";
}

export function saveEnterKeyMode(value: EnterKeyMode) {
  try { window.localStorage.setItem(ENTER_KEY, value); } catch { /* still applied for this window */ }
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT));
}

export function onEnterKeyModeChange(listener: () => void): () => void {
  window.addEventListener(CHANGE_EVENT, listener);
  window.addEventListener("storage", listener);
  return () => { window.removeEventListener(CHANGE_EVENT, listener); window.removeEventListener("storage", listener); };
}

export function isApplePlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);
}

/** The shortcut line under a composer, in the platform's own key names. */
export function composerShortcutHint(mode: EnterKeyMode, apple = isApplePlatform()): string {
  const enter = apple ? "Return" : "Enter";
  const mod = apple ? "⌘" : "Ctrl+";
  return mode === "send"
    ? `${enter} sends · Shift+${enter} adds a line`
    : `${mod}${enter} sends · ${enter} adds a line`;
}
