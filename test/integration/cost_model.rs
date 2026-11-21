//! The planner's cost model (RULES.md D650 to D659): the order a filter
//! judges its conjuncts in, what operators past their work grant cost,
//! the degree a part will run at, and the rows a runtime filter leaves,
//! each seen in the plan EXPLAIN shows and each plan's rows the same.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::{both_ways, fetched};

/// Rows of T.
const ROWS: usize = 2_000;

/// Schema Q: T, of `ROWS` rows, A its key, B one of 100, C one of 7.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION Q CREATE TABLE T (A INTEGER, B INTEGER, C INTEGER)").unwrap();
    let s = db.session("Q");
    let rows: Vec<String> = (0..ROWS).map(|k| format!("({k}, {}, {})", k % 100, k % 7)).collect();
    s.run(&format!("INSERT INTO T VALUES {}", rows.join(", "))).unwrap();
    s.commit().unwrap();
    (db, s)
}

/// The line of `plan`'s physical plan showing its filter.
fn filter_line(plan: &str) -> &str {
    let physical = plan.split("-- physical:").nth(1).unwrap_or_else(|| panic!("no physical plan in {plan}"));
    physical.lines().find(|l| l.contains("Filter")).unwrap_or_else(|| panic!("no filter in {plan}"))
}

/// Verifies: [DESIGN] (RULES.md D650) a filter judges first the conjunct
/// of least rank, its cost over the rows it drops: the equality, which
/// keeps fewest rows, before the order comparison written ahead of it;
/// and gives the same rows by batches and row by row.
#[test]
fn a_filter_judges_its_selective_conjunct_first() {
    let (_db, s) = tables();
    let sql = "SELECT A FROM T WHERE A > 5 AND B = 7";
    let plan = s.explain(sql, &Params::new()).unwrap();
    let line = filter_line(&plan);
    let (b, a) = (line.find("B = 7").unwrap(), line.find("A > 5").unwrap());
    assert!(b < a, "{plan}");
    let (batched, by_rows) = both_ways(&s, sql);
    assert_eq!(batched.len(), 20);
    assert_eq!(by_rows, batched);
}

/// Verifies: [DESIGN] (RULES.md D650, D15) a filter one of whose
/// conjuncts can raise an error keeps the written order, and raises the
/// error of every row the standard judges it of.
#[test]
fn a_filter_that_can_raise_keeps_the_written_order() {
    let (_db, s) = tables();
    let sql = "SELECT A FROM T WHERE A > 5 AND 10 / C > 1 AND B = 7";
    let plan = s.explain(sql, &Params::new()).unwrap();
    let line = filter_line(&plan);
    let (a, b) = (line.find("A > 5").unwrap(), line.find("B = 7").unwrap());
    assert!(a < b, "{plan}");
    assert!(fetched(&s, sql).is_err());
}
