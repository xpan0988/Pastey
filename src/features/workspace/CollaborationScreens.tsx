import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  activityCounts, approveCollaborationMessages, basisLabel, createCollaboration, currentStep, DEFAULT_AUTO_HOURS, DEFAULT_AUTO_TURNS,
  deleteCollaboration, dismissCollaborationEntry, elapsedLabel, folderName, getCollaboration, grantEndLabel, grantUsable, journalCount,
  journalEntries, listCollaborations, MAX_AUTO_HOURS, MAX_AUTO_TURNS, memberStateLabel, policyLabel, providerLabel,
  resumeCollaborationMember, sendCollaborationMessage, setCollaborationGrant, setCollaborationState, statusLabel, suggestAlias,
  turnOutcomeLabel, USER_ALIAS, validAlias, waitingByRecipient, withdrawCollaborationGrant, type CollaborationEntry,
  type CollaborationMember, type CollaborationMemberInput, type CollaborationProvider, type CollaborationSummary, type CollaborationView,
  type JournalFilter, type RunningTurn,
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
      <header className="page-header">{menuButton}<div className="page-header-main"><h1>Collaborations</h1><p>Agents on this device that leave each other messages. Nothing reaches an Agent mid-turn, and a message between Agents waits for your approval unless you allowed its recipient a few automatic turns.</p></div>
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
  const [granting, setGranting] = useState<CollaborationMember | null>(null);
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
            onResume={() => void act(() => resumeCollaborationMember(id, member.alias))}
            onGrant={() => setGranting(member)}
            onWithdraw={() => void act(() => withdrawCollaborationGrant(id, member.alias))} />)}
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
      {granting ? <GrantDialog member={granting} pending={pending} onClose={() => setGranting(null)}
        onGrant={(turns, hours) => { const alias = granting.alias; setGranting(null); void act(() => setCollaborationGrant(id, alias, turns, hours)); }} /> : null}
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

function timeOfDay(seconds: number): string {
  return new Date(seconds * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function MemberCard({ member, waiting, disabled, onApprove, onResume, onGrant, onWithdraw }: {
  member: CollaborationMember; waiting: string[]; disabled: boolean; onApprove: (ids: string[]) => void; onResume: () => void; onGrant: () => void; onWithdraw: () => void;
}) {
  const tone = member.paused ? "warn" : member.busy ? "live" : "neutral";
  const now = Date.now() / 1000;
  const automatic = grantUsable(member.grant, now);
  return <div className="card" style={{ flex: "1 1 260px", minWidth: 240 }}><div className="card-body stack" style={{ gap: 6 }}>
    <div className="row" style={{ gap: 8 }}><Dot tone={tone} pulse={member.busy && !member.paused} /><strong>{member.alias}</strong><span className="small muted">{providerLabel(member.provider)}</span></div>
    <span className="xsmall subtle mono" title={member.workspace}>{folderName(member.workspace)}</span>
    {member.role ? <span className="small">{member.role}</span> : null}
    <span className="small muted">{memberStateLabel(member)}</span>
    <span className="xsmall subtle">{policyLabel(member, now, timeOfDay)}</span>
    {member.live ? <LiveTurn live={member.live} automatic={member.autoTask === member.live.taskId} now={now} /> : null}
    <div className="row row-wrap" style={{ gap: 6 }}>
      {waiting.length > 0 ? <button type="button" className="btn btn-sm btn-primary" disabled={disabled} onClick={() => onApprove(waiting)}>Approve {waiting.length} for {member.alias}</button> : null}
      {member.paused ? <button type="button" className="btn btn-sm" disabled={disabled} onClick={onResume}>Resume {member.alias}</button> : null}
      {automatic
        ? <button type="button" className="btn btn-sm btn-ghost" disabled={disabled} onClick={onWithdraw}>Stop automatic turns</button>
        : <button type="button" className="btn btn-sm btn-ghost" disabled={disabled} onClick={onGrant}>Allow automatic turns…</button>}
    </div>
  </div></div>;
}

/** The live card: what the running turn's harness reported so far. No
 * model is asked anything for it. */
function LiveTurn({ live, automatic, now }: { live: RunningTurn; automatic: boolean; now: number }) {
  const activity = live.activity ?? null;
  const step = currentStep(activity);
  const counts = activityCounts(activity);
  return <div className="stack" style={{ gap: 4, borderLeft: "2px solid var(--border-strong)", paddingLeft: 8 }}>
    <span className="xsmall subtle">{automatic ? "Automatic turn" : "Turn"} running{activity ? ` · ${elapsedLabel(now - activity.since)}` : ""}</span>
    {step ? <span className="small" title="The Agent's own plan (its statement)"><span className="muted">Step: </span>{step}</span> : null}
    {activity && activity.recent.length > 0 ? <ul className="xsmall mono" style={{ margin: 0, paddingLeft: 16 }} title="Reported by the Agent's harness">
      {activity.recent.map((action, index) => <li key={index} style={action.state === "failed" ? { color: "var(--danger)" } : undefined}>{action.label}{action.state === "running" ? " …" : action.state === "failed" ? " (failed)" : ""}</li>)}
    </ul> : <span className="xsmall subtle">Nothing reported yet.</span>}
    {counts ? <span className="xsmall subtle" title="Reported by the Agent's harness">{counts}</span> : null}
    {activity && activity.files.length > 0 ? <span className="xsmall subtle mono" title={activity.files.join("\n")}>{activity.files.slice(0, 3).join(", ")}{activity.filesChanged > 3 ? ` +${activity.filesChanged - 3}` : ""}</span> : null}
  </div>;
}

function GrantDialog({ member, pending, onClose, onGrant }: { member: CollaborationMember; pending: boolean; onClose: () => void; onGrant: (turns: number, hours: number) => void }) {
  const [turns, setTurns] = useState(DEFAULT_AUTO_TURNS);
  const [hours, setHours] = useState(DEFAULT_AUTO_HOURS);
  return <Dialog title={`Allow ${member.alias} automatic turns?`} onClose={onClose}
    description={`A message from another member then starts ${member.alias}'s next turn without asking you, until its turns are used or the time runs out. Messages waiting for it now count too.`}
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending} onClick={() => onGrant(turns, hours)}>Allow {turns} turn{turns === 1 ? "" : "s"}</button></>}>
    <div className="stack-lg">
      <div className="row row-wrap" style={{ gap: 12 }}>
        <Field label="Turns" htmlFor="grant-turns"><select id="grant-turns" className="select select-sm" value={turns} onChange={(event) => setTurns(Number(event.target.value))}>
          {Array.from({ length: MAX_AUTO_TURNS }, (_, index) => index + 1).map((value) => <option key={value} value={value}>{value}</option>)}
        </select></Field>
        <Field label="Within" htmlFor="grant-hours"><select id="grant-hours" className="select select-sm" value={hours} onChange={(event) => setHours(Number(event.target.value))}>
          {Array.from({ length: MAX_AUTO_HOURS }, (_, index) => index + 1).map((value) => <option key={value} value={value}>{value} hour{value === 1 ? "" : "s"}</option>)}
        </select></Field>
      </div>
      <Notice tone="info">These turns use the {providerLabel(member.provider)} account on this device and run with the conversation's own permissions; a message grants nothing more. A turn stops on any approval {providerLabel(member.provider)} would ask for{member.provider === "claude_code" ? ", which a Claude Code member declines: it then asks you under Needs you" : ""}. Stop, Pause or a turn that fails pauses {member.alias}. At most 30 automatic turns start per hour on this device.</Notice>
    </div>
  </Dialog>;
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
    case "turn": {
      const counts = activityCounts(body.activity);
      const kind = body.automatic ? "automatic turn" : body.origin === "collaboration" ? "turn with messages" : "turn";
      return <div className="stack" style={{ gap: 2 }}>
        <span className="small"><strong>{entry.member}</strong> · {kind} {turnOutcomeLabel(body.state, body.code)}</span>
        {counts ? <span className="xsmall subtle" title={body.activity?.files?.join("\n")}>{counts} · reported by the Agent's harness</span> : null}
        {meta}
      </div>;
    }
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
    case "grant":
      return <div className="stack" style={{ gap: 2 }}><span className="small">{body.change === "granted"
        ? <>You allowed <strong>{body.alias}</strong> {body.turns} automatic turn{body.turns === 1 ? "" : "s"} until {timeOfDay(body.expiresAt)}</>
        : <><strong>{body.alias}</strong> asks you again: {grantEndLabel(body.reason)}</>}</span>{meta}</div>;
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
