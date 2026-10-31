//! \[DESIGN\] A column's values gathered by row (RULES.md D31): the rows a
//! join pairs of a batch, in the order paired, a row taken again as often
//! as it is paired and a null where it is paired with none. A vector of
//! integers, numbers or texts is gathered as one of the same kind, so what
//! reads it after a join reads it as from the scan, no value made. Split
//! from storage/batch.rs (RULES.md R-SIZE-2).

use super::{Coded, Column, Floats, Ints, Texts};
use crate::value::Value;

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
        }
    }

    /// A vector of `rows` nulls.
    #[must_use]
    pub fn nulls(rows: usize) -> Column {
        Column::Values(vec![Value::Null; rows])
    }
}

/// Whether the row each of `picks` names is null, as `null` says, or it
/// names none; none if no row is null.
fn nulls(picks: &[Option<usize>], null: impl Fn(usize) -> bool) -> Option<Vec<bool>> {
    let made: Vec<bool> = picks.iter().map(|p| p.is_none_or(&null)).collect();
    made.contains(&true).then_some(made)
}

/// The texts of `texts` at `picks`: of a store's dictionaries, each row's
/// entry, the entries kept; else each text copied into one buffer.
fn texts_gathered(texts: &Texts, picks: &[Option<usize>]) -> Texts {
    let null = nulls(picks, |r| texts.is_null(r) || r >= texts.len());
    if let Some(coded) = &texts.coded {
        let codes = picks.iter().map(|p| p.and_then(|r| coded.codes.get(r)).copied().unwrap_or(0)).collect();
        return Texts::coded(Coded { codes, entries: coded.entries.clone() }, null);
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
