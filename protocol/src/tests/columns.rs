//! Unit tests of CopyColumns (RULES.md D1148): rows by column read back as
//! written, byte for byte, and every malformed column refused before
//! anything takes it.

use turborel::storage::lanes::{Lane, Lanes};

use super::{FAILED, kind_of, request_round_trip};
use crate::capability::{self, Capabilities};
use crate::*;

/// The kind of CopyColumns.
const COPY_COLUMNS: u8 = b'O';

/// Two rows of two columns: 1 and "a,b"; null and the empty string.
fn two_rows() -> Lanes {
    let numbers = Lane { nulls: vec![0b10], ends: vec![1, 1], bytes: b"1".to_vec() };
    let names = Lane { nulls: vec![0], ends: vec![3, 3], bytes: b"a,b".to_vec() };
    Lanes { rows: 2, columns: vec![numbers, names] }
}

/// A column as written: its nulls, its ends' count, its ends, its text.
type Column<'a> = (&'a [u8], u32, &'a [u32], &'a [u8]);

/// The body of a CopyColumns of `rows` rows, each column its nulls, its
/// ends' count, its ends and its text, as given.
fn body(rows: u32, columns: &[Column<'_>]) -> Vec<u8> {
    let mut b = rows.to_le_bytes().to_vec();
    b.extend(u32::try_from(columns.len()).unwrap().to_le_bytes());
    for (nulls, count, ends, text) in columns {
        b.extend(u32::try_from(nulls.len()).unwrap().to_le_bytes());
        b.extend(*nulls);
        b.extend(count.to_le_bytes());
        ends.iter().for_each(|e| b.extend(e.to_le_bytes()));
        b.extend(u32::try_from(text.len()).unwrap().to_le_bytes());
        b.extend(*text);
    }
    b
}

/// Verifies: [DESIGN] RULES.md D1148: CopyColumns reads back as written,
/// rows by column, each column its null bits, its ends and its text after
/// their lengths, and is of the capability `columns`, the last of version
/// 1's; a copy of no rows, and of rows of one null column, read back too.
#[test]
fn copy_columns_reads_back_as_written() {
    let lanes = two_rows();
    let mut bytes = Vec::new();
    Request::CopyColumns(lanes.clone()).encode(&mut bytes);
    let columns = [(&[0b10_u8][..], 2, &[1_u32, 1][..], &b"1"[..]), (&[0], 2, &[3, 3], b"a,b")];
    assert_eq!(bytes[0], COPY_COLUMNS);
    assert_eq!(bytes[HEADER..], body(2, &columns)[..]);
    let Request::CopyColumns(back) = Request::decode(COPY_COLUMNS, &bytes[HEADER..]).unwrap() else { panic!("not CopyColumns") };
    assert_eq!(back, lanes);
    request_round_trip(&Request::CopyColumns(Lanes::default()));
    request_round_trip(&Request::CopyColumns(Lanes {
        rows: 9,
        columns: vec![Lane { nulls: vec![0xFF, 1], ends: vec![0; 9], bytes: vec![] }],
    }));
    assert_eq!(capability::ALL.last(), Some(&capability::COLUMNS));
    assert!(Capabilities::all().has(capability::COLUMNS));
}

/// Verifies: [DESIGN] RULES.md D1148, D42: a CopyColumns is refused as a
/// connection failure (SQLCODE -919, SQLSTATE 08006) where a column has
/// too few or too many null bytes, a null bit past its last row, too few
/// or too many ends, or fewer than its count, ends that fall,
/// a null's end not where the cell before it ends, or a last end short
/// of or past its text; and where rows have no column. Boundaries: a
/// column of exactly a bit and an end for each row, and its last end its
/// text's length, is taken.
#[test]
fn a_malformed_copy_columns_is_refused() {
    let refused = |b: Vec<u8>| Request::decode(COPY_COLUMNS, &b).err().map(|e| kind_of(&e));
    assert!(Request::decode(COPY_COLUMNS, &body(2, &[(&[0b10], 2, &[2, 2], b"ab")])).is_ok(), "exactly");
    assert!(Request::decode(COPY_COLUMNS, &body(8, &[(&[0x80], 8, &[0; 8], b"")])).is_ok(), "the eighth row's bit");
    let cases: [(&str, Vec<u8>); 11] = [
        ("too few null bytes", body(9, &[(&[0], 9, &[0; 9], b"")])),
        ("too many null bytes", body(2, &[(&[0, 0], 2, &[1, 2], b"ab")])),
        ("a bit past the last row", body(2, &[(&[0b100], 2, &[1, 2], b"ab")])),
        ("too few ends", body(2, &[(&[0], 1, &[2], b"ab")])),
        ("too many ends", body(2, &[(&[0], 3, &[1, 2, 2], b"ab")])),
        ("ends cut short", body(64, &[(&[0; 8], 64, &[1, 2], b"ab")])),
        ("ends that fall", body(2, &[(&[0], 2, &[2, 1], b"ab")])),
        ("a null's end moved", body(2, &[(&[0b10], 2, &[1, 2], b"ab")])),
        ("a last end short", body(2, &[(&[0], 2, &[1, 1], b"ab")])),
        ("a last end past its text", body(2, &[(&[0], 2, &[1, 3], b"ab")])),
        ("rows of no column", body(3, &[])),
    ];
    for (why, b) in cases {
        assert_eq!(refused(b), Some(FAILED), "{why}");
    }
}
