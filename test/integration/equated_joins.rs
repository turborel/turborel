//! \[DESIGN\] Columns of two tables each equal to one value are equated
//! (RULES.md D35, D404): the join of the two may then probe an index of
//! one table by the other's rows, its key's first column held by the
//! value, as it may by any other equality of their columns.

use turborel::db::{Database, Session};
use turborel::optimizer::PREDICATE_TRANSITIVITY;
use turborel::params::Params;
use turborel::physical::plannings;
use turborel::value::{DataType, Spelled};

use crate::common::{number, set_rule, sorted};

/// L, lines of orders: its key W, D, O, N, and the item I each names; S,
/// stock: its key W, I, and a quantity Q. Two warehouses of 2000 items,
/// 10 districts of 30 orders of 5 lines.
fn stock() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE L (W INTEGER NOT NULL, D INTEGER NOT NULL, O INTEGER NOT NULL, N INTEGER NOT NULL, I INTEGER,
             PRIMARY KEY (W, D, O, N))
           CREATE TABLE S (W INTEGER NOT NULL, I INTEGER NOT NULL, Q INTEGER, PRIMARY KEY (W, I))",
    )
    .unwrap();
    let a = db.session("A");
    for w in 1..=2_i64 {
        let items: Vec<String> = (1..=2000_i64).map(|i| format!("({w}, {i}, {})", (i * 7 + w) % 30)).collect();
        a.run(&format!("INSERT INTO S VALUES {}", items.join(", "))).unwrap();
        for d in 1..=10_i64 {
            let lines: Vec<String> =
                (0..150_i64).map(|k| format!("({w}, {d}, {}, {}, {})", k.div_euclid(5), k % 5, (k * 131 + d) % 2000 + 1)).collect();
            a.run(&format!("INSERT INTO L VALUES {}", lines.join(", "))).unwrap();
        }
    }
    a.commit().unwrap();
    (db, a)
}

/// The stock level of a district: how many items its recent lines name
/// whose quantity is below a threshold.
const LEVEL: &str = "SELECT COUNT(DISTINCT S.I) FROM L, S
    WHERE L.W = 1 AND L.D = 3 AND L.O < 25 AND L.O >= 5 AND S.W = 1 AND S.I = L.I AND S.Q < 15";

/// Verifies: [DESIGN] RULES.md D404, D761, two columns of two tables
/// equal to one number are equated, and the join probes S's key by each
/// line's item, the warehouse held by the number; without the rule the
/// join probes it still, the warehouse fixed by S's own filter and made a
/// column of the lines, and counts the same.
#[test]
fn columns_equal_to_one_number_join_by_an_index() {
    let (db, a) = stock();
    let plan = a.physical_plan(LEVEL, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop"), "{plan}");
    let counted = sorted(&a, LEVEL);
    set_rule(&db, PREDICATE_TRANSITIVITY, false);
    let plan = a.physical_plan(LEVEL, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop") && plan.contains("L.I, 1"), "{plan}");
    assert_eq!(sorted(&a, LEVEL), counted);
    assert_ne!(counted, vec!["0".to_owned()], "some items are low");
}

/// Verifies: [DESIGN] RULES.md D404, D761, columns each equal to one
/// parameter are equated as those equal to one number are; two different
/// numbers equate nothing, and the join probes S's key by the warehouse
/// its own filter fixes, made a column of the lines.
#[test]
fn columns_equal_to_one_parameter_join_by_an_index() {
    let (_db, a) = stock();
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let p = Params::new().declare("P", integer, number(1));
    let by_parameter = LEVEL.replace("L.W = 1", "L.W = P").replace("S.W = 1", "S.W = P");
    let plan = a.physical_plan(&by_parameter, &p).unwrap();
    assert!(plan.contains("IndexNestedLoop"), "{plan}");
    let apart = LEVEL.replace("S.W = 1", "S.W = 2");
    let plan = a.physical_plan(&apart, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop") && plan.contains("L.I, 2"), "{plan}");
    assert_eq!(sorted(&a, &apart), sorted(&a, &apart.replace("S.I = L.I", "S.I + 0 = L.I")));
}

/// Verifies: [DESIGN] RULES.md D1323, D404, D761, D45, a column compared
/// with a `?` and equal to a column of another table gives that column the
/// same comparison, as a constant does: the stock level with its warehouse
/// given once, by a parameter, fixes S's warehouse in the plan made with
/// its slots as in the one made with its values. So it learns its plan in
/// the five runs learning takes, each planning twice, and plans nothing
/// after; with the `?` not passed on, the two plans never agree and it
/// plans at every execution.
#[test]
fn a_column_equal_to_a_parameter_gives_its_equal_the_comparison() {
    let (_db, a) = stock();
    let level = "SELECT COUNT(DISTINCT S.I) FROM L, S
        WHERE L.W = ? AND L.D = ? AND L.O < 25 AND L.O >= 5 AND S.W = L.W AND S.I = L.I AND S.Q < ?";
    let p = a.prepare(level).unwrap();
    let runs: Vec<u64> = (0..30_i64)
        .map(|k| {
            let before = plannings();
            a.execute_prepared(&p, &[number(1), number(1 + k % 10), number(10 + k % 10)]).unwrap();
            plannings().wrapping_sub(before)
        })
        .collect();
    let learning = runs.iter().take_while(|&&n| n > 0).count();
    assert_eq!((learning, &runs[..learning]), (5, &[2; 5][..]), "{runs:?}");
    assert!(runs[learning..].iter().all(|&n| n == 0), "{runs:?}");
    let given = level.replace("L.W = ?", "L.W = 1").replace("L.D = ?", "L.D = 3").replace("S.Q < ?", "S.Q < 15");
    assert_eq!(sorted(&a, &given), sorted(&a, LEVEL));
}
