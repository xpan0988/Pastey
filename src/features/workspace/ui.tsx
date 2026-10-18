/** Shared presentation primitives. They carry no product state: every
 * decision stays with the feature that renders them. */
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";

const ICON_PATHS = {
  plus: "M12 5v14M5 12h14",
  inbox: "M4 13l2.5-7h11L20 13M4 13v5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-5M4 13h4.5l1.2 2h4.6l1.2-2H20",
  settings: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z",
  arrowUp: "M12 19V5M6 11l6-6 6 6",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  panel: "M4 5h16v14H4zM15 5v14",
  close: "M6 6l12 12M18 6L6 18",
  check: "M5 12.5l4.5 4.5L19 7.5",
  chevron: "M9 6l6 6-6 6",
  back: "M15 6l-6 6 6 6",
  folder: "M3.5 7a1.5 1.5 0 0 1 1.5-1.5h4l2 2h8a1.5 1.5 0 0 1 1.5 1.5v8.5a1.5 1.5 0 0 1-1.5 1.5H5a1.5 1.5 0 0 1-1.5-1.5z",
  file: "M14 3.5H7a1.5 1.5 0 0 0-1.5 1.5v14A1.5 1.5 0 0 0 7 20.5h10a1.5 1.5 0 0 0 1.5-1.5V8zM14 3.5V8h4.5",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM20 20l-4-4",
  terminal: "M4 5h16v14H4zM8 10l3 2.5L8 15M13 15h3",
  copy: "M9 9h10v10H9zM5 15V5h10",
  refresh: "M20 11a8 8 0 0 0-14.6-4.5L4 8M4 4v4h4M4 13a8 8 0 0 0 14.6 4.5L20 16M20 20v-4h-4",
  shield: "M12 3l7.5 3v5.5c0 4.3-3.1 8.2-7.5 9.5-4.4-1.3-7.5-5.2-7.5-9.5V6z",
  device: "M4 5h16v11H4zM9 20h6M12 16v4",
  link: "M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1",
  alert: "M12 4l9 16H3zM12 10v4M12 17h.01",
  info: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 11v5M12 8h.01",
  menu: "M4 7h16M4 12h16M4 17h16",
  agent: "M12 3l1.8 4.6L18.5 9l-4.7 1.6L12 15l-1.8-4.4L5.5 9l4.7-1.4zM18 15l.9 2.1L21 18l-2.1.9L18 21l-.9-2.1L15 18l2.1-.9z",
  sessions: "M4 6h16M4 12h10M4 18h7M17 15l3 3-3 3",
  download: "M12 4v11M7 10l5 5 5-5M5 20h14",
  flame: "M12 3c1 3.5 5 5.4 5 10a5 5 0 0 1-10 0c0-2.5 1.4-4 2.6-5.3.3 1.6 1 2.6 2 3 0-3 .4-5.5.4-7.7z",
  key: "M15 9a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM12 12v8M12 16h3M12 19h2",
} as const;

export type IconName = keyof typeof ICON_PATHS;

export function Icon({ name, small, label }: { name: IconName; small?: boolean; label?: string }) {
  return <svg className={`icon${small ? " icon-sm" : ""}`} viewBox="0 0 24 24" aria-hidden={label ? undefined : true} role={label ? "img" : undefined} aria-label={label}><path d={ICON_PATHS[name]} /></svg>;
}

export type Tone = "neutral" | "ok" | "live" | "warn" | "bad";

export function Dot({ tone = "neutral", pulse }: { tone?: Tone; pulse?: boolean }) {
  return <i className={`dot ${tone === "neutral" ? "" : tone}${pulse ? " pulse" : ""}`} aria-hidden="true" />;
}

export function Badge({ tone = "neutral", children }: { tone?: Tone | "accent"; children: ReactNode }) {
  return <span className={`badge ${tone === "neutral" ? "" : tone}`}>{children}</span>;
}

export function Notice({ tone = "neutral", title, children, action }: { tone?: "neutral" | "info" | "warn" | "bad" | "ok"; title?: ReactNode; children?: ReactNode; action?: ReactNode }) {
  const icon: IconName = tone === "bad" || tone === "warn" ? "alert" : tone === "ok" ? "check" : "info";
  return <div className={`notice ${tone === "neutral" ? "" : tone}`} role={tone === "bad" ? "alert" : undefined}>
    <Icon name={icon} />
    <div>{title ? <strong>{title}</strong> : null}{children ? <span>{children}</span> : null}{action ? <span>{action}</span> : null}</div>
  </div>;
}

export function Switch({ checked, disabled, label, onChange }: { checked: boolean; disabled?: boolean; label: string; onChange: (value: boolean) => void }) {
  return <button type="button" className={`switch${checked ? " on" : ""}`} role="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}><i /></button>;
}

export function Segmented<T extends string>({ value, options, onChange, disabled, label }: { value: T; options: ReadonlyArray<{ value: T; label: string }>; onChange: (value: T) => void; disabled?: boolean; label: string }) {
  return <div className="segmented" role="radiogroup" aria-label={label}>
    {options.map((option) => <button key={option.value} type="button" role="radio" aria-checked={value === option.value} className={value === option.value ? "active" : ""} disabled={disabled} onClick={() => onChange(option.value)}>{option.label}</button>)}
  </div>;
}

export function Tabs<T extends string>({ value, tabs, onChange, label }: { value: T; tabs: ReadonlyArray<{ value: T; label: string; badge?: ReactNode }>; onChange: (value: T) => void; label: string }) {
  return <div className="tabs" role="tablist" aria-label={label}>
    {tabs.map((tab) => <button key={tab.value} type="button" role="tab" aria-selected={value === tab.value} className={value === tab.value ? "active" : ""} onClick={() => onChange(tab.value)}>{tab.label}{tab.badge ? <span className="count">{tab.badge}</span> : null}</button>)}
  </div>;
}

export function Field({ label, hint, children, htmlFor }: { label: ReactNode; hint?: ReactNode; children: ReactNode; htmlFor?: string }) {
  return <div className="field">
    <label className="field-label" htmlFor={htmlFor}>{label}</label>
    {children}
    {hint ? <span className="field-hint">{hint}</span> : null}
  </div>;
}

/** A small popover menu, closed by Escape or a click outside. */
export function MenuButton({ label, icon = "more", children, align = "right", up = false, disabled, buttonClassName = "btn btn-ghost btn-icon" }: { label: string; icon?: IconName; children: (close: () => void) => ReactNode; align?: "left" | "right"; up?: boolean; disabled?: boolean; buttonClassName?: string }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => { if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false); };
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => { document.removeEventListener("mousedown", onDown); document.removeEventListener("keydown", onKey); };
  }, [open]);
  return <div className="popover" ref={ref}>
    <button type="button" className={buttonClassName} aria-label={label} title={label} aria-haspopup="menu" aria-expanded={open} disabled={disabled} onClick={() => setOpen(!open)}><Icon name={icon} /></button>
    {open ? <div className={`menu${align === "left" ? " align-left" : ""}${up ? " open-up" : ""}`} role="menu">{children(() => setOpen(false))}</div> : null}
  </div>;
}

/** A modal dialog for a focused, self-contained flow. */
export function Dialog({ title, description, onClose, children, footer, labelledBy = "dialog-title" }: { title: ReactNode; description?: ReactNode; onClose: () => void; children: ReactNode; footer?: ReactNode; labelledBy?: string }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  return <>
    <div className="scrim" onClick={onClose} />
    <section className="dialog" role="dialog" aria-modal="true" aria-labelledby={labelledBy}>
      <header className="dialog-header">
        <div><h2 id={labelledBy}>{title}</h2>{description ? <p>{description}</p> : null}</div>
        <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Close" onClick={onClose}><Icon name="close" /></button>
      </header>
      <div className="dialog-body">{children}</div>
      {footer ? <footer className="dialog-footer">{footer}</footer> : null}
    </section>
  </>;
}

export function ListRow({ title, meta, side, onClick, selected, disabled, leading }: { title: ReactNode; meta?: ReactNode; side?: ReactNode; onClick?: () => void; selected?: boolean; disabled?: boolean; leading?: ReactNode }) {
  const body = <>{leading}<span className="list-row-main"><span className="list-row-title">{title}</span>{meta ? <span className="list-row-meta">{meta}</span> : null}</span>{side ? <span className="list-row-side">{side}</span> : null}</>;
  return onClick
    ? <button type="button" className={`list-row${selected ? " selected" : ""}`} disabled={disabled} onClick={onClick}>{body}</button>
    : <div className="list-row">{body}</div>;
}

export function Checklist({ items }: { items: ReadonlyArray<{ id: string; label: string; met: boolean }> }) {
  return <ul className="checklist" aria-label="Requirements">
    {items.map((item) => <li key={item.id} className={item.met ? "met" : ""}><span className="mark" aria-hidden="true">{item.met ? <Icon name="check" /> : null}</span><span>{item.label}<span className="sr-only">{item.met ? " (done)" : " (needed)"}</span></span></li>)}
  </ul>;
}

export function EmptyState({ title, children, actions }: { title: ReactNode; children?: ReactNode; actions?: ReactNode }) {
  return <div className="empty"><h2>{title}</h2>{children ? <p>{children}</p> : null}{actions ? <div className="row-wrap">{actions}</div> : null}</div>;
}

export function errorText(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : typeof error === "string" && error ? error : fallback;
}
