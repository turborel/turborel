//! \[DESIGN\] The vectors expression nodes give and take (RULES.md D362):
//! a batch's column read in place, one made, or a constant; every one of
//! the rows its node is evaluated of, in order. A column of a batch of
//! which a filter kept some rows is gathered first, those rows alone, of
//! the same kind, so every node reads its operands densely; integers
//! known as runs stay known as the runs of the rows gathered.

use crate::storage::mask::index;
use crate::storage::{Bools, Column, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::{Datetime, Value};

/// An expression's values of the rows it is evaluated of.
pub(super) enum Vector<'a> {
    /// A column of a batch, or of the rows a filter kept of it, gathered.
    Column(&'a Column),
    /// A column made.
    Owned(Column),
    /// A constant, every row's.
    Const(Value),
}

/// The values of a vector, read one by one, as a row's evaluation sees
/// them: a column's made the first time they are asked for.
pub(super) enum Seq<'v> {
    Values(&'v [Value]),
    Const(&'v Value),
}

/// The null a sequence gives past its values, which none of its rows asks.
static NULL: Value = Value::Null;

impl Seq<'_> {
    /// The value of the `n`th row.
    pub(super) fn get(&self, n: usize) -> &Value {
        match self {
            Seq::Values(values) => values.get(n).unwrap_or(&NULL),
            Seq::Const(v) => v,
        }
    }
}

impl Vector<'_> {
    /// Its column, if it is not a constant.
    pub(super) fn column(&self) -> Option<&Column> {
        match self {
            Vector::Column(c) => Some(c),
            Vector::Owned(c) => Some(c),
            Vector::Const(_) => None,
        }
    }

    /// Its values, one a row.
    pub(super) fn seq(&self) -> Seq<'_> {
        match self {
            Vector::Column(c) => Seq::Values(c.values()),
            Vector::Owned(c) => Seq::Values(c.values()),
            Vector::Const(v) => Seq::Const(v),
        }
    }

    /// Whether it is the null constant.
    pub(super) fn is_null_const(&self) -> bool {
        matches!(self, Vector::Const(Value::Null))
    }

    /// Its values as a column of `count` rows, owned.
    pub(super) fn into_column(self, count: usize) -> Column {
        match self {
            Vector::Column(c) => c.clone(),
            Vector::Owned(c) => c,
            Vector::Const(v) => constant_column(&v, count),
        }
    }
}

/// `count` rows of `v`, a vector of the kind its values read as from a
/// store: of integers, numbers, texts, truth values or datetimes, each
/// value the one `v` is.
pub(in crate::exec::batch) fn constant_column(v: &Value, count: usize) -> Column {
    match v {
        Value::Null => Column::nulls(count),
        Value::Exact(d) => match i64::try_from(d.units) {
            Ok(unit) => Column::Ints(Ints::new(vec![unit; count], d.scale, None).with_runs(runs_of_one(count))),
            Err(_) => Column::Values(vec![v.clone(); count]),
        },
        Value::Approx(f) => Column::Floats(Floats::new(vec![*f; count], None)),
        Value::Char(s) => Column::Texts(repeated(s, count)),
        Value::Bool(b) => {
            let mut bits = Mask::none(count);
            if *b {
                (0..count).for_each(|n| bits.set(n));
            }
            Column::Bools(Bools::new(bits, None))
        }
        Value::Datetime(d) => Column::Datetimes(datetimes_of(&vec![Some(*d); count], d)),
        Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Interval(_)
        | Value::Lob(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Row(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Json(_)
        | Value::Xml(_)
        | Value::Extension(_) => Column::Values(vec![v.clone(); count]),
    }
}

/// The ends of one run of `count` rows, none of none.
fn runs_of_one(count: usize) -> Vec<usize> {
    if count == 0 { Vec::new() } else { vec![count] }
}

/// The texts of `count` rows, each `s`.
fn repeated(s: &str, count: usize) -> Texts {
    let ends = (1..=count).map(|n| n.saturating_mul(s.len())).collect();
    Texts::new(s.repeat(count), ends, None)
}

/// Datetimes of the type of `like`, each of `made` or a null, their zones
/// kept whatever the type, so each value is the one made.
pub(super) fn datetimes_of(made: &[Option<Datetime>], like: &Datetime) -> Datetimes {
    let utc = made.iter().map(|d| d.map_or(0, |d| d.utc)).collect();
    let zones = made.iter().map(|d| d.map_or(0, |d| d.zone)).collect();
    let nulls = Mask::from_bools(&made.iter().map(Option::is_none).collect::<Vec<bool>>());
    Datetimes::new(utc, Some(zones), (like.moment, like.zoned, like.precision), nulls.any().then_some(nulls))
}

/// The rows `rows` of `column`, in order, a vector of the same kind: of
/// integers known as runs, known as the runs of those rows.
pub(in crate::exec::batch) fn gather(column: &Column, rows: &[u32]) -> Column {
    if let Column::Ints(ints) = column {
        let units = rows.iter().map(|&r| ints.units.get(index(r)).copied().unwrap_or(0)).collect();
        let nulls = ints.nulls.as_ref().and_then(|n| n.gathered(rows));
        let gathered = Ints::new(units, ints.scale, nulls);
        return Column::Ints(match ints.runs_of(rows) {
            Some(runs) => gathered.with_runs(ends_of(runs.map(|(_, these)| these.len()))),
            None => gathered,
        });
    }
    let picks: Vec<Option<usize>> = rows.iter().map(|&r| Some(index(r))).collect();
    column.gathered(&picks)
}

/// The ends of runs of these lengths, those of none left out.
fn ends_of(lengths: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut end = 0_usize;
    lengths
        .filter(|&n| n > 0)
        .map(|n| {
            end = end.saturating_add(n);
            end
        })
        .collect()
}

/// How a column tells its nulls.
pub(super) enum Marks<'c> {
    /// By a mask, none if no row is null.
    Of(Option<&'c Mask>),
    /// By its values, as a vector of values or of nulls does.
    Values,
}

/// How `column` tells which of its rows are null.
pub(super) fn marks(column: &Column) -> Marks<'_> {
    match column {
        Column::Ints(Ints { nulls, .. })
        | Column::Floats(Floats { nulls, .. })
        | Column::Texts(Texts { nulls, .. })
        | Column::Datetimes(Datetimes { nulls, .. })
        | Column::Bools(Bools { nulls, .. }) => Marks::Of(nulls.as_ref()),
        Column::Values(_) | Column::Nulls(_) => Marks::Values,
    }
}

/// Whether each of `count` rows of `v` is null.
pub(super) fn nulls(v: &Vector<'_>, count: usize) -> Mask {
    if let Some(column) = v.column()
        && let Marks::Of(mask) = marks(column)
    {
        return mask.cloned().unwrap_or_else(|| Mask::none(count));
    }
    let seq = v.seq();
    let mut out = Mask::none(count);
    for n in 0..count {
        if seq.get(n).is_null() {
            out.set(n);
        }
    }
    out
}

/// `mask`, none if no bit of it is set.
pub(super) fn some(mask: Mask) -> Option<Mask> {
    mask.any().then_some(mask)
}

/// The nulls of either of two vectors' null masks, none if neither has one.
pub(super) fn either(x: Option<&Mask>, y: Option<&Mask>) -> Option<Mask> {
    match (x, y) {
        (Some(a), Some(b)) => some(a.or(b)),
        (Some(a), None) | (None, Some(a)) => Some(a.clone()),
        (None, None) => None,
    }
}

/// A vector of values made of `values`, as a row's evaluation gives them.
pub(super) fn values(values: Vec<Value>) -> Vector<'static> {
    Vector::Owned(Column::Values(values))
}
