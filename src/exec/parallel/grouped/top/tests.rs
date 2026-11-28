//! Unit tests of a pair's first rows taken of one partition (RULES.md
//! D1281).

use super::super::grouping_of;
use super::*;
use crate::db::Database;
use crate::exec::table::{Key, hash_values};
use crate::physical::{Parallel, PhysicalPlanner};
use crate::txn::NO_TRANSACTION;

/// The whole number `v` as a value.
fn whole(v: i128) -> Value {
    Value::Exact(Decimal::new(v, 0))
}

/// Verifies: [DESIGN] (RULES.md D1281) a partition of a pair whose groups
/// of the first grouping are not in the order of their first rows gives
/// the first rows it gives of the same groups in order, each numbered by
/// the least of its first groups' numbers. Three groups of the second
/// tie on their counts, and the two whose first rows came first are
/// given, in that order. The second's table made of the groups out of
/// order would number each by the group met first, and give others.
#[test]
fn a_pairs_first_rows_of_groups_out_of_order_are_those_of_the_groups_in_order() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (G INTEGER NOT NULL, N INTEGER NOT NULL)").unwrap();
    let a = db.session("A");
    let (st, optimized) = a.optimized("SELECT G, COUNT(DISTINCT N) FROM T GROUP BY G").unwrap();
    let planned = PhysicalPlanner::new(&st).plan(&optimized);
    let pair = if let Plan::Project { input, .. } = planned { *input } else { planned };
    let pair = if let Plan::Parallel(part) = pair { part.input } else { pair };
    let part = Plan::Parallel(Box::new(Parallel { degree: 2, input: pair }));
    let run = Db::of(&st);
    let by_count = [SortKey { descending: true, ..SortKey::ascending(1) }];
    let (top, second, _) =
        Top::of((&by_count, Most { rows: 2, ties: false }), &part, (&run, NO_TRANSACTION)).expect("a grouping's first rows");
    let (outer, first) = Outer::of(second).expect("a pair of groupings");
    assert!(outer.in_tables() && outer.spec.groups == [0], "the second by G, made in a table");
    let (grouping, _) = grouping_of(first, (None, Some(outer)), st.functions.seed().clone()).unwrap();
    // Each group of the first, by G and N, with its first row's number.
    let firsts = [(1, 7, 10), (2, 7, 20), (2, 8, 30), (4, 7, 40), (1, 8, 50), (4, 8, 60), (3, 7, 70)];
    let table_of = |order: &[usize]| {
        let mut table = GroupTable::new(2, &grouping.spec.aggs, &grouping.seed, usize::MAX);
        for &at in order {
            let (g, n, number) = firsts[at];
            let key = [whole(g), whole(n)];
            table.found_or_made(hash_values(&grouping.seed, &key), Key::Values(&key), number);
        }
        table
    };
    let runner = Runner::new(&run, NO_TRANSACTION);
    // G, its count of N, and its first row's number.
    let expected = format!("{:?}", [[whole(1), whole(2), whole(10)], [whole(2), whole(2), whole(20)]]);
    let in_order = table_of(&[0, 1, 2, 3, 4, 5, 6]);
    assert!(in_order.in_order());
    assert_eq!(format!("{:?}", top.of_part(&grouping, in_order, &runner).unwrap()), expected, "of the second's table");
    let out_of_order = table_of(&[5, 2, 4, 6, 3, 1, 0]);
    assert!(!out_of_order.in_order());
    assert_eq!(format!("{:?}", top.of_part(&grouping, out_of_order, &runner).unwrap()), expected, "of the second's groups made of rows");
}
