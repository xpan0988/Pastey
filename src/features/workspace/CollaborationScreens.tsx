import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { listRemoteSharedWorkspaces, listRooms } from "../../lib/tauri";
import type { RoomInfo } from "../../lib/types";
import type { SharedWorkspace } from "../../lib/tauri";
import {
  answerCollaborationInvitation, inviteCollaborationMember, listCollaborationInvitations, originLabel, outboxLabel,
  type CollaborationInvitation, type CollaborationLink,
  activityCounts, addCollaborationMember, approveCollaborationMessages, decideCollaborationSummary, fileChangesLabel, fileStateLabel,
  removeCollaborationMember, setCollaborationGoal, setCollaborationRole, summaryDecider, summaryDecision, basisLabel, createCollaboration, currentStep, DEFAULT_AUTO_HOURS, DEFAULT_AUTO_TURNS,
  deleteCollaboration, dismissCollaborationEntry, elapsedLabel, folderName, getCollaboration, grantEndLabel, grantUsable, journalCount,
  journalEntries, listCollaborations, MAX_AUTO_HOURS, MAX_AUTO_TURNS, memberStateLabel, policyLabel, providerLabel,
  resumeCollaborationMember, sendCollaborationMessage, setCollaborationGrant, setCollaborationState, statusLabel, suggestAlias,
  turnOutcomeLabel, USER_ALIAS, validAlias, waitingByRecipient, withdrawCollaborationGrant, type CollaborationEntry,
  optionsLabel, pendingDecisions, setCollaborationMemberOptions, stopCollaborationMember, type MemberOptions,
  executionSourceLabel, setCollaborationExecution, type ExecutionChoice,
  checkCollaborationReadiness, releaseCollaboration, type MemberReadiness,
  type CollaborationMember, type CollaborationMemberInput, type CollaborationProvider, type CollaborationSummary, type CollaborationView,
  type JournalFilter, type RunningTurn,
} from "../../lib/collaboration";
import { formatTimestamp } from "../../lib/format";
import { Badge, Dialog, Dot, EmptyState, errorText, Field, Icon, ListRow, MenuButton, Notice, Segmented } from "./ui";
import { NativeDecisionPanel } from "./NativeDecisionPanel";
import { ExecutionChoiceDialog } from "./ExecutionProfiles";

/** Device-wide: Agent collaborations of this device and their Journals. */
export function CollaborationsScreen({ menuButton }: { menuButton?: ReactNode }) {
  const [list, setList] = useState<CollaborationSummary[]>([]);
  const [invitations, setInvitations] = useState<CollaborationInvitation[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [answering, setAnswering] = useState(false);
  const refresh = useCallback(async () => {
    try {
      const [collaborations, waiting] = await Promise.all([listCollaborations(), listCollaborationInvitations()]);
      setList(collaborations); setInvitations(waiting); setError(null);
    } catch (cause) { setError(errorText(cause, "Collaborations could not be read.")); }
  }, []);
  const answer = async (invitation: CollaborationInvitation, accept: boolean) => {
    setAnswering(true);
    try {
      await answerCollaborationInvitation(invitation.collaborationId, invitation.member.alias, accept);
      await refresh();
      if (accept) setSelected(invitation.collaborationId);
    } catch (cause) { setError(errorText(cause, "That did not work.")); } finally { setAnswering(false); }
  };
  useEffect(() => {
    if (selected) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [refresh, selected]);
  if (selected) return <CollaborationDetail id={selected} menuButton={menuButton} onBack={() => setSelected(null)} />;
  return (
    <section className="page">
      <header className="page-header">{menuButton}<div className="page-header-main"><h1>Collaborations</h1><p>Agents that leave each other messages, on this device and one paired with it. Nothing reaches an Agent mid-turn, and a message for an Agent waits for the approval of its device's user unless they allowed it a few automatic turns.</p></div>
        <div className="page-header-actions"><button type="button" className="btn btn-sm btn-primary" onClick={() => setCreating(true)}><Icon name="plus" small />New collaboration</button></div></header>
      <div className="page-body"><div className="content stack-lg">
        {error ? <Notice tone="bad">{error}</Notice> : null}
        {invitations.map((invitation) => <InvitationCard key={`${invitation.collaborationId}/${invitation.member.alias}`} invitation={invitation} pending={answering}
          onAnswer={(accept) => void answer(invitation, accept)} />)}
        {list.length === 0 ? <EmptyState title="No collaborations yet" actions={<button type="button" className="btn btn-sm" onClick={() => setCreating(true)}><Icon name="plus" small />New collaboration</button>}>
          Start with a Codex or Claude Code conversation of this device, then add others here or invite one from a device paired with this one. Each Agent can then leave the others a message when its turn ends.
        </EmptyState> : <div className="list">
          {list.map((item) => <ListRow key={item.id} title={item.name}
            meta={<span>{item.members.map((member) => `${member.alias} (${providerLabel(member.provider)})`).join(" · ")}{item.link ? ` · with ${item.link.deviceLabel}` : ""}</span>}
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
  const [choosing, setChoosing] = useState<CollaborationMember | null>(null);
  const [executing, setExecuting] = useState<{ alias: string | null; initial: ExecutionChoice } | null>(null);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<{ kind: "goal" } | { kind: "role"; member: CollaborationMember } | null>(null);
  const [removing, setRemoving] = useState<CollaborationMember | null>(null);
  const [inviting, setInviting] = useState(false);
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
  const link = view.link;
  /** A copy of a collaboration another device created: its goal and members are edited there. */
  const guest = !!link && !link.home;
  const waiting = waitingByRecipient(view.entries);
  const shown = journalEntries(view.entries, filter);
  const filters: ReadonlyArray<{ value: JournalFilter; label: string }> = (["activity", "summaries", "needs-you"] as const).map((value) => ({
    value, label: `${value === "activity" ? "Activity" : value === "summaries" ? "Summaries" : "Needs you"} ${journalCount(view.entries, value) + (value === "needs-you" && view.state !== "ended" ? view.members.reduce((sum, member) => sum + pendingDecisions(member), 0) : 0)}`,
  }));
  return (
    <section className="page">
      <header className="page-header">{menuButton}{back}<div className="page-header-main"><h1>{view.name}</h1><p>{view.goal || "No goal given."}</p></div>
        <div className="page-header-actions">
          {view.state === "active" ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void act(() => setCollaborationState(id, "paused"))}>Pause</button> : null}
          {view.state === "paused" ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => void act(() => setCollaborationState(id, "active"))}>Resume</button> : null}
          {!ended && !guest && view.members.length < 6 ? <button type="button" className="btn btn-sm" disabled={pending} onClick={() => setAdding(true)}><Icon name="plus" small />Add member</button> : null}
          <MenuButton label="More collaboration actions">{(close) => <>
            {!ended && !guest && view.members.length < 6 ? <button type="button" role="menuitem" onClick={() => { close(); setInviting(true); }}><Icon name="device" /><span>Invite from another device…<small>A conversation on a device paired with this one</small></span></button> : null}
            {!ended && !guest ? <button type="button" role="menuitem" onClick={() => { close(); setEditing({ kind: "goal" }); }}><Icon name="file" /><span>Edit goal…<small>Members see it from their next turn</small></span></button> : null}
            {!ended ? <button type="button" role="menuitem" onClick={() => { close(); setExecuting({ alias: null, initial: toChoice(view.execution) }); }}><Icon name="settings" /><span>Execution…<small>What its members may do without asking you</small></span></button> : null}
            {!ended ? <button type="button" role="menuitem" onClick={() => { close(); setConfirming("end"); }}><Icon name="close" /><span>{guest ? "Leave collaboration" : "End collaboration"}<small>{guest ? `This device's members leave; ${link?.deviceLabel} is told` : "Members become ordinary conversations; the Journal stays"}</small></span></button> : null}
            <button type="button" role="menuitem" onClick={() => { close(); setConfirming("delete"); }}><Icon name="flame" /><span>Delete…<small>Removes the Journal; conversations stay</small></span></button>
          </>}</MenuButton>
        </div></header>
      <div className="page-body"><div className="content stack-lg">
        {error ? <Notice tone="bad">{error}</Notice> : null}
        {view.state === "paused" ? <Notice tone="warn" title="Paused">No member turn starts. Messages still arrive and wait.</Notice> : null}
        {!ended && view.released === false ? <ReadinessPanel id={id} disabled={pending} onReleased={setView} /> : null}
        {ended ? <Notice title="Ended">Its members are ordinary conversations again. The Journal stays until you delete it.</Notice> : null}
        {link ? <LinkNotice link={link} ended={ended} /> : null}
        <div className="row row-wrap" style={{ gap: 12, alignItems: "stretch" }}>
          {view.members.map((member) => <MemberCard key={member.alias} member={member} link={link} guest={guest} waiting={waiting[member.alias] ?? []} disabled={pending || ended}
            onApprove={(ids) => void act(() => approveCollaborationMessages(id, ids))}
            onResume={() => void act(() => resumeCollaborationMember(id, member.alias))}
            onGrant={() => setGranting(member)}
            onWithdraw={() => void act(() => withdrawCollaborationGrant(id, member.alias))}
            onEditRole={() => setEditing({ kind: "role", member })}
            onStop={() => void act(() => stopCollaborationMember(id, member.alias))}
            onOptions={() => setChoosing(member)}
            onExecution={() => setExecuting({ alias: member.alias, initial: toChoice(member.execution) })}
            onRemove={view.members.length > 2 ? () => setRemoving(member) : undefined} />)}
        </div>
        {!ended ? <Composer members={view.members} link={link} disabled={pending} onSend={(to, text) => act(() => sendCollaborationMessage(id, to, text))} /> : null}
        <Segmented label="Show" value={filter} onChange={setFilter} options={filters} />
        {filter === "needs-you" && !ended && view.members.some((member) => pendingDecisions(member) > 0)
          ? <Notice tone="warn" title="An Agent waits for your approval">{view.members.filter((member) => pendingDecisions(member) > 0).map((member) => member.alias).join(", ")}: answer on the member's card above, or stop the turn.</Notice>
          : null}
        {shown.length === 0 ? <EmptyState title={filter === "needs-you" ? "Nothing needs you" : "Nothing yet"}>
          {filter === "summaries" ? "An Agent writes a summary when its turn finishes a stage, makes a decision or gets blocked." : "Turns, messages and summaries appear here as they happen."}
        </EmptyState> : <div className="stack">
          {shown.map((entry) => <EntryCard key={entry.id} entry={entry} entries={view.entries} members={view.members} link={link} disabled={pending || ended}
            onApprove={() => void act(() => approveCollaborationMessages(id, [entry.id]))}
            onDismiss={() => void act(() => dismissCollaborationEntry(id, entry.id))}
            onDecide={(confirmed) => void act(() => decideCollaborationSummary(id, entry.id, confirmed))} />)}
        </div>}
      </div></div>
      {inviting ? <InviteMemberDialog view={view} onClose={() => setInviting(false)}
        onInvite={async (roomId, member, firstMessage) => { const next = await inviteCollaborationMember(id, roomId, member, firstMessage); setView(next); setInviting(false); }} /> : null}
      {adding ? <AddMemberDialog view={view} onClose={() => setAdding(false)}
        onAdd={async (member, firstMessage) => { const next = await addCollaborationMember(id, member, firstMessage); setView(next); setAdding(false); }} /> : null}
      {editing ? <TextEditDialog
        title={editing.kind === "goal" ? "Edit the goal" : `Edit ${editing.member.alias}'s role`}
        description={editing.kind === "goal" ? "One line every member sees, from its next turn on. The change is recorded in the Journal." : `What ${editing.member.alias} owns and does not touch. Every member sees it from its next turn on.`}
        initial={editing.kind === "goal" ? view.goal : editing.member.role} maxLength={editing.kind === "goal" ? 300 : 200} pending={pending}
        onClose={() => setEditing(null)}
        onSave={(text) => { const which = editing; setEditing(null); void act(() => which.kind === "goal" ? setCollaborationGoal(id, text) : setCollaborationRole(id, which.member.alias, text)); }} /> : null}
      {removing ? <Dialog title={`Remove ${removing.alias}?`} onClose={() => setRemoving(null)}
        description={`${removing.alias} stops receiving messages. Its automatic turns end, and messages waiting for it are not delivered. Its ${providerLabel(removing.provider)} conversation is not changed, and what it wrote stays in the Journal.`}
        footer={<><button type="button" className="btn btn-sm" onClick={() => setRemoving(null)}>Cancel</button>
          <button type="button" className="btn btn-sm btn-danger" disabled={pending} onClick={() => { const alias = removing.alias; setRemoving(null); void act(() => removeCollaborationMember(id, alias)); }}>Remove</button></>}>
        <span />
      </Dialog> : null}
      {executing ? <ExecutionChoiceDialog
        title={executing.alias ? `Execution for ${executing.alias}` : `Execution for ${view.name}`}
        scope={executing.alias ? "member" : "collaboration"} initial={executing.initial} pending={pending}
        onClose={() => setExecuting(null)}
        onSave={(choice) => { const alias = executing.alias; setExecuting(null); void act(() => setCollaborationExecution(id, alias, choice)); }} /> : null}
      {choosing ? <OptionsDialog member={choosing} pending={pending} onClose={() => setChoosing(null)}
        onSave={(options) => { const alias = choosing.alias; setChoosing(null); void act(() => setCollaborationMemberOptions(id, alias, options.model ?? null, options.effort ?? null)); }} /> : null}
      {granting ? <GrantDialog member={granting} pending={pending} onClose={() => setGranting(null)}
        onGrant={(turns, hours) => { const alias = granting.alias; setGranting(null); void act(() => setCollaborationGrant(id, alias, turns, hours)); }} /> : null}
      {confirming ? <Dialog title={confirming === "end" ? `${guest ? "Leave" : "End"} ${view.name}?` : `Delete ${view.name}?`} onClose={() => setConfirming(null)}
        description={confirming === "end"
          ? guest
            ? `This device's members leave and ${link?.deviceLabel} is told when it is reachable. Waiting messages are dismissed; the Journal stays here until you delete it.`
            : `Waiting messages are dismissed and no member turn starts. Its members become ordinary conversations again; the Journal stays.${link ? ` ${link.deviceLabel} is told, and its copy ends too.` : ""}`
          : "The collaboration and its Journal are removed from this device. The members' conversations are not changed."}
        footer={<><button type="button" className="btn btn-sm" onClick={() => setConfirming(null)}>Cancel</button>
          <button type="button" className="btn btn-sm btn-danger" disabled={pending} onClick={() => {
            const which = confirming; setConfirming(null);
            void act(async () => {
              if (which === "end") return setCollaborationState(id, "ended");
              await deleteCollaboration(id); onBack();
            });
          }}>{confirming === "end" ? (guest ? "Leave" : "End") : "Delete"}</button></>}>
        <span />
      </Dialog> : null}
    </section>
  );
}

const BASIS_WORD: Record<string, string> = { configured: "configured", probed: "checked now", measured: "seen in a turn", unknown: "not known" };

/** Preparing: no member turn starts until every member is ready. Checks
 * need no model call; facts say how each is known. */
function ReadinessPanel({ id, disabled, onReleased }: { id: string; disabled: boolean; onReleased: (view: CollaborationView) => void }) {
  const [results, setResults] = useState<MemberReadiness[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const check = async () => {
    setChecking(true); setError(null);
    try { setResults(await checkCollaborationReadiness(id)); } catch (cause) { setError(errorText(cause, "The members could not be checked.")); } finally { setChecking(false); }
  };
  useEffect(() => { void check(); }, [id]);
  const allReady = results !== null && results.length > 0 && results.every((result) => result.ready);
  return <div className="card"><div className="card-body stack" style={{ gap: 10 }}>
    <div className="row row-wrap" style={{ gap: 8, alignItems: "center" }}>
      <strong>Preparing</strong>
      <span className="small muted" style={{ flex: 1 }}>No member turn starts until every member is ready. Messages you send now wait, and all members start together.</span>
      <button type="button" className="btn btn-sm" disabled={checking} onClick={() => void check()}>{checking ? "Checking…" : "Check again"}</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={disabled || checking || !allReady}
        onClick={() => void releaseCollaboration(id).then(onReleased).catch((cause) => setError(errorText(cause, "It could not start.")))}>Start collaboration</button>
    </div>
    {error ? <Notice tone="bad">{error}</Notice> : null}
    {results === null ? <span className="small subtle">Checking each member…</span> : results.map((result) => <div key={result.alias} className="stack" style={{ gap: 4 }}>
      <span className="small"><strong>{result.alias}</strong> · {providerLabel(result.provider)} · {result.ready ? "ready" : "not ready"}</span>
      <ul className="xsmall" style={{ margin: 0, paddingLeft: 16 }}>
        {result.checks.map((item) => <li key={item.name} style={item.ok === true ? undefined : { color: "var(--danger)" }}>
          {item.ok === true ? "✓" : item.ok === false ? "✗" : "?"} {item.name}: {item.detail} <span className="subtle">({BASIS_WORD[item.basis]})</span>
        </li>)}
      </ul>
      {result.facts.length > 0 ? <details className="xsmall"><summary>What the other members will be told it can do</summary>
        <ul style={{ margin: 0, paddingLeft: 16 }}>{result.facts.map((fact, index) => <li key={index}>{fact.text} <span className="subtle">({BASIS_WORD[fact.basis]})</span></li>)}</ul>
      </details> : null}
    </div>)}
  </div></div>;
}

function toChoice(stored: CollaborationMember["execution"]): ExecutionChoice {
  if (!stored) return { kind: "inherit" };
  if (stored.kind === "ask") return { kind: "ask" };
  return { kind: "profile", profile: stored.profile, confirmed: false };
}

function timeOfDay(seconds: number): string {
  return new Date(seconds * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function MemberCard({ member, link, guest, waiting, disabled, onApprove, onResume, onGrant, onWithdraw, onEditRole, onStop, onOptions, onExecution, onRemove }: {
  member: CollaborationMember; link?: CollaborationLink; guest: boolean; waiting: string[]; disabled: boolean; onApprove: (ids: string[]) => void; onResume: () => void; onGrant: () => void; onWithdraw: () => void;
  onEditRole: () => void; onStop: () => void; onOptions: () => void; onExecution: () => void; onRemove?: () => void;
}) {
  if (member.remote) {
    // Runs on the linked device: its user there approves its turns and
    // grants it automatic turns.
    const editable = !disabled && !guest;
    return <div className="card" style={{ flex: "1 1 260px", minWidth: 240 }}><div className="card-body stack" style={{ gap: 6 }}>
      <div className="row" style={{ gap: 8 }}><Dot tone="neutral" /><strong>{member.alias}</strong><span className="small muted">{providerLabel(member.provider)}</span>
        {editable ? <span style={{ marginLeft: "auto" }}><MenuButton label={`More actions for ${member.alias}`}>{(close) => <>
          <button type="button" role="menuitem" onClick={() => { close(); onEditRole(); }}><Icon name="file" /><span>Edit role…</span></button>
          {onRemove ? <button type="button" role="menuitem" onClick={() => { close(); onRemove(); }}><Icon name="close" /><span>{member.invite ? "Withdraw invitation…" : "Remove from collaboration…"}</span></button> : null}
        </>}</MenuButton></span> : null}</div>
      <span className="xsmall subtle"><Icon name="device" small /> on {link?.deviceLabel ?? "the other device"}</span>
      {member.role ? <span className="small">{member.role}</span> : null}
      <span className="small muted">{memberStateLabel(member)}</span>
      <span className="xsmall subtle">Its turns and automatic turns are allowed on that device.</span>
    </div></div>;
  }
  const decisions = pendingDecisions(member);
  const tone = decisions > 0 || member.paused ? "warn" : member.busy ? "live" : "neutral";
  const now = Date.now() / 1000;
  const automatic = grantUsable(member.grant, now);
  return <div className="card" style={{ flex: "1 1 260px", minWidth: 240 }}><div className="card-body stack" style={{ gap: 6 }}>
    <div className="row" style={{ gap: 8 }}><Dot tone={tone} pulse={member.busy && !member.paused} /><strong>{member.alias}</strong><span className="small muted">{providerLabel(member.provider)}</span>
      {!disabled && !guest ? <span style={{ marginLeft: "auto" }}><MenuButton label={`More actions for ${member.alias}`}>{(close) => <>
        <button type="button" role="menuitem" onClick={() => { close(); onEditRole(); }}><Icon name="file" /><span>Edit role…</span></button>
        <button type="button" role="menuitem" onClick={() => { close(); onOptions(); }}><Icon name="settings" /><span>Model and effort…<small>For every turn the collaboration starts</small></span></button>
        <button type="button" role="menuitem" onClick={() => { close(); onExecution(); }}><Icon name="settings" /><span>Execution…<small>What it may do without asking you</small></span></button>
        {onRemove ? <button type="button" role="menuitem" onClick={() => { close(); onRemove(); }}><Icon name="close" /><span>Remove from collaboration…<small>Its conversation stays</small></span></button> : null}
      </>}</MenuButton></span> : null}</div>
    <span className="xsmall subtle mono" title={member.workspace}>{folderName(member.workspace)}</span>
    {member.role ? <span className="small">{member.role}</span> : null}
    <span className="small muted">{memberStateLabel(member)}</span>
    <span className="xsmall subtle">{policyLabel(member, now, timeOfDay)}</span>
    <span className="xsmall subtle">{optionsLabel(member.options)}</span>
    {member.effectiveExecution ? <span className="xsmall subtle" title={member.effectiveExecution.lines.join("\n")}>
      Execution: {member.effectiveExecution.name} · {executionSourceLabel(member.effectiveExecution.source)} · configured, not verified
    </span> : null}
    {member.live ? <LiveTurn live={member.live} automatic={member.autoTask === member.live.taskId} now={now} /> : null}
    {member.live && (member.live.decisions ?? []).length > 0
      ? <NativeDecisionPanel status={{ taskId: member.live.taskId, state: "running", nativeDecisions: member.live.decisions }} remote={false} hostName="this device" />
      : null}
    {member.live?.awaitingDecision && (member.live.decisions ?? []).length === 0
      ? <Notice tone="warn">{providerLabel(member.provider)} waits for an answer Pastey cannot show here. Stop the turn, or answer it in {providerLabel(member.provider)} itself.</Notice>
      : null}
    <div className="row row-wrap" style={{ gap: 6 }}>
      {waiting.length > 0 ? <button type="button" className="btn btn-sm btn-primary" disabled={disabled} onClick={() => onApprove(waiting)}>Approve {waiting.length} for {member.alias}</button> : null}
      {member.paused ? <button type="button" className="btn btn-sm" disabled={disabled} onClick={onResume}>Resume {member.alias}</button> : null}
      {member.live && !guest ? <button type="button" className="btn btn-sm btn-danger" disabled={disabled} onClick={onStop} title="Interrupts the running turn. It does not undo what the Agent already did.">Stop {member.alias}</button> : null}
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

function OptionsDialog({ member, pending, onClose, onSave }: { member: CollaborationMember; pending: boolean; onClose: () => void; onSave: (options: MemberOptions) => void }) {
  const [model, setModel] = useState(member.options?.model ?? "");
  const [effort, setEffort] = useState(member.options?.effort ?? "");
  const efforts = member.provider === "claude_code" ? ["low", "medium", "high", "xhigh", "max"] : ["minimal", "low", "medium", "high", "xhigh"];
  return <Dialog title={`Model and effort for ${member.alias}`} onClose={onClose}
    description={`Every turn the collaboration starts for ${member.alias} runs with these. Empty uses ${providerLabel(member.provider)}'s own default on this device. ${providerLabel(member.provider)} checks the names; an unknown one fails that turn.`}
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending} onClick={() => onSave({ model: model.trim() || undefined, effort: effort || undefined })}>Save</button></>}>
    <div className="row row-wrap" style={{ gap: 12 }}>
      <Field label="Model" htmlFor="member-model"><input id="member-model" className="input input-sm" value={model} placeholder="Agent default" maxLength={64} onChange={(event) => setModel(event.target.value)} /></Field>
      <Field label="Effort" htmlFor="member-effort"><select id="member-effort" className="select select-sm" value={effort} onChange={(event) => setEffort(event.target.value)}>
        <option value="">Agent default</option>
        {efforts.map((value) => <option key={value} value={value}>{value}</option>)}
      </select></Field>
    </div>
  </Dialog>;
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

function Composer({ members, link, disabled, onSend }: { members: CollaborationMember[]; link?: CollaborationLink; disabled: boolean; onSend: (to: string, text: string) => Promise<void> }) {
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
      <span className="xsmall subtle">{members.find((member) => member.alias === to)?.remote
        ? `Waits on ${link?.deviceLabel ?? "the other device"} until its user (or its automatic turns there) lets it run.`
        : "Delivered when its current turn ends. Your messages need no approval."}</span>
    </div>
    <textarea className="textarea" rows={3} maxLength={4096} value={text} disabled={disabled} placeholder={`Write to ${to || "a member"}…`}
      onChange={(event) => setText(event.target.value)}
      onKeyDown={(event) => { if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) void send(); }} />
    <div className="row" style={{ justifyContent: "flex-end" }}><button type="button" className="btn btn-sm btn-primary" disabled={disabled || !text.trim() || !to} onClick={() => void send()}><Icon name="arrowUp" small />Send</button></div>
  </div></div>;
}

function EntryCard({ entry, entries, members, link, disabled, onApprove, onDismiss, onDecide }: {
  entry: CollaborationEntry; entries: CollaborationEntry[]; members: CollaborationMember[]; link?: CollaborationLink; disabled: boolean; onApprove: () => void; onDismiss: () => void; onDecide: (confirmed: boolean) => void;
}) {
  const status = statusLabel(entry.status);
  const unsent = outboxLabel(entry, link);
  const provider = (alias?: string) => members.find((member) => member.alias === alias)?.provider;
  const meta = <span className="xsmall subtle">{formatTimestamp(entry.at)} · {originLabel(entry, link)}{status ? ` · ${status}` : ""}{unsent ? ` · ${unsent}` : ""}</span>;
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
        {body.files ? <span className="xsmall subtle mono" title="From Pastey's scans of the folder at the turn's start and end">{fileStateLabel(body.files)}</span> : null}
        {meta}
      </div>;
    }
    case "message": {
      const fromProvider = provider(body.from);
      return <div className="card"><div className="card-body stack" style={{ gap: 6 }}>
        <span className="small"><strong>{body.from === USER_ALIAS ? (entry.host ? `The user on ${link?.deviceLabel ?? "the other device"}` : "You") : body.from}</strong>{fromProvider ? ` (${providerLabel(fromProvider)})` : ""} → <strong>{body.to === USER_ALIAS ? "you" : body.to}</strong>{body.subject ? `: ${body.subject}` : ""}</span>
        <div className="small" style={{ whiteSpace: "pre-wrap" }}>{body.body}</div>
        {meta}{actions}
      </div></div>;
    }
    case "summary": {
      const decision = body.summary.decisions ? summaryDecision(entries, entry.id) : null;
      const decider = decision !== null && summaryDecider(entries, entry.id) ? `The user on ${link?.deviceLabel ?? "the other device"}` : "You";
      return <div className="card"><div className="card-body stack" style={{ gap: 4 }}>
        <span className="small"><strong>{entry.member}</strong> · stage summary</span>
        {(["done", "decisions", "open", "next"] as const).map((field) => body.summary[field] ? <div key={field} className="small" style={{ whiteSpace: "pre-wrap" }}><span className="muted">{field}: </span>{body.summary[field]}</div> : null)}
        {meta}
        {body.summary.decisions ? <div className="row row-wrap" style={{ gap: 6, alignItems: "center" }}>
          {decision !== null ? <Badge tone={decision ? "ok" : "warn"}>{`${decider} ${decision ? "confirmed" : "rejected"} these decisions`}</Badge> : <span className="xsmall subtle">Members joining later see whether you confirmed these decisions.</span>}
          {decision !== true ? <button type="button" className="btn btn-sm" disabled={disabled} onClick={() => onDecide(true)}>Confirm decisions</button> : null}
          {decision !== false ? <button type="button" className="btn btn-sm btn-ghost" disabled={disabled} onClick={() => onDecide(false)}>Reject</button> : null}
        </div> : null}
      </div></div>;
    }
    case "attention":
      return <Notice tone={entry.status?.state === "open" ? "warn" : "neutral"} action={actions}><span className="stack" style={{ gap: 2 }}><span>{body.text}</span>{meta}</span></Notice>;
    case "user": {
      const action = body.action;
      const decided = action.type === "decided" ? entries.find((other) => other.id === action.entry)?.member : undefined;
      const text = action.type === "created" ? "You created this collaboration"
        : action.type === "paused" ? "You paused it"
        : action.type === "resumed" ? "You resumed it"
        : action.type === "ended" ? "You ended it"
        : action.type === "member_resumed" ? `You resumed ${action.alias}`
        : action.type === "goal_changed" ? `You changed the goal: ${action.goal || "(none)"}`
        : action.type === "role_changed" ? `You changed ${action.alias}'s role: ${action.role || "(none)"}`
        : action.type === "accepted" ? `You accepted the invitation for ${action.alias}`
        : action.type === "released" ? "You started it: every member was ready"
        : action.type === "execution_changed" ? `You set ${action.alias ? `${action.alias}'s` : "the collaboration's"} execution to ${action.name === "inherit" ? (action.alias ? "the collaboration's choice" : "your default") : action.name}`
        : action.type === "options_changed" ? `You set ${action.alias}'s model to ${action.model ?? "its default"} and effort to ${action.effort ?? "its default"}`
        : `You ${action.confirmed ? "confirmed" : "rejected"} ${decided ?? "a member"}'s decisions`;
      const who = entry.host ? text.replace(/^You /, `The user on ${link?.deviceLabel ?? "the other device"} `) : text;
      return <div className="stack" style={{ gap: 2 }}><span className="small">{who}</span>{meta}</div>;
    }
    case "member":
      return <div className="stack" style={{ gap: 2 }}><span className="small"><strong>{body.alias}</strong> {body.change === "left" ? "left" : body.change === "invited" ? (entry.host ? `was invited from ${link?.deviceLabel ?? "the other device"}` : `was invited on ${link?.deviceLabel ?? "the other device"}`) : body.change === "declined" ? "was declined by its device's user" : "joined"} ({providerLabel(body.provider)}){body.role && body.change !== "left" ? ` · ${body.role}` : ""}</span>{meta}</div>;
    case "files_outside":
      return <div className="stack" style={{ gap: 2 }}><span className="small"><strong>{body.alias}</strong>'s folder changed between its turns</span>
        <span className="xsmall subtle mono" title="From Pastey's scans: by you, another app, or another client of its conversation">{fileChangesLabel(body)}</span>{meta}</div>;
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
    description="Each member is Pastey's Codex or Claude Code conversation in a folder of this device; once created, you can also invite one from a device paired with this one. Every turn they run carries a short note about the collaboration."
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !!problem} onClick={() => void create()}>Create</button></>}>
    <div className="stack-lg">
      <Field label="Name" htmlFor="collaboration-name"><input id="collaboration-name" className="input" maxLength={80} value={name} onChange={(event) => setName(event.target.value)} placeholder="orders-v2" /></Field>
      <Field label="Goal" hint="One line every member sees." htmlFor="collaboration-goal"><input id="collaboration-goal" className="input" maxLength={300} value={goal} onChange={(event) => setGoal(event.target.value)} placeholder="Move GET /v2/orders to cursor pagination" /></Field>
      {members.map((member, index) => <div key={member.key} className="card"><div className="card-body stack" style={{ gap: 8 }}>
        <div className="row" style={{ gap: 8, justifyContent: "space-between" }}><strong className="small">Member {index + 1}</strong>
          {members.length > 1 ? <button type="button" className="btn btn-ghost btn-sm" onClick={() => setMembers((current) => current.filter((other) => other.key !== member.key))}>Remove</button> : null}</div>
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

function TextEditDialog({ title, description, initial, maxLength, pending, onClose, onSave }: {
  title: string; description: string; initial: string; maxLength: number; pending: boolean; onClose: () => void; onSave: (text: string) => void;
}) {
  const [text, setText] = useState(initial);
  const changed = text.trim() !== initial.trim();
  return <Dialog title={title} description={description} onClose={onClose}
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !changed} onClick={() => onSave(text.trim())}>Save</button></>}>
    <input className="input" aria-label={title} maxLength={maxLength} value={text} autoFocus onChange={(event) => setText(event.target.value)}
      onKeyDown={(event) => { if (event.key === "Enter" && changed) onSave(text.trim()); }} />
  </Dialog>;
}

function AddMemberDialog({ view, onClose, onAdd }: { view: CollaborationView; onClose: () => void; onAdd: (member: CollaborationMemberInput, firstMessage: string) => Promise<void> }) {
  const [member, setMember] = useState<CollaborationMemberInput>({ alias: "", provider: "codex", workspace: "", role: "" });
  const [firstMessage, setFirstMessage] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const taken = view.members.map((other) => other.alias);
  const choose = async () => {
    const chosen = await open({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setMember((current) => ({ ...current, workspace: chosen, alias: current.alias || suggestAlias(chosen, taken) }));
  };
  const problem = !member.workspace ? "Choose its folder."
    : !validAlias(member.alias) ? `Aliases use lowercase letters, digits, - and _ (and not "${USER_ALIAS}").`
    : taken.includes(member.alias) ? `${member.alias} is already a member.` : null;
  const add = async () => {
    setPending(true);
    try { await onAdd(member, firstMessage); } catch (cause) { setError(errorText(cause, "The member could not be added.")); } finally { setPending(false); }
  };
  return <Dialog wide title={`Add a member to ${view.name}`} onClose={onClose}
    description="Pastey's Codex or Claude Code conversation in a folder of this device. Its first turn starts with a briefing Pastey compiles from the Journal: open items, each member's latest summary, the decisions and whether you confirmed them, recent turns and changed files. No model writes it."
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !!problem} onClick={() => void add()}>{firstMessage.trim() ? "Add and start" : "Add"}</button></>}>
    <div className="stack-lg">
      <div className="row row-wrap" style={{ gap: 12, alignItems: "flex-end" }}>
        <Field label="Agent" htmlFor="add-agent"><select id="add-agent" className="select select-sm" value={member.provider} onChange={(event) => setMember({ ...member, provider: event.target.value as CollaborationProvider })}>
          <option value="codex">Codex</option><option value="claude_code">Claude Code</option>
        </select></Field>
        <Field label="Folder" htmlFor="add-folder"><button id="add-folder" type="button" className="btn btn-sm" onClick={() => void choose()}><Icon name="folder" small />{member.workspace ? folderName(member.workspace) : "Choose folder…"}</button></Field>
        <Field label="Alias" htmlFor="add-alias"><input id="add-alias" className="input input-sm" maxLength={32} value={member.alias} placeholder="alias" onChange={(event) => setMember({ ...member, alias: event.target.value.toLowerCase() })} /></Field>
      </div>
      <Field label="Role" htmlFor="add-role"><input id="add-role" className="input input-sm" maxLength={200} value={member.role} placeholder="e.g. owns docs/; does not edit api/" onChange={(event) => setMember({ ...member, role: event.target.value })} /></Field>
      <Field label="First message (optional)" hint="Starts its first turn now, after the briefing. Leave it empty and the briefing goes with whatever reaches it first." htmlFor="first-message">
        <textarea id="first-message" className="textarea" rows={3} maxLength={4096} value={firstMessage} onChange={(event) => setFirstMessage(event.target.value)}
          placeholder="Read the briefing, then tell me what is open and who owns what." />
      </Field>
      {problem ? <span className="small muted">{problem}</span> : null}
      {error ? <Notice tone="bad">{error}</Notice> : null}
    </div>
  </Dialog>;
}

function LinkNotice({ link, ended }: { link: CollaborationLink; ended: boolean }) {
  return <Notice tone={link.unsent > 0 ? "warn" : "info"} title={link.home ? `Spans ${link.deviceLabel}` : `Created on ${link.deviceLabel}`}>
    {link.home
      ? `Members on ${link.deviceLabel} run there: its user approves their turns and allows automatic turns. Each device sends the other what it writes.`
      : `This device runs its own members here, with your approval or your automatic-turn allowance. The goal and members are edited on ${link.deviceLabel}.`}
    {link.unsent > 0 && !ended ? ` ${link.unsent} entr${link.unsent === 1 ? "y waits" : "ies wait"} for ${link.deviceLabel}; Pastey sends ${link.unsent === 1 ? "it" : "them"} when it is reachable.` : ""}
  </Notice>;
}

function InvitationCard({ invitation, pending, onAnswer }: { invitation: CollaborationInvitation; pending: boolean; onAnswer: (accept: boolean) => void }) {
  const others = invitation.members.map((member) => `${member.alias} (${providerLabel(member.provider)})${member.role ? `: ${member.role}` : ""}`);
  return <div className="card"><div className="card-body stack" style={{ gap: 8 }}>
    <span className="small"><strong>{invitation.deviceLabel}</strong> <span className="mono xsmall subtle">{invitation.displayFingerprint}</span> invites your {providerLabel(invitation.member.provider)} conversation in <strong>{folderName(invitation.workspace)}</strong> to <strong>{invitation.name}</strong> as <strong>{invitation.member.alias}</strong>{invitation.member.role ? ` (${invitation.member.role})` : ""}.</span>
    {invitation.goal ? <span className="small muted">Goal: {invitation.goal}</span> : null}
    {others.length > 0 ? <span className="xsmall subtle">Members there: {others.join(" · ")}</span> : null}
    <Notice tone="info">Its turns run here, on this device's {providerLabel(invitation.member.provider)} account and the conversation's own permissions. Messages from {invitation.deviceLabel} wait for your approval unless you allow it automatic turns here. Accepting gives {invitation.deviceLabel} this collaboration's Journal from this device: turns, messages and summaries, with paths only relative to the folder.</Notice>
    <div className="row" style={{ gap: 6 }}>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending} onClick={() => onAnswer(true)}>Accept</button>
      <button type="button" className="btn btn-sm" disabled={pending} onClick={() => onAnswer(false)}>Decline</button>
    </div>
  </div></div>;
}

interface PairedDeviceV1 { roomId: string; hostRef: string; label: string; connected: boolean }

function pairedDevices(rooms: RoomInfo[]): PairedDeviceV1[] {
  const devices: PairedDeviceV1[] = [];
  for (const room of rooms) {
    if (room.status !== "active" || room.pairing_state !== "paired") continue;
    const peer = (room.peers ?? []).find((candidate) => candidate.hostRef);
    if (!peer?.hostRef) continue;
    devices.push({ roomId: room.id, hostRef: peer.hostRef, label: peer.pairedDeviceLabel || peer.displayName || room.peer_device_name || "Paired device", connected: peer.connected });
  }
  return devices;
}

function InviteMemberDialog({ view, onClose, onInvite }: {
  view: CollaborationView; onClose: () => void; onInvite: (roomId: string, member: { alias: string; provider: CollaborationProvider; role: string; scopeRef: string }, firstMessage: string) => Promise<void>;
}) {
  const [devices, setDevices] = useState<PairedDeviceV1[] | null>(null);
  const [roomId, setRoomId] = useState<string>("");
  const [folders, setFolders] = useState<SharedWorkspace[] | null>(null);
  const [scopeRef, setScopeRef] = useState("");
  const [provider, setProvider] = useState<CollaborationProvider>("claude_code");
  const [alias, setAlias] = useState("");
  const [role, setRole] = useState("");
  const [firstMessage, setFirstMessage] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void listRooms().then((rooms) => {
      const found = pairedDevices(rooms);
      setDevices(found);
      if (found.length > 0) setRoomId((current) => current || found[0].roomId);
    }).catch((cause) => setError(errorText(cause, "Paired devices could not be read.")));
  }, []);
  const device = devices?.find((candidate) => candidate.roomId === roomId);
  useEffect(() => {
    setFolders(null); setScopeRef("");
    if (!device) return;
    void listRemoteSharedWorkspaces(device.roomId, device.hostRef).then((response) => {
      const usable = response.scopes.filter((scope) => scope.readSessions && scope.agentRequests);
      setFolders(usable);
      if (usable.length > 0) { setScopeRef(usable[0].scopeRef); setAlias((current) => current || suggestAlias(usable[0].label, view.members.map((member) => member.alias))); }
    }).catch((cause) => setError(errorText(cause, `${device.label} could not be asked.`)));
  }, [device?.roomId, device?.hostRef]);
  const taken = view.members.map((member) => member.alias);
  const problem = !device ? "Choose a paired device."
    : !scopeRef ? "Choose a folder that device shares with this one."
    : !validAlias(alias) ? `Aliases use lowercase letters, digits, - and _ (and not "${USER_ALIAS}").`
    : taken.includes(alias) ? `${alias} is already a member.` : null;
  const invite = async () => {
    if (!device) return;
    setPending(true);
    try { await onInvite(device.roomId, { alias, provider, role: role.trim(), scopeRef }, firstMessage); } catch (cause) { setError(errorText(cause, "The invitation could not be made.")); } finally { setPending(false); }
  };
  return <Dialog wide title={`Invite a member from another device`} onClose={onClose}
    description={`A Codex or Claude Code conversation on a device paired with this one by code, in a folder that device shares with this one with Sessions and Agent requests on. Its user accepts or declines there; its turns run there, with that user's approval or allowance. Once accepted, it receives this collaboration's Journal${view.link ? "" : " (a collaboration spans at most one other device)"}.`}
    footer={<><button type="button" className="btn btn-sm" onClick={onClose}>Cancel</button>
      <button type="button" className="btn btn-sm btn-primary" disabled={pending || !!problem} onClick={() => void invite()}>Invite</button></>}>
    <div className="stack-lg">
      {devices === null ? <span className="small muted">Looking for paired devices…</span>
        : devices.length === 0 ? <Notice tone="warn">No device is paired with this one by code. Pair a Bridge first.</Notice>
        : <Field label="Device" htmlFor="invite-device"><select id="invite-device" className="select select-sm" value={roomId} onChange={(event) => setRoomId(event.target.value)}>
            {devices.map((candidate) => <option key={candidate.roomId} value={candidate.roomId}>{candidate.label}{candidate.connected ? "" : " (not connected)"}</option>)}
          </select></Field>}
      {device && folders === null ? <span className="small muted">Asking {device.label} which folders it shares…</span> : null}
      {device && folders && folders.length === 0 ? <Notice tone="warn">{device.label} shares no folder with this device with Sessions and Agent requests on. Share one there first.</Notice> : null}
      {folders && folders.length > 0 ? <div className="row row-wrap" style={{ gap: 12, alignItems: "flex-end" }}>
        <Field label="Folder there" htmlFor="invite-folder"><select id="invite-folder" className="select select-sm" value={scopeRef} onChange={(event) => setScopeRef(event.target.value)}>
          {folders.map((folder) => <option key={folder.scopeRef} value={folder.scopeRef}>{folder.label}</option>)}
        </select></Field>
        <Field label="Agent" htmlFor="invite-agent"><select id="invite-agent" className="select select-sm" value={provider} onChange={(event) => setProvider(event.target.value as CollaborationProvider)}>
          <option value="codex">Codex</option><option value="claude_code">Claude Code</option>
        </select></Field>
        <Field label="Alias" htmlFor="invite-alias"><input id="invite-alias" className="input input-sm" maxLength={32} value={alias} placeholder="alias" onChange={(event) => setAlias(event.target.value.toLowerCase())} /></Field>
      </div> : null}
      <Field label="Role" htmlFor="invite-role"><input id="invite-role" className="input input-sm" maxLength={200} value={role} placeholder="e.g. owns web/; does not edit api/" onChange={(event) => setRole(event.target.value)} /></Field>
      <Field label="First message (optional)" hint="Waits there with the briefing until that device's user lets its first turn run." htmlFor="invite-first-message">
        <textarea id="invite-first-message" className="textarea" rows={3} maxLength={4096} value={firstMessage} onChange={(event) => setFirstMessage(event.target.value)} />
      </Field>
      {problem ? <span className="small muted">{problem}</span> : null}
      {error ? <Notice tone="bad">{error}</Notice> : null}
    </div>
  </Dialog>;
}
