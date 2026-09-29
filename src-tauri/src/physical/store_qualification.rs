//! Retired Stage 9 evidence-bundle table. The device-specific Gate B
//! qualification route is gone (see the repository's legacy/ directory); the
//! table stays in the DDL so the staged schema chain is unchanged, and it must
//! stay empty.
use super::*;
pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_gate_b_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_gate_b_schema VALUES(1,1);
CREATE TABLE physical_gate_b_qualifications(qualification_id TEXT PRIMARY KEY REFERENCES physical_qualifications(qualification_id),record_digest TEXT NOT NULL CHECK(length(record_digest)=64),record_json TEXT NOT NULL) STRICT;
CREATE TRIGGER physical_gate_b_immutable BEFORE UPDATE ON physical_gate_b_qualifications BEGIN SELECT RAISE(ABORT,'Gate B qualification immutable');END;
CREATE TRIGGER physical_gate_b_keep BEFORE DELETE ON physical_gate_b_qualifications BEGIN SELECT RAISE(ABORT,'Gate B qualification history required');END;
"#;
pub(super) fn audit(c: &Connection) -> AppResult<()> {
    let v: i64 = c.query_row(
        "SELECT version FROM physical_gate_b_schema WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    require(v == 1, "Unknown Gate B qualification schema")?;
    let rows: i64 = c.query_row(
        "SELECT count(*) FROM physical_gate_b_qualifications",
        [],
        |r| r.get(0),
    )?;
    require(
        rows == 0,
        "Retired Gate B qualification records present; reset required",
    )
}
