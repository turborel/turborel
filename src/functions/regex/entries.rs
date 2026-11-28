//! \[DESIGN\] (RULES.md D1291) A regular expression function over a
//! vector of a dictionary's codes (`storage::Coded`). A column store
//! gives a character string column's rows as codes of the entries of its
//! chunks' dictionaries, and many rows read one entry, so the function
//! is run once of each entry a row reads and its result given to every
//! row of that entry's code.
//!
//! An entry no row reads is not run. A row filtered out above the call
//! leaves its entry in the dictionary, and the call of that entry may
//! raise an error the query never raised a row at a time. A dictionary of
//! more entries than the vector has rows is read a row at a time, as its
//! entries would be marked for nothing (`exec/batch/expr/compare.rs`
//! judges a comparison by the same rule).

use crate::storage::mask::index;
use crate::storage::{Coded, Mask, Texts};

/// The dictionary `subjects` are codes of, and of each of its entries
/// whether a row not null reads it: none unless `subjects` are codes, one
/// of each of `rows` rows, of a dictionary of no more entries than rows.
pub(super) fn read(subjects: &Texts, rows: usize) -> Option<(&Coded, Vec<bool>)> {
    let coded = subjects.coded.as_deref().filter(|coded| coded.codes.len() == rows && coded.entries.len() <= rows)?;
    let mut read = vec![false; coded.entries.len()];
    for (n, &code) in coded.codes.iter().enumerate() {
        if let Some(entry) = read.get_mut(index(code)).filter(|_| !subjects.is_null(n)) {
            *entry = true;
        }
    }
    Some((coded, read))
}

/// Which rows of `subjects`, codes of `coded`, are given a null: a row
/// that is null, and one whose entry `given` says was given no value, as
/// an entry read by no row, or past the dictionary's end, is given none.
/// None if no row is.
pub(super) fn nulls(subjects: &Texts, coded: &Coded, given: &[bool]) -> Option<Mask> {
    let mut nulls = Mask::none(coded.codes.len());
    for (n, &code) in coded.codes.iter().enumerate() {
        if subjects.is_null(n) || given.get(index(code)) != Some(&true) {
            nulls.set(n);
        }
    }
    nulls.any().then_some(nulls)
}
