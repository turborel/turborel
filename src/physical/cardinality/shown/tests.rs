//! Unit tests of what a table's sample shows of an expression grouped by
//! (RULES.md D1279). A time less its seconds, the whole hundreds of a
//! number, the first characters of a string and a sum of two columns are
//! counted as they are, rows that raise are left out, and in each case in
//! which the sample shows nothing the columns' bound stands.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::lineage::computed_of;
use super::super::seen::{named, ungathered};
use super::{Lattice, common_step, guaranteed, shown};
use crate::catalog::QualifiedName;
use crate::db::{Database, Session, State};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Expr, LogicalPlan};
use crate::physical::cardinality::Estimates;
use crate::value::{DataType, Decimal, Spelled, Value};

/// The rows of S.
const ROWS: i32 = 5_000;

/// Schema N: S of 5,000 rows, so its sample is a part of it. K is a key;
/// G is K modulo 1,000; A and B are the last two digits of K; TS is a
/// time seven seconds after the last row's, 584 minutes in all; P is a
/// string that begins with one of 25 pairs of digits; L is one of seven
/// short strings, or of every fiftieth row a string longer than a sampled
/// row keeps. Statistics gathered.
fn database() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION N
           CREATE TABLE S (K INTEGER NOT NULL, G INTEGER NOT NULL, A INTEGER NOT NULL, B INTEGER NOT NULL,
             TS TIMESTAMP NOT NULL, P CHAR(15) NOT NULL, L VARCHAR(400) NOT NULL)",
    )
    .unwrap();
    let s = db.session("N");
    let row = |k: i32| {
        let at = k.saturating_mul(7_i32);
        let (hour, minute) = (at.div_euclid(3_600_i32), at.div_euclid(60_i32) % 60_i32);
        let ts = format!("TIMESTAMP '2001-02-03 {hour:02}:{minute:02}:{:02}'", at % 60_i32);
        let long = if k % 50_i32 == 0_i32 { "x".repeat(300) } else { format!("v{}", k % 7_i32) };
        let (tens, pair) = (k.div_euclid(10_i32) % 10_i32, (k % 25_i32).saturating_add(10_i32));
        format!("({k}, {}, {}, {tens}, {ts}, '{pair}-{k:07}', '{long}')", k % 1_000_i32, k % 10_i32)
    };
    let rows: Vec<String> = (0_i32..ROWS).map(row).collect();
    for part in rows.chunks(500) {
        s.run(&format!("INSERT INTO S VALUES {}", part.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.maintain().unwrap();
    (db, s)
}

/// What a grouping counts of its first grouping column, an expression.
struct Counted {
    /// The distinct values the grouping counts of it.
    grouped: f64,
    /// Those of the columns it reads.
    read: f64,
    /// Those the sample shows, if any.
    shown: Option<f64>,
}

/// `Counted` of the first grouping of `sql`'s optimized plan.
fn counted(s: &Session, sql: &str) -> Counted {
    let (st, plan) = s.optimized(sql).unwrap();
    let (input, column) = grouping(&plan).unwrap();
    let estimates = Estimates::new(&st);
    let computed = computed_of(input, column).unwrap();
    let whole = estimates.distinct(input, column);
    let read = estimates.read_distinct(&computed, whole, 0);
    Counted { grouped: estimates.grouped_distinct(input, column, 0), read, shown: estimates.shown_distinct(&computed, whole) }
}

/// A grouping by the expression `e` of the rows of `from`.
fn grouped(e: &str, from: &str) -> String {
    format!("SELECT M, COUNT(*) FROM (SELECT {e} AS M FROM {from}) AS D GROUP BY M")
}

/// The input and the first grouping column of the first grouping in
/// `plan`.
fn grouping(plan: &LogicalPlan) -> Option<(&LogicalPlan, usize)> {
    if let LogicalPlan::Aggregate { groups, input, .. } = plan {
        return Some((input, *groups.first()?));
    }
    plan.inputs().find_map(grouping)
}

/// Whether `a` is `b`, as counts are.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5_f64
}

/// Whether `a` is within a hundredth of `b`, as a sketch's count of a
/// column's values is of the values.
fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= b / 100.0_f64
}

/// Verifies: [DESIGN] RULES.md D1279: a grouping by an expression is
/// counted as its table's sample shows the expression, never more than
/// the columns it reads allow. A time less its seconds has the minutes of
/// its range, by the lattice of its values; the whole hundreds of a key
/// its fifty; the first two characters of a string their 25 pairs, by
/// the values the sample holds, each seen more than once; and a sum of
/// two columns of ten values its nineteen sums, where the columns allow
/// a hundred.
#[test]
fn an_expression_grouped_by_is_counted_as_the_sample_shows_it() {
    let (_db, s) = database();
    let by = |e: &str| counted(&s, &grouped(e, "S"));
    let minutes = by("TS - EXTRACT(SECOND FROM TS) * INTERVAL '1' SECOND");
    assert!(
        (575.0_f64..=584.0_f64).contains(&minutes.grouped) && minutes.shown == Some(minutes.grouped),
        "584 minutes: {}",
        minutes.grouped
    );
    assert!(near(minutes.read, 5_000.0_f64), "a time a row by its column: {}", minutes.read);
    let hundreds = by("FLOOR(K / 100)");
    assert!(same(hundreds.grouped, 50.0_f64) && near(hundreds.read, 5_000.0_f64), "{} of {}", hundreds.grouped, hundreds.read);
    let pairs = by("SUBSTRING(P FROM 1 FOR 2)");
    assert!(same(pairs.grouped, 25.0_f64) && pairs.read > 4_000.0_f64, "{} of {}", pairs.grouped, pairs.read);
    let sums = by("A + B");
    assert!(same(sums.grouped, 19.0_f64) && same(sums.read, 100.0_f64), "{} of {}", sums.grouped, sums.read);
}

/// Verifies: [DESIGN] RULES.md D1279, D15: a sampled row whose evaluation
/// raises is left out and nothing is raised. A hundred divided by a
/// number that is zero in one row of fifty has, of the rows that do not
/// raise, 49 quotients, each seen more than once.
#[test]
fn a_sampled_row_that_raises_is_left_out() {
    let (_db, s) = database();
    let quotients = counted(&s, &grouped("100 / MOD(K, 50)", "S"));
    assert!(same(quotients.grouped, 49.0_f64) && quotients.shown == Some(quotients.grouped), "{}", quotients.grouped);
    assert!(near(quotients.read, 5_000.0_f64), "{}", quotients.read);
}

/// Verifies: [DESIGN] RULES.md D1279 (MC/DC): the sample shows nothing,
/// and the count is that of the columns read, in each case alone. No
/// statistics are kept. No two rows that differ in the columns read give
/// one value, as of a key and one. Most sampled rows hold a value seen
/// once, as of strings of half as many values as rows, which a count of
/// the values seen would put below the values there are. A column read
/// may be cut in the sample. The columns read are two tables'. The plan has no more rows
/// than the sample. And what the sample shows is never taken where it is
/// more than the columns allow: strings of G's thousand values, a few
/// made one, are counted past a thousand by the values seen once.
#[test]
fn where_the_sample_shows_nothing_the_columns_bound_stands() {
    let (_db, s) = database();
    let by = |e: &str, from: &str| counted(&s, &grouped(e, from));
    let stands = |c: &Counted| c.shown.is_none() && same(c.grouped, c.read);
    let merged = by("K + 1", "S");
    assert!(stands(&merged) && near(merged.read, 5_000.0_f64), "nothing merged: {} {:?}", merged.grouped, merged.shown);
    let cut = by("SUBSTRING(L FROM 1 FOR 1)", "S");
    assert!(stands(&cut) && cut.read > 2.0_f64, "a column cut: {} {:?}", cut.read, cut.shown);
    let two = by("X.A + Y.B", "S AS X, S AS Y");
    assert!(stands(&two) && same(two.read, 100.0_f64), "two tables: {} {:?}", two.grouped, two.shown);
    let few = by("FLOOR(K / 100)", "S WHERE K < 1000");
    assert!(stands(&few) && few.read > 500.0_f64, "no more rows than the sample: {} {:?}", few.read, few.shown);
    let more = by("FLOOR(K / 100)", "S WHERE K < 2000");
    assert!(
        more.shown.is_some_and(|d| same(d, 50.0_f64)) && same(more.grouped, 50.0_f64) && more.read > 1_024.0_f64,
        "more rows: {}",
        more.read
    );
    let many = by("'p' || CAST(FLOOR(K / 2) AS VARCHAR(40))", "S");
    assert!(stands(&many) && near(many.read, 5_000.0_f64), "more values than the sample counts: {} {:?}", many.read, many.shown);
    let past = by("CASE WHEN G < 10 THEN 'few' ELSE CAST(G AS VARCHAR(10)) END", "S");
    assert!(past.shown.is_some_and(|d| d > past.read) && same(past.grouped, past.read), "never more: {} {:?}", past.read, past.shown);
    let (_other, bare) = ungathered(&[("T", 3_000, 3_000)]);
    let project = LogicalPlan::Project { exprs: vec![halved(Expr::Column(1))], input: Box::new(scan(named("T"), 2)) };
    let estimates = Estimates::new(&bare);
    let computed = computed_of(&project, 0).unwrap();
    assert!(estimates.shown_distinct(&computed, 3_000.0_f64).is_none(), "no statistics");
    assert!(same(estimates.grouped_distinct(&project, 0, 0), 3_000.0_f64), "a value a row, as before");
}

/// `e` divided by two.
fn halved(e: Expr) -> Expr {
    Expr::Arith(Box::new(e), crate::value::ArithOp::Div, Box::new(Expr::Const(Value::Exact(Decimal::new(2, 0)))))
}

/// A scan of `table`, of `width` columns.
fn scan(table: QualifiedName, width: usize) -> LogicalPlan {
    LogicalPlan::Scan { table, names: (0..width).map(|c| format!("C{c}")).collect(), needed: (0..width).collect(), located: false }
}

/// A function of one exact number, its remainder by `.0`, that counts its
/// calls in `.1`: two of them are two functions of one name.
struct Remainder(i128, Arc<AtomicUsize>);

impl ScalarFunction for Remainder {
    fn name(&self) -> &'static str {
        "REMAINDER"
    }
    fn return_type(&self, _: &[DataType]) -> crate::error::Result<DataType> {
        Ok(DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
        self.1.fetch_add(1, Ordering::Relaxed);
        let Some(Value::Exact(d)) = args.first() else { return Ok(Value::Null) };
        Ok(Value::Exact(Decimal::new(d.units.rem_euclid(self.0), 0)))
    }
}

/// Verifies: [DESIGN] RULES.md D1279: what a sample shows of an
/// expression is found once for a statement, however often it is asked,
/// and of a plan of no more rows than the sample it is not asked at all;
/// and two functions of one name that differ in what they hold have each
/// its own answer, where a key of the name alone would give the second
/// the first's.
#[test]
fn an_answer_is_found_once_and_is_its_own_function_s() {
    let (_db, s) = database();
    let st: Arc<State> = s.optimized("VALUES (1)").unwrap().0;
    let calls = Arc::new(AtomicUsize::new(0));
    let of = |by: i128| Expr::Call(ScalarRef(Arc::new(Remainder(by, Arc::clone(&calls)))), vec![Expr::Column(0)]);
    let over =
        |e: Expr| LogicalPlan::Project { exprs: vec![e], input: Box::new(scan(QualifiedName { auth: "N".into(), ident: "S".into() }, 7)) };
    let (sevens, elevens) = (over(of(7)), over(of(11)));
    let estimates = Estimates::new(&st);
    let few = computed_of(&sevens, 0).unwrap();
    assert!(estimates.shown_distinct(&few, 1_024.0_f64).is_none() && calls.load(Ordering::Relaxed) == 0, "a plan of few rows asks nothing");
    assert!(same(estimates.grouped_distinct(&sevens, 0, 0), 7.0_f64));
    let once = calls.load(Ordering::Relaxed);
    assert_eq!(once, crate::stats::sample::SAMPLE_ROWS, "each sampled row once");
    assert!(same(estimates.grouped_distinct(&sevens, 0, 0), 7.0_f64) && calls.load(Ordering::Relaxed) == once, "kept for the statement");
    assert!(same(estimates.grouped_distinct(&elevens, 0, 0), 11.0_f64), "another function of the name");
}

/// Verifies: [DESIGN] RULES.md D1279: the count GEE makes of a sample.
/// Every value seen more than once is the values seen; each value seen
/// once counts as the square root of the rows a sampled row stands for;
/// and a sample that is the whole table, or more than the rows now
/// counted, holds every value. The greatest common step of two distances
/// is found whichever is nothing.
#[test]
fn values_seen_once_count_as_the_root_of_the_rows_a_row_stands_for() {
    assert!(same(guaranteed((7.0_f64, 0.0_f64), 1_024.0_f64, 1.0e8_f64), 7.0_f64));
    assert!(same(guaranteed((10.0_f64, 4.0_f64), 100.0_f64, 10_000.0_f64), 46.0_f64), "four seen once, ten rows each way");
    assert!(same(guaranteed((10.0_f64, 4.0_f64), 100.0_f64, 100.0_f64), 10.0_f64), "the whole table");
    assert!(same(guaranteed((10.0_f64, 4.0_f64), 100.0_f64, 50.0_f64), 10.0_f64), "rows deleted since");
    assert_eq!((common_step(0, 60), common_step(60, 0), common_step(180, 120), common_step(7, 5)), (60, 60, 60, 1));
}

/// Verifies: [DESIGN] RULES.md D1279 (MC/DC): the places of the lattice
/// the values lie on. Numbers of one scale, sixty apart at the least,
/// have a place each sixty between the least and the greatest; a null is
/// one place more; one value has one place. No lattice is told of no
/// value, of a value that lies on no line, as a string, nor of numbers
/// of two scales.
#[test]
fn a_lattice_has_a_place_each_step_and_one_of_a_null() {
    let exact = |units: i128, scale: u32| Value::Exact(Decimal::new(units, scale));
    let places = |values: &[Value]| {
        let mut lattice = Lattice::default();
        for v in values {
            lattice.add(v);
        }
        lattice.places()
    };
    let sixties = [exact(300, 0), exact(0, 0), exact(120, 0)];
    assert_eq!(places(&sixties), Some(6.0_f64));
    assert_eq!(places(&[sixties.as_slice(), &[Value::Null]].concat()), Some(7.0_f64), "a null");
    assert_eq!(places(&[exact(5, 0), exact(5, 0)]), Some(1.0_f64), "one value");
    assert_eq!(places(&[Value::Null]), None, "no value");
    assert_eq!(places(&[exact(1, 0), Value::Char("a".into())]), None, "on no line");
    assert_eq!(places(&[exact(1, 0), exact(15, 1)]), None, "two scales");
}

/// A function of one exact number, the name of its remainder by `.0`, a
/// character string.
struct Named(i128);

impl ScalarFunction for Named {
    fn name(&self) -> &'static str {
        "NAMED"
    }
    fn return_type(&self, _: &[DataType]) -> crate::error::Result<DataType> {
        Ok(DataType::varying(40))
    }
    fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
        let Some(Value::Exact(d)) = args.first() else { return Ok(Value::Null) };
        Ok(Value::Char(format!("v{}", d.units.rem_euclid(self.0)).into()))
    }
}

/// A sample of 1,024 rows of one column, a key, whose remainders by
/// `values` are drawn across them with no pattern, the last row's the
/// first's again, as one chance repeat among them would be.
fn drawn(values: i128) -> Vec<crate::storage::Row> {
    // Of both parities, as a cube and a square alone would not be.
    let of = |i: i128| {
        let (square, cube) = (i.wrapping_mul(i), i.wrapping_mul(i).wrapping_mul(i));
        cube.wrapping_mul(1_000_003).wrapping_add(square.wrapping_mul(7_919)).wrapping_add(i).rem_euclid(values)
    };
    let key = |i: i128, value: i128| vec![Value::Exact(Decimal::new(values.wrapping_mul(i.wrapping_add(1)).wrapping_add(value), 0))];
    let mut rows: Vec<crate::storage::Row> = (0_i128..1_023).map(|i| key(i, of(i))).collect();
    rows.push(key(1_023, of(0)));
    rows
}

/// Verifies: [DESIGN] RULES.md D1279: an expression of more values than
/// the sample can count is never counted below the values there are. Of
/// a table of a hundred million rows and an expression of a million
/// equally common values, and of ten million, the sample holds nearly
/// every value once and one by chance twice. Counted by the values seen
/// alone it would be put at some three hundred thousand. Of strings the
/// sample then shows nothing, and the columns' bound stands; of numbers
/// it shows the places of their range, which are the values but for the
/// few thousandths of the range past the least and greatest sampled.
#[test]
fn more_values_than_the_sample_counts_are_never_counted_below_them() {
    const TABLE: f64 = 1.0e8;
    assert!(guaranteed((1_023.0_f64, 1_022.0_f64), 1_024.0_f64, TABLE) < 1.0e6_f64, "what the values seen alone would count");
    for (values, in_all) in [(1_000_000_i128, 1.0e6_f64), (10_000_000_i128, 1.0e7_f64)] {
        let rows = drawn(values);
        let named = Expr::Call(ScalarRef(Arc::new(Named(values))), vec![Expr::Column(0)]);
        assert_eq!(shown(&named, &[0], &rows, TABLE), None, "strings of {values} values");
        let numbered = Expr::Call(ScalarRef(Arc::new(Remainder(values, Arc::new(AtomicUsize::new(0))))), vec![Expr::Column(0)]);
        let places = shown(&numbered, &[0], &rows, TABLE).unwrap();
        assert!(places >= in_all * 0.98_f64 && places <= in_all, "numbers of {values} values: {places}");
    }
}
