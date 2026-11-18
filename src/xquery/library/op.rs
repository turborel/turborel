//! The operators of \[XQueryFO\], the `op:` functions that back up XQuery's
//! operators [XQuery Appendix B.2], each a plain function named after the
//! one it is: `op:numeric-add` is `numeric_add`. They are not functions a
//! query calls by name [XQueryFO §1.2], so they are no built-in functions:
//! the evaluation of XQuery's operators calls them, its operands atomized
//! as [XQuery §3.4, §3.5] says.

pub use super::binary::{base64_binary_equal, hex_binary_equal, notation_equal};
pub use super::booleans::{boolean_equal, boolean_greater_than, boolean_less_than};
pub use super::calendar::{
    add_day_time_duration_to_date, add_day_time_duration_to_date_time, add_day_time_duration_to_time, add_year_month_duration_to_date,
    add_year_month_duration_to_date_time, subtract_date_times, subtract_dates, subtract_day_time_duration_from_date,
    subtract_day_time_duration_from_date_time, subtract_day_time_duration_from_time, subtract_times,
    subtract_year_month_duration_from_date, subtract_year_month_duration_from_date_time,
};
pub use super::durations::{
    add_day_time_durations, add_year_month_durations, day_time_duration_equal, day_time_duration_greater_than, day_time_duration_less_than,
    divide_day_time_duration, divide_day_time_duration_by_day_time_duration, divide_year_month_duration,
    divide_year_month_duration_by_year_month_duration, duration_equal, duration_order, multiply_day_time_duration,
    multiply_year_month_duration, subtract_day_time_durations, subtract_year_month_durations, year_month_duration_equal,
    year_month_duration_greater_than, year_month_duration_less_than,
};
pub use super::moments::{
    date_equal, date_greater_than, date_less_than, date_time_equal, date_time_greater_than, date_time_less_than, gregorian_equal,
    moment_order, time_equal, time_greater_than, time_less_than,
};
pub use super::node_sets::{except, intersect, union};
pub use super::nodes::{is_same_node, node_after, node_before};
pub use super::numeric::{
    Number, numeric_add, numeric_divide, numeric_equal, numeric_greater_than, numeric_integer_divide, numeric_less_than, numeric_mod,
    numeric_multiply, numeric_order, numeric_subtract, numeric_unary_minus, numeric_unary_plus,
};
pub use super::qnames::qname_equal;
pub use super::sequences::{concatenate, to};
