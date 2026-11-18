//! Unit tests of `spill/named.rs`: rows and values whose large objects and
//! XML values are kept apart, written and read back as they were; a
//! table's objects named by one source of the table; records not as
//! written refused (RULES.md R-TEST-1, R-TEST-2).

use std::collections::HashMap;

use super::*;
use crate::value::Decimal;
use crate::value::lob::fixtures::{octets_by, text, texts};
use crate::value::lob::{Chunk, Directory, Kept, LobKind};
use crate::value::types::TypeRegistry;

/// The objects of a table in memory, each its chunks, by id.
#[derive(Debug, Default)]
struct Objects(HashMap<u64, Vec<Vec<u8>>>);

/// An object of a table in memory, which makes others of it.
#[derive(Debug)]
struct Stored {
    table: u64,
    id: u64,
    objects: Arc<Objects>,
    directory: Directory,
}

impl Stored {
    /// Object `id` of `objects`, of table `table`.
    fn new(table: u64, id: u64, objects: &Arc<Objects>) -> Stored {
        let chunks = objects.0.get(&id).map(Vec::as_slice).unwrap_or_default();
        let sizes = chunks.iter().map(|c| Chunk { octets: u32::try_from(c.len()).unwrap(), chars: u32::try_from(c.len()).unwrap() });
        Stored { table, id, objects: Arc::clone(objects), directory: Directory::new(sizes.collect()) }
    }
}

impl LobSource for Stored {
    fn totals(&self) -> (u64, u64) {
        self.directory.totals()
    }
    fn directory(&self) -> Result<&Directory> {
        Ok(&self.directory)
    }
    fn chunk(&self, k: usize) -> Result<Arc<[u8]>> {
        let chunks = self.objects.0.get(&self.id).ok_or_else(|| unheld("a test's"))?;
        chunks.get(k).map(|c| Arc::from(c.as_slice())).ok_or_else(|| unheld("a test's"))
    }
    fn kept(&self) -> Option<Kept> {
        Some(Kept { table: self.table, id: self.id })
    }
    fn sibling(&self, id: u64, totals: (u64, u64)) -> Option<Arc<dyn LobSource>> {
        let made = Stored::new(self.table, id, &self.objects);
        if made.totals() != totals {
            return None;
        }
        let source: Arc<dyn LobSource> = Arc::new(made);
        Some(source)
    }
}

/// A table of tag 7 in memory, of objects 1 to 3, each two chunks of its
/// id's octets.
fn table() -> Arc<Objects> {
    Arc::new(Objects((1..=3_u8).map(|id| (u64::from(id), vec![vec![id; 300], vec![id.wrapping_add(10); 200]])).collect()))
}

/// The binary large object of table 7's object `id`.
fn object(objects: &Arc<Objects>, id: u64) -> Lob {
    Lob::of_source(LobKind::Binary, Arc::new(Stored::new(7, id, objects)))
}

/// The octets of `v`, a large object, or an XML value's stored form.
fn octets(v: &Value) -> Vec<u8> {
    let l = v.as_lob().unwrap();
    l.reader().read(0, l.octets()).unwrap()
}

/// `row` written by `lobs` and read back by `again`.
fn read_back(lobs: &SpilledLobs, again: &SpilledLobs, row: &Row) -> Row {
    let record = lobs.record(row);
    again.record_row(&record, &TypeRegistry::default()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D38, D79: a row of values held inline
/// alone is written as the value codec writes it, then one octet naming
/// nothing, and read back as it was; the spill holds no source.
#[test]
fn a_row_held_inline_names_nothing() {
    let lobs = SpilledLobs::new();
    let row = vec![Value::Exact(Decimal::new(5, 0)), Value::Lob(Lob::of_text("short")), Value::Null];
    let record = lobs.record(&row);
    let mut plain = Vec::new();
    put_row(&mut plain, &row);
    assert_eq!(record, [plain.as_slice(), &[0_u8]].concat());
    assert_eq!(format!("{:?}", read_back(&lobs, &lobs, &row)), format!("{row:?}"));
    assert_eq!(format!("{lobs:?}"), "SpilledLobs { tables: 0, others: 0 }");
}

/// Verifies: [DESIGN] RULES.md D38, D79, D86: a table's objects, whole and
/// sliced, alone, in an XML value's stored form and inside an array, a
/// row and a multiset, are read back with their octets after the values
/// written are dropped, each a source the one source of their table held
/// makes; a source no table keeps, whole or sliced, is held itself; a
/// value of several parts, a reference and a locator keep their parts.
#[test]
fn values_kept_apart_are_read_back_by_what_keeps_them() {
    let objects = table();
    let lobs = SpilledLobs::new();
    let temp = texts(&["ab€", "cd"]);
    let joined = Lob::of_text("x").concat(&object(&objects, 3));
    let row: Row = vec![
        Value::Lob(object(&objects, 1)),
        Value::Lob(object(&objects, 2).slice_octets(250, 100)),
        Value::Xml(XmlValue::stored(object(&objects, 3))),
        Value::Array(Arc::from(vec![Value::Lob(object(&objects, 2)), Value::Null])),
        Value::Row(Arc::from(vec![Value::Multiset(Arc::from(vec![Value::Lob(temp.clone())]))])),
        Value::Lob(temp.slice_octets(1, 4)),
        Value::Lob(joined),
        Value::Lob(Lob::of_reference(LobKind::Character, 9, (4, 4))),
        Value::Lob(Lob::of_locator(LobKind::Binary, 12, (8, 8))),
    ];
    let record = lobs.record(&row);
    let wanted: Vec<Vec<u8>> = row.iter().take(3).map(octets).collect();
    drop(row);
    let read = lobs.record_row(&record, &TypeRegistry::default()).unwrap();
    assert_eq!(read.iter().take(3).map(octets).collect::<Vec<_>>(), wanted);
    assert_eq!(octets(&read[1]), [vec![2_u8; 50], vec![12_u8; 50]].concat(), "a slice across chunks");
    let Value::Array(parts) = &read[3] else { panic!("{:?}", read[3]) };
    assert_eq!(octets(&parts[0]), [vec![2_u8; 300], vec![12_u8; 200]].concat());
    let Value::Row(fields) = &read[4] else { panic!("{:?}", read[4]) };
    let Value::Multiset(elements) = &fields[0] else { panic!("{:?}", fields[0]) };
    assert_eq!(text(elements[0].as_lob().unwrap()), "ab€cd");
    assert_eq!(text(read[5].as_lob().unwrap()), "b€");
    assert_eq!(octets(&read[6]), [b"x".to_vec(), vec![3_u8; 300], vec![13_u8; 200]].concat());
    assert!(matches!(read[7].as_lob().unwrap().parts(), [Part::Reference(9)]) && read[7].as_lob().unwrap().octets() == 4);
    assert_eq!(read[8].as_lob().and_then(Lob::locator), Some(12));
    assert_eq!(format!("{lobs:?}"), "SpilledLobs { tables: 1, others: 1 }", "one source a table, and the temporary one");
}

/// Verifies: [DESIGN] RULES.md D38, D79: a value written alone
/// (`put_value`) is read back as one; records of one spill read by
/// another are refused (SQLCODE -910) until it holds what the first does
/// (`absorb`); a naming of no known form, a part of none, and a reference
/// to a value no record names are SQLCODE -916.
#[test]
fn records_name_only_what_their_spill_holds() {
    let objects = table();
    let (lobs, other) = (SpilledLobs::new(), SpilledLobs::new());
    let v = Value::Array(Arc::from(vec![Value::Lob(object(&objects, 1)), Value::Lob(octets_by(&[5_u8; 9], 4))]));
    let mut out = Vec::new();
    lobs.put_value(&mut out, &v);
    let mut r = Reader::new(&out, "a test");
    let Value::Array(read) = lobs.value(&mut r).unwrap() else { panic!("an array") };
    r.finish().unwrap();
    assert_eq!((octets(&read[0]).len(), octets(&read[1])), (500, vec![5_u8; 9]));
    let row = vec![v];
    let record = lobs.record(&row);
    let refused = other.record_row(&record, &TypeRegistry::default()).err().map(|e| e.code);
    assert_eq!(refused, Some(Code::Api));
    other.absorb(&lobs);
    other.absorb(&other.clone());
    assert_eq!(read_back(&lobs, &other, &row).len(), 1);
    let types = TypeRegistry::default();
    let mut plain = Vec::new();
    put_row(&mut plain, &row);
    let code = |bytes: &[u8]| lobs.record_row(bytes, &types).err().map(|e| e.code);
    assert_eq!(code(&[plain.as_slice(), &[2_u8]].concat()), Some(Code::Corrupt), "no such naming");
    let no_part = [plain.as_slice(), &[1_u8], &1_u32.to_le_bytes(), &1_u32.to_le_bytes(), &[9_u8]].concat();
    assert_eq!(code(&no_part), Some(Code::Corrupt), "no such part");
    let dangling = vec![Value::Lob(Lob::of_reference(LobKind::Binary, 4, (1, 1)))];
    let mut unnamed = Vec::new();
    put_row(&mut unnamed, &dangling);
    let one_inline =
        [&[1_u8][..], &1_u32.to_le_bytes(), &1_u32.to_le_bytes(), &[INLINE], &1_u64.to_le_bytes(), &1_u32.to_le_bytes(), b"z"].concat();
    assert_eq!(code(&[unnamed.as_slice(), &one_inline].concat()), Some(Code::Corrupt), "a reference past those named");
}
