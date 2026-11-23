//! Index keys: ordered tuples of values, by the [X3.135-1986 §5.11]
//! comparison rules.

use std::cmp::Ordering;

use crate::value::{Value, compare};

/// A tuple of column values ordered exactly as SQL compares them, so that
/// 'OPS' and 'OPS ' are the same key and 1.5 sorts after 1.
#[derive(Debug, Clone)]
pub struct Key(pub Vec<Value>);

fn total_cmp(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap_or_else(|| a.is_null().cmp(&b.is_null()).reverse())
}

impl Key {
    /// Whether the values of `row` at `columns` are this key, compared as
    /// two keys are, with no key made of them (RULES.md D501).
    pub fn is_of(&self, row: &[Value], columns: &[usize]) -> bool {
        Key::values_are_of(&self.0, row, columns)
    }

    /// As `is_of`, of the key whose values are `key`, with no key made.
    pub fn values_are_of(key: &[Value], row: &[Value], columns: &[usize]) -> bool {
        let same = |(&column, value): (&usize, &Value)| row.get(column).is_some_and(|v| total_cmp(v, value) == Ordering::Equal);
        columns.len() == key.len() && columns.iter().zip(key).all(same)
    }
}

impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        for (a, b) in self.0.iter().zip(&o.0) {
            let c = total_cmp(a, b);
            if c != Ordering::Equal {
                return c;
            }
        }
        self.0.len().cmp(&o.0.len())
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Key {}
