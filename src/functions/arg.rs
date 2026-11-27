//! \[DESIGN\] (RULES.md D363) An argument of a call of a scalar function
//! over vectors (`ScalarFunction::eval_batch`).

use crate::storage::Column;
use crate::value::Value;

/// \[DESIGN\] An argument of a call over vectors (RULES.md D363,
/// `ScalarFunction::eval_batch`): a vector of a value a row, or a
/// constant, every row's.
#[derive(Debug, Clone, Copy)]
pub enum Arg<'a> {
    /// A vector of a value a row.
    Column(&'a Column),
    /// A constant.
    Const(&'a Value),
}
