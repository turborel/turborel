//! Window functions refused and placed [ISO/IEC 9075-2:2023 §6.10, §7.15,
//! §9.23, §9.16]: each Syntax Rule, ROW_NUMBER and a window of ROWS in a
//! constraint, the words a window is written with still names elsewhere;
//! views over windows, filtered after them; a partition past the work area,
//! spilled, row by row and by batches; and a window over a column store
//! kept in its order, read without a sort.

use turborel::db::Session;
use turborel::error::Code;
use turborel::params::Params;

use crate::common::{Counted, both_ways, exec, fetched, rows, sorted};
use crate::windows::sales;

/// The SQLCODE of `failed`, and whether its error cites `rule`.
fn cites<T: std::fmt::Debug>(failed: Result<T, turborel::error::SqlError>, rule: &str) -> (Code, bool) {
    let e = failed.expect_err("a failure");
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// The SQLCODE of the query `sql`, and whether its error cites `rule`.
fn refused(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    cites(fetched(a, sql), rule)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.10 SR2b, §10.9 SR7a, §7.15
/// SR7, §9.23 SR3] a window function is only in a select list: not in
/// WHERE, HAVING, a set function's argument or a window's keys; it holds
/// no outer reference and no query expression. Each is SQLCODE -103.
#[test]
fn window_functions_are_only_in_a_select_list_6_10() {
    let (_db, a) = sales();
    let cases = [
        ("SELECT D FROM S WHERE RANK() OVER (ORDER BY D) = 1", "§6.10 SR2b"),
        ("SELECT D FROM S GROUP BY D HAVING RANK() OVER (ORDER BY D) = 1", "§6.10 SR2b"),
        ("SELECT SUM(RANK() OVER (ORDER BY D)) FROM S", "§10.9 SR7a"),
        ("SELECT RANK() OVER (ORDER BY RANK() OVER (ORDER BY D)) FROM S", "§7.15 SR7"),
        ("SELECT RANK() OVER W FROM S WINDOW W AS (ORDER BY SUM(D) OVER ())", "§7.15 SR7"),
        ("SELECT (SELECT MAX(T.D) OVER (ORDER BY S.D) FROM S AS T) FROM S", "§9.23 SR3"),
        ("SELECT SUM(D) OVER (ORDER BY (SELECT MAX(D) FROM S AS T)) FROM S", "§9.23 SR3"),
    ];
    for (sql, rule) in cases {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 SR2, SR4, SR5a, SR6,
/// SR12a, SR12c, SR12d, SR12e, SR16] window names are unique and in scope;
/// a window over an existing one adds no PARTITION BY, no ORDER BY to one
/// with an ordering, and the existing one has no frame; a partitioning
/// column is a column of the table expression, COLLATE only of a string,
/// and of a grouped query a grouping column. Each is SQLCODE -103.
#[test]
fn windows_are_defined_once_and_named_in_scope_7_15() {
    let (_db, a) = sales();
    let cases = [
        ("SELECT RANK() OVER W FROM S WINDOW W AS (ORDER BY D), W AS (ORDER BY R)", "§7.15 SR2"),
        ("SELECT RANK() OVER X FROM S", "§7.15 SR16"),
        ("SELECT RANK() OVER W FROM S WINDOW V AS (W), W AS (ORDER BY D)", "§7.15 SR12a"),
        ("SELECT RANK() OVER (W PARTITION BY R) FROM S WINDOW W AS (ORDER BY D)", "§7.15 SR12c"),
        ("SELECT RANK() OVER (W ORDER BY R) FROM S WINDOW W AS (ORDER BY D)", "§7.15 SR12d"),
        ("SELECT SUM(D) OVER (W) FROM S WINDOW W AS (ORDER BY D ROWS UNBOUNDED PRECEDING)", "§7.15 SR12e"),
        ("SELECT (SELECT COUNT(*) OVER (PARTITION BY S.R) FROM S AS T) FROM S", "§7.15 SR4"),
        ("SELECT COUNT(*) OVER (PARTITION BY D COLLATE UCS_BASIC) FROM S", "§7.15 SR5a"),
        ("SELECT COUNT(*) OVER (PARTITION BY D) FROM S GROUP BY R", "§7.15 SR6"),
    ];
    for (sql, rule) in cases {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
    assert_eq!(rows(&a, "SELECT SUM(D) OVER W FROM S WINDOW W AS (ORDER BY D ROWS UNBOUNDED PRECEDING)").map(|r| r.len()), Ok(7));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 SR10, SR15, §6.10] a
/// frame starts not UNBOUNDED FOLLOWING and ends not UNBOUNDED PRECEDING,
/// from CURRENT ROW not PRECEDING and from FOLLOWING only FOLLOWING; an
/// offset is a literal or a parameter, of ROWS and GROUPS exact of scale
/// 0, of RANGE of one numeric, datetime or interval key, numeric of a
/// number, else an interval; GROUPS has an ordering. Each is SQLCODE -103;
/// a start alone that follows, or an offset signed, -102.
#[test]
fn frames_are_well_bounded_7_15() {
    let (_db, a) = sales();
    let over = |frame: &str| format!("SELECT SUM(AMOUNT) OVER (ORDER BY D {frame}) FROM S");
    let cases = [
        (over("ROWS BETWEEN UNBOUNDED FOLLOWING AND CURRENT ROW"), "§7.15 SR10a"),
        (over("ROWS BETWEEN CURRENT ROW AND UNBOUNDED PRECEDING"), "§7.15 SR10b"),
        (over("ROWS BETWEEN CURRENT ROW AND 1 PRECEDING"), "§7.15 SR10c"),
        (over("RANGE BETWEEN 1 FOLLOWING AND CURRENT ROW"), "§7.15 SR10d"),
        (over("ROWS BETWEEN 2 FOLLOWING AND 1 PRECEDING"), "§7.15 SR10d"),
        (over("ROWS 1.5 PRECEDING"), "§7.15 SR15"),
        (over("GROUPS 'a' PRECEDING"), "§7.15 SR15"),
        ("SELECT SUM(D) OVER (ORDER BY D, R RANGE 1 PRECEDING) FROM S".to_owned(), "§7.15 SR15a ii"),
        ("SELECT SUM(D) OVER (ORDER BY R RANGE 1 PRECEDING) FROM S".to_owned(), "§7.15 SR15a iii"),
        (over("RANGE INTERVAL '1' DAY PRECEDING"), "§7.15 SR15a iii"),
        ("SELECT SUM(D) OVER (GROUPS 1 PRECEDING) FROM S".to_owned(), "§7.15 SR15c i"),
    ];
    for (sql, rule) in &cases {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
    for syntax in [over("ROWS 1 FOLLOWING"), over("ROWS UNBOUNDED FOLLOWING"), over("ROWS -1 PRECEDING"), over("ROWS D PRECEDING")] {
        assert_eq!(fetched(&a, &syntax).map_err(|e| e.code), Err(Code::Syntax), "{syntax}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.23 SR5a, SR5b, SR8d, §6.10;
/// T627] a rank function's window has an ordering; neither it nor
/// ROW_NUMBER has a frame; over a window of an ordering or a frame a set
/// function is not LISTAGG and is DISTINCT only of COUNT, Feature T627,
/// whose values are counted; OVER follows a set function or a rank
/// function alone, else -102. The rest are SQLCODE -103.
#[test]
fn window_function_types_fit_their_windows_9_23() {
    let (_db, a) = sales();
    let cases = [
        ("SELECT RANK() OVER () FROM S", Code::SyntaxRule, "§9.23 SR5a"),
        ("SELECT CUME_DIST() OVER (PARTITION BY R) FROM S", Code::SyntaxRule, "§9.23 SR5a"),
        ("SELECT ROW_NUMBER() OVER (ORDER BY D ROWS UNBOUNDED PRECEDING) FROM S", Code::SyntaxRule, "§9.23 SR5b"),
        ("SELECT DENSE_RANK() OVER (ORDER BY D RANGE UNBOUNDED PRECEDING) FROM S", Code::SyntaxRule, "§9.23 SR5b"),
        ("SELECT LISTAGG(R, ',') WITHIN GROUP (ORDER BY D) OVER (ORDER BY D) FROM S", Code::SyntaxRule, "§9.23 SR8d i"),
        ("SELECT SUM(DISTINCT D) OVER (ORDER BY D) FROM S", Code::SyntaxRule, "§9.23 SR8d ii"),
    ];
    for (sql, code, rule) in cases {
        assert_eq!(refused(&a, sql, rule), (code, true), "{sql}");
    }
    assert_eq!(sorted(&a, "SELECT COUNT(DISTINCT D) OVER () FROM S WHERE R = 's'"), ["2", "2"]);
    assert_eq!(sorted(&a, "SELECT COUNT(DISTINCT D) OVER (ORDER BY D ROWS 1 PRECEDING) FROM S WHERE R = 'n'"), ["1", "1", "2", "2"]);
    assert_eq!(fetched(&a, "SELECT ABS(D) OVER () FROM S").map_err(|e| e.code), Err(Code::Syntax), "OVER after no set function");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16 SR1c, §11.9 CR2]
/// ROW_NUMBER and a window function over a window of ROWS are potential
/// sources of non-determinism, so neither a CHECK nor an assertion may
/// contain one: SQLCODE -103. RANK and a set function over a window of
/// RANGE may.
#[test]
fn nondeterministic_window_functions_constrain_nothing_9_16() {
    let (_db, a) = sales();
    let refused_calls = [
        "ROW_NUMBER() OVER (ORDER BY D)",
        "SUM(D) OVER (ORDER BY D ROWS UNBOUNDED PRECEDING)",
        "MAX(D) OVER (ORDER BY D ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)",
    ];
    for call in refused_calls {
        let assertion = format!("CREATE ASSERTION N CHECK ((SELECT MAX(X) FROM (SELECT {call} FROM S) AS Q (X)) < 100)");
        assert_eq!(cites(a.run(&assertion), "§9.16 SR1c"), (Code::SyntaxRule, true), "{assertion}");
        let check = format!("CREATE TABLE C (X INTEGER CHECK (X < (SELECT MAX(Y) FROM (SELECT {call} FROM S) AS Q (Y))))");
        assert_eq!(cites(a.run(&check), "§9.16 SR1c"), (Code::SyntaxRule, true), "{check}");
    }
    let allowed = "CREATE ASSERTION N CHECK ((SELECT MAX(X) FROM (SELECT RANK() OVER (ORDER BY D) + SUM(D) OVER (ORDER BY D) FROM S) AS Q (X)) < 100)";
    a.run(allowed).unwrap();
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §6.10, §7.15] the words
/// a window is written with are read so only in context, so each names a
/// column elsewhere: OVER after a column or before FROM is a select item's
/// name, WINDOW a correlation name, the rank functions' names columns.
#[test]
fn window_words_are_names_elsewhere_5_2() {
    let (_db, a) = sales();
    a.run(
        "CREATE TABLE WORDS (OVER INTEGER, PARTITION INTEGER, RANGE INTEGER, GROUPS INTEGER, PRECEDING INTEGER, \
         FOLLOWING INTEGER, UNBOUNDED INTEGER, EXCLUDE INTEGER, TIES INTEGER, OTHERS INTEGER, RANK INTEGER, \
         DENSE_RANK INTEGER, PERCENT_RANK INTEGER, CUME_DIST INTEGER, ROW_NUMBER INTEGER)",
    )
    .unwrap();
    a.run("INSERT INTO WORDS VALUES (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15)").unwrap();
    let all = "SELECT OVER, PARTITION, RANGE, GROUPS, PRECEDING, FOLLOWING, UNBOUNDED, EXCLUDE, TIES, OTHERS, RANK, \
               DENSE_RANK, PERCENT_RANK, CUME_DIST, ROW_NUMBER FROM WORDS";
    assert_eq!(sorted(&a, all), ["1,2,3,4,5,6,7,8,9,10,11,12,13,14,15"]);
    assert_eq!(sorted(&a, "SELECT RANGE OVER FROM WORDS"), ["3"]);
    assert_eq!(sorted(&a, "SELECT COUNT(*) OVER FROM WORDS"), ["1"]);
    assert_eq!(sorted(&a, "SELECT WINDOW.RANK FROM WORDS WINDOW"), ["11"]);
    let framed = "SELECT SUM(RANGE) OVER (ORDER BY GROUPS ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE NO OTHERS) FROM WORDS";
    assert_eq!(sorted(&a, framed), ["3"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15, §11.32 SR, §7.6] a
/// view over a window function gives its values, a condition on the view
/// judged after the windows, so it does not change the rows they are
/// computed over; it is not updatable. A prepared statement over a window
/// gives its rows each time it is run.
#[test]
fn views_of_windows_are_filtered_after_them_7_15() {
    let (_db, a) = sales();
    a.run("CREATE VIEW RANKED AS SELECT R, D, RANK() OVER (PARTITION BY R ORDER BY D) AS RK FROM S").unwrap();
    assert_eq!(sorted(&a, "SELECT R, D, RK FROM RANKED WHERE D > 1 AND R = 'n'"), ["n,2,2", "n,2,2", "n,4,4"]);
    assert_eq!(sorted(&a, "SELECT R, D FROM RANKED WHERE RK = 1"), ["NULL,2", "n,1", "s,1"]);
    assert!(a.run("UPDATE RANKED SET D = 0").is_err(), "a view of a window function is not updatable");
    let p = a.prepare("SELECT D, SUM(AMOUNT) OVER (ORDER BY D ROWS UNBOUNDED PRECEDING) FROM S WHERE R = 's'").unwrap();
    for _ in 0_u32..2 {
        let mut cursor = a.open_prepared(&p, &[]).unwrap();
        let mut found = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            found.push(format!("{},{}", row[0], row[1]));
        }
        found.sort();
        assert_eq!(found, ["1,5", "3,12"]);
    }
    a.rollback().unwrap();
}

/// The windows of the partition tests, each over N's three partitions:
/// moving frames of each unit and exclusion, ranks, and a set function
/// with no state to merge.
const SPILLED: [&str; 5] = [
    "SELECT I, SUM(V) OVER (PARTITION BY P ORDER BY I ROWS BETWEEN 5 PRECEDING AND 5 FOLLOWING) FROM N",
    "SELECT I, SUM(V) OVER (PARTITION BY P ORDER BY K RANGE BETWEEN 2 PRECEDING AND 1 FOLLOWING EXCLUDE TIES) FROM N",
    "SELECT I, RANK() OVER (PARTITION BY P ORDER BY K), DENSE_RANK() OVER (PARTITION BY P ORDER BY K), CUME_DIST() OVER (PARTITION BY P ORDER BY K) FROM N",
    "SELECT I, MIN(V) OVER (PARTITION BY P ORDER BY K GROUPS BETWEEN 1 PRECEDING AND 2 FOLLOWING EXCLUDE GROUP), COUNT(*) OVER (PARTITION BY P) FROM N",
    "SELECT I, CARDINALITY(ARRAY_AGG(V) OVER (PARTITION BY P ORDER BY I ROWS BETWEEN 3 PRECEDING AND CURRENT ROW)) FROM N",
];

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR5] [DESIGN] RULES.md
/// D38: windows over partitions past the work area give what they give in
/// memory, their rows and the values their frames are found by spilled;
/// row by row and by batches alike; and a frame of ROWS is what its
/// definition by a subquery gives.
#[test]
fn partitions_past_the_work_area_spill_7_15() {
    let (db, a) = sales();
    a.run("CREATE TABLE N (I INTEGER, P INTEGER, K INTEGER, V INTEGER)").unwrap();
    for chunk in 0_i64..12 {
        let values: Vec<String> = (0_i64..100)
            .map(|n| {
                let i = chunk.checked_mul(100).and_then(|c| c.checked_add(n)).unwrap();
                let v = if i % 7 == 0 { "NULL".to_owned() } else { (i % 31).to_string() };
                format!("({i}, {}, {}, {v})", i % 3, i % 40)
            })
            .collect();
        a.run(&format!("INSERT INTO N VALUES {}", values.join(", "))).unwrap();
    }
    let defined = "SELECT I, (SELECT SUM(V) FROM N AS M WHERE M.P = N.P AND M.I BETWEEN N.I - 15 AND N.I + 15) FROM N";
    assert_eq!(sorted(&a, SPILLED[0]), sorted(&a, defined), "a frame of ROWS of 5 rows each way, I 3 apart in a partition");
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let in_memory: Vec<Vec<String>> = SPILLED.iter().map(|sql| sorted(&a, sql)).collect();
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for (sql, expected) in SPILLED.iter().zip(&in_memory) {
        let (batched, by_rows) = both_ways(&a, sql);
        assert_eq!(&batched, expected, "{sql}, spilled");
        assert_eq!(&by_rows, expected, "{sql}, spilled, row by row");
    }
    assert!(counted.taken() > 0, "they spilled");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.15 GR3, GR4] [DESIGN] a
/// window over a column store kept in order of its partitioning or
/// ordering reads the rows so, no sort made; of another ordering, they
/// are sorted; the values alike either way. EXPLAIN shows each window
/// with its frame.
#[test]
fn windows_read_rows_kept_in_order_unsorted_7_15() {
    let (_db, a) = sales();
    a.run("CREATE TABLE O (I INTEGER, K INTEGER) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048, COLUMNSTORE.ORDER = 'K')").unwrap();
    a.run("CREATE TABLE U (I INTEGER, K INTEGER)").unwrap();
    for table in ["O", "U"] {
        a.run(&format!("INSERT INTO {table} VALUES (1, 3), (2, 1), (3, NULL), (4, 2), (5, 1)")).unwrap();
    }
    let physical = |sql: &str| {
        let plan = a.explain(sql, &Params::new()).unwrap();
        plan.split_once("-- physical:").map(|(_, p)| p.to_owned()).unwrap_or(plan)
    };
    let kept = "SELECT I, SUM(I) OVER (ORDER BY K ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM O";
    let plan = physical(kept);
    assert!(!plan.contains("Sort"), "{plan}");
    assert!(plan.contains("ROWS BETWEEN 1 PRECEDING AND CURRENT ROW"), "{plan}");
    assert!(!physical("SELECT COUNT(*) OVER (PARTITION BY K) FROM O").contains("Sort"));
    assert!(physical("SELECT SUM(K) OVER (ORDER BY I) FROM O").contains("Sort"));
    assert_eq!(sorted(&a, "SELECT I, RANK() OVER (ORDER BY K) FROM O"), sorted(&a, "SELECT I, RANK() OVER (ORDER BY K) FROM U"));
    a.rollback().unwrap();
}
