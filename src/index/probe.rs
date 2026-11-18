//! \[DESIGN\] Whether a key answers a probe, by the [X3.135-1986 §5.11]
//! comparison rules, as storage checks entries under its locks.

use super::{ColumnComparison, Probe};
use crate::value::{CompOp, Value, compare};

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
