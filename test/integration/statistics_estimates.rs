//! Estimates from the statistics of RULES.md D381 and D382: a column's
//! most common values, a table's sample of rows for a conjunction whose
//! columns go together, a LIKE's prefix, an index's range by its share of
//! the tree, and bounds after rows are written past them. These are `[DESIGN]`
//! requirements on the plan; the rows are the same whatever it estimates.

use turborel::db::{Database, Session};
use turborel::params::Params;
use turborel::threads::JobPolicy;

/// The rows the first estimate of `sql`'s physical plan, its filter's or
/// its read's, says it keeps, and the plan.
fn estimated(s: &Session, sql: &str) -> (f64, String) {
    let plan = s.physical_plan(sql, &Params::new()).unwrap();
    let est = plan.split("est. rows=").nth(1).unwrap().split([')', ',']).next().unwrap().parse().unwrap();
    (est, plan)
}

/// Whether `got` is within `by` times of `want`.
fn within(got: f64, want: f64, by: f64) -> bool {
    got <= want * by && want <= got * by
}

/// A database of the schema APP and its background work done only when
/// a test asks (JOBS.POLICY MANUAL, RULES.md D55), with `sql` run.
fn database(schema: &str, rows: &[(&str, Vec<String>)]) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(schema).unwrap();
    let s = db.session("APP");
    for (table, values) in rows {
        for chunk in values.chunks(500) {
            s.run(&format!("INSERT INTO {table} VALUES {}", chunk.join(", "))).unwrap();
        }
    }
    s.commit().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D381: of a column whose one value half the
/// rows hold, an equality with it is estimated near its share, and the
/// index on the column is not read for it but a scan; an equality with a
/// rare value is estimated near its rows and reads the index. MC/DC of
/// the common list: a value on it (its share) against one not (the rest
/// shared).
#[test]
fn a_common_value_is_estimated_by_its_share_and_scanned() {
    let rows: Vec<String> =
        (0..20_000).map(|i| format!("({i}, '{}')", if i % 2 == 0 { "HOT".to_owned() } else { format!("V{i}") })).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER, S VARCHAR(10))", &[("T", rows)]);
    db.create_index("T_S", "APP.T", &["S"], "btree").unwrap();
    db.maintain().unwrap();
    let s = db.session("APP");
    let (hot, plan) = estimated(&s, "SELECT ID FROM T WHERE S = 'HOT'");
    assert!(within(hot, 10_000.0, 1.2), "{hot}\n{plan}");
    assert!(plan.contains("SeqScan") && !plan.contains("IndexScan"), "{plan}");
    let (rare, plan) = estimated(&s, "SELECT ID FROM T WHERE S = 'V7'");
    assert!(rare < 5.0 && plan.contains("IndexScan"), "{rare}\n{plan}");
}

/// The database of APP.C, 20,000 rows: A of 0 to 999 in turn, B = A mod
/// 10 and D = A mod 100, so B goes with D, and T a tag of the row's
/// number.
fn correlated() -> Database {
    let rows = (0..20_000).map(|i| format!("({}, {}, {}, 'tag{}x{}')", i % 1_000, i % 1_000 % 10, i % 100, i % 37, i % 11)).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE C (A INTEGER, B INTEGER, D INTEGER, T VARCHAR(20))", &[("C", rows)]);
    db.maintain().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D382: a conjunction of two columns that go
/// together is estimated on the table's sample near the rows it keeps,
/// not at the product of each column's share, ten times fewer; a
/// conjunction the sample keeps no row of is estimated at few rows; a
/// comparison of two columns and a pattern with a leading wildcard, which
/// no column's statistics answer, are estimated on the sample too.
#[test]
fn a_correlated_conjunction_is_estimated_on_the_sample() {
    let db = correlated();
    let s = db.session("APP");
    let (both, plan) = estimated(&s, "SELECT * FROM C WHERE D = 5 AND B = 5");
    assert!(within(both, 200.0, 2.0), "{both}\n{plan}");
    let (none, _) = estimated(&s, "SELECT * FROM C WHERE A = 5 AND B = 6");
    assert!(none <= 3.0 / 1_024.0 * 20_000.0, "{none}");
    let (columns, _) = estimated(&s, "SELECT * FROM C WHERE A = B");
    assert!(within(columns, 200.0, 2.0), "{columns}");
    let (pattern, _) = estimated(&s, "SELECT * FROM C WHERE T LIKE '%x7'");
    assert!(within(pattern, 20_000.0 / 11.0, 1.5), "{pattern}");
}

/// Verifies: [DESIGN] RULES.md D381, LIKE: a pattern of a constant prefix
/// is estimated as the range of strings beginning with it, near the rows
/// it keeps, not a third of the table.
#[test]
fn a_like_of_a_prefix_is_estimated_as_its_range() {
    let db = correlated();
    let s = db.session("APP");
    let (prefix, plan) = estimated(&s, "SELECT * FROM C WHERE T LIKE 'tag3%'");
    // tag3 and tag30 to tag36: 8 of 37 tags.
    assert!(within(prefix, 20_000.0 * 8.0 / 37.0, 1.5), "{prefix}\n{plan}");
}

/// Verifies: [DESIGN] RULES.md D381, an index's range: a range of a
/// third of a 30,000-row table's keys is estimated by its share of the
/// index's tree near its rows, where a count stopped at 1,000 entries.
#[test]
fn an_index_range_is_estimated_by_its_share_of_the_tree() {
    let rows = (0..30_000).map(|i| format!("({i}, {})", i % 7)).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE R (ID INTEGER, V INTEGER)", &[("R", rows)]);
    db.create_index("R_ID", "APP.R", &["ID"], "btree").unwrap();
    db.maintain().unwrap();
    let s = db.session("APP");
    let (range, plan) = estimated(&s, "SELECT ID FROM R WHERE ID BETWEEN 10000 AND 19999 ORDER BY ID");
    assert!(within(range, 10_000.0, 1.5), "{range}\n{plan}");
}

/// Verifies: [DESIGN] RULES.md D381, bounds after change: once rows are
/// written past a table's greatest value and before its statistics are
/// gathered again, a range past it is estimated at some rows, not none.
#[test]
fn a_range_past_the_greatest_value_keeps_rows_once_written() {
    let rows = (0..1_000).map(|i| format!("({i}, {})", i % 7)).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE G (ID INTEGER, V INTEGER)", &[("G", rows)]);
    db.maintain().unwrap();
    let s = db.session("APP");
    let past = "SELECT * FROM G WHERE ID > 1500";
    assert!(estimated(&s, past).0 < 1.0, "none past the greatest, unchanged");
    s.run("INSERT INTO G VALUES (2000, 1)").unwrap();
    s.commit().unwrap();
    assert!(estimated(&s, past).0 >= 1.0, "once written");
}

/// Verifies: [DESIGN] RULES.md D381, growth: of a key whose greatest
/// value rose at each of the last two passes that found it changed, a
/// pass between them finding the same, a range past it, once rows
/// are written since, is estimated at the share written since, near the
/// rows it keeps.
#[test]
fn a_growing_keys_range_past_its_greatest_keeps_the_rows_written_since() {
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE G (ID INTEGER, V INTEGER)", &[]);
    let s = db.session("APP");
    let insert = |from: u32, to: u32| {
        let rows: Vec<String> = (from..to).map(|i| format!("({i}, {})", i % 7)).collect();
        for chunk in rows.chunks(500) {
            s.run(&format!("INSERT INTO G VALUES {}", chunk.join(", "))).unwrap();
        }
        s.commit().unwrap();
    };
    for round in 0..3_u32 {
        insert(round * 2_000, (round + 1) * 2_000);
        // A second pass soon after, as maintenance and ANALYZE may make.
        db.analyze("APP.G").unwrap();
        db.analyze("APP.G").unwrap();
    }
    insert(6_000, 6_500);
    let (past, plan) = estimated(&s, "SELECT * FROM G WHERE ID >= 6000");
    assert!(within(past, 500.0, 2.0), "{past}\n{plan}");
}

/// Verifies: [DESIGN] RULES.md D381, LIKE: a prefix narrower than a
/// histogram's bucket keeps its part of the bucket, placed by its
/// characters; a pattern with `_` after its prefix keeps the share of the
/// sampled strings with the prefix that it keeps, near its rows, not a
/// quarter of the prefix's.
#[test]
fn narrow_prefixes_and_single_character_wildcards_are_estimated() {
    let rows = (0..20_000).map(|i| format!("({i}, 'name{}')", i % 2_000)).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE N (ID INTEGER, S VARCHAR(20))", &[("N", rows)]);
    db.maintain().unwrap();
    let s = db.session("APP");
    // name123 and name1230 to name1239: 11 values of 10 rows each.
    let (narrow, plan) = estimated(&s, "SELECT * FROM N WHERE S LIKE 'name123%'");
    assert!(within(narrow, 110.0, 3.0), "{narrow}\n{plan}");
    // name409 to name499 by tens: 10 values.
    let (wild, plan) = estimated(&s, "SELECT * FROM N WHERE S LIKE 'name4_9'");
    assert!(within(wild, 100.0, 2.5), "{wild}\n{plan}");
}

/// Verifies: [DESIGN] RULES.md D382, a small table: of a table its sample
/// holds whole, a conjunction of columns that never go together is
/// estimated to keep no row, and one that keeps a few rows near them.
#[test]
fn a_small_tables_conjunction_is_counted_on_its_sample() {
    let rows = (0..800).map(|i| format!("({i}, {}, {})", i % 8, i % 40)).collect();
    let db = database("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE P (ID INTEGER, B INTEGER, S INTEGER)", &[("P", rows)]);
    db.maintain().unwrap();
    let s = db.session("APP");
    // S mod 8 is B, so B = 1 and S < 1 never hold together.
    let (none, plan) = estimated(&s, "SELECT * FROM P WHERE B = 1 AND S < 1");
    assert!(none < 1.0, "{none}\n{plan}");
    let (few, plan) = estimated(&s, "SELECT * FROM P WHERE B = 1 AND S < 10");
    // S of 1 and 9: 2 of its 40 values.
    assert!(within(few, 40.0, 1.2), "{few}\n{plan}");
}
