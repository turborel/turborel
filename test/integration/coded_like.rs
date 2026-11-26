//! LIKE and NOT LIKE a constant pattern of a column store's texts coded by
//! a table of symbols, judged of their codes with the column given late
//! (RULES.md D1122), against the same rows of a row store.

use turborel::db::{Database, Session};
use turborel::threads::JobPolicy;

use crate::common::rows;

/// The words the comments are made of.
const WORDS: [&str; 16] = [
    "special",
    "requests",
    "furiously",
    "pending",
    "deposits",
    "the",
    "carefully",
    "ironic",
    "accounts",
    "wake",
    "quickly",
    "slyly",
    "10%",
    "above",
    "é€",
    "packages",
];

/// A comment of row `k`: four to nine words of a sequence fixed by `k`
/// (xorshift), its last word ending in its number so most comments differ.
fn comment(k: u64) -> String {
    let mut x = k.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        x ^= x << 13_u32;
        x ^= x >> 7_u32;
        x ^= x << 17_u32;
        usize::try_from(x % 16).unwrap_or(0)
    };
    let count = 4_usize.saturating_add(next() % 6);
    let words: Vec<&str> = (0..count).map(|_| WORDS[next()]).collect();
    format!("{} {k}", words.join(" "))
}

/// A database of C's column store table G and row store table R alike:
/// a key K, a comment C, null in every seventh row, and a text D of three
/// values; G's 4,096 rows frozen, and 100 more in its tree.
fn stored() -> (Database, Session) {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION C \
           CREATE TABLE G (K INTEGER, C VARCHAR(120), D VARCHAR(10)) USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048) \
           CREATE TABLE R (K INTEGER, C VARCHAR(120), D VARCHAR(10))",
    )
    .unwrap();
    let c = db.session("C");
    let insert = |c: &Session, range: std::ops::Range<u64>| {
        let values: Vec<String> = range
            .map(|k| {
                let text = if k % 7 == 0 { "NULL".to_owned() } else { format!("'{}'", comment(k)) };
                format!("({k}, {text}, 'd{}')", k % 3)
            })
            .collect();
        for table in ["G", "R"] {
            c.run(&format!("INSERT INTO {table} VALUES {}", values.join(", "))).unwrap();
        }
        c.commit().unwrap();
    };
    insert(&c, 0..2048);
    insert(&c, 2048..4096);
    db.maintain().unwrap();
    insert(&c, 4096..4196);
    let owner = db.session("OWNER");
    let encoding = "SELECT COLUMN_NAME, ENCODING FROM INFORMATION_SCHEMA.COLUMN_STORAGE \
                    WHERE TABLE_NAME = 'G' AND COLUMN_NAME IN ('C', 'D') AND ENCODING <> 'ROW' ORDER BY COLUMN_NAME";
    assert_eq!(rows(&owner, encoding).unwrap(), ["C,FSST", "D,DICTIONARY"], "coded as the test means");
    (db, c)
}

/// The rows of `sql` of G, then of R in its place.
fn both(c: &Session, sql: &str) -> (Vec<String>, Vec<String>) {
    (rows(c, &sql.replace("{T}", "G")).unwrap(), rows(c, &sql.replace("{T}", "R")).unwrap())
}

/// Verifies: [X3.135-1986 §5.14 GR4, GR5]; [DESIGN] RULES.md D1122: of
/// texts coded by symbols, LIKE and NOT LIKE a constant pattern keep the
/// rows a row store's keep: of pieces at either end, between, repeated and
/// spanning symbols, of characters of several bytes, of `%` alone, of an
/// empty pattern, of ESCAPE, and of `_`, which is matched of decoded
/// values; a null neither LIKE nor NOT LIKE; of a dictionary's texts too;
/// the comment read above the filter or not; the tree's rows with them.
#[test]
fn a_like_of_coded_texts_keeps_the_rows_a_row_store_keeps() {
    let (_db, c) = stored();
    let patterns = [
        "%special%requests%",
        "the%",
        "%7",
        "%e%e%",
        "%€ 1%",
        "%",
        "%%",
        "",
        "%a_o%",
        "slyly_%",
        "%quickly %ironic%",
        "%y slyly w%",
        "%sly%sly%sly%",
    ];
    for p in patterns {
        for not in ["", "NOT "] {
            let sql = format!("SELECT COUNT(*), SUM(K) FROM C.{{T}} WHERE C {not}LIKE '{p}'");
            let (g, r) = both(&c, &sql);
            assert_eq!(g, r, "{sql}");
            let read = format!("SELECT COUNT(*), MAX(C) FROM C.{{T}} WHERE C {not}LIKE '{p}' AND K > 100");
            let (g, r) = both(&c, &read);
            assert_eq!(g, r, "{read}");
        }
    }
    for sql in [
        "SELECT COUNT(*) FROM C.{T} WHERE C LIKE '%10!%%' ESCAPE '!'",
        "SELECT COUNT(*) FROM C.{T} WHERE C NOT LIKE '%10!%%' ESCAPE '!'",
        "SELECT COUNT(*) FROM C.{T} WHERE D LIKE 'd1%'",
        "SELECT COUNT(*) FROM C.{T} WHERE D NOT LIKE '%2'",
        "SELECT COUNT(*) FROM C.{T} WHERE C LIKE '%wake%' AND D NOT LIKE 'd0'",
        "SELECT COUNT(*) FROM C.{T} WHERE NOT (C LIKE '%wake%') OR K < 5",
    ] {
        let (g, r) = both(&c, sql);
        assert_eq!(g, r, "{sql}");
    }
    let (some, none) = both(&c, "SELECT COUNT(*) FROM C.{T} WHERE C LIKE '%special%requests%'");
    assert_eq!(some, none);
    assert_ne!(some, ["0"], "the pattern matches some rows");
    let (nulls, _) = both(&c, "SELECT COUNT(*) FROM C.{T} WHERE C LIKE '%' OR C NOT LIKE '%'");
    assert_eq!(nulls, ["3596"], "a null neither");
}
