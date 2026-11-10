//! Unit tests of the plans a prepared statement keeps (RULES.md D45):
//! what is kept of a physical plan, and when, which no API shows.

use super::*;
use crate::db::Database;
use crate::value::Decimal;

use super::learning::{LEARNED_RUNS, MOST_LEARNING_RUNS};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// What `p` keeps of its physical plan, written.
fn physical(p: &Prepared) -> String {
    format!("{:?}", lock(&p.plans.kept).unwrap().as_ref().map(|k| &k.physical))
}

/// A database with the table `A.P` (A, B, C INTEGER) of 1,200 rows, A
/// from 0 to 119, ten rows of each, B from 0 to 9 in each, its primary key
/// (A, B); and a session of `A`.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER, PRIMARY KEY (A, B))")
        .unwrap();
    let s = db.session("A");
    for start in (0_i32..1200_i32).step_by(100) {
        let values: Vec<String> =
            (start..start.saturating_add(100_i32)).map(|i| format!("({}, {}, {i})", i.div_euclid(10), i.rem_euclid(10))).collect();
        s.run(&format!("INSERT INTO P VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    (db, s)
}

/// Verifies: [DESIGN] RULES.md D45: a prepared query's physical plan is
/// kept once an execution finds it one probe fixing a whole unique key,
/// after the execution that bound it; a range query's, made with its
/// slots, once `LEARNED_RUNS` executions after that one have planned it of
/// one shape with their values and with their slots, the bounds of its
/// probe slots; neither binds again.
#[test]
fn a_point_plan_is_kept_at_once_and_a_range_plan_once_learned() {
    let (_db, s) = table();
    let point = s.prepare("SELECT C FROM P WHERE A = ? AND B = ?").unwrap();
    let range = s.prepare("SELECT SUM(C) FROM P WHERE A BETWEEN ? AND ?").unwrap();
    drop(s.open_prepared(&point, &[int(1), int(2)]).unwrap());
    assert!(physical(&point).contains("Untried"), "{}", physical(&point));
    drop(s.open_prepared(&point, &[int(1), int(2)]).unwrap());
    assert!(physical(&point).starts_with("Some(Fixed"), "{}", physical(&point));
    drop(s.open_prepared(&range, &[int(3), int(5)]).unwrap());
    for n in 0..i128::from(LEARNED_RUNS) {
        assert!(!physical(&range).contains("Fixed"), "{n}: {}", physical(&range));
        // Away from 0, where a lower bound every row is above is no probe's.
        drop(s.open_prepared(&range, &[int(n + 3), int(n + 5)]).unwrap());
    }
    let kept = physical(&range);
    assert!(kept.starts_with("Some(Fixed") && kept.contains("place: Low") && kept.contains("place: High"), "{kept}");
    assert_eq!([point.plans_made(), range.plans_made()], [1, 1]);
}

/// Verifies: [DESIGN] RULES.md D45: a query whose values choose its paths
/// in turn, an index's probe for a few rows and a scan for all of them,
/// keeps no plan, and stops learning after `MOST_LEARNING_RUNS`
/// executions.
#[test]
fn a_query_whose_values_choose_its_paths_stops_learning() {
    let (_db, s) = table();
    let range = s.prepare("SELECT C FROM P WHERE A < ?").unwrap();
    drop(s.open_prepared(&range, &[int(1)]).unwrap());
    for n in 0..MOST_LEARNING_RUNS {
        assert!(physical(&range).contains(if n == 0 { "Untried" } else { "Learning" }), "{n}: {}", physical(&range));
        let most = if n % 2 == 0 { 1 } else { 1000 };
        drop(s.open_prepared(&range, &[int(most)]).unwrap());
    }
    assert!(physical(&range).contains("Varies"), "{}", physical(&range));
    assert_eq!(range.plans_made(), 1);
}

/// Verifies: [DESIGN] RULES.md D45: a plan learned is learned again once
/// a table it reads gains more rows than its statistics are gathered again
/// for, the execution that finds it so planned with its values.
#[test]
fn a_learned_plan_is_learned_again_once_its_table_grows() {
    let (_db, s) = table();
    let range = s.prepare("SELECT C FROM P WHERE A BETWEEN ? AND ?").unwrap();
    for n in 0..=i128::from(LEARNED_RUNS) {
        drop(s.open_prepared(&range, &[int(n + 3), int(n + 4)]).unwrap());
    }
    assert!(physical(&range).starts_with("Some(Fixed"), "{}", physical(&range));
    s.run("INSERT INTO P SELECT A + 200, B, C FROM P WHERE A < 18").unwrap();
    s.commit().unwrap();
    drop(s.open_prepared(&range, &[int(1), int(2)]).unwrap());
    assert!(physical(&range).contains("Learning { last: None, agreed: 0, runs: 0 }"), "{}", physical(&range));
}

/// Verifies: [DESIGN] RULES.md D45: a prepared UPDATE or DELETE whose
/// object rows are one probe fixing a whole unique key keeps that plan,
/// made with its slots, from the execution that bound it, and fills it at
/// each execution, writing the row its values name and no other; one
/// whose object rows' paths follow its values keeps none. Neither binds
/// again.
#[test]
fn a_point_write_keeps_its_plan_and_a_range_write_none() {
    let (_db, s) = table();
    let update = s.prepare("UPDATE P SET C = C + ? WHERE A = ? AND B = ?").unwrap();
    let delete = s.prepare("DELETE FROM P WHERE A = ? AND B = ?").unwrap();
    let range = s.prepare("UPDATE P SET C = 0 WHERE A < ?").unwrap();
    for n in 0_i128..3 {
        s.execute_prepared(&update, &[int(1000), int(n), int(n)]).unwrap();
        assert!(physical(&update).starts_with("Some(Fixed"), "{n}: {}", physical(&update));
    }
    s.execute_prepared(&delete, &[int(4), int(4)]).unwrap();
    s.execute_prepared(&delete, &[int(5), int(5)]).unwrap();
    assert!(physical(&delete).starts_with("Some(Fixed"), "{}", physical(&delete));
    s.execute_prepared(&range, &[int(1)]).unwrap();
    assert!(physical(&range).contains("Varies"), "{}", physical(&range));
    let sum = |sql: &str| s.open_cursor(sql).unwrap().fetch().unwrap().and_then(|r| r.into_iter().next()).map(|v| v.to_string());
    assert_eq!(sum("SELECT SUM(C) FROM P WHERE C >= 1000 AND A < 100"), Some("2033".to_owned()), "rows 11 and 22, row 0 set to 0 since");
    assert_eq!(sum("SELECT COUNT(*) FROM P"), Some("1198".to_owned()), "rows 44 and 55 deleted");
    assert_eq!([update.plans_made(), delete.plans_made(), range.plans_made()], [1, 1, 1]);
}
