//! [X3.135-1986 §8.3; ISO/IEC 9075-2:2023 §10.10] A `<sort
//! specification>`: its `<sort key>` a position, a `<column
//! specification>` or any other `<value expression>`, then its direction
//! and `<null ordering>`.

use crate::ast::{Literal, SortColumn, SortSpecification, ValueExpression, ValueSpecification};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, kw};
use crate::parser::Parser;

impl Parser {
    /// [X3.135-1986 §8.3] `<sort specification>` ::= {`<unsigned integer>`
    /// | `<column specification>`} \[ASC | DESC\] \[ISO/IEC 9075-2:2023
    /// §10.10\] \[`<null ordering>`\]; [§10.10] its `<sort key>` any
    /// `<value expression>`.
    pub(in crate::parser) fn sort_specification(&mut self) -> Result<SortSpecification> {
        let digits = if let Tok::ExactNumeric(t) = self.peek() { t.chars().all(|c| c.is_ascii_digit()) } else { false };
        let at = self.here();
        // [ISO/IEC 9075:1992 §13.1] [<collate clause>], of a position or a
        // column specification too.
        let (key, collate) = match self.value_expression()? {
            ValueExpression::Collated(v, name) => (*v, Some(name)),
            v @ (ValueExpression::Dyadic(..)
            | ValueExpression::Monadic(..)
            | ValueExpression::Value(_)
            | ValueExpression::Column(_)
            | ValueExpression::SetFunction(_)
            | ValueExpression::Nested(_)
            | ValueExpression::Boolean(_)
            | ValueExpression::CollationFor(_)
            | ValueExpression::NextValue { .. }
            | ValueExpression::Call { .. }
            | ValueExpression::Function { .. }
            | ValueExpression::Case(_)
            | ValueExpression::Cast { .. }
            | ValueExpression::Datetime(_)
            | ValueExpression::Extract { .. }
            | ValueExpression::Difference { .. }
            | ValueExpression::AtZone { .. }
            | ValueExpression::Subquery(_)
            | ValueExpression::Composite(..)
            | ValueExpression::Window(_)
            | ValueExpression::NestedWindow(_)) => (v, None),
        };
        let key = sort_column(key, digits).map_err(|e| e.or_at(at))?;
        let descending = if self.accept_kw(kw::DESC) {
            true
        } else {
            self.accept_kw(kw::ASC);
            false
        };
        let nulls_first = self.null_ordering()?;
        Ok(SortSpecification { key, collate, descending, nulls_first, at })
    }
}

/// What the `<sort key>` `key` sorts on: a position, written as the digits
/// of an `<unsigned integer>` alone if `digits`; a `<column
/// specification>`; or any other `<value expression>`.
///
/// # Errors
///
/// SQLCODE -900 if a position is past this implementation's maximum.
fn sort_column(key: ValueExpression, digits: bool) -> Result<SortColumn> {
    if let Some(n) = written_position(&key, digits) {
        // [X3.135-1986 §5.2] sets no maximum; this implementation's is u32::MAX.
        let n = u32::try_from(n).map_err(|_| {
            SqlError::new(Code::Unsupported, format!("[DESIGN] {n} is larger than this implementation's maximum of {}", u32::MAX))
        })?;
        return Ok(SortColumn::Position(n));
    }
    Ok(match key {
        ValueExpression::Column(cs) => SortColumn::Column(cs),
        other @ (ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::SetFunction(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Boolean(_)
        | ValueExpression::Collated(..)
        | ValueExpression::CollationFor(_)
        | ValueExpression::NextValue { .. }
        | ValueExpression::Call { .. }
        | ValueExpression::Function { .. }
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Composite(..)
        | ValueExpression::Window(_)
        | ValueExpression::NestedWindow(_)) => SortColumn::Value(other),
    })
}

/// The position the `<sort key>` `key` gives, if it is the digits of an
/// `<unsigned integer>` alone, as `digits` says.
fn written_position(key: &ValueExpression, digits: bool) -> Option<i128> {
    match key {
        ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(d))) => digits.then_some(d.units),
        ValueExpression::Column(_)
        | ValueExpression::Dyadic(..)
        | ValueExpression::Monadic(..)
        | ValueExpression::Value(_)
        | ValueExpression::SetFunction(_)
        | ValueExpression::Nested(_)
        | ValueExpression::Boolean(_)
        | ValueExpression::Collated(..)
        | ValueExpression::CollationFor(_)
        | ValueExpression::NextValue { .. }
        | ValueExpression::Call { .. }
        | ValueExpression::Function { .. }
        | ValueExpression::Case(_)
        | ValueExpression::Cast { .. }
        | ValueExpression::Datetime(_)
        | ValueExpression::Extract { .. }
        | ValueExpression::Difference { .. }
        | ValueExpression::AtZone { .. }
        | ValueExpression::Subquery(_)
        | ValueExpression::Composite(..)
        | ValueExpression::Window(_)
        | ValueExpression::NestedWindow(_) => None,
    }
}

// Unit tests: each form of a sort key (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::CursorSpecification;

    /// The cursor specification `sql`, parsed.
    fn parsed(sql: &str) -> Result<CursorSpecification> {
        Parser::new(sql)?.cursor_specification()
    }

    /// Verifies: [X3.135-1986 §8.3; ISO/IEC 9075-2:2023 §10.10] an unsigned
    /// integer is a position, a column specification a column, and any
    /// other value expression a value, its COLLATE taken off, where it is
    /// written.
    #[test]
    fn each_form_of_a_sort_key_10_10() {
        let c = parsed("SELECT A FROM T ORDER BY 2 DESC, A NULLS FIRST, A COLLATE X, A + 1, 1.0, (1), B.A").unwrap();
        let keys: Vec<&SortColumn> = c.order_by.iter().map(|s| &s.key).collect();
        assert!(matches!(
            keys.as_slice(),
            [
                SortColumn::Position(2),
                SortColumn::Column(_),
                SortColumn::Column(_),
                SortColumn::Value(ValueExpression::Dyadic(..)),
                SortColumn::Value(ValueExpression::Value(_)),
                SortColumn::Value(ValueExpression::Nested(_)),
                SortColumn::Column(_),
            ]
        ));
        let [first, second, third, fourth, ..] = c.order_by.as_slice() else { panic!("seven keys") };
        assert!(first.descending && first.collate.is_none());
        assert_eq!(second.nulls_first, Some(true));
        assert!(third.collate.is_some() && !third.descending);
        assert!(fourth.collate.is_none());
        assert_eq!((first.at, third.at), (25, 48));
    }

    /// Verifies: [DESIGN] a position past `u32::MAX` is SQLCODE -900, one
    /// at it a position.
    #[test]
    fn a_position_past_the_maximum_is_refused() {
        assert_eq!(parsed("SELECT A FROM T ORDER BY 4294967296").err().map(|e| e.code), Some(Code::Unsupported));
        let c = parsed("SELECT A FROM T ORDER BY 4294967295").unwrap();
        assert!(matches!(c.order_by.first().map(|s| &s.key), Some(SortColumn::Position(u32::MAX))));
    }
}
