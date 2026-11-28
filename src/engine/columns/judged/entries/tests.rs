//! Unit tests of a LIKE judged of a dictionary's entries once each
//! (`entries.rs`, RULES.md D1253).

use super::super::super::Fields;
use super::*;
use crate::kernels::Portable;
use crate::value::{DataType, LikePattern, Value};

const TEXT: DataType = DataType::varying(100);

/// The rows of the chunk, and its entries.
const ROWS: u32 = 400;
const ENTRIES: u32 = 40;

/// The text of row `n`, one of `ENTRIES` web addresses, null every 13th.
fn text(n: u32) -> Value {
    if n % 13 == 5 {
        return Value::Null;
    }
    let e = n.wrapping_mul(7) % ENTRIES;
    Value::Char(format!("http://site{e}.example/{}/p", ["red", "green", "blue"][index(e % 3)]).into())
}

/// The bytes of a chunk of `ROWS` rows of `text`.
fn written() -> Vec<u8> {
    let fields = Fields::of(&TEXT, (0..ROWS).map(text)).unwrap();
    fields.chunk(&TEXT, 0, (fields.len(), true)).unwrap()
}

/// `LIKE pattern`, or NOT LIKE if `not`.
fn like(pattern: &str, not: bool) -> Liked {
    Liked::of(&LikePattern::new(pattern, None).unwrap(), not).unwrap()
}

/// The rows of `these`, places from `from`, whose text `like` keeps, as
/// each alone is matched.
fn alone(like: &Liked, from: u32, these: &[u32]) -> Vec<u32> {
    let keeps = |r: &u32| if let Value::Char(s) = text(from.saturating_add(*r)) { like.keeps(s.as_bytes()) } else { false };
    these.iter().copied().filter(keeps).collect()
}

/// The rows of `these`, places from `from`, that `like` keeps of `chunk`
/// judged by its entries, pushed after what was held.
fn by_entries(chunk: &Chunk<'_>, like: &Liked, from: u32, these: &[u32]) -> Vec<u32> {
    let mut out = vec![7];
    assert_eq!(liked(chunk, (like, &Portable), (0, from), these, &mut out), Some(()));
    assert_eq!(out.first(), Some(&7), "what was held stays");
    out.split_off(1)
}

/// Verifies: [X3.135-1986 §5.14 GR4, GR5] (RULES.md D1253) a LIKE and a
/// NOT LIKE judged of a dictionary's entries once each keep the rows
/// whose text the predicate is true of, as each row's text matched alone
/// keeps them, and no null: of a piece between two `%`s, searched for in
/// the entries as one arena, of two pieces, of a first piece and of a
/// last, matched an entry at a time, and of `%` alone; of every row, of
/// every other, and of rows from a place past the chunk's first.
#[test]
fn a_dictionarys_entries_are_judged_once_each() {
    let bytes = written();
    let chunk = Chunk::parse(&bytes).unwrap();
    assert!(matches!(chunk.body, Body::Dictionary { .. }), "a dictionary");
    let every: Vec<u32> = (0..ROWS).collect();
    let other: Vec<u32> = (0..ROWS).step_by(2).collect();
    let later: Vec<u32> = (0..300).collect();
    let patterns = ["%green%", "%site1%e/%", "http://site3%", "%blue/p", "%", "%nothing%"];
    for (pattern, not) in patterns.into_iter().flat_map(|p| [(p, false), (p, true)]) {
        let like = like(pattern, not);
        assert_eq!(by_entries(&chunk, &like, 0, &every), alone(&like, 0, &every), "{pattern:?}, NOT {not}");
        assert_eq!(by_entries(&chunk, &like, 0, &other), alone(&like, 0, &other), "{pattern:?}, NOT {not}, every other row");
        assert_eq!(by_entries(&chunk, &like, 100, &later), alone(&like, 100, &later), "{pattern:?}, NOT {not}, from row 100");
    }
    assert!(!alone(&like("%green%", false), 0, &every).is_empty() && !alone(&like("%green%", true), 0, &every).is_empty());
}

/// Verifies: [DESIGN] RULES.md D1253: the entries are judged only where
/// the rows asked of the chunk are no fewer than they. Of fewer rows
/// nothing is judged or pushed, and the rows are matched a row at a
/// time; so of a chunk not a dictionary. A dictionary whose entries do
/// not lie one after another within its octets is not judged, and
/// neither is it a row at a time, so its rows are all kept and decoding
/// raises what is malformed.
#[test]
fn a_dictionary_of_more_entries_than_rows_asked_is_not_judged_by_them() {
    let bytes = written();
    let chunk = Chunk::parse(&bytes).unwrap();
    let like = like("%green%", false);
    let (as_many, fewer): (Vec<u32>, Vec<u32>) = ((0..ENTRIES).collect(), (0..ENTRIES - 1).collect());
    let mut out = vec![7];
    assert_eq!(liked(&chunk, (&like, &Portable), (0, 0), &fewer, &mut out), None, "fewer rows than entries");
    assert_eq!(out, [7], "nothing pushed");
    assert_eq!(liked(&chunk, (&like, &Portable), (0, 0), &as_many, &mut out), Some(()), "as many rows as entries");
    let addresses = Fields::of(&TEXT, (0..300_u32).map(|k| Value::Char(format!("customer{k}@mail{}.example.com", k % 7).into()))).unwrap();
    let coded = addresses.chunk(&TEXT, 0, (addresses.len(), true)).unwrap();
    let not_one = Chunk::parse(&coded).unwrap();
    assert!(!matches!(not_one.body, Body::Dictionary { .. }));
    assert_eq!(liked(&not_one, (&like, &Portable), (0, 0), &as_many, &mut out), None, "not a dictionary");
    // The end of the first entry, past every other: the word after the
    // chunk's kind, count and nulls, the dictionary's bits and its count
    // of entries, and the first entry's start.
    let first_end = 1 + 4 + usize::try_from(ROWS).unwrap().div_ceil(8) + 1 + 4 + 4;
    let mut broken = bytes.clone();
    broken[first_end..first_end.saturating_add(4)].copy_from_slice(&u32::MAX.to_le_bytes());
    let malformed = Chunk::parse(&broken).unwrap();
    let every: Vec<u32> = (0..ROWS).collect();
    assert_eq!(liked(&malformed, (&like, &Portable), (0, 0), &every, &mut out), None, "entries out of order");
    let mut stepped = super::super::super::fsst::Stepped::default();
    let a_row_at_a_time = super::super::coded::liked_of(&malformed, (0, &like, &mut stepped), (0, 0), &every, &mut out);
    assert_eq!(a_row_at_a_time, None, "nor a row at a time");
}
