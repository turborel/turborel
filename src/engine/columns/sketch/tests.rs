//! Unit tests of column sketches (`sketch.rs`): the order of padded texts,
//! the edges drawn and the codes given, and a group's sketches written and
//! read back.

use std::borrow::Cow;
use std::sync::Arc;

use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::columns::group::{Group, read};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::memory::MemoryBudget;
use crate::value::{CharUnits, Decimal, Spelled, compare_strings};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const URL: DataType = DataType::varying(100);
const CHARACTER: DataType = DataType::Character { length: 12, units: CharUnits::Characters };
/// K, a text of many values, a text of few, and a CHARACTER string.
const TYPES: [DataType; 4] = [INTEGER, URL, URL, CHARACTER];
const ROWS: u32 = 6000;

/// Pages stored as they are given.
struct Plain;

impl Seal for Plain {
    fn seal(&self, _: u64, content: &[u8]) -> Result<Vec<u8>> {
        Ok(content.to_vec())
    }
    fn open<'a>(&self, _: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(stored))
    }
}

fn file() -> (Arc<BufferPool>, FileId) {
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// A text of many values, as a column of web addresses holds: every
/// seventeenth null, every fifth one address, some ending in spaces.
fn url(n: u32) -> Value {
    if n.is_multiple_of(17) {
        return Value::Null;
    }
    if n % 7 == 3 {
        return Value::Char("http://often.example/".into());
    }
    let pad = if n.is_multiple_of(13) { "  " } else { "" };
    Value::Char(format!("http://site{}.example/page{}{pad}", n.wrapping_mul(7919) % 500, n.wrapping_mul(104_729) % 100_000).into())
}

fn row(n: u32) -> Row {
    vec![
        Value::Exact(Decimal::new(i128::from(n), 0)),
        url(n),
        Value::Char(format!("w{}", n % 6).into()),
        Value::Char(format!("c{}", n % 900).into()),
    ]
}

/// Verifies: [DESIGN] RULES.md D525, texts ordered padded: of texts with
/// trailing spaces, a tab, one not ASCII, one below a space and the empty
/// one, each pair orders by bytes as `compare_strings` orders it.
#[test]
fn texts_order_padded_as_strings_compare() {
    let texts = ["", " ", "a", "a ", "a  ", "a\t", "ab", "b", "grün", "grun", "\u{1}", "zz", "a b", "a\u{1}"];
    for a in texts {
        for b in texts {
            assert_eq!(padded_order(a.as_bytes(), b.as_bytes()), compare_strings(a, b), "{a:?} against {b:?}");
        }
    }
}

/// Every text of `values`, of the strings `more` too, each the order of
/// its code's bin against each as `Bin::order` gives it, if it gives one,
/// is the text's own.
fn bins_order_as_values(edges: &Edges, values: &[&Value], more: &[&str]) {
    let bins: Vec<(u8, Bin)> = edges.bins().collect();
    let texts: Vec<&str> = values.iter().filter_map(|v| if let Value::Char(s) = v { Some(s.as_str()) } else { None }).collect();
    let constants: Vec<&str> = edges.each().map(|(t, _)| t).chain(more.iter().copied()).chain(texts.iter().copied().step_by(97)).collect();
    for v in &texts {
        let code = edges.code(v.as_bytes());
        let (_, bin) = bins.iter().find(|(c, _)| *c == code).copied().unwrap();
        for c in &constants {
            if let Some(o) = bin.order(edges.place(c.as_bytes())) {
                assert_eq!(o, compare_strings(v, c), "{v:?} of {bin:?} against {c:?}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D525, edges and codes: of a column of few
/// values, each an edge and a point; of one of many, at most 255 codes,
/// the most frequent value a point, codes in the order of the values,
/// equal values one code, and each code's bin ordering against every
/// constant, an edge, a value or one of none, as its values do; edges out
/// of order or of too many codes refused.
#[test]
fn codes_order_as_their_values() {
    let few: Vec<Value> = (0_u32..1000).map(|n| Value::Char(format!("w{}", n % 40).into())).collect();
    let few: Vec<&Value> = few.iter().collect();
    let edges = Edges::of(&few);
    assert_eq!(edges.each().count(), 40);
    assert!(edges.each().all(|(_, point)| point), "each a point");
    bins_order_as_values(&edges, &few, &["", "w", "w1 ", "zz"]);
    let many: Vec<Value> = (0..20_000).map(url).collect();
    let many: Vec<&Value> = many.iter().collect();
    let sketch = Edges::of(&many);
    let codes = sketch.bins().count();
    assert!((200..=MOST_CODES).contains(&codes), "{codes} codes");
    assert!(sketch.each().any(|(t, point)| point && t == "http://often.example/"), "the frequent value a point");
    assert!(sketch.each().filter(|(_, point)| *point).count() < 3, "few points");
    let mut texts: Vec<&str> = many.iter().filter_map(|v| if let Value::Char(s) = v { Some(s.as_str()) } else { None }).collect();
    texts.sort_by(|a, b| compare_strings(a, b));
    for pair in texts.windows(2) {
        let (a, b) = (sketch.code(pair[0].as_bytes()), sketch.code(pair[1].as_bytes()));
        assert!(a <= b, "{pair:?}");
        assert!(compare_strings(pair[0], pair[1]).is_ne() || a == b, "{pair:?} one code");
    }
    bins_order_as_values(&sketch, &many, &["", "http://", "http://often.example/ ", "http://site1", "zz"]);
    assert_eq!(code_of(&sketch, &Value::Null), NULL_CODE);
    assert!(Edges::new(vec!["b".into(), "a".into()], vec![false, false]).is_none(), "out of order");
    assert!(Edges::new(vec!["a".into(), "a ".into()], vec![false, false]).is_none(), "equal as padded");
    let wide: Vec<String> = (0_u32..200).map(|n| format!("e{n:03}")).collect();
    assert!(Edges::new(wide.clone(), vec![true; 200]).is_none(), "too many codes");
    assert!(Edges::new(wide, vec![false; 200]).is_some());
    assert!(Edges::of(&[&Value::Null, &Value::Null]).is_empty(), "nulls alone");
}

/// The group of `ROWS` rows, of pages of `capacity` bytes, the columns at
/// `named` sketched.
fn group(pages: &Pages<'_, Plain>, named: &[usize]) -> Group {
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let all: Vec<Row> = (0..ROWS).map(row).collect();
    Group::write(pages, &TYPES, (&ids, &all), (&[], &[], named)).unwrap()
}

/// Verifies: [DESIGN] RULES.md D525, COLUMNSTORE.SKETCH: of the columns
/// named, only a VARCHAR coded by symbols gets a sketch, not a key, a
/// dictionary's texts nor a CHARACTER string; each part of it has codes,
/// read back from the header with its edges; the codes a scan reads of a
/// run over parts are each value's; a group of pages too small for a
/// part's codes, or of no column named, has none.
#[test]
fn groups_keep_sketches_of_texts_coded_by_symbols() {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 8192,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: false,
        kernels: &crate::kernels::Portable,
    };
    let sketched = group(&pages, &[0, 1, 2, 3]);
    let has: Vec<bool> = sketched.sketches.iter().map(|e| !e.is_empty()).collect();
    assert_eq!(has, [false, true, false, false]);
    assert_eq!(sketched.sketched, [0, 1, 2, 3]);
    assert!(sketched.columns[1].iter().flat_map(|z| &z.parts).all(|p| p.sketch.is_some()));
    assert!(sketched.columns[2].iter().flat_map(|z| &z.parts).all(|p| p.sketch.is_none()));
    let back = read(&pages, sketched.header[0], &TYPES).unwrap();
    assert_eq!(back.sketches, sketched.sketches, "edges read back");
    assert_eq!(back.sketched, sketched.sketched);
    let mut held = Held::default();
    let codes = held.codes(&back, &pages, 1, (1000, 4100)).unwrap().unwrap();
    let expected: Vec<u8> = (1000..4100).map(|n| code_of(&back.sketches[1], &url(n))).collect();
    assert_eq!(codes, expected);
    assert_eq!(held.codes(&back, &pages, 0, (0, 10)).unwrap(), None, "no sketch of the key");
    held.clear();
    let small = Pages { capacity: 512, ..pages };
    assert!(group(&small, &[1]).sketches.iter().all(Edges::is_empty), "pages too small");
    let none = group(&pages, &[]);
    assert!(none.sketches.iter().all(Edges::is_empty) && none.sketched.is_empty());
}
