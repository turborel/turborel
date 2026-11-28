//! A column of an exact numeric type of 19 to 38 digits whose values 64
//! bits hold, frozen by the column store in 8-byte fields (RULES.md
//! D1285) and read of its chunks as integers (RULES.md D1287): filters, sums, groupings, joins, orderings, arithmetic
//! and windows over it give what the same rows of the row store give, a
//! column some of whose values pass 64 bits too, and sums and overflow are
//! those of its declared type [ISO/IEC 9075-2:2023 §10.9 GR7g].

use turborel::db::{Database, Session};

use crate::common::{refused, rows};

/// The greatest number 64 bits hold, of 19 digits.
const MOST: i128 = 9_223_372_036_854_775_807;
/// The rows of each table, half of them a group.
const ROWS: i128 = 4096;
const GROUP: i128 = 2048;

/// The identifier of row `k`: one of 2,048 numbers at the end of what 64
/// bits hold, each once a group, none of every thirteenth row.
fn ident(k: i128) -> Option<i128> {
    (k % 13 != 5).then(|| MOST.wrapping_sub(k.wrapping_mul(7919).rem_euclid(GROUP).wrapping_mul(1_000_003)))
}

/// The amount of row `k`, in hundredths: past what 64 bits hold of a
/// hundred rows of the second group, above and below, and of fifty rows
/// after it, and near zero of the rest.
fn amount_units(k: i128) -> i128 {
    let whole = match k {
        2500..2550 | 5000..5050 => 10_i128.pow(30).wrapping_add(k),
        3500..3550 => 10_i128.pow(30).wrapping_add(k).wrapping_neg(),
        _ => k.wrapping_mul(1_000_003).wrapping_sub(1_000_000_000),
    };
    let cents = k % 100;
    whole.wrapping_mul(100).wrapping_add(if whole < 0 { cents.wrapping_neg() } else { cents })
}

/// `units` hundredths as an exact number of two digits after the point
/// is written.
fn hundredths(units: i128) -> String {
    let (sign, most) = (if units < 0 { "-" } else { "" }, units.unsigned_abs());
    format!("{sign}{}.{:02}", most.checked_div(100).unwrap_or_default(), most % 100)
}

/// The amount of row `k`, with two digits after the point.
fn amount(k: i128) -> String {
    hundredths(amount_units(k))
}

/// Rows `first` to `first + GROUP`: I in order, U an identifier, D one of
/// four numbers far apart, C one number, M an amount, W a number of 38
/// digits in two rows, R a hundred rows alike.
fn values(first: i128) -> String {
    let far = ["9223372036854775807", "-9223372036854775808", "5", "1000000000000000000"];
    let each = (first..first.wrapping_add(GROUP)).map(|k| {
        let null_or = |every: i128, v: String| if k.rem_euclid(every) == 0 { "NULL".to_owned() } else { v };
        let u = ident(k).map_or_else(|| "NULL".to_owned(), |v| v.to_string());
        let d = null_or(5, far[usize::try_from(k.wrapping_mul(31) % 4).unwrap()].to_owned());
        let c = null_or(9, "9000000000000000001".to_owned());
        let w = if k == 3000 || k == 3001 { format!("6{}", "0".repeat(37)) } else { k.to_string() };
        format!("({k}, {u}, {d}, {c}, {}, {w}, {})", amount(k), k.checked_div(100).unwrap())
    });
    each.collect::<Vec<_>>().join(", ")
}

/// `PLAIN`, the rows of the row store, and `FROZEN`, the same of the column
/// store, frozen in two groups.
fn tables() -> (Database, Session) {
    let columns = "(I INTEGER, U NUMERIC(19, 0), D NUMERIC(19, 0), C NUMERIC(20, 0), M NUMERIC(38, 2), W NUMERIC(38, 0), R INTEGER)";
    let db = Database::new();
    db.schema(&format!(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE PLAIN {columns}
           CREATE TABLE FROZEN {columns} USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)"
    ))
    .unwrap();
    let a = db.session("A");
    for table in ["PLAIN", "FROZEN"] {
        for first in [0, GROUP] {
            a.run(&format!("INSERT INTO {table} VALUES {}", values(first))).unwrap();
        }
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Queries over a table `{t}`, of the identifiers, the dictionary, the
/// constant and the amounts.
const QUERIES: [&str; 30] = [
    "SELECT I FROM {t} WHERE U = 9223372036854775807 ORDER BY I",
    "SELECT COUNT(*) FROM {t} WHERE U < 9223372036854000000",
    "SELECT COUNT(*) FROM {t} WHERE U < 99999999999999999999999",
    "SELECT COUNT(*) FROM {t} WHERE U BETWEEN 9223372036000000000 AND 9223372036500000000.5",
    "SELECT COUNT(*) FROM {t} WHERE U IN (9223372036854775807, 9223372035854772807, 12)",
    "SELECT SUM(U), AVG(U), MIN(U), MAX(U), COUNT(U), COUNT(DISTINCT U) FROM {t}",
    "SELECT R, SUM(U), COUNT(*) FROM {t} GROUP BY R ORDER BY R",
    "SELECT U, COUNT(*), SUM(I) FROM {t} GROUP BY U ORDER BY U",
    "SELECT D, COUNT(*), SUM(U), SUM(D), AVG(D) FROM {t} GROUP BY D ORDER BY D",
    "SELECT COUNT(*), SUM(X.I) FROM {t} X JOIN {t} Y ON X.U = Y.U WHERE Y.I < 500",
    "SELECT COUNT(*) FROM {t} X JOIN PLAIN Y ON X.D = Y.D AND X.I = Y.I",
    "SELECT I, U FROM {t} ORDER BY U DESC, I FETCH FIRST 20 ROWS ONLY",
    "SELECT I, U FROM {t} WHERE U IS NOT NULL ORDER BY U, I FETCH FIRST 20 ROWS ONLY",
    "SELECT I, U + U, U * 100, U / 3, -U, U - D, CAST(U AS VARCHAR(40)), CAST(U AS DOUBLE PRECISION), C FROM {t} WHERE I < 300 ORDER BY I",
    "SELECT I, SUM(U) OVER (ORDER BY I ROWS BETWEEN 5 PRECEDING AND CURRENT ROW), SUM(D) OVER (), MAX(U) OVER (PARTITION BY R) FROM {t} \
     ORDER BY I",
    "SELECT DISTINCT D FROM {t} ORDER BY D",
    "SELECT COUNT(*) FROM {t} WHERE D IN (5, -9223372036854775808)",
    "SELECT COUNT(*) FROM {t} WHERE C = 9000000000000000001",
    "SELECT I FROM {t} WHERE C IS NULL AND I < 100 ORDER BY I",
    "SELECT I FROM {t} WHERE M = 1000000000000000000000000002525.25",
    "SELECT I FROM {t} WHERE M = 47003141.47",
    "SELECT COUNT(*), SUM(M), MIN(M), MAX(M), AVG(M) FROM {t}",
    "SELECT COUNT(*) FROM {t} WHERE M > 1000.00",
    "SELECT COUNT(*) FROM {t} WHERE M < -5",
    "SELECT R, SUM(M), COUNT(DISTINCT M) FROM {t} GROUP BY R ORDER BY R",
    "SELECT M, COUNT(*) FROM {t} GROUP BY M ORDER BY M DESC FETCH FIRST 80 ROWS ONLY",
    "SELECT I, M FROM {t} ORDER BY M, I FETCH FIRST 80 ROWS ONLY",
    "SELECT COUNT(*), SUM(X.M) FROM {t} X JOIN {t} Y ON X.M = Y.M",
    "SELECT SUM(W), MAX(W) FROM {t} WHERE I < 2048",
    "SELECT SUM(W) FROM {t} WHERE I NOT IN (3000, 3001)",
];

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7g] [DESIGN] RULES.md D1285,
/// D1287, a wide exact column of the column store written and read as
/// integers: a group's column whose values 64 bits hold is frozen as a
/// narrow column is, a frame of reference, a dictionary or a constant, and
/// one whose second group holds rows past 64 bits keeps fixed fields of
/// 16 bytes there, read as integers of the chunks that hold none; the
/// columns' declared precisions are as written; comparisons with constants of 64 bits
/// and past them, sums and averages, groupings, joins, orderings,
/// arithmetic whose results pass 64 bits, casts and windows give what the
/// row store's same rows give; the sum of the identifiers is the exact
/// number of 23 digits; and a sum past 38 digits raises SQLSTATE 22003 on
/// both stores.
#[test]
fn a_wide_column_of_narrow_values_reads_as_the_row_store_gives_it() {
    let (_db, a) = tables();
    let shown = "SELECT COLUMN_NAME, ENCODING FROM INFORMATION_SCHEMA.COLUMN_STORAGE WHERE TABLE_NAME = 'FROZEN' \
                 AND COLUMN_NAME IN ('U', 'D', 'C', 'M') GROUP BY COLUMN_NAME, ENCODING ORDER BY COLUMN_NAME";
    assert_eq!(rows(&a, shown).unwrap(), ["C,CONSTANT", "D,DICTIONARY", "M,FIXED", "M,FRAME", "U,FRAME"], "narrowed where 64 bits hold");
    let declared = "SELECT COLUMN_NAME, NUMERIC_PRECISION, NUMERIC_SCALE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'FROZEN' \
                    AND COLUMN_NAME IN ('U', 'C', 'M') ORDER BY COLUMN_NAME";
    assert_eq!(rows(&a, declared).unwrap(), ["C,20,0", "M,38,2", "U,19,0"], "the types are as declared");
    for query in QUERIES {
        let of = |table: &str| rows(&a, &query.replace("{t}", table)).unwrap();
        let (frozen, plain) = (of("FROZEN"), of("PLAIN"));
        assert!(!frozen.is_empty(), "{query}");
        assert_eq!(frozen, plain, "{query}");
    }
    let total: i128 = (0..ROWS).filter_map(ident).sum();
    assert!(total > MOST * 3000, "the sum passes 64 bits");
    assert_eq!(rows(&a, "SELECT SUM(U) FROM FROZEN").unwrap(), [total.to_string()]);
    assert_eq!(rows(&a, "SELECT I FROM FROZEN WHERE M = 1000000000000000000000000002525.25").unwrap(), ["2525"]);
    let past = |table: &str| refused(&a, &format!("SELECT SUM(W) FROM {table}"));
    assert_eq!(past("FROZEN"), past("PLAIN"));
    assert_eq!(past("FROZEN").1, "22003", "a sum past 38 digits");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.9 GR7g] [DESIGN] RULES.md D1286, a
/// wide exact column gathered from records as integers while 64 bits hold
/// its values: over the row store's rows, and over the column store's
/// frozen rows with rows not yet frozen after them, fifty of which pass 64
/// bits, every query gives on both stores the same rows; the sum of the
/// amounts, of rows within 64 bits and past them, is the exact number
/// computed here on both; and a sum past 38 digits raises SQLSTATE 22003.
#[test]
fn a_wide_column_gathered_from_records_keeps_its_values() {
    let (_db, a) = tables();
    for table in ["PLAIN", "FROZEN"] {
        a.run(&format!("INSERT INTO {table} VALUES {}", values(ROWS))).unwrap();
    }
    a.commit().unwrap();
    for query in QUERIES {
        let of = |table: &str| rows(&a, &query.replace("{t}", table)).unwrap();
        let (frozen, plain) = (of("FROZEN"), of("PLAIN"));
        assert!(!frozen.is_empty(), "{query}");
        assert_eq!(frozen, plain, "{query}");
    }
    let total: i128 = (0..ROWS + GROUP).map(amount_units).sum();
    let within: i128 = (0..GROUP).map(amount_units).sum();
    for table in ["PLAIN", "FROZEN"] {
        assert_eq!(rows(&a, &format!("SELECT SUM(M) FROM {table}")).unwrap(), [hundredths(total)], "{table}");
        assert_eq!(rows(&a, &format!("SELECT SUM(M) FROM {table} WHERE I < 2048")).unwrap(), [hundredths(within)], "{table}");
        let past = format!("SELECT COUNT(*) FROM {table} WHERE M > 100000000000000000000000000000 OR M < -100000000000000000000000000000");
        assert_eq!(rows(&a, &past).unwrap(), ["150"], "{table}");
        assert_eq!(refused(&a, &format!("SELECT SUM(W) FROM {table}")).1, "22003", "{table}");
    }
}
