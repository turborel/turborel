//! \[DESIGN\] Rows read a batch at a time (`TableStorage::scan_batches`,
//! RULES.md D31): each column asked for a vector of its values, all of one
//! length, so work over many rows needs no row made for each. A column's
//! vector is of values, or, of an exact number a store keeps as an
//! integer, of those integers, its nulls beside them: work that reads them
//! as integers reads no value made; any other reads them as values, made
//! the first time it asks. Split from storage.rs (RULES.md R-SIZE-2).

use std::sync::OnceLock;

use super::{Row, Rows};
use crate::error::Result;
use crate::value::{Decimal, Value};

/// \[DESIGN\] The most rows of a batch: a vector, 2048.
pub const BATCH_ROWS: usize = 2048;

/// \[DESIGN\] Rows of a scan, by column: the values of each column asked
/// for, in the order asked, a vector of `rows` of them.
#[derive(Debug, Default)]
pub struct Batch {
    /// Each column's values.
    pub columns: Vec<Column>,
    /// How many rows.
    pub rows: usize,
}

/// \[DESIGN\] A column's values in a batch.
#[derive(Debug, Clone)]
pub enum Column {
    /// The values.
    Values(Vec<Value>),
    /// Exact numbers, as integers.
    Ints(Ints),
}

/// \[DESIGN\] Exact numbers of one scale, each its units at that scale
/// [X3.135-1986 §4.2.2]: a column of an exact numeric type, as a store
/// keeps it.
#[derive(Debug, Clone)]
pub struct Ints {
    /// Each row's units, a null's 0.
    pub units: Vec<i64>,
    /// The digits of each after the decimal point.
    pub scale: u32,
    /// Whether each row's is null, if any is.
    pub nulls: Option<Vec<bool>>,
    /// The values, made the first time they are asked for.
    made: OnceLock<Vec<Value>>,
}

impl Ints {
    /// Exact numbers of `scale`, each of `units`, null where `nulls` says.
    pub fn new(units: Vec<i64>, scale: u32, nulls: Option<Vec<bool>>) -> Ints {
        Ints { units, scale, nulls, made: OnceLock::new() }
    }

    /// Whether row `n`'s is null.
    pub fn is_null(&self, n: usize) -> bool {
        self.nulls.as_ref().is_some_and(|nulls| nulls.get(n).copied().unwrap_or(false))
    }

    /// The values, made the first time they are asked for.
    pub fn values(&self) -> &[Value] {
        self.made.get_or_init(|| {
            (0..self.units.len())
                .map(|n| {
                    if self.is_null(n) {
                        Value::Null
                    } else {
                        Value::Exact(Decimal::new(i128::from(self.units.get(n).copied().unwrap_or(0)), self.scale))
                    }
                })
                .collect()
        })
    }
}

impl Column {
    /// Its values, made the first time they are asked for if they are
    /// integers.
    pub fn values(&self) -> &[Value] {
        match self {
            Column::Values(values) => values,
            Column::Ints(ints) => ints.values(),
        }
    }

    /// Its values, to change: of integers, made, it then of values.
    pub fn values_mut(&mut self) -> &mut Vec<Value> {
        match self {
            Column::Values(values) => values,
            Column::Ints(ints) => {
                let made = std::mem::take(ints).into_values();
                *self = Column::Values(made);
                self.values_mut()
            }
        }
    }

    /// Its values, owned.
    pub fn into_values(self) -> Vec<Value> {
        match self {
            Column::Values(values) => values,
            Column::Ints(ints) => ints.into_values(),
        }
    }
}

impl Default for Ints {
    fn default() -> Ints {
        Ints::new(Vec::new(), 0, None)
    }
}

impl Ints {
    /// Its values, owned.
    fn into_values(self) -> Vec<Value> {
        self.values();
        self.made.into_inner().unwrap_or_default()
    }
}

impl Default for Column {
    fn default() -> Column {
        Column::Values(Vec::new())
    }
}

impl From<Vec<Value>> for Column {
    fn from(values: Vec<Value>) -> Column {
        Column::Values(values)
    }
}

impl Batch {
    /// A batch of no rows of `columns` columns, room made for a whole one.
    pub fn empty(columns: usize) -> Batch {
        Batch { columns: (0..columns).map(|_| Column::Values(Vec::with_capacity(BATCH_ROWS))).collect(), rows: 0 }
    }

    /// The values of column `at`, none if it has none.
    pub fn values(&self, at: usize) -> &[Value] {
        self.columns.get(at).map_or(&[][..], Column::values)
    }

    /// The value of column `at` of row `r`, if there is one.
    pub fn value(&self, at: usize, r: usize) -> Option<&Value> {
        self.values(at).get(r)
    }

    /// Adds `row`'s values of `columns`, as a table's row holds them.
    pub fn push(&mut self, row: &Row, columns: &[usize]) {
        for (vector, &c) in self.columns.iter_mut().zip(columns) {
            vector.values_mut().push(row.get(c).cloned().unwrap_or(Value::Null));
        }
        self.rows = self.rows.saturating_add(1);
    }
}

/// \[DESIGN\] One value of a batch: of a vector of values, read in
/// place; of one of integers, its units and scale; or a null.
#[derive(Debug, Clone, Copy)]
pub enum Cell<'b> {
    /// A value of a vector of values.
    Value(&'b Value),
    /// An exact number's units, and its scale.
    Exact(i64, u32),
    /// A null of a vector of integers, or past the end of a vector.
    Null,
}

impl Cell<'_> {
    /// Whether it is null.
    pub fn is_null(&self) -> bool {
        match self {
            Cell::Value(v) => v.is_null(),
            Cell::Exact(..) => false,
            Cell::Null => true,
        }
    }

    /// Its value, made.
    pub fn to_value(&self) -> Value {
        match self {
            Cell::Value(v) => (*v).clone(),
            Cell::Exact(units, scale) => Value::Exact(Decimal::new(i128::from(*units), *scale)),
            Cell::Null => Value::Null,
        }
    }
}

impl Batch {
    /// The value of column `at` of row `r`, read in place: none made of a
    /// vector of integers.
    pub fn cell(&self, at: usize, r: usize) -> Cell<'_> {
        match self.columns.get(at) {
            Some(Column::Values(values)) => values.get(r).map_or(Cell::Null, Cell::Value),
            Some(Column::Ints(ints)) => ints.units.get(r).filter(|_| !ints.is_null(r)).map_or(Cell::Null, |&u| Cell::Exact(u, ints.scale)),
            None => Cell::Null,
        }
    }
}

/// \[DESIGN\] Batches of a scan's rows.
pub type Batches = Box<dyn Iterator<Item = Result<Batch>> + Send>;

/// `rows`, by batch, of their values of `columns`.
pub fn batches_of(rows: Rows, columns: &[usize]) -> Batches {
    let columns = columns.to_vec();
    let mut rows = rows;
    Box::new(std::iter::from_fn(move || {
        let mut batch = Batch::empty(columns.len());
        while batch.rows < BATCH_ROWS {
            match rows.next() {
                Some(Ok((_, row))) => batch.push(&row, &columns),
                Some(Err(e)) => return Some(Err(e)),
                None => break,
            }
        }
        (batch.rows > 0).then_some(Ok(batch))
    }))
}

// Unit tests: columns of values and of integers, and their cells
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
