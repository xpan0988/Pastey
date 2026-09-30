//! Retired Stage 8 native receipt table. No binding can produce a native
//! receipt (no verifier exists), so NativeFence installation fails closed; the
//! table stays in the DDL so the staged schema chain is unchanged, and it must
//! stay empty. See docs/device-binding-protocol.md for the fence semantics.
use super::*;
pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_native_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_native_schema VALUES(1,1);
CREATE TABLE physical_native_receipts(session_id TEXT NOT NULL REFERENCES physical_sessions(session_id),kind TEXT NOT NULL CHECK(kind IN ('install','fence','command')),request_id TEXT NOT NULL,digest TEXT NOT NULL CHECK(length(digest)=64),receipt_json TEXT NOT NULL,PRIMARY KEY(session_id,kind,request_id)) STRICT;
CREATE UNIQUE INDEX physical_native_transitions ON physical_native_receipts(session_id,kind) WHERE kind IN ('install','fence');
CREATE TRIGGER physical_native_immutable BEFORE UPDATE ON physical_native_receipts BEGIN SELECT RAISE(ABORT,'native evidence immutable');END;
CREATE TRIGGER physical_native_keep BEFORE DELETE ON physical_native_receipts BEGIN SELECT RAISE(ABORT,'native evidence history required');END;
"#;
pub(super) fn audit(c: &Connection, scope: super::AuditScopeV1) -> AppResult<()> {
    let version: i64 = c.query_row(
        "SELECT version FROM physical_native_schema WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    require(version == 1, "Unsupported native receipt schema")?;
    let rows: i64 = c.query_row("SELECT count(*) FROM physical_native_receipts", [], |r| {
        r.get(0)
    })?;
    require(
        rows == 0,
        "Retired native receipt records present; reset required",
    )?;
    let native:bool=c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM physical_sessions WHERE {} AND (install_evidence='native_fence' OR fence_ack='native_fence'))", scope.filter("root_id", "root")),[],|r|r.get(0))?;
    require(!native, "Missing native enforcement receipt")
}
