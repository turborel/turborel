//! Unit tests: rows grouped by their codes as by their texts (RULES.md
//! R-TEST-1, D1161).

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
