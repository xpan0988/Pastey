//! Persisted record bodies are gated by the ledger content format, never by
//! decoding an older shape. Reset keeps domains, epochs and environments.
use super::*;

fn init(f: &ControlFixture) -> crate::error::AppResult<()> {
    storage::init_database(&f.paths)
}
fn reset_script(f: &ControlFixture) -> std::process::Output {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reset-physical-ledger.py");
    std::process::Command::new("python3")
        .arg("-B")
        .arg(script)
        .arg(&f.paths.db_path)
        .output()
        .expect("python3 is required for the ledger reset tool")
}
fn epochs(f: &ControlFixture) -> Vec<(String, i64)> {
    f.sql()
        .prepare("SELECT domain_id, epoch FROM physical_domains ORDER BY domain_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn older_or_missing_format_with_content_fails_closed_until_reset() {
    let f = ControlFixture::new();
    let s = f.active().await;
    let (g, p) = f.challenged(&s);
    f.admit(&g, p);
    f.core.lock().close().unwrap();
    init(&f).unwrap();
    assert_eq!(
        f.scalar("SELECT format_version FROM physical_ledger_meta"),
        crate::physical::store::LEDGER_FORMAT
    );
    let floors = epochs(&f);
    let environments = f.scalar("SELECT count(*) FROM physical_environments");
    assert!(floors.iter().all(|(_, e)| *e > 1) && environments > 0);

    for (marker, expected) in [
        (
            "DROP TABLE physical_ledger_meta",
            "legacy physical ledger (pre-decouple); reset required",
        ),
        (
            "UPDATE physical_ledger_meta SET format_version=1",
            "legacy physical ledger (pre-decouple); reset required",
        ),
        (
            "UPDATE physical_ledger_meta SET format_version=99",
            "Unknown newer physical ledger format",
        ),
    ] {
        f.sql().execute_batch(marker).unwrap();
        let error = init(&f).unwrap_err().to_string();
        assert!(error.contains(expected), "{marker}: {error}");
        // Restore the current marker for the next case.
        f.sql()
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS physical_ledger_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format_version INTEGER NOT NULL CHECK(format_version>=1)) STRICT;
                 INSERT OR REPLACE INTO physical_ledger_meta VALUES(1,{})",
                crate::physical::store::LEDGER_FORMAT
            ))
            .unwrap();
    }

    // Legacy ledger: marker gone, content present. The reset tool clears content
    // while keeping domain epoch floors and environment registrations.
    f.sql()
        .execute_batch("UPDATE physical_ledger_meta SET format_version=1")
        .unwrap();
    assert!(init(&f).is_err());
    let out = reset_script(&f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    init(&f).unwrap();
    assert_eq!(epochs(&f), floors);
    assert_eq!(
        f.scalar("SELECT count(*) FROM physical_environments"),
        environments
    );
    for table in [
        "physical_reviews",
        "physical_attempts",
        "physical_sessions",
        "physical_actions",
        "physical_qualifications",
    ] {
        assert_eq!(
            f.scalar(&format!("SELECT count(*) FROM {table}")),
            0,
            "{table}"
        );
    }
    assert_eq!(
        f.scalar("SELECT format_version FROM physical_ledger_meta"),
        crate::physical::store::LEDGER_FORMAT
    );
}

#[test]
fn older_format_without_content_upgrades_in_place() {
    let dir = std::env::temp_dir().join(format!("pastey-physical-format-{}", uuid::Uuid::new_v4()));
    let paths = AppPaths::new(dir.clone(), dir.join("logs"));
    paths.ensure_directories().unwrap();
    storage::init_database(&paths).unwrap();
    let sql = rusqlite::Connection::open(&paths.db_path).unwrap();
    // Schema and version markers only: no record body in an older format exists.
    sql.execute_batch("DROP TABLE physical_ledger_meta")
        .unwrap();
    storage::init_database(&paths).unwrap();
    let format: i64 = sql
        .query_row("SELECT format_version FROM physical_ledger_meta", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(format, crate::physical::store::LEDGER_FORMAT);
    drop(sql);
    let _ = std::fs::remove_dir_all(dir);
}
