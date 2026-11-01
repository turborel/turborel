//! \[DESIGN\] Batches held by an operator (RULES.md D31, D38): the bytes
//! their vectors hold, near enough to bound a work area by; and many
//! batches' rows joined into one batch, each vector of the kind the
//! batches' vectors at its place all are, so what a join holds of its
//! right operand is gathered into its joined rows as a scan's is
//! (`gathered.rs`). Split from storage/batch.rs (RULES.md R-SIZE-2).

use super::{Batch, Column, Floats, Ints, Texts};
use crate::spill::heap_bytes;
use crate::value::{Decimal, Value};

impl Batch {
    /// The bytes its vectors hold, near enough to bound a work area by.
    pub fn bytes(&self) -> usize {
        self.columns.iter().fold(0_usize, |sum, column| sum.saturating_add(column.bytes()))
    }

    /// The rows of `parts`, each batch's at the places beside it, in
    /// order, as one batch of the first's width: each vector of integers
    /// of one scale, of approximate numbers, of character strings or of
    /// nulls if every part's at its place is, else of values. One part of
    /// all its rows is that batch as it is.
    #[must_use]
    pub fn joined(mut parts: Vec<(Batch, Vec<usize>)>) -> Batch {
        let rows = parts.iter().fold(0_usize, |n, (_, places)| n.saturating_add(places.len()));
        if let [(batch, places)] = parts.as_slice()
            && places.len() == batch.rows
            && places.iter().enumerate().all(|(n, &r)| n == r)
        {
            return parts.pop().map_or_else(|| Batch { columns: Vec::new(), rows: 0 }, |(whole, _)| whole);
        }
        let width = parts.first().map_or(0, |(batch, _)| batch.columns.len());
        let columns = (0..width)
            .map(|at| {
                let of: Vec<(Option<&Column>, &[usize])> = parts.iter().map(|(b, p)| (b.columns.get(at), p.as_slice())).collect();
                joined_column(&of, rows)
            })
            .collect();
        Batch { columns, rows }
    }
}

impl Column {
    /// The bytes the vector holds.
    pub fn bytes(&self) -> usize {
        let nulls = |nulls: &Option<Vec<bool>>| nulls.as_ref().map_or(0, Vec::len);
        match self {
            Column::Values(values) => {
                values.iter().fold(values.len().saturating_mul(size_of::<Value>()), |sum, v| sum.saturating_add(heap_bytes(v)))
            }
            Column::Ints(ints) => ints.units.len().saturating_mul(size_of::<i64>()).saturating_add(nulls(&ints.nulls)),
            Column::Floats(floats) => floats.numbers.len().saturating_mul(size_of::<f64>()).saturating_add(nulls(&floats.nulls)),
            // None held, but as values once they are asked for.
            Column::Nulls(held) => held.rows.saturating_mul(size_of::<Value>()),
            Column::Texts(texts) => {
                let coded = texts.coded.as_ref().map_or(0, |c| {
                    let entries = c.entries.text.len().saturating_add(c.entries.ends.len().saturating_mul(size_of::<usize>()));
                    c.codes.len().saturating_mul(size_of::<u32>()).saturating_add(entries)
                });
                let ends = texts.ends.len().saturating_mul(size_of::<usize>());
                texts.text.len().saturating_add(ends).saturating_add(nulls(&texts.nulls)).saturating_add(coded)
            }
        }
    }

    /// The scale of its integers, if it is a vector of them.
    fn scale(&self) -> Option<u32> {
        match self {
            Column::Ints(ints) => Some(ints.scale),
            Column::Values(_) | Column::Floats(_) | Column::Texts(_) | Column::Nulls(_) => None,
        }
    }

    /// Row `r`'s units, if it is a vector of integers; 0 of a null.
    fn unit_at(&self, r: usize) -> Option<i64> {
        match self {
            Column::Ints(ints) => Some(ints.units.get(r).copied().unwrap_or(0)),
            Column::Values(_) | Column::Floats(_) | Column::Texts(_) | Column::Nulls(_) => None,
        }
    }

    /// Whether row `r`'s value is null, or past the end.
    fn null_at(&self, r: usize) -> bool {
        match self {
            Column::Values(values) => values.get(r).is_none_or(Value::is_null),
            Column::Ints(ints) => r >= ints.units.len() || ints.is_null(r),
            Column::Floats(floats) => floats.get(r).is_none(),
            Column::Texts(texts) => texts.get(r).is_none(),
            Column::Nulls(_) => true,
        }
    }

    /// Row `r`'s value, made; a null past the end.
    fn value_at(&self, r: usize) -> Value {
        match self {
            Column::Values(values) => values.get(r).cloned().unwrap_or(Value::Null),
            Column::Ints(ints) => ints
                .units
                .get(r)
                .filter(|_| !ints.is_null(r))
                .map_or(Value::Null, |&u| Value::Exact(Decimal::new(i128::from(u), ints.scale))),
            Column::Floats(floats) => floats.get(r).map_or(Value::Null, Value::Approx),
            Column::Texts(texts) => texts.get(r).map_or(Value::Null, |s| Value::Char(s.to_owned())),
            Column::Nulls(_) => Value::Null,
        }
    }
}

/// The vector of `rows` rows of each part's column, if it has one, at the
/// places beside it, in order: of the kind every part's is, as
/// `Batch::joined` says.
fn joined_column(parts: &[(Option<&Column>, &[usize])], rows: usize) -> Column {
    let each = || parts.iter().flat_map(|&(column, places)| places.iter().map(move |&r| (column, r)));
    let nulls = || {
        let made: Vec<bool> = each().map(|(c, r)| c.is_none_or(|c| c.null_at(r))).collect();
        made.contains(&true).then_some(made)
    };
    let scales: Vec<Option<u32>> = parts.iter().map(|(c, _)| c.and_then(Column::scale)).collect();
    if let Some(&Some(scale)) = scales.first()
        && scales.iter().all(|&s| s == Some(scale))
    {
        let units = each().map(|(c, r)| c.and_then(|c| c.unit_at(r)).unwrap_or(0)).collect();
        return Column::Ints(Ints::new(units, scale, nulls()));
    }
    let all = |kind: fn(&Column) -> bool| parts.iter().all(|(c, _)| c.is_some_and(kind));
    if all(|c| matches!(c, Column::Floats(_))) {
        let numbers = each().map(|(c, r)| c.and_then(|c| c.value_at(r).as_f64()).unwrap_or(0.0_f64)).collect();
        return Column::Floats(Floats::new(numbers, nulls()));
    }
    if all(|c| matches!(c, Column::Texts(_))) {
        let (mut text, mut ends) = (String::new(), Vec::with_capacity(rows));
        for (c, r) in each() {
            if let Some(Column::Texts(texts)) = c
                && let Some(s) = texts.get(r)
            {
                text.push_str(s);
            }
            ends.push(text.len());
        }
        return Column::Texts(Texts::new(text, ends, nulls()));
    }
    if all(|c| matches!(c, Column::Nulls(_))) {
        return Column::nulls(rows);
    }
    Column::Values(each().map(|(c, r)| c.map_or(Value::Null, |c| c.value_at(r))).collect())
}
