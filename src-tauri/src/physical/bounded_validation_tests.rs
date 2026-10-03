//! Bounded Root-group validation (docs/physical.md, "Ledger validation"): a
//! write that changes no evidence or consequence row trusts its Roots'
//! validated history instead of replaying it, and reaches the verdicts the
//! full group audit reaches. Under `cfg(test)` every bounded validation is
//! also run as the full group audit and must agree (the oracle), so every
//! test here, and every other ledger test, checks that equivalence.
use super::*;
use crate::physical::store::validation_stats;

/// The walk with leases long enough to let a stream collect evidence for
/// minutes, as a brain that thinks between calls lets it.
const LONG: EnvelopeV1 = EnvelopeV1 {
    idle_lease_us: 900_000_000,
    lease_us: 900_000_000,
    approval_lifetime_us: 900_000_000,
    root_lifetime_us: 900_000_000,
    ..WALK
};

/// A started walk whose first action keeps collecting evidence: the body
/// keeps reporting after the action ends, one observation and one
/// consequence revision per tick.
struct GrownV1 {
    executor: Arc<ExecutorV1>,
    demo: DemoV1,
    root: RootId,
}
impl GrownV1 {
    fn to(n: u64) -> Self {
        let executor = ExecutorV1::launch_walk(false, &LONG);
        let demo = executor.walk();
        demo.started();
        let root = demo.root();
        let first = demo.call(
            "brain",
            &DecisionCallV1 {
                option: "turn_left".into(),
                duration_ms: 500,
            },
        );
        assert!(first.allowed(), "{first:?}");
        let grown = Self {
            executor,
            demo,
            root,
        };
        grown.grow(n);
        grown
    }
    fn grow(&self, n: u64) {
        while self.evidence() < n {
            self.demo.wait_ms(200);
        }
    }
    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.executor.paths.db_path).unwrap()
    }
    fn root_text(&self) -> String {
        String::from(self.root.clone())
    }
    fn scalar(&self, sql: &str) -> i64 {
        self.sql()
            .query_row(sql, [self.root_text()], |r| r.get(0))
            .unwrap()
    }
    fn evidence(&self) -> u64 {
        self.scalar("SELECT count(*) FROM physical_evidence WHERE action_id IN (SELECT action_id FROM physical_actions WHERE root_id=?1)") as u64
    }
    fn decisions(&self) -> i64 {
        self.scalar("SELECT count(*) FROM physical_decisions WHERE root_id=?1")
    }
    fn latest_action(&self) -> String {
        self.sql()
            .query_row(
                "SELECT action_id FROM physical_actions WHERE root_id=?1 ORDER BY decision_sequence DESC LIMIT 1",
                [self.root_text()],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn store<T>(&self, f: impl FnOnce(&PhysicalStoreV1) -> T) -> T {
        let core = self.executor.core.lock();
        f(core_fake::store(&core))
    }
    /// One Root-keyed write: a refusal record, as a refused decide writes it.
    fn refusal(&self) -> crate::error::AppResult<()> {
        let now = UnixMillis::try_from(self.executor.clock.wall.load(Ordering::SeqCst)).unwrap();
        self.store(|s| {
            s.record_refusal(
                &self.root,
                &label("brain"),
                "forward",
                500_000,
                "refused",
                now,
            )
        })
    }
    /// The Root's history heads in the ledger, by action.
    fn heads(&self) -> BTreeMap<String, (i64, i64)> {
        let c = self.sql();
        let mut stmt = c
            .prepare(
                "SELECT a.action_id,
                 COALESCE((SELECT max(revision) FROM physical_evidence e WHERE e.action_id=a.action_id),0),
                 COALESCE((SELECT max(revision) FROM physical_consequences q WHERE q.action_id=a.action_id),0)
                 FROM physical_actions a WHERE a.root_id=?1",
            )
            .unwrap();
        let heads = stmt
            .query_map([self.root_text()], |r| {
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        heads
    }
    fn baseline(&self) -> Option<(i64, BTreeMap<String, (i64, i64)>)> {
        self.store(|s| s.test_history_baseline())
            .map(|(v, roots)| (v, roots.get(&self.root_text()).cloned().unwrap_or_default()))
    }
}
/// The validation paths this thread's commits took since the last call.
fn paths() -> Vec<(&'static str, &'static str)> {
    validation_stats::take().validations
}

#[tokio::test(flavor = "multi_thread")]
async fn root_keyed_writes_trust_validated_history_and_agree_with_the_group_audit() {
    let g = GrownV1::to(30);
    paths();
    let decided = g.demo.call(
        "brain",
        &DecisionCallV1 {
            option: "forward".into(),
            duration_ms: 500,
        },
    );
    assert!(decided.allowed(), "{decided:?}");
    let refused = g.demo.call(
        "brain",
        &DecisionCallV1 {
            option: UNAPPROVED_OPTION.into(),
            duration_ms: 500,
        },
    );
    assert!(!refused.allowed(), "{refused:?}");
    g.demo.revoke();
    let taken = paths();
    for kind in [
        "admit_action",
        "prepare_write",
        "finish_write",
        "record_refusal",
        "close_attempt",
        "acknowledge_fence",
    ] {
        assert!(
            taken.contains(&(kind, "bounded")),
            "{kind} did not take the bounded path: {taken:?}"
        );
        assert!(
            !taken.contains(&(kind, "replayed")),
            "{kind} replayed validated history: {taken:?}"
        );
    }
}

/// The work of a Root-keyed write does not depend on how much history the
/// Root has. `PASTEY_TEST_HISTORY_SIZES` (comma separated) widens the sizes.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_keyed_write_reads_no_history_at_any_size() {
    let sizes: Vec<u64> = std::env::var("PASTEY_TEST_HISTORY_SIZES")
        .ok()
        .map(|v| v.split(',').map(|n| n.trim().parse().unwrap()).collect())
        .unwrap_or_else(|| vec![25, 100]);
    let g = GrownV1::to(sizes[0]);
    g.store(|s| s.test_without_oracle());
    for n in sizes {
        g.grow(n);
        validation_stats::take();
        let started = std::time::Instant::now();
        g.refusal().unwrap();
        let elapsed = started.elapsed();
        let stats = validation_stats::take();
        assert!(stats.validations.contains(&("record_refusal", "bounded")));
        assert_eq!(stats.history_rows, 0, "history rows read at N={n}");
        eprintln!(
            "bounded Root-keyed write: N={} history_rows={} elapsed_us={}",
            g.evidence(),
            stats.history_rows,
            elapsed.as_micros()
        );
    }
}

/// Invalid writes on a Root with history, each caught by a check the
/// bounded validation keeps (the oracle requires the same verdict from the
/// full group audit); nothing they wrote remains.
#[tokio::test(flavor = "multi_thread")]
async fn checks_coupling_written_rows_to_history_still_reject() {
    let g = GrownV1::to(20);
    let root = g.root_text();
    let action = g.latest_action();
    let cases: Vec<(
        &str,
        &str,
        Box<dyn Fn(&rusqlite::Connection) -> rusqlite::Result<()>>,
    )> = vec![
        (
            "prepare_write",
            "Budget reservation/dispatch mismatch",
            Box::new({
                let root = root.clone();
                move |tx| {
                    tx.execute("UPDATE physical_control_budgets SET reserved_us=reserved_us+1,revision=revision+1 WHERE root_id=?1", [&root])?;
                    Ok(())
                }
            }),
        ),
        (
            "close_attempt",
            "Closed Root has live session",
            Box::new({
                let root = root.clone();
                move |tx| {
                    tx.execute("UPDATE physical_attempts SET state='closed',revision=revision+1,close_reason='revoked' WHERE root_id=?1", [&root])?;
                    Ok(())
                }
            }),
        ),
        (
            "commit_acceptance",
            "Terminal task has open control authority",
            Box::new({
                let (root, action) = (root.clone(), action.clone());
                move |tx| {
                    tx.execute("UPDATE physical_task_acceptance SET state='accepted',revision=2,action_id=?2,consequence_revision=(SELECT max(revision) FROM physical_consequences WHERE action_id=?2) WHERE root_id=?1", [&root, &action])?;
                    Ok(())
                }
            }),
        ),
        (
            "record_refusal",
            "",
            Box::new({
                let root = root.clone();
                move |tx| {
                    tx.execute("INSERT INTO physical_decisions VALUES(?1,99,'mcp:brain','forward',500000,'refused','gap',NULL,1)", [&root])?;
                    Ok(())
                }
            }),
        ),
        (
            "reconcile",
            "",
            Box::new({
                let action = action.clone();
                move |tx| {
                    tx.execute("INSERT INTO physical_reconciliations VALUES(?1,1,(SELECT max(revision) FROM physical_consequences WHERE action_id=?1),'resolved',printf('%064d',0),'{}')", [&action])?;
                    Ok(())
                }
            }),
        ),
        (
            "reconcile",
            "",
            Box::new({
                let action = action.clone();
                move |tx| {
                    tx.execute("INSERT INTO physical_handovers SELECT session_id,action_id,(SELECT max(revision) FROM physical_evidence WHERE action_id=?1)+5,1,printf('%064d',0),printf('%064d',0) FROM physical_actions WHERE action_id=?1", [&action])?;
                    Ok(())
                }
            }),
        ),
    ];
    for (kind, message, write) in cases {
        let (decisions, heads) = (g.decisions(), g.heads());
        paths();
        let result = g.store(|s| s.test_write_as(kind, |tx| write(tx)));
        let err = result.expect_err(kind);
        assert!(err.message().contains(message), "{kind}: {}", err.message());
        assert_eq!(paths(), vec![(kind, "bounded")], "{kind}");
        assert_eq!((g.decisions(), g.heads()), (decisions, heads), "{kind}");
    }
    // A valid write of the same shape commits on the bounded path.
    let decisions = g.decisions();
    g.refusal().unwrap();
    assert_eq!(paths(), vec![("record_refusal", "bounded")]);
    assert_eq!(g.decisions(), decisions + 1);
}

/// A declared kind may write only its declared tables, and a kind that may
/// skip history may never write history (debug builds; the write aborts).
#[tokio::test(flavor = "multi_thread")]
async fn a_write_outside_its_declared_tables_aborts() {
    let g = GrownV1::to(5);
    let root = g.root_text();
    let action = g.latest_action();
    let evidence = g.evidence();
    let undeclared = g.store(|s| {
        s.test_write_as("record_refusal", |tx| {
            tx.execute(
                "UPDATE physical_sessions SET revision=revision+1 WHERE root_id=?1",
                [&root],
            )?;
            Ok(())
        })
    });
    assert_eq!(
        undeclared.unwrap_err().message(),
        "Undeclared physical ledger write: physical_sessions in record_refusal"
    );
    let history = g.store(|s| {
        s.test_write_as("record_refusal", |tx| {
            tx.execute("INSERT INTO physical_evidence SELECT 'physical-observation:v1:'||lower(hex(randomblob(16))),action_id,kind,sequence+100000,revision+1,capture_us+1,receipt_us+1,source_digest,ordered,qualified,digest,record_json FROM physical_evidence WHERE action_id=?1 ORDER BY revision DESC LIMIT 1", [&action])?;
            Ok(())
        })
    });
    assert_eq!(
        history.unwrap_err().message(),
        "Undeclared physical ledger write: physical_evidence in record_refusal"
    );
    assert_eq!(g.evidence(), evidence);
}

/// A write that changes history without being a bounded append replays the
/// whole group, and is rejected if that history does not validate.
#[tokio::test(flavor = "multi_thread")]
async fn a_history_changing_write_replays_the_group() {
    let g = GrownV1::to(5);
    let (root, action) = (g.root_text(), g.latest_action());
    paths();
    let mixed = g.store(|s| {
        s.test_write_as("test_history_and_budget", |tx| {
            tx.execute("INSERT INTO physical_evidence SELECT 'physical-observation:v1:'||lower(hex(randomblob(16))),action_id,kind,sequence+100000,revision+1,capture_us+1,receipt_us+1,source_digest,ordered,qualified,digest,record_json FROM physical_evidence WHERE action_id=?1 ORDER BY revision DESC LIMIT 1", [&action])?;
            tx.execute("UPDATE physical_control_budgets SET revision=revision+1 WHERE root_id=?1", [&root])?;
            Ok(())
        })
    });
    assert!(mixed.is_err());
    assert_eq!(paths(), vec![("test_history_and_budget", "replayed")]);
}

/// The baseline: established by the full audit, advanced by validated
/// appends, re-learned by a replay when it does not match, dropped by
/// another connection's commit and re-established by the next full audit,
/// and established again by a restart's first transaction.
#[tokio::test(flavor = "multi_thread")]
async fn the_history_baseline_follows_trust() {
    let g = GrownV1::to(10);
    let (version, heads) = g.baseline().expect("trusted");
    assert_eq!(heads, g.heads(), "validated heads after appends");
    g.grow(15);
    assert_eq!(g.baseline().unwrap().1, g.heads(), "appends advance it");
    assert_eq!(
        g.baseline().unwrap().0,
        version,
        "own commits keep the version"
    );

    // A baseline that does not cover the history makes the write replay it,
    // and the replay re-learns the exact heads.
    let action = g.latest_action();
    let (e, q) = g.heads()[&action];
    g.store(|s| s.test_set_history_head(&g.root_text(), &action, (e - 1, q)));
    paths();
    g.refusal().unwrap();
    assert_eq!(paths(), vec![("record_refusal", "replayed")]);
    assert_eq!(g.baseline().unwrap().1, g.heads());

    // Another connection's commit: the next transaction audits in full and
    // re-establishes the baseline at the new version.
    g.sql()
        .execute_batch("CREATE TABLE IF NOT EXISTS bounded_validation_foreign(x); INSERT INTO bounded_validation_foreign VALUES(1);")
        .unwrap();
    paths();
    g.refusal().unwrap();
    let taken = paths();
    assert_eq!(taken.first(), Some(&("trust_audit", "full")), "{taken:?}");
    assert!(taken.contains(&("record_refusal", "bounded")), "{taken:?}");
    // The file changed, so the connection was reopened (its data version
    // starts afresh) and the old baseline was dropped with the trust.
    assert_eq!(g.baseline().unwrap().1, g.heads());

    // A restart: another process opens a copy and audits it in full first.
    let dir = std::env::temp_dir().join(format!("pastey-bounded-restart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&g.executor.paths.db_path, dir.join("db.sqlite")).unwrap();
    let restarted = crate::physical::store::PhysicalStoreV1::open(&AppPaths::new(
        dir.clone(),
        dir.join("logs"),
    ))
    .unwrap();
    assert!(paths().contains(&("trust_audit", "full")));
    let (_, roots) = restarted.test_history_baseline().expect("trusted");
    assert_eq!(roots[&g.root_text()], g.heads());
    drop(restarted);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Another connection forges, alters or exposes history: the next Root-keyed
/// write audits in full and is rejected; triggers refuse direct changes; a
/// changed trigger fails the schema pin at write start.
#[tokio::test(flavor = "multi_thread")]
async fn other_connections_cannot_slip_history_past_the_baseline() {
    // (a) A forged evidence row inserted after the baseline.
    let g = GrownV1::to(8);
    let decisions = g.decisions();
    g.sql()
        .execute("INSERT INTO physical_evidence SELECT 'physical-observation:v1:'||lower(hex(randomblob(16))),action_id,kind,sequence+100000,revision+1,capture_us+1,receipt_us+1,source_digest,ordered,qualified,digest,record_json FROM physical_evidence WHERE action_id=?1 ORDER BY revision DESC LIMIT 1", [g.latest_action()])
        .unwrap();
    paths();
    assert!(g.refusal().is_err());
    assert_eq!(paths(), vec![("trust_audit", "full")]);
    assert_eq!(g.decisions(), decisions);

    // (b) is in `the_history_baseline_follows_trust`. (c) Direct changes.
    let g = GrownV1::to(8);
    let c = g.sql();
    let action = g.latest_action();
    for (sql, refusal) in [
        ("UPDATE physical_evidence SET capture_us=capture_us+1,receipt_us=receipt_us+1 WHERE action_id=?1", "physical immutable evidence"),
        ("DELETE FROM physical_evidence WHERE action_id=?1", "physical evidence history required"),
        ("UPDATE physical_consequences SET state='verified' WHERE action_id=?1", "physical immutable consequence"),
        ("DELETE FROM physical_consequences WHERE action_id=?1", "physical consequence history required"),
    ] {
        let err = c.execute(sql, [&action]).unwrap_err();
        assert!(err.to_string().contains(refusal), "{sql}: {err}");
    }
    g.refusal().unwrap();

    // (d) Trigger dropped, history altered, trigger restored.
    let g = GrownV1::to(8);
    let c = g.sql();
    let trigger: String = c
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='physical_evidence_immutable'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.execute_batch("DROP TRIGGER physical_evidence_immutable")
        .unwrap();
    c.execute("UPDATE physical_evidence SET capture_us=capture_us+1,receipt_us=receipt_us+1 WHERE action_id=?1 AND revision=1", [g.latest_action()])
        .unwrap();
    c.execute_batch(&trigger).unwrap();
    paths();
    assert!(g.refusal().is_err());
    assert_eq!(paths(), vec![("trust_audit", "full")]);

    // Trigger pins: a missing or changed trigger fails at write start.
    for change in [
        "DROP TRIGGER physical_evidence_immutable",
        "DROP TRIGGER physical_evidence_immutable; CREATE TRIGGER physical_evidence_immutable BEFORE UPDATE ON physical_evidence WHEN 0 BEGIN SELECT RAISE(ABORT,'physical immutable evidence');END;",
    ] {
        let g = GrownV1::to(3);
        g.sql().execute_batch(change).unwrap();
        paths();
        let err = g.refusal().unwrap_err();
        assert!(err.message().contains("schema"), "{change}: {}", err.message());
        assert!(paths().is_empty(), "{change}: no validation may run");
    }
}

/// Validation time is not lost from an action's lifetime: with the clock
/// following real time and a Root that has collected the E2E's history
/// (about 820 rows; `PASTEY_TEST_DEADLINE_HISTORY`), a decision is admitted,
/// written within its deadline and recorded on time. Before bounded
/// validation its admission spent about 24 s validating and the action was
/// refused as "Action deadline passed".
#[tokio::test(flavor = "multi_thread")]
#[ignore = "grows 820 history rows (minutes in a debug build)"]
async fn an_admitted_action_keeps_its_lifetime_on_a_long_history() {
    let n = std::env::var("PASTEY_TEST_DEADLINE_HISTORY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(820);
    let g = GrownV1::to(n);
    // Timed: the oracle would replay the history it checks the skip against.
    g.store(|s| s.test_without_oracle());
    g.executor.clock.follow_real_time();
    let started = std::time::Instant::now();
    let decided = g.demo.call(
        "brain",
        &DecisionCallV1 {
            option: "forward".into(),
            duration_ms: 500,
        },
    );
    let elapsed = started.elapsed();
    assert_eq!(
        decided,
        ToolResultV1::Allowed {
            disposition: "accepted".into()
        },
        "after {elapsed:?}"
    );
    // Accepted is judged at the tick the write returned: the action reached
    // the binding and returned within its lifetime.
    let (disposition, applied): (String, Option<String>) = g
        .sql()
        .query_row(
            "SELECT disposition,apply_result FROM physical_actions WHERE action_id=?1",
            [g.latest_action()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    eprintln!("decide at N={}: {elapsed:?}", g.evidence());
    assert_eq!(
        (disposition.as_str(), applied.as_deref()),
        ("accepted", Some("accepted"))
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "decide took {elapsed:?}"
    );
}
