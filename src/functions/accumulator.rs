//! The running state of a set function over one group (`Accumulator`):
//! given its argument's values one at a time or as vectors (RULES.md
//! D31), its state written and merged where a grouping spills (D38, D53).

use crate::error::{Code, Result, fail};
use crate::kernels::Kernels;
use crate::storage::{Floats, Ints, Texts};
use crate::value::Value;

/// The running state of a set function over one group.
pub trait Accumulator {
    /// Called once per value of the argument set S (never null).
    /// For COUNT(*), called once per row with a null placeholder.
    ///
    /// # Errors
    ///
    /// The value cannot be taken in. The built-in SUM and AVG raise here
    /// only of a sum 128 bits cannot hold, a limit: a result out of range is
    /// raised by `finish` [ISO/IEC 9075:1992 §6.5 GR2b iv], so whether it
    /// is does not depend on the order of the values (RULES.md D15).
    fn update(&mut self, v: &Value) -> Result<()>;
    /// \[DESIGN\] Called with a vector of the argument's values (RULES.md
    /// D31): of those at `rows`, or all, each not null, as `update` is.
    /// Default: `update` of each.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_many(&mut self, values: &[Value], rows: Option<&[u32]>) -> Result<()> {
        match rows {
            Some(rows) => rows
                .iter()
                .filter_map(|&r| values.get(crate::storage::mask::index(r)))
                .filter(|v| !v.is_null())
                .try_for_each(|v| self.update(v)),
            None => values.iter().filter(|v| !v.is_null()).try_for_each(|v| self.update(v)),
        }
    }
    /// \[DESIGN\] Called with a vector of exact numbers as integers
    /// (`Ints`, RULES.md D31): of those at `rows`, or all, nulls left out,
    /// as `update_many` is, on the vector kernels `kernels` where it runs
    /// any (RULES.md D570). Default: `update_many` of their values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_ints(&mut self, ints: &Ints, rows: Option<&[u32]>, _kernels: &dyn Kernels) -> Result<()> {
        self.update_many(ints.values(), rows)
    }
    /// \[DESIGN\] Called with a vector of approximate numbers (`Floats`,
    /// RULES.md D31), as `update_many` is. Default: `update_many` of their
    /// values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_floats(&mut self, floats: &Floats, rows: Option<&[u32]>) -> Result<()> {
        self.update_many(floats.values(), rows)
    }
    /// \[DESIGN\] Called with a vector of character strings as their text
    /// (`Texts`, RULES.md D31), as `update_many` is. Default: `update_many`
    /// of their values.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_texts(&mut self, texts: &Texts, rows: Option<&[u32]>) -> Result<()> {
        self.update_many(texts.values(), rows)
    }
    /// \[DESIGN\] For COUNT(*): called once for `rows` rows, as `update`
    /// is once a row. Default: `update` of a null placeholder for each.
    ///
    /// # Errors
    ///
    /// As `update`.
    fn update_rows(&mut self, rows: usize) -> Result<()> {
        (0..rows).try_for_each(|_| self.update(&Value::Null))
    }
    /// \[DESIGN\] Appends its state so far to `out`, as bytes `merge` takes
    /// in another accumulator of its function and result type (RULES.md
    /// D38, D53): a grouping past its work area then writes groups partly
    /// made, not their rows. Whether it has one: by default it has none,
    /// writes nothing, and its rows are written, so it is given them all.
    fn write_state(&self, _out: &mut Vec<u8>) -> bool {
        false
    }
    /// \[DESIGN\] Takes in `state`, as `write_state` wrote it of an accumulator
    /// of its function and result type given the values after those it
    /// was given, and before any it is given next: as if it had been given
    /// them. It judges no result: `finish` does.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `state` is not one it writes; by default, as it
    /// writes none.
    fn merge(&mut self, _state: &[u8]) -> Result<()> {
        fail(Code::Corrupt, "[DESIGN] a set function's state that it never writes")
    }
    /// The result for the values seen so far, possibly none.
    ///
    /// # Errors
    ///
    /// The result cannot be computed or is out of range.
    fn finish(&self) -> Result<Value>;
}
