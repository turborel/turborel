//! Decimal floating point [ISO/IEC 9075-2:2023 §4.5, §6.1]: DECFLOAT, its
//! values' assignment, arithmetic, comparison, CAST, keys and set
//! functions.

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{Scratch, code, exec, os, rows, sorted};

/// Schema A: table T of a DECFLOAT D, a DECFLOAT(16) S and an INTEGER K.
fn decimals() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (D DECFLOAT, S DECFLOAT(16), K INTEGER)").unwrap();
    db
}

/// The rows of `sql`, each with its runs of white space made one space.
fn squeezed(s: &turborel::db::Session, sql: &str) -> Vec<String> {
    rows(s, sql).unwrap().iter().map(|r| r.split_whitespace().collect::<Vec<_>>().join(" ")).collect()
}

/// The Information Schema's DATA_TYPE, NUMERIC_PRECISION and
/// NUMERIC_PRECISION_RADIX of the columns of `table`, by name.
fn described(s: &turborel::db::Session, table: &str) -> Vec<String> {
    let sql = format!(
        "SELECT COLUMN_NAME, DATA_TYPE, NUMERIC_PRECISION, NUMERIC_PRECISION_RADIX, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS \
         WHERE TABLE_NAME = '{table}' ORDER BY 1"
    );
    squeezed(s, &sql)
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.1 SR22, SR23, SR35, GR18]
/// DECFLOAT, of 34 digits if no precision is written (ID062), DECFLOAT(p)
/// of p from 1 to 34 (IA021), described as DECFLOAT of decimal precision;
/// a precision of 0 is refused (SQLCODE -103), one past 34 is valid but
/// not processed (SQLCODE -900, RULES.md D18). DECFLOAT is no key word, so
/// a column or a domain keeps the name, yet DECFLOAT as a column's data
/// type is always the type.
#[test]
fn the_decfloat_type_6_1() {
    let db = decimals();
    let a = db.session("A");
    a.run("CREATE TABLE U (A DECFLOAT, B DECFLOAT(16), C DECFLOAT(1))").unwrap();
    assert_eq!(described(&a, "U"), ["A,DECFLOAT,34,10,NULL", "B,DECFLOAT,16,10,NULL", "C,DECFLOAT,1,10,NULL"]);
    assert_eq!(exec(&a, "CREATE TABLE W (A DECFLOAT(0))").err(), Some(Code::SyntaxRule), "more than 0");
    assert_eq!(exec(&a, "CREATE TABLE W (A DECFLOAT(35))").err(), Some(Code::Unsupported), "past 34 here");
    a.run("CREATE TABLE W (DECFLOAT INTEGER)").unwrap();
    assert_eq!(rows(&a, "SELECT DECFLOAT FROM W").unwrap(), Vec::<String>::new(), "the name of a column");
    a.run("CREATE DOMAIN DECFLOAT AS INTEGER").unwrap();
    a.run("CREATE TABLE X (A DECFLOAT)").unwrap();
    assert_eq!(described(&a, "X"), ["A,DECFLOAT,34,10,NULL"], "the type, not the domain");
    a.commit().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.2 GR3b xv, §4.5.2] a
/// number assigned to DECFLOAT(p) is rounded to p digits, half to even
/// (IA192); an exact number keeps its value, 1.50 being 1.5; one out of
/// range is refused (SQLSTATE 22003), and a character string too (§9.2
/// SR3). A DECFLOAT value assigned to an exact column keeps its digits
/// to the column's scale, the others truncated (IA002), unless it loses
/// leading digits (22003); to an approximate one, its nearest, unless it
/// is past its range (22003).
#[test]
fn decfloat_values_are_assigned_9_2() {
    let db = decimals();
    let a = db.session("A");
    a.run(
        "INSERT INTO T VALUES (1.50, 1.50, 1), (CAST('1.2345678901234567890' AS DECFLOAT), CAST('1.2345678901234567890' AS DECFLOAT), 2)",
    )
    .unwrap();
    a.run("INSERT INTO T VALUES (2.5E0, 12345678901234565, 3), (-0.0, CAST('12345678901234575' AS DECFLOAT), 4)").unwrap();
    assert_eq!(
        sorted(&a, "SELECT K, D, S FROM T"),
        ["1,1.5,1.5", "2,1.234567890123456789,1.234567890123457", "3,2.5,12345678901234560", "4,0,12345678901234580"]
    );
    assert_eq!(exec(&a, "INSERT INTO T (D) VALUES ('1.5')").err(), Some(Code::SyntaxRule), "a character string");
    assert_eq!(exec(&a, "INSERT INTO T (D) VALUES (CAST('1E7000' AS DECFLOAT))").err(), Some(Code::Data), "out of range");
    a.run("CREATE TABLE N (M NUMERIC(5,2), F DOUBLE PRECISION, R REAL)").unwrap();
    a.run("INSERT INTO N SELECT D, D, D FROM T WHERE K = 2").unwrap();
    assert_eq!(rows(&a, "SELECT M, F FROM N").unwrap(), ["1.23,1.2345678901234567E0"]);
    assert_eq!(exec(&a, "INSERT INTO N (M) VALUES (CAST('-999.999' AS DECFLOAT))").map(|_| ()), Ok(()), "truncated, not rounded");
    assert_eq!(rows(&a, "SELECT M FROM N WHERE M < 0").unwrap(), ["-999.99"]);
    assert_eq!(exec(&a, "INSERT INTO N (M) VALUES (CAST('1000' AS DECFLOAT))").err(), Some(Code::Data), "a leading digit lost");
    assert_eq!(exec(&a, "INSERT INTO N (F) VALUES (CAST('1E400' AS DECFLOAT))").err(), Some(Code::Data), "past DOUBLE PRECISION");
    assert_eq!(exec(&a, "INSERT INTO N (R) VALUES (CAST('1E100' AS DECFLOAT))").err(), Some(Code::Data), "past REAL");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.30 SR1a, GR4, GR7] a
/// dyadic operator with a DECFLOAT operand is DECFLOAT(34) (IV134), its
/// result the value of 34 digits nearest the exact one, so 0.1 + 0.2 is
/// 0.3; an exact or approximate operand is taken as DECFLOAT; a divisor
/// of 0 is refused (SQLSTATE 22012), a result past the exponent's range
/// too (22003). Monadic minus reverses the sign.
#[test]
fn decfloat_arithmetic_6_30() {
    let db = decimals();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (CAST('0.1' AS DECFLOAT), CAST('0.2' AS DECFLOAT), 3)").unwrap();
    assert_eq!(rows(&a, "SELECT D + S, D + S = CAST('0.3' AS DECFLOAT), D * K, -D, D - 1 FROM T").unwrap(), ["0.3,TRUE,0.3,-0.1,-0.9"]);
    assert_eq!(rows(&a, "SELECT S / K, K / S FROM T").unwrap(), [format!("0.0{}7,15", "6".repeat(33))]);
    assert_eq!(rows(&a, "SELECT S * 0.1E0 FROM T").unwrap(), ["0.02000000000000000111022302462515654"], "the double's binary value");
    a.run("CREATE VIEW V (X, Y) AS SELECT S + K, S * 1.5E0 FROM T").unwrap();
    assert_eq!(described(&a, "V"), ["X,DECFLOAT,34,10,NULL", "Y,DECFLOAT,34,10,NULL"]);
    assert_eq!(code(&a, "SELECT D / 0 FROM T"), Code::Data, "22012");
    assert_eq!(code(&a, "SELECT D / CAST('0' AS DECFLOAT) FROM T"), Code::Data, "22012");
    assert_eq!(code(&a, "SELECT CAST('1E6144' AS DECFLOAT) * 10 FROM T"), Code::Data, "22003");
    assert_eq!(code(&a, "SELECT D + 'A' FROM T"), Code::SyntaxRule, "no character string");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.5.1, §8.2 GR3] any two
/// numbers compare by value: a DECFLOAT with an exact number and with an
/// approximate one, exactly; 1.5 and 1.50 equal, so one group, one
/// distinct value, one key; ordered by value, negatives first. A
/// DECFLOAT column joins an exact one by their values.
#[test]
fn decfloat_values_compare_8_2() {
    let db = decimals();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1.5, 1.50, 1), (-2, CAST('1E-30' AS DECFLOAT), 2), (CAST('1E30' AS DECFLOAT), 0.1E0, 3)").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE D = 1.5 OR D = -2.0 OR D > 1E29"), ["1", "2", "3"]);
    assert_eq!(rows(&a, "SELECT K FROM T WHERE S = 0.1").unwrap(), ["3"], "the double rounded to 16 digits");
    assert_eq!(rows(&a, "SELECT K FROM T WHERE S = 0.1E0").unwrap(), Vec::<String>::new(), "0.1 is not the double nearest it");
    assert_eq!(rows(&a, "SELECT D FROM T ORDER BY D").unwrap(), ["-2", "1.5", "1000000000000000000000000000000"]);
    assert_eq!(rows(&a, "SELECT S FROM T ORDER BY S DESC").unwrap(), ["1.5", "0.1", "1E-30"]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM (SELECT D FROM T UNION SELECT S FROM T WHERE K = 1) AS U (X)").unwrap(), ["3"]);
    assert_eq!(rows(&a, "SELECT COUNT(DISTINCT X) FROM (SELECT D FROM T UNION ALL SELECT S FROM T) AS U (X)").unwrap(), ["5"]);
    a.run("CREATE TABLE E (N NUMERIC(4,2))").unwrap();
    a.run("INSERT INTO E VALUES (1.50), (-2), (7)").unwrap();
    assert_eq!(sorted(&a, "SELECT K FROM T, E WHERE D = N"), ["1", "2"], "joined by value");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.13 SR6, GR10-GR12] CAST
/// of a character string to DECFLOAT reads its `<signed numeric
/// literal>`, its spaces trimmed, rounded to the precision (SQLSTATE
/// 22018 if it is none, 22003 if out of range); of DECFLOAT to a
/// character string writes it plain, or with an exponent past 6 zeros
/// after the point or 34 digits before it (IV110), 22001 if too long; to
/// an exact number truncates past the scale. No CAST takes DECFLOAT to
/// or from a datetime, an interval or a binary string (SQLCODE -103).
#[test]
fn decfloat_values_are_cast_6_13() {
    let db = decimals();
    let a = db.session("A");
    a.run("INSERT INTO T (K) VALUES (1)").unwrap();
    let cast = |text: &str, t: &str| rows(&a, &format!("SELECT CAST(CAST('{text}' AS DECFLOAT) AS {t}) FROM T"));
    assert_eq!(cast("  -1_000.50 ", "VARCHAR(40)"), Ok(vec!["-1000.5".to_owned()]), "spaces trimmed, underscores read");
    assert_eq!(cast("12.5E-3", "VARCHAR(40)"), Ok(vec!["0.0125".to_owned()]));
    assert_eq!(cast("0.0000001", "VARCHAR(40)"), Ok(vec!["1E-7".to_owned()]));
    assert_eq!(cast("1.5E40", "VARCHAR(40)"), Ok(vec!["1.5E40".to_owned()]));
    assert_eq!(cast("100", "CHAR(5)"), Ok(vec!["100  ".to_owned()]));
    assert_eq!(cast("123456", "CHAR(5)"), Err(Code::Data), "22001");
    assert_eq!(cast("-7.89", "INTEGER"), Ok(vec!["-7".to_owned()]), "truncated");
    assert_eq!(cast("1.5", "DOUBLE PRECISION"), Ok(vec!["1.5E0".to_owned()]));
    assert_eq!(
        rows(
            &a,
            "SELECT CAST(CAST('2.675' AS DECFLOAT) AS DECFLOAT(3)), CAST(2.665 AS DECFLOAT(3)), CAST(2.6650001 AS DECFLOAT(3)) FROM T"
        )
        .unwrap(),
        ["2.68,2.66,2.67"]
    );
    assert_eq!(code(&a, "SELECT CAST('1.5.0' AS DECFLOAT) FROM T"), Code::Data, "22018");
    assert_eq!(code(&a, "SELECT CAST('1E9999' AS DECFLOAT) FROM T"), Code::Data, "22003");
    assert_eq!(code(&a, "SELECT CAST(CAST('1' AS DECFLOAT) AS INTERVAL DAY) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CAST(INTERVAL '1' DAY AS DECFLOAT) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CAST(DATE '2026-11-11' AS DECFLOAT) FROM T"), Code::SyntaxRule);
    assert_eq!(code(&a, "SELECT CAST(X'01' AS DECFLOAT) FROM T"), Code::SyntaxRule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.9 SR7f, SR7g v, §9.5
/// SR3c ii 1] SUM and AVG of DECFLOAT values are DECFLOAT(34) (IV170),
/// each sum of 34 digits; MAX and MIN of its type; CASE, UNION and
/// COALESCE of DECFLOAT and other numbers DECFLOAT of the greatest
/// precision.
#[test]
fn decfloat_set_functions_and_set_types_10_9() {
    let db = decimals();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (CAST('0.1' AS DECFLOAT), 1, 1), (CAST('0.2' AS DECFLOAT), 2, 2), (NULL, 4, 3)").unwrap();
    assert_eq!(
        rows(&a, "SELECT SUM(D), AVG(D), MAX(D), MIN(S), COUNT(D), AVG(S) FROM T").unwrap(),
        [format!("0.3,0.15,0.2,1,2,2.{}3", "3".repeat(32))]
    );
    a.run("CREATE VIEW G (SU, AV, MA, CA, UN) AS SELECT SUM(S), AVG(S), MAX(S), CASE WHEN K = 1 THEN S ELSE K END, COALESCE(S, 1.5E0) FROM T GROUP BY S, K").unwrap();
    assert_eq!(
        described(&a, "G"),
        ["AV,DECFLOAT,34,10,NULL", "CA,DECFLOAT,16,10,NULL", "MA,DECFLOAT,16,10,NULL", "SU,DECFLOAT,34,10,NULL", "UN,DECFLOAT,16,10,NULL"]
    );
    a.run("CREATE VIEW H (X) AS SELECT S FROM T UNION SELECT D FROM T UNION SELECT K FROM T").unwrap();
    assert_eq!(described(&a, "H"), ["X,DECFLOAT,34,10,NULL"]);
    assert_eq!(code(&a, "SELECT CASE WHEN K = 1 THEN D ELSE 'A' END FROM T"), Code::SyntaxRule);
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 SR4a, §11.7] a
/// DECFLOAT column's default is a numeric literal, an exact or an
/// approximate one, rounded to its precision; a UNIQUE DECFLOAT column
/// holds no two equal values, 1.5 and 1.50 being equal, of either sign
/// and of exponents far apart; its index is probed by DECFLOAT and by
/// exact values.
#[test]
fn decfloat_defaults_keys_and_indexes_11_7() {
    let db = decimals();
    let a = db.session("A");
    a.run("CREATE TABLE U (V DECFLOAT(4) UNIQUE, W DECFLOAT DEFAULT 2.25, Z DECFLOAT(2) DEFAULT 1.25E0)").unwrap();
    a.run("INSERT INTO U (V) VALUES (1.5), (-1.5), (CAST('1E-6000' AS DECFLOAT)), (CAST('1E6000' AS DECFLOAT)), (0)").unwrap();
    assert_eq!(rows(&a, "SELECT DISTINCT W, Z FROM U").unwrap(), ["2.25,1.2"], "defaults, rounded half to even");
    assert_eq!(exec(&a, "INSERT INTO U (V) VALUES (1.50)").err(), Some(Code::Constraint), "1.50 is 1.5");
    assert_eq!(
        exec(&a, "INSERT INTO U (V) VALUES (CAST('1.5001' AS DECFLOAT))").err(),
        Some(Code::Constraint),
        "rounded to 4 digits first"
    );
    assert_eq!(exec(&a, "INSERT INTO U (V) VALUES (-0.0)").err(), Some(Code::Constraint), "-0 is 0");
    a.run("INSERT INTO U (V) VALUES (1.502)").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM U WHERE V = 1.5").unwrap(), ["1.5"]);
    assert_eq!(rows(&a, "SELECT V FROM U WHERE V > 1 ORDER BY V").unwrap(), ["1.5", "1.502", "1E6000"]);
    assert_eq!(rows(&a, "SELECT V FROM U WHERE V < 0.000001 AND V > -1 ORDER BY V").unwrap(), ["0", "1E-6000"]);
    a.run("INSERT INTO U (V) VALUES (-2), (CAST('-1E-6000' AS DECFLOAT))").unwrap();
    assert_eq!(rows(&a, "SELECT V FROM U WHERE V > -1.6 AND V < -1").unwrap(), ["-1.5"], "negatives in order");
    assert_eq!(rows(&a, "SELECT V FROM U WHERE V < 0 ORDER BY V").unwrap(), ["-2", "-1.5", "-1E-6000"]);
    a.run("CREATE TABLE P (X DECFLOAT, Y DECFLOAT, UNIQUE (X, Y))").unwrap();
    a.run("INSERT INTO P VALUES (1, 10), (10, 1), (1.5, 1), (CAST('1E-40' AS DECFLOAT), 1)").unwrap();
    assert_eq!(exec(&a, "INSERT INTO P VALUES (1.0, 10.00)").err(), Some(Code::Constraint), "of two columns");
    a.rollback().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.5.2] DECFLOAT values and
/// a UNIQUE DECFLOAT column's index are kept: after the database is
/// opened again, from its log and from its checkpoint.
#[test]
fn decfloat_values_are_kept() {
    let scratch = Scratch::new("decfloat");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (D DECFLOAT UNIQUE, S DECFLOAT(7))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (CAST('-1.23456789012345678901234567890123E-6000' AS DECFLOAT), 1.5), (0, CAST('12345678' AS DECFLOAT))")
        .unwrap();
    a.commit().unwrap();
    drop((a, db));
    let kept = |db: &Database| {
        let a = db.session("A");
        assert_eq!(sorted(&a, "SELECT D, S FROM T"), ["-1.23456789012345678901234567890123E-6000,1.5", "0,12345680"]);
        assert_eq!(exec(&a, "INSERT INTO T VALUES (0.000, 1)").err(), Some(Code::Constraint), "its index kept");
        a.rollback().unwrap();
    };
    let db = open();
    kept(&db);
    db.close().unwrap();
    drop(db);
    kept(&open());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.1 GR5o] a DECFLOAT value
/// retrieved into a parameter of DECFLOAT(p) is rounded to p digits; into
/// an exact one, truncated to its scale; one that loses a leading digit
/// is refused (SQLSTATE 22003).
#[test]
fn decfloat_values_are_retrieved_9_1() {
    use turborel::params::Params;
    use turborel::value::{DataType, Spelled, Value};
    let db = decimals();
    let a = db.session("A");
    a.run("INSERT INTO T (D) VALUES (CAST('12.3456789' AS DECFLOAT))").unwrap();
    let cases = [
        (DataType::DecFloat { precision: 4 }, Ok("12.35".to_owned())),
        (DataType::DecFloat { precision: 34 }, Ok("12.3456789".to_owned())),
        (DataType::Exact { precision: 5, scale: 2, spelled: Spelled::Numeric }, Ok("12.34".to_owned())),
        (DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Numeric }, Err(Code::Data)),
    ];
    for (target, expected) in cases {
        let mut p = Params::new().declare("OUT", target, Value::Null);
        let got = a.execute("SELECT D INTO OUT FROM T", &mut p).map(|_| p.value("OUT").to_string()).map_err(|e| e.code);
        assert_eq!(got, expected, "into {target}");
    }
}

/// Verifies: [DESIGN] RULES.md D34: a comparison of a DECFLOAT column with
/// a DECFLOAT or an exact value, equal or of a range, probes an index on
/// it, negative values ordered as they compare. An approximate value
/// compares by its exact binary value, so 0.1E0 equals no tenth.
#[test]
fn a_decfloat_index_is_probed_d34() {
    let db = decimals();
    let a = db.session("A");
    let tenths = |k: i32| format!("CAST('{k}E-1' AS DECFLOAT)");
    let values: Vec<String> = (-1000..1000).map(|k| format!("({}, NULL, {k})", tenths(k))).collect();
    a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    a.commit().unwrap();
    a.run("CREATE INDEX TD ON T (D)").unwrap();
    let cases = [
        ("SELECT K FROM T WHERE D > -1.6 AND D < -1", vec!["-11", "-12", "-13", "-14", "-15"]),
        ("SELECT K FROM T WHERE D = CAST('-1.5' AS DECFLOAT)", vec!["-15"]),
        ("SELECT K FROM T WHERE D = -1.50", vec!["-15"]),
        ("SELECT K FROM T WHERE D >= 99.8", vec!["998", "999"]),
        ("SELECT K FROM T WHERE D > CAST('-1.6' AS DECFLOAT) AND D < CAST('-1' AS DECFLOAT)", vec!["-11", "-12", "-13", "-14", "-15"]),
    ];
    for (query, expected) in cases {
        assert_eq!(sorted(&a, query), expected, "{query}");
        let plan = a.explain(query, &turborel::params::Params::new()).unwrap();
        assert!(plan.contains("using TD"), "{query}: {plan}");
    }
    assert_eq!(sorted(&a, "SELECT K FROM T WHERE D = -1.5E0 OR D = 0.1E0"), ["-15"], "an approximate value, by its binary value");
}
