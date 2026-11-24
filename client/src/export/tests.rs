use std::collections::HashMap;
use std::sync::Arc;

use turborel::value::types::RowType;
use turborel::value::{CharUnits, Decimal};
use turborel_protocol::MAX_BODY;

use super::delimited::hexits;
use super::run::EXPORT_ROWS;
use crate::lobs::LOB_INLINE_OCTETS;
use super::*;

/// What a server holds by locator, read a chunk of `chunk` octets at a time.
struct Held {
    values: HashMap<u32, Vec<u8>>,
    chunk: usize,
    reads: usize,
}

impl Lobs for Held {
    fn chunks(&mut self, lob: &Lob, each: &mut dyn FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        if let Some(bytes) = lob.inline() {
            return each(bytes);
        }
        let bytes = self.values[&lob.locator().unwrap()].clone();
        for piece in bytes.chunks(self.chunk) {
            self.reads = self.reads.saturating_add(1);
            each(piece)?;
        }
        Ok(())
    }
}

/// A server holding `values`, each by its locator, from 1.
fn held_by(values: &[&[u8]], chunk: usize) -> Held {
    let values = (1..).zip(values).map(|(k, v)| (k, v.to_vec())).collect();
    Held { values, chunk, reads: 0 }
}

/// The value locator `k` names, of `kind`, `bytes` long.
fn by_locator(kind: LobKind, k: u32, bytes: &[u8]) -> Value {
    let chars = match kind {
        LobKind::Character => std::str::from_utf8(bytes).unwrap().chars().count(),
        LobKind::Binary => bytes.len(),
    };
    Value::Lob(Lob::of_locator(kind, k, (u64::try_from(bytes.len()).unwrap(), u64::try_from(chars).unwrap())))
}

/// `rows` written by `layout`, their large objects read from `lobs`.
fn written(layout: &mut dyn Layout, rows: &[Row], lobs: &mut dyn Lobs) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let columns = [Column { name: Some("K".to_owned()), data_type: DataType::Boolean, not_null: false, declared: None }];
    layout.columns(&mut out, &columns)?;
    layout.rows(&mut out, rows, lobs)?;
    layout.end(&mut out)?;
    Ok(out)
}

/// Verifies: [DESIGN] [RFC 4180 §2] RULES.md D195, D975: a CSV field is
/// quoted where it holds a comma, a quote, a line break or spaces at
/// either end, its quotes doubled; an empty string quoted, a null empty;
/// a binary string in upper case hexits, two to each; text refuses a
/// field it cannot hold.
#[test]
fn delimited_fields_read_back_as_written() {
    let csv = Delimited::csv();
    let field = |how: &Delimited, s: &str| {
        let mut buf = Vec::new();
        how.text_field(&mut buf, s).map(|()| String::from_utf8(buf).unwrap())
    };
    assert_eq!(field(&csv, "plain").unwrap(), "plain");
    assert_eq!(field(&csv, "a,b").unwrap(), "\"a,b\"");
    assert_eq!(field(&csv, "say \"hi\"").unwrap(), "\"say \"\"hi\"\"\"");
    assert_eq!(field(&csv, " pad").unwrap(), "\" pad\"");
    assert_eq!(field(&csv, "two\nlines").unwrap(), "\"two\nlines\"");
    assert_eq!(field(&csv, "").unwrap(), "\"\"");
    let text = Delimited { null: "N".to_owned(), ..Delimited::text() };
    assert_eq!(field(&text, "a b").unwrap(), "a b");
    assert!(field(&text, "a\tb").is_err());
    assert!(field(&text, "N").is_err());
    assert_eq!(hexits(&[0x00, 0xAB, 0x7F]), b"00AB7F");
    let row = vec![Value::Null, Value::Char("".into()), Value::Binary(turborel::value::Binary::of(&[1, 255]))];
    let out = written(&mut Delimited::csv(), &[row], &mut held_by(&[], 1)).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "K\r\n,\"\",01FF\r\n");
}

/// Verifies: [DESIGN] RULES.md D975: a large object sent by its locator
/// is written as its value, a chunk at a time: a character one's text in
/// quotes, its quotes doubled, a chunk that ends within a character
/// carried to the next; a binary one in hexits; one within a row read
/// whole; text refuses a chunk that holds the delimiter.
#[test]
fn located_large_objects_are_written_as_their_values() {
    let text = "a\"é,".repeat(5);
    let mut lobs = held_by(&[text.as_bytes(), &[0xDE, 0xAD, 0xBE, 0xEF], b"inner"], 3);
    let clob = by_locator(LobKind::Character, 1, text.as_bytes());
    let blob = by_locator(LobKind::Binary, 2, &[0xDE, 0xAD, 0xBE, 0xEF]);
    let nested = Value::Row(Arc::from(vec![by_locator(LobKind::Character, 3, b"inner")]));
    let out = written(&mut Delimited { header: false, ..Delimited::csv() }, &[vec![clob.clone(), blob, nested]], &mut lobs).unwrap();
    let expected = format!("\"{}\",DEADBEEF,ROW('inner')\r\n", text.replace('"', "\"\""));
    assert_eq!(String::from_utf8(out).unwrap(), expected);
    assert!(lobs.reads > 3, "read a chunk at a time");
    let refused = written(&mut Delimited { delimiter: ',', ..Delimited::text() }, &[vec![clob]], &mut held_by(&[text.as_bytes()], 3));
    assert!(refused.unwrap_err().to_string().contains("text cannot hold it"));
}

/// Verifies: [DESIGN] RULES.md D975: native rows write each large object
/// sent by its locator numbered in the file, its octets in Lob Data
/// replies after its Rows reply, however deep it is; `read_native` gives
/// each its value, held whole.
#[test]
fn native_rows_carry_large_objects_after_their_rows() {
    let long: Vec<u8> = (0..1000_u32).map(|i| u8::try_from(i % 251).unwrap()).collect();
    let mut lobs = held_by(&[&long, b"deep text"], 300);
    let row = vec![
        Value::Exact(Decimal::new(1, 0)),
        by_locator(LobKind::Binary, 1, &long),
        Value::Array(Arc::from(vec![by_locator(LobKind::Character, 2, b"deep text")])),
        Value::Lob(Lob::of_text("short")),
    ];
    let out = written(&mut Native, &[row.clone(), vec![Value::Null; 4]], &mut lobs).unwrap();
    let (_, back) = read_native(&mut out.as_slice()).unwrap();
    assert_eq!(back.len(), 2);
    let Value::Lob(blob) = &back[0][1] else { panic!("{:?}", back[0]) };
    assert_eq!(blob.inline(), Some(long.as_slice()));
    assert_eq!(back[0][2].to_string(), "ARRAY['deep text']");
    assert_eq!(back[0][3].to_string(), "short");
    let cut = &out[..out.len() - 40];
    assert!(read_native(&mut &cut[..]).is_err(), "cut short");
}

/// Verifies: [DESIGN] RULES.md D975: an export fetches as many rows at a
/// time as a reply's longest body holds of large objects sent whole at
/// their longest; a column of a large object type, or of a composite type
/// of one, may hold one, any other none.
#[test]
fn an_export_batches_only_what_may_hold_large_objects() {
    let inline = u32::try_from(LOB_INLINE_OCTETS).unwrap();
    assert_eq!(MAX_BODY.checked_div(inline), Some(EXPORT_ROWS));
    let clob = DataType::Lob { kind: LobKind::Character, length: 10, units: CharUnits::Characters };
    assert!(may_hold_lob(&clob));
    assert!(may_hold_lob(&DataType::array(clob.clone(), 3)));
    let fields = vec![turborel::value::types::Field { name: "F".to_owned(), data_type: clob, unnamed: false }];
    assert!(may_hold_lob(&DataType::Row(Arc::new(RowType { fields }))));
    assert!(!may_hold_lob(&DataType::varying(10)));
    assert!(!may_hold_lob(&DataType::array(DataType::Boolean, 3)));
}
