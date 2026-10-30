/** The pieces every Agents conversation is made of: a header that says
 * what and where it is, a thread of turns, and a composer. They carry no
 * product state. The same message language as the Share thread: what this
 * device's user wrote on the right, what came back on the left, and events
 * as quiet lines between them. What this device's user wrote, and text from
 * another device, is plain text; an Agent's reply is read as Markdown
 * (AgentMarkdown), and nothing from either is ever HTML. */
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { composerKeyAction, composerShortcutHint, isApplePlatform, onEnterKeyModeChange, readEnterKeyMode, type CompositionState, type EnterKeyMode } from "../../lib/composerKeys";
import { Icon } from "./ui";

export function ConversationHeader({ title, subtitle, side, onBack }: { title: ReactNode; subtitle?: ReactNode; side?: ReactNode; onBack?: () => void }) {
  return <header className="conversation-header">
    {onBack ? <button type="button" className="btn btn-ghost btn-icon btn-sm conversation-back" aria-label="Back to sessions" onClick={onBack}><Icon name="back" /></button> : null}
    <div className="conversation-title">
      <h2>{title}</h2>
      {subtitle ? <span>{subtitle}</span> : null}
    </div>
    {side ? <div className="conversation-side">{side}</div> : null}
  </header>;
}

/** How close to the end counts as reading the newest turn. */
const FOLLOW_SLACK_PX = 64;

/** Sent by a Composer when its user sends: the thread then follows to the
 * turn they just began, wherever they had scrolled. */
const SENT_EVENT = "pastey:conversation-sent";

/** The scrolling thread. It follows the newest turn as it grows, unless the
 * user has scrolled up to read: then it stays where they are and offers a
 * way back to the newest. */
export function ConversationThread({ children, follow }: { children: ReactNode; follow: unknown }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const following = useRef(true);
  const [behind, setBehind] = useState(false);
  const toEnd = useCallback(() => {
    const element = ref.current;
    if (!element) return;
    element.scrollTop = element.scrollHeight;
    following.current = true;
    setBehind(false);
  }, []);
  useLayoutEffect(() => {
    if (following.current) toEnd();
    else setBehind(true);
  }, [follow, toEnd]);
  useEffect(() => {
    const onSent = () => { following.current = true; };
    window.addEventListener(SENT_EVENT, onSent);
    return () => window.removeEventListener(SENT_EVENT, onSent);
  }, []);
  function onScroll() {
    const element = ref.current;
    if (!element) return;
    following.current = element.scrollHeight - element.scrollTop - element.clientHeight <= FOLLOW_SLACK_PX;
    setBehind(!following.current);
  }
  return <div className="thread-frame">
    <div className="thread conversation-thread" ref={ref} onScroll={onScroll}><div className="thread-inner">{children}</div></div>
    {behind ? <button type="button" className="btn btn-sm jump-latest" onClick={toEnd}><Icon name="arrowDown" small />Newest</button> : null}
  </div>;
}

export type BubbleTone = "neutral" | "pending" | "live" | "danger" | "complete";

/** One turn's message: `me` is this device's user, `agent` the Agent. */
/** `rich`: laid out parts (a turn's status, actions, details, a Markdown
 * reply) rather than one message's text, whose line breaks are kept. */
export function Bubble({ from, children, meta, tone, label, rich }: { from: "me" | "agent"; children: ReactNode; meta?: ReactNode; tone?: BubbleTone; label?: string; rich?: boolean }) {
  return <div className={`msg ${from === "me" ? "out" : "in"}`} aria-label={label}>
    <div className={`msg-bubble${rich ? " rich" : ""}${tone && tone !== "neutral" ? ` tone-${tone}` : ""}`}>{children}</div>
    {meta ? <div className="msg-meta">{meta}</div> : null}
  </div>;
}

/** An event between turns, not a message. */
export function SystemLine({ children, tone }: { children: ReactNode; tone?: "warn" | "bad" | "ok" }) {
  return <div className={`system-line${tone ? ` ${tone}` : ""}`} role="note">{children}</div>;
}

/** This device's Enter preference, kept current when Settings changes it. */
export function useEnterKeyMode(): EnterKeyMode {
  const [mode, setMode] = useState<EnterKeyMode>(readEnterKeyMode);
  useEffect(() => onEnterKeyModeChange(() => setMode(readEnterKeyMode())), []);
  return mode;
}

/** One send at a time. A second Enter, a held Enter or a click before the
 * first send has settled does nothing; the sender's own `canSend` still
 * decides whether a send is allowed at all. A sender that returns its
 * promise is held until it settles; one that starts its work without
 * returning it is held until `release` runs (the next frame), past any
 * repeated key or double click. */
export function createSendGuard(send: () => void | Promise<unknown>, release: (done: () => void) => void): () => void {
  let inFlight = false;
  return () => {
    if (inFlight) return;
    inFlight = true;
    let result: void | Promise<unknown>;
    try { result = send(); }
    catch (error) { inFlight = false; throw error; }
    if (result && typeof (result as Promise<unknown>).then === "function") {
      void (result as Promise<unknown>).then(() => { inFlight = false; }, () => { inFlight = false; });
    } else {
      release(() => { inFlight = false; });
    }
  };
}

export function useSendGuard(onSend: () => void | Promise<unknown>): () => void {
  const latest = useRef(onSend);
  latest.current = onSend;
  const [submit] = useState(() => createSendGuard(() => latest.current(), (done) => window.requestAnimationFrame(done)));
  return submit;
}

/** The text field of a composer: Enter, IME and size, the same in every
 * conversation and in Share. `pending` keeps the text and the focus but
 * takes no edits while a send is in flight; `disabled` is for a composer
 * that cannot be used at all. */
export function ComposerInput({ value, onChange, onSubmit, placeholder, ariaLabel, disabled, pending, autoFocus, inputRef }: {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  placeholder: string;
  ariaLabel: string;
  disabled?: boolean;
  pending?: boolean;
  autoFocus?: boolean;
  inputRef?: RefObject<HTMLTextAreaElement>;
}) {
  const ownRef = useRef<HTMLTextAreaElement | null>(null);
  const ref = inputRef ?? ownRef;
  const composition = useRef<CompositionState>({ composing: false, endedAt: null });
  const mode = useEnterKeyMode();

  // Grow with the text up to the stylesheet's max-height, then scroll.
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    element.style.height = "auto";
    element.style.height = `${element.scrollHeight + element.offsetHeight - element.clientHeight}px`;
  }, [value, ref]);

  useEffect(() => {
    if (autoFocus && !disabled) ref.current?.focus({ preventScroll: true });
  }, [autoFocus]); // eslint-disable-line react-hooks/exhaustive-deps

  return <textarea
    ref={ref as RefObject<HTMLTextAreaElement>}
    value={value}
    onChange={(event) => { if (!pending) onChange(event.target.value); }}
    onCompositionStart={() => { composition.current = { composing: true, endedAt: null }; }}
    onCompositionEnd={(event) => { composition.current = { composing: false, endedAt: event.timeStamp }; }}
    onKeyDown={(event) => {
      const native = event.nativeEvent;
      const action = composerKeyAction({
        key: event.key, shiftKey: event.shiftKey, altKey: event.altKey, ctrlKey: event.ctrlKey, metaKey: event.metaKey,
        isComposing: native.isComposing, keyCode: native.keyCode, repeat: event.repeat, timeStamp: event.timeStamp,
      }, composition.current, mode);
      if (action === "default") return;
      event.preventDefault();
      if (action === "send") onSubmit();
    }}
    placeholder={placeholder}
    aria-label={ariaLabel}
    aria-keyshortcuts={mode === "send" ? "Enter Meta+Enter Control+Enter" : "Meta+Enter Control+Enter"}
    aria-busy={pending || undefined}
    readOnly={pending}
    disabled={disabled}
    rows={1}
    spellCheck
  />;
}

/** The send button: what it does, and its shortcut, on hover. */
export function SendButton({ onClick, disabled, sending }: { onClick: () => void; disabled: boolean; sending?: boolean }) {
  const mode = useEnterKeyMode();
  const apple = isApplePlatform();
  const shortcut = mode === "send" ? (apple ? "Return" : "Enter") : (apple ? "⌘Return" : "Ctrl+Enter");
  return <button type="button" className="btn btn-primary btn-icon send-button" aria-label={sending ? "Sending" : "Send"} title={`Send (${shortcut})`} disabled={disabled} onClick={onClick}>
    {sending ? <span className="spinner" aria-hidden="true" /> : <Icon name="arrowUp" />}
  </button>;
}

/** Where the next message is written. Enter sends (or adds a line, as this
 * device prefers); Shift+Enter adds a line; Cmd/Ctrl+Enter sends; the Enter
 * that confirms an input method's candidate never sends. When it cannot
 * send, `hint` says why, beside it, in its tone; an untoned hint is
 * followed by the shortcuts, which are the whole hint otherwise.
 * While a send is in flight the text stays, read only, and the focus stays
 * in it, so the next message can follow without a click. */
export function Composer({ value, onChange, onSend, canSend, disabled, placeholder, hint, hintTone, above, bar, sending, label, autoFocus }: {
  value: string;
  onChange: (value: string) => void;
  /** Return the send's promise, so a second Enter waits for it. */
  onSend: () => void | Promise<unknown>;
  canSend: boolean;
  /** The composer cannot be used here at all (not: a turn is running). */
  disabled?: boolean;
  placeholder: string;
  hint?: ReactNode;
  hintTone?: "bad" | "warn";
  /** Fields the message needs first, such as a folder. */
  above?: ReactNode;
  /** Context shown in the composer's bar, such as where it runs. */
  bar?: ReactNode;
  sending?: boolean;
  label: string;
  autoFocus?: boolean;
}) {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const mode = useEnterKeyMode();
  const ready = canSend && !disabled && !sending;
  const submit = useSendGuard(() => {
    if (!ready) return;
    window.dispatchEvent(new CustomEvent(SENT_EVENT));
    return onSend();
  });
  return <div className="composer-wrap">
    <section className={`composer${disabled ? " is-disabled" : ""}`} aria-label={label}>
      {above ? <div className="composer-above">{above}</div> : null}
      <ComposerInput inputRef={inputRef} value={value} onChange={onChange} onSubmit={submit} placeholder={placeholder} ariaLabel={placeholder} disabled={disabled} pending={sending} autoFocus={autoFocus} />
      <div className="composer-bar">
        <span className="composer-target">{bar}</span>
        <span className="spacer" />
        <SendButton disabled={!ready} sending={sending} onClick={() => { submit(); inputRef.current?.focus({ preventScroll: true }); }} />
      </div>
    </section>
    <p className={`composer-hint${hintTone === "bad" ? " error-text" : hintTone === "warn" ? " warn-text" : ""}`} role="status">{hint && hintTone ? hint : hint ? <>{hint}<span className="composer-keys"> · {composerShortcutHint(mode)}</span></> : composerShortcutHint(mode)}</p>
  </div>;
}
