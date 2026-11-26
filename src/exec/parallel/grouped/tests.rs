//! Unit tests of how a part judges its HAVING condition (RULES.md
//! D1135).

use super::*;
use crate::db::{Database, Session};
use crate::exec::table::{Key, hash_values};
use crate::logical::LogicalPlan;

/// The grouping of `sql`, a query of one grouping and its HAVING
/// condition, as its part makes it, and the state it was planned in.
fn grouping_of(a: &Session, sql: &str) -> (Arc<crate::db::State>, Grouping) {
    let (st, optimized) = a.optimized(sql).unwrap();
    let LogicalPlan::Project { input, .. } = optimized else { panic!("a projection: {sql}") };
    let LogicalPlan::Filter { cond, input, .. } = *input else { panic!("a filter: {sql}") };
    let LogicalPlan::Aggregate { groups, aggs, .. } = *input else { panic!("a grouping: {sql}") };
    let spec = Spec { groups, collations: Vec::new(), aggs, seeds: Vec::new() };
    let seed = st.functions.seed().clone();
    (st, Grouping { spec, having: Some(cond), distinct: false, read: None, seed, outer: None })
}

/// Verifies: [DESIGN] (RULES.md D1135) a part judges its HAVING condition
/// of the set function's value alone where that is compared with a
/// constant, on either side, and of the group's row where it is not: the
/// condition is read of the plan, and the groups are given by
/// `finished_above`, which alone refuses a table of two set functions'
/// states.
#[test]
fn a_having_of_one_set_function_and_a_constant_is_judged_of_its_value() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, N INTEGER NOT NULL)").unwrap();
    let a = db.session("A");
    for (sql, alone) in [
        ("SELECT K, SUM(N) FROM T GROUP BY K HAVING SUM(N) > 3", true),
        ("SELECT K, SUM(N) FROM T GROUP BY K HAVING 3 <= SUM(N)", true),
        ("SELECT K, SUM(N) FROM T GROUP BY K HAVING SUM(N) > K", false),
        ("SELECT K, SUM(N) FROM T GROUP BY K HAVING SUM(N) > 3 AND SUM(N) < 9", false),
        ("SELECT K, SUM(N), COUNT(*) FROM T GROUP BY K HAVING SUM(N) > 3", false),
    ] {
        let (st, grouping) = grouping_of(&a, sql);
        let cond = grouping.having.clone().unwrap();
        assert_eq!(grouping.bounded(&cond).is_some(), alone, "{sql}: {cond:?}");
        if !alone {
            continue;
        }
        let call = grouping.spec.aggs.first().unwrap().clone();
        let key = [Value::Exact(crate::value::Decimal::new(1, 0))];
        let mut table = GroupTable::new(1, &[call.clone(), call], &grouping.seed, usize::MAX);
        table.found_or_made(hash_values(&grouping.seed, &key), Key::Values(&key), 0);
        let runner = Runner::new(&Db::of(&st), crate::txn::NO_TRANSACTION);
        assert!(table.finished_if(|_| Ok(true)).is_ok(), "{sql}: a table of two is finished by rows");
        assert!(grouping.kept_of(table, &runner).is_err(), "{sql}: judged of the value alone");
    }
}
