//! The VECTOR type: its declarations, formats, layouts and text forms, and
//! what it refuses of them.

use super::{error, got, table};

/// Verifies: [EXTENSION] vector_standard §3.1 R3.1, R3.2, R3.3, §3.2: each
/// format, `*` and the layouts are declared, in any case, and
/// VECTOR_DIMENSION_FORMAT and VECTOR_LAYOUT report what each column keeps;
/// INFORMATION_SCHEMA.VECTOR_COLUMNS what each fixes (§16).
#[test]
fn every_format_and_star_is_declared() {
    let (_db, a) = table(
        "F32 VECTOR(3), F16 VECTOR(3, float16), F64 VECTOR(3, FLOAT64), I8 VECTOR(3, INT8), B VECTOR(8, BINARY), \
         ANY_DIMS VECTOR(*, FLOAT32), FLEX VECTOR, STAR VECTOR(*, *), S VECTOR(10, FLOAT32, SPARSE)",
    );
    a.run(
        "INSERT INTO T VALUES ('[1, 2, 3]', '[1, 2, 3]', '[1, 2, 3]', '[1, 2, 3]', '[255]', '[1, 2, 3, 4]', '[0.5]', '[1]', \
         '[10, [2, 7], [1.5, 2.5]]')",
    )
    .unwrap();
    assert_eq!(
        got(
            &a,
            "SELECT VECTOR_DIMENSION_FORMAT(F32), VECTOR_DIMENSION_FORMAT(F16), VECTOR_DIMENSION_FORMAT(F64), \
             VECTOR_DIMENSION_FORMAT(I8), VECTOR_DIMENSION_FORMAT(B), VECTOR_DIMENSION_COUNT(ANY_DIMS), \
             VECTOR_DIMENSION_COUNT(FLEX), VECTOR_LAYOUT(S), VECTOR_NNZ(S), VECTOR_DIMENSION_COUNT(S) FROM T"
        ),
        ["FLOAT32,FLOAT16,FLOAT64,INT8,BINARY,4,1,SPARSE,2,10"]
    );
    assert_eq!(
        got(
            &a,
            "SELECT COLUMN_NAME, ORDINAL_POSITION, VECTOR_DIMENSIONS, VECTOR_FORMAT, VECTOR_LAYOUT \
             FROM INFORMATION_SCHEMA.VECTOR_COLUMNS WHERE COLUMN_NAME IN ('F16', 'ANY_DIMS', 'FLEX', 'S') ORDER BY ORDINAL_POSITION"
        ),
        ["F16,2,3,FLOAT16,DENSE", "ANY_DIMS,6,NULL,FLOAT32,DENSE", "FLEX,7,NULL,NULL,NULL", "S,9,10,FLOAT32,SPARSE"]
    );
}

/// Verifies: [EXTENSION] vector_standard §5.1, §5.2, R5.6: a vector's text,
/// FROM_VECTOR and CAST to VARCHAR give back what CAST and TO_VECTOR read,
/// dense and sparse, the shortest numbers that read back.
#[test]
fn text_and_cast_round_trip() {
    let (_db, a) = table("V VECTOR(3), S VECTOR(5, FLOAT32, SPARSE)");
    a.run("INSERT INTO T VALUES (CAST('[0.1, -2.5, 3E2]' AS VECTOR(3)), TO_VECTOR('[5, [1, 4], [0.25, -1]]', 5, FLOAT32, SPARSE))")
        .unwrap();
    assert_eq!(
        got(&a, "SELECT FROM_VECTOR(V), CAST(V AS VARCHAR(100)), FROM_VECTOR(S), FROM_VECTOR(S RETURNING VARCHAR FORMAT DENSE) FROM T"),
        ["[0.1,-2.5,300],[0.1,-2.5,300],[5,[1,4],[0.25,-1]],[0,0.25,0,0,-1]"]
    );
    assert_eq!(got(&a, "SELECT FROM_VECTOR(CAST(FROM_VECTOR(V) AS VECTOR(3))) FROM T"), ["[0.1,-2.5,300]"]);
    assert_eq!(got(&a, "SELECT CHARACTER_LENGTH(FROM_VECTOR(V RETURNING CLOB)) FROM T"), ["14"]);
    assert_eq!(
        got(&a, "SELECT FROM_VECTOR(VECTOR('{\"dims\":3,\"indices\":[0],\"values\":[2]}', 3, FLOAT32, SPARSE)) FROM T"),
        ["[3,[0],[2]]"]
    );
}

/// Verifies: [EXTENSION] vector_standard §5.2 R5.1, R5.3: a character
/// string is stored in a VECTOR column by INSERT and UPDATE, and CAST
/// changes a vector's format.
#[test]
fn text_is_assigned_and_formats_cast() {
    let (_db, a) = table("V VECTOR(2, FLOAT16)");
    a.run("INSERT INTO T VALUES ('[1.5, 2]')").unwrap();
    a.run("UPDATE T SET V = '[3, 4]'").unwrap();
    assert_eq!(got(&a, "SELECT FROM_VECTOR(V), VECTOR_DIMENSION_FORMAT(CAST(V AS VECTOR(2, INT8))) FROM T"), ["[3,4],INT8"]);
}

/// Verifies: [EXTENSION] vector_standard §3.1 R3.4, R3.8, §18 VEC-0001,
/// VEC-0002: a type of a dimension count out of range, an unknown format
/// or BINARY SPARSE is refused with SQLCODE -103.
#[test]
fn type_specifications_are_refused() {
    let (_db, a) = table("K INTEGER");
    for column in ["VECTOR(0)", "VECTOR(65536)", "VECTOR(12, BINARY)", "VECTOR(8, BINARY, SPARSE)", "VECTOR(3, FLOAT8)"] {
        let made = a.run(&format!("CREATE TABLE U (V {column})"));
        assert_eq!(made.map_err(|e| e.sqlcode()).err(), Some(-103), "{column}");
    }
}

/// Verifies: [EXTENSION] vector_standard §3.2 R3.9, §5.2 R5.2, §18
/// VEC-0003, VEC-0005, VEC-0006, VEC-0007: another dimension count is
/// 22000, a malformed literal, NaN among them, 22018, an element past
/// FLOAT32's range or an INT8 element out of range or not whole 22003, each SQLCODE -301.
#[test]
fn bad_values_are_refused_with_their_states() {
    let (_db, a) = table("V VECTOR(2), I VECTOR(2, INT8)");
    let state = |sql: &str| error(&a, sql);
    a.run("INSERT INTO T VALUES ('[1, 2]', '[1, 2]')").unwrap();
    assert_eq!(state("SELECT CAST('[1, 2, 3]' AS VECTOR(2)) FROM T"), (-301, "22000".to_owned()));
    assert_eq!(state("SELECT CAST('[1, x]' AS VECTOR(2)) FROM T"), (-301, "22018".to_owned()));
    // NaN is no number of a vector's text, as of JSON's.
    assert_eq!(state("SELECT CAST('[1, NaN]' AS VECTOR(2)) FROM T"), (-301, "22018".to_owned()));
    assert_eq!(state("SELECT CAST('[1, 1E39]' AS VECTOR(2)) FROM T"), (-301, "22003".to_owned()));
    assert_eq!(state("SELECT CAST('[1, 128]' AS VECTOR(2, INT8)) FROM T"), (-301, "22003".to_owned()));
    assert_eq!(state("SELECT CAST('[1, 1.5]' AS VECTOR(2, INT8)) FROM T"), (-301, "22003".to_owned()));
    let inserted = a.run("INSERT INTO T VALUES ('[1, 2, 3]', NULL)");
    assert_eq!(inserted.map_err(|e| e.sqlcode()).err(), Some(-301));
}
