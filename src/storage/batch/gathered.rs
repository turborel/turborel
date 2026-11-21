//! \[DESIGN\] A column's values gathered by row (RULES.md D31): the rows a
//! join pairs of a batch, in the order paired, a row taken again as often
//! as it is paired and a null where it is paired with none. A vector of
//! integers, numbers or texts is gathered as one of the same kind, so what
//! reads it after a join reads it as from the scan, no value made. Picks
//! given as row numbers (`Picks`) gather integers on the vector kernels
//! (RULES.md D570).

use std::cell::OnceCell;

use super::{Coded, Column, Floats, Ints, Mask, Texts};
use crate::kernels::Kernels;
use crate::storage::mask::index;
use crate::value::Value;

/// A pick of no row, whose values are nulls (`Picks`).
pub const NO_ROW: u32 = u32::MAX;

/// \[DESIGN\] The rows a gathering picks, each a row's number or `NO_ROW`,
/// made as options once, the first time a vector not of integers is
/// gathered of them.
pub struct Picks<'a> {
    rows: &'a [u32],
    options: OnceCell<Vec<Option<usize>>>,
}

impl<'a> Picks<'a> {
    /// The picks `rows`.
    pub fn new(rows: &'a [u32]) -> Picks<'a> {
        Picks { rows, options: OnceCell::new() }
    }

    /// Each pick a row or none.
    fn options(&self) -> &[Option<usize>] {
        self.options.get_or_init(|| self.rows.iter().map(|&r| (r != NO_ROW).then(|| index(r))).collect())
    }
}

impl Column {
    /// The values of the rows at `picks`, in order, a null for none: a
    /// vector of the same kind.
    #[must_use]
    pub fn gathered(&self, picks: &[Option<usize>]) -> Column {
        match self {
            Column::Values(values) => {
                Column::Values(picks.iter().map(|p| p.and_then(|r| values.get(r)).cloned().unwrap_or(Value::Null)).collect())
            }
            Column::Ints(ints) => {
                let units = picks.iter().map(|p| p.and_then(|r| ints.units.get(r)).copied().unwrap_or(0)).collect();
                Column::Ints(Ints::new(units, ints.scale, nulls(picks, |r| ints.is_null(r))))
            }
            Column::Floats(floats) => {
                let numbers = picks.iter().map(|p| p.and_then(|r| floats.numbers.get(r)).copied().unwrap_or(0.0_f64)).collect();
                Column::Floats(Floats::new(numbers, nulls(picks, |r| floats.is_null(r))))
            }
            Column::Texts(texts) => Column::Texts(texts_gathered(texts, picks)),
            Column::Datetimes(datetimes) => Column::Datetimes(datetimes.gathered(picks)),
            Column::Bools(bools) => Column::Bools(bools.gathered(picks)),
            Column::Nulls(_) => Column::nulls(picks.len()),
            Column::Late(late) => Column::Late(late.gathered(picks)),
        }
    }

    /// The values of the rows `picks` picks, in order, as `gathered`:
    /// integers gathered by `kernels`, a pick of no row or past the end
    /// read as 0, its null marked as `gathered` marks it.
    #[must_use]
    pub fn gathered_by(&self, picks: &Picks<'_>, kernels: &dyn Kernels) -> Column {
        if let Column::Ints(ints) = self
            && u32::try_from(ints.units.len()).is_ok_and(|n| n < NO_ROW)
        {
            let mut units = Vec::with_capacity(picks.rows.len());
            kernels.gather_i64(&ints.units, picks.rows, &mut units);
            let mut made = Mask::none(picks.rows.len());
            for (n, &r) in picks.rows.iter().enumerate() {
                if r == NO_ROW || ints.is_null(index(r)) {
                    made.set(n);
                }
            }
            return Column::Ints(Ints::new(units, ints.scale, made.any().then_some(made)));
        }
        self.gathered(picks.options())
    }

    /// A vector of `rows` nulls.
    #[must_use]
    pub fn nulls(rows: usize) -> Column {
        Column::Nulls(super::Nulls::new(rows))
    }
}

/// Whether the row each of `picks` names is null, as `null` says, or it
/// names none; none if no row is null.
pub(super) fn nulls(picks: &[Option<usize>], null: impl Fn(usize) -> bool) -> Option<Mask> {
    let mut made = Mask::none(picks.len());
    for (n, p) in picks.iter().enumerate() {
        if p.is_none_or(&null) {
            made.set(n);
        }
    }
    made.any().then_some(made)
}

/// The texts of `texts` at `picks`: of a store's dictionaries, each row's
/// entry, the entries kept; else each text copied into one buffer.
fn texts_gathered(texts: &Texts, picks: &[Option<usize>]) -> Texts {
    let null = nulls(picks, |r| texts.is_null(r) || r >= texts.len());
    if let Some(coded) = &texts.coded {
        let codes = picks.iter().map(|p| p.and_then(|r| coded.codes.get(r)).copied().unwrap_or(0)).collect();
        return Texts::coded(Coded { codes, entries: std::sync::Arc::clone(&coded.entries) }, null);
    }
    let (mut text, mut ends) = (String::new(), Vec::with_capacity(picks.len()));
    for p in picks {
        if let Some(s) = p.and_then(|r| texts.get(r)) {
            text.push_str(s);
        }
        ends.push(text.len());
    }
    Texts::new(text, ends, null)
}

// Unit tests: integers gathered by the kernels as by options (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests {
    use super::{Column, Ints, NO_ROW, Picks};
    use crate::kernels::Portable;

    /// Verifies: [DESIGN] RULES.md D570 integers gathered by row numbers on
    /// the kernels are those gathered by options: a row's null kept, a
    /// pick of no row null, a row past the end 0 and not null.
    #[test]
    fn integers_gathered_by_the_kernels_keep_their_nulls() {
        let ints = Column::Ints(Ints::new(vec![4, 9, -2], 0, Some(vec![false, true, false].into())));
        let rows = [2_u32, NO_ROW, 1, 0, 7];
        let options = [Some(2), None, Some(1), Some(0), Some(7)];
        let by_kernels = ints.gathered_by(&Picks::new(&rows), &Portable);
        assert_eq!(format!("{:?}", by_kernels.values()), format!("{:?}", ints.gathered(&options).values()));
    }
}
