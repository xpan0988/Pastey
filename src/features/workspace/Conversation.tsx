/** The pieces every Agents conversation is made of: a header that says
 * what and where it is, a thread of turns, and a composer. They carry no
 * product state. The same message language as the Share thread: what this
 * device's user wrote on the right, what came back on the left, and events
 * as quiet lines between them. Text from an Agent or another device is
 * plain text only. */
import { useEffect, useRef, type ReactNode } from "react";
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

/** The scrolling thread. It follows the newest turn as it grows. */
export function ConversationThread({ children, follow }: { children: ReactNode; follow: unknown }) {
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const element = ref.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [follow]);
  return <div className="thread conversation-thread" ref={ref}><div className="thread-inner">{children}</div></div>;
}

export type BubbleTone = "neutral" | "pending" | "live" | "danger" | "complete";

/** One turn's message: `me` is this device's user, `agent` the Agent. */
/** `rich`: laid out parts (a turn's status, actions, details) rather than
 * one message's text, whose line breaks are kept. */
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

/** Where the next message is written. Enter sends; Shift+Enter adds a line.
 * When it cannot send, `hint` says why, beside it. */
export function Composer({ value, onChange, onSend, canSend, disabled, placeholder, hint, hintTone, above, bar, sending, label }: {
  value: string;
  onChange: (value: string) => void;
  onSend: () => void;
  canSend: boolean;
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
}) {
  return <div className="composer-wrap">
    <section className="composer" aria-label={label}>
      {above ? <div className="composer-above">{above}</div> : null}
      <textarea
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (canSend) onSend(); } }}
        placeholder={placeholder}
        aria-label={placeholder}
        disabled={disabled}
        rows={2}
      />
      <div className="composer-bar">
        <span className="composer-target">{bar}</span>
        <span className="spacer" />
        <button type="button" className="btn btn-primary btn-icon send-button" aria-label="Send" disabled={!canSend || disabled} onClick={onSend}>{sending ? <span className="spinner" aria-hidden="true" /> : <Icon name="arrowUp" />}</button>
      </div>
    </section>
    <p className={`composer-hint${hintTone === "bad" ? " error-text" : hintTone === "warn" ? " warn-text" : ""}`} role="status">{hint ?? "Enter sends · Shift+Enter adds a line"}</p>
  </div>;
}
