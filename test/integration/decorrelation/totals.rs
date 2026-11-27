//! Correlated subqueries of SUM and AVG pulled up by their equalities
//! (RULES.md D1060, D314): computed once for every value of the columns
//! they are correlated by, where the declared types keep the sum within an
//! exact number's digits, and by the domain of bindings where they do not.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

use super::{both, unnested};

/// Schema T: R (K, V) and S (K, W, BIG), W of few digits and BIG of
/// thirty-eight. R has a key twice, one S has no row of, and a null one; S has
/// a null key, a key R has not, and a null value.
fn tables() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION T CREATE TABLE R (K INTEGER, V DECIMAL(12, 2))
         CREATE TABLE S (K INTEGER, W DECIMAL(12, 2), BIG DECIMAL(38, 0))",
    )
    .unwrap();
    let s = db.session("T");
    s.run("INSERT INTO R VALUES (1, 2.00), (1, 5.00), (2, 1.00), (3, 1.00), (NULL, 1.00)").unwrap();
    s.run("INSERT INTO S VALUES (1, 2.00, 1), (1, 4.00, 2), (1, NULL, 3), (2, 1.50, 4), (NULL, 9.00, 5), (7, 8.00, 6)").unwrap();
    s.commit().unwrap();
    db
}

/// The physical plan EXPLAIN shows of `sql`.
fn physical(s: &Session, sql: &str) -> String {
    let plan = s.explain(sql, &Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap().to_owned()
}

/// Whether the plan of `sql` reads a domain of bindings, whose columns
/// count each binding's trailing spaces (RULES.md D319).
fn by_domain(s: &Session, sql: &str) -> bool {
    physical(s, sql).contains("TRAILING_SPACES")
}

/// Verifies: [X3.135-1986 §5.8 GR4c, GR4d, §5.21 GR3] [DESIGN] RULES.md
/// D1060: a correlated subquery of SUM or AVG of a column of few digits
/// is unnested with no domain of bindings, and gives what it gives run
/// for each row: null of a key with no row and of a null key, 0 of
/// COUNT there, each outer row of a key its value, a row of a null key in
/// the subquery's table read by none, and nulls eliminated.
#[test]
fn sums_of_few_digits_are_pulled_up_and_keep_their_values() {
    let db = tables();
    let s = db.session("T");
    let cases: &[(&str, &[&str])] = &[
        (
            "SELECT K, V, (SELECT SUM(W) FROM S WHERE S.K = R.K) FROM R",
            &["-,[1.00],-", "[1],[2.00],[6.00]", "[1],[5.00],[6.00]", "[2],[1.00],[1.50]", "[3],[1.00],-"],
        ),
        (
            "SELECT K, V, (SELECT AVG(W) FROM S WHERE S.K = R.K) FROM R",
            &["-,[1.00],-", "[1],[2.00],[3.0000]", "[1],[5.00],[3.0000]", "[2],[1.00],[1.5000]", "[3],[1.00],-"],
        ),
        (
            "SELECT K, (SELECT COUNT(W) + COALESCE(SUM(W), 0) FROM S WHERE S.K = R.K) FROM R",
            &["-,[0.00]", "[1],[8.00]", "[1],[8.00]", "[2],[2.50]", "[3],[0.00]"],
        ),
        ("SELECT K, V FROM R WHERE V < (SELECT 0.5 * SUM(W) FROM S WHERE S.K = R.K)", &["[1],[2.00]"]),
        ("SELECT K, V FROM R WHERE V > (SELECT AVG(W) FROM S WHERE S.K = R.K)", &["[1],[5.00]"]),
        ("SELECT K, V FROM R WHERE (SELECT SUM(W) FROM S WHERE S.K = R.K) IS NULL", &["-,[1.00]", "[3],[1.00]"]),
        ("SELECT K, V FROM R WHERE V >= (SELECT SUM(W) FROM S WHERE S.K = R.K AND S.W > 3.00)", &["[1],[5.00]"]),
    ];
    for (q, want) in cases {
        assert!(unnested(&s, q) && !by_domain(&s, q), "{q}: {}", physical(&s, q));
        let (on, off) = both(&db, &s, q);
        let want: Vec<String> = want.iter().map(|r| (*r).to_owned()).collect();
        assert_eq!(on, Ok(want.clone()), "{q}");
        assert_eq!(off, Ok(want), "{q}");
    }
}

/// Verifies: [ISO/IEC 9075:1992 §6.5 GR2b iv] [DESIGN] RULES.md D1060, D15:
/// a SUM of a column of thirty-eight digits can pass an exact number's digits,
/// so its subquery is unnested by the domain of bindings and computed for
/// the outer rows' keys alone: a key no outer row has, whose sum is out of
/// range, raises nothing, and one an outer row has raises as it does run
/// for that row.
#[test]
fn a_sum_that_can_overflow_is_computed_for_the_outer_keys_alone() {
    let db = tables();
    let s = db.session("T");
    // Two values of 38 digits, whose sum has 39.
    let big = format!("6{}", "0".repeat(37));
    s.run(&format!("INSERT INTO S VALUES (7, 1.00, {big}), (7, 1.00, {big})")).unwrap();
    s.commit().unwrap();
    let q = "SELECT K, (SELECT SUM(BIG) FROM S WHERE S.K = R.K) FROM R";
    assert!(unnested(&s, q) && by_domain(&s, q), "{}", physical(&s, q));
    let (on, off) = both(&db, &s, q);
    let want: Vec<String> = ["-,-", "[1],[6]", "[1],[6]", "[2],[4]", "[3],-"].iter().map(|r| (*r).to_owned()).collect();
    assert_eq!(on, Ok(want.clone()));
    assert_eq!(off, Ok(want));
    assert!(s.run("SELECT SUM(BIG) FROM S WHERE K = 7").is_err(), "the sum of key 7 is out of range");
    s.run("INSERT INTO R VALUES (7, 1.00)").unwrap();
    let (on, off) = both(&db, &s, q);
    assert_eq!(on, Err(Code::Data));
    assert_eq!(off, Err(Code::Data));
}

/// Verifies: [X3.135-1986 §5.13 GR2; ISO/IEC 9075:1992 §7.11 GR1] [DESIGN]
/// RULES.md D1060, D541: IN of a subquery whose condition compares with a
/// correlated subquery of SUM is a semi
/// join of the subquery's rows, the SUM grouped once with no domain, as
/// a subquery of one row at most whose sum fits raises nothing; one whose
/// condition compares with a subquery that may give two rows, and so may
/// raise, is not, and fails as it does run for each row.
#[test]
fn a_subquery_holding_a_sum_s_subquery_is_a_semi_join() {
    let db = tables();
    let s = db.session("T");
    let summed = "SELECT K, V FROM R WHERE K IN (SELECT S.K FROM S WHERE S.W > (SELECT 0.5 * SUM(X.W) FROM S AS X WHERE X.K = S.K))";
    let plan = physical(&s, summed);
    assert!(unnested(&s, summed) && !by_domain(&s, summed) && plan.contains("SemiJoin"), "{plan}");
    let (on, off) = both(&db, &s, summed);
    let want: Vec<String> = ["[1],[2.00]", "[1],[5.00]", "[2],[1.00]"].iter().map(|r| (*r).to_owned()).collect();
    assert_eq!(on, Ok(want.clone()));
    assert_eq!(off, Ok(want));
    let single = "SELECT K, V FROM R WHERE K IN (SELECT S.K FROM S WHERE S.W > (SELECT X.W FROM S AS X WHERE X.K = S.K AND X.W < 3.00))";
    assert!(by_domain(&s, single), "{}", physical(&s, single));
    let (on, off) = both(&db, &s, single);
    assert_eq!(on, off);
    s.run("INSERT INTO S VALUES (1, 1.00, 0)").unwrap();
    let (on, off) = both(&db, &s, single);
    assert_eq!(on, Err(Code::Cardinality));
    assert_eq!(off, Err(Code::Cardinality));
}

/// Whether, in the optimized plan EXPLAIN shows of `sql`, an operator
/// whose line holds `above` has the first operator whose line holds
/// `below` under it.
fn over(s: &Session, sql: &str, above: &str, below: &str) -> bool {
    let plan = s.explain(sql, &Params::new()).unwrap();
    let optimized = plan.split("-- optimized:").nth(1).unwrap().split("-- physical:").next().unwrap();
    let lines: Vec<&str> = optimized.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let Some(at) = lines.iter().position(|l| l.contains(below)) else { return false };
    let mut depth = indent(lines[at]);
    let mut found = false;
    for line in lines[..at].iter().rev() {
        if indent(line) < depth {
            depth = indent(line);
            found = found || line.contains(above);
        }
    }
    found
}

/// Verifies: [X3.135-1986 §5.21 GR2] [DESIGN] RULES.md D1064, D15: a
/// WHERE conjunct that compares with a correlated subquery, beside IN of
/// another, is judged of the rows the semi join keeps, the subquery's
/// grouping joined above the semi join, where it cannot raise; one that
/// can raise, a quotient by a column, stays below it, judged of every
/// row; and beside NOT IN it stays below the antijoin. Each query gives
/// the rows, or the error, it gives with no rule.
#[test]
fn a_subquery_s_conjunct_is_judged_after_a_semi_join() {
    let db = tables();
    let s = db.session("T");
    let summed = "(SELECT 0.5 * SUM(X.W) FROM S AS X WHERE X.K = S.K)";
    let quiet = format!("SELECT K, W FROM S WHERE K IN (SELECT R.K FROM R WHERE R.V > 1.50) AND W > {summed}");
    assert!(unnested(&s, &quiet) && over(&s, &quiet, "SUM(X.W)", "Semi join"), "{}", s.explain(&quiet, &Params::new()).unwrap());
    let (on, off) = both(&db, &s, &quiet);
    assert_eq!(on, Ok(vec!["[1],[4.00]".to_owned()]));
    assert_eq!(off, on);
    let raising = format!("SELECT K, W FROM S WHERE K IN (SELECT R.K FROM R WHERE R.V > 1.50) AND 10 / (W - 8.00) > {summed}");
    assert!(!over(&s, &raising, "SUM(X.W)", "Semi join"), "{}", s.explain(&raising, &Params::new()).unwrap());
    let (on, off) = both(&db, &s, &raising);
    assert_eq!(on, Err(Code::Data), "the row of key 7, which the semi join drops, divides by zero");
    assert_eq!(off, on);
    let anti = format!("SELECT K, W FROM S WHERE K NOT IN (SELECT R.K FROM R WHERE R.K IS NOT NULL) AND W > {summed}");
    assert!(!over(&s, &anti, "SUM(X.W)", "Anti"), "{}", s.explain(&anti, &Params::new()).unwrap());
    let (on, off) = both(&db, &s, &anti);
    assert_eq!(on, Ok(vec!["[7],[8.00]".to_owned()]));
    assert_eq!(off, on);
}
