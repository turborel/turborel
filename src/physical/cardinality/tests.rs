//! Unit tests of the estimator: each node's rule on a database of known
//! statistics, lineage, Cardenas' formula at its boundaries, classes
//! giving one answer for every order of a set, and guarded operands.

use std::sync::Arc;

use super::groups::cardenas;
use super::joins::Classes;
use super::lineage::{Over, one_table, origin};
use super::{Estimates, MAX_ESTIMATE_DEPTH, UNKNOWN_ROWS, UNNEST_FANOUT};
use crate::catalog::QualifiedName;
use crate::db::{Database, Session, State};
use crate::logical::{Cond, Expr, LogicalPlan, Truth};
use crate::value::{CompOp, Decimal, Value};

/// Schema E: T of 1,000 rows, K a key, G of 10 values; U of 100 rows, K a
/// key; V of 10 rows; statistics gathered.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION E
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER NOT NULL)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE V (K INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("E");
    let rows = |n: i32, f: &dyn Fn(i32) -> String| (0_i32..n).map(f).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO T VALUES {}", rows(1_000_i32, &|k| format!("({k}, {})", k % 10_i32)))).unwrap();
    s.run(&format!("INSERT INTO U VALUES {}", rows(100_i32, &|k| format!("({k}, {k})")))).unwrap();
    s.run(&format!("INSERT INTO V VALUES {}", rows(10_i32, &|k| format!("({k})")))).unwrap();
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// The estimated rows of `sql`'s optimized plan.
fn rows(s: &Session, sql: &str) -> f64 {
    let (st, plan) = s.optimized(sql).unwrap();
    Estimates::new(&st).rows(&plan)
}

/// Whether `estimate` is within twice or half of `actual`.
fn near(estimate: f64, actual: f64) -> bool {
    estimate <= 2.0_f64 * actual && estimate >= actual / 2.0_f64
}

/// Verifies: [DESIGN] RULES.md D370: a scan is its table's rows, a
/// filter its input's kept by the table's statistics, a filter over a
/// join by the base tables under its columns, and a list of rows how many.
#[test]
fn scans_filters_and_joins_are_estimated_by_their_tables() {
    let (_db, s) = database();
    assert!(near(rows(&s, "SELECT * FROM T"), 1000.0), "a scan");
    assert!(near(rows(&s, "SELECT * FROM T WHERE G = 3"), 100.0), "a filter by the histogram");
    assert!(near(rows(&s, "SELECT * FROM T, U WHERE T.K = U.K"), 100.0), "a join by its key");
    assert!(near(rows(&s, "SELECT * FROM T, U WHERE T.K = U.K AND T.G = 3"), 10.0), "a filter above a join");
    assert!(near(rows(&s, "SELECT * FROM T, V"), 10_000.0), "a product");
    assert!(near(rows(&s, "VALUES (1), (2), (3)"), 3.0), "a list of rows");
}

/// Verifies: [DESIGN] RULES.md D370: a grouping's rows are its groups, one
/// of no grouping column; DISTINCT's the groups of its columns; and a
/// derived table of a grouping joined is estimated through it.
#[test]
fn groupings_are_estimated_by_their_groups() {
    let (_db, s) = database();
    assert!(near(rows(&s, "SELECT G, COUNT(*) FROM T GROUP BY G"), 10.0));
    assert!(near(rows(&s, "SELECT COUNT(*) FROM T"), 1.0));
    assert!(near(rows(&s, "SELECT COUNT(*) FROM T WHERE K < 0"), 1.0), "one row of no rows");
    assert!(near(rows(&s, "SELECT DISTINCT G FROM T"), 10.0));
    let keyed = rows(&s, "SELECT K, G FROM T GROUP BY K, G");
    assert!((keyed - 1000.0_f64).abs() < 1.0_f64, "every row of the table is a group of its key: {keyed}");
    let joined = "SELECT * FROM U, (SELECT G, COUNT(*) AS N FROM T GROUP BY G) AS D WHERE U.K = D.G";
    assert!(near(rows(&s, joined), 10.0), "{}", rows(&s, joined));
}

/// Verifies: [DESIGN] RULES.md D370: UNION ALL is the sum, UNION the
/// groups of the sum, INTERSECT at most the smaller, EXCEPT at most the
/// left; a limit its count, past its offset.
#[test]
fn set_operations_and_limits_are_estimated() {
    let (_db, s) = database();
    assert!(near(rows(&s, "SELECT K FROM U UNION ALL SELECT K FROM V"), 110.0));
    assert!(near(rows(&s, "SELECT K FROM U UNION SELECT K FROM V"), 110.0));
    assert!(rows(&s, "SELECT K FROM U INTERSECT SELECT K FROM V") <= 10.0_f64);
    assert!(rows(&s, "SELECT K FROM V EXCEPT SELECT K FROM U") <= 10.0_f64);
    assert!(near(rows(&s, "SELECT G FROM T UNION SELECT G FROM T"), 20.0), "distinct values of either side, summed");
    assert!(near(rows(&s, "SELECT K FROM T FETCH FIRST 5 ROWS ONLY"), 5.0));
    assert!(near(rows(&s, "SELECT K FROM T OFFSET 998 ROWS"), 2.0));
    assert!(near(rows(&s, "SELECT K FROM T OFFSET 2 ROWS FETCH FIRST 5 ROWS ONLY"), 5.0));
}

/// Verifies: [DESIGN] RULES.md D370: an outer join keeps at least each
/// preserved operand's rows, of no match too.
#[test]
fn outer_joins_keep_their_preserved_rows() {
    let (_db, s) = database();
    assert!(rows(&s, "SELECT * FROM T LEFT JOIN U ON T.K = U.K + 5000") >= 1_000.0_f64);
    assert!(rows(&s, "SELECT * FROM V RIGHT JOIN U ON V.K = U.K") >= 100.0_f64);
    assert!(rows(&s, "SELECT * FROM V FULL JOIN U ON V.K = U.W + 1000") >= 100.0_f64);
    assert!(near(rows(&s, "SELECT * FROM U LEFT JOIN V ON U.K = V.K"), 100.0));
}

/// Verifies: [DESIGN] RULES.md D370: Cardenas' formula at its boundaries:
/// no rows no groups, one row one, one value one group, and many more rows
/// than values every value.
#[test]
fn cardenas_at_its_boundaries() {
    assert!(cardenas(100.0, 0.0).abs() < f64::EPSILON);
    assert!((cardenas(100.0, 1.0) - 1.0).abs() < 1e-9_f64);
    assert!((cardenas(1.0, 1_000.0) - 1.0).abs() < 1e-9_f64);
    assert!((cardenas(10.0, 1_000_000.0) - 10.0).abs() < 1e-6_f64);
    assert!(cardenas(1e9, 1_000.0) <= 1_000.0_f64 && cardenas(1e9, 1_000.0) > 999.0_f64);
    assert!((cardenas(1_000.0_f64, 1_000.0_f64) - 632.3_f64).abs() < 1.0_f64, "1 - 1/e of the values");
    assert!(cardenas(0.0, 5.0) >= 1.0_f64, "a count of no values is one");
    assert!(cardenas(10.0, f64::NAN).abs() < f64::EPSILON);
}

/// A generator of fixed seed (R-TEST-5): xorshift.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self, below: u64) -> u64 {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        self.0.checked_rem(below).unwrap_or(0)
    }
}

/// Verifies: [DESIGN] RULES.md D302, D370: the classes give a set of
/// relations the same rows whatever order they are joined in, each join
/// keeping the classes' fraction of the union over its operands'.
#[test]
fn classes_give_one_answer_for_every_order() {
    let mut random = Seeded(0x5EED_CAFE);
    for _ in 0_i32..50_i32 {
        let n = 2 + usize::try_from(random.next(4)).unwrap();
        let rows: Vec<f64> = (0..n).map(|_| f64::from(u32::try_from(1 + random.next(1000)).unwrap())).collect();
        let mut classes: Vec<Vec<(usize, f64)>> = Vec::new();
        for _ in 0..=random.next(3) {
            let mut members = Vec::new();
            for r in 0..n {
                if random.next(2) == 0 {
                    members.push((r, f64::from(u32::try_from(1 + random.next(500)).unwrap())));
                }
            }
            classes.push(members);
        }
        let classes = Classes::new(classes);
        let fraction = |set: &[usize]| classes.fraction(|r| set.contains(&r));
        let joined = |order: &[usize]| {
            let mut set = vec![order[0]];
            let mut got = rows[order[0]];
            for &r in &order[1..] {
                let union: Vec<usize> = set.iter().copied().chain([r]).collect();
                got = got * rows[r] * fraction(&union) / (fraction(&set) * fraction(&[r]));
                set = union;
            }
            got
        };
        let forward: Vec<usize> = (0..n).collect();
        let backward: Vec<usize> = (0..n).rev().collect();
        let all = rows.iter().product::<f64>() * fraction(&forward);
        assert!((joined(&forward) / all - 1.0).abs() < 1e-9_f64 && (joined(&backward) / all - 1.0).abs() < 1e-9_f64);
    }
}

/// A scan of `table` of schema E, of `width` columns.
fn scan(table: &str, width: usize) -> LogicalPlan {
    LogicalPlan::Scan {
        table: QualifiedName { auth: "E".into(), ident: table.into() },
        names: (0..width).map(|c| format!("C{c}")).collect(),
        needed: (0..width).collect(),
        located: false,
    }
}

/// "column `c` = `v`".
fn equal(c: usize, v: i128) -> Cond {
    Cond::Compare(Expr::Column(c), CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(v, 0))))
}

/// A guard of no columns, of the shape `optimizer::raising` makes, or of
/// `width` columns.
fn guard(width: usize) -> LogicalPlan {
    let values = LogicalPlan::Values { rows: vec![vec![Expr::Const(Value::Null); width]], types: vec![crate::value::DataType::Boolean; width] };
    LogicalPlan::Filter { cond: Cond::Const(Truth::True), input: Box::new(values) }
}

/// The state of schema E.
fn state(s: &Session) -> Arc<State> {
    s.optimized("VALUES (1)").unwrap().0
}

/// Verifies: [DESIGN] RULES.md D370, MC/DC of the guard's decision
/// (`filter_at`): a filter over a product whose right operand has no
/// columns is the guarded operand, its own rows kept by its conjuncts,
/// the guard one row (pair: product and no columns, against product of a
/// column); a filter over a product of a column is a join; one over no
/// product its input's (pair: no product).
#[test]
fn a_guarded_operand_is_estimated_as_its_operand() {
    let (_db, s) = database();
    let st = state(&s);
    let est = Estimates::new(&st);
    let guarded = |g: LogicalPlan| LogicalPlan::Filter { cond: equal(1, 3), input: Box::new(LogicalPlan::Product { left: Box::new(scan("T", 2)), right: Box::new(g) }) };
    assert!(near(est.rows(&guarded(guard(0))), 100.0), "{}", est.rows(&guarded(guard(0))));
    // A guard of a column is a relation of one row joined, its filter a third.
    assert!(est.rows(&guarded(guard(1))) < 100.0_f64);
    let plain = LogicalPlan::Filter { cond: equal(1, 3), input: Box::new(scan("T", 2)) };
    assert!(near(est.rows(&plain), 100.0));
}

/// Verifies: [DESIGN] RULES.md D370: lineage passes through filters,
/// sorts, limits, DISTINCT, projections of a column, products, outer joins
/// by their offsets, a window's input columns and a grouping's grouping
/// columns; a projected expression and a set operation's column have none;
/// two scans of one table are two relations; and a null-supplying side's
/// column keeps its table's distinct values.
#[test]
fn lineage_through_each_node() {
    let (_db, s) = database();
    let (_, grouped) = s.optimized("SELECT G, COUNT(*) FROM T GROUP BY G").unwrap();
    assert_eq!(origin(&grouped, 0).map(|o| o.column), Some(1));
    assert!(origin(&grouped, 1).is_none(), "a set function's column has no table");
    let (_, joined) = s.optimized("SELECT U.W, T.G FROM T LEFT JOIN U ON T.K = U.K ORDER BY 2 FETCH FIRST 3 ROWS ONLY").unwrap();
    assert_eq!(origin(&joined, 0).map(|o| (o.table.ident.as_str(), o.column)), Some(("U", 1)));
    assert_eq!(origin(&joined, 1).map(|o| (o.table.ident.as_str(), o.column)), Some(("T", 1)));
    let (st, windowed) = s.optimized("SELECT DISTINCT K, ROW_NUMBER() OVER (ORDER BY K) FROM V").unwrap();
    assert_eq!(origin(&windowed, 0).map(|o| o.column), Some(0));
    assert!(origin(&windowed, 1).is_none());
    let (_, expression) = s.optimized("SELECT K + 1 FROM V UNION SELECT K FROM V").unwrap();
    assert!(origin(&expression, 0).is_none());
    let est = Estimates::new(&st);
    assert!(near(est.distinct(&joined, 0), 100.0), "the null-supplying side's distinct values");
    let product = LogicalPlan::Product { left: Box::new(scan("V", 1)), right: Box::new(scan("V", 1)) };
    assert!(one_table(Over::One(&product), &[0, 1]).is_none(), "a table joined with itself is two relations");
    assert!(one_table(Over::One(&product), &[1]).is_some());
    assert!(one_table(Over::Pair(&scan("V", 1), &scan("U", 2)), &[2]).is_some_and(|(t, places)| t.ident == "U" && places == [(2, 1)]));
}

/// Verifies: [DESIGN] RULES.md D35, D370: what has no count takes a
/// default, an UNNEST its input's times the fan-out, and a plan deeper than
/// the estimator looks a relation of unknown rows.
#[test]
fn defaults_of_what_has_no_count() {
    let (_db, s) = database();
    let st = state(&s);
    let est = Estimates::new(&st);
    let missing = LogicalPlan::Scan { table: QualifiedName { auth: "E".into(), ident: "NONE".into() }, names: vec![], needed: vec![], located: false };
    assert!((est.rows(&missing) - UNKNOWN_ROWS).abs() < f64::EPSILON);
    let unnested = rows(&s, "SELECT * FROM V, UNNEST(ARRAY[1, 2]) AS A(X)");
    assert!(near(unnested, 10.0 * UNNEST_FANOUT), "{unnested}");
    let below = est.card_at(&scan("V", 1), MAX_ESTIMATE_DEPTH.saturating_add(1));
    assert!((below.rows - UNKNOWN_ROWS).abs() < f64::EPSILON, "past the depth looked to");
    assert!(est.rows(&LogicalPlan::Filter { cond: Cond::Const(Truth::False), input: Box::new(scan("V", 1)) }) >= 1.0_f64, "at least a row");
    let distinct = super::super::PhysicalPlanner::new(&st).card(&LogicalPlan::Distinct { input: Box::new(scan("T", 2)) });
    assert!(near(distinct.rows, 1000.0) && distinct.upper >= distinct.rows);
}
