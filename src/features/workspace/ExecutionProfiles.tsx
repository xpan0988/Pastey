import { useEffect, useState } from "react";
import {
  getUserPreferences, setUserPreferences, type PreferenceSections, type UserPreferences,
  describeProfile, getExecutionDefault, listExecutionPresets, setExecutionDefault,
  type ExecutionChoice, type ExecutionDefault, type ExecutionProfile,
} from "../../lib/collaboration";
import { formatTimestamp } from "../../lib/format";
import { Dialog, errorText, Field, Notice } from "./ui";

/** What a profile passes to each Agent, as the user consents to it. */
function ProfileLines({ profile }: { profile: ExecutionProfile }) {
  return <div className="stack" style={{ gap: 6 }}>
    {(["claude_code", "codex"] as const).map((provider) => <div key={provider} className="stack" style={{ gap: 2 }}>
      <strong className="small">{provider === "claude_code" ? "Claude Code" : "Codex"}</strong>
      <ul className="xsmall" style={{ margin: 0, paddingLeft: 16 }}>{describeProfile(profile, provider).map((line) => <li key={line}>{line}</li>)}</ul>
    </div>)}
  </div>;
}

const BOUNDARY_NOTE = "The Agents enforce these settings themselves; Pastey approves nothing. Their sandbox, network list and deny rules are the boundary. Auto mode and auto-review are a model's judgement, not a boundary. These turns use each Agent's account on this device.";

/** Picks a preset or edits one as JSON, and asks for explicit consent to
 * exactly what it passes. `onConfirm` receives a confirmed profile. */
function ProfileEditor({ initial, onChange }: {
  initial: ExecutionProfile | null;
  onChange: (profile: ExecutionProfile | null, problem: string | null) => void;
}) {
  const [presets, setPresets] = useState<ExecutionProfile[]>([]);
  const [text, setText] = useState(initial ? JSON.stringify(initial, null, 2) : "");
  useEffect(() => {
    void listExecutionPresets().then((list) => {
      setPresets(list);
      if (!initial) {
        const unattended = list.find((profile) => profile.name !== "Ask") ?? list[0];
        if (unattended) { setText(JSON.stringify(unattended, null, 2)); onChange(unattended, null); }
      }
    }).catch(() => undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const update = (next: string) => {
    setText(next);
    try { onChange(JSON.parse(next) as ExecutionProfile, null); } catch { onChange(null, "Not valid JSON."); }
  };
  return <div className="stack" style={{ gap: 8 }}>
    <div className="row row-wrap" style={{ gap: 6 }}>
      {presets.filter((profile) => profile.name !== "Ask").map((profile) => <button key={profile.name} type="button" className="btn btn-sm" onClick={() => update(JSON.stringify(profile, null, 2))}>Start from {profile.name}</button>)}
    </div>
    <Field label="Profile (native settings, as JSON)" htmlFor="execution-profile">
      <textarea id="execution-profile" className="input mono xsmall" rows={10} value={text} spellCheck={false} onChange={(event) => update(event.target.value)} />
    </Field>
  </div>;
}

function ConsentBlock({ profile, confirmed, setConfirmed }: { profile: ExecutionProfile; confirmed: boolean; setConfirmed: (value: boolean) => void }) {
  return <div className="stack" style={{ gap: 8 }}>
    <ProfileLines profile={profile} />
    <Notice tone="warn">{BOUNDARY_NOTE}</Notice>
    <label className="row small" style={{ gap: 8 }}>
      <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
      <span>I allow collaboration members to run with “{profile.name}” without asking me each time.</span>
    </label>
  </div>;
}

/** Settings: the user's default execution profile for every collaboration
 * member, authorised once, shown, editable and revocable. */
export function ExecutionDefaultSettings() {
  const [current, setCurrent] = useState<ExecutionDefault | null | undefined>(undefined);
  const [editing, setEditing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => { void getExecutionDefault().then(setCurrent).catch(() => setCurrent(null)); }, []);
  return <section className="section" aria-label="Collaboration execution">
    <div className="section-head-row"><div className="section-head"><h2>Unattended collaboration</h2>
      <p>What collaboration members on this device may do without asking you, passed to each Agent's own permissions and sandbox. New collaborations inherit it; a collaboration or a member can choose otherwise.</p></div>
      <div className="row" style={{ gap: 6 }}>
        <button type="button" className="btn btn-sm" onClick={() => setEditing(true)}>{current ? "Change…" : "Set up…"}</button>
        {current ? <button type="button" className="btn btn-sm btn-ghost" onClick={() => void setExecutionDefault(null, false).then(setCurrent).catch((cause) => setError(errorText(cause, "It could not be revoked.")))}>Revoke</button> : null}
      </div></div>
    {error ? <Notice tone="bad">{error}</Notice> : null}
    <div className="card"><div className="card-body stack" style={{ gap: 6 }}>
      {current === undefined ? <span className="small subtle">Reading…</span>
        : current === null ? <span className="small">Ask: nothing is passed, and each Agent uses its own settings. A headless Claude Code turn declines whatever would prompt.</span>
        : <><span className="small"><strong>{current.profile.name}</strong> · authorised {formatTimestamp(current.authorizedAt * 1000)} · configured, not verified</span><ProfileLines profile={current.profile} /></>}
    </div></div>
    {editing ? <DefaultDialog initial={current?.profile ?? null} onClose={() => setEditing(false)}
      onSaved={(saved) => { setCurrent(saved); setEditing(false); }} /> : null}
  </section>;
}

function DefaultDialog({ initial, onClose, onSaved }: { initial: ExecutionProfile | null; onClose: () => void; onSaved: (saved: ExecutionDefault | null) => void }) {
  const [profile, setProfile] = useState<ExecutionProfile | null>(initial);
  const [problem, setProblem] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [pending, setPending] = useState(false);
  const save = async () => {
    if (!profile) return;
    setPending(true);
    try { onSaved(await setExecutionDefault(profile, confirmed)); } catch (cause) { setProblem(errorText(cause, "It could not be saved.")); } finally { setPending(false); }
  };
  return <Dialog title="Default for collaboration members" onClose={onClose}
    description="Every member of every collaboration on this device runs with this, from its next turn, unless its collaboration or the member chooses otherwise."
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !profile || !confirmed || problem !== null} onClick={() => void save()}>Authorise</button></>}>
    <div className="stack-lg">
      <ProfileEditor initial={initial} onChange={(next, nextProblem) => { setProfile(next); setProblem(nextProblem); setConfirmed(false); }} />
      {problem ? <Notice tone="bad">{problem}</Notice> : null}
      {profile && !problem ? <ConsentBlock profile={profile} confirmed={confirmed} setConfirmed={setConfirmed} /> : null}
    </div>
  </Dialog>;
}

/** A collaboration's or a member's execution choice. */
export function ExecutionChoiceDialog({ title, scope, initial, pending, onClose, onSave }: {
  title: string;
  /** "collaboration" inherits from the default; "member" from its collaboration. */
  scope: "collaboration" | "member";
  initial: ExecutionChoice;
  pending: boolean;
  onClose: () => void;
  onSave: (choice: ExecutionChoice) => void;
}) {
  const [kind, setKind] = useState<ExecutionChoice["kind"]>(initial.kind);
  const [profile, setProfile] = useState<ExecutionProfile | null>(initial.kind === "profile" ? initial.profile : null);
  const [problem, setProblem] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const ready = kind !== "profile" || (profile !== null && problem === null && confirmed);
  return <Dialog title={title} onClose={onClose}
    description="Applies from the next turn. A turn you type in a member's conversation runs with it too."
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !ready}
        onClick={() => onSave(kind === "profile" && profile ? { kind, profile, confirmed } : kind === "ask" ? { kind: "ask" } : { kind: "inherit" })}>Save</button></>}>
    <div className="stack-lg">
      <div className="stack" style={{ gap: 4 }}>
        {([
          ["inherit", scope === "collaboration" ? "Your default (Settings)" : "The collaboration's choice"],
          ["ask", "Ask: pass nothing; each Agent's own settings"],
          ["profile", "A profile of its own"],
        ] as const).map(([value, label]) => <label key={value} className="row small" style={{ gap: 8 }}>
          <input type="radio" name="execution-kind" checked={kind === value} onChange={() => { setKind(value); setConfirmed(false); }} />
          <span>{label}</span>
        </label>)}
      </div>
      {kind === "profile" ? <>
        <ProfileEditor initial={profile} onChange={(next, nextProblem) => { setProfile(next); setProblem(nextProblem); setConfirmed(false); }} />
        {problem ? <Notice tone="bad">{problem}</Notice> : null}
        {profile && !problem ? <ConsentBlock profile={profile} confirmed={confirmed} setConfirmed={setConfirmed} /> : null}
      </> : null}
    </div>
  </Dialog>;
}

const PREFERENCE_FIELDS: ReadonlyArray<{ key: keyof PreferenceSections; label: string; hint: string }> = [
  { key: "restrictions", label: "Never do", hint: "Hard restrictions Agents must follow. To have them enforced, also add deny rules to the execution profile." },
  { key: "delegated", label: "Agents may decide on their own", hint: "Routine, reversible decisions they make and record without asking you." },
  { key: "preferences", label: "Preferences", hint: "Product, architecture, engineering, technology, platform, risk tolerance." },
  { key: "decisions", label: "Decisions already made", hint: "What you decided before, to be respected." },
];

/** Settings: the user's preference profile for Agents. Pastey stores,
 * versions and delivers it; Agents interpret it; it grants nothing. */
export function PreferencesSettings() {
  const [current, setCurrent] = useState<UserPreferences | null | undefined>(undefined);
  const [draft, setDraft] = useState<PreferenceSections>({ restrictions: "", delegated: "", preferences: "", decisions: "" });
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState<{ tone: "ok" | "bad"; text: string } | null>(null);
  useEffect(() => {
    void getUserPreferences().then((read) => {
      setCurrent(read);
      if (read) setDraft({ restrictions: read.restrictions, delegated: read.delegated, preferences: read.preferences, decisions: read.decisions });
    }).catch(() => setCurrent(null));
  }, []);
  const save = async () => {
    setPending(true); setMessage(null);
    try { const saved = await setUserPreferences(draft); setCurrent(saved); setMessage({ tone: "ok", text: `Saved as version ${saved.version}. Each member gets it in full on its next turn.` }); }
    catch (cause) { setMessage({ tone: "bad", text: errorText(cause, "It could not be saved.") }); }
    finally { setPending(false); }
  };
  return <section className="section" aria-label="Preferences for Agents">
    <div className="section-head-row"><div className="section-head"><h2>Preferences for Agents</h2>
      <p>What collaboration members should assume when they read an objective, plan and decide. They decide routine, reversible matters themselves within what you delegate, and ask you only about material uncertainty, risk, or decisions outside it. Preferences are guidance: they never let an Agent do more.</p></div>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || current === undefined} onClick={() => void save()}>Save</button></div>
    {message ? <Notice tone={message.tone}>{message.text}</Notice> : null}
    <div className="card"><div className="card-body stack" style={{ gap: 10 }}>
      {current ? <span className="xsmall subtle">Version {current.version} · saved {formatTimestamp(current.updatedAt * 1000)}</span> : null}
      {PREFERENCE_FIELDS.map((field) => <Field key={field.key} label={field.label} htmlFor={`preferences-${field.key}`} hint={field.hint}>
        <textarea id={`preferences-${field.key}`} className="input small" rows={3} maxLength={1200} value={draft[field.key]}
          onChange={(event) => setDraft((current) => ({ ...current, [field.key]: event.target.value }))} />
      </Field>)}
    </div></div>
  </section>;
}
