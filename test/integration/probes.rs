//! Index probes of a filter's conjuncts (RULES.md D31, D34): a LIKE's
//! prefix, an OR of one column's ranges and an IN list, columns of every
//! standard kind, and numbers of the other kind; and conjuncts that can
//! raise an error moved onto the operand of a product they read (D15). Each
//! query's rows are those of the same query of a table without indexes.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::optimizer::PUSH_RAISING_FILTER_INTO_PRODUCT;
use turborel::params::Params;

use crate::common::{rows, sorted};

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
    assert!(plan.contains("HashJoin") && plan.contains("IndexScan P.I using I_K [btree]: I.K < 50"), "{plan}");
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
    assert!(db.with_optimizer(|o| o.set_enabled(PUSH_RAISING_FILTER_INTO_PRODUCT, false)).unwrap());
    assert!(!s.explain(query, &Params::new()).unwrap().contains("HashJoin"));
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
    assert_eq!(plan.matches("HashJoin").count(), 2, "{plan}");
    assert_eq!(rows(&s, "SELECT I.K FROM I, S, E WHERE I.K = S.K AND S.K = E.K AND 100 / (S.K - 7) < 0"), Err(Code::Data));
    s.run("DELETE FROM E").unwrap();
    assert_eq!(rows(&s, "SELECT I.K FROM I, S, E WHERE I.K = S.K AND S.K = E.K AND 100 / (S.K - 7) < 0"), Ok(Vec::new()));
}
