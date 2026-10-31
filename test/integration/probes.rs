//! Index probes of a filter's conjuncts (RULES.md D31, D34): a LIKE's
//! prefix, an OR of one column's ranges and an IN list, columns of every
//! standard kind, and numbers of the other kind, an OR's in order; a few
//! rows of a join probing an index of the other operand's table (D35);
//! long IN lists judged by a set (D31); and conjuncts that can raise an
//! error moved onto the operand of a product they read, or of an outer
//! join's ON (D15). Each query's rows are those of the same query of a
//! table without indexes.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::optimizer::{PUSH_FILTER_INTO_OUTER_JOIN, PUSH_RAISING_FILTER_INTO_PRODUCT};
use turborel::params::Params;

use crate::common::{both_ways, rows, sorted};

/// Rows in each table of `twins`.
const ROWS: i32 = 300;

/// Schema P: tables I, indexed, and S, not, of the same rows: K 0 to 299,
/// N 'n' and K, C 'c' and K mod 40 (CHAR, padded), D a day from
/// 2020-01-01, Z an instant written at a displacement of K mod 3 hours, V
/// an interval of K days, B the bits of K mod 256, X K / 10 as a DOUBLE
/// PRECISION, M K + 0.25 as a DECIMAL(6,2).
fn twins() -> Database {
    let db = Database::new();
    let columns = "K INTEGER NOT NULL, N VARCHAR(8), C CHAR(6), D DATE, Z TIMESTAMP(0) WITH TIME ZONE, V INTERVAL DAY(3), \
                   B BIT(8), X DOUBLE PRECISION, M DECIMAL(6,2)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION P CREATE TABLE I ({columns}) CREATE TABLE S ({columns})")).unwrap();
    let s = db.session("P");
    for k in 0..ROWS {
        let zone = k % 3;
        let values = format!(
            "{k}, 'n{k}', 'c{}', DATE '2020-01-01' + INTERVAL '{k}' DAY(3), TIMESTAMP '2020-01-01 {:02}:00:00+0{zone}:00' + \
             INTERVAL '{k}' HOUR(3), INTERVAL '{k}' DAY(3), B'{:08b}', {k}E-1, {k}.25",
            k % 40,
            zone,
            k % 256
        );
        for table in ["I", "S"] {
            s.run(&format!("INSERT INTO {table} VALUES ({values})")).unwrap();
        }
    }
    s.commit().unwrap();
    for column in ["K", "N", "C", "D", "Z", "V", "B", "X", "M"] {
        db.create_index(&format!("I_{column}"), "P.I", &[column], "btree").unwrap();
    }
    db.create_index("I_K_HASH", "P.I", &["K"], "hash").unwrap();
    db
}

/// The physical plan of `query`, over table I, whose rows, sorted, must be
/// those of it over S, and at least `least` of them.
fn planned(s: &Session, query: &str, least: usize) -> String {
    let (indexed, unindexed) = (sorted(s, query), sorted(s, &query.replace("FROM I", "FROM S")));
    assert_eq!(indexed, unindexed, "{query}");
    assert!(indexed.len() >= least, "{query}: {indexed:?}");
    let plan = s.explain(query, &Params::new()).unwrap();
    plan.split("-- physical:").nth(1).unwrap_or_default().to_string()
}

/// Verifies: [X3.135-1986 §5.14 GR5, §5.11 GR5]; [DESIGN] RULES.md D34,
/// D15: a LIKE of a constant pattern is answered by a probe of the range
/// of the characters it begins with, the last one less and one more,
/// strings padded or not, an escaped `%` among them; the filter still
/// judges the LIKE of every row the probe finds.
#[test]
fn a_like_prefix_is_probed_and_still_judged() {
    let db = twins();
    let s = db.session("P");
    // Of these, 'n29\t' and 'c39\t' compare below the prefixes, padded.
    for table in ["I", "S"] {
        s.run(&format!("INSERT INTO {table} (K, N, C) VALUES (900, 'n29%5', 'c39'), (901, 'n29', 'c39 '), (902, 'n29\t', 'c39\t')"))
            .unwrap();
    }
    let plan = planned(&s, "SELECT K FROM I WHERE N LIKE 'n29%'", 14);
    assert!(plan.contains("IndexScan P.I using I_N [btree]: I.N > 'n28' AND I.N < 'n2:'"), "{plan}");
    assert!(plan.contains("Filter I.N LIKE 'n29%'"), "{plan}");
    let plan = planned(&s, "SELECT K FROM I WHERE C LIKE 'c39%' AND K < 1000", 10);
    assert!(plan.contains("using I_C"), "{plan}");
    let plan = planned(&s, "SELECT K FROM I WHERE N LIKE 'n29!%%' ESCAPE '!'", 1);
    assert!(plan.contains("I.N > 'n29$' AND I.N < 'n29&'"), "the escaped % is the prefix's: {plan}");
    let plan = planned(&s, "SELECT K FROM I WHERE N LIKE 'n29' OR N LIKE 'n29_'", 13);
    assert!(plan.contains("using I_N"), "{plan}");
    planned(&s, "SELECT K FROM I WHERE C LIKE 'c39'", 0);
}

/// Verifies: [X3.135-1986 §5.13 GR2, §5.18 GR2]; [DESIGN] RULES.md D34:
/// an IN list of constants, and an OR of ranges of one column, are
/// answered by a probe of each range, joined where they overlap or touch,
/// so each row is found once; by a hash index too, for equalities; an
/// UPDATE and a DELETE find their rows so.
#[test]
fn an_in_list_and_an_or_of_ranges_are_probed() {
    let db = twins();
    let s = db.session("P");
    let plan = planned(&s, "SELECT K FROM I WHERE K IN (3, 7, 7.0, 299, 1000)", 3);
    assert_eq!(plan.matches("IndexScan P.I using I_K").count(), 4, "{plan}");
    assert!(plan.contains("Append") && plan.contains("I.K = 7"), "{plan}");
    assert!(!plan.contains("Filter"), "an IN of constants is answered exactly: {plan}");
    let plan = planned(&s, "SELECT K FROM I WHERE K < 3 OR K > 297 OR K = 150 OR K BETWEEN 1 AND 4", 7);
    assert!(plan.contains("I.K < 3") || plan.contains("I.K <= 4"), "{plan}");
    assert_eq!(plan.matches("IndexScan").count(), 3, "{plan}");
    let plan = planned(&s, "SELECT K FROM I WHERE K < 30 OR K > 20", 300);
    assert!(plan.contains("SeqScan"), "every row: {plan}");
    planned(&s, "SELECT K FROM I WHERE N IN ('n5', 'n50  ') OR N = 'n7'", 3);
    let plan = planned(&s, "SELECT K FROM I WHERE M IN (5.25, 9.5E0, 7.25E0)", 2);
    assert!(plan.contains("using I_M"), "{plan}");
    // An UPDATE and a DELETE of the rows the probes find.
    for table in ["I", "S"] {
        s.run(&format!("UPDATE {table} SET N = 'x' WHERE K IN (10, 12, 14)")).unwrap();
        s.run(&format!("DELETE FROM {table} WHERE K IN (11, 12) OR K = 13")).unwrap();
    }
    assert_eq!(rows(&s, "SELECT K, N FROM I WHERE K BETWEEN 9 AND 15"), rows(&s, "SELECT K, N FROM S WHERE K BETWEEN 9 AND 15"));
    assert_eq!(sorted(&s, "SELECT K FROM I WHERE N = 'x'"), ["10", "14"]);
}

/// Verifies: [DESIGN] RULES.md D34 [ISO/IEC 9075:1992 §8.2 GR4, GR5,
/// GR6]: columns of DATE, TIMESTAMP WITH TIME ZONE, INTERVAL and BIT are
/// probed by constants of their kinds, an instant written at another
/// displacement finding the rows of that instant.
#[test]
fn datetimes_intervals_and_bits_are_probed() {
    let db = twins();
    let s = db.session("P");
    for (query, least, index) in [
        ("SELECT K FROM I WHERE D = DATE '2020-01-05'", 1, "I_D"),
        ("SELECT K FROM I WHERE D BETWEEN DATE '2020-03-01' AND DATE '2020-03-03'", 3, "I_D"),
        ("SELECT K FROM I WHERE Z = TIMESTAMP '2020-01-01 06:00:00+01:00'", 1, "I_Z"),
        ("SELECT K FROM I WHERE Z < TIMESTAMP '2020-01-01 03:00:00+00:00'", 2, "I_Z"),
        ("SELECT K FROM I WHERE V = INTERVAL '12' DAY", 1, "I_V"),
        ("SELECT K FROM I WHERE V > INTERVAL '6960' HOUR(4)", 9, "I_V"),
        ("SELECT K FROM I WHERE B = B'00001100'", 2, "I_B"),
        ("SELECT K FROM I WHERE D IN (DATE '2020-01-02', DATE '2020-01-09')", 2, "I_D"),
    ] {
        let plan = planned(&s, query, least);
        assert!(plan.contains(&format!("using {index}")), "{query}: {plan}");
    }
}

/// Verifies: [X3.135-1986 §5.11 GR4]; [DESIGN] RULES.md D34: a column of
/// approximate numbers is probed by an exact constant, and one of exact
/// numbers by an approximate one, each placed among the column's values,
/// so the probe finds the rows SQL's comparison keeps.
///
/// Regression test (R-TEST-7): an exact constant was made its nearest
/// approximate number, so `X = 0.1` found the row of 0.1E0, which is not
/// 0.1 by algebraic value and which a scan rightly leaves out.
#[test]
fn numbers_of_the_other_kind_are_probed_exactly() {
    let db = twins();
    let s = db.session("P");
    for (query, least, index) in [
        ("SELECT K FROM I WHERE X = 0.1", 0, "I_X"),
        ("SELECT K FROM I WHERE X = 0.5", 1, "I_X"),
        ("SELECT K FROM I WHERE X < 0.3", 3, "I_X"),
        ("SELECT K FROM I WHERE X >= 0.3 AND X <= 0.6", 3, "I_X"),
        ("SELECT K FROM I WHERE M = 3.25E0", 1, "I_M"),
        ("SELECT K FROM I WHERE M = 3.3E0", 0, "I_M"),
        ("SELECT K FROM I WHERE M <= 3.3E0", 4, "I_M"),
        ("SELECT K FROM I WHERE M > 296.3E0", 3, "I_M"),
    ] {
        let plan = planned(&s, query, least);
        assert!(plan.contains(&format!("using {index}")), "{query}: {plan}");
    }
    assert_eq!(rows(&s, "SELECT K FROM I WHERE X = 0.1"), Ok(Vec::new()));
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2, §5.18 GR2]; [DESIGN]
/// RULES.md D15, D35: a conjunct that can raise an error and reads one
/// operand of a product is evaluated on every row of that operand, if the
/// other has rows, so the equality of the operands is still a hash join's
/// key, and the other's comparison an index probe; it raises its error
/// whatever rows the join keeps, and none if the other operand is empty,
/// as over the product; one reading both operands keeps the product.
#[test]
fn a_raising_conjunct_of_one_operand_keeps_the_hash_join() {
    let db = twins();
    let s = db.session("P");
    s.run("CREATE TABLE E (K INTEGER)").unwrap();
    s.commit().unwrap();
    let query = "SELECT I.K, S.N FROM I, S WHERE I.K = S.K AND UPPER(S.N) = 'N7' AND I.K < 50";
    let plan = s.explain(query, &Params::new()).unwrap();
    assert!(plan.contains(PUSH_RAISING_FILTER_INTO_PRODUCT), "{plan}");
    // On the equality: S's few rows each probing I's index, or a hash join
    // of I's rows its index finds.
    let probed = plan.contains("IndexNestedLoop P.I using I_K [btree]: S.K = I.K AND I.K < 50");
    assert!(probed || plan.contains("HashJoin") && plan.contains("IndexScan P.I using I_K [btree]: I.K < 50"), "{plan}");
    let joined = rows(&s, query);
    assert_eq!(joined, Ok(vec!["7,n7".to_string()]));
    // The error of a row the join leaves out, and none over no rows.
    let raising = "SELECT I.K FROM I, S WHERE I.K = S.K AND I.K = 5 AND 100 / (S.K - 9) > 0";
    let quiet = "SELECT I.K FROM I, E WHERE I.K = E.K AND 100 / (I.K - 9) > 0";
    assert_eq!(rows(&s, raising), Err(Code::Data));
    assert_eq!(rows(&s, quiet), Ok(Vec::new()));
    assert!(s.explain(quiet, &Params::new()).unwrap().contains("HashJoin"));
    let spanning = "SELECT I.K FROM I, S WHERE I.K = S.K AND 100 / (I.K - S.K + 9) > 0";
    let plan = s.explain(spanning, &Params::new()).unwrap();
    assert!(!plan.contains("HashJoin") && !plan.contains(PUSH_RAISING_FILTER_INTO_PRODUCT), "{plan}");
    // The same rows and errors without the rule.
    crate::common::set_rule(&db, PUSH_RAISING_FILTER_INTO_PRODUCT, false);
    let product = s.explain(query, &Params::new()).unwrap();
    assert!(!product.contains("HashJoin") && !product.contains("IndexNestedLoop"), "{product}");
    assert_eq!(rows(&s, query), joined);
    assert_eq!((rows(&s, raising), rows(&s, quiet)), (Err(Code::Data), Ok(Vec::new())));
}

/// Verifies: [X3.135-1986 §5.20 GR2b]; [DESIGN] RULES.md D15, D35: over
/// three tables, a conjunct that can raise an error and reads one of them
/// is evaluated on its rows when the other two have rows, and the others
/// are joined in the order found cheapest.
#[test]
fn a_raising_conjunct_of_one_of_three_operands() {
    let db = twins();
    let s = db.session("P");
    s.run("CREATE TABLE E (K INTEGER)").unwrap();
    s.run("INSERT INTO E VALUES (4), (8)").unwrap();
    s.commit().unwrap();
    let query = "SELECT I.K FROM I, S, E WHERE I.K = S.K AND S.K = E.K AND 100 / (S.K - 300) < 0";
    assert_eq!(sorted(&s, query), ["4", "8"]);
    let plan = s.explain(query, &Params::new()).unwrap();
    assert_eq!(plan.matches("HashJoin").count() + plan.matches("IndexNestedLoop").count(), 2, "{plan}");
    assert_eq!(rows(&s, "SELECT I.K FROM I, S, E WHERE I.K = S.K AND S.K = E.K AND 100 / (S.K - 7) < 0"), Err(Code::Data));
    s.run("DELETE FROM E").unwrap();
    assert_eq!(rows(&s, "SELECT I.K FROM I, S, E WHERE I.K = S.K AND S.K = E.K AND 100 / (S.K - 7) < 0"), Ok(Vec::new()));
}

/// Schema P of `twins`, with table E of a few rows: K 4, 8, 8, a null and
/// 1000, C, D and B of K as I's and S's are, of 1000 C 'c0', D
/// 2030-01-01 and B 11111111.
fn with_few() -> Database {
    let db = twins();
    let s = db.session("P");
    s.run("CREATE TABLE E (K INTEGER, C CHAR(6), D DATE, B BIT(8))").unwrap();
    s.run(
        "INSERT INTO E VALUES (4, 'c4', DATE '2020-01-05', B'00000100'), (8, 'c8', DATE '2020-01-09', B'00001000'), \
         (8, 'c8', DATE '2020-01-09', B'00001000'), (NULL, 'c0', NULL, NULL), (1000, 'c0', DATE '2030-01-01', B'11111111')",
    )
    .unwrap();
    s.commit().unwrap();
    db
}

/// The physical plan of `query`, joining E and table I, whose rows,
/// sorted, read by batches and row by row, must be those of it joined
/// with S, unindexed, and at least `least` of them.
fn joined(s: &Session, query: &str, least: usize) -> String {
    let unindexed = query.replace(" I ", " S ").replace("I.", "S.");
    let (batched, by_rows) = both_ways(s, query);
    assert_eq!(batched, by_rows, "{query}");
    assert_eq!(batched, sorted(s, &unindexed), "{query}");
    assert!(batched.len() >= least, "{query}: {batched:?}");
    s.explain(query, &Params::new()).unwrap().split("-- physical:").nth(1).unwrap_or_default().to_string()
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.21 GR2, §5.11 GR2]; [ISO/IEC
/// 9075:1992 §7.5]; [DESIGN] RULES.md D35, D15: a few rows joined on an
/// equality with a table's indexed column each probe the index, in either
/// order of the FROM clause, the rest of the condition and the table's own
/// filter judged of the pairs found, an equality the probe does not answer
/// among them; a null finds nothing; of a LEFT join, a row that finds none
/// is kept with nulls. Many rows are hash joined; a conjunct that can raise
/// an error of the table's columns keeps its every row read, and raises
/// its error, as one of the few rows' does.
#[test]
fn few_rows_probe_an_index_of_the_other_table() {
    let db = with_few();
    let s = db.session("P");
    let plan = joined(&s, "SELECT E.K, I.N FROM E, I WHERE E.K = I.K", 3);
    assert!(plan.contains("IndexNestedLoop P.I using I_K") && !plan.contains("HashJoin"), "{plan}");
    let plan = joined(&s, "SELECT E.K, I.N FROM I , E WHERE I.K = E.K", 3);
    assert!(plan.contains("IndexNestedLoop P.I using I_K"), "the few rows joined first: {plan}");
    let plan = joined(&s, "SELECT E.K, I.N FROM E, I WHERE E.K = I.K AND E.C = I.C AND I.N <> 'n4' AND I.X > 0.5", 2);
    assert!(plan.contains("IndexNestedLoop") && plan.contains("I.N <> 'n4'") && plan.contains("I.X > 0.5"), "{plan}");
    let plan = joined(&s, "SELECT E.K, I.N FROM E LEFT JOIN I ON E.K = I.K AND I.C <> 'c8'", 5);
    assert!(plan.contains("LeftOuterIndexNestedLoop P.I using I_K"), "{plan}");
    let kept = rows(&s, "SELECT E.K, I.N FROM E LEFT JOIN I ON E.K = I.K ORDER BY 1").unwrap();
    assert!(kept.contains(&"1000,NULL".to_string()), "{kept:?}");
    // The table's own read by an IN list's probes, set against the join's.
    joined(&s, "SELECT E.K, I.N FROM E, I WHERE E.K = I.K AND I.K IN (4, 8, 9)", 3);
    let plan = s.explain("SELECT S.K FROM S, I WHERE S.K = I.K", &Params::new()).unwrap();
    assert!(plan.contains("HashJoin") && !plan.contains("IndexNestedLoop"), "many rows: {plan}");
    // A conjunct that can raise an error, of the table's columns or of E's.
    let raising = "SELECT E.K FROM E, I WHERE E.K = I.K AND 100 / (I.K - 9) > 0";
    assert_eq!(rows(&s, raising), Err(Code::Data));
    assert!(!s.explain(raising, &Params::new()).unwrap().contains("IndexNestedLoop"));
    let of_few = "SELECT E.K FROM E, I WHERE E.K = I.K AND 100 / (E.K - 1000) < 0";
    assert!(s.explain(of_few, &Params::new()).unwrap().contains("IndexNestedLoop"));
    assert_eq!(rows(&s, of_few), Err(Code::Data));
    assert!(joined(&s, "SELECT E.K FROM E, I WHERE E.K = I.K AND 100 / (E.K - 1001) < 0", 3).contains("IndexNestedLoop"));
}

/// Verifies: [X3.135-1986 §5.20 GR2b, §5.11 GR4]; [DESIGN] RULES.md D35,
/// D34: a few rows probe an index on a DATE or a BIT column, by values of
/// its kind; an index on two columns by both, or by its first if the
/// second is NOT NULL; not a hash index by its first column alone, nor a
/// B-link index whose second column may be null, nor an index by values
/// of another kind of number. A left operand of an outer join, or of a
/// view's expression, probes nothing.
#[test]
fn few_rows_probe_indexes_of_every_kind_they_can() {
    let db = with_few();
    let s = db.session("P");
    s.run("CREATE TABLE W (K INTEGER NOT NULL, N VARCHAR(8), C CHAR(6))").unwrap();
    s.run("INSERT INTO W SELECT K, N, C FROM S").unwrap();
    s.run("CREATE VIEW V (K) AS SELECT K + 0 FROM E").unwrap();
    s.commit().unwrap();
    for (name, columns, method) in [("W_C_K", ["C", "K"], "btree"), ("W_K_C", ["K", "C"], "hash"), ("W_K_N", ["K", "N"], "btree")] {
        db.create_index(name, "P.W", &columns, method).unwrap();
    }
    for (query, least, probed) in [
        ("SELECT E.K, I.K FROM E, I WHERE E.D = I.D", 3, true),
        ("SELECT E.K, I.K FROM E, I WHERE E.B = I.B", 4, true),
        ("SELECT E.K, I.K FROM E, I WHERE E.K = I.X", 1, false),
        ("SELECT V.K, I.N FROM V, I WHERE V.K = I.K", 3, false),
        ("SELECT E.K, I.N FROM (E LEFT JOIN S AS L ON E.K = L.K) JOIN I ON L.K = I.K", 3, false),
    ] {
        let plan = joined(&s, query, least);
        assert_eq!(plan.contains("IndexNestedLoop P.I"), probed, "{query}: {plan}");
    }
    for (query, least, index) in [
        ("SELECT E.K, W.N FROM E, W WHERE E.C = W.C AND E.K = W.K", 3, Some("W_")),
        ("SELECT E.K, W.N FROM E, W WHERE E.C = W.C", 30, Some("W_C_K")),
        ("SELECT E.K, W.N FROM E, W WHERE E.K = W.K", 3, None),
    ] {
        assert_eq!(sorted(&s, query), sorted(&s, &query.replace("W.", "S.").replace(" W ", " S ")), "{query}");
        assert!(sorted(&s, query).len() >= least, "{query}");
        let plan = s.explain(query, &Params::new()).unwrap();
        let used = index.map(|i| format!("IndexNestedLoop P.W using {i}"));
        assert_eq!(used.as_ref().is_some_and(|u| plan.contains(u)), index.is_some(), "{query}: {plan}");
        assert_eq!(plan.contains("IndexNestedLoop"), index.is_some(), "{query}: {plan}");
    }
}

/// Verifies: [X3.135-1986 §5.13 GR2, §5.18 GR2, §5.11 GR2, GR4, GR5];
/// [DESIGN] RULES.md D31: a long IN list of constants a filter judges is
/// judged by their set, row by row and over vectors alike: of numbers of
/// either kind by algebraic value, of strings padded; NOT IN of a list with
/// a null is true of no row, IN of one true of the rows equal to one; the
/// rows are those the index's probes find.
#[test]
fn long_in_lists_are_judged_by_a_set() {
    let db = twins();
    let s = db.session("P");
    let list = |of: &dyn Fn(i32) -> String| (0..12_i32).map(|n| of(n * 25)).collect::<Vec<_>>().join(", ");
    for (query, least) in [
        (format!("SELECT K FROM I WHERE K IN ({})", list(&|n| n.to_string())), 12),
        (format!("SELECT K FROM I WHERE K IN ({}, CAST(NULL AS INTEGER))", list(&|n| format!("{n}.0"))), 12),
        (format!("SELECT K FROM I WHERE X IN ({})", list(&|n| format!("{n}E-1"))), 12),
        (format!("SELECT K FROM I WHERE M IN ({})", list(&|n| format!("{n}.25E0"))), 12),
        (format!("SELECT K FROM I WHERE C IN ({})", list(&|n| format!("'c{}  '", n % 40))), 60),
        (format!("SELECT K FROM I WHERE N IN ({}) AND K + 0 < 200", list(&|n| format!("'n{n}'"))), 8),
    ] {
        let unindexed = query.replace("FROM I", "FROM S");
        let plan = s.explain(&unindexed, &Params::new()).unwrap();
        assert!(plan.contains(" IN ("), "{unindexed}: {plan}");
        let (batched, by_rows) = both_ways(&s, &unindexed);
        assert_eq!(batched, by_rows, "{unindexed}");
        planned(&s, &query, least);
        assert_eq!(sorted(&s, &query), batched, "{query}");
    }
    let not_in = format!("SELECT K FROM S WHERE K NOT IN ({})", list(&|n| n.to_string()));
    assert_eq!(both_ways(&s, &not_in).0.len(), 288);
    let nulled = format!("SELECT K FROM S WHERE K NOT IN ({}, CAST(NULL AS INTEGER))", list(&|n| n.to_string()));
    assert_eq!(both_ways(&s, &nulled), (Vec::new(), Vec::new()));
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §7.5 GR1c]; RULES.md D15, D35: an
/// ON conjunct of an outer join that can raise an error, of the columns of
/// the operand given nulls alone, is evaluated on every row of that
/// operand when the other has rows, as on every row of their product, so
/// the join is on its equalities still; its error is raised whatever rows
/// match, and none when the preserved operand is empty.
#[test]
fn a_raising_on_conjunct_of_the_operand_given_nulls_moves() {
    let db = with_few();
    let s = db.session("P");
    let query = "SELECT E.K, S.N FROM E LEFT JOIN S ON E.K = S.K AND 100 / (S.K - 400) < 0";
    let joined_rows = sorted(&s, query);
    assert_eq!(joined_rows.len(), 5, "{joined_rows:?}");
    let plan = s.explain(query, &Params::new()).unwrap();
    assert!(plan.contains(PUSH_FILTER_INTO_OUTER_JOIN) && plan.contains("EXISTS"), "{plan}");
    let right = "SELECT E.K, S.N FROM S RIGHT JOIN E ON E.K = S.K AND 100 / (S.K - 400) < 0";
    assert_eq!(sorted(&s, right), joined_rows);
    assert!(s.explain(right, &Params::new()).unwrap().contains("EXISTS"));
    let raising = "SELECT E.K FROM E LEFT JOIN S ON E.K = S.K AND 100 / (S.K - 9) < 0";
    assert_eq!(rows(&s, raising), Err(Code::Data), "of a row no row of E matches");
    // The same rows and errors without the rule; none of an empty E.
    crate::common::set_rule(&db, PUSH_FILTER_INTO_OUTER_JOIN, false);
    assert_eq!((sorted(&s, query), rows(&s, raising)), (joined_rows, Err(Code::Data)));
    crate::common::set_rule(&db, PUSH_FILTER_INTO_OUTER_JOIN, true);
    s.run("DELETE FROM E").unwrap();
    assert_eq!(rows(&s, raising), Ok(Vec::new()));
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §7.5 GR1c]; RULES.md D15, D35: an
/// ON conjunct of an outer join that can raise an error, of the columns of
/// one operand alone, the preserved one's among them, is judged of every
/// row of that operand when the other has rows, as on every row of their
/// product, and the join is on its equalities still (hashed on them): a
/// row it is not true of matches none, its error is raised though the row
/// matches nothing by its keys, and none is raised when the other operand
/// is empty. LEFT, RIGHT and FULL joins, read by batches and row by row.
#[test]
fn a_raising_on_conjunct_of_one_operand_is_judged_of_its_rows() {
    let db = with_few();
    let s = db.session("P");
    let lonely = ["1000,NULL", "4,4", "8,NULL", "8,NULL", "NULL,NULL"];
    let own = "AND 100 / (E.K - 7) < 1";
    for query in [
        format!("SELECT E.K, S.K FROM E LEFT JOIN S ON E.K = S.K {own}"),
        format!("SELECT E.K, S.K FROM S RIGHT JOIN E ON E.K = S.K {own}"),
    ] {
        assert_eq!(both_ways(&s, &query), (lonely.map(String::from).to_vec(), lonely.map(String::from).to_vec()), "{query}");
        let plan = s.explain(&query, &Params::new()).unwrap();
        assert!(plan.contains("(hashed on "), "{query}: {plan}");
    }
    let full = format!("SELECT E.K, S.K FROM E FULL JOIN S ON E.K = S.K {own} AND S.K / (S.K - 300) > -1");
    let (batched, by_rows) = both_ways(&s, &full);
    assert_eq!((batched.len(), batched.contains(&"4,4".to_owned())), (304, true), "{batched:?}");
    assert_eq!(batched, by_rows);
    // Errors of rows no row of the other operand matches by its keys.
    let raising = [
        "SELECT E.K FROM E LEFT JOIN S ON E.K = S.K AND 100 / (E.K - 1000) < 1",
        "SELECT E.K FROM S RIGHT JOIN E ON E.K = S.K AND 100 / (E.K - 1000) < 1",
        "SELECT E.K FROM E FULL JOIN S ON E.K = S.K AND 100 / (S.K - 200) < 1",
        "SELECT E.K FROM E FULL JOIN S ON E.K = S.K AND 100 / (E.K - 1000) < 1 AND 100 / (S.K - 200) < 1",
    ];
    for query in raising {
        assert_eq!(rows(&s, query), Err(Code::Data), "{query}");
    }
    // None of an operand when the other is empty.
    s.run("DELETE FROM E").unwrap();
    assert_eq!(rows(&s, raising[2]).map(|r| r.len()), Ok(300));
    s.run("DELETE FROM S").unwrap();
    s.run("INSERT INTO E (K) VALUES (1000)").unwrap();
    for query in raising {
        assert_eq!(rows(&s, query), Ok(vec!["1000".to_owned()]), "{query}");
    }
}

/// Verifies: [X3.135-1986 §8.3 GR3b]; [DESIGN] RULES.md D34: the index
/// scans of an OR's ranges, an IN list's among them, give their rows in the
/// index's order, ascending, or descending read backward, the last range
/// first, so an ORDER BY of the column sorts nothing.
#[test]
fn an_ors_index_scans_are_in_order() {
    let db = twins();
    let s = db.session("P");
    for (query, expected) in [
        ("SELECT K FROM I WHERE K IN (150, 3, 299, 7) ORDER BY K", vec!["3", "7", "150", "299"]),
        ("SELECT K FROM I WHERE K IN (150, 3, 299, 7) ORDER BY K DESC", vec!["299", "150", "7", "3"]),
        ("SELECT K FROM I WHERE K < 2 OR K > 297 OR K BETWEEN 100 AND 101 ORDER BY K DESC", vec!["299", "298", "101", "100", "1", "0"]),
    ] {
        let plan = planned(&s, query, expected.len());
        assert!(!plan.contains("Sort") && plan.contains("Append"), "{query}: {plan}");
        assert_eq!(plan.contains("backward"), query.ends_with("DESC"), "{plan}");
        assert_eq!(rows(&s, query), Ok(expected.iter().map(ToString::to_string).collect()), "{query}");
    }
}
