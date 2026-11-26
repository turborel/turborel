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
/// key; V of 10 rows; W of 1,000 rows, A and B of 100 values, equal, and
/// an index of both; statistics gathered.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION E
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER NOT NULL)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE V (K INTEGER NOT NULL)
           CREATE TABLE W (A INTEGER NOT NULL, B INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("E");
    let rows = |n: i32, f: &dyn Fn(i32) -> String| (0_i32..n).map(f).collect::<Vec<_>>().join(", ");
    s.run(&format!("INSERT INTO T VALUES {}", rows(1_000_i32, &|k| format!("({k}, {})", k % 10_i32)))).unwrap();
    s.run(&format!("INSERT INTO U VALUES {}", rows(100_i32, &|k| format!("({k}, {k})")))).unwrap();
    s.run(&format!("INSERT INTO V VALUES {}", rows(10_i32, &|k| format!("({k})")))).unwrap();
    s.run(&format!("INSERT INTO W VALUES {}", rows(1_000_i32, &|k| format!("({}, {})", k % 100_i32, k % 100_i32)))).unwrap();
    s.commit().unwrap();
    s.run("CREATE INDEX W_AB ON W (A, B)").unwrap();
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
    let correlated = rows(&s, "SELECT A, B FROM W GROUP BY A, B");
    assert!(near(correlated, 100.0_f64), "the index's sketch counts correlated columns once: {correlated}");
    let joined = "SELECT * FROM U, (SELECT G, COUNT(*) AS N FROM T GROUP BY G) AS D WHERE U.K = D.G";
    assert!(near(rows(&s, joined), 10.0), "{}", rows(&s, joined));
}

/// Verifies: [DESIGN] RULES.md D1082: a grouping by a deterministic
/// expression has no more groups than the columns it reads have values
/// together: of one column its values, of two their combinations, of a
/// constant one, beside a column of a table its groups times that
/// column's; an expression over an expression is bounded through both.
#[test]
fn a_grouping_by_an_expression_is_bounded_by_its_columns() {
    let (_db, s) = database();
    let groups = |sql: &str| rows(&s, sql);
    assert!(groups("SELECT H, COUNT(*) FROM (SELECT G + 1 AS H FROM T) AS D GROUP BY H") <= 10.5_f64, "of one column of 10 values");
    assert!(groups("SELECT H, COUNT(*) FROM (SELECT G * 2 - K AS H FROM T) AS D GROUP BY H") > 500.0_f64, "K is a key");
    let pair = groups("SELECT H, COUNT(*) FROM (SELECT A + B AS H FROM W) AS D GROUP BY H");
    assert!(pair <= 1_000.0_f64 && pair > 500.0_f64, "of two columns of 100 values each, at most the rows: {pair}");
    let nested = groups("SELECT I, COUNT(*) FROM (SELECT H * 2 AS I FROM (SELECT G + 1 AS H FROM T) AS D) AS E GROUP BY I");
    assert!(nested <= 10.5_f64, "through two projections: {nested}");
    let beside = groups("SELECT H, J, COUNT(*) FROM (SELECT T.G + 1 AS H, V.K AS J FROM T, V) AS D GROUP BY H, J");
    assert!(near(beside, 100.0_f64), "10 values beside 10: {beside}");
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

/// Verifies: [DESIGN] RULES.md D1200, R-TEST-7: Cardenas' formula of far
/// more values than rows gives the rows, each meeting a value of its own.
/// A regression test: (1 - 1/D)^n rounded to 1 once D passed 2^53 times n,
/// so a grouping by several wide columns, their distinct counts multiplied
/// to 10^30, was estimated at one group of 381,105. Values as many as the
/// rows meet 1 - 1/e of them, and a product past every number is the
/// greatest one.
#[test]
fn cardenas_of_values_past_a_number_s_digits() {
    let n = 1_147_084.0_f64;
    for d in [1e17_f64, 1e30_f64, 1e300_f64, f64::MAX, f64::INFINITY] {
        let met = cardenas(d, n);
        assert!(met <= n && met > n * 0.999_f64, "{d} values, {n} rows: {met}");
    }
    assert!((cardenas(n, n) / n - 0.632_12_f64).abs() < 1e-4_f64, "as many values as rows");
    assert!((cardenas(f64::NAN, n) - 1.0_f64).abs() < 1e-9_f64, "no count of values is one value");
    assert!((cardenas(1e30_f64, 1.0_f64) - 1.0_f64).abs() < 1e-9_f64, "one row meets one");
    assert!((cardenas(2.0_f64, 2.0_f64) - 1.5_f64).abs() < 1e-12_f64, "two rows of two values");
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
    let values =
        LogicalPlan::Values { rows: vec![vec![Expr::Const(Value::Null); width]], types: vec![crate::value::DataType::Boolean; width] };
    LogicalPlan::Filter { cond: Cond::Const(Truth::True), input: Box::new(values) }
}

/// The state of schema E.
fn state(s: &Session) -> Arc<State> {
    s.optimized("VALUES (1)").unwrap().0
}

/// Verifies: [DESIGN] RULES.md D370: a filter over a product whose right
/// operand, the guard, has no columns is the guarded operand, its own rows
/// kept by its conjuncts, the guard one row; over a guard of a column it
/// is no more; over no product, its input's kept.
#[test]
fn a_guarded_operand_is_estimated_as_its_operand() {
    let (_db, s) = database();
    let st = state(&s);
    let est = Estimates::new(&st);
    let guarded = |g: LogicalPlan| LogicalPlan::Filter {
        cond: equal(1, 3),
        input: Box::new(LogicalPlan::Product { left: Box::new(scan("T", 2)), right: Box::new(g) }),
    };
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
/// default, an UNNEST of a collection not known as planned its input's
/// times the fan-out, and a plan deeper than the estimator looks a
/// relation of unknown rows.
#[test]
fn defaults_of_what_has_no_count() {
    let (_db, s) = database();
    let st = state(&s);
    let est = Estimates::new(&st);
    let missing = LogicalPlan::Scan {
        table: QualifiedName { auth: "E".into(), ident: "NONE".into() },
        names: vec![],
        needed: vec![],
        located: false,
    };
    assert!((est.rows(&missing) - UNKNOWN_ROWS).abs() < f64::EPSILON);
    let unnested = rows(&s, "SELECT * FROM V, UNNEST(CASE WHEN V.K > 0 THEN ARRAY[1] ELSE ARRAY[1, 2] END) AS A(X)");
    assert!(near(unnested, 10.0 * UNNEST_FANOUT), "{unnested}");
    let below = est.card_at(&scan("V", 1), MAX_ESTIMATE_DEPTH.saturating_add(1));
    assert!((below.rows - UNKNOWN_ROWS).abs() < f64::EPSILON, "past the depth looked to");
    assert!(est.rows(&LogicalPlan::Filter { cond: Cond::Const(Truth::False), input: Box::new(scan("V", 1)) }) >= 1.0_f64, "at least a row");
    let distinct = super::super::PhysicalPlanner::new(&st).card(&LogicalPlan::Distinct { input: Box::new(scan("T", 2)) });
    assert!(near(distinct.rows, 1000.0) && distinct.upper >= distinct.rows);
}

/// Each node of `plan` and its subtrees: whether its width counts its names.
fn widths_agree(plan: &LogicalPlan) -> bool {
    plan.width() == plan.names().len() && plan.inputs().all(widths_agree)
}

/// Verifies: [DESIGN] RULES.md D370: a plan's width, which the estimator
/// asks at every node, is counted as its names are, of every node kind a
/// query below makes.
#[test]
fn widths_count_the_names() {
    let (_db, s) = database();
    for sql in [
        "SELECT G, COUNT(*), MAX(K) + 1 FROM T WHERE K > 3 GROUP BY G ORDER BY 2 FETCH FIRST 2 ROWS ONLY",
        "SELECT DISTINCT U.W FROM T LEFT JOIN U ON T.K = U.K",
        "SELECT K, RANK() OVER (ORDER BY K), ROW_NUMBER() OVER (ORDER BY K) FROM V",
        "SELECT * FROM V, UNNEST(ARRAY[1, 2]) WITH ORDINALITY AS A(X, N)",
        "SELECT K FROM U EXCEPT SELECT K FROM V",
        "VALUES (1, 2), (3, 4)",
        "WITH RECURSIVE R (N) AS (VALUES (1) UNION ALL SELECT N + 1 FROM R WHERE N < 3) SELECT * FROM R",
        "SELECT * FROM T MATCH_RECOGNIZE (ORDER BY K MEASURES COUNT(*) AS C PATTERN (A B) DEFINE B AS B.G > A.G)",
    ] {
        let (_, plan) = s.optimized(sql).unwrap();
        assert!(widths_agree(&plan), "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D370, regression test (R-TEST-7): a plan of
/// 18 nested groupings, each by its input's count, which has no table, is
/// estimated once a node in an asking: each grouping asks its input's rows
/// twice, as its rows and as its column's distinct values, so the estimator
/// keeping no estimate within an asking ran 2^18 times, and 2^n of n.
#[test]
fn nested_groupings_are_estimated_once_a_node() {
    let (_db, s) = database();
    let mut sql = "SELECT K AS A, G AS B FROM T".to_owned();
    for level in 0..18_usize {
        sql = format!("SELECT B AS A, COUNT(*) AS B FROM ({sql}) AS D{level} GROUP BY B");
    }
    let estimated = rows(&s, &sql);
    assert!((1.0_f64..=1_000.0_f64).contains(&estimated), "{estimated}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR8f, SR8h]; [DESIGN] RULES.md
/// D777: an UNNEST of constant collections is its input's rows times the
/// elements of the longest, an array constructor's its values, of a
/// column too, and one of a null collection at least one row each.
#[test]
fn an_unnest_of_constants_is_estimated_by_their_elements() {
    let (_db, s) = database();
    let two = rows(&s, "SELECT * FROM V, UNNEST(ARRAY[1, 2]) AS A(X)");
    assert!(near(two, 20.0), "{two}");
    let longest = rows(&s, "SELECT * FROM UNNEST(ARRAY[1, 2, 3, 4], ARRAY[1]) AS A(X, Y)");
    assert!(near(longest, 4.0), "{longest}");
    let columns = rows(&s, "SELECT * FROM V, UNNEST(ARRAY[V.K, V.K, 2]) AS A(X)");
    assert!(near(columns, 30.0), "{columns}");
    let null = rows(&s, "SELECT * FROM V, UNNEST(CAST(NULL AS INTEGER ARRAY)) AS A(X)");
    assert!(near(null, 10.0), "{null}");
    let empty = rows(&s, "SELECT * FROM V, UNNEST(ARRAY[1, 2], CAST(NULL AS INTEGER ARRAY)) AS A(X, Y)");
    assert!(near(empty, 20.0), "a null collection beside another: {empty}");
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.6 SR5]; [DESIGN] RULES.md D779: a
/// lateral derived table of an ungrouped set function, the UNNEST of a
/// subquery gathering one row at most, is a row for each row before it;
/// one of many rows is the default fanout of each.
#[test]
fn a_lateral_of_one_row_is_estimated_at_one_for_each_row_before_it() {
    let (_db, s) = database();
    let least = rows(&s, "SELECT * FROM V, LATERAL (SELECT MIN(K) FROM T WHERE T.G = V.K) AS M (L)");
    assert!(near(least, 10.0_f64), "{least}");
    let many = rows(&s, "SELECT * FROM V, LATERAL (SELECT K FROM T WHERE T.G = V.K) AS M (L)");
    assert!(near(many, 10.0_f64 * UNNEST_FANOUT), "{many}");
}

/// `per` keys of each of ten groups into X.G, from `from`, committed,
/// and its statistics gathered if `analyzed`.
fn grown(db: &Database, s: &Session, (from, per): (i32, i32), analyzed: bool) {
    let rows: Vec<String> = (1_i32..=10_i32).flat_map(|g| (from..from.saturating_add(per)).map(move |k| format!("({g}, {k})"))).collect();
    for part in rows.chunks(500) {
        s.run(&format!("INSERT INTO G VALUES {}", part.join(", "))).unwrap();
    }
    s.commit().unwrap();
    if analyzed {
        db.analyze("X.G").unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D891, D381: once a key's greatest value
/// rose at two passes of the statistics, a range of its index past the
/// greatest the last pass saw, its first column held equal, is estimated
/// at no more rows than the index holds there, not at the share of every
/// row written since; a range within what the pass saw is left to the
/// statistics, which scale it by the rows written since. A bound of a
/// slot less a constant is read as the execution's value folded.
///
/// Regression test (R-TEST-7): the ten rows of a group's newest ten keys
/// were estimated at about a tenth of the 17,000 rows written since.
#[test]
fn a_range_past_the_statistics_is_estimated_by_its_index() {
    let db = Database::new();
    db.set_job_policy(crate::threads::JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE G (G INTEGER NOT NULL, K INTEGER NOT NULL, PRIMARY KEY (G, K))").unwrap();
    let s = db.session("X");
    for from in [1_i32, 101_i32, 201_i32] {
        grown(&db, &s, (from, 100_i32), true);
    }
    grown(&db, &s, (301_i32, 1_700_i32), false);
    let past = rows(&s, "SELECT * FROM G WHERE G = 3 AND K >= 1991");
    assert!(near(past, 10.0_f64), "past the pass: {past}");
    let within = rows(&s, "SELECT * FROM G WHERE G = 3 AND K >= 251");
    assert!(within > 20.0_f64, "within the pass, left to the statistics: {within}");
    let slotted = slotted_share(&s, 2_001);
    assert!(near(slotted * 20_000.0_f64, 10.0_f64), "a slot less a constant, by its value: {slotted}");
}

/// `G = 3 AND K >= ? - 10 AND K < ?` of X.G, its slot the first.
fn slotted() -> [Cond; 3] {
    use crate::logical::Slot;
    use crate::value::{ArithOp, DataType, Spelled};
    let slot = || Expr::Dynamic(Slot { index: 0, data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, at: 0 });
    let int = |n: i128| Expr::Const(Value::Exact(Decimal::new(n, 0)));
    let low = Expr::Arith(Box::new(slot()), ArithOp::Sub, Box::new(int(10)));
    [
        Cond::Compare(Expr::Column(0), CompOp::Eq, int(3)),
        Cond::Compare(Expr::Column(1), CompOp::Ge, low),
        Cond::Compare(Expr::Column(1), CompOp::Lt, slot()),
    ]
}

/// The share of X.G's rows `slotted` keeps, capped past the statistics,
/// its slot `value` as an execution gives it.
fn slotted_share(s: &Session, value: i128) -> f64 {
    let (st, _) = s.optimized("SELECT * FROM G").unwrap();
    let table = QualifiedName { auth: "X".into(), ident: "G".into() };
    let values = [Value::Exact(Decimal::new(value, 0))];
    crate::physical::estimate::valued(Some(&values), || Estimates::new(&st).past_capped(&table, &slotted(), 1.0_f64))
}

/// Verifies: [DESIGN] RULES.md D892: with no values at hand, a range
/// between a slot less ten and the slot, its index's first column held
/// equal, keeps at most ten values' rows of the key under that column,
/// as the pass saw them, not the rows written since; the cap of a range
/// past the statistics has no range to ask without them.
///
/// Regression test (R-TEST-7): a stock level's plan made with no values
/// estimated its lines at about 1,100 rows, five times them, and hashed
/// a read of the whole of STOCK.
#[test]
fn a_range_of_a_slot_is_estimated_by_its_width() {
    let db = Database::new();
    db.set_job_policy(crate::threads::JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE G (G INTEGER NOT NULL, K INTEGER NOT NULL, PRIMARY KEY (G, K))").unwrap();
    let s = db.session("X");
    for from in [1_i32, 101_i32, 201_i32] {
        grown(&db, &s, (from, 100_i32), true);
    }
    grown(&db, &s, (301_i32, 1_700_i32), false);
    let (st, _) = s.optimized("SELECT * FROM G").unwrap();
    let table = QualifiedName { auth: "X".into(), ident: "G".into() };
    let estimates = Estimates::new(&st);
    let past = estimates.past_capped(&table, &slotted(), 1.0_f64);
    assert!((past - 1.0_f64).abs() < f64::EPSILON, "no range to ask: {past}");
    let wide = estimates.width_capped(&table, &slotted(), 1.0_f64);
    assert!(near(wide * 20_000.0_f64, 10.0_f64), "ten values of K under G: {wide}");
}

/// A function of one argument, deterministic if `.0`.
struct Drawn(bool);

impl crate::functions::ScalarFunction for Drawn {
    fn name(&self) -> &'static str {
        "DRAWN"
    }
    fn return_type(&self, _: &[crate::value::DataType]) -> crate::error::Result<crate::value::DataType> {
        Ok(crate::value::DataType::Exact { precision: 1, scale: 0, spelled: crate::value::Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> crate::error::Result<Value> {
        Ok(Value::Null)
    }
    fn deterministic(&self) -> bool {
        self.0
    }
}

/// Verifies: [DESIGN] RULES.md D1082 (MC/DC): a projection's expression
/// is bounded by the columns it reads only if it is deterministic.
/// Decision: the column is an expression, and the expression is
/// deterministic. Each alone leaves it unbounded: a column passed on, of
/// a table, and a call that is not deterministic, which is the plan's
/// rows; the deterministic call of the same column has that column's
/// values.
#[test]
fn an_expression_not_deterministic_is_not_bounded() {
    let (_db, s) = database();
    let st = state(&s);
    let call = |fixed: bool| Expr::Call(crate::functions::ScalarRef(Arc::new(Drawn(fixed))), vec![Expr::Column(1)]);
    let over = |e: Expr| LogicalPlan::Project { exprs: vec![e], input: Box::new(scan("T", 2)) };
    let estimates = Estimates::new(&st);
    assert!(super::lineage::computed_of(&over(Expr::Column(1)), 0).is_none(), "a table's column");
    assert!(super::lineage::computed_of(&over(call(false)), 0).is_none(), "not deterministic");
    assert_eq!(super::lineage::computed_of(&over(call(true)), 0).map(|(_, read)| read), Some(vec![1]));
    assert!(near(estimates.grouped_distinct(&over(call(true)), 0, 0), 10.0_f64), "G's 10 values");
    assert!(near(estimates.grouped_distinct(&over(call(false)), 0, 0), 1_000.0_f64), "a value a row");
    assert!(near(estimates.distinct(&over(call(true)), 0), 1_000.0_f64), "a join's key is counted as before");
}
