//! Correlated subqueries unnested (RULES.md D310 to D319): each query's
//! rows, or its error, are the same as when each subquery runs for each
//! binding of its outer references [X3.135-1986 §5.21 GR3, §5.23 GR4].

mod functions;
mod tpch;

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::optimizer::{INLINE_FUNCTIONS, UNNEST_SUBQUERIES};
use turborel::params::Params;

use crate::common::{Rng, count, set_rule};

/// The rows of `sql`, sorted, each value as it is written, trailing
/// spaces and all; or the condition it raises.
pub fn outcome(s: &Session, sql: &str) -> Result<Vec<String>, Code> {
    let mut c = s.open_cursor(sql).map_err(|e| e.code)?;
    let mut out = Vec::new();
    while let Some(row) = c.fetch().map_err(|e| e.code)? {
        out.push(row.iter().map(|v| if v.is_null() { "-".to_owned() } else { format!("[{v}]") }).collect::<Vec<_>>().join(","));
    }
    out.sort();
    Ok(out)
}

/// The outcome of `sql` unnested and nested, the rules on, then off.
pub fn both(db: &Database, s: &Session, sql: &str) -> (Result<Vec<String>, Code>, Result<Vec<String>, Code>) {
    let unnested = outcome(s, sql);
    set_rule(db, UNNEST_SUBQUERIES, false);
    set_rule(db, INLINE_FUNCTIONS, false);
    let nested = outcome(s, sql);
    set_rule(db, UNNEST_SUBQUERIES, true);
    set_rule(db, INLINE_FUNCTIONS, true);
    (unnested, nested)
}

/// Whether the plan of `sql` is unnested: the rule applied, and no
/// subquery left in the plan run.
pub fn unnested(s: &Session, sql: &str) -> bool {
    let plan = s.explain(sql, &Params::new()).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let rules = plan.lines().find(|l| l.starts_with("-- rules:")).unwrap_or_default().to_owned();
    let physical = plan.split("-- physical:").nth(1).unwrap_or_default();
    rules.contains(UNNEST_SUBQUERIES) && !physical.contains("Subquery")
}

/// Schema A: R (K, V, C) and S (K, W, C), of small values, nulls and
/// strings that differ in trailing spaces alone among them.
fn random_tables(seed: u64) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V INTEGER, C VARCHAR(5)) CREATE TABLE S (K INTEGER, W INTEGER, C VARCHAR(5))")
        .unwrap();
    let s = db.session("A");
    let mut rng = Rng::new(seed);
    let value =
        |rng: &mut Rng, n: usize| if rng.chance(12) { "NULL".to_owned() } else { (i64::try_from(rng.below(n)).unwrap() - 2).to_string() };
    for (table, count) in [("R", 30), ("S", 50)] {
        for _ in 0..count {
            let (k, v) = (value(&mut rng, 7), value(&mut rng, 8));
            let c = rng.pick(&["'a'", "'a '", "'b'", "'B'", "NULL"]);
            s.run(&format!("INSERT INTO {table} VALUES ({k}, {v}, {c})")).unwrap();
        }
    }
    s.commit().unwrap();
    db
}

/// The correlated queries compared, and whether each is unnested.
const QUERIES: &[(&str, bool)] = &[
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > R.V)", true),
    ("SELECT K, V FROM R WHERE NOT EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W <> R.V)", true),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K) OR V > 2", true),
    ("SELECT K, V FROM R WHERE V IN (SELECT W FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE V NOT IN (SELECT W FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE V = ANY (SELECT W FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE V < ALL (SELECT W FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE V >= SOME (SELECT W FROM S WHERE S.C = R.C)", true),
    ("SELECT K, V FROM R WHERE V <> ALL (SELECT W FROM S WHERE S.K = R.K AND S.W IS NOT NULL)", true),
    ("SELECT K, (V IN (SELECT W FROM S WHERE S.K = R.K)) FROM R", true),
    ("SELECT K, (V > ALL (SELECT W FROM S WHERE S.K = R.K)) FROM R", true),
    ("SELECT K, (V < SOME (SELECT W FROM S WHERE S.W > R.V)) FROM R", true),
    ("SELECT K, NOT (V = ANY (SELECT W FROM S WHERE S.K = R.K)) FROM R WHERE K > 0", true),
    ("SELECT K, (SELECT MIN(W) FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT COUNT(W) + 1 FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT SUM(W) FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT AVG(W) FROM S WHERE S.K = R.K AND S.W < R.V) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.W < R.V) FROM R", true),
    ("SELECT K, V FROM R WHERE V = (SELECT MAX(W) FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE V < (SELECT COUNT(*) FROM S WHERE S.K = R.K)", true),
    ("SELECT K, V FROM R WHERE 0 = (SELECT COUNT(*) FROM S WHERE S.K = R.K AND S.W > R.V)", true),
    ("SELECT K, (SELECT W FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT W FROM S WHERE S.K = R.K AND S.W = R.V) FROM R", true),
    ("SELECT K, (SELECT DISTINCT W FROM S WHERE S.K = R.K AND S.W = R.V) FROM R", true),
    ("SELECT K, V FROM R WHERE V = (SELECT W FROM S WHERE S.K = R.K AND S.W = R.V)", true),
    ("SELECT C, (SELECT COUNT(*) FROM S WHERE S.C = R.C) FROM R", true),
    ("SELECT C, (SELECT MAX(S.C) || R.C || '|' FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT C, (SELECT MIN(S.C) || R.C FROM S WHERE S.W > R.V) FROM R", true),
    ("SELECT K, (SELECT MAX(W) FROM S WHERE S.K = R.K GROUP BY S.K) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.K = R.K GROUP BY S.C) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.K = R.K HAVING COUNT(*) > 1) FROM R", true),
    ("SELECT K, (SELECT R.V + MIN(W) FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K FROM R GROUP BY K HAVING COUNT(*) > (SELECT COUNT(*) FROM S WHERE S.K = R.K)", true),
    ("SELECT K FROM R GROUP BY K HAVING EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > 3)", true),
    (
        "SELECT K, V FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND EXISTS (SELECT * FROM R AS X WHERE X.V = S.W AND X.K <> R.K))",
        true,
    ),
    ("SELECT K, V FROM R WHERE V > (SELECT MIN(W) FROM S WHERE S.K = R.K AND S.W > (SELECT MIN(V) FROM R AS X WHERE X.C = S.C))", true),
    ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.W IN (SELECT V FROM R AS X WHERE X.K = R.K)) FROM R", true),
    ("SELECT K, V FROM R WHERE EXISTS (SELECT S.K FROM S WHERE S.K = R.K UNION SELECT X.K FROM R AS X WHERE X.V = R.V)", true),
    ("SELECT K, (SELECT COUNT(*) FROM (SELECT W FROM S WHERE S.K = R.K EXCEPT SELECT V FROM R AS X) AS E) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM (SELECT W FROM S WHERE S.K = R.K INTERSECT ALL SELECT W FROM S WHERE S.W > R.V) AS E) FROM R", true),
    ("SELECT K, (SELECT W FROM S WHERE S.K = R.K ORDER BY W DESC FETCH FIRST 1 ROW ONLY) FROM R", true),
    ("SELECT K, (SELECT SUM(W) FROM (SELECT W FROM S WHERE S.K = R.K ORDER BY W OFFSET 1 ROW FETCH FIRST 2 ROWS ONLY) AS F) FROM R", true),
    ("SELECT K, (SELECT MAX(N) FROM (SELECT ROW_NUMBER() OVER (ORDER BY W) AS N FROM S WHERE S.K = R.K) AS F) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S LEFT JOIN R AS X ON X.K = S.K AND X.V = R.V WHERE S.W = R.V) FROM R", true),
    ("SELECT K, (SELECT COUNT(X.K) FROM S LEFT JOIN R AS X ON X.K = S.K AND X.V = R.V) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S FULL JOIN R AS X ON X.K = S.K AND S.W = R.V WHERE COALESCE(S.K, X.K) = R.K) FROM R", true),
    ("SELECT K, (SELECT COUNT(*) FROM S, R AS X WHERE S.K = R.K AND X.K = R.K) FROM R", true),
    ("SELECT K, (SELECT 10 / MIN(W) FROM S WHERE S.K = R.K) FROM R", true),
    ("SELECT K, (SELECT MIN(D) FROM (SELECT 10 / (W - R.V) AS D FROM S WHERE S.K = R.K) AS F) FROM R", true),
    ("SELECT K, CASE WHEN K > 1 THEN (SELECT MIN(W) FROM S WHERE S.K = R.K) END FROM R", true),
    ("SELECT K, CASE WHEN K > 1 THEN (SELECT MIN(10 / W) FROM S WHERE S.K = R.K) END FROM R", false),
    ("SELECT K, V FROM R WHERE UNIQUE (SELECT W FROM S WHERE S.K = R.K)", false),
];

/// Verifies: [X3.135-1986 §5.11 GR1, §5.16 GR2, §5.17 GR2, GR3, §5.21 GR3;
/// ISO/IEC 9075:1992 §7.11 GR1] every correlated form unnested (RULES.md
/// D310 to D315) has the rows, nulls and trailing spaces among them, or
/// the condition, it has run for each binding: over random tables of
/// nulls and duplicates, quantified comparisons of null and empty sets,
/// COUNT over no row, a scalar subquery of more rows, and errors raised
/// in the subquery only for the bindings the outer rows have.
#[test]
fn unnested_queries_match_nested_ones() {
    for seed in [0x5EED_DEC0_0000_0001, 0x5EED_DEC0_0000_0002, 0x5EED_DEC0_0000_0003] {
        let db = random_tables(seed);
        let s = db.session("A");
        for (q, unnests) in QUERIES {
            assert_eq!(unnested(&s, q), *unnests, "{q}");
            let (on, off) = both(&db, &s, q);
            assert_eq!(on, off, "{q}");
        }
    }
}

/// Verifies: [X3.135-1986 §5.16 GR2] a quantified comparison's truth
/// value of nulls and of no row: NOT IN of a set with a null is unknown,
/// ALL of none true, SOME of none false; and [ISO/IEC 9075:1992 §7.11 GR1]
/// a scalar subquery of two rows for a binding an outer row has raises
/// cardinality violation, unnested as nested.
#[test]
fn nulls_empty_sets_and_cardinality() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (K INTEGER, V INTEGER) CREATE TABLE S (K INTEGER, W INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO R VALUES (1, 1), (2, 1), (3, 1), (NULL, 1)").unwrap();
    s.run("INSERT INTO S VALUES (1, 2), (1, NULL), (2, 1), (4, 5), (4, 6)").unwrap();
    let cases: &[(&str, Result<&[&str], Code>)] = &[
        ("SELECT K FROM R WHERE V NOT IN (SELECT W FROM S WHERE S.K = R.K)", Ok(&["-", "[3]"])),
        ("SELECT K, (V NOT IN (SELECT W FROM S WHERE S.K = R.K)) FROM R", Ok(&["-,[TRUE]", "[1],-", "[2],[FALSE]", "[3],[TRUE]"])),
        ("SELECT K, (V < ALL (SELECT W FROM S WHERE S.K = R.K)) FROM R", Ok(&["-,[TRUE]", "[1],-", "[2],[FALSE]", "[3],[TRUE]"])),
        ("SELECT K, (V < SOME (SELECT W FROM S WHERE S.K = R.K)) FROM R", Ok(&["-,[FALSE]", "[1],[TRUE]", "[2],[FALSE]", "[3],[FALSE]"])),
        ("SELECT K, (SELECT COUNT(*) FROM S WHERE S.K = R.K) FROM R", Ok(&["-,[0]", "[1],[2]", "[2],[1]", "[3],[0]"])),
        ("SELECT K, (SELECT W FROM S WHERE S.K = R.K AND S.W IS NOT NULL) FROM R", Ok(&["-,-", "[1],[2]", "[2],[1]", "[3],-"])),
        ("SELECT K, (SELECT W FROM S WHERE S.K = R.K) FROM R", Err(Code::Cardinality)),
        ("SELECT K FROM R WHERE V = (SELECT W FROM S WHERE S.K = R.K)", Err(Code::Cardinality)),
        ("SELECT K FROM R WHERE K > 1 AND V = (SELECT W FROM S WHERE S.K > R.K)", Err(Code::Cardinality)),
    ];
    for (q, want) in cases {
        let want = want.map(|rows| rows.iter().map(|r| (*r).to_owned()).collect::<Vec<_>>());
        assert!(unnested(&s, q), "{q}");
        let (on, off) = both(&db, &s, q);
        assert_eq!(on, want, "{q}");
        assert_eq!(off, want, "{q}");
    }
}

/// Verifies: [DESIGN] (RULES.md D317) EXPLAIN shows the unnested plan:
/// the rule among those applied, the join made and the set functions that
/// count each binding's rows; with OPTIMIZER.UNNEST_SUBQUERIES OFF the
/// subquery runs for each binding; a subquery that calls a function that
/// is not deterministic is left nested.
#[test]
fn explain_shows_the_unnested_plan() {
    let db = random_tables(1);
    let s = db.session("A");
    let q = "SELECT K, (SELECT W FROM S WHERE S.K = R.K) FROM R";
    let plan = s.explain(q, &Params::new()).unwrap();
    let physical = plan.split("-- physical:").nth(1).unwrap();
    assert!(physical.contains("LeftOuterJoin R.K = S.K") && physical.contains("SINGLE(COUNT(*), SINGLE_VALUE(#1))"), "{plan}");
    set_rule(&db, UNNEST_SUBQUERIES, false);
    assert!(!unnested(&s, q));
    set_rule(&db, UNNEST_SUBQUERIES, true);
    s.run("CREATE FUNCTION MAYBE (X INTEGER) RETURNS INTEGER NOT DETERMINISTIC CONTAINS SQL RETURN X").unwrap();
    assert!(!unnested(&s, "SELECT K FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = MAYBE(R.K))"));
    assert!(!unnested(&s, "SELECT K FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND MAYBE(S.W) > 0)"));
}

/// Verifies: [X3.135-1986 §8.5 GR1, §8.12 GR1] (RULES.md D318) a searched
/// DELETE or UPDATE whose condition has a correlated subquery finds its
/// object rows by an access to its table, the subquery run for each row,
/// as through an updatable cursor.
#[test]
fn changes_find_their_rows_with_subqueries_nested() {
    let db = random_tables(2);
    let s = db.session("A");
    let doomed = count(&s, "SELECT K FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K)");
    s.run("UPDATE R SET V = (SELECT MIN(W) FROM S WHERE S.K = R.K) WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K AND S.W > R.V)").unwrap();
    let before = count(&s, "SELECT K FROM R");
    s.run("DELETE FROM R WHERE EXISTS (SELECT * FROM S WHERE S.K = R.K)").unwrap();
    assert_eq!(count(&s, "SELECT K FROM R"), before - doomed);
}
