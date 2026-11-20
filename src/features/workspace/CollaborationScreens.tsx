import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  approveCollaborationMessages, basisLabel, createCollaboration, deleteCollaboration, dismissCollaborationEntry, folderName,
  getCollaboration, journalCount, journalEntries, listCollaborations, memberStateLabel, providerLabel, resumeCollaborationMember,
  sendCollaborationMessage, setCollaborationState, statusLabel, suggestAlias, turnOutcomeLabel, USER_ALIAS, validAlias,
  waitingByRecipient, type CollaborationEntry, type CollaborationMember, type CollaborationMemberInput, type CollaborationProvider,
  type CollaborationSummary, type CollaborationView, type JournalFilter,
} from "../../lib/collaboration";
import { formatTimestamp } from "../../lib/format";
import { Badge, Dialog, Dot, EmptyState, errorText, Field, Icon, ListRow, MenuButton, Notice, Segmented } from "./ui";

/** Device-wide: Agent collaborations of this device and their Journals. */
export function CollaborationsScreen({ menuButton }: { menuButton?: ReactNode }) {
  const [list, setList] = useState<CollaborationSummary[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try { setList(await listCollaborations()); setError(null); } catch (cause) { setError(errorText(cause, "Collaborations could not be read.")); }
  }, []);
  useEffect(() => {
    if (selected) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [refresh, selected]);
  if (selected) return <CollaborationDetail id={selected} menuButton={menuButton} onBack={() => setSelected(null)} />;
  return (
    <section className="page">
      <header className="page-header">{menuButton}<div className="page-header-main"><h1>Collaborations</h1><p>Agents on this device that leave each other messages. Nothing reaches an Agent mid-turn, and a message between Agents waits for your approval.</p></div>
        <div className="page-header-actions"><button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)}><Icon name="plus" small />New collaboration</button></div></header>
      <div className="page-body"><div className="content stack-lg">
        {error ? <Notice tone="bad">{error}</Notice> : null}
        {list.length === 0 ? <EmptyState title="No collaborations yet" actions={<button type="button" className="btn btn-sm" onClick={() => setCreating(true)}><Icon name="plus" small />New collaboration</button>}>
          Group two or more Codex or Claude Code conversations of this device. Each Agent can then leave the others a message when its turn ends.
        </EmptyState> : <div className="list">
          {list.map((item) => <ListRow key={item.id} title={item.name}
            meta={<span>{item.members.map((member) => `${member.alias} (${providerLabel(member.provider)})`).join(" · ")}</span>}
            side={<span className="row" style={{ gap: 6 }}>
              {item.state !== "active" ? <Badge>{item.state === "paused" ? "Paused" : "Ended"}</Badge> : null}
              {item.needsYou > 0 ? <Badge tone="warn">{item.needsYou} need you</Badge> : null}
              <Icon name="chevron" small />
            </span>}
            onClick={() => setSelected(item.id)} />)}
        </div>}
      </div></div>
      {creating ? <CreateCollaborationDialog onClose={() => setCreating(false)} onCreated={(view) => { setCreating(false); setSelected(view.id); }} /> : null}
    </section>
  );
}

function CollaborationDetail({ id, menuButton, onBack }: { id: string; menuButton?: ReactNode; onBack: () => void }) {
  const [view, setView] = useState<CollaborationView | null>(null);
  const [filter, setFilter] = useState<JournalFilter>("activity");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [confirming, setConfirming] = useState<"end" | "delete" | null>(null);
  const refresh = useCallback(async () => {
    try { setView(await getCollaboration(id)); setError(null); } catch (cause) { setError(errorText(cause, "This collaboration could not be read.")); }
  }, [id]);
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2000);
    return () => window.clearInterval(timer);
  }, [refresh]);
  const act = async (action: () => Promise<CollaborationView | void>) => {
    setPending(true);
    try { const next = await action(); if (next) setView(next); setError(null); } catch (cause) { setError(errorText(cause, "That did not work.")); } finally { setPending(false); }
  };
  const back = <button type="button" className="btn btn-ghost btn-icon btn-sm" aria-label="Back to collaborations" onClick={onBack}><Icon name="back" /></button>;
  if (!view) {
    return <section className="page"><header className="page-header">{menuButton}{back}<div className="page-header-main"><h1>Collaboration</h1></div></header>
      <div className="page-body"><div className="content">{error ? <Notice tone="bad">{error}</Notice> : <p className="muted">Loading…</p>}</div></div></section>;
  }
  const ended = view.state === "ended";
  const waiting = waitingByRecipient(view.entries);
  const shown = journalEntries(view.entries, filter);
  const filters: ReadonlyArray<{ value: JournalFilter; label: string }> = (["activity", "summaries", "needs-you"] as const).map((value) => ({
    value, label: `${value === "activity" ? "Activity" : value === "summaries" ? "Summaries" : "Needs you"} ${journalCount(view.entries, value)}`,
  }));
  return (
    <section className="page">
      <header className="page-header">{menuButton}{back}<div className="page-header-main"><h1>{view.name}</h1><p>{view.goal || "No goal given."}</p></div>
        <div className="page-header-actions">
          {view.state === "active" ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void act(() => setCollaborationState(id, "paused"))}>Pause</button> : null}
          {view.state === "paused" ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void act(() => setCollaborationState(id, "active"))}>Resume</button> : null}
          <MenuButton label="More collaboration actions">{(close) => <>
            {!ended ? <button type="button" role="menuitem" onClick={() => { close(); setConfirming("end"); }}><Icon name="close" /><span>End collaboration<small>Members become ordinary conversations; the Journal stays</small></span></button> : null}
            <button type="button" role="menuitem" onClick={() => { close(); setConfirming("delete"); }}><Icon name="flame" /><span>Delete…<small>Removes the Journal; conversations stay</small></span></button>
          </>}</MenuButton>
        </div></header>
      <div className="page-body"><div className="content stack-lg">
        {error ? <Notice tone="bad">{error}</Notice> : null}
        {view.state === "paused" ? <Notice tone="warn" title="Paused">No member turn starts. Messages still arrive and wait.</Notice> : null}
        {ended ? <Notice title="Ended">Its members are ordinary conversations again. The Journal stays until you delete it.</Notice> : null}
        <div className="row row-wrap" style={{ gap: 12, alignItems: "stretch" }}>
          {view.members.map((member) => <MemberCard key={member.alias} member={member} waiting={waiting[member.alias] ?? []} disabled={pending || ended}
            onApprove={(ids) => void act(() => approveCollaborationMessages(id, ids))}
            onResume={() => void act(() => resumeCollaborationMember(id, member.alias))} />)}
        </div>
        {!ended ? <Composer members={view.members} disabled={pending} onSend={(to, text) => act(() => sendCollaborationMessage(id, to, text))} /> : null}
        <Segmented label="Show" value={filter} onChange={setFilter} options={filters} />
        {shown.length === 0 ? <EmptyState title={filter === "needs-you" ? "Nothing needs you" : "Nothing yet"}>
          {filter === "summaries" ? "An Agent writes a summary when its turn finishes a stage, makes a decision or gets blocked." : "Turns, messages and summaries appear here as they happen."}
        </EmptyState> : <div className="stack">
          {shown.map((entry) => <EntryCard key={entry.id} entry={entry} members={view.members} disabled={pending || ended}
            onApprove={() => void act(() => approveCollaborationMessages(id, [entry.id]))}
            onDismiss={() => void act(() => dismissCollaborationEntry(id, entry.id))} />)}
        </div>}
      </div></div>
      {confirming ? <Dialog title={confirming === "end" ? `End ${view.name}?` : `Delete ${view.name}?`} onClose={() => setConfirming(null)}
        description={confirming === "end" ? "Waiting messages are dismissed and no member turn starts. Its members become ordinary conversations again; the Journal stays." : "The collaboration and its Journal are removed from this device. The members' conversations are not changed."}
        footer={<><button type="button" className="btn btn-sm" onClick={() => setConfirming(null)}>Cancel</button>
          <button type="button" className="btn btn-sm btn-danger" disabled={pending} onClick={() => {
            const which = confirming; setConfirming(null);
            void act(async () => {
              if (which === "end") return setCollaborationState(id, "ended");
              await deleteCollaboration(id); onBack();
            });
          }}>{confirming === "end" ? "End" : "Delete"}</button></>}>
        <span />
      </Dialog> : null}
    </section>
  );
}

function MemberCard({ member, waiting, disabled, onApprove, onResume }: { member: CollaborationMember; waiting: string[]; disabled: boolean; onApprove: (ids: string[]) => void; onResume: () => void }) {
  const tone = member.paused ? "warn" : member.busy ? "live" : "neutral";
  return <div className="card" style={{ flex: "1 1 220px", minWidth: 220 }}><div className="card-body stack" style={{ gap: 6 }}>
    <div className="row" style={{ gap: 8 }}><Dot tone={tone} pulse={member.busy && !member.paused} /><strong>{member.alias}</strong><span className="small muted">{providerLabel(member.provider)}</span></div>
    <span className="xsmall subtle mono" title={member.workspace}>{folderName(member.workspace)}</span>
    {member.role ? <span className="small">{member.role}</span> : null}
    <span className="small muted">{memberStateLabel(member)}</span>
    <div className="row row-wrap" style={{ gap: 6 }}>
      {waiting.length > 0 ? <button type="button" className="btn btn-sm btn-primary" disabled={disabled} onClick={() => onApprove(waiting)}>Approve {waiting.length} for {member.alias}</button> : null}
      {member.paused ? <button type="button" className="btn btn-sm" disabled={disabled} onClick={onResume}>Resume {member.alias}</button> : null}
    </div>
  </div></div>;
}

function Composer({ members, disabled, onSend }: { members: CollaborationMember[]; disabled: boolean; onSend: (to: string, text: string) => Promise<void> }) {
  const [to, setTo] = useState(members[0]?.alias ?? "");
  const [text, setText] = useState("");
  const send = async () => {
    if (!to || !text.trim()) return;
    await onSend(to, text.trim());
    setText("");
  };
  return <div className="card"><div className="card-body stack" style={{ gap: 8 }}>
    <div className="row" style={{ gap: 8 }}>
      <label className="small" htmlFor="collaboration-to">Message</label>
      <select id="collaboration-to" className="select select-sm" value={to} disabled={disabled} onChange={(event) => setTo(event.target.value)}>
        {members.map((member) => <option key={member.alias} value={member.alias}>{member.alias} ({providerLabel(member.provider)})</option>)}
      </select>
      <span className="xsmall subtle">Delivered when its current turn ends. Your messages need no approval.</span>
    </div>
    <textarea className="textarea" rows={3} maxLength={4096} value={text} disabled={disabled} placeholder={`Write to ${to || "a member"}…`}
      onChange={(event) => setText(event.target.value)}
      onKeyDown={(event) => { if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) void send(); }} />
    <div className="row" style={{ justifyContent: "flex-end" }}><button type="button" className="btn btn-sm btn-primary" disabled={disabled || !text.trim() || !to} onClick={() => void send()}><Icon name="arrowUp" small />Send</button></div>
  </div></div>;
}

function EntryCard({ entry, members, disabled, onApprove, onDismiss }: { entry: CollaborationEntry; members: CollaborationMember[]; disabled: boolean; onApprove: () => void; onDismiss: () => void }) {
  const status = statusLabel(entry.status);
  const provider = (alias?: string) => members.find((member) => member.alias === alias)?.provider;
  const meta = <span className="xsmall subtle">{formatTimestamp(entry.at)} · {basisLabel(entry.basis)}{status ? ` · ${status}` : ""}</span>;
  const actions = entry.status?.state === "waiting"
    ? <div className="row" style={{ gap: 6 }}><button type="button" className="btn btn-sm btn-primary" disabled={disabled} onClick={onApprove}>Approve</button><button type="button" className="btn btn-sm" disabled={disabled} onClick={onDismiss}>Dismiss</button></div>
    : entry.status?.state === "open" ? <div className="row"><button type="button" className="btn btn-sm" disabled={disabled} onClick={onDismiss}>Done</button></div> : null;
  const body = entry.body;
  switch (body.type) {
    case "turn":
      return <div className="stack" style={{ gap: 2 }}>
        <span className="small"><strong>{entry.member}</strong> · {body.origin === "collaboration" ? "turn with messages" : "turn"} {turnOutcomeLabel(body.state, body.code)}</span>{meta}
      </div>;
    case "message": {
      const fromProvider = provider(body.from);
      return <div className="card"><div className="card-body stack" style={{ gap: 6 }}>
        <span className="small"><strong>{body.from === USER_ALIAS ? "You" : body.from}</strong>{fromProvider ? ` (${providerLabel(fromProvider)})` : ""} → <strong>{body.to === USER_ALIAS ? "you" : body.to}</strong>{body.subject ? `: ${body.subject}` : ""}</span>
        <div className="small" style={{ whiteSpace: "pre-wrap" }}>{body.body}</div>
        {meta}{actions}
      </div></div>;
    }
    case "summary":
      return <div className="card"><div className="card-body stack" style={{ gap: 4 }}>
        <span className="small"><strong>{entry.member}</strong> · stage summary</span>
        {(["done", "decisions", "open", "next"] as const).map((field) => body.summary[field] ? <div key={field} className="small" style={{ whiteSpace: "pre-wrap" }}><span className="muted">{field}: </span>{body.summary[field]}</div> : null)}
        {meta}
      </div></div>;
    case "attention":
      return <Notice tone={entry.status?.state === "open" ? "warn" : "neutral"} action={actions}><span className="stack" style={{ gap: 2 }}><span>{body.text}</span>{meta}</span></Notice>;
    case "user": {
      const action = body.action;
      const text = action.type === "created" ? "You created this collaboration" : action.type === "paused" ? "You paused it" : action.type === "resumed" ? "You resumed it" : action.type === "ended" ? "You ended it" : `You resumed ${action.alias}`;
      return <div className="stack" style={{ gap: 2 }}><span className="small">{text}</span>{meta}</div>;
    }
    case "member":
      return <div className="stack" style={{ gap: 2 }}><span className="small"><strong>{body.alias}</strong> joined ({providerLabel(body.provider)}){body.role ? ` · ${body.role}` : ""}</span>{meta}</div>;
  }
}

interface DraftMember extends CollaborationMemberInput { key: number }

function CreateCollaborationDialog({ onClose, onCreated }: { onClose: () => void; onCreated: (view: CollaborationView) => void }) {
  const [name, setName] = useState("");
  const [goal, setGoal] = useState("");
  const [members, setMembers] = useState<DraftMember[]>([
    { key: 1, alias: "", provider: "codex", workspace: "", role: "" },
    { key: 2, alias: "", provider: "claude_code", workspace: "", role: "" },
  ]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const update = (key: number, change: Partial<DraftMember>) => setMembers((current) => current.map((member) => member.key === key ? { ...member, ...change } : member));
  const choose = async (member: DraftMember) => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    const taken = members.filter((other) => other.key !== member.key).map((other) => other.alias);
    update(member.key, { workspace: chosen, alias: member.alias || suggestAlias(chosen, taken) });
  };
  const aliases = members.map((member) => member.alias);
  const problem = !name.trim() ? "Give the collaboration a name."
    : members.some((member) => !member.workspace) ? "Choose a folder for every member."
    : members.some((member) => !validAlias(member.alias)) ? `Aliases use lowercase letters, digits, - and _ (and not "${USER_ALIAS}").`
    : new Set(aliases).size !== aliases.length ? "Every member needs its own alias." : null;
  const create = async () => {
    setPending(true);
    try {
      onCreated(await createCollaboration(name.trim(), goal.trim(), members.map(({ key: _key, ...member }) => member)));
    } catch (cause) { setError(errorText(cause, "The collaboration could not be created.")); } finally { setPending(false); }
  };
  return <Dialog wide title="New collaboration" onClose={onClose}
    description="Each member is Pastey's Codex or Claude Code conversation in a folder of this device. Every turn they run carries a short note about the collaboration."
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !!problem} onClick={() => void create()}>Create</button></>}>
    <div className="stack-lg">
      <Field label="Name" htmlFor="collaboration-name"><input id="collaboration-name" className="input" maxLength={80} value={name} onChange={(event) => setName(event.target.value)} placeholder="orders-v2" /></Field>
      <Field label="Goal" hint="One line every member sees." htmlFor="collaboration-goal"><input id="collaboration-goal" className="input" maxLength={300} value={goal} onChange={(event) => setGoal(event.target.value)} placeholder="Move GET /v2/orders to cursor pagination" /></Field>
      {members.map((member, index) => <div key={member.key} className="card"><div className="card-body stack" style={{ gap: 8 }}>
        <div className="row" style={{ gap: 8, justifyContent: "space-between" }}><strong className="small">Member {index + 1}</strong>
          {members.length > 2 ? <button type="button" className="btn btn-ghost btn-sm" onClick={() => setMembers((current) => current.filter((other) => other.key !== member.key))}>Remove</button> : null}</div>
        <div className="row row-wrap" style={{ gap: 8 }}>
          <select className="select select-sm" aria-label={`Member ${index + 1} Agent`} value={member.provider} onChange={(event) => update(member.key, { provider: event.target.value as CollaborationProvider })}>
            <option value="codex">Codex</option><option value="claude_code">Claude Code</option>
          </select>
          <button type="button" className="btn btn-sm" onClick={() => void choose(member)}><Icon name="folder" small />{member.workspace ? folderName(member.workspace) : "Choose folder…"}</button>
          <input className="input input-sm" aria-label={`Member ${index + 1} alias`} maxLength={32} value={member.alias} placeholder="alias" onChange={(event) => update(member.key, { alias: event.target.value.toLowerCase() })} />
        </div>
        <input className="input input-sm" aria-label={`Member ${index + 1} role`} maxLength={200} value={member.role} placeholder="Role, e.g. owns api/; does not edit web/" onChange={(event) => update(member.key, { role: event.target.value })} />
      </div></div>)}
      {members.length < 6 ? <button type="button" className="btn btn-sm" onClick={() => setMembers((current) => [...current, { key: Math.max(...current.map((member) => member.key)) + 1, alias: "", provider: "codex", workspace: "", role: "" }])}><Icon name="plus" small />Add member</button> : null}
      {problem ? <span className="small muted">{problem}</span> : null}
      {error ? <Notice tone="bad">{error}</Notice> : null}
    </div>
  </Dialog>;
}
