//! Joins keyed by whole numbers past the 53 bits an approximate number
//! keeps (RULES.md D1288): keys one after another near the end of what 64
//! bits hold, whose nearest approximate numbers are few, join by their
//! exact values [ISO/IEC 9075-2:2023 §8.2 GR2], an exact key with an
//! exact one of another scale and with an approximate one alike, and
//! decimal floating-point keys with their own.

use turborel::db::{Database, Session};
use turborel::params::Params;

use crate::common::rows;

/// The first key: a number of 19 digits, a multiple of 1,024, so it and
/// each 1,024th after it is an approximate number exactly.
const FIRST: i128 = 9_000_000_000_000_000_000;
/// The keys of L, one after another from `FIRST`.
const KEYS: i128 = 3000;
/// The small keys of P, each beside the wide key whose word it shares.
const STRAYS: u32 = 2000;

/// The whole number of 19 digits whose word, as a join hashes it, is the
/// small number `k`'s (RULES.md D1288): the bits of `k` as an approximate
/// number, crossed with the number a null is given as to a direct table.
fn stray_of(k: u32) -> i64 {
    i64::from_le_bytes((f64::from(k).to_bits() ^ 0x7FF8_0000_0000_0E11_u64).to_le_bytes())
}

/// What is added to a key to make it a number of 26 digits.
const PAST: i128 = 10_000_000_000_000_000_000_000_000;

/// The rows `(key, v)` of the keys `FIRST + step * j` below `FIRST + KEYS`,
/// each written as `written` writes its key.
fn values(step: i128, written: impl Fn(i128) -> String) -> String {
    let each = (0..KEYS).filter(|i| i.rem_euclid(step) == 0).map(|i| format!("({}, {i})", written(FIRST.wrapping_add(i))));
    each.collect::<Vec<_>>().join(", ")
}

/// L, every key; R, every third, of the same type; S, every fifth, with
/// two digits after the point; D, every seventh, decimal floating-point;
/// F, every 1,024th, approximate; WA and WB, numbers of 26 digits, past
/// what 64 bits hold, one after another and every third at another scale;
/// P, 2,000 small numbers and beside each the wide number whose word it
/// shares; and N and M, the numbers 0 to 9 as INTEGER and with a digit
/// after the point. L and R are frozen.
fn tables() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE L (K NUMERIC(19, 0), V INTEGER) USING columnstore
           CREATE TABLE R (K NUMERIC(19, 0), V INTEGER) USING columnstore
           CREATE TABLE S (K NUMERIC(21, 2), V INTEGER)
           CREATE TABLE D (K DECFLOAT(34), V INTEGER)
           CREATE TABLE F (K DOUBLE PRECISION, V INTEGER)
           CREATE TABLE WA (K NUMERIC(30, 0), V INTEGER)
           CREATE TABLE WB (K NUMERIC(32, 2), V INTEGER)
           CREATE TABLE P (K NUMERIC(19, 0), V INTEGER)
           CREATE TABLE N (K INTEGER, V INTEGER)
           CREATE TABLE M (K NUMERIC(5, 1), V INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(&format!("INSERT INTO L VALUES {}", values(1, |k| k.to_string()))).unwrap();
    a.run(&format!("INSERT INTO R VALUES {}, (NULL, -1)", values(3, |k| k.to_string()))).unwrap();
    a.run(&format!("INSERT INTO S VALUES {}", values(5, |k| format!("{k}.00")))).unwrap();
    a.run(&format!("INSERT INTO D VALUES {}", values(7, |k| format!("CAST({k} AS DECFLOAT(34))")))).unwrap();
    a.run(&format!("INSERT INTO F VALUES {}", values(1024, |k| format!("{k}E0")))).unwrap();
    a.run(&format!("INSERT INTO WA VALUES {}", values(1, |k| format!("{}", k.wrapping_add(PAST))))).unwrap();
    a.run(&format!("INSERT INTO WB VALUES {}, (NULL, -1)", values(3, |k| format!("{}.00", k.wrapping_add(PAST))))).unwrap();
    let pairs = (0..STRAYS).map(|k| format!("({k}, {k}), ({}, {})", stray_of(k), k.wrapping_add(STRAYS)));
    a.run(&format!("INSERT INTO P VALUES {}", pairs.collect::<Vec<_>>().join(", "))).unwrap();
    a.run("INSERT INTO N VALUES (0, 0), (1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (7, 7), (8, 8), (9, 9), (NULL, -1)").unwrap();
    a.run("INSERT INTO M VALUES (0.0, 0), (1.0, 1), (2.5, 2), (3.0, 3), (-0.0, 4), (9.0, 9), (NULL, -1)").unwrap();
    a.commit().unwrap();
    db.maintain().unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 9075-2:2023 §8.2 GR2] [DESIGN] RULES.md D1288, a
/// join keyed by whole numbers of 19 digits one after another matches
/// each key with its equal alone: with keys of its own type, every third;
/// with exact keys of another scale, every fifth; decimal floating-point
/// keys with their own, every seventh; with approximate keys, the three
/// that are approximate numbers exactly, and not the 1,023 keys beside
/// each that share its nearest approximate number; and an approximate key
/// with an exact one of another scale; keys past 64 bits join by their
/// exact values at two scales; 1 and 1.0 meet, 2 and 2.5 do not, and a
/// null key matches none; and a wide key whose word is a small number's
/// joins its own match alone, beside 2,000 such pairs. Each join is a hash
/// join, and gives the rows the same condition gives as two inequalities.
#[test]
fn keys_past_53_bits_join_by_their_exact_values() {
    let (_db, a) = tables();
    let sum = |step: i128| (0..KEYS).filter(|i| i.rem_euclid(step) == 0).sum::<i128>();
    let count = |step: i128| (0..KEYS).filter(|i| i.rem_euclid(step) == 0).count();
    for (other, step) in [("R", 3_i128), ("S", 5), ("F", 1024)] {
        let equal = format!("SELECT COUNT(*), SUM(L.V), SUM(O.V) FROM L JOIN {other} O ON L.K = O.K");
        let plan = a.explain(&equal, &Params::new()).unwrap();
        assert!(plan.split("-- physical:").nth(1).unwrap_or_default().contains("HashJoin"), "{other}: {plan}");
        assert_eq!(rows(&a, &equal).unwrap(), [format!("{},{},{}", count(step), sum(step), sum(step))], "{other}");
        let between = format!("SELECT COUNT(*), SUM(L.V), SUM(O.V) FROM L JOIN {other} O ON L.K <= O.K AND L.K >= O.K");
        assert_eq!(rows(&a, &between).unwrap(), rows(&a, &equal).unwrap(), "{other}, as two inequalities");
    }
    assert_eq!(rows(&a, "SELECT L.V FROM L JOIN F ON L.K = F.K ORDER BY L.V").unwrap(), ["0", "1024", "2048"]);
    assert_eq!(rows(&a, "SELECT S.V FROM F JOIN S ON F.K = S.K").unwrap(), ["0"], "an approximate key and an exact one");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM L X JOIN L Y ON X.K = Y.K").unwrap(), [KEYS.to_string()], "each key its own alone");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM R X JOIN R Y ON X.K = Y.K").unwrap(), ["1000"], "a null matches none");
    let floating = "SELECT COUNT(*), SUM(X.V), SUM(Y.V) FROM D X JOIN D Y ON X.K = Y.K";
    assert_eq!(rows(&a, floating).unwrap(), [format!("{},{},{}", count(7), sum(7), sum(7))], "decimal floating-point keys");
    let wide = "SELECT COUNT(*), SUM(X.V), SUM(Y.V) FROM WA X JOIN WB Y ON X.K = Y.K";
    assert_eq!(rows(&a, wide).unwrap(), [format!("{},{},{}", count(3), sum(3), sum(3))], "keys past 64 bits, of two scales");
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM WA X JOIN WA Y ON X.K = Y.K").unwrap(), [KEYS.to_string()]);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM WA X JOIN L Y ON X.K = Y.K").unwrap(), ["0"], "no key of 26 digits is one of 19");
    let strays = "SELECT COUNT(*), SUM(CASE WHEN X.V = Y.V THEN 0 ELSE 1 END), MIN(X.K), MAX(X.K) FROM P X JOIN P Y ON X.K = Y.K";
    let (least, most) = (0..STRAYS).map(stray_of).fold((i64::MAX, 0_i64), |(l, m), s| (l.min(s).min(0), m.max(s)));
    assert_eq!(
        rows(&a, strays).unwrap(),
        [format!("{},0,{least},{most}", STRAYS.wrapping_mul(2))],
        "a stray pair shares a word, not a match"
    );
    assert_eq!(rows(&a, "SELECT N.V FROM N JOIN M ON N.K = M.K ORDER BY N.V, M.V").unwrap(), ["0", "0", "1", "3", "9"], "1 and 1.0 meet");
}
