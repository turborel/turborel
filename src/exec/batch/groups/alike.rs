//! \[DESIGN\] (RULES.md D1323) Rows of one key that come one after
//! another, found before any is looked up. A table of groups takes a row
//! by its key's hash and a probe of its slots, and a store that keeps
//! rows as they were written holds the rows of one key in a row wherever
//! the data was made so. So each row's key is compared with the key of
//! the row given before it, a grouping column at a time over the
//! column's vector, and a row alike in every column goes to that row's
//! group, as the rows of a run of a grouping by one column of runs do
//! (RULES.md D31), whatever the number of the columns. Only the first
//! row of each such run is looked up.
//!
//! A column tells by its vector where it holds integers, datetimes,
//! the codes of a dictionary or character strings: two rows of one word,
//! or of the same octets, are of one value, and two nulls are of one
//! group [ISO/IEC 9075-2:2023 §7.13 GR2]. Two rows that differ so may
//! still be of one group, as two strings that differ by trailing spaces
//! are; they are two runs, and the table finds them one group. Of any
//! other column no run is told and every row is looked up, as before.

use crate::storage::sel::nth;
use crate::storage::{Batch, Column, Mask, Texts};

/// Where each run of rows alike in every column of `groups` ends among
/// the rows `rows` of `batch`, or all, counted in the order given, the
/// last their count: none of no row, of no column, or of a column that
/// tells no run by its vector.
pub(super) fn ends(batch: &Batch, groups: &[usize], rows: Option<&[u32]>) -> Option<Vec<usize>> {
    let count = rows.map_or(batch.rows, <[u32]>::len);
    if groups.is_empty() || count == 0 {
        return None;
    }
    // Whether the row given at each place begins a run, its key unlike
    // the key of the row before it in some column.
    let mut begins = vec![false; count];
    for &c in groups {
        match batch.columns.get(c)? {
            Column::Ints(ints) => marked((ints.units.as_slice(), ints.nulls.as_ref()), rows, &mut begins)?,
            Column::Datetimes(datetimes) => marked((datetimes.utc.as_slice(), datetimes.nulls.as_ref()), rows, &mut begins)?,
            Column::Texts(texts) => {
                if let Some(coded) = &texts.coded {
                    marked((coded.codes.as_slice(), texts.nulls.as_ref()), rows, &mut begins)?;
                } else {
                    marked_texts(texts, rows, &mut begins)?;
                }
            }
            Column::Values(_) | Column::Floats(_) | Column::Bools(_) | Column::Nulls(_) | Column::Late(_) => return None,
        }
    }
    let mut ends: Vec<usize> = begins.iter().enumerate().skip(1).filter(|&(_, &begun)| begun).map(|(n, _)| n).collect();
    ends.push(count);
    Some(ends)
}

/// Marks in `begins` each row given, of `rows` or all, whose word of
/// `words` is unlike that of the row given before it, a null alike a
/// null alone: none if a row given lies past the words.
fn marked<T: PartialEq>((words, nulls): (&[T], Option<&Mask>), rows: Option<&[u32]>, begins: &mut [bool]) -> Option<()> {
    if rows.is_none() && nulls.is_none() {
        // Every row, none null: each word against the one before it.
        if words.len() != begins.len() {
            return None;
        }
        for (begun, pair) in begins.iter_mut().skip(1).zip(words.windows(2)) {
            *begun |= pair.first() != pair.last();
        }
        return Some(());
    }
    let null = |r: usize| nulls.is_some_and(|nulls| nulls.get(r));
    let mut before: Option<(&T, bool)> = None;
    for (n, begun) in begins.iter_mut().enumerate() {
        let r = nth(rows, n);
        let this = (words.get(r)?, null(r));
        if let Some((word, was_null)) = before {
            let alike = if was_null || this.1 { was_null && this.1 } else { word == this.0 };
            *begun |= !alike;
        }
        before = Some(this);
    }
    Some(())
}

/// As `marked`, of the character strings of `texts`, each held as its
/// own text: a row is unlike the row given before it unless both are
/// null or their octets are the same.
fn marked_texts(texts: &Texts, rows: Option<&[u32]>, begins: &mut [bool]) -> Option<()> {
    if rows.is_none() && texts.nulls.is_none() {
        return marked_octets(texts.text.as_bytes(), &texts.ends, begins);
    }
    let mut before: Option<Option<&str>> = None;
    for (n, begun) in begins.iter_mut().enumerate() {
        let r = nth(rows, n);
        if r >= texts.len() {
            return None;
        }
        // A null gives none, and is alike a null alone.
        let this = texts.get(r);
        if let Some(text) = before {
            *begun |= text != this;
        }
        before = Some(this);
    }
    Some(())
}

/// As `marked_texts`, of every row, none null: each string's octets, the
/// part of `octets` up to its end in `ends`, against those of the string
/// before it, their lengths first, so two empty strings and two strings
/// of unlike lengths are told with no octet read.
fn marked_octets(octets: &[u8], ends: &[usize], begins: &mut [bool]) -> Option<()> {
    if ends.len() != begins.len() {
        return None;
    }
    let (mut from, mut before): (usize, &[u8]) = (0, &[]);
    for (n, (begun, &end)) in begins.iter_mut().zip(ends).enumerate() {
        let this = octets.get(from..end)?;
        if n > 0 {
            *begun |= this.len() != before.len() || (!this.is_empty() && this != before);
        }
        (from, before) = (end, this);
    }
    Some(())
}

// Unit tests: the runs told of each kind of vector, of selected rows and
// of nulls, and none told of a vector that tells none (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
