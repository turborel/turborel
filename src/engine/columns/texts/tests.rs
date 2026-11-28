//! Unit tests of a run's texts gathered of its chunks (`texts.rs`).

use std::collections::BTreeSet;

use super::super::encoded::{CONSTANT, DICTIONARY, FIXED, FSST, VARYING};
use super::super::tests::records_of;
use super::super::{Fields, HEADER, padding};
use super::*;
use crate::kernels::Portable;
use crate::value::{DataType, Decimal, Spelled, Value};

const TEXT: DataType = DataType::varying(100);
const CHAR: DataType = DataType::character(40);
const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };

/// The chunk of all of `values` of `t`, compressed or not, which is of
/// the encoding `kind`.
fn chunk_of(t: &DataType, values: &[Value], (compress, kind): (bool, u8)) -> Vec<u8> {
    let fields = Fields::of(t, values.iter().cloned()).unwrap();
    let bytes = fields.chunk(t, 0, (fields.len(), compress)).unwrap();
    assert_eq!(bytes[0], kind, "the encoding chosen");
    bytes
}

/// A VARCHAR dictionary chunk of 40 values of the three `entries`, whatever
/// octets they are.
fn dictionary_of(entries: [&[u8]; 3]) -> Vec<u8> {
    let written: Vec<Option<Vec<u8>>> = (0..40).map(|n| Some(entries[n % 3].to_vec())).collect();
    let fields = Fields::from_records(&TEXT, std::borrow::Cow::Owned(records_of(&written)));
    let bytes = fields.chunk(&TEXT, 0, (40, true)).unwrap();
    assert_eq!(bytes[0], DICTIONARY);
    bytes
}

/// `n` texts, every eleventh null, the rest `text` of their number.
fn values(n: u32, text: impl Fn(u32) -> String) -> Vec<Value> {
    (0..n).map(|k| if k % 11 == 4 { Value::Null } else { Value::Char(text(k).into()) }).collect()
}

/// Texts every one apart, some empty, some not ASCII, some with a
/// trailing space, as varying fields hold them.
fn apart(n: u32) -> Vec<Value> {
    values(n, |k| {
        ["", "é", "a ", "text"][usize::try_from(k % 4).unwrap()].repeat(usize::try_from(k % 3).unwrap().saturating_add(1)) + &k.to_string()
    })
}

/// Texts of 40 values, one of them empty, as a dictionary holds them.
fn few(n: u32) -> Vec<Value> {
    values(n, |k| if k % 40 == 9 { String::new() } else { format!("entrée {}", k.wrapping_mul(7) % 40) })
}

/// Texts alike in their parts, as a table of symbols codes them.
fn alike(n: u32) -> Vec<Value> {
    values(n, |k| format!("customer{k}@mail{}.example.com", k % 7))
}

/// A chunk's bytes and the rows asked of it.
type Part<'a> = (&'a [u8], Asked<'a>);

/// What `texts` read as, each row's text or NULL.
fn shown(texts: &Texts) -> Vec<String> {
    (0..texts.len()).map(|n| texts.get(n).map_or_else(|| "NULL".to_owned(), str::to_owned)).collect()
}

/// The values of the chunk `bytes` of `t` at `asked`, each read alone.
fn read(t: &DataType, bytes: &[u8], asked: Asked<'_>) -> Vec<String> {
    let chunk = Chunk::parse(bytes).unwrap();
    let mut out = Vec::new();
    asked
        .each(|n| {
            out.push(chunk.value(n, t).unwrap().to_string());
            Some(())
        })
        .unwrap();
    out
}

/// The texts of `t` gathered of each of `parts`, a chunk's bytes and the
/// rows asked of it, one after another as a run's.
fn gathered<B: AsRef<[u8]>>(t: &DataType, parts: &[(B, Asked<'_>)]) -> Option<Texts> {
    let mut into = Gathered::with_room(parts.iter().map(|(_, asked)| asked.count()).sum(), 0);
    for (bytes, asked) in parts {
        Chunk::parse(bytes.as_ref()).unwrap().gather(*asked, padding(t)?, &mut into)?;
    }
    into.finish(&Portable)
}

/// How many entries `texts` hold, if their rows are codes.
fn entries(texts: &Texts) -> Option<usize> {
    texts.coded.as_ref().map(|coded| coded.entries.len())
}

/// How many texts of `rows` differ, nulls left out.
fn distinct(rows: &[String]) -> usize {
    rows.iter().filter(|text| *text != "NULL").collect::<BTreeSet<_>>().len()
}

/// Verifies: [DESIGN] RULES.md D1252, D1131 [X3.135-1986 §8.7 GR7b], a
/// run's texts gathered: every encoding of strings gives, of a run of
/// its positions and of positions apart, dense and few of their span,
/// the values read one at a time, nulls as nulls, the empty string as
/// itself, VARCHAR as written and CHARACTER padded to its length by
/// characters, not octets. Of no rows it gives no text; of integers
/// none; none past its end; and texts coded by symbols none padded.
#[test]
fn every_encoding_gives_its_rows_texts() {
    let constant: Vec<Value> = values(60, |_| "sämé".to_owned());
    let dense: Vec<usize> = (2..58).filter(|n| n % 2 == 0).collect();
    let rows = [Asked::Run(0, 60), Asked::Run(3, 17), Asked::Run(5, 5), Asked::At(&[1, 4, 40, 59]), Asked::At(&dense)];
    for t in [TEXT, CHAR] {
        let chunks = [
            (chunk_of(&t, &apart(60), (false, VARYING)), "varying"),
            (chunk_of(&t, &few(300), (true, DICTIONARY)), "a dictionary"),
            (chunk_of(&t, &constant, (true, CONSTANT)), "a constant"),
        ];
        for (bytes, kind) in &chunks {
            for asked in rows {
                assert_eq!(shown(&gathered(&t, &[(bytes, asked)]).unwrap()), read(&t, bytes, asked), "{kind} of {t:?}, {asked:?}");
            }
        }
    }
    let symbols = chunk_of(&TEXT, &alike(300), (true, FSST));
    for asked in rows.into_iter().chain([Asked::Run(17, 260)]) {
        assert_eq!(shown(&gathered(&TEXT, &[(&symbols, asked)]).unwrap()), read(&TEXT, &symbols, asked), "symbols, {asked:?}");
    }
    let padded = gathered(&CHAR, &[(&chunk_of(&CHAR, &constant, (true, CONSTANT)), Asked::Run(0, 1))]).unwrap();
    assert_eq!(shown(&padded), [format!("sämé{}", " ".repeat(36))], "padded by characters, not octets");
    let numbers: Vec<Value> = (0_i128..10_i128).map(|n| Value::Exact(Decimal::new(n, 0))).collect();
    let fixed = chunk_of(&INTEGER, &numbers, (false, FIXED));
    assert!(Chunk::parse(&fixed).unwrap().gather(Asked::Run(0, 10), None, &mut Gathered::default()).is_none(), "integers");
    assert!(gathered(&TEXT, &[(&symbols, Asked::Run(0, 301))]).is_none(), "a run past the end");
    assert!(gathered(&TEXT, &[(&symbols, Asked::At(&[0, 300]))]).is_none(), "a position past the end");
    assert!(Chunk::parse(&symbols).unwrap().gather(Asked::Run(0, 3), Some(40), &mut Gathered::default()).is_none(), "symbols padded");
}

/// Verifies: [DESIGN] RULES.md D1316, a dictionary's entries: a chunk
/// asked for no fewer rows than it has entries gives its entries whole
/// and each row's code, and one asked for fewer gives only the entries
/// those rows read, each once, so a run never holds more entries than
/// rows; either way the rows read as their values, of VARCHAR and of
/// CHARACTER padded. MC/DC of the entries given whole: 40 rows of 40
/// entries (whole) against 39 (those read). Two chunks read by their
/// rows' entries one after another keep their entries apart.
#[test]
fn a_dictionary_gives_its_entries_whole_or_those_its_rows_read() {
    let dense: Vec<usize> = (0..60).filter(|n| n % 2 == 0).collect();
    for t in [TEXT, CHAR] {
        let bytes = chunk_of(&t, &few(300), (true, DICTIONARY));
        for (asked, whole) in [(Asked::Run(0, 300), true), (Asked::Run(20, 60), true), (Asked::At(&(100..140).collect::<Vec<_>>()), true)] {
            let made = gathered(&t, &[(&bytes, asked)]).unwrap();
            assert_eq!((shown(&made), entries(&made)), (read(&t, &bytes, asked), Some(40)), "{asked:?} whole={whole}");
        }
        for asked in [Asked::Run(20, 59), Asked::Run(7, 8), Asked::At(&[3, 50, 51, 52, 290]), Asked::At(&dense)] {
            let (made, rows) = (gathered(&t, &[(&bytes, asked)]).unwrap(), read(&t, &bytes, asked));
            assert_eq!(shown(&made), rows, "{asked:?}");
            assert_eq!(entries(&made), Some(distinct(&rows)), "{asked:?}: an entry of each text the rows read, once");
        }
        let other = chunk_of(&t, &values(300, |k| format!("other {}", k % 37)), (true, DICTIONARY));
        let parts =
            [(bytes.as_slice(), Asked::Run(20, 50)), (other.as_slice(), Asked::Run(20, 50)), (bytes.as_slice(), Asked::Run(20, 50))];
        let made = gathered(&t, &parts).unwrap();
        let rows: Vec<String> = parts.iter().flat_map(|(chunk, asked)| read(&t, chunk, *asked)).collect();
        assert_eq!(shown(&made), rows, "three chunks by the entries their rows read");
        assert_eq!(entries(&made), Some(parts.iter().map(|(chunk, asked)| distinct(&read(&t, chunk, *asked))).sum()));
    }
}

/// Verifies: [DESIGN] RULES.md D1316, a run across encodings: rows of
/// chunks that share no entry are texts and their ends; once a chunk of
/// the run shares entries among its rows, a dictionary or one value,
/// every row has a code, the rows gathered before given the codes of the
/// entries they are and the rows after of a chunk that shares nothing
/// each an entry of its own; in any order of the chunks the rows read
/// as their values, nulls and the empty string among them, with no more
/// entries than rows. A chunk of one value is one entry, and one of
/// nulls alone none. MC/DC of the codes: no sharing chunk (texts)
/// against one (codes), first, in the middle and last.
#[test]
fn a_run_across_encodings_has_codes_once_a_chunk_shares_entries() {
    let same = values(60, |_| "one value".to_owned());
    let (varying, symbols) = (chunk_of(&TEXT, &apart(60), (false, VARYING)), chunk_of(&TEXT, &alike(300), (true, FSST)));
    let (dictionary, constant) = (chunk_of(&TEXT, &few(300), (true, DICTIONARY)), chunk_of(&TEXT, &same, (true, CONSTANT)));
    let (v, s, d, c) = (varying.as_slice(), symbols.as_slice(), dictionary.as_slice(), constant.as_slice());
    let part = |bytes, asked| (bytes, asked);
    let runs: [(Vec<Part<'_>>, bool); 6] = [
        (vec![part(v, Asked::Run(0, 60)), part(s, Asked::Run(10, 90)), part(v, Asked::At(&[2, 4, 30]))], false),
        (vec![part(v, Asked::Run(0, 60)), part(d, Asked::Run(0, 300)), part(s, Asked::Run(10, 90)), part(c, Asked::Run(0, 60))], true),
        (vec![part(d, Asked::Run(250, 300)), part(v, Asked::Run(3, 60)), part(s, Asked::At(&[4, 5, 200]))], true),
        (vec![part(s, Asked::Run(0, 300)), part(v, Asked::At(&[4, 9, 15])), part(d, Asked::At(&[4, 8, 299]))], true),
        (vec![part(s, Asked::Run(0, 30)), part(c, Asked::Run(2, 6)), part(v, Asked::Run(0, 12))], true),
        (vec![part(c, Asked::Run(0, 60))], true),
    ];
    for (parts, coded) in &runs {
        let made = gathered(&TEXT, parts).unwrap();
        let rows: Vec<String> = parts.iter().flat_map(|(bytes, asked)| read(&TEXT, bytes, *asked)).collect();
        assert_eq!(shown(&made), rows, "{parts:?}");
        assert_eq!(made.coded.is_some(), *coded, "codes once a chunk shares entries");
        assert!(entries(&made).is_none_or(|held| held <= rows.len()), "no more entries than rows");
    }
    assert_eq!(entries(&gathered(&TEXT, &runs[5].0).unwrap()), Some(1), "one value, one entry");
    let nulls = chunk_of(&TEXT, &vec![Value::Null; 20], (true, CONSTANT));
    let of_nulls = gathered(&TEXT, &[(&nulls, Asked::Run(0, 20))]).unwrap();
    assert_eq!((shown(&of_nulls), entries(&of_nulls)), (vec!["NULL".to_owned(); 20], Some(0)), "nulls alone, no entry");
}

/// Verifies: [DESIGN] RULES.md D1252, octets judged once a run: entries
/// that are UTF-8 give each row's text, of a run with no null after
/// another chunk's rows too; a chunk with an entry that is
/// not UTF-8 gives no text, alone and after rows that are text; two
/// entries that are UTF-8 only together, the first ending within a
/// character, give none, as each end must lie on a character's boundary;
/// and a dictionary whose entry ends before it begins, or whose row's
/// code is of no entry, gives none. Robustness: octets no text holds and
/// malformed chunks.
#[test]
fn octets_that_are_no_text_give_no_column() {
    let good = dictionary_of([b"alpha entry", b"beta entry", b"gamma entry"]);
    let rows = shown(&gathered(&TEXT, &[(&good, Asked::At(&[0, 1, 2, 39]))]).unwrap());
    assert_eq!(rows, ["alpha entry", "beta entry", "gamma entry", "alpha entry"]);
    let other = dictionary_of([b"one", b"two", b"three"]);
    let after = gathered(&TEXT, &[(&other, Asked::At(&[0, 1, 2, 3])), (&good, Asked::Run(0, 40))]).unwrap();
    assert_eq!(shown(&after)[4..8], rows, "a run with no null takes its codes past the entries before it");
    let bad = dictionary_of([b"alpha entry", b"\xffbeta entry", b"gamma entry"]);
    for asked in [Asked::Run(0, 40), Asked::Run(0, 2), Asked::At(&[1])] {
        assert!(gathered(&TEXT, &[(&bad, asked)]).is_none(), "octets not UTF-8, {asked:?}");
        assert!(gathered(&TEXT, &[(&good, Asked::Run(0, 40)), (&bad, asked)]).is_none(), "after rows that are text, {asked:?}");
    }
    assert!(gathered(&TEXT, &[(&bad, Asked::At(&[0, 2]))]).is_some(), "the entry that is no text is not read");
    let split = dictionary_of(["é".as_bytes().split_at(1).0, "é".as_bytes().split_at(1).1, b"e"]);
    assert!(gathered(&TEXT, &[(&split, Asked::Run(0, 40))]).is_none(), "an end within a character");
    // The chunk's kind, count and nulls, then the bits of a code, the
    // number of entries, and where each entry begins.
    let offsets = HEADER + 40_usize.div_ceil(8) + 1 + WORD;
    let mut crossed = good.clone();
    crossed[offsets + WORD] = 30;
    assert!(gathered(&TEXT, &[(&crossed, Asked::Run(0, 40))]).is_none(), "an entry that ends before it begins");
    let mut coded = good.clone();
    let last = coded.len() - 1;
    coded[last] = 0xFF;
    assert!(gathered(&TEXT, &[(&coded, Asked::Run(0, 40))]).is_none(), "a code of no entry");
    assert!(gathered(&TEXT, &[(&coded, Asked::At(&[39]))]).is_none(), "a code of no entry, read alone");
}
