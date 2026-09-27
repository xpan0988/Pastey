//! Stage 8 native receipts are historical facts, never session restoration inputs.
use super::*;
use crate::physical::{core::gate_b, native_protocol as wire};
pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_native_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_native_schema VALUES(1,1);
CREATE TABLE physical_native_receipts(session_id TEXT NOT NULL REFERENCES physical_sessions(session_id),kind TEXT NOT NULL CHECK(kind IN ('install','fence','command')),request_id TEXT NOT NULL,digest TEXT NOT NULL CHECK(length(digest)=64),receipt_json TEXT NOT NULL,PRIMARY KEY(session_id,kind,request_id)) STRICT;
CREATE UNIQUE INDEX physical_native_transitions ON physical_native_receipts(session_id,kind) WHERE kind IN ('install','fence');
CREATE TRIGGER physical_native_immutable BEFORE UPDATE ON physical_native_receipts BEGIN SELECT RAISE(ABORT,'native evidence immutable');END;
CREATE TRIGGER physical_native_keep BEFORE DELETE ON physical_native_receipts BEGIN SELECT RAISE(ABORT,'native evidence history required');END;
"#;
pub(in crate::physical) fn record(
    tx: &Connection,
    session: &SessionId,
    kind: &str,
    receipt: &wire::Receipt,
) -> AppResult<()> {
    tx.execute(
        "INSERT INTO physical_native_receipts VALUES(?1,?2,?3,?4,?5)",
        params![
            text(session),
            kind,
            receipt.request,
            text(&digest("pastey-native-enforcement-receipt-v1", receipt)?),
            serde_json::to_string(receipt)?
        ],
    )?;
    Ok(())
}
pub(super) fn audit(c: &Connection) -> AppResult<()> {
    let version: i64 = c.query_row(
        "SELECT version FROM physical_native_schema WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    require(version == 1, "Unsupported native receipt schema")?;
    let mut q = c.prepare(
        "SELECT session_id,kind,request_id,digest,receipt_json FROM physical_native_receipts",
    )?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })? {
        let (id, kind, request, d, raw) = row?;
        let receipt: wire::Receipt = decode(&raw)?;
        require(
            d == text(&digest("pastey-native-enforcement-receipt-v1", &receipt)?)
                && request == receipt.request,
            "Changed native receipt audit",
        )?;
        let sraw: String = c.query_row(
            "SELECT audit_json FROM physical_sessions WHERE session_id=?1",
            [&id],
            |r| r.get(0),
        )?;
        let session: control_ledger::SessionAuditV1 = decode(&sraw)?;
        require(
            session.enforcement == SessionEnforcementClassV1::NativeFence,
            "Native proof attached to isolation session",
        )?;
        if kind == "command" {
            let action = receipt.action.as_ref().ok_or_else(|| {
                crate::error::AppError::InvalidInput("Missing native action".into())
            })?;
            let raw: String = c.query_row(
                "SELECT audit_json FROM physical_actions WHERE action_id=?1",
                [&action.action],
                |r| r.get(0),
            )?;
            let a: control_ledger::ActionAuditV1 = decode(&raw)?;
            gate_b::validate_command_receipt(
                &receipt,
                &session.scope.fields().environment,
                &a,
                &RequestId::try_from(request)?,
                receipt.accepted,
            )?;
            continue;
        }
        let (epochs, expected) = if kind == "install" {
            (session.epochs.clone(), session.installation.clone())
        } else {
            let raw: String = c.query_row(
                "SELECT fence_json FROM physical_sessions WHERE session_id=?1",
                [&id],
                |r| r.get(0),
            )?;
            let f: control_ledger::FenceAuditV1 = decode(&raw)?;
            (f.epochs, f.request)
        };
        gate_b::validate_historical_receipt(
            &receipt,
            &session.scope.fields().environment,
            &session.id,
            &epochs,
            &expected,
            kind == "fence",
        )?;
    }
    let missing:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM physical_sessions s WHERE (s.install_evidence='native_fence' AND NOT EXISTS(SELECT 1 FROM physical_native_receipts r WHERE r.session_id=s.session_id AND r.kind='install')) OR (s.fence_ack='native_fence' AND NOT EXISTS(SELECT 1 FROM physical_native_receipts r WHERE r.session_id=s.session_id AND r.kind='fence')))",[],|r|r.get(0))?;
    require(!missing, "Missing native enforcement receipt")
}
