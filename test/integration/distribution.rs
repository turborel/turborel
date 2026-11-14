//! The inverse distribution functions [ISO/IEC 9075-2:2023 §10.9, Feature
//! T612]: PERCENTILE_CONT and PERCENTILE_DISC WITHIN GROUP of a group,
//! each against GR10's definition by a scalar subquery, of each fraction
//! of eighths and others, ascending and descending, of ties and nulls; of
//! numbers, intervals and DECFLOAT; of no rows, a null fraction and one
//! out of range; in grouping sets, filtered, prepared and in a view; over
//! a window's partition; past the work area; each Syntax Rule; the names
//! still names; and a constraint of one.

use turborel::db::{Database, Session};
use turborel::error::{Code, SqlState};
use turborel::value::{Decimal, Value};

use crate::common::{Counted, both_ways, exec, fetched, sorted};
use crate::windows::prepared;

/// Schema A: T, of rows in four groups, ties, nulls, a group of nulls
/// alone, and keys of numbers, intervals and DECFLOAT; U, of numbers.
fn distributed_rows() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (I INTEGER, G INTEGER, V INTEGER, IV INTERVAL DAY TO SECOND, F DECFLOAT, D DOUBLE PRECISION)
           CREATE TABLE U (X INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES (1, 1, 1, INTERVAL '1' DAY, 1.5, 1E0), (2, 1, 2, INTERVAL '2:00:00' HOUR TO SECOND, 2, 2.5E0), \
         (3, 1, NULL, NULL, NULL, NULL), (4, 1, 2, INTERVAL '3' DAY, 2.25, 2E0), (5, 1, 4, INTERVAL '0.5' SECOND, 4, 4E0), \
         (6, 1, 7, INTERVAL '7' DAY, 7, 7E0), (7, 2, 0, INTERVAL '0' DAY, 0, 0E0), (8, 2, 5, INTERVAL '5' DAY, 5, 5E0), \
         (9, 2, 5, INTERVAL '5' DAY, 5, 5E0), (10, 2, 9, INTERVAL '9 12:00' DAY TO MINUTE, 9, 9E0), (11, 3, NULL, NULL, NULL, NULL), \
         (12, 4, 3, INTERVAL '3' DAY, 3, 3E0)",
    )
    .unwrap();
    a.run("INSERT INTO U VALUES (1), (2), (4)").unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The fractions tried: each eighth, and some not of a binary fraction.
const FRACTIONS: [&str; 11] = ["0", "0.125", "0.25", "0.375", "0.5", "0.625", "0.75", "0.875", "1", "0.3", "0.9"];

/// [ISO/IEC 9075-2:2023 §10.9 GR10h ii] PERCENTILE_CONT of `p` of the
/// values `key` of T of the group's G in `order`, of type `dt`: GR10's
/// scalar subquery written out, of the count of the values not null, ROW0
/// the row number at or below NVE·(N−1) and less than 1 below, ROW1 the
/// one at or above it and less than 1 above, FACTOR what is between.
fn cont_definition(p: &str, (key, order): (&str, &str), dt: &str) -> String {
    let txa = format!("SELECT {key} FROM T AS T2 WHERE T2.G = T.G AND {key} IS NOT NULL");
    let at = format!("{p} * ((SELECT COUNT({key}) FROM T AS T3 WHERE T3.G = T.G) - 1)");
    format!(
        "(WITH TEMPTABLE (X, Y) AS (SELECT ROW_NUMBER() OVER (ORDER BY C {order}) - 1, C FROM ({txa}) AS TXA (C)) \
         SELECT CAST(T0.Y + 1E0 * ({at} - T0.X) * (T1.Y - T0.Y) AS {dt}) FROM TEMPTABLE T0, TEMPTABLE T1 \
         WHERE T0.X <= {at} AND {at} < T0.X + 1 AND T1.X >= {at} AND {at} > T1.X - 1)"
    )
}

/// [ISO/IEC 9075-2:2023 §10.9 GR10h iii] PERCENTILE_DISC of `p` of the
/// values `key` of T of the group's G in `order`, of type `dt`: GR10's
/// scalar subquery written out, MAX of DESC, else MIN, of the values whose
/// CUME_DIST is at least `p`.
fn disc_definition(p: &str, (key, order): (&str, &str), dt: &str) -> String {
    let extreme = if order.starts_with("DESC") { "MAX" } else { "MIN" };
    format!(
        "(SELECT CAST({extreme}(C) AS {dt}) FROM (SELECT C, CUME_DIST() OVER (ORDER BY C {order}) \
         FROM (SELECT {key} FROM T AS T2 WHERE T2.G = T.G AND {key} IS NOT NULL) AS TXA (C)) AS TEMPTABLE (C, CUMEDIST) \
         WHERE CUMEDIST >= {p})"
    )
}

/// `f` WITHIN GROUP of the fraction `p` ordered by `key` in `order`.
fn distribution(f: &str, p: &str, (key, order): (&str, &str)) -> String {
    format!("{f}({p}) WITHIN GROUP (ORDER BY {key} {order})")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10d, GR10h]
/// PERCENTILE_CONT and PERCENTILE_DISC of a group are GR10's scalar
/// subqueries over its values not null: of each eighth, 0 and 1, and
/// fractions of no binary fraction, ascending and descending, of ties,
/// of one value, and of none; by batches and row by row.
#[test]
fn each_function_is_its_definition_10_9() {
    let (_db, a) = distributed_rows();
    for order in ["ASC", "DESC", "DESC NULLS LAST"] {
        let (mut selected, mut defined) = (Vec::new(), Vec::new());
        for p in FRACTIONS {
            selected.push(distribution("PERCENTILE_CONT", p, ("V", order)));
            selected.push(distribution("PERCENTILE_DISC", p, ("V", order)));
            defined.push(cont_definition(p, ("V", order), "DOUBLE PRECISION"));
            defined.push(disc_definition(p, ("V", order), "DOUBLE PRECISION"));
        }
        let sql = format!("SELECT G, {} FROM T GROUP BY G", selected.join(", "));
        let (batched, by_rows) = both_ways(&a, &sql);
        assert_eq!(batched, sorted(&a, &format!("SELECT G, {} FROM T GROUP BY G", defined.join(", "))), "{order}");
        assert_eq!(by_rows, batched, "{order}");
    }
    let shown = "SELECT G, PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY V DESC), PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY V DESC), \
                 PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V), PERCENTILE_DISC(0.3) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G";
    assert_eq!(sorted(&a, shown), ["1,4E0,4E0,2E0,2E0", "2,6E0,9E0,5E0,5E0", "3,NULL,NULL,NULL,NULL", "4,3E0,3E0,3E0,3E0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR12e, GR10h] of an
/// interval key the result is of its type, interpolated as GR10's
/// arithmetic has it; of a DECFLOAT key DECFLOAT; of an approximate key
/// DOUBLE PRECISION; each as GR10's subquery gives it.
#[test]
fn intervals_decfloat_and_approximate_keys_10_9() {
    let (_db, a) = distributed_rows();
    for (key, dt) in [("IV", "INTERVAL DAY TO SECOND"), ("F", "DECFLOAT(34)"), ("D", "DOUBLE PRECISION")] {
        for order in ["ASC", "DESC"] {
            let (mut selected, mut defined) = (Vec::new(), Vec::new());
            for p in ["0.25", "0.5", "0.875", "0.3"] {
                selected.push(distribution("PERCENTILE_CONT", p, (key, order)));
                selected.push(distribution("PERCENTILE_DISC", p, (key, order)));
                defined.push(cont_definition(p, (key, order), dt));
                defined.push(disc_definition(p, (key, order), dt));
            }
            let sql = format!("SELECT G, {} FROM T GROUP BY G", selected.join(", "));
            assert_eq!(sorted(&a, &sql), sorted(&a, &format!("SELECT G, {} FROM T GROUP BY G", defined.join(", "))), "{key} {order}");
        }
    }
    let shown = "SELECT PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY IV), PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY F), \
                 PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY F) FROM T WHERE G = 2";
    assert_eq!(sorted(&a, shown), ["3 18:00:00.000000,3.75,0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10b, GR10c, GR10d,
/// GR10h i] a null fraction gives null; one below 0 or above 1 is SQLSTATE
/// 22003, even of a group of no rows; a group of no values gives null;
/// a null eliminated warns 01003, and none of no null; approximate,
/// DECFLOAT and `?` fractions pick as exact ones do.
#[test]
fn nulls_empty_groups_and_fractions_10_9() {
    let (_db, a) = distributed_rows();
    let nulls = "SELECT PERCENTILE_CONT(CAST(NULL AS INTEGER)) WITHIN GROUP (ORDER BY V), PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V) \
                 FROM T WHERE V IS NULL";
    assert_eq!(sorted(&a, nulls), ["NULL,NULL"]);
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "nulls eliminated");
    sorted(&a, "SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V) FROM T WHERE V IS NOT NULL");
    assert_eq!(a.warning(), None, "no null eliminated");
    let empty = "SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V), PERCENTILE_DISC(1) WITHIN GROUP (ORDER BY V) FROM T WHERE 1 = 0";
    assert_eq!(sorted(&a, empty), ["NULL,NULL"]);
    for sql in [
        "SELECT PERCENTILE_CONT(1.5) WITHIN GROUP (ORDER BY V) FROM T WHERE 1 = 0",
        "SELECT PERCENTILE_DISC(-0.01) WITHIN GROUP (ORDER BY V) FROM T",
        "SELECT G, PERCENTILE_DISC(1.0E0 + G) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G",
        "SELECT PERCENTILE_CONT(2) WITHIN GROUP (ORDER BY V) OVER (PARTITION BY G) FROM T",
    ] {
        assert_eq!(fetched(&a, sql).map_err(|e| e.state), Err(Some(SqlState::NUMERIC_VALUE_OUT_OF_RANGE)), "{sql}");
    }
    let typed = "SELECT PERCENTILE_CONT(0.25E0) WITHIN GROUP (ORDER BY V), PERCENTILE_CONT(CAST(0.25 AS DECFLOAT)) WITHIN GROUP (ORDER BY V), \
                 PERCENTILE_DISC(0.5E0) WITHIN GROUP (ORDER BY V), PERCENTILE_DISC(CAST(0.5 AS DECFLOAT)) WITHIN GROUP (ORDER BY V) FROM T";
    assert_eq!(sorted(&a, typed), ["2E0,2E0,3E0,3E0"]);
    let asked = prepared(
        &a,
        "SELECT G, PERCENTILE_CONT(?) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G ORDER BY G",
        &[Value::Exact(Decimal::new(25, 2))],
    );
    assert_eq!(asked, Ok(vec!["2E0".to_owned(), "3.75E0".to_owned(), "NULL".to_owned(), "3E0".to_owned()]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR14, GR4, §7.13
/// SR17] the fraction is not aggregated, so of a grouped query it may
/// name grouping columns, and an outer reference is a value of the
/// enclosing row; a filter leaves rows out; of grouping sets a column a
/// set leaves out is null in the fraction as in the select list; a view
/// of one gives what its query does; calls unlike in their function,
/// fraction, key, direction or filter are computed apart.
#[test]
fn grouping_sets_filters_outer_references_and_views_10_9() {
    let (_db, a) = distributed_rows();
    let grouped = "SELECT G, PERCENTILE_CONT(G / 4.0) WITHIN GROUP (ORDER BY V), PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V) \
                   FILTER (WHERE V > 1), PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V), PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V DESC) \
                   FROM T GROUP BY ROLLUP (G)";
    assert_eq!(
        sorted(&a, grouped),
        ["1,2E0,3E0,2E0,2E0", "2,5E0,5E0,5E0,5E0", "3,NULL,NULL,NULL,NULL", "4,3E0,3E0,3E0,3E0", "NULL,NULL,4.5E0,3.5E0,4E0"]
    );
    let outer = "SELECT X, (SELECT PERCENTILE_DISC(U.X / 4.0) WITHIN GROUP (ORDER BY V) FROM T) FROM U";
    assert_eq!(sorted(&a, outer), ["1,2E0", "2,3E0", "4,9E0"]);
    a.run("CREATE VIEW W (G, P) AS SELECT G, PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G").unwrap();
    let viewed = "SELECT G, PERCENTILE_CONT(0.75) WITHIN GROUP (ORDER BY V DESC) FROM T GROUP BY G";
    assert_eq!(sorted(&a, "SELECT G, P FROM W"), sorted(&a, viewed));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10, §9.23 SR8d i]
/// over a window of a partition alone each row's frame is its partition,
/// so each row has its group's value, of a DESC key from the greatest;
/// over an empty window, the whole table's; a fraction of each row picks for that row; a filter leaves
/// rows out; a null eliminated warns 01003.
#[test]
fn over_a_partition_10_9() {
    let (_db, a) = distributed_rows();
    let windowed = "SELECT I, PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY V DESC) OVER (PARTITION BY G), \
                    PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V) FILTER (WHERE V > 1) OVER (PARTITION BY G) FROM T";
    let grouped = "SELECT I, P, Q FROM T JOIN (SELECT G, PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY V DESC), \
                   PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V) FILTER (WHERE V > 1) FROM T GROUP BY G) AS S (H, P, Q) ON G = H";
    assert_eq!(sorted(&a, windowed), sorted(&a, grouped));
    assert_eq!(a.warning().map(SqlState::as_str), Some("01003"), "a null eliminated");
    let descending = "SELECT DISTINCT G, PERCENTILE_CONT(0.25) WITHIN GROUP (ORDER BY V DESC) OVER (PARTITION BY G) FROM T";
    assert_eq!(sorted(&a, descending), ["1,4E0", "2,6E0", "3,NULL", "4,3E0"]);
    let each = "SELECT I, P, Q FROM (SELECT I, PERCENTILE_DISC(I / 16.0) WITHIN GROUP (ORDER BY V) OVER (), \
                PERCENTILE_CONT(I / 16.0) WITHIN GROUP (ORDER BY V) OVER () FROM T) AS S (I, P, Q) WHERE I IN (2, 8, 12)";
    assert_eq!(sorted(&a, each), ["12,5E0,5E0", "2,1E0,1.125E0", "8,3E0,3.5E0"]);
    let whole = "SELECT DISTINCT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V) OVER () FROM T";
    assert_eq!(sorted(&a, whole), ["3.5E0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 GR10, RULES.md D38]
/// groups and partitions past the work area, their values sorted spilled,
/// give what they do in memory and what counting the values gives; so do
/// groups too many for the work area, made in passes, each sorting its
/// own groups' values.
#[test]
fn inverse_distribution_functions_past_the_work_area_10_9() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE M (I INTEGER, G INTEGER, H INTEGER, V INTEGER)").unwrap();
    let a = db.session("A");
    for first in (0_u32..6000).step_by(500) {
        // Group I mod 3 of the values 0 to 1999, each once; group I mod 997
        // of six or seven.
        let values: Vec<String> = (first..first.checked_add(500).unwrap())
            .map(|i| format!("({i}, {}, {}, {})", i.checked_rem(3).unwrap(), i.checked_rem(997).unwrap(), i.checked_div(3).unwrap()))
            .collect();
        a.run(&format!("INSERT INTO M VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let picked = "SELECT G, PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY V), PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V DESC), \
                  PERCENTILE_DISC(1) WITHIN GROUP (ORDER BY V DESC) FROM M GROUP BY G";
    let windowed = "SELECT DISTINCT G, PERCENTILE_DISC(0.25) WITHIN GROUP (ORDER BY V) OVER (PARTITION BY G), \
                    PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V DESC) OVER (PARTITION BY G), \
                    PERCENTILE_DISC(1) WITHIN GROUP (ORDER BY V DESC) OVER (PARTITION BY G) FROM M";
    // Of 2000 values 0 to 1999: the 500th, 499; between 999 and 1000; 0.
    let counted = ["0,4.99E2,9.995E2,0E0", "1,4.99E2,9.995E2,0E0", "2,4.99E2,9.995E2,0E0"];
    assert_eq!(sorted(&a, picked), counted);
    assert_eq!(sorted(&a, windowed), counted);
    let many = "SELECT H, PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V), PERCENTILE_CONT(0.4) WITHIN GROUP (ORDER BY V DESC) \
                FROM M GROUP BY H";
    let in_memory = sorted(&a, many);
    assert_eq!(in_memory.len(), 997);
    let spilled = Counted::default();
    db.set_spill(Box::new(spilled.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!(sorted(&a, picked), counted);
    assert!(spilled.taken() > 0, "the groups' values spilled");
    let before = spilled.taken();
    assert_eq!(sorted(&a, windowed), counted);
    assert!(spilled.taken() > before, "the partitions' values spilled");
    assert_eq!(sorted(&a, many), in_memory);
}

/// The SQLCODE of the query `sql`, and whether its error cites `rule`.
fn refused(a: &Session, sql: &str, rule: &str) -> (Code, bool) {
    let e = fetched(a, sql).expect_err(sql);
    let cited = format!("{}{}", e.msg, e.rule.as_deref().unwrap_or_default());
    (e.code, cited.contains(rule))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR12a, SR12b, SR12d,
/// SR14, §6.11 SR1, §9.23 SR8d i] an inverse distribution function of two
/// sort keys, of a subquery, set function or window function in its
/// fraction or key, of a nested window function, of a key neither numeric
/// nor an interval, of a fraction not numeric, of a fraction aggregated,
/// or over a window of an ordering or a frame, is SQLCODE -103, citing its
/// rule; a sort key of an outer reference, which would aggregate the
/// enclosing query, -900.
#[test]
fn inverse_distribution_functions_misused_are_refused_10_9() {
    let (_db, a) = distributed_rows();
    for (sql, rule) in [
        ("SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V, I) FROM T", "§10.9 SR12a"),
        ("SELECT PERCENTILE_CONT((SELECT MAX(X) / 4.0 FROM U)) WITHIN GROUP (ORDER BY V) FROM T", "§10.9 SR12b"),
        ("SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY CASE WHEN EXISTS (SELECT X FROM U) THEN V END) FROM T", "§10.9 SR12b"),
        ("SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY SUM(V)) FROM T", "§10.9 SR12b"),
        ("SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY ROW_NUMBER() OVER (ORDER BY I)) FROM T", "§10.9 SR12b"),
        ("SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY ROW_NUMBER(CURRENT_ROW)) FROM T", "§6.11 SR1"),
        ("SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY CAST(V AS VARCHAR(5))) FROM T", "§10.9 SR12d"),
        ("SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY CURRENT_DATE) FROM T", "§10.9 SR12d"),
        ("SELECT PERCENTILE_CONT('0.5') WITHIN GROUP (ORDER BY V) FROM T", "<numeric value expression>"),
        ("SELECT PERCENTILE_CONT(V / 10.0) WITHIN GROUP (ORDER BY V) FROM T", "§5.25 SR7"),
        ("SELECT G, PERCENTILE_CONT(I / 12.0) WITHIN GROUP (ORDER BY V) FROM T GROUP BY G", "§5.25 SR7"),
        ("SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V) OVER (ORDER BY I) FROM T", "§9.23 SR8d i"),
        ("SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY V) OVER (PARTITION BY G ROWS UNBOUNDED PRECEDING) FROM T", "§9.23 SR8d i"),
    ] {
        assert_eq!(refused(&a, sql, rule), (Code::SyntaxRule, true), "{sql}");
    }
    let keyed = "SELECT X, (SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY U.X) FROM T) FROM U";
    assert_eq!(fetched(&a, keyed).map_err(|e| e.code), Err(Code::Unsupported));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §5.2, §10.9] PERCENTILE_CONT
/// and PERCENTILE_DISC, no key words of SQL-92, are the functions only
/// before a parenthesis, so a column and a select item may be so named;
/// and WITHIN GROUP must follow the fraction.
#[test]
fn distribution_names_name_columns_5_2() {
    let (_db, a) = distributed_rows();
    a.run("CREATE TABLE N (PERCENTILE_CONT INTEGER, PERCENTILE_DISC INTEGER)").unwrap();
    a.run("INSERT INTO N VALUES (1, 2), (3, 4)").unwrap();
    assert_eq!(sorted(&a, "SELECT PERCENTILE_CONT, PERCENTILE_DISC FROM N"), ["1,2", "3,4"]);
    assert_eq!(sorted(&a, "SELECT PERCENTILE_DISC(0.5) WITHIN GROUP (ORDER BY PERCENTILE_CONT) FROM N"), ["1E0"]);
    assert_eq!(fetched(&a, "SELECT PERCENTILE_CONT(0.5) FROM T").map_err(|e| e.code), Err(Code::Syntax));
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.16, §10.9 GR10] an
/// assertion of an inverse distribution function is checked as its query
/// says. A value that leaves the median within the bound changes nothing,
/// and one that moves it past the bound is refused.
#[test]
fn constraints_of_inverse_distribution_functions_10_9() {
    let (_db, a) = distributed_rows();
    a.run("CREATE ASSERTION MIDDLE CHECK ((SELECT PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY V) FROM T) < 4)").unwrap();
    a.run("INSERT INTO T VALUES (13, 1, 0, NULL, NULL, NULL), (14, 1, NULL, NULL, NULL, NULL)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO T VALUES (15, 2, 50, NULL, NULL, NULL), (16, 2, 50, NULL, NULL, NULL)").err(), Some(Code::Constraint));
    a.rollback().unwrap();
}
