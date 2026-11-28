//! Unit tests: rows grouped by their codes as by their texts, and a
//! place's hash read of its run's entries' (RULES.md R-TEST-1, D1161,
//! D1330).

use std::sync::Arc;

use super::*;
use crate::kernels::Portable;
use crate::storage::{Coded, Ints, Mask};

/// The dictionary of `entries`.
fn dictionary(entries: &[&str]) -> Arc<Texts> {
    let mut ends = Vec::new();
    let mut text = String::new();
    for e in entries {
        text.push_str(e);
        ends.push(text.len());
    }
    Arc::new(Texts::new(text, ends, None))
}

/// A column of `codes` of `entries`, a row of none null.
fn coded_column(entries: &Arc<Texts>, codes: &[Option<u32>]) -> Column {
    let nulls = codes.iter().any(Option::is_none).then(|| Mask::from_bools(&codes.iter().map(Option::is_none).collect::<Vec<_>>()));
    Column::Texts(Texts::coded(Coded { codes: codes.iter().map(|c| c.unwrap_or(0)).collect(), entries: Arc::clone(entries) }, nulls))
}

/// The same column with its texts in place of its codes.
fn plain_column(entries: &Texts, codes: &[Option<u32>]) -> Column {
    let (mut text, mut ends) = (String::new(), Vec::new());
    for code in codes {
        if let Some(c) = code {
            text.push_str(entries.get(usize::try_from(*c).unwrap()).unwrap());
        }
        ends.push(text.len());
    }
    let nulls = codes.iter().any(Option::is_none).then(|| Mask::from_bools(&codes.iter().map(Option::is_none).collect::<Vec<_>>()));
    Column::Texts(Texts::new(text, ends, nulls))
}

/// A batch's dictionary of its first column, and its two columns' codes.
type Rows<'a> = (&'a Arc<Texts>, Vec<Option<u32>>, Vec<Option<u32>>);

/// The groups `batches` make, by codes if `coded` and else by hashing and
/// comparing every row: each its first row's number and its values.
fn groups_of(batches: &[Batch], coded: bool) -> String {
    let seed = Seed::fixed(0x5EED);
    let at = [0_usize, 1_usize];
    let mut table = GroupTable::new(2, &[], &seed, usize::MAX);
    let (mut memo, mut hashes, mut first) = (Memo::default(), Vec::new(), 0_u64);
    for batch in batches {
        if coded {
            assert!(grouped((&mut table, &mut memo), (batch, &at, None), (first, &seed, &Portable), &mut hashes), "coded");
        } else {
            hash_rows(batch, (&at, None), (&seed, &Portable), &mut hashes);
            for (r, &hash) in hashes.iter().enumerate() {
                let g = table.found_or_made(hash, Key::Row(batch, &at, r), first.saturating_add(u64::try_from(r).unwrap()));
                table.take(g, (r, r));
            }
        }
        first = first.saturating_add(u64::try_from(batch.rows).unwrap());
    }
    format!("{:?}", table.finished().unwrap())
}

/// Verifies: [DESIGN] RULES.md D1161: rows grouped by their codes make the
/// groups their texts make, in the same order, each of its first row and
/// its texts: batches of one dictionary, then of another that orders its
/// entries otherwise (the memo made anew, so the first row of the other,
/// R and F, makes a group of its own where the place of its codes held
/// that of N and F), nulls of either column in a group of their own, a
/// null's place apart from its column's first entry's (N and O after a
/// null and O), and two entries equal as keys, a text and the same with
/// trailing spaces, in one group.
#[test]
fn rows_grouped_by_their_codes_make_the_groups_of_their_texts() {
    let (flags, other, status) = (dictionary(&["N", "R", "A"]), dictionary(&["R", "N", "A  ", "A"]), dictionary(&["F", "O"]));
    let rows: [Rows<'_>; 3] = [
        (&flags, vec![Some(0), Some(1), Some(0), Some(2), None, Some(1)], vec![Some(0), Some(1), Some(0), Some(0), Some(1), None]),
        (&flags, vec![Some(2), Some(2), Some(0), None, Some(0)], vec![Some(0), Some(1), Some(0), Some(1), Some(1)]),
        (&other, vec![Some(0), Some(1), Some(2), Some(3), Some(0)], vec![Some(0), Some(0), Some(0), Some(0), None]),
    ];
    let batch = |coded: bool| -> Vec<Batch> {
        rows.iter()
            .map(|(entries, first, second)| {
                let columns = if coded {
                    vec![coded_column(entries, first), coded_column(&status, second)]
                } else {
                    vec![plain_column(entries, first), plain_column(&status, second)]
                };
                Batch { columns, rows: first.len() }
            })
            .collect()
    };
    let by_codes = groups_of(&batch(true), true);
    assert_eq!(by_codes, groups_of(&batch(false), false));
    assert!(
        !by_codes.contains("A  ") && by_codes.contains("\"A\""),
        "the text with its spaces found the group of the one without: {by_codes}"
    );
}

/// Verifies: [DESIGN] RULES.md D1161, the fallbacks: a grouping by a
/// column that is not coded, of one coded column beside one of integers,
/// or by dictionaries whose places pass `MOST_PLACES`, is not grouped by
/// codes, and nothing is put in the table. MC/DC of `ready`: every column
/// coded and few places (taken) against each alone false.
#[test]
fn keys_not_all_coded_or_of_too_many_places_are_grouped_as_before() {
    let seed = Seed::fixed(1);
    let mut table = GroupTable::new(2, &[], &seed, usize::MAX);
    let (mut memo, mut hashes) = (Memo::default(), Vec::new());
    let mut try_group = |batch: &Batch| grouped((&mut table, &mut memo), (batch, &[0, 1], None), (0, &seed, &Portable), &mut hashes);
    let small = dictionary(&["x", "y"]);
    let ints = Column::Ints(Ints::new(vec![1_i64, 2_i64], 0, None));
    assert!(!try_group(&Batch { columns: vec![coded_column(&small, &[Some(0), Some(1)]), ints], rows: 2 }), "one column of integers");
    let names: Vec<String> = (0_u32..300_u32).map(|n| format!("e{n}")).collect();
    let many = dictionary(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let wide = Batch { columns: vec![coded_column(&many, &[Some(0)]), coded_column(&many, &[Some(299)])], rows: 1 };
    assert!(!try_group(&wide), "301 times 301 places");
    let taken = Batch { columns: vec![coded_column(&small, &[Some(0)]), coded_column(&many, &[Some(299)])], rows: 1 };
    assert!(try_group(&taken), "3 times 301 places");
    assert_eq!(table.len(), 1, "only the batch taken made a group");
}

/// Asserts that each row given of `batch`, `rows` or all, grouped by its
/// columns `at`, reads of a memo made anew for them the hash `hash_of`
/// makes of the row, which is the hash `plain`, the same rows with their
/// texts in place of their codes, is given a vector at a time.
fn assert_hashes_read((batch, plain): (&Batch, &Batch), at: &[usize], rows: Option<&[u32]>, seed: &Seed) {
    let (mut memo, mut scratch, mut by_vector) = (Memo::default(), Vec::new(), Vec::new());
    assert!(memo.ready(batch, at, rows, (seed, &Portable)), "coded");
    hash_rows(plain, (at, rows), (seed, &Portable), &mut by_vector);
    for (n, of_texts) in by_vector.iter().enumerate() {
        let r = nth(rows, n);
        let read = memo.hash(memo.found(n).0, seed);
        assert_eq!(read, Some(hash_of(batch, (at, r), (seed, &Portable), &mut scratch)), "row {r} by {at:?}");
        assert_eq!(read, Some(*of_texts), "row {r} by {at:?}, as of its text");
    }
}

/// Verifies: [DESIGN] RULES.md D1330: the hash a memo reads for a place
/// is, entry by entry, the hash its row is given alone and the hash the
/// same text is given in a vector of texts, so the tables of every morsel
/// agree on a key's partition: of one column and of two joined in the
/// key's order, of a null's place in either column, of an empty text, of
/// texts that differ by trailing spaces (one hash padded, two unpadded)
/// and of a text longer than the kernels hash in one piece, by a seed
/// that pads and by one whose first column is grouped unpadded.
#[test]
fn a_place_reads_the_hash_its_row_is_given() {
    let long = "u".repeat(300);
    let (urls, status) = (dictionary(&["a", "a  ", "", long.as_str(), "b "]), dictionary(&["F", "O"]));
    let first = [Some(0), Some(1), None, Some(2), Some(3), Some(4), Some(0), Some(3)];
    let second = [Some(0), Some(1), Some(1), None, Some(0), Some(1), None, Some(1)];
    let batch = Batch { columns: vec![coded_column(&urls, &first), coded_column(&status, &second)], rows: first.len() };
    let plain = Batch { columns: vec![plain_column(&urls, &first), plain_column(&status, &second)], rows: first.len() };
    let padded = Seed::fixed(0x5EED);
    let unpadded = padded.unpadded(&[true, false]);
    for seed in [&padded, &unpadded] {
        assert_hashes_read((&batch, &plain), &[0], None, seed);
        assert_hashes_read((&batch, &plain), &[0, 1], None, seed);
        assert_hashes_read((&batch, &plain), &[1, 0], None, seed);
        assert_hashes_read((&batch, &plain), &[0, 1], Some(&[7, 2, 3, 1, 0, 5, 4]), seed);
    }
    let hash_at = |seed: &Seed, r: usize| hash_of(&batch, (&[0], r), (seed, &Portable), &mut Vec::new());
    assert_eq!(hash_at(&padded, 0), hash_at(&padded, 1), "padded, the spaces are no part of the key");
    assert_ne!(hash_at(&unpadded, 0), hash_at(&unpadded, 1), "unpadded, they are");
}

/// Verifies: [DESIGN] RULES.md D1330, the bound: a memo made anew for
/// fewer rows than its dictionaries hold entries keeps no hash, so the
/// few entries those rows meet are hashed alone, and it keeps none of a
/// dictionary whose entries are themselves codes; the groups are the
/// same either way. MC/DC of `hashed`: entries no more than the rows and
/// each dictionary plain (kept) against each alone false, the rows
/// counted as those given, not the batch's.
#[test]
fn a_run_of_more_entries_than_rows_given_is_not_hashed_whole() {
    let seed = Seed::fixed(7);
    let entries = dictionary(&["p", "q", "r", "s"]);
    let codes = [Some(3), Some(0), Some(1), Some(2), Some(3)];
    let batch = Batch { columns: vec![coded_column(&entries, &codes)], rows: codes.len() };
    let kept = |batch: &Batch, rows: Option<&[u32]>| {
        let mut memo = Memo::default();
        assert!(memo.ready(batch, &[0], rows, (&seed, &Portable)), "coded");
        memo.hash(memo.found(0).0, &seed)
    };
    let alone = |r: usize| Some(hash_of(&batch, (&[0], r), (&seed, &Portable), &mut Vec::new()));
    assert_eq!(kept(&batch, None), alone(0), "four entries, five rows");
    assert_eq!(kept(&batch, Some(&[1, 4, 2, 0])), alone(1), "four entries, four rows given");
    assert_eq!(kept(&batch, Some(&[4, 2, 0])), None, "four entries, three rows given of five");
    let nested = Arc::new(Texts::coded(Coded { codes: vec![1, 0], entries: dictionary(&["x", "y"]) }, None));
    let of_codes = Batch { columns: vec![coded_column(&nested, &[Some(0), Some(1), Some(0)])], rows: 3 };
    assert_eq!(kept(&of_codes, None), None, "two entries, three rows, the entries codes themselves");
    let groups = |batch: &Batch, rows: Option<&[u32]>| {
        let mut table = GroupTable::new(1, &[], &seed, usize::MAX);
        assert!(grouped((&mut table, &mut Memo::default()), (batch, &[0], rows), (0, &seed, &Portable), &mut Vec::new()), "coded");
        format!("{:?}", table.finished().unwrap())
    };
    assert_eq!(groups(&batch, Some(&[4, 2, 0])), groups(&batch, Some(&[4, 2, 0, 0])), "hashed alone or read, the groups of s, q and r");
    assert!(groups(&of_codes, None).contains("\"y\""), "the entries of codes are grouped by their texts");
}
