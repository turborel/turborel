//! Unit tests of the plans a prepared statement keeps (RULES.md D45):
//! what is kept of a physical plan, and when, which no API shows.

use super::*;
use crate::db::Database;
use crate::logical::{Expr, Slot};
use crate::value::Decimal;

use super::learning::{LEARNED_RUNS, MOST_LEARNING_RUNS};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// What `p` keeps of its physical plan, written.
fn physical(p: &Prepared) -> String {
    format!("{:?}", read(&p.plans.kept.kept).unwrap().as_ref().map(|k| &k.physical))
}

fn spatial_table() -> (Database, Session) {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE P (ID INTEGER, G GEOMETRY(0))").unwrap();
    let s = db.session("A");
    for start in (0_i32..1200_i32).step_by(100) {
        let values: Vec<_> = (start..start.saturating_add(100)).map(|n| format!("({n}, ST_POINT({n}, {n}))")).collect();
        s.run(&format!("INSERT INTO P VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    s.run("CREATE INDEX P_G ON P (G) USING rtree").unwrap();
    (db, s)
}

/// Verifies: [DESIGN] D45, D85: preparing a geometry operand must not
/// prevent an indexed plan from learning. This regresses comparisons of
/// the prepared function's object identity with its original function.
/// Changed values, NULL, EMPTY and reference errors survive reuse.
#[test]
fn geometry_index_plans_learn_despite_scalar_preparation() {
    use crate::geometry::Geometry;
    let (_db, s) = spatial_table();
    let query = s.prepare("SELECT ID FROM P WHERE ST_INTERSECTS(G, ?) = 1").unwrap();
    for n in 0_i32..16_i32 {
        let value = Geometry::point(f64::from(n), f64::from(n), 0).unwrap().to_value();
        let mut cursor = s.open_prepared(&query, &[value]).unwrap();
        assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), n.to_string());
        assert!(cursor.fetch().unwrap().is_none());
    }
    let kept = physical(&query);
    assert!(kept.starts_with("Some(Fixed") && kept.contains("IndexScan"), "{kept}");
    for value in [Value::Null, Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value()] {
        assert!(s.open_prepared(&query, &[value]).unwrap().fetch().unwrap().is_none());
    }
    for text in ["POINT (1 1)", "POINT EMPTY"] {
        let value = Geometry::from_wkt(text, 4326).unwrap().to_value();
        let result = s.open_prepared(&query, &[value]).and_then(|mut cursor| cursor.fetch());
        assert_eq!(result.unwrap_err().code, crate::error::Code::Data);
    }
    let value = Geometry::point(2.0, 2.0, 0).unwrap().to_value();
    assert_eq!(s.open_prepared(&query, &[value]).unwrap().fetch().unwrap().unwrap()[0].to_string(), "2");
    assert_eq!(query.plans_made(), 1);
}

/// Verifies: [DESIGN] D53, D85: constant folding reaches a set
/// function's argument as it reaches a select list: a sum of distances to
/// a constant polygon is folded and prepared, and sums the ordinary
/// measures.
#[test]
fn set_function_arguments_are_folded_and_prepared() {
    use crate::geometry::Geometry;
    let (_db, s) = spatial_table();
    let wkt = "POLYGON ((100 0, 200 0, 200 100, 100 100, 100 0))";
    let sql = format!("SELECT SUM(ST_DISTANCE(G, ST_GEOMFROMTEXT('{wkt}', 0))) FROM P");
    let plan = s.explain(&sql, &Params::new()).unwrap();
    let rules = plan.lines().find(|line| line.starts_with("-- rules:")).unwrap_or_default();
    assert!(rules.contains(crate::optimizer::CONSTANT_FOLDING), "{plan}");
    let polygon = Geometry::from_wkt(wkt, 0).unwrap();
    let expected: f64 =
        (0_i32..1200_i32).map(|n| Geometry::point(f64::from(n), f64::from(n), 0).unwrap().distance(&polygon).unwrap().unwrap()).sum();
    let row = s.open_cursor(&sql).unwrap().fetch().unwrap().unwrap();
    let Value::Approx(sum) = row[0] else { panic!("not a double: {row:?}") };
    assert!((sum - expected).abs() <= expected * 1e-12_f64, "{sum} against {expected}");
}

/// Verifies: [DESIGN] D15, D45, D85: a nearest plan learns with one
/// distance parameter, fills its measure and filter on reuse, and keeps
/// NULL, EMPTY and SRID-changing arguments correct after specialization.
#[test]
fn nearest_geometry_plans_learn_and_fill_each_execution() {
    use crate::geometry::Geometry;
    let (_db, s) = spatial_table();
    let query = s.prepare("SELECT ID, ST_DISTANCE(G, ?) AS D FROM P WHERE ID >= ? ORDER BY D, ID FETCH FIRST 1 ROW ONLY").unwrap();
    for n in 0_i32..16_i32 {
        let value = Geometry::point(f64::from(n), f64::from(n), 0).unwrap().to_value();
        let minimum = n.saturating_add(1);
        let mut cursor = s.open_prepared(&query, &[value, int(i128::from(minimum))]).unwrap();
        assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), minimum.to_string());
        assert!(cursor.fetch().unwrap().is_none());
    }
    let kept = physical(&query);
    assert!(kept.starts_with("Some(Fixed") && kept.contains("Nearest("), "{kept}");
    for value in [Value::Null, Geometry::from_wkt("POINT EMPTY", 0).unwrap().to_value()] {
        let mut cursor = s.open_prepared(&query, &[value, int(10)]).unwrap();
        assert_eq!(cursor.fetch().unwrap().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>(), ["10", "NULL"]);
        assert!(cursor.fetch().unwrap().is_none());
    }
    for text in ["POINT (1 1)", "POINT EMPTY"] {
        let value = Geometry::from_wkt(text, 4326).unwrap().to_value();
        let result = s.open_prepared(&query, &[value, int(0)]).and_then(|mut cursor| cursor.fetch());
        assert_eq!(result.unwrap_err().code, crate::error::Code::Data);
    }
    let value = Geometry::point(2.0, 2.0, 0).unwrap().to_value();
    let row = s.open_prepared(&query, &[value, int(0)]).unwrap().fetch().unwrap().unwrap();
    assert_eq!(row.iter().map(ToString::to_string).collect::<Vec<_>>(), ["2", "0E0"]);
    assert_eq!(query.plans_made(), 1);
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

/// Verifies: [DESIGN] RULES.md D45, D613: a plan learned that reads a
/// table whole is learned again once the table gains more rows than its
/// statistics are gathered again for, the execution that finds it so
/// planned with its values.
#[test]
fn a_learned_plan_is_learned_again_once_its_table_grows() {
    let (_db, s) = table();
    let range = s.prepare("SELECT C FROM P WHERE C BETWEEN ? AND ?").unwrap();
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
/// whose object rows' paths follow its values learns (D422). Neither binds
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
    assert!(physical(&range).contains("Learning"), "{}", physical(&range));
    let sum = |sql: &str| s.open_cursor(sql).unwrap().fetch().unwrap().and_then(|r| r.into_iter().next()).map(|v| v.to_string());
    assert_eq!(sum("SELECT SUM(C) FROM P WHERE C >= 1000 AND A < 100"), Some("2033".to_owned()), "rows 11 and 22, row 0 set to 0 since");
    assert_eq!(sum("SELECT COUNT(*) FROM P"), Some("1198".to_owned()), "rows 44 and 55 deleted");
    assert_eq!([update.plans_made(), delete.plans_made(), range.plans_made()], [1, 1, 1]);
}

/// Verifies: [DESIGN] D34, D85: a scan deciding a spatial conjunct of each
/// entry, chosen under a count or a projection reading no other column,
/// yields the rows a filter would: of an intersection with a triangle and
/// of a distance within a constant of it, over points, counted of no row
/// read where every reader sees the rows; a projection reading the column
/// or another conjunct reading it reads each row; a row another session
/// has not committed is not counted, though that session counts it; nor
/// is a row the session deleted, until it takes that back.
#[test]
fn scans_decide_spatial_conjuncts_of_their_entries() {
    use crate::geometry::Geometry;
    let (db, s) = spatial_table();
    db.maintain().unwrap();
    let wkt = "POLYGON ((9.5 9.5, 20.5 9.5, 9.5 20.5, 9.5 9.5))";
    let triangle = Geometry::from_wkt(wkt, 0).unwrap().to_value();
    let count = |session: &Session, sql: &str| {
        let query = session.prepare(sql).unwrap();
        session.open_prepared(&query, std::slice::from_ref(&triangle)).unwrap().fetch().unwrap().unwrap()[0].to_string()
    };
    // The points (n, n) on or inside the triangle have n from 10 to 15;
    // within 1 of it, (9, 9) too, 0.71 away; (16, 16) is 1.41 away.
    for (sql, expected) in [
        ("SELECT COUNT(*) FROM P WHERE ST_INTERSECTS(G, ?) = 1", "6"),
        ("SELECT COUNT(*) FROM P WHERE 1 = ST_INTERSECTS(?, G)", "6"),
        ("SELECT COUNT(*) FROM P WHERE ST_DISTANCE(G, ?) <= 1.0", "7"),
        ("SELECT COUNT(*) FROM P WHERE ST_INTERSECTS(G, ?) = 1 AND G IS NOT NULL", "6"),
        ("SELECT SUM(ST_X(G)) FROM P WHERE ST_INTERSECTS(G, ?) = 1", "7.5E1"),
        ("SELECT SUM(ID) FROM P WHERE ST_INTERSECTS(G, ?) = 1", "75"),
    ] {
        assert_eq!(count(&s, sql), expected, "{sql}");
        let literal = sql.replace('?', &format!("ST_GEOMFROMTEXT('{wkt}', 0)"));
        let plan = s.explain(&literal, &Params::new()).unwrap();
        assert!(plan.contains("IndexScan") && plan.contains(", decided of each entry"), "{sql}: {plan}");
    }
    let query = s.prepare("SELECT ST_X(G) FROM P WHERE ST_INTERSECTS(G, ?) = 1").unwrap();
    let mut cursor = s.open_prepared(&query, std::slice::from_ref(&triangle)).unwrap();
    let mut xs = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        xs.push(row[0].to_string());
    }
    xs.sort_unstable();
    assert_eq!(xs, ["1.1E1", "1.2E1", "1.3E1", "1.4E1", "1.5E1", "1E1"]);
    let other = db.session("A");
    other.run("INSERT INTO P VALUES (2000, ST_POINT(12, 12))").unwrap();
    let sql = "SELECT COUNT(*) FROM P WHERE ST_INTERSECTS(G, ?) = 1";
    assert_eq!(count(&s, sql), "6", "another session's uncommitted row");
    assert_eq!(count(&other, sql), "7", "the session's own row");
    other.rollback().unwrap();
    s.run("DELETE FROM P WHERE ID = 12").unwrap();
    assert_eq!(count(&s, sql), "5", "the session's own deletion");
    s.rollback().unwrap();
    assert_eq!(count(&s, sql), "6");
}

/// ORDER_LINE and STOCK of one warehouse, as TPC-C makes them: 100,000
/// items in stock and a district of 5,000 orders of ten lines each, their
/// statistics gathered.
fn order_lines() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION T \
         CREATE TABLE ORDER_LINE (OL_W_ID INTEGER NOT NULL, OL_D_ID SMALLINT NOT NULL, OL_O_ID INTEGER NOT NULL, OL_NUMBER SMALLINT NOT NULL, OL_I_ID INTEGER, CONSTRAINT ORDER_LINE_I1 PRIMARY KEY (OL_W_ID, OL_D_ID, OL_O_ID, OL_NUMBER)) \
         CREATE TABLE STOCK (S_W_ID INTEGER NOT NULL, S_I_ID INTEGER NOT NULL, S_QUANTITY SMALLINT, CONSTRAINT STOCK_I1 PRIMARY KEY (S_W_ID, S_I_ID)) \
         CREATE TABLE DIGIT (D INTEGER NOT NULL) CREATE TABLE N (N INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("T");
    for sql in [
        "INSERT INTO DIGIT VALUES (0), (1), (2), (3), (4), (5), (6), (7), (8), (9)",
        "INSERT INTO N SELECT A.D * 10000 + B.D * 1000 + C.D * 100 + E.D * 10 + F.D + 1 FROM DIGIT A, DIGIT B, DIGIT C, DIGIT E, DIGIT F",
        "INSERT INTO STOCK SELECT 1, N, MOD(N, 90) + 10 FROM N",
        "INSERT INTO ORDER_LINE SELECT 1, 1, N, L.D + 1, MOD(N * 7919 + L.D * 104729, 100000) + 1 FROM N, DIGIT AS L WHERE N <= 5000",
    ] {
        s.run(sql).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// Verifies: [DESIGN] RULES.md D760: TPC-C's STOCK-LEVEL query as its
/// procedure runs it, its warehouse one slot, its order a slot bounding
/// the order's lines by it and by it less 20, is planned with its slots,
/// to be kept, as an index nested-loop join probing STOCK's key for the
/// twenty orders' lines, not a hash join reading STOCK whole, the join
/// costed as in the plan made with its values: the slot less 20 is read
/// as its value less 20, not as an unknown bound.
#[test]
fn the_stock_level_query_with_slots_probes_stock() {
    let (db, s) = order_lines();
    let p = s
        .prepare(
            "SELECT COUNT(DISTINCT S_I_ID) FROM ORDER_LINE, STOCK WHERE OL_W_ID = ? AND OL_D_ID = ? \
             AND OL_O_ID < ? AND OL_O_ID >= ? - CAST(20 AS INTEGER) AND S_W_ID = ? AND S_I_ID = OL_I_ID AND S_QUANTITY < ?",
        )
        .unwrap();
    drop(s.open_prepared(&p, &[int(1), int(1), int(5001), int(5001), int(1), int(15)]).unwrap());
    let Some(KeptPlan::Query(_, bound)) = read(&p.plans.kept.kept).unwrap().as_ref().and_then(|k| k.plan.clone()) else {
        panic!("a query's plan")
    };
    // The procedure's warehouse is one variable, of OL_W_ID and S_W_ID both.
    let slotted = crate::logical::map_plan_exprs(bound, &mut |e| {
        let Expr::Dynamic(slot) = e else { return e };
        Expr::Dynamic(Slot { index: if slot.index == 4 { 0 } else { slot.index }, ..slot })
    });
    let values = [int(1), int(1), int(5001), int(5001), int(1), int(15)];
    let st = db.snapshot().unwrap();
    let made = super::super::super::compiled::plan(&st, filled(slotted.clone(), &values).unwrap()).2.explain();
    let plan = super::super::super::compiled::plan_slotted(&st, slotted, &values).explain();
    assert!(plan.contains("IndexNestedLoop T.STOCK") && !plan.contains("HashJoin"), "{plan}");
    let join = |text: &str| text.lines().find(|l| l.contains("IndexNestedLoop")).map(str::to_owned);
    assert_eq!(join(&plan), join(&made), "its join costed as with its values");
}
