//! \[DESIGN\] What an index is asked for (`Probe`), and whether a key
//! answers it, by the [X3.135-1986 §5.11] comparison rules, as storage
//! checks entries under its locks.

use crate::functions::ScalarRef;
use crate::value::{CompOp, Value, compare};

/// \[EXTENSION\] A function call used to find rows through an index, such
/// as `f(column, ?)` (RULES.md D34). The function must opt in through
/// `ScalarFunction::index_predicate`, or the separately proven spatial
/// contract in D85. An index method declares support
/// for this function and both positions, for every accepted value of the
/// other argument, including null and later prepared parameter values.
#[derive(Debug, Clone)]
pub struct Predicate {
    /// The registered function, also used to check each fetched row.
    pub function: ScalarRef,
    /// The indexed column's position in the key, from zero.
    pub column: usize,
    /// The column's position among the function's two arguments, zero or one.
    pub argument: usize,
}

/// One SQL comparison of an independently constrained index column.
#[derive(Debug, Clone)]
pub struct ColumnComparison {
    /// The position of the column in the index key, from zero.
    pub column: usize,
    /// The comparison operator.
    pub op: CompOp,
    /// The value compared with that column.
    pub value: Value,
}

/// What an index is asked for.
#[derive(Debug, Clone)]
pub enum Probe {
    /// All comparisons must hold, including comparisons on columns after
    /// a range. This describes a multidimensional window without requiring
    /// an equality prefix (`IndexMethod::supports_columns`).
    Columns(Vec<ColumnComparison>),
    /// Every entry (used to read rows in key order): of a key with no null,
    /// but in an index that holds them (`IndexDesc::nulls`).
    All,
    /// "column `<op>` value", of the key's first column.
    Compare {
        /// The comparison operator.
        op: CompOp,
        /// The constant the column is compared with.
        value: Value,
    },
    /// The keys whose first columns equal `equal`, one value for each, and,
    /// if `then` is given, whose next column compares with its value by
    /// its operator (`IndexMethod::supports_prefix`).
    Prefix {
        /// The values of the key's first columns.
        equal: Vec<Value>,
        /// The comparison of the next column, if any.
        then: Option<(CompOp, Value)>,
    },
    /// The keys whose first columns equal `equal`, one value for each, if
    /// any, and whose next column lies between two bounds: above `low` by
    /// its operator, `>` or `>=`, and below `high` by its, `<` or `<=`; as
    /// BETWEEN gives [ISO/IEC 9075:1992 §8.3 SR6], or two comparisons
    /// (`IndexMethod::supports_range`).
    Range {
        /// The values of the key's first columns.
        equal: Vec<Value>,
        /// The lower bound of the next column, and its operator; boxed, as
        /// are `high`'s, so that a probe, and each plan holding one, is no
        /// larger than another kind of probe makes it.
        low: Box<(CompOp, Value)>,
        /// The upper bound of the next column, and its operator.
        high: Box<(CompOp, Value)>,
    },
    /// \[EXTENSION\] Rows that may satisfy a Boolean function call.
    /// The index returns every matching entry and may return extras.
    /// The executor fetches each visible row and checks the full WHERE
    /// condition to reject those extras (D34).
    Predicate {
        /// The function and the column's positions in the key and call.
        predicate: Box<Predicate>,
        /// The other argument, as the bound function receives it.
        value: Value,
    },
}

impl Probe {
    /// Whether a key, its values in order, answers the probe by the
    /// [X3.135-1986 §5.11] comparison rules: never with a null where a
    /// value is compared (GR2). For `Probe::Predicate`, this always returns
    /// true: storage uses it under locks, where calling a plug-in is
    /// forbidden. The executor checks the function on fetched rows
    /// (RULES.md D34).
    pub fn answers<'v>(&self, key: impl IntoIterator<Item = &'v Value>) -> bool {
        let mut key = key.into_iter();
        let holds = |op: CompOp, k: Option<&Value>, v: &Value| k.and_then(|k| compare(k, v)).is_some_and(|o| op.holds(o));
        match self {
            Probe::All | Probe::Predicate { .. } => true,
            Probe::Columns(each) => columns_answer(key, each),
            Probe::Compare { op, value } => holds(*op, key.next(), value),
            Probe::Prefix { equal, then } => {
                equal.iter().all(|v| holds(CompOp::Eq, key.next(), v)) && then.as_ref().is_none_or(|(op, v)| holds(*op, key.next(), v))
            }
            Probe::Range { equal, low, high } => {
                let equals = equal.iter().all(|v| holds(CompOp::Eq, key.next(), v));
                let next = key.next();
                equals && holds(low.0, next, &low.1) && holds(high.0, next, &high.1)
            }
        }
    }
}

/// Test independent comparisons without allocating a copy of the key.
/// Missing key columns cannot satisfy a comparison.
fn columns_answer<'a>(key: impl Iterator<Item = &'a Value>, each: &[ColumnComparison]) -> bool {
    let mut checked = 0_usize;
    for (at, value) in key.enumerate() {
        for c in each.iter().filter(|c| c.column == at) {
            if !compare(value, &c.value).is_some_and(|o| c.op.holds(o)) {
                return false;
            }
            checked = checked.saturating_add(1);
        }
    }
    checked == each.len()
}
