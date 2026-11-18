//! The operators of \[XQueryFO\], the `op:` functions that back up XQuery's
//! operators [XQuery Appendix B.2], each a plain function named after the
//! one it is: `op:numeric-add` is `numeric_add`. They are not functions a
//! query calls by name [XQueryFO §1.2], so they are not in `lookup`; the
//! evaluation of XQuery's operators calls them, its operands atomized as
//! [XQuery §3.4, §3.5] says.

pub use super::booleans::{boolean_equal, boolean_greater_than, boolean_less_than};
pub use super::numeric::{
    Number, numeric_add, numeric_divide, numeric_equal, numeric_greater_than, numeric_integer_divide, numeric_less_than, numeric_mod,
    numeric_multiply, numeric_order, numeric_subtract, numeric_unary_minus, numeric_unary_plus,
};
