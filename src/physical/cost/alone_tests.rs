//! Tests of the grant the planner costs spills by: a statement's running
//! alone, whatever runs beside it or the writes keep (RULES.md D1159).

use crate::db::{Database, Session};
use crate::params::Params;
use crate::physical::cost::spill::grant_bytes;

/// \[DESIGN\] The bytes MEMORY.WORK_BYTES is set to: past the floor, so a
/// grant of it is shared, not given whole.
const WORK: usize = 16 << 20;

/// Schema H: T of 60,000 rows of a key, a group and a string of 98
/// characters, its statistics gathered, and a session of it whose
/// operators are granted up to `WORK`.
fn grouped() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(crate::threads::JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, G INTEGER NOT NULL, S VARCHAR(100) NOT NULL)",
    )
    .unwrap();
    let s = db.session("H");
    let pad = "x".repeat(90);
    for chunk in (0..60_000_usize).collect::<Vec<_>>().chunks(1_000) {
        let rows = chunk.iter().map(|i| format!("({i}, {}, '{pad}{i:08}')", i % 997)).collect::<Vec<_>>().join(", ");
        s.run(&format!("INSERT INTO T VALUES {rows}")).unwrap();
    }
    s.commit().unwrap();
    db.analyze("H.T").unwrap();
    s.run(&format!("SET MEMORY.WORK_BYTES = {WORK}")).unwrap();
    (db, s)
}

/// Verifies: [DESIGN] RULES.md D1159, D651, D683: a grouping whose rows
/// fit a statement's grant alone but not the floor is planned the same,
/// and costed at the same grant, alone, while two other statements hold
/// their grants, and while a standing reservation that grows with the
/// writes, as the kept undo does, holds most of its bound; the grant the
/// executor takes still shrinks beside the others. Regression test
/// (R-TEST-7): the grant was half of what was free as the statement was
/// planned, and the grouping beside two grants was planned streamed over
/// the key's index in order, alone hashed over a scan.
#[test]
fn a_plan_is_costed_at_the_grant_of_a_statement_alone() {
    let (db, s) = grouped();
    let sql = "SELECT K, S, COUNT(*) FROM T GROUP BY K, S";
    let st = db.snapshot().unwrap();
    // As the kept undo is: standing, empty, up to twice `WORK`.
    let (standing, first) = (st.memory.standing(), st.memory.planned());
    let mut kept = st.memory.reserve(0, "a growing standing reservation").unwrap();
    kept.stand_up_to(2 * WORK);
    assert_eq!((st.memory.standing(), st.memory.planned()), (standing, first + 2 * WORK), "planned at its bound");
    // Four times `WORK` past the standing reservations as planned: a
    // statement alone granted `WORK`, one beside two others' the floor.
    st.memory.set_limit(st.memory.planned() + 4 * WORK);
    let planned = || (grant_bytes(&db.snapshot().unwrap()), s.physical_plan(sql, &Params::new()).unwrap());
    let alone = planned();
    assert!((alone.0 - crate::storage::cost_of(WORK)).abs() < 0.5_f64, "{}", alone.0);
    let held = [st.work.grant(WORK), st.work.grant(WORK)];
    assert!(st.work.grant(WORK).bytes() < WORK, "the executor's grant shrinks beside the others");
    assert_eq!(planned(), alone, "beside two grants");
    drop(held);
    let before = st.memory.planned();
    kept.resize(2 * WORK - 1).unwrap();
    assert_eq!(st.memory.planned(), before, "the same, grown");
    assert_eq!(planned(), alone, "a standing reservation grown");
    drop(kept);
    assert_eq!(st.memory.planned(), before - 2 * WORK, "given back at its bound");
}
