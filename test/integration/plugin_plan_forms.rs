//! \[EXTENSION\] The plan form of RULES.md D275 to D277 over the plans of
//! every kind of query: an optimizer rule of the example plug-in
//! (`plugin/examples/plugin_example`) rewrites a filter no row passes,
//! deep in each plan, and the plan given back, made of the form whole,
//! returns the rows the query returns with no rule.

use turborel::db::Session;
use turborel::params::Params;

use crate::common::sorted;
use crate::plugin_planner::planner;

/// A derived table of no rows, which the rule finds and makes an empty
/// VALUES: the rest of each query's plan is taken back from its form.
const EMPTY: &str = "(SELECT A, B FROM T WHERE A > 5 AND A < 3) AS E (X, Y)";

/// Queries of every operator the form says, or keeps, each with `EMPTY`
/// in place of `{E}`.
const QUERIES: [&str; 19] = [
    "SELECT T.A, E.X FROM T LEFT JOIN {E} ON T.A = E.X WHERE T.A < 3",
    "SELECT T.A, E.X FROM {E} RIGHT JOIN T ON E.X = T.A WHERE T.A <> 3 AND T.A <= 4",
    "SELECT T.A, E.Y FROM T FULL JOIN {E} ON T.B = E.Y WHERE T.A >= 19",
    "SELECT T.A FROM T CROSS JOIN {E}",
    "SELECT A FROM T WHERE A < 3 UNION SELECT X FROM {E}",
    "SELECT B FROM T EXCEPT ALL SELECT Y FROM {E}",
    "SELECT B FROM T INTERSECT SELECT Y FROM {E}",
    "SELECT DISTINCT B FROM T UNION ALL SELECT DISTINCT Y FROM {E}",
    "SELECT A, B FROM T WHERE A > 17 UNION ALL SELECT X, Y FROM {E} ORDER BY 2 DESC NULLS FIRST, 1",
    "SELECT B, COUNT(*) FROM T GROUP BY B UNION SELECT Y, COUNT(*) FROM {E} GROUP BY Y",
    "SELECT A FROM T UNION ALL SELECT X FROM {E} ORDER BY 1 FETCH FIRST 2 ROWS ONLY",
    "SELECT -A, A + 1, A - 1, A * 2, A / 2, FLOOR(A - 10), CAST(A AS BIGINT) FROM T \
     WHERE (A = 1 OR NOT (A <> 2)) AND B IS NOT NULL UNION ALL SELECT X, X, X, X, X, X, X FROM {E}",
    "SELECT CASE WHEN A > 1 THEN 'y' ELSE 'n' END FROM T WHERE A IN (1, 2) UNION ALL SELECT 'e' FROM {E}",
    "SELECT P, Q FROM (VALUES (1, 'a'), (2, 'b')) AS V (P, Q) UNION ALL SELECT X, 'c' FROM {E}",
    "SELECT A, ROW_NUMBER() OVER (ORDER BY A) FROM T WHERE A < 3 UNION ALL SELECT X, Y FROM {E}",
    "WITH RECURSIVE R (N) AS (SELECT A FROM T WHERE A = 1 UNION ALL SELECT N + 1 FROM R WHERE N < 3) \
     SELECT N FROM R UNION ALL SELECT X FROM {E}",
    "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'T' UNION ALL SELECT 'E' FROM {E}",
    "SELECT E.X FROM {E} WHERE E.X IS NULL OR E.Y = 1",
    "SELECT A FROM T WHERE EXISTS (SELECT 1 FROM T WHERE B = 3) AND A = 2 UNION SELECT X FROM {E}",
];

/// `QUERIES`, `EMPTY` in place.
fn queries() -> Vec<String> {
    QUERIES.iter().map(|q| q.replace("{E}", EMPTY)).collect()
}

/// Each of `all`'s rows, sorted.
fn each(s: &Session, all: &[String]) -> Vec<Vec<String>> {
    all.iter().map(|q| sorted(s, q)).collect()
}

/// Verifies: [EXTENSION] RULES.md D275, D277: the form of a plan of each
/// kind of node, joins of each kind, each set operator, DISTINCT, ORDER
/// BY, VALUES, nodes kept with their inputs open (aggregate, limit,
/// window), kept whole (a recursive query, a system table), and
/// expressions said whole or shown opaque, is taken back as a valid plan
/// once a rule rewrites a filter deep in it: EXPLAIN names the rule
/// fired, none skipped, and each query returns the rows it returns with
/// the extension dropped.
#[test]
fn every_kind_of_plan_is_taken_back_from_its_form() {
    let (_scratch, db) = planner("plugin-plan-forms", "OPTIMIZER RULE CONTRADICTION PHASE BOUND;\n");
    let s = db.session("U");
    let all = queries();
    for q in &all {
        let shown = s.explain(q, &Params::new()).unwrap_or_else(|e| panic!("{q}: {}", e.msg));
        assert!(shown.contains("PLANNER.CONTRADICTION") && !shown.contains("skipped"), "{q}\n{shown}");
    }
    let with = each(&s, &all);
    s.commit().unwrap();
    s.run("DROP EXTENSION planner").unwrap();
    assert_eq!(with, each(&s, &all));
    assert_eq!(with[3], Vec::<String>::new(), "a product with no rows");
    assert_eq!(with[0].len(), 2, "the left join keeps T's rows");
}

/// Verifies: [EXTENSION] RULES.md D276: a rule of PHASE REWRITTEN sees
/// the plan the built-in rules made, and its form, rewritten, is taken
/// back as the plan run, with the rows of the plan the rule did not see.
#[test]
fn a_rewritten_plan_is_taken_back_from_its_form() {
    let (_scratch, db) = planner("plugin-plan-forms-late", "OPTIMIZER RULE CONTRADICTION PHASE REWRITTEN;\n");
    let s = db.session("U");
    let all = queries();
    let with = each(&s, &all);
    s.commit().unwrap();
    s.run("DROP EXTENSION planner").unwrap();
    assert_eq!(with, each(&s, &all));
}
