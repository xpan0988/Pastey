#!/usr/bin/env python3
"""Reset a development physical ledger whose record format is out of date.

Pastey refuses to start with a physical ledger whose content format predates the
current build ("legacy physical ledger (older format); reset required"). Record
bodies are never migrated. This tool returns the ledger to the Stage 2 base
schema so the next Pastey start rebuilds every later stage empty.

Kept untouched: physical domains (including their epoch floors), aliases,
environment registrations and environment/domain membership.
Removed: qualifications, reviews, attempts, sessions, actions, evidence,
decision records, remote/native/qualification ledgers and the ledger format
marker. Old reviews, approvals and roots can therefore never be reused.

Quit Pastey first. Usage: reset-physical-ledger.py [--dry-run] PATH/TO/pastey.db
"""
import argparse
import sqlite3
import sys

# The Stage 2 base ledger. Everything else under physical_* is created by later
# stages (or the format marker) and is rebuilt by Pastey on the next start.
BASE_TABLES = {
    "physical_schema",
    "physical_domains",
    "physical_aliases",
    "physical_environments",
    "physical_environment_domains",
    "physical_qualifications",
}
BASE_CONTENT = "physical_qualifications"


def reset(path, dry_run=False):
    conn = sqlite3.connect(path, isolation_level=None)
    try:
        conn.execute("PRAGMA foreign_keys=OFF")
        conn.execute("BEGIN IMMEDIATE")
        tables = [r[0] for r in conn.execute(
            "SELECT name FROM sqlite_master WHERE type='table' AND name GLOB 'physical_*' ORDER BY name")]
        if "physical_schema" not in tables or not BASE_TABLES.issubset(tables):
            raise SystemExit(f"{path}: no recognizable Pastey physical ledger")
        report = []
        for table in tables:
            if table in BASE_TABLES and table != BASE_CONTENT:
                continue
            rows = conn.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
            report.append((table, rows, "cleared" if table == BASE_CONTENT else "dropped"))
        # Base content table: lift only its delete guard for the duration.
        guards = conn.execute(
            "SELECT name, sql FROM sqlite_master WHERE type='trigger' AND tbl_name=? "
            "AND sql LIKE '%BEFORE DELETE%'", (BASE_CONTENT,)).fetchall()
        for name, _ in guards:
            conn.execute(f"DROP TRIGGER {name}")
        conn.execute(f"DELETE FROM {BASE_CONTENT}")
        for _, sql in guards:
            conn.execute(sql)
        for table, _, action in report:
            if action == "dropped":
                conn.execute(f"DROP TABLE {table}")  # drops its triggers and indexes
        if conn.execute("PRAGMA foreign_key_check").fetchone() is not None:
            raise SystemExit(f"{path}: foreign key violation after reset; nothing changed")
        kept = conn.execute(
            "SELECT domain_id, epoch FROM physical_domains ORDER BY domain_id").fetchall()
        conn.execute("ROLLBACK" if dry_run else "COMMIT")
    except BaseException:
        if conn.in_transaction:
            conn.execute("ROLLBACK")
        raise
    finally:
        conn.close()
    return report, kept


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dry-run", action="store_true", help="report without changing the ledger")
    parser.add_argument("database")
    args = parser.parse_args()
    report, kept = reset(args.database, args.dry_run)
    for table, rows, action in report:
        print(f"{action:8} {table} ({rows} rows)")
    print(f"kept     physical_domains ({len(kept)} domains, epoch floors unchanged)")
    print("dry run: no changes written" if args.dry_run else "reset complete; start Pastey to rebuild the ledger")


if __name__ == "__main__":
    sys.exit(main())
