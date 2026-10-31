//! \[DESIGN\] Rows read a batch at a time (`TableStorage::scan_batches`,
//! RULES.md D31): each column asked for a vector of its values, all of one
//! length, so work over many rows needs no row made for each. A column's
//! vector is of values, or, of an exact number a store keeps as an
//! integer, of those integers, its nulls beside them; or, of an
//! approximate number, of those numbers; or, of a character
//! string a store keeps as written, of its bytes, one buffer of them all
//! and where each ends: work that reads them as integers or as text reads
//! no value made; any other reads them as values, made the first time it
//! asks. Split from storage.rs (RULES.md R-SIZE-2).

mod gathered;

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
    /// Approximate numbers, as `f64`s.
    Floats(Floats),
    /// Character strings, as their text.
    Texts(Texts),
}

/// \[DESIGN\] Character strings as read, of a VARCHAR column as written, of
/// a CHARACTER one padded to its length, as its values are and as LIKE
/// sees them: their text in one buffer, and where each ends in it; or, of
/// a store's dictionaries, each row's entry among theirs (`Coded`), so a
/// condition is judged once of each entry, not of each row.
#[derive(Debug, Clone, Default)]
pub struct Texts {
    /// Every row's text, one after another, a null's empty.
    pub text: String,
    /// Where each row's text ends in `text`.
    pub ends: Vec<usize>,
    /// Whether each row's is null, if any is.
    pub nulls: Option<Vec<bool>>,
    /// Of a store's dictionaries, each row's entry, and the entries, if the
    /// rows are so: `text` and `ends` are then empty.
    pub coded: Option<Box<Coded>>,
    /// The values, made the first time they are asked for.
    made: OnceLock<Vec<Value>>,
}

/// \[DESIGN\] Character strings of a store's dictionaries: of each row,
/// its entry's place among `entries`, every dictionary's one after
/// another.
#[derive(Debug, Clone, Default)]
pub struct Coded {
    /// Each row's entry, a null's 0.
    pub codes: Vec<u32>,
    /// The entries.
    pub entries: Texts,
}

impl Texts {
    /// The texts of `text` ending at `ends`, null where `nulls` says.
    pub fn new(text: String, ends: Vec<usize>, nulls: Option<Vec<bool>>) -> Texts {
        Texts { text, ends, nulls, coded: None, made: OnceLock::new() }
    }

    /// The texts of `coded`'s entries, each row's as its code says, null
    /// where `nulls` says.
    pub fn coded(coded: Coded, nulls: Option<Vec<bool>>) -> Texts {
        Texts { text: String::new(), ends: Vec::new(), nulls, coded: Some(Box::new(coded)), made: OnceLock::new() }
    }

    /// Whether row `n`'s is null.
    pub fn is_null(&self, n: usize) -> bool {
        self.nulls.as_ref().is_some_and(|nulls| nulls.get(n).copied().unwrap_or(false))
    }

    /// Row `n`'s text, none if it is null or past the end.
    pub fn get(&self, n: usize) -> Option<&str> {
        if self.is_null(n) {
            return None;
        }
        if let Some(coded) = &self.coded {
            return coded.entries.get(usize::try_from(*coded.codes.get(n)?).ok()?);
        }
        let from = n.checked_sub(1).map_or(Some(0), |p| self.ends.get(p).copied())?;
        self.text.get(from..*self.ends.get(n)?)
    }

    /// How many rows.
    pub fn len(&self) -> usize {
        self.coded.as_ref().map_or(self.ends.len(), |coded| coded.codes.len())
    }

    /// Whether it has no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The values, made the first time they are asked for.
    pub fn values(&self) -> &[Value] {
        self.made.get_or_init(|| (0..self.len()).map(|n| self.get(n).map_or(Value::Null, |s| Value::Char(s.to_owned()))).collect())
    }

    /// Its values, owned.
    fn into_values(self) -> Vec<Value> {
        self.values();
        self.made.into_inner().unwrap_or_default()
    }
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

/// \[DESIGN\] Approximate numbers [X3.135-1986 §4.2.2]: a column of an
/// approximate numeric type, as a store keeps it.
#[derive(Debug, Clone, Default)]
pub struct Floats {
    /// Each row's number, a null's 0.
    pub numbers: Vec<f64>,
    /// Whether each row's is null, if any is.
    pub nulls: Option<Vec<bool>>,
    /// The values, made the first time they are asked for.
    made: OnceLock<Vec<Value>>,
}

impl Floats {
    /// The approximate numbers `numbers`, null where `nulls` says.
    pub fn new(numbers: Vec<f64>, nulls: Option<Vec<bool>>) -> Floats {
        Floats { numbers, nulls, made: OnceLock::new() }
    }

    /// Whether row `n`'s is null.
    pub fn is_null(&self, n: usize) -> bool {
        self.nulls.as_ref().is_some_and(|nulls| nulls.get(n).copied().unwrap_or(false))
    }

    /// Row `n`'s number, none if it is null or past the end.
    pub fn get(&self, n: usize) -> Option<f64> {
        self.numbers.get(n).copied().filter(|_| !self.is_null(n))
    }

    /// The values, made the first time they are asked for.
    pub fn values(&self) -> &[Value] {
        self.made.get_or_init(|| (0..self.numbers.len()).map(|n| self.get(n).map_or(Value::Null, Value::Approx)).collect())
    }

    /// Its values, owned.
    fn into_values(self) -> Vec<Value> {
        self.values();
        self.made.into_inner().unwrap_or_default()
    }
}

impl Column {
    /// Its values, made the first time they are asked for if it is not a
    /// vector of values.
    pub fn values(&self) -> &[Value] {
        match self {
            Column::Values(values) => values,
            Column::Ints(ints) => ints.values(),
            Column::Floats(floats) => floats.values(),
            Column::Texts(texts) => texts.values(),
        }
    }

    /// Its values, to change: of integers, numbers or texts, made, it then
    /// of values.
    pub fn values_mut(&mut self) -> &mut Vec<Value> {
        match self {
            Column::Values(values) => values,
            Column::Ints(ints) => {
                let made = std::mem::take(ints).into_values();
                *self = Column::Values(made);
                self.values_mut()
            }
            Column::Floats(floats) => {
                let made = std::mem::take(floats).into_values();
                *self = Column::Values(made);
                self.values_mut()
            }
            Column::Texts(texts) => {
                let made = std::mem::take(texts).into_values();
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
            Column::Floats(floats) => floats.into_values(),
            Column::Texts(texts) => texts.into_values(),
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
/// place; of one of integers, its units and scale; of one of approximate
/// numbers, its number; of one of texts, its text; or a null.
#[derive(Debug, Clone, Copy)]
pub enum Cell<'b> {
    /// A value of a vector of values.
    Value(&'b Value),
    /// An exact number's units, and its scale.
    Exact(i64, u32),
    /// An approximate number.
    Approx(f64),
    /// A character string's text.
    Text(&'b str),
    /// A null of a vector of integers, numbers or texts, or past the end of
    /// a vector.
    Null,
}

impl Cell<'_> {
    /// Whether it is null.
    pub fn is_null(&self) -> bool {
        match self {
            Cell::Value(v) => v.is_null(),
            Cell::Exact(..) | Cell::Approx(_) | Cell::Text(_) => false,
            Cell::Null => true,
        }
    }

    /// Its value, made.
    pub fn to_value(&self) -> Value {
        match self {
            Cell::Value(v) => (*v).clone(),
            Cell::Exact(units, scale) => Value::Exact(Decimal::new(i128::from(*units), *scale)),
            Cell::Approx(f) => Value::Approx(*f),
            Cell::Text(text) => Value::Char((*text).to_owned()),
            Cell::Null => Value::Null,
        }
    }
}

impl Batch {
    /// The value of column `at` of row `r`, read in place: none made of a
    /// vector of integers, numbers or texts.
    pub fn cell(&self, at: usize, r: usize) -> Cell<'_> {
        match self.columns.get(at) {
            Some(Column::Values(values)) => values.get(r).map_or(Cell::Null, Cell::Value),
            Some(Column::Ints(ints)) => ints.units.get(r).filter(|_| !ints.is_null(r)).map_or(Cell::Null, |&u| Cell::Exact(u, ints.scale)),
            Some(Column::Floats(floats)) => floats.get(r).map_or(Cell::Null, Cell::Approx),
            Some(Column::Texts(texts)) => texts.get(r).map_or(Cell::Null, Cell::Text),
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

// Unit tests: columns of values, of integers and of approximate numbers,
// and their cells (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
