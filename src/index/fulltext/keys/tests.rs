//! Unit tests of the full-text index's entries: each key and value reads
//! back as written, and bytes cut short or left over are refused.

use super::*;

/// A document of row `row`, its hash every byte `fill`.
fn doc(row: RowId, fill: u8) -> Doc {
    Doc { row, hash: [fill; HASH_BYTES] }
}

/// That `r` is the error of a malformed entry, SQLCODE -916.
fn refused<T: std::fmt::Debug>(r: Result<T>) {
    assert_eq!(r.unwrap_err().code, Code::Corrupt);
}

/// Verifies: [DESIGN] RULES.md D86, full-text entries: a document key, a
/// document number's value, a hashed key, a block key and a count read
/// back as written, at the extremes of their numbers.
#[test]
fn keys_and_values_read_back() {
    for (row, length) in [(0, 0), (7, 300), (u64::MAX, u32::MAX)] {
        let d = doc(row, 0xA5);
        assert_eq!(document_of(&document_key(d)).unwrap(), d);
        assert_eq!(number_doc(&number_value(d, length)).unwrap(), (d, length));
        assert_eq!(hashed_row(&hashed_key(d)).unwrap(), row);
        assert_eq!(block_docno(&block_key(b"term", row), b"term").unwrap(), row);
        assert_eq!(count_of(&count_value(row)).unwrap(), row);
    }
    let value = DocumentValue { count: 3, docno: u64::MAX, length: 129 };
    assert_eq!(DocumentValue::decode(&value.encode()).unwrap(), value);
    let stats = TermStatistics { documents: 9, most: 4 };
    assert_eq!(TermStatistics::decode(&stats.encode()).unwrap(), stats);
    let counts = Counts { totals: Totals { documents: 2, length: 50 }, next: 3 };
    assert_eq!(Counts::decode(&counts.encode()).unwrap(), counts);
}

/// Verifies: [DESIGN] RULES.md D86, full-text entries, R-ERR-2: every
/// entry cut at any byte, or followed by one, is refused with SQLCODE
/// -916; so are a document key of another tag, a varint longer than a
/// u64's, and a length or count past 32 bits.
#[test]
fn malformed_entries_are_refused() {
    let d = doc(5, 1);
    let value = DocumentValue { count: 1, docno: 1 << 40, length: 1 << 20 }.encode();
    let stats = TermStatistics { documents: 1, most: 1 }.encode();
    let counts = Counts::default().encode();
    let numbered = number_value(d, 1 << 20);
    for whole in [document_key(d), value.clone(), stats.clone(), counts.clone(), numbered.clone(), count_value(1)] {
        for cut in 0..whole.len() {
            let part = &whole[..cut];
            refused(DocumentValue::decode(part).and(TermStatistics::decode(part)).and(Counts::decode(part)).and(count_of(part)));
        }
    }
    for cut in 0..numbered.len() {
        refused(number_doc(&numbered[..cut]));
    }
    let key = document_key(d);
    for cut in 0..key.len() {
        refused(document_of(&key[..cut]));
    }
    refused(document_of(&[key, vec![0]].concat()));
    for long in [[value.clone(), vec![0]].concat(), [stats, vec![0]].concat(), [counts, vec![0]].concat()] {
        refused(DocumentValue::decode(&long));
        refused(TermStatistics::decode(&long));
        refused(Counts::decode(&long));
    }
    refused(number_doc(&[numbered, vec![0]].concat()));
    refused(document_of(&hashed_key(d)));
    refused(hashed_row(&[HASHED]));
    refused(hashed_row(&hashed_key(d)[..DOCUMENT_KEY - 1]));
    refused(block_docno(b"P", b"term"));
    refused(block_docno(&block_key(b"term", 1)[..9], b"term"));
    refused(take_varint(&[0x80; VARINT_MOST]));
    refused(take_varint(&[]));
    let mut wide = Vec::new();
    put_varint(&mut wide, 1);
    put_varint(&mut wide, 1);
    put_varint(&mut wide, u64::from(u32::MAX) + 1);
    refused(DocumentValue::decode(&wide));
    let mut wide_count = Vec::new();
    put_varint(&mut wide_count, u64::from(u32::MAX) + 1);
    put_varint(&mut wide_count, 1);
    put_varint(&mut wide_count, 1);
    refused(DocumentValue::decode(&wide_count));
    let mut wide_length = doc(1, 1).row.to_be_bytes().to_vec();
    wide_length.extend_from_slice(&[1; HASH_BYTES]);
    put_varint(&mut wide_length, u64::from(u32::MAX) + 1);
    refused(number_doc(&wide_length));
}
