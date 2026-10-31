//! Access paths, cost models, optimizer rules and sort elimination. These are
//! `[DESIGN]` requirements: the standard does not govern plans, only results.

use std::sync::Arc;

use turborel::catalog::TableDesc;
use turborel::db::{Database, OpenOptions, Session};
use turborel::error::{self, Code};
use turborel::logical::{Cond, LogicalPlan, Truth};
use turborel::optimizer::{
    CONSTANT_FOLDING, MERGE_FILTERS, PUSH_FILTER_INTO_OUTER_JOIN, PUSH_FILTER_INTO_PRODUCT, PUSH_HAVING_INTO_WHERE,
    REMOVE_REDUNDANT_DISTINCT, Rule, RuleContext, SIMPLIFY_CONDITIONS,
};
use turborel::params::Params;
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::TxId;

use crate::common::{MemoryTable, Scratch, after_crash, big, code, rows, set_all_rules, setup};
use turborel::value::Spelled;

/// Verifies: [DESIGN] access path selection; [EXTENSION] indexes
#[test]
fn index_choice_matches_results() {
    let db = setup();
    let s = db.session("APP");
    // On a 3-row table a scan is cheapest; add rows so an index can win.
    for i in 10..60 {
        s.run(&format!("INSERT INTO T VALUES ({i}, 'N{i}', {i}, 0)")).unwrap();
    }
    let plan = s.explain("SELECT ID FROM T WHERE NAME = 'BOB'", &Params::new()).unwrap();
    assert!(plan.contains("IndexScan") && plan.contains("[hash]"), "{plan}");
    // A hash index cannot answer a range, so this must not use it.
    let plan = s.explain("SELECT ID FROM T WHERE NAME > 'A'", &Params::new()).unwrap();
    assert!(plan.contains("SeqScan") && !plan.contains("[hash]"), "{plan}");
    // Same answers whether or not an index is used.
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE NAME = 'BOB'"), Ok(vec!["2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE AMT < 0"), Ok(vec!["2".into()]));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE 0 > AMT"), Ok(vec!["2".into()]));
}

/// A memory table whose engine prices each fetch by id far above a scan.
struct CostlyFetch;
struct CostlyFetchTable(MemoryTable);

/// What `CostlyFetchTable` charges for one fetch by id.
const COSTLY_FETCH: f64 = 100.0;

impl StorageEngine for CostlyFetch {
    fn name(&self) -> &'static str {
        "costly_fetch"
    }
    fn create_table(&self, _: &TableDesc) -> error::Result<Box<dyn TableStorage>> {
        Ok(Box::new(CostlyFetchTable(MemoryTable::new())))
    }
}

impl TableStorage for CostlyFetchTable {
    fn insert(&self, tx: TxId, row: Row) -> error::Result<RowId> {
        self.0.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> error::Result<()> {
        self.0.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> error::Result<()> {
        self.0.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> error::Result<()> {
        self.0.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> error::Result<Option<Row>> {
        self.0.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.0.live_rows()
    }
    fn row_count(&self) -> usize {
        self.0.row_count()
    }
    fn fetch_cost(&self) -> f64 {
        COSTLY_FETCH
    }
}

/// Verifies: [DESIGN] engine cost models: the same query on the same data is
/// planned differently when the engine prices its operations differently, and
/// gives the same rows.
#[test]
fn engine_cost_model_changes_the_plan() {
    let db = Database::new();
    db.register_engine(Box::new(CostlyFetch)).unwrap();
    db.use_engine("costly_fetch").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE T (A INT, B INT, C INT, D INT)").unwrap();
    db.use_engine("rowstore").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION R CREATE TABLE T (A INT, B INT, C INT, D INT)").unwrap();
    for auth in ["C", "R"] {
        db.create_index(&format!("{auth}_A"), &format!("{auth}.T"), &["A"], "btree").unwrap();
        let s = db.session(auth);
        for i in 0..100 {
            s.run(&format!("INSERT INTO T VALUES ({i}, {i}, {i}, {i})")).unwrap();
        }
    }
    // 3 matching rows: cheap to fetch by id from the row store, but at the
    // costly engine's price, scanning all 100 rows is cheaper.
    let sql = "SELECT A FROM T WHERE A >= 97";
    assert!(db.session("R").explain(sql, &Params::new()).unwrap().contains("IndexScan"));
    assert!(db.session("C").explain(sql, &Params::new()).unwrap().contains("SeqScan"));
    let expected: Vec<String> = (97..100).map(|i: i32| i.to_string()).collect();
    for auth in ["R", "C"] {
        let mut got = rows(&db.session(auth), sql).unwrap();
        got.sort_by_key(|v| v.parse::<i32>().unwrap());
        assert_eq!(got, expected, "{auth}");
    }
}

/// Verifies: [DESIGN] optimizer rules preserve run-time errors; [X3.135-1986 §5.23 GR2, §5.18 GR2]
///
/// Regression test: HAVING was pushed into
/// WHERE and merged with it, so the HAVING condition ran on row 2, which WHERE
/// removes, and divided by zero. HAVING applies only to groups, and row 2 is in none.
#[test]
fn pushed_having_is_not_evaluated_on_rows_where_removed() {
    let db = setup();
    let query = "SELECT ID FROM T WHERE ID <> 2 GROUP BY ID HAVING 100 / (ID - 2) > 0";
    let expected = Ok(vec!["3".to_owned()]);
    assert_eq!(rows(&db.session("APP"), query), expected);
    set_all_rules(&db, false);
    assert_eq!(rows(&db.session("APP"), query), expected);
}

/// Runs `query` with every optimizer rule on, then off, and returns both outcomes.
fn with_and_without_rules(db: &Database, query: &str) -> [Result<Vec<String>, Code>; 2] {
    let with_rules = rows(&db.session("APP"), query);
    set_all_rules(db, false);
    let without_rules = rows(&db.session("APP"), query);
    set_all_rules(db, true);
    [with_rules, without_rules]
}

/// Verifies: [X3.135-1986 §5.18 GR2, §5.21 GR2, §5.23 GR2]; [DESIGN] RULES.md D15:
/// a condition that can raise an error is evaluated on every row or group its
/// clause applies to, whatever the plan, so the error never depends on the plan.
///
/// Regression test: an index probe, the
/// product pushdown and the HAVING pushdown each skipped rows that raise.
#[test]
fn run_time_errors_do_not_depend_on_the_plan() {
    let db = big();
    // Index probe: row 7 divides by zero even though only row 8 is selected.
    let probe = "SELECT ID FROM T WHERE ID = 8 AND 100 / (ID - 7) > 0";
    assert!(db.session("APP").explain(probe, &Params::new()).unwrap().contains("SeqScan"));
    assert!(db.session("APP").explain("SELECT ID FROM T WHERE ID = 8 AND NAME IS NOT NULL", &Params::new()).unwrap().contains("IndexScan"));
    // Product: X.ID > 99 removes every X row, but the product row (1, 1) divides by zero.
    let product = "SELECT X.ID FROM T X, U Y WHERE X.ID > 99 AND 100 / (X.ID - Y.ID) > 0";
    // HAVING: group K = 1 has one row and divides by zero, though K > 1 removes it.
    db.schema("CREATE SCHEMA AUTHORIZATION G CREATE TABLE G (K INTEGER)").unwrap();
    let g = db.session("G");
    for k in [1, 2, 2] {
        g.run(&format!("INSERT INTO G VALUES ({k})")).unwrap();
    }
    g.commit().unwrap();
    drop(g);
    let having = "SELECT K FROM G.G GROUP BY K HAVING K > 1 AND 100 / (COUNT(*) - 1) > 0";
    for query in [probe, product] {
        assert_eq!(with_and_without_rules(&db, query), [Err(Code::Data), Err(Code::Data)], "{query}");
    }
    let session = |db: &Database| db.session("G");
    assert_eq!(rows(&session(&db), having), Err(Code::Data));
}

/// Verifies: [DESIGN] EXPLAIN lists the rules that fired, collapsing a rule
/// that fired several times in a row: over three tables the filter is pushed
/// into the outer product, then into the inner one.
#[test]
fn explain_collapses_repeated_rules() {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE X (A INT) CREATE TABLE Y (B INT) CREATE TABLE Z (C INT)").unwrap();
    let plan = db.session("E").explain("SELECT A FROM X, Y, Z WHERE A = 1 AND B = 2 AND C = 3", &Params::new()).unwrap();
    assert!(plan.contains("-- rules: push_filter_into_product x2"), "{plan}");
}

/// Verifies: [DESIGN] filters whose conditions cannot raise an error are still
/// merged, so the planner sees one conjunction (the rewrite is kept where it is safe, RULES.md D15).
#[test]
fn safe_filters_are_still_merged() {
    let db = setup();
    let plan = db.session("APP").explain("SELECT ID FROM T WHERE ID <> 2 GROUP BY ID HAVING ID > 1", &Params::new()).unwrap();
    assert!(plan.contains(PUSH_HAVING_INTO_WHERE) && plan.contains(MERGE_FILTERS), "{plan}");
    let plan = db.session("APP").explain("SELECT ID FROM T WHERE ID <> 2 GROUP BY ID HAVING 100 / (ID - 2) > 0", &Params::new()).unwrap();
    assert!(plan.contains(PUSH_HAVING_INTO_WHERE) && !plan.contains(MERGE_FILTERS), "{plan}");
}

/// Verifies: [DESIGN] optimizer rules preserve results
#[test]
fn rules_rewrite_the_plan_without_changing_results() {
    let db = big();
    let s = db.session("APP");
    let queries = [
        "SELECT X.ID, Y.ID FROM T X, T Y WHERE X.ID = Y.ID AND X.ID < 3 AND Y.AMT > 1 + 1",
        "SELECT NAME, COUNT(*) FROM T GROUP BY NAME HAVING NAME > 'N5' AND COUNT(*) > 0",
        "SELECT DISTINCT ID, NAME FROM T WHERE 2 * 3 = 6 AND NOT (1 = 2) AND AMT = 3",
        "SELECT COUNT(*) FROM T HAVING 1 = 2",
    ];
    let mut with_rules = Vec::new();
    for q in queries {
        with_rules.push(rows(&s, q));
    }
    let plan = s.explain(queries[0], &Params::new()).unwrap();
    assert!(plan.contains(PUSH_FILTER_INTO_PRODUCT) && plan.contains(CONSTANT_FOLDING), "{plan}");
    assert!(s.explain(queries[1], &Params::new()).unwrap().contains(PUSH_HAVING_INTO_WHERE));
    let plan = s.explain(queries[2], &Params::new()).unwrap();
    assert!(plan.contains(REMOVE_REDUNDANT_DISTINCT) && plan.contains(SIMPLIFY_CONDITIONS), "{plan}");
    // Without GROUP BY a constant HAVING must stay above the single group: false, it
    // removes that group, and 0 groups give no row [ISO/IEC 9075:1992 §7.8 GR1-GR2,
    // §7.9 GR1b i]; pushed into WHERE, it would leave one group and give a row of 0.
    assert_eq!(with_rules[3], Ok(Vec::<String>::new()));
    drop(s);

    // Same answers with every rule turned off.
    set_all_rules(&db, false);
    let s = db.session("APP");
    for (q, expected) in queries.iter().zip(with_rules) {
        let mut a = rows(&s, q).unwrap();
        let mut b = expected.unwrap();
        a.sort();
        b.sort();
        assert_eq!(a, b, "{q}");
        assert!(s.explain(q, &Params::new()).unwrap().contains("-- rules: none"));
    }
}

/// Verifies: [DESIGN] constant folding; [X3.135-1986 §5.9 GR5, §5.21 GR2]
///
/// Corrected: the second query used to expect no error, because an
/// index probe skipped every row. WHERE applies to each row of T and AND is
/// evaluated in full (RULES.md D15), so it raises.
#[test]
fn folding_keeps_run_time_errors() {
    let db = setup();
    db.schema("CREATE SCHEMA AUTHORIZATION E CREATE TABLE E (K INTEGER)").unwrap();
    let s = db.session("APP");
    // 1/0 is not folded, so the error happens at run time, per row...
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID = 1 / 0"), Code::Data);
    // ...on every row, even when another conjunct is false for all of them...
    assert_eq!(code(&s, "SELECT ID FROM T WHERE ID > 99 AND ID = 1 / 0"), Code::Data);
    // ...and not at all when the table has no rows [X3.135-1986 §5.21 GR2].
    assert_eq!(rows(&db.session("E"), "SELECT K FROM E WHERE K = 1 / 0"), Ok(vec![]));
    // FALSE AND (error) still raises the error.
    assert_eq!(code(&s, "SELECT ID FROM T WHERE 1 = 2 AND ID / 0 = 1"), Code::Data);
}

/// A rule written outside the engine: it drops a filter whose condition is
/// known TRUE, over a table it finds in the catalog. Reading the catalog and
/// the row count through `RuleContext` is a regression test: those were once
/// private to the crate.
struct DropTrueFilters;
impl Rule for DropTrueFilters {
    fn name(&self) -> &'static str {
        "drop_filter_true_marker"
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond: Cond::Const(Truth::True), input } = plan else { return None };
        let LogicalPlan::Scan { table, .. } = &**input else { return None };
        ctx.db.catalog().table(table)?;
        ctx.db.row_count(table)?;
        Some((**input).clone())
    }
}

/// Verifies: [EXTENSION] optimizer `Rule` plug-ins, which can read the catalog
/// and row counts through `RuleContext`
#[test]
fn custom_rules_plug_in() {
    let db = setup();
    // Disable the built-in rule that would do the same job...
    assert!(db.with_optimizer(|o| o.set_enabled(SIMPLIFY_CONDITIONS, false)).unwrap());
    assert!(!db.with_optimizer(|o| o.set_enabled("no_such_rule", false)).unwrap());
    let s = db.session("APP");
    assert!(s.physical_plan("SELECT ID FROM T WHERE 1 = 1", &Params::new()).unwrap().contains("Filter TRUE"));
    drop(s);
    // ...and plug in our own.
    db.with_optimizer(|o| o.add(Box::new(DropTrueFilters))).unwrap();
    let s = db.session("APP");
    let plan = s.explain("SELECT ID FROM T WHERE 1 = 1", &Params::new()).unwrap();
    assert!(plan.contains("drop_filter_true_marker") && !plan.split("-- physical:").nth(1).unwrap().contains("Filter"), "{plan}");
}

/// Verifies: [DESIGN] sort elimination; [X3.135-1986 §8.3 GR3]
#[test]
fn sort_elimination_uses_ordered_indexes() {
    let db = big();
    let s = db.session("APP");
    let physical = |q: &str| s.physical_plan(q, &Params::new()).unwrap();
    // A btree probe on the sort column returns rows in order: no Sort.
    let p = physical("SELECT ID, NAME FROM T WHERE ID > 57 ORDER BY ID");
    assert!(!p.contains("Sort") && p.contains("ID > 57"), "{p}");
    let p = physical("SELECT ID FROM T WHERE ID > 57 ORDER BY ID DESC");
    assert!(!p.contains("Sort") && p.contains("backward"), "{p}");
    // A hash index gives no order, and NAME may be null (indexes hold no nulls).
    assert!(physical("SELECT NAME FROM T ORDER BY NAME").contains("Sort"));
    // Two keys, or a key that is not a plain column: still sorted.
    assert!(physical("SELECT ID, NAME FROM T ORDER BY ID, NAME").contains("Sort"));
    assert!(physical("SELECT ID + 1 FROM T ORDER BY 1").contains("Sort"));
    // Results are the same as with an explicit sort.
    let ordered = rows(&s, "SELECT ID FROM T WHERE ID > 57 ORDER BY ID DESC").unwrap();
    assert_eq!(ordered, (58..=60).rev().map(|i| i.to_string()).collect::<Vec<_>>());
    // Reading all 60 rows through the index costs more than scanning and sorting...
    assert!(physical("SELECT ID, NAME FROM T ORDER BY ID").contains("Sort"));
    drop(s);

    // ...unless sorting is expensive. The cost model is a setting, not a constant.
    db.with_cost_model(|c| c.sort_compare = 10.0).unwrap();
    let s = db.session("APP");
    let p = s.physical_plan("SELECT ID, NAME FROM T ORDER BY ID", &Params::new()).unwrap();
    assert!(!p.contains("Sort") && p.contains("all keys, in order"), "{p}");
    assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY ID").unwrap(), (1..=60).map(|i| i.to_string()).collect::<Vec<_>>());
}

/// Verifies: [X3.135-1986 §8.3 GR2b ii] UNION ALL appends the rows of both
/// operands, and EXPLAIN shows it.
#[test]
fn union_all_is_an_append() {
    let db = setup();
    let s = db.session("APP");
    assert!(s.explain("SELECT ID FROM T UNION ALL SELECT ID FROM T", &Params::new()).unwrap().contains("Append"));
    assert_eq!(rows(&s, "SELECT ID FROM T WHERE ID = 1 UNION ALL SELECT ID FROM T WHERE ID = 1"), Ok(vec!["1".into(), "1".into()]));
}

/// Verifies: [DESIGN] an index on an approximate column is probed with exact
/// and approximate constants, and gives the rows a scan would
/// ([X3.135-1986 §5.11 GR4] numbers compare by algebraic value).
#[test]
fn approximate_columns_are_probed_by_value() {
    let db = big();
    db.create_index("T_F", "APP.T", &["F"], "btree").unwrap();
    let s = db.session("APP");
    for sql in ["SELECT ID FROM T WHERE F = 2", "SELECT ID FROM T WHERE F = 2.0E0"] {
        assert!(s.explain(sql, &Params::new()).unwrap().contains("IndexScan"), "{sql}");
        assert_eq!(rows(&s, sql), Ok(vec!["2".into()]), "{sql}");
    }
}

/// Verifies: [DESIGN] a plan is shown with its parameters'
/// values, as OPEN takes them [X3.135-1986 §8.8 GR3a], so a parameter is
/// not read as a column, and an index can answer its comparison.
#[test]
fn plans_are_shown_with_their_parameters() {
    let db = setup();
    let s = db.session("APP");
    for i in 10..60 {
        s.run(&format!("INSERT INTO T VALUES ({i}, 'N{i}', {i}, 0)")).unwrap();
    }
    let p = Params::new().declare(
        "P",
        turborel::value::DataType::Exact { precision: 6, scale: 2, spelled: Spelled::Numeric },
        crate::common::number(-1),
    );
    let plan = s.physical_plan("SELECT ID FROM T WHERE AMT < P", &p).unwrap();
    assert!(plan.contains("IndexScan") && plan.contains("AMT < -1"), "{plan}");
    assert!(s.explain("SELECT ID FROM T WHERE AMT < P", &p).unwrap().contains("AMT < -1"));
    assert_eq!(s.explain("SELECT ID FROM T WHERE AMT < P", &Params::new()).map_err(|e| e.code), Err(Code::SyntaxRule), "P undeclared");
}

/// Schema M: table M of 400 rows, A and B NOT NULL and C nullable, with a
/// B-link index on (A, B) and a hash index on (A, C); table N, those rows'
/// A and C, with a B-link index on (A, C) only.
fn two_column_indexes() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION M
           CREATE TABLE M (A INTEGER NOT NULL, B INTEGER NOT NULL, C INTEGER, V INTEGER)
           CREATE TABLE N (A INTEGER NOT NULL, C INTEGER)",
    )
    .unwrap();
    let s = db.session("M");
    for k in 0..400 {
        let c = if k % 5 == 0 { "NULL".to_string() } else { (k % 7).to_string() };
        s.run(&format!("INSERT INTO M VALUES ({}, {}, {c}, {k})", k % 20, (k * 7) % 23)).unwrap();
        s.run(&format!("INSERT INTO N VALUES ({}, {c})", k % 20)).unwrap();
    }
    s.commit().unwrap();
    db.create_index("M_AB", "M.M", &["A", "B"], "btree").unwrap();
    db.create_index("M_AC", "M.M", &["A", "C"], "hash").unwrap();
    db.create_index("N_AC", "M.N", &["A", "C"], "btree").unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D34, indexes on several columns answer
/// equalities of their first columns, then a comparison of the next: a
/// B-link index a prefix, a hash index only its whole key; the rows are
/// those of a scan (`+ 0` keeps a comparison from any index). An index
/// holds no key with a null, so one is not probed on fewer columns than it
/// has unless the others are NOT NULL: N's rows whose C is null are found.
#[test]
fn indexes_on_several_columns_answer_prefixes() {
    let db = two_column_indexes();
    let s = db.session("M");
    let cases = [
        ("SELECT V FROM M WHERE A = 3 AND B = 21", "SELECT V FROM M WHERE A + 0 = 3 AND B = 21", "M_AB"),
        ("SELECT V FROM M WHERE B > 15 AND A = 3", "SELECT V FROM M WHERE B > 15 AND A + 0 = 3", "M_AB"),
        ("SELECT V FROM M WHERE A = 3", "SELECT V FROM M WHERE A + 0 = 3", "M_AB"),
        ("SELECT V FROM M WHERE C = 2 AND A = 3", "SELECT V FROM M WHERE C = 2 AND A + 0 = 3", "M_AC"),
    ];
    for (sql, scan, index) in cases {
        let plan = s.physical_plan(sql, &Params::new()).unwrap();
        assert!(plan.contains(&format!("using {index}")), "{sql}: {plan}");
        assert_eq!(crate::common::sorted(&s, sql), crate::common::sorted(&s, scan), "{sql}");
    }
    let sql = "SELECT C FROM N WHERE A = 5";
    assert!(!s.physical_plan(sql, &Params::new()).unwrap().contains("N_AC"), "C may be null");
    assert_eq!(crate::common::count(&s, sql), 20);
}

/// Verifies: [DESIGN] RULES.md D35, [X3.135-1986 §8.3 GR3] sort elimination
/// for several keys: an index on (A, B) gives ORDER BY A, B, both
/// descending read backward, and B alone where A is held equal, and the
/// groups of GROUP BY A in A's order; keys of mixed directions are sorted.
/// The rows are those of a sort.
#[test]
fn indexes_give_orders_of_several_keys() {
    let db = two_column_indexes();
    db.with_cost_model(|c| c.sort_compare = 10.0).unwrap();
    let s = db.session("M");
    let cases = [
        ("SELECT A, B, V FROM M ORDER BY A, B", "SELECT A, B, V FROM M ORDER BY A, B, V"),
        ("SELECT A, B, V FROM M ORDER BY A DESC, B DESC", "SELECT A, B, V FROM M ORDER BY A DESC, B DESC, V DESC"),
        ("SELECT B, V FROM M WHERE A = 4 ORDER BY B", "SELECT B, V FROM M WHERE A + 0 = 4 ORDER BY B, V"),
        ("SELECT A, B FROM M WHERE A = 4 AND B > 3 ORDER BY A, B DESC", "SELECT A, B FROM M WHERE A + 0 = 4 AND B > 3 ORDER BY B DESC"),
        ("SELECT A, COUNT(*), MAX(B) FROM M GROUP BY A ORDER BY A", "SELECT A, COUNT(*), MAX(B + 0) FROM M GROUP BY A ORDER BY A"),
    ];
    for (sql, sorted) in cases {
        let plan = s.physical_plan(sql, &Params::new()).unwrap();
        assert!(!plan.contains("Sort") && plan.contains("M_AB"), "{sql}: {plan}");
        let distinct_keys = !sql.contains(", V");
        if distinct_keys {
            assert_eq!(rows(&s, sql), rows(&s, sorted), "{sql}");
        } else {
            assert_eq!(crate::common::sorted(&s, sql), crate::common::sorted(&s, sorted), "{sql}");
        }
    }
    assert!(s.physical_plan("SELECT A, B FROM M ORDER BY A, B DESC", &Params::new()).unwrap().contains("Sort"), "mixed directions");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.5 GR1c, GR5]; [DESIGN]
/// RULES.md D15: conditions move into the operands of an outer join where
/// they keep the same rows: a WHERE conjunct over the preserved operand of
/// a LEFT or RIGHT join; an ON conjunct over the operand given nulls; a
/// WHERE conjunct not true of that operand's nulls makes the join inner, or
/// a FULL one one-sided. A conjunct over both operands, one true of nulls,
/// an ON conjunct over the preserved operand, and a condition that can
/// raise an error stay. Each query gives what it gives with no rule.
#[test]
fn conditions_move_into_outer_joins_7_5() {
    let db = setup();
    let s = db.session("APP");
    let lj = "SELECT T.ID, CODE FROM T LEFT JOIN U ON T.ID = U.ID";
    let cases: [(String, &[&str], &str); 12] = [
        (format!("{lj} WHERE T.ID > 1"), &["2,NULL", "3,NULL"], "LeftOuter"),
        (format!("{lj} WHERE U.CODE = 'B'"), &["1,B  "], "IndexNestedLoop APP.T"),
        (format!("{lj} WHERE U.CODE IS NULL"), &["2,NULL", "3,NULL"], "LeftOuterJoin"),
        (format!("{lj} WHERE COALESCE(U.CODE, 'Z') = 'Z'"), &["2,NULL", "3,NULL"], "LeftOuterJoin"),
        (format!("{lj} WHERE T.ID = 1 OR U.CODE = 'B'"), &["1,A  ", "1,B  "], "LeftOuterJoin"),
        (format!("{lj} AND U.CODE = 'A'"), &["1,A  ", "2,NULL", "3,NULL"], "LeftOuterJoin"),
        (format!("{lj} AND T.ID = 2"), &["1,NULL", "2,NULL", "3,NULL"], "LeftOuterJoin"),
        ("SELECT T.ID, CODE FROM U RIGHT JOIN T ON T.ID = U.ID WHERE T.ID < 2".into(), &["1,A  ", "1,B  "], "RightOuterJoin"),
        ("SELECT T.ID, CODE FROM T FULL JOIN U ON T.ID = U.ID + 1 WHERE U.CODE = 'A'".into(), &["2,A  "], "RightOuterJoin"),
        ("SELECT T.ID, CODE FROM T FULL JOIN U ON T.ID = U.ID + 1 WHERE U.CODE = 'A' AND T.ID = 2".into(), &["2,A  "], "NestedLoop"),
        ("SELECT T.ID, CODE FROM T FULL JOIN U ON T.ID = U.ID + 1 WHERE T.ID = 2".into(), &["2,A  ", "2,B  "], "LeftOuterJoin"),
        ("SELECT T.ID, CODE FROM T FULL JOIN U ON T.ID = U.ID + 1 WHERE COALESCE(T.ID, 0) < 2".into(), &["1,NULL"], "FullOuterJoin"),
    ];
    for (sql, want, operator) in &cases {
        let [with, without] = with_and_without_rules(&db, sql);
        let mut found = with.unwrap();
        found.sort();
        assert_eq!(found, want.to_vec(), "{sql}");
        let mut unoptimized = without.unwrap();
        unoptimized.sort();
        assert_eq!(unoptimized, found, "{sql}: as with no rule");
        let plan = s.explain(sql, &Params::new()).unwrap();
        assert!(plan.contains(operator), "{sql}: {plan}");
    }
    let moved = |sql: &str| s.explain(sql, &Params::new()).unwrap().contains(PUSH_FILTER_INTO_OUTER_JOIN);
    assert!(moved(&cases[0].0) && moved(&cases[1].0) && moved(&cases[5].0) && moved(&cases[8].0));
    assert!(!moved(&cases[2].0) && !moved(&cases[3].0) && !moved(&cases[4].0) && !moved(&cases[6].0));
    let raising = format!("{lj} WHERE 1 / (T.ID - 2) > 0");
    assert_eq!(with_and_without_rules(&db, &raising), [Err(Code::Data), Err(Code::Data)]);
    assert!(!moved(&raising), "D15");
}

/// A database with S.T of `rows` rows: K a key, A and B both K mod 10, with
/// a B-link index and a hash index on K, and a B-link index on (A, B).
fn indexed_both_ways(rows: i64) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, A INTEGER NOT NULL, B INTEGER NOT NULL)").unwrap();
    let s = db.session("S");
    for from in (0..rows).step_by(500) {
        let values: Vec<String> = (from..(from + 500).min(rows)).map(|k| format!("({k}, {}, {})", k % 10, k % 10)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
    db.create_index("T_K_TREE", "S.T", &["K"], "btree").unwrap();
    db.create_index("T_K_HASH", "S.T", &["K"], "hash").unwrap();
    db.create_index("T_AB", "S.T", &["A", "B"], "btree").unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D35, index statistics (`Index::probe_pages`,
/// `Index::distinct_keys`): an equality is probed in the index that finds
/// its entries in fewer pages, a hash table's one bucket over a B-link
/// tree's levels; and an equality on all of an index's columns, its rows
/// past what the index counts at once, is estimated as the table's rows
/// over the index's distinct keys, not as if its columns were independent.
#[test]
fn index_statistics_choose_and_estimate() {
    let db = indexed_both_ways(20_000);
    let s = db.session("S");
    let plan = s.explain("SELECT A FROM T WHERE K = 1234", &Params::new()).unwrap();
    assert!(plan.contains("using T_K_HASH [hash]"), "{plan}");
    assert_eq!(rows(&s, "SELECT A FROM T WHERE K = 1234"), Ok(vec!["4".into()]));
    // A = B in every row: 2,000 rows have A = 3 and B = 3, of ten keys in
    // all; as if independent, a tenth of a tenth, 200. Asked in the
    // index's order, which the index gives as it probes.
    // The keys counted by a sketch, so near enough.
    let est = ab_estimate(&s, "SELECT K, A, B FROM T WHERE A = 3 AND B = 3 ORDER BY A, B");
    assert!((est - 2000.0).abs() < 100.0, "{est}");
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE A = 3 AND B = 3"), Ok(vec!["2000".into()]));
}

/// The estimated rows of the scan of `T_AB` that `sql` plans, in `s`.
fn ab_estimate(s: &Session, sql: &str) -> f64 {
    let plan = s.explain(sql, &Params::new()).unwrap();
    let est = plan.lines().find(|l| l.contains("using T_AB")).unwrap_or_else(|| panic!("{plan}"));
    est.split("est. rows=").nth(1).and_then(|r| r.split(',').next()).and_then(|n| n.parse::<f64>().ok()).unwrap_or_else(|| panic!("{est}"))
}

/// Inserts rows `from..to` of `T` in `s`, `A` and `B` both the row's
/// number modulo `keys`, and commits.
fn insert_keyed(s: &Session, (from, to): (i64, i64), keys: i64) {
    for start in (from..to).step_by(500) {
        let values: Vec<String> = (start..(start + 500).min(to)).map(|k| format!("({k}, {}, {})", k % keys, k % keys)).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D35, an index's distinct keys kept with it
/// (`Index::locator`): reopened after a clean close, an index estimates as
/// it did; recovered after a crash from a checkpoint, it estimates of the
/// keys the checkpoint kept and those recovery mended it with since.
#[test]
fn index_statistics_are_kept_with_the_index() {
    let scratch = Scratch::new("planning-kept-statistics");
    // Asked in the index's order, which the index gives as it probes.
    let sql = "SELECT K, A, B FROM T WHERE A = 3 AND B = 3 ORDER BY A, B";
    let db = Database::new();
    db.open_files(&after_crash(), &scratch.dir, &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER NOT NULL, A INTEGER NOT NULL, B INTEGER NOT NULL)").unwrap();
    db.create_index("T_AB", "S.T", &["A", "B"], "btree").unwrap();
    insert_keyed(&db.session("S"), (0, 10_000), 10);
    let before = ab_estimate(&db.session("S"), sql);
    assert!((before - 1000.0).abs() < 50.0, "{before}");
    db.close().unwrap();

    let db = Database::new();
    db.open_files(&after_crash(), &scratch.dir, &OpenOptions::default()).unwrap();
    let s = db.session("S");
    assert_eq!(ab_estimate(&s, sql), before, "as before the close");
    db.checkpoint().unwrap();
    // Ten keys more after the checkpoint, then a crash.
    insert_keyed(&s, (10_000, 20_000), 20);
    drop(s);
    drop(db);

    let db = Database::new();
    db.open_files(&after_crash(), &scratch.dir, &OpenOptions::default()).unwrap();
    let after = ab_estimate(&db.session("S"), sql);
    assert!((after - 1000.0).abs() < 50.0, "20,000 rows of 20 keys: {after}");
    assert_eq!(rows(&db.session("S"), "SELECT COUNT(*) FROM T WHERE A = 3 AND B = 3"), Ok(vec!["1500".into()]));
}
