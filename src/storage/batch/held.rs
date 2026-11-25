//! \[DESIGN\] Batches held by an operator (RULES.md D31, D38): the bytes
//! their vectors hold, near enough to bound a work area by; and many
//! batches' rows joined into one batch, each vector of the kind the
//! batches' vectors at its place all are (`joined.rs`), so what a join
//! holds of its right operand is gathered into its joined rows as a scan's
//! is (`gathered.rs`).

use super::{Batch, Column};
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
                super::joined::joined_column(&of, rows)
            })
            .collect();
        Batch { columns, rows }
    }

    /// \[DESIGN\] (RULES.md D1080) The vector at place `at` of the rows of
    /// `parts`, every row of each, in order, as `joined` makes it: one
    /// vector of a batch joined of the parts, which threads make at once,
    /// each its own.
    #[must_use]
    pub fn joined_at(parts: &[Batch], at: usize) -> Column {
        let rows = parts.iter().fold(0_usize, |n, part| n.saturating_add(part.rows));
        let all: Vec<usize> = (0..parts.iter().map(|part| part.rows).max().unwrap_or(0)).collect();
        let of: Vec<(Option<&Column>, &[usize])> =
            parts.iter().map(|part| (part.columns.get(at), all.get(..part.rows).unwrap_or_default())).collect();
        super::joined::joined_column(&of, rows)
    }
}

impl Column {
    /// The bytes the vector holds.
    pub fn bytes(&self) -> usize {
        let nulls = |nulls: Option<&super::Mask>| nulls.map_or(0, |m| m.words().len().saturating_mul(size_of::<u64>()));
        match self {
            Column::Values(values) => {
                values.iter().fold(values.len().saturating_mul(size_of::<Value>()), |sum, v| sum.saturating_add(heap_bytes(v)))
            }
            Column::Ints(ints) => ints.units.len().saturating_mul(size_of::<i64>()).saturating_add(nulls(ints.nulls.as_ref())),
            Column::Floats(floats) => floats.numbers.len().saturating_mul(size_of::<f64>()).saturating_add(nulls(floats.nulls.as_ref())),
            Column::Datetimes(d) => {
                let zones = d.zones.as_ref().map_or(0, |z| z.len().saturating_mul(size_of::<i16>()));
                d.utc.len().saturating_mul(size_of::<i64>()).saturating_add(zones).saturating_add(nulls(d.nulls.as_ref()))
            }
            Column::Bools(b) => nulls(Some(&b.bits)).saturating_add(nulls(b.nulls.as_ref())),
            // None held, but as values once they are asked for.
            Column::Nulls(held) => held.rows.saturating_mul(size_of::<Value>()),
            Column::Late(late) => late.bytes(),
            Column::Texts(texts) => {
                let coded = texts.coded.as_ref().map_or(0, |c| {
                    let entries = c.entries.text.len().saturating_add(c.entries.ends.len().saturating_mul(size_of::<usize>()));
                    c.codes.len().saturating_mul(size_of::<u32>()).saturating_add(entries)
                });
                let ends = texts.ends.len().saturating_mul(size_of::<usize>());
                texts.text.len().saturating_add(ends).saturating_add(nulls(texts.nulls.as_ref())).saturating_add(coded)
            }
        }
    }

    /// The scale of its integers, if it is a vector of them.
    pub(super) fn scale(&self) -> Option<u32> {
        match self {
            Column::Ints(ints) => Some(ints.scale),
            Column::Values(_)
            | Column::Floats(_)
            | Column::Texts(_)
            | Column::Datetimes(_)
            | Column::Bools(_)
            | Column::Nulls(_)
            | Column::Late(_) => None,
        }
    }

    /// Whether row `r`'s value is null, or past the end.
    pub(super) fn null_at(&self, r: usize) -> bool {
        match self {
            Column::Values(values) => values.get(r).is_none_or(Value::is_null),
            Column::Ints(ints) => r >= ints.units.len() || ints.is_null(r),
            Column::Floats(floats) => floats.get(r).is_none(),
            Column::Texts(texts) => texts.get(r).is_none(),
            Column::Datetimes(d) => d.get(r).is_none(),
            Column::Bools(b) => b.get(r).is_none(),
            Column::Nulls(_) => true,
            Column::Late(late) => late.made().is_none_or(|made| made.null_at(r)),
        }
    }

    /// Row `r`'s value, made; a null past the end.
    pub(super) fn value_at(&self, r: usize) -> Value {
        match self {
            Column::Values(values) => values.get(r).cloned().unwrap_or(Value::Null),
            Column::Ints(ints) => ints
                .units
                .get(r)
                .filter(|_| !ints.is_null(r))
                .map_or(Value::Null, |&u| Value::Exact(Decimal::new(i128::from(u), ints.scale))),
            Column::Floats(floats) => floats.get(r).map_or(Value::Null, Value::Approx),
            Column::Texts(texts) => texts.get(r).map_or(Value::Null, |s| Value::Char(s.into())),
            Column::Datetimes(d) => d.get(r).map_or(Value::Null, Value::Datetime),
            Column::Bools(b) => b.get(r).map_or(Value::Null, Value::Bool),
            Column::Nulls(_) => Value::Null,
            Column::Late(late) => late.values().get(r).cloned().unwrap_or(Value::Null),
        }
    }
}
