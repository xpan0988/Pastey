//! Immutable Stage 9 evidence bundle; never a live launch/session recovery row.
use super::*;
use crate::physical::core::qualification::GateBQualificationRecordV1;
pub(super) const SCHEMA: &str = r#"
CREATE TABLE physical_gate_b_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL CHECK(version=1)) STRICT;
INSERT INTO physical_gate_b_schema VALUES(1,1);
CREATE TABLE physical_gate_b_qualifications(qualification_id TEXT PRIMARY KEY REFERENCES physical_qualifications(qualification_id),record_digest TEXT NOT NULL CHECK(length(record_digest)=64),record_json TEXT NOT NULL) STRICT;
CREATE TRIGGER physical_gate_b_immutable BEFORE UPDATE ON physical_gate_b_qualifications BEGIN SELECT RAISE(ABORT,'Gate B qualification immutable');END;
CREATE TRIGGER physical_gate_b_keep BEFORE DELETE ON physical_gate_b_qualifications BEGIN SELECT RAISE(ABORT,'Gate B qualification history required');END;
"#;
pub(super) fn insert(c: &Connection, r: &GateBQualificationRecordV1) -> AppResult<()> {
    r.validate()?;
    c.execute(
        "INSERT INTO physical_gate_b_qualifications VALUES(?1,?2,?3)",
        params![
            text(&r.qualification.qualification_id),
            text(&digest("pastey-gate-b-qualification-record-v1", r)?),
            serde_json::to_string(r)?
        ],
    )?;
    Ok(())
}
pub(super) fn audit(c: &Connection) -> AppResult<()> {
    let v: i64 = c.query_row(
        "SELECT version FROM physical_gate_b_schema WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    require(v == 1, "Unknown Gate B qualification schema")?;
    let mut q = c.prepare(
        "SELECT qualification_id,record_digest,record_json FROM physical_gate_b_qualifications",
    )?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, d, raw) = row?;
        let record: GateBQualificationRecordV1 = decode(&raw)?;
        record.validate()?;
        let qraw: String = c.query_row(
            "SELECT record_json FROM physical_qualifications WHERE qualification_id=?1",
            [&id],
            |r| r.get(0),
        )?;
        let qualification: PhysicalQualificationV1 = decode(&qraw)?;
        require(
            id == text(&record.qualification.qualification_id)
                && qualification == record.qualification
                && d == text(&digest("pastey-gate-b-qualification-record-v1", &record)?),
            "Gate B record/body correlation mismatch",
        )?;
    }
    Ok(())
}
impl PhysicalStoreV1 {
    pub(in crate::physical) fn record_gate_b_qualification(
        &self,
        environment: &EnvironmentRefV1,
        reg: &DigestV1,
        q: &PhysicalQualificationV1,
        provenance: &DigestV1,
        record: &GateBQualificationRecordV1,
    ) -> AppResult<()> {
        require(
            record.qualification == *q && record.registration_digest == *reg,
            "Native qualification registration mismatch",
        )?;
        self.record_qualification_inner(environment, reg, q, provenance, Some(record))
    }
    pub(in crate::physical) fn gate_b_record(
        &self,
        id: &QualificationId,
    ) -> AppResult<GateBQualificationRecordV1> {
        let c = self.connection()?;
        let raw: String = c.query_row(
            "SELECT record_json FROM physical_gate_b_qualifications WHERE qualification_id=?1",
            [text(id)],
            |r| r.get(0),
        )?;
        let record: GateBQualificationRecordV1 = decode(&raw)?;
        record.validate()?;
        Ok(record)
    }
}
