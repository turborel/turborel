//! Unit tests of a term's blocks of postings: their encoding, splits,
//! removals, cursors and their skips, and writers of one term at once
//! (RULES.md R-TEST-1, R-TEST-2).

use std::sync::atomic::Ordering;
use std::thread;

use super::*;
use crate::index::fulltext::block::{self, Builder, MOST_BLOCK_DOCUMENTS, Places, Posting, fits};
use crate::index::fulltext::cursor::{Cursor, Docs, intersect};
use crate::index::searched;

/// The blocks of `term` in `index`: each one's first number and postings.
fn blocks(index: &FullTextIndex, term: &[u8]) -> Vec<(u64, Vec<Posting>)> {
    let (lower, upper) = keys::block_range(term);
    let found = index.tree.scan(Bound::Included(&lower), Bound::Excluded(&upper), Direction::Forward);
    found
        .map(|entry| {
            let (key, value) = entry.unwrap();
            let first = keys::block_docno(&key, term).unwrap();
            (first, block::decode(first, &value).unwrap())
        })
        .collect()
}

/// The document numbers of `term`'s blocks, checking that each block holds
/// some, at most `MOST_BLOCK_DOCUMENTS`, from its key on, within its room,
/// and that the numbers rise across blocks.
fn numbers(index: &FullTextIndex, term: &[u8]) -> Vec<u64> {
    let mut all: Vec<u64> = Vec::new();
    for (first, postings) in blocks(index, term) {
        assert!(!postings.is_empty() && postings.len() <= MOST_BLOCK_DOCUMENTS, "{} documents", postings.len());
        assert!(fits(first, &postings, index.room(term)));
        assert!(postings[0].docno >= first);
        assert!(all.last().is_none_or(|last| *last < first), "blocks overlap at {first}");
        all.extend(postings.iter().map(|p| p.docno));
    }
    assert!(all.is_sorted_by(|a, b| a < b));
    all
}

/// Inserts a document holding "apple" and a word of its own in each row
/// of `rows`.
fn apples(index: &FullTextIndex, rows: impl Iterator<Item = RowId>) {
    for row in rows {
        index.insert(apple(row), row).unwrap();
    }
}

/// The document of row `row` that `apples` inserts.
fn apple(row: RowId) -> Key {
    doc(&format!("apple filler{row}"))
}

/// Verifies: [DESIGN] RULES.md D86, a block's encoding: numbers kept as
/// differences, from the key on, gaps of any size; frequencies of any
/// size; places kept in the block or flagged as past it; read back alike,
/// the numbers alone too. A block built a posting at a time takes those
/// that fit as encoding them says; a cut block is refused.
#[test]
fn a_block_reads_back_what_it_holds() {
    let postings = vec![
        Posting { docno: 7, frequency: 3, places: Places::Inline(vec![1, 0, 0, 0, 4, 1, 0, 6]) },
        Posting { docno: 8, frequency: 100_000, places: Places::Overflow },
        Posting { docno: 1 << 40_i32, frequency: 1, places: Places::Inline(vec![9, 1, 1, 0]) },
        Posting { docno: (1 << 40_i32) + 300, frequency: 2, places: Places::Inline(Vec::new()) },
    ];
    let value = block::encode(5, &postings);
    assert_eq!(block::decode(5, &value).unwrap(), postings);
    assert_eq!(block::docnos(5, &value).unwrap(), postings.iter().map(|p| p.docno).collect::<Vec<_>>());
    assert_eq!(block::decode(5, &value[..value.len() - 1]).unwrap_err().code, Code::Corrupt);
    for room in [10, 20, 30, 40, 1_000] {
        let mut builder = Builder::new(5);
        let taken = postings.iter().take_while(|p| builder.add((*p).clone(), room).is_none()).count();
        let most = (0..=postings.len()).rev().find(|n| fits(5, &postings[..*n], room)).unwrap();
        assert_eq!(taken, most, "room {room}");
        assert_eq!(block::encode(5, &builder.postings), block::encode(5, &postings[..taken]));
    }
}

/// Verifies: [DESIGN] RULES.md D86, splits: a term's documents fill
/// blocks of `MOST_BLOCK_DOCUMENTS`, each new one keyed by its first
/// number; documents of many places fill blocks by bytes first, each
/// within an entry's room, their places kept in the block and read back.
#[test]
fn blocks_split_at_their_count_and_their_bytes() {
    let index = index();
    apples(&index, 0..300);
    let split: Vec<(u64, usize)> = blocks(&index, b"apple").iter().map(|(first, p)| (*first, p.len())).collect();
    assert_eq!(split, [(0, 128), (128, 128), (256, 44)]);
    assert_eq!(numbers(&index, b"apple"), (0..300).collect::<Vec<u64>>());
    let wide = super::index();
    let each = wide.room(b"pear").checked_div(40).unwrap();
    let text = "pear ".repeat(each);
    for row in 0..60 {
        wide.insert(doc(&format!("{text}row{row}")), row).unwrap();
    }
    let found = blocks(&wide, b"pear");
    assert!(found.len() > 4 && found.iter().all(|(_, p)| p.len() < 20), "{} blocks", found.len());
    assert!(found.iter().flat_map(|(_, p)| p).all(|p| matches!(p.places, Places::Inline(_))));
    assert_eq!(numbers(&wide, b"pear").len(), 60);
    let (frequency, places) = wide.postings("pear", &doc(&format!("{text}row33")), 33).unwrap().unwrap();
    assert_eq!(usize::try_from(frequency).unwrap(), each);
    assert!(places.iter().map(|p| p.ordinal).eq(1..=u32::try_from(each).unwrap()));
}

/// Verifies: [DESIGN] RULES.md D86, removals: a document taken from the
/// first, the middle or the last of its block leaves the others, the
/// block keeping its key; a block emptied is deleted; searches find what
/// is left.
#[test]
fn removals_keep_blocks_and_delete_the_emptied() {
    let index = index();
    apples(&index, 0..300);
    for row in [0, 200, 299] {
        index.remove(&apple(row), row).unwrap();
    }
    let left: Vec<u64> = (1..300).filter(|n| ![200, 299].contains(n)).collect();
    assert_eq!(numbers(&index, b"apple"), left);
    assert_eq!(blocks(&index, b"apple").iter().map(|(first, _)| *first).collect::<Vec<_>>(), [0, 128, 256]);
    for row in (256..299).chain(1..128) {
        index.remove(&apple(row), row).unwrap();
    }
    assert_eq!(blocks(&index, b"apple").iter().map(|(first, _)| *first).collect::<Vec<_>>(), [128]);
    let want: Vec<u64> = (128..256).filter(|n| *n != 200).collect();
    assert_eq!(numbers(&index, b"apple"), want);
    let probe = Probe::Predicate {
        predicate: Box::new(Predicate {
            function: crate::functions::ScalarRef(Arc::new(crate::fulltext::matches::TextMatches)),
            column: 0,
            argument: Some(0),
        }),
        value: Value::Char("apple".into()),
    };
    let mut rows: Vec<RowId> = searched(&index, &probe).unwrap().into_iter().map(|(_, row)| row).collect();
    rows.sort_unstable();
    assert_eq!(rows, want);
}

/// Verifies: [DESIGN] RULES.md D86, a cursor: `advance_to` gives the
/// first number at or past its target, across many blocks, near or far,
/// none past the last; several cursors intersect to the numbers all hold.
#[test]
fn cursors_advance_across_blocks() {
    let index = index();
    for row in 0..2_000_u64 {
        let text = format!("{} {}", if row % 3 == 0 { "third" } else { "rest" }, if row % 5 == 0 { "fifth" } else { "other" });
        index.insert(doc(&text), row).unwrap();
    }
    let mut cursor = Cursor::new(&index, b"third".to_vec());
    for (target, want) in [(0, Some(0)), (1, Some(3)), (3, Some(3)), (500, Some(501)), (502, Some(504)), (1_998, Some(1_998))] {
        assert_eq!(cursor.advance_to(target).unwrap(), want, "to {target}");
    }
    assert_eq!(cursor.advance_to(1_999).unwrap(), None);
    let mut far = Cursor::new(&index, b"third".to_vec());
    assert_eq!(far.advance_to(1_500).unwrap(), Some(1_500));
    let both = intersect(vec![Cursor::new(&index, b"third".to_vec()), Cursor::new(&index, b"fifth".to_vec())]);
    let found: Vec<u64> = both.map(Result::unwrap).collect();
    assert_eq!(found, (0..2_000).step_by(15).collect::<Vec<u64>>());
}

/// Verifies: [DESIGN] RULES.md D86, skipping: a rare word and a common
/// one intersect by decoding the common word's blocks that may hold the
/// rare word's documents, far fewer than all of them.
#[test]
fn a_rare_word_skips_the_common_ones_blocks() {
    let index = index();
    for row in 0..20_000_u64 {
        index.insert(doc(if row % 2_000 == 7 { "common rare" } else { "common" }), row).unwrap();
    }
    let common = blocks(&index, b"common").len();
    assert!(common > 150, "{common} blocks");
    index.blocks_read.store(0, Ordering::Relaxed);
    let both = intersect(vec![Cursor::new(&index, b"common".to_vec()), Cursor::new(&index, b"rare".to_vec())]);
    let found: Vec<u64> = both.map(Result::unwrap).collect();
    assert_eq!(found, (7..20_000).step_by(2_000).collect::<Vec<u64>>());
    let read = index.blocks_read.load(Ordering::Relaxed);
    assert!(read <= 25, "{read} blocks decoded of {common}");
}

/// Writers of one term at once.
const WRITERS: u64 = 8;
/// The documents each writes.
const EACH: u64 = 300;

/// Inserts, or removes, writer `t`'s documents of the shared word.
fn sharing(index: Arc<FullTextIndex>, t: u64, remove: bool) -> impl FnOnce() + Send {
    move || {
        let first = t.checked_mul(1_000).unwrap();
        for row in first..first.checked_add(EACH).unwrap() {
            let key = doc(&format!("shared word{row}"));
            if remove {
                index.remove(&key, row).unwrap();
            } else {
                index.insert(key, row).unwrap();
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D86, term stripes: eight threads inserting
/// documents of one term at once, their numbers given in one order and
/// posted in another, leave its blocks whole: each document once, the
/// numbers rising, each block within its count and room; then removed at
/// once, they leave none.
#[test]
fn writers_of_one_term_keep_its_blocks_whole() {
    let index = index();
    let run = |remove: bool| {
        let handles: Vec<_> = (0..WRITERS).map(|t| thread::spawn(sharing(Arc::clone(&index), t, remove))).collect();
        for handle in handles {
            handle.join().unwrap();
        }
    };
    run(false);
    let all = WRITERS * EACH;
    assert_eq!(numbers(&index, b"shared"), (0..all).collect::<Vec<u64>>());
    assert_eq!(index.statistics("shared").unwrap().documents, all);
    run(true);
    assert_eq!(blocks(&index, b"shared"), Vec::new());
    assert_eq!(left(&index), Vec::new());
}
