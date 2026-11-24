//! Exact numbers of 19 to 38 digits (RULES.md D695 to D699): their
//! arithmetic, casts, comparison, grouping, ordering and hashing, and
//! columns of them kept by the row store, the column store and a spill,
//! to `MAX_EXACT_PRECISION` [ISO/IEC 9075-2:2023 §6.1 SR27a, §6.30 SR1c]
//! and no further.

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::threads::JobPolicy;
use turborel::value::MAX_EXACT_PRECISION;

use crate::common::{Counted, Scratch, exec, os, refused, rows, sorted};

/// Schema H: TPC-H's PARTSUPP, SUPPLIER and NATION, of the columns
/// query 11 reads, as TPC-H declares them, of the row store, or of the
/// column store if `columns`.
fn tpch(columns: bool) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let using = if columns { " USING columnstore" } else { "" };
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION H
           CREATE TABLE NATION (N_NATIONKEY INTEGER, N_NAME CHAR(25)){using}
           CREATE TABLE SUPPLIER (S_SUPPKEY INTEGER, S_NATIONKEY INTEGER){using}
           CREATE TABLE PARTSUPP (PS_PARTKEY INTEGER, PS_SUPPKEY INTEGER, PS_AVAILQTY INTEGER, PS_SUPPLYCOST DECIMAL(15,2)){using}"
    ))
    .unwrap();
    let h = db.session("H");
    h.run("INSERT INTO NATION VALUES (0, 'GERMANY'), (1, 'FRANCE')").unwrap();
    h.run("INSERT INTO SUPPLIER VALUES (1, 0), (2, 0), (3, 1)").unwrap();
    h.run(
        "INSERT INTO PARTSUPP VALUES (1, 1, 5000, 2000000.00), (2, 1, 100, 30000.00), (3, 2, 9000, 1500000.00), \
         (1, 2, 10, 5.00), (4, 3, 9999, 9999999.99), (5, 2, 1, 1.00)",
    )
    .unwrap();
    h.commit().unwrap();
    db.maintain().unwrap();
    db
}

/// TPC-H query 11 as the benchmark runs it, its substitution parameters
/// validated: GERMANY and the fraction 0.0001, which the specification
/// writes with ten digits after the point; its sum named `"VALUE"`, as
/// VALUE is a reserved word.
const Q11: &str = r#"select
    ps_partkey,
    sum(ps_supplycost * ps_availqty) as "VALUE"
from
    partsupp,
    supplier,
    nation
where
    ps_suppkey = s_suppkey
    and s_nationkey = n_nationkey
    and n_name = 'GERMANY'
group by
    ps_partkey having
        sum(ps_supplycost * ps_availqty) > (
            select
                sum(ps_supplycost * ps_availqty) * 0.0001000000
            from
                partsupp,
                supplier,
                nation
            where
                ps_suppkey = s_suppkey
                and s_nationkey = n_nationkey
                and n_name = 'GERMANY'
        )
order by
    "VALUE" desc"#;

/// Verifies: [ISO/IEC 9075-2:2023 §6.30 SR1c iii, GR5a, §10.9 SR7g ii]
/// RULES.md D696: TPC-H query 11 runs as written. Its threshold, a sum of
/// DECIMAL(15,2) by INTEGER times a constant of scale 10, is of scale 12
/// and 19 digits, past the 18 digits an exact result held before, and is
/// exact; on either store, the parts above it come in order.
#[test]
fn tpch_query_11_runs_as_written() {
    for columns in [false, true] {
        let db = tpch(columns);
        let h = db.session("H");
        let threshold = "SELECT SUM(PS_SUPPLYCOST * PS_AVAILQTY) * 0.0001000000 FROM PARTSUPP, SUPPLIER, NATION \
                         WHERE PS_SUPPKEY = S_SUPPKEY AND S_NATIONKEY = N_NATIONKEY AND N_NAME = 'GERMANY'";
        assert_eq!(rows(&h, threshold).unwrap(), ["2350300.005100000000"], "column store: {columns}");
        assert_eq!(rows(&h, Q11).unwrap(), ["3,13500000000.00", "1,10000000050.00", "2,3000000.00"], "column store: {columns}");
    }
}

/// A session of a new database's schema A, with no table.
fn session(db: &Database) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE ONE (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO ONE VALUES (1)").unwrap();
    a
}

/// `n` nines.
fn nines(n: u32) -> String {
    "9".repeat(usize::try_from(n).unwrap())
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.30 SR1c, GR5, §5.3 SR28] RULES.md
/// D695, D697: sums, differences, products and quotients are exact to 38
/// digits, and a literal holds 38; a result of 39 is SQLSTATE 22003
/// (SQLCODE -301), and a literal of 39 breaks a Syntax Rule, SQLCODE
/// -103 [§5.3 SR29] (RULES.md D970).
#[test]
fn arithmetic_is_exact_to_38_digits_6_30() {
    let db = Database::new();
    let a = session(&db);
    let most = nines(MAX_EXACT_PRECISION);
    let one = |sql: &str| rows(&a, &format!("SELECT {sql} FROM ONE")).unwrap().join(";");
    assert_eq!(one(&format!("{most} + 0")), most);
    assert_eq!(one(&format!("-{most} - 0")), format!("-{most}"));
    assert_eq!(one("12345678901234567890 * 1000000000000000000"), "12345678901234567890000000000000000000");
    assert_eq!(one("999999999999999999 + 1"), "1000000000000000000");
    assert_eq!(one("123456789012345678.9012345678901234567 * 10"), "1234567890123456789.0123456789012345670");
    assert_eq!(one("1 / 3"), "0.333333");
    assert_eq!(one("2 / 3"), "0.666666", "truncated");
    assert_eq!(one("1.0000000000 / 3"), "0.333333333333", "past the dividend's scale and the divisor's digit");
    assert_eq!(one("100000000000000000000 / 8"), "12500000000000000000");
    assert_eq!(one("-12345678901234567890123.5 / 0.5"), "-24691357802469135780247.0");
    assert_eq!(refused(&a, &format!("SELECT {most} + 1 FROM ONE")), (-301, "22003".to_owned(), Some("X3.135-1986 §5.9 GR6".to_owned())));
    assert_eq!(refused(&a, "SELECT 10000000000000000000 * 10000000000000000000 FROM ONE").0, -301, "39 digits");
    assert_eq!(refused(&a, "SELECT 10 / 0.0000000000000000000000000000000000001 FROM ONE").0, -301, "a quotient of 39");
    assert_eq!(exec(&a, &format!("SELECT {} FROM ONE", nines(MAX_EXACT_PRECISION + 1))).err(), Some(Code::SyntaxRule));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.30 SR1c iv, §10.9 SR7g ii] RULES.md
/// D696, D697: a quotient of INTEGER values keeps room for its integer
/// digits, so a sum of a thousand of them is exact, at their scale, as a
/// row at a time and in batches, and a table made of one has the
/// quotient's type.
#[test]
fn sums_of_quotients_keep_integer_room() {
    let db = Database::new();
    let a = session(&db);
    a.run("CREATE TABLE N (K INTEGER)").unwrap();
    let thousand = "WITH RECURSIVE R (I) AS (VALUES (CAST(1 AS INTEGER)) UNION ALL SELECT I + 1 FROM R WHERE I < 1000) SELECT I FROM R";
    a.run(&format!("INSERT INTO N {thousand}")).unwrap();
    for batches in ["ON", "OFF"] {
        a.run(&format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
        assert_eq!(rows(&a, "SELECT SUM(100 / K) FROM N").unwrap(), ["748.546612"], "{batches}");
        assert_eq!(rows(&a, "SELECT SUM(100000000000 / K) FROM N").unwrap(), ["748547086055.034031"], "{batches}");
    }
    a.run("CREATE TABLE Q AS (SELECT K / 7 AS V FROM N) WITH DATA").unwrap();
    let types = "SELECT NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'Q'";
    assert_eq!(rows(&a, types).unwrap(), ["16,6"]);
    assert_eq!(rows(&a, "SELECT MAX(V) FROM Q").unwrap(), ["142.857142"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.13 GR8a, §9.2 GR3k; §8.2 GR2]
/// RULES.md D695: CAST to and from types of 19 to 38 digits keeps every
/// digit, truncates past the scale and is SQLSTATE 22003 past the
/// precision; wide values compare by their algebraic value, with each
/// other, narrow ones and approximate ones.
#[test]
fn wide_values_cast_and_compare_6_13() {
    let db = Database::new();
    let a = session(&db);
    let one = |sql: &str| rows(&a, &format!("SELECT {sql} FROM ONE")).unwrap().join(";");
    assert_eq!(one("CAST('123456789012345678901234567890' AS DECIMAL(30,0))"), "123456789012345678901234567890");
    assert_eq!(one("CAST(1234567890123456789012.987654 AS NUMERIC(25,3))"), "1234567890123456789012.987");
    assert_eq!(one("CAST(CAST(5 AS INTEGER) AS DECIMAL(38,30))"), format!("5.{}", "0".repeat(30)));
    assert_eq!(one("CAST(12345678901234567890123 AS DOUBLE PRECISION) > 1.2E22"), "TRUE");
    assert_eq!(one("CAST(12345678901234567890123 AS CHARACTER VARYING(40))"), "12345678901234567890123");
    assert_eq!(refused(&a, "SELECT CAST(123456789012345678901 AS DECIMAL(20,0)) FROM ONE").1, "22003");
    assert_eq!(one("12345678901234567890123 > 12345678901234567890122"), "TRUE");
    assert_eq!(one("12345678901234567890123.00 = 12345678901234567890123"), "TRUE");
    assert_eq!(one("-12345678901234567890123 < 5"), "TRUE");
    assert_eq!(one("12345678901234567890123 BETWEEN 1.2E22 AND 1.3E22"), "TRUE");
}

/// Schema A's table W of wide columns, of the row store or the column
/// store, its rows frozen into groups, with a primary key on its wide
/// column D and an index on N.
fn wide_table(a: &Session, columns: bool) {
    let using = if columns { " USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)" } else { "" };
    a.run(&format!("CREATE TABLE W (D DECIMAL(38,10) PRIMARY KEY, N NUMERIC(25,0), K INTEGER){using}")).unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX WN ON W (N)").unwrap();
    a.commit().unwrap();
    for block in (0..4500_i64).step_by(500) {
        let row = |k: i64| {
            let n = if k % 7 == 0 { "NULL".to_owned() } else { format!("{}000000000000000000000000", k % 5 - 2) };
            format!("({}{k:04}.0123456789 * {}, {n}, {k})", "9".repeat(24), 1 - 2 * (k % 2))
        };
        let values: Vec<String> = (block..block + 500).map(row).collect();
        a.run(&format!("INSERT INTO W VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
}

/// What `wide_table`'s rows give a query of order, ranges, grouping,
/// keys and sums.
fn read_wide(a: &Session) -> Vec<Vec<String>> {
    vec![
        rows(a, "SELECT D FROM W ORDER BY D FETCH FIRST 2 ROWS ONLY").unwrap(),
        rows(a, "SELECT D FROM W ORDER BY D DESC FETCH FIRST 2 ROWS ONLY").unwrap(),
        rows(a, "SELECT K FROM W WHERE D = 9999999999999999999999991001.0123456789 * -1").unwrap(),
        rows(a, "SELECT COUNT(*) FROM W WHERE D > -9999999999999999999999991100 AND D < 9999999999999999999999991100").unwrap(),
        // Each D is near 10^38 units: two of a sign would pass the 128
        // bits a sum is kept in, so the groups give their least.
        rows(a, "SELECT N, COUNT(*), MIN(D) FROM W GROUP BY N ORDER BY N").unwrap(),
        rows(a, "SELECT COUNT(*) FROM W WHERE N = -2000000000000000000000000").unwrap(),
        rows(a, "SELECT SUM(N), MAX(N), MIN(D), COUNT(DISTINCT N) FROM W").unwrap(),
    ]
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1 SR27a, §8.2 GR2, §10.9] RULES.md
/// D698: columns of 25 and 38 digits are stored in 128 bits by the row
/// store, the column store and their keys, and read back whole: ordered,
/// found by a key and by a range, grouped and summed alike on both stores.
#[test]
fn wide_columns_round_trip_on_both_stores() {
    let mut read = Vec::new();
    for columns in [false, true] {
        let db = Database::new();
        db.set_job_policy(JobPolicy::Manual).unwrap();
        let a = session(&db);
        wide_table(&a, columns);
        db.maintain().unwrap();
        read.push(read_wide(&a));
    }
    let first = &read[0];
    assert_eq!(first[0], ["-9999999999999999999999994499.0123456789", "-9999999999999999999999994497.0123456789"]);
    assert_eq!(first[1], ["9999999999999999999999994498.0123456789", "9999999999999999999999994496.0123456789"]);
    assert_eq!(first[2], ["1001"]);
    assert_eq!(first[3], ["1100"]);
    assert_eq!(first[4].len(), 6);
    assert_eq!(first[6], ["0,2000000000000000000000000,-9999999999999999999999994499.0123456789,5"]);
    assert_eq!(read[1], read[0], "the column store reads as the row store");
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1 SR27a] RULES.md D698: a table of
/// wide columns is kept whole when the database is opened again, from its
/// log and from its checkpoint.
#[test]
fn wide_columns_are_kept_on_disk() {
    let scratch = Scratch::new("wide-exact");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    let a = session(&db);
    wide_table(&a, false);
    let expected = read_wide(&a);
    drop((a, db));
    let db = open();
    assert_eq!(read_wide(&db.session("A")), expected, "from the log");
    db.close().unwrap();
    drop(db);
    assert_eq!(read_wide(&open().session("A")), expected, "from the checkpoint");
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR2, §7.13, §7.10] RULES.md D698:
/// wide values equal at different scales are one group, one DISTINCT
/// value and one join key, their hashes equal, and order as their values
/// do.
#[test]
fn equal_wide_values_at_other_scales_group_and_join() {
    let db = Database::new();
    let a = session(&db);
    a.run("CREATE TABLE P (V DECIMAL(30,0))").unwrap();
    a.run("CREATE TABLE Q (V DECIMAL(34,4), T CHARACTER(1))").unwrap();
    a.run("INSERT INTO P VALUES (123456789012345678901234567890), (-123456789012345678901234567890), (7)").unwrap();
    a.run("INSERT INTO Q VALUES (123456789012345678901234567890.0000, 'a'), (7.0000, 'b'), (7.5, 'c')").unwrap();
    assert_eq!(sorted(&a, "SELECT T FROM P JOIN Q ON P.V = Q.V"), ["a", "b"]);
    let both = "SELECT V FROM P UNION ALL SELECT V FROM Q";
    assert_eq!(
        rows(&a, &format!("SELECT V, COUNT(*) FROM ({both}) AS U GROUP BY V ORDER BY V")).unwrap(),
        ["-123456789012345678901234567890.0000,1", "7.0000,2", "7.5000,1", "123456789012345678901234567890.0000,2"]
    );
    assert_eq!(rows(&a, &format!("SELECT COUNT(DISTINCT V) FROM ({both}) AS U")).unwrap(), ["4"]);
    assert_eq!(sorted(&a, "SELECT V FROM P INTERSECT SELECT V FROM Q"), ["123456789012345678901234567890.0000", "7.0000"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §7.13, §10.10] RULES.md D38, D698: wide
/// keys and sums grouped and sorted past the work area, spilled, are as
/// in memory.
#[test]
fn wide_values_spill_as_in_memory() {
    let db = Database::new();
    let a = session(&db);
    a.run("CREATE TABLE S (G DECIMAL(30,2), V DECIMAL(36,6))").unwrap();
    for first in (0..4000).step_by(500) {
        let values: Vec<String> =
            (first..first + 500).map(|i| format!("({}{:03}.25, {}{i}.000001)", "8".repeat(25), i % 997, "7".repeat(25))).collect();
        a.run(&format!("INSERT INTO S VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    let grouped = "SELECT G, COUNT(*), SUM(V) FROM S GROUP BY G";
    let ordered = "SELECT V, G FROM S ORDER BY V DESC, G";
    let in_memory = (sorted(&a, grouped), rows(&a, ordered).unwrap());
    assert_eq!(in_memory.0.len(), 997);
    let counted = Counted::default();
    db.set_spill(Box::new(counted.clone())).unwrap();
    exec(&a, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    assert_eq!((sorted(&a, grouped), rows(&a, ordered).unwrap()), in_memory);
    assert!(counted.taken() > 0, "they spilled");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR16, §14.11 GR, §6.30 SR1c]
/// RULES.md D696, D698: a result of more than 18 digits is stored by
/// CREATE TABLE AS of its type, and by INSERT ... SELECT into a wide
/// column, and read back whole; a value past the column's precision is
/// SQLSTATE 22003.
#[test]
fn wide_results_are_stored_by_create_and_insert() {
    let db = Database::new();
    let a = session(&db);
    a.run("CREATE TABLE B (X DECIMAL(15,2), Y INTEGER)").unwrap();
    a.run("INSERT INTO B VALUES (9999999999999.99, 2147483647), (-1.50, 3)").unwrap();
    a.run("CREATE TABLE M AS (SELECT X * Y AS P, SUM(X) AS S FROM B GROUP BY X, Y) WITH DATA").unwrap();
    let types = "SELECT COLUMN_NAME, NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'M' ORDER BY 1";
    assert_eq!(rows(&a, types).unwrap(), ["P,25,2", "S,38,2"]);
    assert_eq!(sorted(&a, "SELECT P FROM M"), ["-4.50", "21474836469999978525163.53"]);
    a.run("CREATE TABLE I (V DECIMAL(38,4))").unwrap();
    a.run("INSERT INTO I SELECT X * Y * Y FROM B").unwrap();
    assert_eq!(sorted(&a, "SELECT V FROM I"), ["-13.5000", "46116860141324159973139858675793.9100"]);
    assert_eq!(refused(&a, "SELECT X * Y * X FROM B").1, "22003", "40 digits");
    assert_eq!(exec(&a, "INSERT INTO I SELECT X * Y * Y * 10000000 FROM B").err(), Some(Code::Data), "past DECIMAL(38,4)");
}
