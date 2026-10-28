//! Unit tests of the LZ4 encoding: its contract (RULES.md R-SAFE-3), what
//! it shrinks and what it keeps, damaged input refused (R-TEST-2), and a
//! table's rows through it end to end (RULES.md R-TEST-1).

use turborel::db::Database;
use turborel::disk::encoding::Encoding;
use turborel::error::Code;

use super::*;

fn encoded(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    Lz4.encode(0, input, &mut out).unwrap();
    out
}

fn decoded(input: &[u8]) -> Vec<u8> {
    let mut out = b"kept".to_vec();
    Lz4.decode(0, input, &mut out).unwrap();
    assert_eq!(&out[..4], b"kept", "decoding appends");
    out.split_off(4)
}

/// Bytes that do not repeat, from a linear congruential generator.
fn varied(n: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_u64;
    (0..n)
        .map(|_| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            state.to_le_bytes()[7]
        })
        .collect()
}

/// Verifies: [EXTENSION] RULES.md D30, D48, encoding contract: what is
/// encoded decodes to itself, empty, short, repeating or varied; repeating
/// input shrinks, and input that would not is kept as it is, behind one
/// byte; its id and name are fixed.
#[test]
fn what_is_encoded_decodes_to_itself() {
    let repeating = b"a row of text that repeats; ".repeat(300);
    for input in [&b""[..], b"x", b"abcabcabcabc", &repeating, &varied(10_000)] {
        assert_eq!(decoded(&encoded(input)), input, "{} bytes", input.len());
    }
    assert!(encoded(&repeating).len() * 10 < repeating.len(), "{} bytes", encoded(&repeating).len());
    let kept = encoded(&varied(1_000));
    assert_eq!((kept.len(), kept[0]), (1_001, STORED));
    assert_eq!((Lz4.id(), Lz4.name()), (LZ4_ID, "LZ4"));
}

/// Verifies: [EXTENSION] RULES.md D48, robustness: input empty, of no tag,
/// too short for its length, recording a length more than its block can
/// give or other than it gives, or a block damaged, is refused (SQLCODE
/// -916), and nothing is appended.
#[test]
fn damaged_input_is_refused() {
    let good = encoded(&b"text that repeats ".repeat(100));
    assert_eq!(good[0], COMPRESSED);
    let mut longer = good.clone();
    longer[1] = longer[1].wrapping_add(1);
    let mut huge = good.clone();
    huge[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut damaged = good.clone();
    damaged.truncate(good.len() - 3);
    for input in [&b""[..], &[9, 1, 2], &[COMPRESSED, 1, 0], &longer, &huge, &damaged] {
        let mut out = Vec::new();
        assert_eq!(Lz4.decode(0, input, &mut out).err().map(|e| e.code), Some(Code::Corrupt), "{input:?}");
        assert_eq!(out, b"");
    }
}

/// Verifies: [EXTENSION] RULES.md D41, D48: registered, LZ4 is named by a
/// table's option ENCODINGS, in any case, and the table's rows, short and
/// long, read back through it.
#[test]
fn a_tables_rows_pass_through_it() {
    let db = Database::new();
    db.register_encoding(Box::new(Lz4)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, S CHARACTER(4000)) WITH (ENCODINGS = 'lz4')").unwrap();
    let s = db.session("S");
    let long = "words that repeat ".repeat(200);
    s.run(&format!("INSERT INTO T VALUES (1, 'short'), (2, '{long}')")).unwrap();
    let mut cursor = s.open_cursor("SELECT K, S FROM T ORDER BY K").unwrap();
    let mut found = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        found.push(row[1].to_string().trim_end().to_owned());
    }
    assert_eq!(found, vec!["short".to_owned(), long.trim_end().to_owned()]);
}
