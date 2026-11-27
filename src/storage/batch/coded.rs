//! \[DESIGN\] (RULES.md D1099) Character strings kept as codes through a
//! join. A join gathered a vector of texts at its pairs by reading each
//! row's text where it lay and copying it, and put the parts of its built
//! operand together the same way, so a store's dictionary codes were
//! decoded into texts at the first join, and every join above read a text
//! of each row at random, a row's end and its bytes, from a vector that
//! could hold millions.
//!
//! A vector of texts of a dictionary keeps it now. The parts of a built
//! operand whose vectors are codes of one dictionary make a vector of
//! their codes (`of_codes`). A join's built operand of no more rows than a
//! batch's, `MOST_ENTRIES`, makes each vector of texts a dictionary once
//! (`dictionaries`), and a join gathers that vector as codes of it, a
//! built row's code its row (`Column::gathered_coded`). Every reader of a
//! batch's vectors already reads a store's codes, judging, hashing and
//! comparing each entry once, never two codes as unequal texts, so
//! entries that repeat a text are read as a store's are.

use std::sync::Arc;

use super::mask::index;
use super::{BATCH_ROWS, Batch, Coded, Column, Mask, NO_ROW, Picks, Texts};
use crate::kernels::Kernels;

/// \[DESIGN\] (RULES.md D1099) Of a built operand of no more rows than a
/// batch holds, `BATCH_ROWS`, a vector of texts is made a dictionary,
/// derived, not tuned. A reader of codes judges, hashes or compares each
/// entry once a batch, so entries no more than a batch's rows cost it no
/// more than its rows would. A batch of codes is charged its entries with
/// its codes (`Column::bytes`), entries no more than a full batch of the
/// same texts, so the charge is at most that batch's and four bytes a row.
const MOST_ENTRIES: usize = BATCH_ROWS;

/// Of each vector of `batch`, a join's built operand: of texts no
/// dictionary codes, of no more than `MOST_ENTRIES` rows, its texts as a
/// dictionary, each row an entry; none of any other.
pub fn dictionaries(batch: &Batch) -> Vec<Option<Arc<Texts>>> {
    let small = |t: &Texts| t.len() <= MOST_ENTRIES;
    let of = |c: &Column| match c {
        Column::Texts(t) if t.coded.is_none() && small(t) => Some(Arc::new(Texts::new(t.text.clone(), t.ends.clone(), None))),
        Column::Texts(_)
        | Column::Ints(_)
        | Column::Floats(_)
        | Column::Datetimes(_)
        | Column::Bools(_)
        | Column::Values(_)
        | Column::Nulls(_)
        | Column::Late(_) => None,
    };
    batch.columns.iter().map(of).collect()
}

impl Column {
    /// \[DESIGN\] (RULES.md D1099) The rows `picks` picks, as
    /// `gathered_by`; of a vector of texts whose dictionary is `entries`,
    /// itself (`dictionaries`), the codes of those rows, each its row, a
    /// null for a pick of none, past the end or of a null.
    #[must_use]
    pub fn gathered_coded(&self, picks: &Picks<'_>, entries: Option<&Arc<Texts>>, kernels: &dyn Kernels) -> Column {
        let (Column::Texts(texts), Some(entries)) = (self, entries) else { return self.gathered_by(picks, kernels) };
        let len = texts.len();
        let mut null = Mask::none(picks.rows.len());
        let codes: Vec<u32> = picks
            .rows
            .iter()
            .enumerate()
            .map(|(n, &r)| {
                let none = r == NO_ROW || index(r) >= len || texts.is_null(index(r));
                if none {
                    null.set(n);
                }
                if none { 0 } else { r }
            })
            .collect();
        Column::Texts(Texts::coded(Coded { codes, entries: Arc::clone(entries) }, null.any().then_some(null)))
    }
}

/// \[DESIGN\] (RULES.md D1099) Of parts each a vector of codes of one
/// dictionary, the same for all, the vector of their codes at their
/// places, in order, `nulls` its nulls; none if a part's vector is not.
pub(super) fn of_codes(parts: &[(Option<&Column>, &[usize])], rows: usize, nulls: impl FnOnce() -> Option<Mask>) -> Option<Column> {
    let first = dictionary_of(parts.first()?.0)?;
    if !parts.iter().all(|(c, _)| dictionary_of(*c).is_some_and(|k| Arc::ptr_eq(&k.entries, &first.entries))) {
        return None;
    }
    let mut codes = Vec::with_capacity(rows);
    for &(column, places) in parts {
        let held = dictionary_of(column).map_or(&[][..], |k| k.codes.as_slice());
        codes.extend(places.iter().map(|&r| held.get(r).copied().unwrap_or(0)));
    }
    Some(Column::Texts(Texts::coded(Coded { codes, entries: Arc::clone(&first.entries) }, nulls())))
}

/// The codes of `column`, if it is a vector of texts of a dictionary.
fn dictionary_of(column: Option<&Column>) -> Option<&Coded> {
    if let Some(Column::Texts(t)) = column { t.coded.as_deref() } else { None }
}

// Unit tests: texts gathered as codes and parts' codes kept (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::Portable;

    /// The values of `column` as text, and whether it is codes.
    fn shown(column: &Column) -> (Vec<String>, bool) {
        let coded = matches!(column, Column::Texts(t) if t.coded.is_some());
        (column.values().iter().map(ToString::to_string).collect(), coded)
    }

    /// Verifies: [DESIGN] RULES.md D1099: a vector of texts not coded of
    /// no more rows than a batch's is made a dictionary, one of a row more,
    /// a coded one and one of integers not; gathered of it, its rows are codes whose values are those a
    /// gather of texts gives, a pick of none, past the end or of a null a
    /// null; and without a dictionary the gather is as before.
    #[test]
    fn small_built_texts_are_gathered_as_codes() {
        let names = Texts::new("ab cde".to_owned(), vec![2, 3, 3, 6], Some(Mask::from_bools(&[false, false, true, false])));
        let wide = Texts::new(String::new(), vec![0; MOST_ENTRIES + 1], None);
        let coded = Texts::coded(Coded { codes: vec![0], entries: Arc::new(Texts::new("q".to_owned(), vec![1], None)) }, None);
        let batch = Batch {
            columns: vec![
                Column::Texts(names),
                Column::Texts(wide),
                Column::Texts(coded),
                Column::Ints(super::super::Ints::new(vec![1], 0, None)),
            ],
            rows: 4,
        };
        let made = dictionaries(&batch);
        assert_eq!(made.iter().map(Option::is_some).collect::<Vec<_>>(), [true, false, false, false]);
        let rows = [3_u32, NO_ROW, 0, 2, 9, 3, 1];
        let picks = Picks::new(&rows);
        let as_codes = batch.columns[0].gathered_coded(&picks, made[0].as_ref(), &Portable);
        let as_texts = batch.columns[0].gathered_by(&picks, &Portable);
        assert_eq!(shown(&as_codes), (shown(&as_texts).0, true));
        assert_eq!(shown(&as_codes).0, ["cde", "NULL", "ab", "NULL", "NULL", "cde", " "]);
        assert!(!shown(&batch.columns[1].gathered_coded(&picks, None, &Portable)).1, "no dictionary, gathered as texts");
    }

    /// Verifies: [DESIGN] RULES.md D1099: parts whose vectors are codes of
    /// one dictionary are put together as their codes at their places, a
    /// part's null kept; parts of two dictionaries, or one of texts, are
    /// put together as texts, their values alike.
    #[test]
    fn parts_of_one_dictionary_keep_their_codes() {
        let entries = Arc::new(Texts::new("redblue".to_owned(), vec![3, 7], None));
        let part =
            |codes: Vec<u32>, nulls: Option<Mask>| Column::Texts(Texts::coded(Coded { codes, entries: Arc::clone(&entries) }, nulls));
        let (a, b) = (part(vec![0, 1, 1], None), part(vec![1, 0], Some(Mask::from_bools(&[true, false]))));
        let joined = Batch::joined(vec![
            (Batch { columns: vec![a.clone()], rows: 3 }, vec![2, 0]),
            (Batch { columns: vec![b], rows: 2 }, vec![0, 1]),
        ]);
        assert_eq!(shown(&joined.columns[0]), (vec!["blue".into(), "red".into(), "NULL".into(), "red".into()], true));
        let other =
            Column::Texts(Texts::coded(Coded { codes: vec![0], entries: Arc::new(Texts::new("red".to_owned(), vec![3], None)) }, None));
        let mixed = Batch::joined(vec![(Batch { columns: vec![a], rows: 3 }, vec![1]), (Batch { columns: vec![other], rows: 1 }, vec![0])]);
        assert_eq!(shown(&mixed.columns[0]), (vec!["blue".into(), "red".into()], false));
    }
}
