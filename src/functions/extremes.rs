//! MAX and MIN, the set functions that keep the best value of a group
//! [X3.135-1986 §5.8]. Split from functions.rs (RULES.md R-SIZE-2).

use super::{Accumulator, AggregateFunction, floats, ints};
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::storage::{Floats, Ints};
use crate::value::{DataType, Value, compare};

/// MAX and MIN. [X3.135-1986 §5.8 SR8] the result type is the argument's. [X3.135-1986 §5.8 GR4d] by the [X3.135-1986 §5.11] rules.
pub(super) struct Extreme {
    pub(super) max: bool,
}

impl AggregateFunction for Extreme {
    fn name(&self) -> &str {
        if self.max { kw::MAX } else { kw::MIN }
    }
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        // [X3.135-1986 §5.8 F] only COUNT has the form (*); the extension call
        // syntax always passes an argument too.
        arg.ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[X3.135-1986 §5.8 F] {} takes an argument", self.name())))
    }
    fn accumulator(&self, _result: DataType) -> Box<dyn Accumulator> {
        Box::new(ExtremeAcc { best: Value::Null, max: self.max })
    }
}

struct ExtremeAcc {
    best: Value,
    max: bool,
}

impl Accumulator for ExtremeAcc {
    /// [X3.135-1986 §5.8 GR4d] keeps the maximum or minimum by the comparison
    /// rules of [X3.135-1986 §5.11].
    fn update(&mut self, v: &Value) -> Result<()> {
        let better = match compare(v, &self.best) {
            None => true, // nothing seen yet
            Some(o) => o == if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less },
        };
        if better {
            self.best = v.clone();
        }
        Ok(())
    }
    /// The best of the vector found first, by `update`'s rule, then
    /// cloned once.
    fn update_many(&mut self, values: &[Value], rows: Option<&[usize]>) -> Result<()> {
        let wanted = if self.max { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less };
        let found = match rows {
            Some(rows) => best_of(rows.iter().filter_map(|&r| values.get(r)), wanted),
            None => best_of(values.iter(), wanted),
        };
        found.map_or(Ok(()), |v| self.update(v))
    }
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[usize]>) -> Result<()> {
        ints::extreme(ints, rows, self.max).map_or(Ok(()), |v| self.update(&v))
    }
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[usize]>) -> Result<()> {
        floats::extreme(floats, rows, self.max).map_or(Ok(()), |v| self.update(&v))
    }
    fn finish(&self) -> Result<Value> {
        Ok(self.best.clone())
    }
}

/// The value of `values` not null that each one after replaces if it
/// compares with it as `wanted`, or not at all (`ExtremeAcc::update`).
fn best_of<'v>(values: impl Iterator<Item = &'v Value>, wanted: std::cmp::Ordering) -> Option<&'v Value> {
    values.filter(|v| !v.is_null()).reduce(|best, v| if compare(v, best).is_none_or(|o| o == wanted) { v } else { best })
}
