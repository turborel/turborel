//! The `<distinct predicate>` [ISO/IEC 9075-2:2023 §8.15, Features T151,
//! T152]: X IS [NOT] DISTINCT FROM Y, never Unknown, of values, rows,
//! arrays, distinct types and collated strings; where it is refused; as a
//! simple CASE's when operand; and a join on IS NOT DISTINCT FROM, hashed
//! on it, a null key matching a null key.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;
use turborel::threads::JobPolicy;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::{Counted, both_ways, exec, rows, sorted};

/// The truth of predicate `p` of each row of T, in the order of K: T, F or
/// U, so Unknown and False differ.
fn truths(s: &Session, p: &str) -> Vec<String> {
    let sql = format!("SELECT K, CASE WHEN {p} THEN 'T' WHEN NOT ({p}) THEN 'F' ELSE 'U' END FROM T ORDER BY K");
    rows(s, &sql).unwrap().iter().map(|r| r.split_once(',').map_or(String::new(), |(_, t)| t.to_owned())).collect()
}

/// Schema A: T of K and two INTEGERs A and B, holding each pairing of a
/// null and a value; O, of one row.
fn pairs() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER, A INTEGER, B INTEGER)
           CREATE TABLE O (K INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 1, 1), (2, 1, 2), (3, NULL, 1), (4, 1, NULL), (5, NULL, NULL)").unwrap();
    a.run("INSERT INTO O VALUES (1)").unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1a, GR1b, GR1c i, SR6] two
/// nulls are not distinct, a null and a value are either way round, two
/// values are if unequal; IS NOT DISTINCT FROM is the negation, and
/// neither is ever Unknown, where `=` is of a null. So IS FALSE of it
/// holds of the rows it is false of. 1 and 1.0E0 are one value, and 'a'
/// and 'a ' too, as pad space compares them [§8.2 GR3].
#[test]
fn values_are_distinct_8_15() {
    let db = pairs();
    let a = db.session("A");
    assert_eq!(truths(&a, "A IS DISTINCT FROM B"), ["F", "T", "T", "T", "F"]);
    assert_eq!(truths(&a, "A IS NOT DISTINCT FROM B"), ["T", "F", "F", "F", "T"]);
    assert_eq!(truths(&a, "NOT (A IS DISTINCT FROM B)"), ["T", "F", "F", "F", "T"]);
    assert_eq!(truths(&a, "A = B"), ["T", "F", "U", "U", "U"], "a comparison, for contrast");
    assert_eq!(rows(&a, "SELECT K FROM T WHERE (A IS DISTINCT FROM B) IS FALSE ORDER BY K").unwrap(), ["1", "5"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE (A IS DISTINCT FROM B) IS UNKNOWN").unwrap(), Vec::<String>::new());
    assert_eq!(rows(&a, "SELECT K FROM T WHERE A IS NOT DISTINCT FROM CAST(NULL AS INTEGER) ORDER BY K").unwrap(), ["3", "5"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE A + 1 IS DISTINCT FROM B * 2 ORDER BY K").unwrap(), ["2", "3", "4"]);
    for (p, expected) in [
        ("1 IS DISTINCT FROM 1.0E0", "F"),
        ("'a' IS DISTINCT FROM 'a '", "F"),
        ("'a' IS DISTINCT FROM 'b'", "T"),
        ("CAST(NULL AS INTEGER) IS NOT DISTINCT FROM CAST(NULL AS SMALLINT)", "T"),
        ("DATE '2020-01-01' IS DISTINCT FROM DATE '2020-01-02'", "T"),
    ] {
        assert_eq!(rows(&a, &format!("SELECT CASE WHEN {p} THEN 'T' ELSE 'F' END FROM O")).unwrap(), [expected], "{p}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1c iv, v, §7.1] row value
/// predicands are distinct if a pair of their fields is, a null field not
/// distinct from a null one; a value of a row type alike. Arrays of other
/// cardinalities are distinct, and of one if an element at one place is,
/// a null element not distinct from a null one, though `=` of them is
/// Unknown.
#[test]
fn rows_and_arrays_are_distinct_by_their_parts_8_15() {
    let db = pairs();
    let a = db.session("A");
    assert_eq!(truths(&a, "(A, B) IS DISTINCT FROM (1, CAST(NULL AS INTEGER))"), ["T", "T", "T", "F", "T"]);
    assert_eq!(truths(&a, "(A, B) IS NOT DISTINCT FROM (CAST(NULL AS INTEGER), CAST(NULL AS SMALLINT))"), ["F", "F", "F", "F", "T"]);
    assert_eq!(truths(&a, "(A, B) = (1, CAST(NULL AS INTEGER))"), ["U", "U", "U", "U", "U"], "a comparison, for contrast");
    assert_eq!(truths(&a, "ROW(A, B) IS DISTINCT FROM ROW(CAST(NULL AS INTEGER), 1)"), ["T", "T", "F", "T", "T"]);
    a.run("CREATE TABLE Z (K INTEGER, R ROW(X INTEGER, Y INTEGER), V INTEGER ARRAY[3])").unwrap();
    a.run("INSERT INTO Z VALUES (1, ROW(1, CAST(NULL AS INTEGER)), ARRAY[1, CAST(NULL AS INTEGER)]), (2, ROW(1, 2), ARRAY[1]), (3, NULL, ARRAY[1, 2])").unwrap();
    let of_z = |p: &str| rows(&a, &format!("SELECT K FROM Z WHERE {p} ORDER BY K")).unwrap();
    assert_eq!(of_z("R IS NOT DISTINCT FROM ROW(1, CAST(NULL AS INTEGER))"), ["1"]);
    assert_eq!(of_z("R IS DISTINCT FROM ROW(1, CAST(NULL AS INTEGER))"), ["2", "3"]);
    assert_eq!(of_z("V IS NOT DISTINCT FROM ARRAY[1, CAST(NULL AS INTEGER)]"), ["1"]);
    assert_eq!(of_z("V IS DISTINCT FROM ARRAY[1]"), ["1", "3"], "the cardinality");
    assert_eq!(of_z("V = ARRAY[1, CAST(NULL AS INTEGER)]"), Vec::<String>::new(), "a comparison, for contrast");
    assert_eq!(of_z("V IS NOT DISTINCT FROM ARRAY[1, 2]"), ["3"]);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 SR3, §9.11, §4.8.4] a value of a
/// distinct type is distinct from another of it as their sources are;
/// strings of a collation are as it compares them, so 'bob' is not
/// distinct from 'Bob' under CASE_INSENSITIVE, and COLLATE says another.
#[test]
fn distinct_types_and_collations_8_15() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TYPE EUROS AS NUMERIC(10, 2)
           CREATE TABLE M (K INTEGER, P EUROS, Q EUROS)
           CREATE TABLE P (NAME VARCHAR(20) COLLATE CASE_INSENSITIVE, AGE INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO M VALUES (1, CAST(1 AS EUROS), CAST(1.00 AS EUROS)), (2, CAST(1 AS EUROS), NULL), (3, NULL, NULL)").unwrap();
    a.run("INSERT INTO P VALUES ('Bob', 30), ('Ann', NULL), (NULL, 50)").unwrap();
    assert_eq!(rows(&a, "SELECT K FROM M WHERE P IS NOT DISTINCT FROM Q ORDER BY K").unwrap(), ["1", "3"]);
    assert_eq!(rows(&a, "SELECT K FROM M WHERE P IS DISTINCT FROM CAST(1 AS EUROS) ORDER BY K").unwrap(), ["3"]);
    assert_eq!(rows(&a, "SELECT AGE FROM P WHERE NAME IS NOT DISTINCT FROM 'bob'").unwrap(), ["30"]);
    assert_eq!(rows(&a, "SELECT AGE FROM P WHERE NAME IS NOT DISTINCT FROM CAST(NULL AS VARCHAR(5))").unwrap(), ["50"]);
    let binary = "SELECT AGE FROM P WHERE NAME IS NOT DISTINCT FROM 'bob' COLLATE UCS_BASIC";
    assert_eq!(rows(&a, binary).unwrap(), Vec::<String>::new(), "COLLATE");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 SR1, SR3, SR5] the predicands'
/// degrees must be equal and their respective values comparable, each an
/// operand of an equality operation, else SQLCODE -103: (A, B) and (1, 2,
/// 3); a number and a string; a distinct type and its source; a BOOLEAN
/// and a number; (A, B) and (1). A bare NULL is no row value predicand
/// [§7.2, §6.5], as of `=`.
#[test]
fn distinct_predicates_are_refused_8_15() {
    let db = pairs();
    let a = db.session("A");
    a.run("CREATE TYPE EUROS AS NUMERIC(10, 2)").unwrap();
    for refused in [
        "SELECT K FROM T WHERE (A, B) IS NOT DISTINCT FROM (1, 2, 3)",
        "SELECT K FROM T WHERE (A, B) IS DISTINCT FROM (1)",
        "SELECT K FROM T WHERE A IS DISTINCT FROM 'a'",
        "SELECT K FROM T WHERE CAST(A AS EUROS) IS DISTINCT FROM 1",
        "SELECT K FROM T WHERE TRUE IS DISTINCT FROM 1",
    ] {
        assert_eq!(rows(&a, refused).err(), Some(Code::SyntaxRule), "{refused}");
    }
    assert_eq!(rows(&a, "SELECT K FROM T WHERE A IS DISTINCT FROM NULL").err(), Some(Code::SyntaxRule), "NULL");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.12 SR2f ii, §8.15, Feature F262] a
/// simple CASE's when operand IS [NOT] DISTINCT FROM y is the predicate of
/// the case operand, so a null operand finds the WHEN of a null.
#[test]
fn distinct_when_operands_6_12() {
    let db = pairs();
    let a = db.session("A");
    let sql = "SELECT K, CASE A WHEN IS NOT DISTINCT FROM CAST(NULL AS INTEGER) THEN 'empty' WHEN IS DISTINCT FROM B THEN 'apart' ELSE 'alike' END \
               FROM T ORDER BY K";
    assert_eq!(rows(&a, sql).unwrap(), ["1,alike", "2,apart", "3,empty", "4,apart", "5,empty"]);
    let equal = "SELECT CASE A WHEN CAST(NULL AS INTEGER) THEN 'none' ELSE 'some' END FROM T WHERE K = 3";
    assert_eq!(rows(&a, equal).unwrap(), ["some"], "a comparison, for contrast");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15, §20.6 SR] [X3.135-1986 §5.11] a
/// dynamic parameter of a distinct predicate takes the other predicand's
/// type, as of `=`, and a null for it finds the null values; a CHECK
/// constraint of it is false of two nulls, so refuses them, where one of
/// `<>` is Unknown of them and allows them [§11.9 GR1].
#[test]
fn distinct_predicates_with_parameters_and_checks_8_15() {
    let db = pairs();
    let a = db.session("A");
    let p = a.prepare("SELECT K FROM T WHERE A IS NOT DISTINCT FROM ? ORDER BY K").unwrap();
    assert_eq!(
        p.parameters().iter().map(|t| t.clone().data_type()).collect::<Vec<_>>(),
        [DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }]
    );
    let given = |v: Value| {
        let mut cursor = a.open_prepared(&p, &[v]).unwrap();
        let mut out = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            out.push(row[0].to_string());
        }
        out
    };
    assert_eq!(given(Value::Null), ["3", "5"]);
    assert_eq!(given(Value::Exact(Decimal::new(1, 0))), ["1", "2", "4"]);
    a.run("CREATE TABLE C (A INTEGER, B INTEGER, CHECK (A IS DISTINCT FROM B))").unwrap();
    a.run("INSERT INTO C VALUES (NULL, 1), (1, 2)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO C VALUES (NULL, NULL)").err(), Some(Code::Constraint));
    assert_eq!(exec(&a, "INSERT INTO C VALUES (2, 2)").err(), Some(Code::Constraint));
    a.run("CREATE TABLE U (A INTEGER, B INTEGER, CHECK (A <> B))").unwrap();
    a.run("INSERT INTO U VALUES (NULL, NULL)").unwrap();
    a.run("CREATE VIEW V AS SELECT K FROM T WHERE A IS NOT DISTINCT FROM B").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM V"), ["1", "5"], "a view's text read back");
    a.rollback().unwrap();
}

/// Schema A: F, of the column store, and R, of the row store, of K and a
/// key G, of one of 7 values or null; D, naming the values of G and null
/// twice; N, of a collated NAME, null for some.
fn joined() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE F (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, B INTEGER) USING columnstore
           CREATE TABLE R (K INTEGER PRIMARY KEY, G INTEGER, H INTEGER, B INTEGER)
           CREATE TABLE D (G INTEGER, H INTEGER, NAME VARCHAR(10))
           CREATE TABLE N (NAME VARCHAR(10) COLLATE CASE_INSENSITIVE, N INTEGER)
           CREATE TABLE M (NAME VARCHAR(10) COLLATE CASE_INSENSITIVE, M INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for table in ["F", "R"] {
        for first in (0..6000).step_by(1000) {
            let values: Vec<String> = (first..first + 1000)
                .map(|k: i64| {
                    let g = if k % 11 == 0 { "NULL".to_owned() } else { (k % 7).to_string() };
                    let h = if k % 5 == 0 { "NULL".to_owned() } else { (k % 3).to_string() };
                    format!("({k}, {g}, {h}, {})", k * 3 % 1000)
                })
                .collect();
            a.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
    }
    a.run("INSERT INTO D VALUES (0, 0, 'zero'), (1, 1, 'one'), (2, NULL, 'two'), (NULL, 2, 'none'), (NULL, NULL, 'nil'), (9, 0, 'nine')")
        .unwrap();
    a.run("INSERT INTO N VALUES ('Bob', 1), ('ann', 2), (NULL, 3), (NULL, 4)").unwrap();
    a.run("INSERT INTO M VALUES ('bob', 10), ('ANN', 20), (NULL, 30), ('cy', 40)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Joins on IS NOT DISTINCT FROM, `{t}` the table, F or R, `{on}` the
/// condition its keys are matched by, so each is run with the predicate
/// and with the OR it means, which is no key.
const JOINS: [&str; 8] = [
    "SELECT COUNT(*), SUM({t}.B), MAX(D.NAME) FROM {t}, D WHERE {on}",
    "SELECT D.NAME, COUNT(*), SUM({t}.B) FROM {t} JOIN D ON {on} GROUP BY D.NAME",
    "SELECT D.NAME, COUNT({t}.K), SUM({t}.B) FROM D LEFT JOIN {t} ON {on} GROUP BY D.NAME",
    "SELECT COUNT(*), COUNT(D.NAME), SUM({t}.B) FROM {t} LEFT JOIN D ON {on}",
    "SELECT D.NAME, COUNT({t}.K) FROM {t} RIGHT JOIN D ON {on} GROUP BY D.NAME",
    "SELECT COUNT(*), COUNT(D.NAME), COUNT({t}.K) FROM {t} FULL JOIN D ON {on} AND D.NAME <> 'one'",
    "SELECT COUNT(*), SUM({t}.B) FROM {t} LEFT JOIN D ON {on} AND {t}.B / ({t}.K + 1) >= 0",
    "SELECT {t}.K, D.NAME FROM {t} FULL JOIN D ON {on} WHERE {t}.K < 40 OR {t}.K IS NULL",
];

/// The conditions of `JOINS`: the predicate, then the OR it means.
const KEYS: [(&str, &str); 2] = [
    ("{t}.G IS NOT DISTINCT FROM D.G", "({t}.G = D.G OR {t}.G IS NULL AND D.G IS NULL)"),
    ("{t}.G IS NOT DISTINCT FROM D.G AND {t}.H = D.H", "({t}.G = D.G OR {t}.G IS NULL AND D.G IS NULL) AND {t}.H = D.H"),
];

/// Runs each of `JOINS` on each of `KEYS` of F and R, by batches and row
/// by row: each gives what the OR it means gives.
fn joins_answer_as_their_meaning(a: &Session) {
    for query in JOINS {
        for (key, meant) in KEYS {
            for table in ["F", "R"] {
                let sql = query.replace("{on}", key).replace("{t}", table);
                let oracle = query.replace("{on}", meant).replace("{t}", table);
                let (batched, row) = both_ways(a, &sql);
                assert_eq!(batched, sorted(a, &oracle), "{sql}");
                assert_eq!(row, batched, "{sql}, row by row");
            }
        }
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15 GR1a, §7.10 GR1] [DESIGN] RULES.md
/// D31, D38: a join on IS NOT DISTINCT FROM of a column of each operand is
/// hashed on it, as EXPLAIN shows, a null key matching a null key; inner,
/// LEFT, RIGHT and FULL, by batches and row by row, of either store, with
/// a key of `=` beside it, gives what the condition it means gives, which
/// has no key; a key of `=` alone still matches no null.
#[test]
fn joins_on_distinct_keys_match_nulls_8_15() {
    let (_db, a) = joined();
    let plan = a.explain("SELECT F.K FROM F, D WHERE F.G IS NOT DISTINCT FROM D.G", &Params::new()).unwrap();
    assert!(plan.contains("HashJoin") && plan.contains("IS NOT DISTINCT FROM"), "{plan}");
    joins_answer_as_their_meaning(&a);
    // Of 6000 rows, 546 have a null G, each matching 'none' and 'nil'.
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM R, D WHERE R.G IS NOT DISTINCT FROM D.G AND D.G IS NULL").unwrap(), ["1092"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM R, D WHERE R.G = D.G AND D.G IS NULL").unwrap(), ["0"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15, §9.11] [EXTENSION] RULES.md D60:
/// a join on IS NOT DISTINCT FROM of collated strings matches them as the
/// collation compares them, and a null name a null name.
#[test]
fn collated_joins_on_distinct_keys_8_15() {
    let (_db, a) = joined();
    let sql = "SELECT N.N, M.M FROM N JOIN M ON N.NAME IS NOT DISTINCT FROM M.NAME";
    let plan = a.explain(sql, &Params::new()).unwrap();
    assert!(plan.contains("HashJoin"), "{plan}");
    let expected = ["1,10", "2,20", "3,30", "4,30"];
    let (batched, row) = both_ways(&a, sql);
    assert_eq!(batched, expected);
    assert_eq!(row, expected, "row by row");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §8.15] [DESIGN] RULES.md D38: with
/// MEMORY.WORK_BYTES at its least, joins on IS NOT DISTINCT FROM spill,
/// their operands partitioned on their keys, and give what they give in
/// memory, a null key partitioned with a null key.
#[test]
fn spilled_joins_on_distinct_keys_8_15() {
    let (db, a) = joined();
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    let spilled = [
        "SELECT F.H, COUNT(*), SUM(R.B) FROM F, R WHERE F.G IS NOT DISTINCT FROM R.G AND F.K = R.K GROUP BY F.H",
        "SELECT F.H, COUNT(R.K), SUM(R.B) FROM F LEFT JOIN R ON F.G IS NOT DISTINCT FROM R.G AND F.K = R.B GROUP BY F.H",
        "SELECT R.H, COUNT(F.K), COUNT(*) FROM F FULL JOIN R ON F.H IS NOT DISTINCT FROM R.H AND F.K = R.K GROUP BY R.H",
    ];
    let meant = |sql: &str| {
        sql.replace("F.G IS NOT DISTINCT FROM R.G", "(F.G = R.G OR F.G IS NULL AND R.G IS NULL)")
            .replace("F.H IS NOT DISTINCT FROM R.H", "(F.H = R.H OR F.H IS NULL AND R.H IS NULL)")
    };
    let in_memory: Vec<Vec<String>> = spilled.iter().map(|sql| sorted(&a, &meant(sql))).collect();
    assert_eq!(counted.taken(), 0, "the default MEMORY.WORK_BYTES holds them");
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    for (sql, expected) in spilled.iter().zip(&in_memory) {
        let (batched, row) = both_ways(&a, sql);
        assert!(counted.taken() > 0, "{sql} spilled");
        assert_eq!(&batched, expected, "{sql}");
        assert_eq!(&row, expected, "{sql}, row by row");
    }
}
