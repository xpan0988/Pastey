//! Agent collaboration across two devices: M4 of
//! `docs/research/agent-collaboration-links.md`.
//!
//! A collaboration may span this device and one other, over one Bridge
//! paired by code (`bridge_peer_pins`). The device that creates it invites
//! a conversation in a folder the other device shares with it (Sessions and
//! Agent requests on); that device's user accepts or declines. Each device
//! then keeps its own copy of the collaboration, writes only its own
//! entries, and sends them to the other over Room Control
//! (`native_agent.collab_entries`), where the transport receipt says the
//! receiver stored them. Entries not yet stored there wait in the sender's
//! outbox (the user decided on 2026-11-21: until the membership or the
//! collaboration ends) and are sent again when the device is reachable.
//!
//! Authority stays with each device: a turn runs only on the device whose
//! member it is, admitted by that device's user (an approval or that
//! device's own auto grant). Nothing the other device sends can start a turn
//! without that, change a grant, or add a member here without this user's
//! acceptance.
//!
//! Identity: an event counts only if the device proved its pinned key in
//! exactly the session it arrived on, and that key is the one the Bridge's
//! pairing pinned and the link recorded. A changed or missing pin (the
//! device was forgotten, the Bridge re-paired with another key, or Burned)
//! ends the link: this device deletes the other device's members and
//! entries, and a copy of a collaboration the other device created is
//! deleted whole, with its running turns asked to stop.
//!
//! Disclosure: an entry crosses in a wire form that carries no task id and
//! no folder; paths in it are relative to a member's folder, and commands
//! appear only as counts. Messages and summaries cross as the Agent wrote
//! them, within their bounds. A linked collaboration's records are sealed at
//! rest with that Bridge's payload key, which Burn deletes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    canonical_workspace, invalid, now, valid_alias, BasisV1, CollaborationRecordV1,
    CollaborationServiceV1, CollaborationStateV1, CollaborationV1, EntryBodyV1, EntryV1,
    GrantEndV1, MemberChangeV1, MemberV1, ProviderV1, StatusV1, UserActionV1, MAX_GOAL_CHARS,
    MAX_MEMBERS, MAX_NAME_CHARS, MAX_ROLE_CHARS, SCHEMA_VERSION, USER_ALIAS,
};
use crate::error::{AppError, AppResult};
use crate::native_agent::MAX_MESSAGE_BODY_BYTES;

pub(crate) const COLLAB_MEMBER_KIND: &str = "native_agent.collab_member";
pub(crate) const COLLAB_ENTRIES_KIND: &str = "native_agent.collab_entries";
const WIRE_SCHEMA: &str = "pastey-collaboration-wire-v1";
/// Entries one event carries, and their bytes (an event is at most 64 KiB).
const MAX_BATCH_ENTRIES: usize = 50;
const MAX_BATCH_BYTES: usize = 40 * 1024;
/// One entry on the wire.
const MAX_WIRE_ENTRY_BYTES: usize = 8 * 1024;
/// Invitations one Bridge may have waiting here.
const MAX_INVITATIONS_PER_BRIDGE: usize = 10;
const MAX_SUBJECT_CHARS: usize = 300;
const MAX_TEXT_BYTES: usize = 2 * 1024;
const MAX_LISTED_PATHS: usize = 20;
/// The code a turn gets when it is asked to stop because its link ended.
pub(crate) const LINK_ENDED_CODE: &str = "native_agent_collaboration_link_ended";

/// The other device a collaboration spans.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BridgeLinkV1 {
    pub(crate) bridge_id: String,
    pub(crate) peer_host_ref: String,
    /// The other device's key, as the Bridge's pairing pinned it when the
    /// link was made. Anything else ends the link.
    pub(crate) peer_device_key: String,
    pub(crate) device_label: String,
    /// This device created the collaboration and invited the other.
    pub(crate) home: bool,
    /// Entries written here are sent there: once a member there accepted
    /// (always, on the invited device), until none is left.
    pub(crate) accepted: bool,
}

/// A member invited on the linked device, until its user answers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MemberInviteV1 {
    /// The folder there, as that device names it to this one (`scope:`).
    pub(crate) scope_ref: String,
    /// The invitation reached that device (its receipt).
    #[serde(default)]
    pub(crate) sent: bool,
}

/// Whether the linked device has stored an entry written here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum OutboxV1 {
    Pending,
    Delivered { at: i64 },
}

/// A member as an invitation describes it: no folder, no session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WireMemberV1 {
    pub(crate) alias: String,
    pub(crate) provider: ProviderV1,
    #[serde(default)]
    pub(crate) role: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MemberOpV1 {
    Invite,
    Decline,
}

/// `native_agent.collab_member`: an invitation, or declining one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollabMemberPayloadV1 {
    pub(crate) schema_version: String,
    pub(crate) request_id: String,
    pub(crate) collaboration_id: String,
    pub(crate) op: MemberOpV1,
    /// The member invited (or declined).
    pub(crate) member: WireMemberV1,
    /// Invite only: the folder the receiving device shares with the sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) goal: Option<String>,
    /// Invite only: the members already there.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) members: Vec<WireMemberV1>,
}

/// `native_agent.collab_entries`: entries the sender wrote.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollabEntriesPayloadV1 {
    pub(crate) schema_version: String,
    pub(crate) request_id: String,
    pub(crate) collaboration_id: String,
    pub(crate) entries: Vec<Value>,
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|rest| uuid::Uuid::parse_str(rest).is_ok())
}

/// The replay identity Room Control keeps for one of these events, after a
/// shape check (the handler checks the rest).
pub(crate) fn replay_id(kind: &str, payload: &Value) -> AppResult<String> {
    let refused = || AppError::InvalidInput("Invalid collaboration event.".into());
    let request = payload.get("requestId").and_then(Value::as_str).ok_or_else(refused)?;
    if payload.get("schemaVersion").and_then(Value::as_str) != Some(WIRE_SCHEMA)
        || uuid::Uuid::parse_str(request).is_err()
    {
        return Err(refused());
    }
    match kind {
        COLLAB_MEMBER_KIND => {
            serde_json::from_value::<CollabMemberPayloadV1>(payload.clone()).map_err(|_| refused())?;
            Ok(format!("native-agent-collab-member:{request}"))
        }
        COLLAB_ENTRIES_KIND => {
            let batch = serde_json::from_value::<CollabEntriesPayloadV1>(payload.clone()).map_err(|_| refused())?;
            if batch.entries.is_empty() || batch.entries.len() > MAX_BATCH_ENTRIES {
                return Err(refused());
            }
            Ok(format!("native-agent-collab-entries:{request}"))
        }
        _ => Err(refused()),
    }
}

/// An entry written here, as it crosses: no status, no outbox, no task id,
/// and the turn's outcome code under a name Room Control allows.
pub(crate) fn wire_entry(entry: &EntryV1) -> AppResult<Value> {
    let mut body = serde_json::to_value(&entry.body)?;
    if let Some(object) = body.as_object_mut() {
        if object.get("type").and_then(Value::as_str) == Some("turn") {
            object.remove("taskId");
            if let Some(code) = object.remove("code") {
                object.insert("outcomeCode".into(), code);
            }
        }
    }
    let mut wire = serde_json::json!({
        "id": entry.id,
        "at": entry.at,
        "basis": entry.basis,
        "body": body,
    });
    if let Some(member) = &entry.member {
        wire["member"] = Value::String(member.clone());
    }
    Ok(wire)
}

/// What a wire entry claims, before this device checks it.
struct IncomingV1 {
    id: String,
    at: i64,
    member: Option<String>,
    basis: BasisV1,
    body: EntryBodyV1,
}

fn bounded(text: &str, bytes: usize) -> bool {
    text.len() <= bytes
}

fn paths_bounded(paths: &[String]) -> bool {
    paths.len() <= MAX_LISTED_PATHS
        && paths
            .iter()
            .all(|path| path.len() <= 1024 && !Path::new(path).is_absolute() && !path.starts_with('/') && !path.contains(":\\"))
}

/// Reads one wire entry, refusing anything over its bounds or carrying an
/// absolute path.
fn read_wire_entry(value: &Value) -> AppResult<IncomingV1> {
    let refused = |why: &str| AppError::InvalidInput(format!("A collaboration entry was refused: {why}."));
    if serde_json::to_vec(value)?.len() > MAX_WIRE_ENTRY_BYTES {
        return Err(refused("it is too large"));
    }
    let object = value.as_object().ok_or_else(|| refused("it is not an entry"))?;
    if object.keys().any(|key| !["id", "at", "member", "basis", "body"].contains(&key.as_str())) {
        return Err(refused("it has unknown fields"));
    }
    let id = object.get("id").and_then(Value::as_str).filter(|id| valid_id(id, "entry:")).ok_or_else(|| refused("its id"))?;
    let at = object.get("at").and_then(Value::as_i64).ok_or_else(|| refused("its time"))?;
    let member = match object.get("member") {
        None => None,
        Some(Value::String(alias)) if valid_alias(alias) => Some(alias.clone()),
        Some(_) => return Err(refused("its member")),
    };
    let basis: BasisV1 = serde_json::from_value(object.get("basis").cloned().unwrap_or(Value::Null)).map_err(|_| refused("its basis"))?;
    let mut body = object.get("body").cloned().ok_or_else(|| refused("no body"))?;
    if let Some(fields) = body.as_object_mut() {
        if fields.get("type").and_then(Value::as_str) == Some("turn") {
            if fields.contains_key("taskId") || fields.contains_key("code") {
                return Err(refused("a turn names a task"));
            }
            // A task id unique here, never the other device's.
            fields.insert("taskId".into(), Value::String(format!("peer:{id}")));
            if let Some(code) = fields.remove("outcomeCode") {
                fields.insert("code".into(), code);
            }
        }
    }
    let body: EntryBodyV1 = serde_json::from_value(body).map_err(|_| refused("its body"))?;
    let within = match &body {
        EntryBodyV1::Turn { code, activity, files, .. } => {
            code.as_deref().is_none_or(|code| code.len() <= 128)
                && activity.as_ref().is_none_or(|activity| paths_bounded(&activity.files))
                && match files {
                    Some(super::FileStateV1::Observed { changes }) => {
                        paths_bounded(&changes.added) && paths_bounded(&changes.changed) && paths_bounded(&changes.removed)
                    }
                    Some(super::FileStateV1::Unavailable { reason }) => bounded(reason, 300),
                    None => true,
                }
        }
        EntryBodyV1::Message { from, to, subject, body, hop } => {
            (from == USER_ALIAS || valid_alias(from))
                && (to == USER_ALIAS || valid_alias(to))
                && subject.chars().count() <= MAX_SUBJECT_CHARS
                && !body.trim().is_empty()
                && bounded(body, MAX_MESSAGE_BODY_BYTES)
                && *hop <= super::HOP_LIMIT + 1
        }
        EntryBodyV1::Summary { summary } => [&summary.done, &summary.decisions, &summary.open, &summary.next]
            .iter()
            .all(|field| field.as_deref().is_none_or(|text| bounded(text, MAX_TEXT_BYTES))),
        EntryBodyV1::Attention { text } => bounded(text, 1024),
        EntryBodyV1::User { action } => match action {
            UserActionV1::Created { name, goal } => name.chars().count() <= MAX_NAME_CHARS && goal.chars().count() <= MAX_GOAL_CHARS,
            UserActionV1::GoalChanged { goal } => goal.chars().count() <= MAX_GOAL_CHARS,
            UserActionV1::RoleChanged { alias, role } => valid_alias(alias) && role.chars().count() <= MAX_ROLE_CHARS,
            UserActionV1::MemberResumed { alias } => valid_alias(alias),
            UserActionV1::Decided { entry, .. } => valid_id(entry, "entry:"),
            UserActionV1::Accepted { .. }
            | UserActionV1::Paused
            | UserActionV1::Resumed
            | UserActionV1::Ended
            | UserActionV1::Released => true,
            UserActionV1::PreferencesChanged { note } => bounded(note, 1024),
            UserActionV1::ExecutionChanged { alias, name } => {
                alias.as_deref().is_none_or(valid_alias) && name.chars().count() <= 60
            }
            UserActionV1::OptionsChanged { alias, model, effort } => {
                valid_alias(alias)
                    && crate::native_agent::NativeSessionOptionsV1 { model: model.clone(), effort: effort.clone() }
                        .validate()
                        .is_ok()
            }
        },
        EntryBodyV1::Member { alias, role, .. } => valid_alias(alias) && role.chars().count() <= MAX_ROLE_CHARS,
        EntryBodyV1::Grant { alias, .. } => valid_alias(alias),
        EntryBodyV1::Wait { on, reason, .. } => {
            (on == USER_ALIAS || valid_alias(on)) && reason.chars().count() <= MAX_SUBJECT_CHARS
        }
        EntryBodyV1::FilesOutside { alias, changes } => {
            valid_alias(alias) && paths_bounded(&changes.added) && paths_bounded(&changes.changed) && paths_bounded(&changes.removed)
        }
    };
    if !within {
        return Err(refused("it is over its bounds"));
    }
    Ok(IncomingV1 {
        id: id.to_owned(),
        at,
        member,
        basis,
        body,
    })
}

/// An invitation this device's user has not answered. Host-private: it
/// names the folder here.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvitationV1 {
    pub(crate) collaboration_id: String,
    pub(crate) bridge_id: String,
    pub(crate) peer_host_ref: String,
    pub(crate) peer_device_key: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
    pub(crate) name: String,
    pub(crate) goal: String,
    pub(crate) members: Vec<WireMemberV1>,
    pub(crate) member: WireMemberV1,
    pub(crate) workspace: String,
    pub(crate) received_at: i64,
}

/// What one event sent from here carries, and what its receipt settles.
#[derive(Clone, Debug)]
pub(crate) struct OutgoingV1 {
    pub(crate) bridge_id: String,
    pub(crate) peer_host_ref: String,
    pub(crate) kind: &'static str,
    pub(crate) payload: Value,
    settles: SettlesV1,
}

#[derive(Clone, Debug)]
enum SettlesV1 {
    Invite { collaboration_id: String, alias: String },
    Decline,
    Entries { collaboration_id: String, ids: Vec<String> },
}

/// The pin a Bridge has now, as the runtime reads it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PinV1 {
    pub(crate) peer_host_ref: String,
    pub(crate) device_key: String,
}

/// The device that sent an event: its Bridge, HostRef and the key it
/// proved in that session, already checked against the Bridge's pin.
#[derive(Clone, Debug)]
pub(crate) struct SenderV1 {
    pub(crate) bridge_id: String,
    pub(crate) peer_host_ref: String,
    pub(crate) device_key: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
}

/// A member of the linked device, as the invite dialog names it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteMemberInputV1 {
    pub(crate) alias: String,
    pub(crate) provider: ProviderV1,
    #[serde(default)]
    pub(crate) role: String,
    pub(crate) scope_ref: String,
}

impl CollaborationServiceV1 {
    /// Invites a conversation on the device `link` names, in the folder it
    /// shares with this one (`input.scope_ref`). Only the device that
    /// created a collaboration invites, and a collaboration spans at most
    /// one other device. `first_message`, when given, waits for its first
    /// turn there.
    pub(crate) fn invite_member(
        &mut self,
        id: &str,
        link: BridgeLinkV1,
        input: RemoteMemberInputV1,
        first_message: Option<&str>,
    ) -> AppResult<()> {
        let item = self.item(id)?;
        if item.record.state == CollaborationStateV1::Ended {
            return invalid("This collaboration has ended.");
        }
        if item.record.members.len() >= MAX_MEMBERS {
            return invalid(format!("A collaboration has at most {MAX_MEMBERS} members."));
        }
        match &item.record.link {
            Some(existing) if !existing.home => {
                return invalid(format!("Only {}, which created this collaboration, can invite members.", existing.device_label));
            }
            Some(existing) if existing.bridge_id != link.bridge_id || existing.peer_device_key != link.peer_device_key => {
                return invalid(format!("This collaboration already spans {}; it can span one other device.", existing.device_label));
            }
            _ => {}
        }
        let alias = input.alias.trim().to_owned();
        if !valid_alias(&alias) || alias == USER_ALIAS {
            return invalid(format!("\"{alias}\" is not a usable alias."));
        }
        if item.member(&alias).is_some() {
            return invalid(format!("Two members are called \"{alias}\"."));
        }
        if input.role.chars().count() > MAX_ROLE_CHARS {
            return invalid(format!("{alias}'s role is over {MAX_ROLE_CHARS} characters."));
        }
        if !input.scope_ref.starts_with("scope:") || input.scope_ref.len() > 64 {
            return invalid("Choose a folder that device shares with this one.");
        }
        let first_message = first_message.map(str::trim).filter(|text| !text.is_empty());
        if first_message.is_some_and(|text| text.len() > MAX_MESSAGE_BODY_BYTES) {
            return invalid(format!("A message is 1 to {MAX_MESSAGE_BODY_BYTES} bytes."));
        }
        let role = input.role.trim().to_owned();
        let item = self.item_mut(id)?;
        let newly_linked = item.record.link.is_none();
        if newly_linked {
            item.record.link = Some(BridgeLinkV1 {
                home: true,
                accepted: false,
                ..link
            });
        }
        item.record.members.push(MemberV1 {
            options: Default::default(),
            execution: None,
            facts: Vec::new(),
            preferences_seen: String::new(),
            alias: alias.clone(),
            provider: input.provider,
            workspace: String::new(),
            role: role.clone(),
            paused: false,
            grant: None,
            auto_task: None,
            auto_starts: Vec::new(),
            joined_seq: 0,
            remote: true,
            invite: Some(MemberInviteV1 {
                scope_ref: input.scope_ref,
                sent: false,
            }),
        });
        self.save_record(id)?;
        if newly_linked {
            // From now on its records are sealed with that Bridge's key,
            // those written before too.
            let all: Vec<EntryV1> = self.item(id)?.entries.clone();
            for entry in all {
                self.save_entry(id, &entry)?;
            }
        }
        self.append(
            id,
            Some(&alias),
            BasisV1::User,
            EntryBodyV1::Member {
                alias: alias.clone(),
                provider: input.provider,
                role,
                change: MemberChangeV1::Invited,
            },
            None,
        )?;
        if let Some(text) = first_message {
            self.user_message(id, &alias, text)?;
        }
        Ok(())
    }

    /// Everything waiting to be sent, in order: invitations, declines, then
    /// each collaboration's entries in batches.
    pub(crate) fn outgoing(&self) -> Vec<OutgoingV1> {
        let mut out = Vec::new();
        for item in self.items.values() {
            let Some(link) = &item.record.link else { continue };
            for member in &item.record.members {
                let Some(invite) = member.invite.as_ref().filter(|invite| !invite.sent) else { continue };
                let members = item
                    .record
                    .members
                    .iter()
                    .filter(|other| other.invite.is_none())
                    .map(|other| WireMemberV1 {
                        alias: other.alias.clone(),
                        provider: other.provider,
                        role: other.role.clone(),
                    })
                    .collect();
                out.push(OutgoingV1 {
                    bridge_id: link.bridge_id.clone(),
                    peer_host_ref: link.peer_host_ref.clone(),
                    kind: COLLAB_MEMBER_KIND,
                    payload: serde_json::to_value(CollabMemberPayloadV1 {
                        schema_version: WIRE_SCHEMA.into(),
                        request_id: uuid::Uuid::new_v4().to_string(),
                        collaboration_id: item.record.id.clone(),
                        op: MemberOpV1::Invite,
                        member: WireMemberV1 {
                            alias: member.alias.clone(),
                            provider: member.provider,
                            role: member.role.clone(),
                        },
                        scope_ref: Some(invite.scope_ref.clone()),
                        name: Some(item.record.name.clone()),
                        goal: Some(item.record.goal.clone()),
                        members,
                    })
                    .unwrap_or(Value::Null),
                    settles: SettlesV1::Invite {
                        collaboration_id: item.record.id.clone(),
                        alias: member.alias.clone(),
                    },
                });
            }
        }
        for (bridge_id, peer_host_ref, payload) in &self.declines {
            out.push(OutgoingV1 {
                bridge_id: bridge_id.clone(),
                peer_host_ref: peer_host_ref.clone(),
                kind: COLLAB_MEMBER_KIND,
                payload: payload.clone(),
                settles: SettlesV1::Decline,
            });
        }
        for item in self.items.values() {
            let Some(link) = item.record.link.as_ref().filter(|link| link.accepted) else { continue };
            let pending: Vec<&EntryV1> = item
                .entries
                .iter()
                .filter(|entry| entry.host.is_none() && entry.outbox == Some(OutboxV1::Pending))
                .collect();
            let mut batch: Vec<Value> = Vec::new();
            let mut ids: Vec<String> = Vec::new();
            let mut bytes = 0;
            let flush = |batch: &mut Vec<Value>, ids: &mut Vec<String>, out: &mut Vec<OutgoingV1>| {
                if batch.is_empty() {
                    return;
                }
                out.push(OutgoingV1 {
                    bridge_id: link.bridge_id.clone(),
                    peer_host_ref: link.peer_host_ref.clone(),
                    kind: COLLAB_ENTRIES_KIND,
                    payload: serde_json::to_value(CollabEntriesPayloadV1 {
                        schema_version: WIRE_SCHEMA.into(),
                        request_id: uuid::Uuid::new_v4().to_string(),
                        collaboration_id: item.record.id.clone(),
                        entries: std::mem::take(batch),
                    })
                    .unwrap_or(Value::Null),
                    settles: SettlesV1::Entries {
                        collaboration_id: item.record.id.clone(),
                        ids: std::mem::take(ids),
                    },
                });
            };
            for entry in pending {
                let Ok(wire) = wire_entry(entry) else { continue };
                let size = serde_json::to_vec(&wire).map_or(usize::MAX, |bytes| bytes.len());
                if size > MAX_WIRE_ENTRY_BYTES {
                    continue;
                }
                if batch.len() >= MAX_BATCH_ENTRIES || bytes + size > MAX_BATCH_BYTES {
                    flush(&mut batch, &mut ids, &mut out);
                    bytes = 0;
                }
                bytes += size;
                batch.push(wire);
                ids.push(entry.id.clone());
            }
            flush(&mut batch, &mut ids, &mut out);
        }
        out
    }

    /// The linked device stored what `sent` carried (its receipt).
    pub(crate) fn delivered(&mut self, sent: &OutgoingV1) -> AppResult<()> {
        match &sent.settles {
            SettlesV1::Invite { collaboration_id, alias } => {
                if let Some(invite) = self
                    .items
                    .get_mut(collaboration_id)
                    .and_then(|item| item.member_mut(alias))
                    .and_then(|member| member.invite.as_mut())
                {
                    invite.sent = true;
                    self.save_record(collaboration_id)?;
                }
            }
            SettlesV1::Decline => {
                self.declines.retain(|(_, _, payload)| payload != &sent.payload);
            }
            SettlesV1::Entries { collaboration_id, ids } => {
                let at = now();
                let Some(item) = self.items.get_mut(collaboration_id) else { return Ok(()) };
                let mut changed = Vec::new();
                for entry in item.entries.iter_mut().filter(|entry| ids.contains(&entry.id)) {
                    entry.outbox = Some(OutboxV1::Delivered { at });
                    changed.push(entry.clone());
                }
                for entry in changed {
                    self.save_entry(collaboration_id, &entry)?;
                }
            }
        }
        Ok(())
    }

    /// An invitation from `sender`, whose `workspace` the caller resolved
    /// from the folder that device was given here.
    pub(crate) fn receive_invitation(
        &mut self,
        sender: &SenderV1,
        payload: CollabMemberPayloadV1,
        workspace: &Path,
    ) -> AppResult<()> {
        if !valid_id(&payload.collaboration_id, "collaboration:") || !valid_alias(&payload.member.alias) {
            return invalid("Invalid invitation.");
        }
        let name = payload.name.unwrap_or_default();
        let goal = payload.goal.unwrap_or_default();
        if name.trim().is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || goal.chars().count() > MAX_GOAL_CHARS
            || payload.member.role.chars().count() > MAX_ROLE_CHARS
            || payload.members.len() >= MAX_MEMBERS
            || payload.members.iter().any(|member| {
                !valid_alias(&member.alias) || member.alias == payload.member.alias || member.role.chars().count() > MAX_ROLE_CHARS
            })
        {
            return invalid("Invalid invitation.");
        }
        if self.items.contains_key(&payload.collaboration_id) {
            return invalid("That collaboration is already here.");
        }
        let workspace = canonical_workspace(&workspace.to_string_lossy())?;
        if self.member_of(payload.member.provider, &workspace).is_some() {
            return invalid("That conversation is already in a collaboration.");
        }
        let waiting = self.invitations.values().filter(|invitation| invitation.bridge_id == sender.bridge_id).count();
        let key = (payload.collaboration_id.clone(), payload.member.alias.clone());
        if waiting >= MAX_INVITATIONS_PER_BRIDGE && !self.invitations.contains_key(&key) {
            return invalid("Too many invitations from that device are waiting.");
        }
        let invitation = InvitationV1 {
            collaboration_id: payload.collaboration_id,
            bridge_id: sender.bridge_id.clone(),
            peer_host_ref: sender.peer_host_ref.clone(),
            peer_device_key: sender.device_key.clone(),
            device_label: sender.device_label.clone(),
            display_fingerprint: sender.display_fingerprint.clone(),
            name,
            goal,
            members: payload.members,
            member: payload.member,
            workspace: workspace.to_string_lossy().into_owned(),
            received_at: now(),
        };
        self.save_invitation(&invitation)?;
        self.invitations.insert(key, invitation);
        Ok(())
    }

    pub(crate) fn invitations(&self) -> Vec<InvitationV1> {
        let mut list: Vec<InvitationV1> = self.invitations.values().cloned().collect();
        list.sort_by_key(|invitation| std::cmp::Reverse(invitation.received_at));
        list
    }

    /// This device's user answers an invitation. Accepting needs the folder
    /// still shared with that device as it was (`share_holds`) and the pin
    /// unchanged (`pin`); it creates this device's copy of the
    /// collaboration, whose first entries tell the other device.
    pub(crate) fn answer_invitation(
        &mut self,
        collaboration_id: &str,
        alias: &str,
        accept: bool,
        pin: Option<PinV1>,
        share_holds: bool,
    ) -> AppResult<()> {
        let key = (collaboration_id.to_owned(), alias.to_owned());
        let Some(invitation) = self.invitations.get(&key).cloned() else {
            return invalid("That invitation is no longer here.");
        };
        let pinned = pin.is_some_and(|pin| pin.device_key == invitation.peer_device_key && pin.peer_host_ref == invitation.peer_host_ref);
        if !accept {
            self.invitations.remove(&key);
            self.delete_invitation(&invitation);
            if pinned {
                let payload = serde_json::to_value(CollabMemberPayloadV1 {
                    schema_version: WIRE_SCHEMA.into(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    collaboration_id: invitation.collaboration_id.clone(),
                    op: MemberOpV1::Decline,
                    member: invitation.member.clone(),
                    scope_ref: None,
                    name: None,
                    goal: None,
                    members: Vec::new(),
                })?;
                self.declines.push((invitation.bridge_id.clone(), invitation.peer_host_ref.clone(), payload));
            }
            return Ok(());
        }
        if !pinned {
            return invalid(format!("{} is no longer paired with this device by the same key.", invitation.device_label));
        }
        if !share_holds {
            return invalid("That folder is no longer shared with that device with Sessions and Agent requests on.");
        }
        if self.items.contains_key(collaboration_id) {
            return invalid("That collaboration is already here.");
        }
        let workspace = canonical_workspace(&invitation.workspace)?;
        if self.member_of(invitation.member.provider, &workspace).is_some() {
            return invalid("That conversation is already in a collaboration.");
        }
        let mut members: Vec<MemberV1> = invitation
            .members
            .iter()
            .map(|member| MemberV1 {
                options: Default::default(),
            execution: None,
            facts: Vec::new(),
            preferences_seen: String::new(),
                alias: member.alias.clone(),
                provider: member.provider,
                workspace: String::new(),
                role: member.role.clone(),
                paused: false,
                grant: None,
                auto_task: None,
                auto_starts: Vec::new(),
                joined_seq: 0,
                remote: true,
                invite: None,
            })
            .collect();
        members.push(MemberV1 {
            options: Default::default(),
            execution: None,
            facts: Vec::new(),
            preferences_seen: String::new(),
            alias: invitation.member.alias.clone(),
            provider: invitation.member.provider,
            workspace: workspace.to_string_lossy().into_owned(),
            role: invitation.member.role.clone(),
            paused: false,
            grant: None,
            auto_task: None,
            auto_starts: Vec::new(),
            joined_seq: 0,
            remote: false,
            invite: None,
        });
        let record = CollaborationRecordV1 {
            execution: None,
            // The device that created it held its readiness barrier.
            released: true,
            no_progress: 0,
            preferences_note: String::new(),
            schema_version: SCHEMA_VERSION.into(),
            id: invitation.collaboration_id.clone(),
            name: invitation.name.clone(),
            goal: invitation.goal.clone(),
            state: CollaborationStateV1::Active,
            created_at: now(),
            members,
            link: Some(BridgeLinkV1 {
                bridge_id: invitation.bridge_id.clone(),
                peer_host_ref: invitation.peer_host_ref.clone(),
                peer_device_key: invitation.peer_device_key.clone(),
                device_label: invitation.device_label.clone(),
                home: false,
                accepted: true,
            }),
        };
        self.items.insert(
            collaboration_id.to_owned(),
            CollaborationV1 {
                record,
                entries: Vec::new(),
            },
        );
        self.save_record(collaboration_id)?;
        self.invitations.remove(&key);
        self.delete_invitation(&invitation);
        self.append(
            collaboration_id,
            None,
            BasisV1::User,
            EntryBodyV1::User {
                action: UserActionV1::Accepted {
                    alias: invitation.member.alias.clone(),
                },
            },
            None,
        )?;
        let member = &invitation.member;
        self.append(
            collaboration_id,
            Some(&member.alias),
            BasisV1::Fact,
            EntryBodyV1::Member {
                alias: member.alias.clone(),
                provider: member.provider,
                role: member.role.clone(),
                change: MemberChangeV1::Joined,
            },
            None,
        )?;
        let item = self.item_mut(collaboration_id)?;
        let seq = item.entries.last().map_or(0, |entry| entry.seq);
        if let Some(joined) = item.member_mut(&member.alias) {
            joined.joined_seq = seq;
        }
        self.save_record(collaboration_id)
    }

    /// The linked device declined an invitation from here.
    pub(crate) fn receive_decline(&mut self, sender: &SenderV1, payload: CollabMemberPayloadV1) -> AppResult<()> {
        let id = payload.collaboration_id;
        let item = self.item(&id)?;
        let linked = item.record.link.as_ref().is_some_and(|link| link.home && link.bridge_id == sender.bridge_id);
        let invited = item.member(&payload.member.alias).is_some_and(|member| member.invite.is_some());
        if !linked || !invited {
            return invalid("No such invitation.");
        }
        let alias = payload.member.alias;
        let (provider, role) = item.member(&alias).map(|member| (member.provider, member.role.clone())).expect("invited");
        self.drop_remote_member(&id, &alias, &format!("{alias} was declined on {}", sender.device_label))?;
        self.append(
            &id,
            Some(&alias),
            BasisV1::Fact,
            EntryBodyV1::Member {
                alias: alias.clone(),
                provider,
                role,
                change: MemberChangeV1::Declined,
            },
            None,
        )
        .map(|_| ())
    }

    /// Removes a member of the linked device here: what waited for it is
    /// not delivered. Publishing stops when none is left.
    pub(super) fn drop_remote_member(&mut self, id: &str, alias: &str, reason: &str) -> AppResult<()> {
        let pending: Vec<String> = self
            .item(id)?
            .entries
            .iter()
            .filter(|entry| entry.host.is_none() && entry.addressed_to(alias) && entry.status == Some(StatusV1::Elsewhere) && entry.outbox == Some(OutboxV1::Pending))
            .map(|entry| entry.id.clone())
            .collect();
        for entry_id in pending {
            self.set_status(id, &entry_id, StatusV1::Undeliverable { reason: reason.into() })?;
        }
        let item = self.item_mut(id)?;
        item.record.members.retain(|member| member.alias != alias);
        let any_remote = item.record.members.iter().any(|member| member.remote);
        if let Some(link) = item.record.link.as_mut() {
            if !any_remote {
                link.accepted = false;
            }
        }
        self.save_record(id)
    }

    /// Entries the linked device wrote. Each is checked against what that
    /// device may write here: its own members' turns, messages, summaries
    /// and grants, its user's own actions, and nothing about this device's
    /// members. Each is stored once.
    pub(crate) fn receive_entries(&mut self, sender: &SenderV1, payload: CollabEntriesPayloadV1) -> AppResult<usize> {
        let id = payload.collaboration_id.clone();
        let item = self.item(&id)?;
        let Some(link) = item.record.link.clone() else {
            return invalid("That collaboration does not span that device.");
        };
        if link.bridge_id != sender.bridge_id || link.peer_host_ref != sender.peer_host_ref || link.peer_device_key != sender.device_key {
            return invalid("That collaboration does not span that device.");
        }
        // Each entry on its own: one this device refuses is noted here and
        // never blocks the rest, nor the sender's outbox.
        let mut stored = 0;
        let mut refused = 0;
        for value in &payload.entries {
            let Ok(entry) = read_wire_entry(value) else {
                refused += 1;
                continue;
            };
            if self.item(&id)?.entries.iter().any(|known| known.id == entry.id) {
                continue;
            }
            match self.store_peer_entry(&id, &link, sender, entry) {
                Ok(()) => stored += 1,
                Err(_) => refused += 1,
            }
        }
        if refused > 0 {
            self.attention(
                &id,
                None,
                format!(
                    "Pastey refused {refused} entr{} from {}: over its bounds, or about members it does not run.",
                    if refused == 1 { "y" } else { "ies" },
                    sender.device_label
                ),
            )?;
        }
        Ok(stored)
    }

    fn store_peer_entry(&mut self, id: &str, link: &BridgeLinkV1, sender: &SenderV1, entry: IncomingV1) -> AppResult<()> {
        let item = self.item(id)?;
        let peer_member = |alias: &str| item.member(alias).is_some_and(|member| member.remote && member.invite.is_none());
        let refused = |why: String| -> AppResult<()> { invalid(format!("A collaboration entry was refused: {why}.")) };
        // Who may it speak for?
        let member_ok = match (&entry.body, entry.member.as_deref()) {
            (EntryBodyV1::Member { alias, change: MemberChangeV1::Joined, .. }, Some(member)) if member == alias => {
                if link.home {
                    // A member it accepted an invitation for.
                    item.member(alias).is_some_and(|member| member.invite.is_some())
                } else {
                    // A member added there.
                    item.member(alias).is_none_or(|member| member.remote)
                }
            }
            (EntryBodyV1::Member { alias, change: MemberChangeV1::Left, .. }, Some(member)) if member == alias => {
                peer_member(alias) || (!link.home && item.member(alias).is_some())
            }
            (EntryBodyV1::Member { alias, .. }, Some(member)) if member == alias => !link.home,
            (EntryBodyV1::Message { from, to, .. }, member) => {
                if from == USER_ALIAS {
                    entry.basis == BasisV1::User && member == Some(to.as_str())
                } else {
                    entry.basis == BasisV1::Claim && member == Some(from.as_str()) && peer_member(from)
                }
            }
            (EntryBodyV1::User { action }, member) => match action {
                UserActionV1::RoleChanged { alias, .. } => {
                    member == Some(alias.as_str()) && (!link.home || peer_member(alias))
                }
                UserActionV1::GoalChanged { .. } | UserActionV1::Created { .. } => !link.home && member.is_none(),
                UserActionV1::Decided { entry: decided, .. } => item.entries.iter().any(|known| &known.id == decided),
                _ => member.is_none_or(|alias| peer_member(alias) || !link.home),
            },
            (_, None) => true,
            (_, Some(member)) => peer_member(member) || (!link.home && item.member(member).is_some_and(|known| known.remote)),
        };
        if !member_ok {
            return refused(format!("{} may not write that", sender.device_label));
        }
        // Its effect here.
        let mut status = None;
        match &entry.body {
            EntryBodyV1::Message { from, to, hop, .. } => {
                status = Some(self.delivery_status(id, from, to, *hop)?);
            }
            EntryBodyV1::Member { alias, provider, role, change } => match change {
                MemberChangeV1::Joined => {
                    let item = self.item_mut(id)?;
                    match item.member_mut(alias) {
                        Some(member) => {
                            member.invite = None;
                            member.remote = true;
                        }
                        None => item.record.members.push(MemberV1 {
                            options: Default::default(),
            execution: None,
            facts: Vec::new(),
            preferences_seen: String::new(),
                            alias: alias.clone(),
                            provider: *provider,
                            workspace: String::new(),
                            role: role.clone(),
                            paused: false,
                            grant: None,
                            auto_task: None,
                            auto_starts: Vec::new(),
                            joined_seq: 0,
                            remote: true,
                            invite: None,
                        }),
                    }
                    if let Some(link) = item.record.link.as_mut() {
                        link.accepted = true;
                    }
                    // What waited for its acceptance goes now, history first.
                    for known in item.entries.iter_mut().filter(|known| known.host.is_none() && known.outbox.is_none()) {
                        known.outbox = Some(OutboxV1::Pending);
                    }
                    let all: Vec<EntryV1> = item.entries.iter().filter(|known| known.host.is_none()).cloned().collect();
                    self.save_record(id)?;
                    for known in all {
                        self.save_entry(id, &known)?;
                    }
                }
                MemberChangeV1::Left => {
                    let local = self.item(id)?.member(alias).is_some_and(|member| !member.remote);
                    if local {
                        // The device that created it removed this device's
                        // member: this copy ends.
                        self.end_copy(id)?;
                    } else {
                        self.drop_remote_member(id, alias, &format!("{alias} left the collaboration"))?;
                    }
                }
                MemberChangeV1::Invited | MemberChangeV1::Declined => {}
            },
            EntryBodyV1::User { action } => match action {
                UserActionV1::GoalChanged { goal } => {
                    self.item_mut(id)?.record.goal = goal.clone();
                    self.save_record(id)?;
                }
                UserActionV1::RoleChanged { alias, role } => {
                    if let Some(member) = self.item_mut(id)?.member_mut(alias) {
                        member.role = role.clone();
                    }
                    self.save_record(id)?;
                }
                UserActionV1::Ended if !link.home => {
                    self.end_copy(id)?;
                }
                _ => {}
            },
            EntryBodyV1::Attention { .. } => {}
            _ => {}
        }
        let host = Some(sender.peer_host_ref.clone());
        let item = self.item_mut(id)?;
        let stored = EntryV1 {
            id: entry.id,
            seq: item.entries.last().map_or(1, |last| last.seq + 1),
            at: entry.at,
            member: entry.member,
            basis: entry.basis,
            body: entry.body,
            status,
            host,
            outbox: None,
        };
        item.entries.push(stored.clone());
        self.save_entry(id, &stored)
    }

    /// The other device's copy ends: no turn starts here any more and what
    /// waited is dismissed; the Journal stays until deleted.
    fn end_copy(&mut self, id: &str) -> AppResult<()> {
        if self.item(id)?.record.state == CollaborationStateV1::Ended {
            return Ok(());
        }
        let granted: Vec<String> = self
            .item(id)?
            .record
            .members
            .iter()
            .filter(|member| member.grant.is_some())
            .map(|member| member.alias.clone())
            .collect();
        for alias in granted {
            self.end_grant(id, &alias, GrantEndV1::CollaborationEnded)?;
        }
        let pending: Vec<String> = self
            .item(id)?
            .entries
            .iter()
            .filter(|entry| matches!(entry.status, Some(StatusV1::Waiting) | Some(StatusV1::Approved { .. }) | Some(StatusV1::Auto { .. })))
            .map(|entry| entry.id.clone())
            .collect();
        for entry_id in pending {
            self.set_status(id, &entry_id, StatusV1::Dismissed)?;
        }
        self.item_mut(id)?.record.state = CollaborationStateV1::Ended;
        self.save_record(id)
    }

    /// On the invited device: its user leaves. Its members leave (the other
    /// device is told), and this copy ends.
    pub(crate) fn leave(&mut self, id: &str) -> AppResult<()> {
        let item = self.item(id)?;
        if !item.record.link.as_ref().is_some_and(|link| !link.home) {
            return invalid("Only a collaboration another device created can be left.");
        }
        let local: Vec<(String, ProviderV1, String)> = item
            .record
            .members
            .iter()
            .filter(|member| !member.remote)
            .map(|member| (member.alias.clone(), member.provider, member.role.clone()))
            .collect();
        for (alias, provider, role) in local {
            if self.item(id)?.scheduled_for(&alias) {
                return invalid(format!("{alias} has a turn running: stop it or wait for it to end first."));
            }
            self.append(
                id,
                Some(&alias),
                BasisV1::User,
                EntryBodyV1::Member {
                    alias: alias.clone(),
                    provider,
                    role,
                    change: MemberChangeV1::Left,
                },
                None,
            )?;
        }
        self.end_copy(id)?;
        self.append(id, None, BasisV1::User, EntryBodyV1::User { action: UserActionV1::Ended }, None)
            .map(|_| ())
    }

    /// The Bridge to `bridge_id` no longer carries this device's trust
    /// (Burn, the device forgotten, the Bridge paired with another key):
    /// its invitations go; a collaboration created here loses that device's
    /// members and every entry it wrote; a copy of one it created is deleted
    /// whole. Returns the tasks of turns to ask to stop.
    pub(crate) fn purge_bridge(&mut self, bridge_id: &str) -> AppResult<Vec<String>> {
        let invitations: Vec<_> = self
            .invitations
            .iter()
            .filter(|(_, invitation)| invitation.bridge_id == bridge_id)
            .map(|(key, invitation)| (key.clone(), invitation.clone()))
            .collect();
        for (key, invitation) in invitations {
            self.invitations.remove(&key);
            self.delete_invitation(&invitation);
        }
        self.declines.retain(|(bridge, _, _)| bridge != bridge_id);
        self.sealing_keys.borrow_mut().remove(bridge_id);
        let linked: Vec<(String, bool)> = self
            .items
            .values()
            .filter_map(|item| {
                let link = item.record.link.as_ref()?;
                (link.bridge_id == bridge_id).then(|| (item.record.id.clone(), link.home))
            })
            .collect();
        let mut stop = Vec::new();
        for (id, home) in linked {
            if !home {
                let item = self.item(&id)?;
                stop.extend(item.record.members.iter().filter_map(|member| member.auto_task.clone()));
                stop.extend(item.entries.iter().filter_map(|entry| match &entry.status {
                    Some(StatusV1::Scheduled { task_id, .. }) => Some(task_id.clone()),
                    _ => None,
                }));
                self.remove_collaboration(&id)?;
                continue;
            }
            let item = self.item_mut(&id)?;
            let label = item.record.link.as_ref().map(|link| link.device_label.clone()).unwrap_or_default();
            item.record.members.retain(|member| !member.remote);
            item.record.link = None;
            item.entries.retain(|entry| entry.host.is_none());
            for entry in item.entries.iter_mut() {
                entry.outbox = None;
                if entry.status == Some(StatusV1::Elsewhere) {
                    entry.status = Some(StatusV1::Undeliverable {
                        reason: format!("{label} is no longer linked"),
                    });
                }
            }
            if let Some(paths) = self.paths.as_ref() {
                crate::storage::delete_collaboration(paths, &id)?;
            }
            self.save_record(&id)?;
            let all: Vec<EntryV1> = self.item(&id)?.entries.clone();
            for entry in all {
                self.save_entry(&id, &entry)?;
            }
            self.attention(
                &id,
                None,
                format!("The link to {label} ended (the Bridge was burned, the device forgotten, or paired with another key): its members and what it wrote were removed here."),
            )?;
        }
        Ok(stop)
    }

    /// Ends every link whose Bridge pin no longer matches (`pin_of`).
    /// Returns the tasks of turns to ask to stop.
    pub(crate) fn settle_links(&mut self, pin_of: impl Fn(&str) -> Option<PinV1>) -> AppResult<Vec<String>> {
        let mut bridges: Vec<(String, String, String)> = self
            .items
            .values()
            .filter_map(|item| item.record.link.as_ref())
            .map(|link| (link.bridge_id.clone(), link.peer_host_ref.clone(), link.peer_device_key.clone()))
            .chain(self.invitations.values().map(|invitation| {
                (invitation.bridge_id.clone(), invitation.peer_host_ref.clone(), invitation.peer_device_key.clone())
            }))
            .collect();
        bridges.sort();
        bridges.dedup();
        let mut stop = Vec::new();
        for (bridge_id, peer_host_ref, device_key) in bridges {
            let holds = pin_of(&bridge_id).is_some_and(|pin| pin.device_key == device_key && pin.peer_host_ref == peer_host_ref);
            if !holds {
                stop.extend(self.purge_bridge(&bridge_id)?);
            }
        }
        Ok(stop)
    }

    /// Members of copies of collaborations created elsewhere whose folder is
    /// no longer shared with that device as it must be (`share_holds`) are
    /// paused, with the reason.
    pub(crate) fn pause_unshared(&mut self, share_holds: impl Fn(&BridgeLinkV1, &Path) -> bool) -> AppResult<()> {
        let mut pause = Vec::new();
        for item in self.items.values() {
            let Some(link) = item.record.link.as_ref().filter(|link| !link.home) else { continue };
            if item.record.state == CollaborationStateV1::Ended {
                continue;
            }
            for member in item.record.members.iter().filter(|member| !member.remote && !member.paused) {
                if !share_holds(link, Path::new(&member.workspace)) {
                    pause.push((item.record.id.clone(), member.alias.clone(), link.device_label.clone()));
                }
            }
        }
        for (id, alias, label) in pause {
            if let Some(member) = self.item_mut(&id)?.member_mut(&alias) {
                member.paused = true;
            }
            self.save_record(&id)?;
            self.attention(
                &id,
                Some(&alias),
                format!("{alias} is paused: its folder is no longer shared with {label} with Sessions and Agent requests on. Share it again, then resume it."),
            )?;
        }
        Ok(())
    }

    // --- at rest -----------------------------------------------------------

    fn invitation_file(&self, invitation: &InvitationV1) -> Option<PathBuf> {
        let key = blake3::hash(format!("{}\0{}", invitation.collaboration_id, invitation.member.alias).as_bytes()).to_hex();
        self.paths
            .as_ref()
            .map(|paths| paths.app_data_dir.join("collaboration-invitations").join(format!("{}.json", &key[..32])))
    }

    fn save_invitation(&self, invitation: &InvitationV1) -> AppResult<()> {
        let Some(file) = self.invitation_file(invitation) else { return Ok(()) };
        let json = self.seal(&invitation.bridge_id, serde_json::to_string(invitation)?)?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, json)?;
        Ok(())
    }

    fn delete_invitation(&self, invitation: &InvitationV1) {
        if let Some(file) = self.invitation_file(invitation) {
            let _ = std::fs::remove_file(file);
        }
    }

    /// Invitations kept on disk (those whose Bridge key is gone are deleted).
    pub(super) fn load_invitations(&mut self) {
        let Some(dir) = self.paths.as_ref().map(|paths| paths.app_data_dir.join("collaboration-invitations")) else { return };
        let Ok(listing) = std::fs::read_dir(&dir) else { return };
        for file in listing.flatten() {
            let opened = std::fs::read_to_string(file.path())
                .ok()
                .and_then(|json| self.open(&json))
                .and_then(|json| serde_json::from_str::<InvitationV1>(&json).ok());
            match opened {
                Some(invitation) => {
                    self.invitations
                        .insert((invitation.collaboration_id.clone(), invitation.member.alias.clone()), invitation);
                }
                None if self.master_key.is_some() => {
                    let _ = std::fs::remove_file(file.path());
                }
                None => {}
            }
        }
    }

    pub(super) fn sealing_key(&self, bridge_id: &str) -> Option<[u8; 32]> {
        if let Some(key) = self.sealing_keys.borrow().get(bridge_id) {
            return *key;
        }
        let key = match (self.paths.as_ref(), self.master_key.as_ref()) {
            (Some(paths), Some(master_key)) => crate::storage::bridge_sealing_key(paths, master_key, bridge_id).ok().flatten(),
            _ => None,
        };
        self.sealing_keys.borrow_mut().insert(bridge_id.to_owned(), key);
        key
    }

    /// `json` sealed with the Bridge's payload key. Without paths (tests)
    /// it is kept as it is; with paths and no key, it is refused.
    pub(super) fn seal(&self, bridge_id: &str, json: String) -> AppResult<String> {
        if self.paths.is_none() {
            return Ok(json);
        }
        let key = self
            .sealing_key(bridge_id)
            .ok_or_else(|| AppError::Crypto("That Bridge's key is unavailable.".into()))?;
        let (ciphertext, nonce) = crate::crypto::encrypt_bytes(json.as_bytes(), &key)?;
        Ok(serde_json::to_string(&SealedV1 {
            sealed: bridge_id.to_owned(),
            nonce: crate::crypto::encode_nonce(&nonce),
            data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, ciphertext),
        })?)
    }

    /// The JSON a stored record holds: itself when not sealed; opened with
    /// its Bridge's key when sealed; None when that key is gone.
    pub(super) fn open(&self, stored: &str) -> Option<String> {
        let Ok(sealed) = serde_json::from_str::<SealedV1>(stored) else {
            return Some(stored.to_owned());
        };
        let key = self.sealing_key(&sealed.sealed)?;
        let nonce = crate::crypto::decode_nonce(&sealed.nonce).ok()?;
        let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &sealed.data).ok()?;
        let plain = crate::crypto::decrypt_bytes(&data, &key, &nonce).ok()?;
        String::from_utf8(plain).ok()
    }

    /// The label the linked device has in this collaboration.
    pub(super) fn device_of(record: &CollaborationRecordV1) -> &str {
        record.link.as_ref().map_or("the other device", |link| link.device_label.as_str())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SealedV1 {
    sealed: String,
    nonce: String,
    data: String,
}

/// Whether `workspace` is shared with the device `fingerprint` names over
/// `bridge_id` now, with Sessions and Agent requests on, or opened by that
/// device under its Full Access.
pub(crate) fn share_holds(paths: &crate::storage::AppPaths, bridge_id: &str, fingerprint: &str, workspace: &Path) -> bool {
    let now = crate::storage::now_ts();
    let shared = crate::session_read_access::bridge_shares(paths, bridge_id, now).is_ok_and(|shares| {
        shares.iter().any(|share| {
            share.state == crate::session_read_access::SessionReadGrantStateV1::Active
                && share.device_fingerprint == fingerprint
                && share.read_sessions
                && share.agent_requests
                && Path::new(&share.workspace) == workspace
        })
    });
    shared
        || crate::full_access::active_grants(paths).is_ok_and(|grants| {
            grants.iter().any(|grant| {
                grant.bridge_id == bridge_id && grant.folders.iter().any(|folder| Path::new(folder) == workspace)
            })
        })
}

/// The device fingerprint for a pinned key (`peer_device_key`, encoded).
pub(crate) fn fingerprint_of(device_key: &str) -> Option<String> {
    let key = crate::crypto::decode_key(device_key).ok()?;
    Some(crate::device_identity::fingerprint(&key))
}

// --- runtime glue ----------------------------------------------------------

use std::sync::{atomic::Ordering, Arc};

use crate::host_identity::HostSessionBinding;
use crate::host_runtime::HostRuntime;

/// How long a send may wait out the receiver's rate limit.
const SEND_PATIENCE: std::time::Duration = std::time::Duration::from_secs(4);

fn pin_of(paths: &crate::storage::AppPaths, bridge_id: &str) -> Option<PinV1> {
    crate::storage::bridge_peer_pin(paths, bridge_id)
        .ok()
        .flatten()
        .map(|pin| PinV1 {
            peer_host_ref: pin.peer_host_ref,
            device_key: crate::crypto::encode_key(&pin.peer_device_public_key),
        })
}

/// The label this device's user knows the device by: as trusted, as paired
/// by code, as the Bridge shows its peer, or as the Bridge names it.
pub(crate) fn device_label(paths: &crate::storage::AppPaths, bridge_id: &str, fingerprint: &str) -> String {
    let pin = crate::storage::bridge_peer_pin(paths, bridge_id).ok().flatten();
    crate::storage::list_trusted_devices(paths)
        .ok()
        .and_then(|devices| devices.into_iter().find(|device| device.fingerprint == fingerprint).map(|device| device.label))
        .or_else(|| {
            let pin = pin.as_ref()?;
            crate::storage::known_device(paths, &pin.peer_device_public_key).ok().flatten().map(|device| device.label)
        })
        .or_else(|| {
            let pin = pin.as_ref()?;
            crate::storage::list_bridge_peer_endpoints(paths, bridge_id)
                .ok()?
                .into_iter()
                .find(|peer| peer.logical_host_ref.as_deref() == Some(pin.peer_host_ref.as_str()))
                .and_then(|peer| peer.display_name)
        })
        .or_else(|| {
            crate::storage::get_room_by_id(paths, bridge_id)
                .ok()
                .and_then(|room| room.peer_device_name)
        })
        .filter(|label| !label.trim().is_empty() && label.chars().count() <= 64 && !label.chars().any(char::is_control))
        .unwrap_or_else(|| "the other device".into())
}

/// The link a new invitation over `bridge_id` makes: the Bridge must be
/// paired with a device by code.
pub(crate) fn link_for(paths: &crate::storage::AppPaths, bridge_id: &str) -> AppResult<BridgeLinkV1> {
    let pin = pin_of(paths, bridge_id).ok_or_else(|| {
        AppError::InvalidInput("That Bridge is not paired with a device. Pair it with a code first.".into())
    })?;
    let fingerprint = fingerprint_of(&pin.device_key).unwrap_or_default();
    Ok(BridgeLinkV1 {
        bridge_id: bridge_id.to_owned(),
        peer_host_ref: pin.peer_host_ref,
        device_label: device_label(paths, bridge_id, &fingerprint),
        peer_device_key: pin.device_key,
        home: true,
        accepted: false,
    })
}

/// Ends links whose pin changed, and stops the turns that ran under them.
pub(crate) fn settle(runtime: &HostRuntime) {
    let stop = runtime
        .collaborations
        .lock()
        .settle_links(|bridge_id| pin_of(&runtime.paths, bridge_id))
        .unwrap_or_default();
    stop_turns(runtime, &stop);
}

fn stop_turns(runtime: &HostRuntime, task_ids: &[String]) {
    if !task_ids.is_empty() {
        runtime.native_agents.lock().stop_tasks_with_code(task_ids, LINK_ENDED_CODE);
    }
}

/// Burn of `bridge_id` (from `purge_bridge_runtime_authority`).
pub(crate) fn purge(runtime: &HostRuntime, bridge_id: &str) -> AppResult<()> {
    let stop = runtime.collaborations.lock().purge_bridge(bridge_id)?;
    stop_turns(runtime, &stop);
    Ok(())
}

/// Pauses members whose folder is no longer shared as it must be.
pub(crate) fn pause_unshared(runtime: &HostRuntime) {
    let paths = runtime.paths.clone();
    let _ = runtime.collaborations.lock().pause_unshared(|link, workspace| {
        fingerprint_of(&link.peer_device_key)
            .is_some_and(|fingerprint| share_holds(&paths, &link.bridge_id, &fingerprint, workspace))
    });
}

/// Sends what waits in the outbox, soon, off the caller's thread.
pub(crate) fn sync_soon(runtime: &Arc<HostRuntime>) {
    let state = runtime.clone();
    runtime.spawn(async move { sync(state).await });
}

/// One outbox pass: for each device reachable now, prove this device's
/// identity over the current session, then send in order; the first failure
/// leaves the rest of that device's items for the next pass.
pub(crate) async fn sync(runtime: Arc<HostRuntime>) {
    // One pass at a time on this Host; the next tick catches up.
    let syncing = runtime.collaborations.lock().syncing.clone();
    if syncing.swap(true, Ordering::AcqRel) {
        return;
    }
    settle(&runtime);
    let outgoing = runtime.collaborations.lock().outgoing();
    let mut sessions: std::collections::HashMap<String, Option<HostSessionBinding>> = std::collections::HashMap::new();
    for item in outgoing {
        if !sessions.contains_key(&item.bridge_id) {
            let session = match crate::host_identity::HostRef::parse_peer(item.peer_host_ref.clone(), &runtime.local_host_ref) {
                Ok(target) => runtime.resolve_current_remote_host_session(&item.bridge_id, &target).await.ok(),
                Err(_) => None,
            };
            let binding = match session {
                Some(session) => crate::commands::present_device_identity_over(runtime.clone(), session.binding())
                    .await
                    .ok()
                    .map(|()| session.binding().clone()),
                None => None,
            };
            sessions.insert(item.bridge_id.clone(), binding);
        }
        let Some(binding) = sessions.get(&item.bridge_id).cloned().flatten() else {
            continue;
        };
        let sent = async {
            let context =
                crate::room_control::room_control_session_context_for_peer(&runtime, &binding.bridge_id, &binding.peer_route_ref)?;
            crate::room_control::send_room_control_event_with_backpressure(
                runtime.clone(),
                &binding.bridge_id,
                || crate::room_control::native_agent_event(item.kind, item.payload.clone(), &context),
                Some(crate::room_control::selected_peer_route(&binding.bridge_id, &binding.peer_route_ref)),
                SEND_PATIENCE,
            )
            .await
        }
        .await;
        match sent {
            Ok(_) => {
                let _ = runtime.collaborations.lock().delivered(&item);
            }
            Err(_) => {
                // Order matters within a device: the rest wait.
                sessions.insert(item.bridge_id.clone(), None);
            }
        }
    }
    syncing.store(false, Ordering::Release);
}

/// Why a received event was refused.
#[derive(Debug)]
pub(crate) enum RefusedV1 {
    /// The device did not prove the key the Bridge pinned in this session.
    Identity,
    Invalid(String),
}

/// A `native_agent.collab_member` or `native_agent.collab_entries` event
/// that arrived on exactly `binding`. Stored (or refused) before the
/// transport receipt is sent; what it makes possible starts afterwards.
pub(crate) fn receive(runtime: &Arc<HostRuntime>, kind: &str, payload: Value, binding: &HostSessionBinding) -> Result<(), RefusedV1> {
    let device = crate::host_runtime::verified_peer_device_for(runtime, binding).ok_or(RefusedV1::Identity)?;
    let pin = pin_of(&runtime.paths, &binding.bridge_id).ok_or(RefusedV1::Identity)?;
    let device_key = crate::crypto::encode_key(&device.public_key);
    if pin.device_key != device_key || pin.peer_host_ref != binding.peer_host_ref.as_str() || device.binding.bridge_id != binding.bridge_id {
        return Err(RefusedV1::Identity);
    }
    settle(runtime);
    let sender = SenderV1 {
        bridge_id: binding.bridge_id.clone(),
        peer_host_ref: binding.peer_host_ref.as_str().to_owned(),
        device_label: device_label(&runtime.paths, &binding.bridge_id, &device.fingerprint),
        display_fingerprint: crate::device_identity::display_fingerprint(&device.fingerprint),
        device_key,
    };
    let refused = |error: AppError| RefusedV1::Invalid(error.message());
    match kind {
        COLLAB_MEMBER_KIND => {
            let payload: CollabMemberPayloadV1 =
                serde_json::from_value(payload).map_err(|_| RefusedV1::Invalid("Invalid collaboration invitation.".into()))?;
            match payload.op {
                MemberOpV1::Invite => {
                    let scope_ref = payload.scope_ref.clone().unwrap_or_default();
                    let now = crate::storage::now_ts();
                    // Only a folder this device shares with that one, with
                    // Sessions and Agent requests on (or one it opened under
                    // Full Access here).
                    let workspace = crate::session_read_access::agent_request_scopes(&runtime.paths, &device, binding, now, true)
                        .map_err(refused)?
                        .into_iter()
                        .find(|(scope, _)| *scope == scope_ref)
                        .map(|(_, workspace)| workspace)
                        .ok_or_else(|| {
                            RefusedV1::Invalid("That folder is not shared with that device with Sessions and Agent requests on.".into())
                        })?;
                    runtime
                        .collaborations
                        .lock()
                        .receive_invitation(&sender, payload, &workspace)
                        .map_err(refused)
                }
                MemberOpV1::Decline => runtime.collaborations.lock().receive_decline(&sender, payload).map_err(refused),
            }
        }
        COLLAB_ENTRIES_KIND => {
            let payload: CollabEntriesPayloadV1 =
                serde_json::from_value(payload).map_err(|_| RefusedV1::Invalid("Invalid collaboration entries.".into()))?;
            runtime
                .collaborations
                .lock()
                .receive_entries(&sender, payload)
                .map(|_| ())
                .map_err(refused)
        }
        _ => Err(RefusedV1::Invalid("Unknown collaboration event.".into())),
    }
}

/// An invitation as this device's user sees it: the folder by its name.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InvitationViewV1 {
    pub(crate) collaboration_id: String,
    pub(crate) bridge_id: String,
    pub(crate) device_label: String,
    pub(crate) display_fingerprint: String,
    pub(crate) name: String,
    pub(crate) goal: String,
    pub(crate) members: Vec<WireMemberV1>,
    pub(crate) member: WireMemberV1,
    /// The folder here (this device's own).
    pub(crate) workspace: String,
    pub(crate) received_at: i64,
}

impl From<&InvitationV1> for InvitationViewV1 {
    fn from(invitation: &InvitationV1) -> Self {
        Self {
            collaboration_id: invitation.collaboration_id.clone(),
            bridge_id: invitation.bridge_id.clone(),
            device_label: invitation.device_label.clone(),
            display_fingerprint: invitation.display_fingerprint.clone(),
            name: invitation.name.clone(),
            goal: invitation.goal.clone(),
            members: invitation.members.clone(),
            member: invitation.member.clone(),
            workspace: invitation.workspace.clone(),
            received_at: invitation.received_at,
        }
    }
}

/// This device's user answers an invitation (a local command only).
pub(crate) fn answer(runtime: &Arc<HostRuntime>, collaboration_id: &str, alias: &str, accept: bool) -> AppResult<()> {
    settle(runtime);
    let invitation = runtime
        .collaborations
        .lock()
        .invitations()
        .into_iter()
        .find(|invitation| invitation.collaboration_id == collaboration_id && invitation.member.alias == alias);
    let Some(invitation) = invitation else {
        return invalid("That invitation is no longer here.");
    };
    let pin = pin_of(&runtime.paths, &invitation.bridge_id);
    let holds = fingerprint_of(&invitation.peer_device_key).is_some_and(|fingerprint| {
        share_holds(&runtime.paths, &invitation.bridge_id, &fingerprint, Path::new(&invitation.workspace))
    });
    runtime
        .collaborations
        .lock()
        .answer_invitation(collaboration_id, alias, accept, pin, holds)?;
    sync_soon(runtime);
    Ok(())
}
