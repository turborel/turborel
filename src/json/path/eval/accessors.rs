//! The accessor ops of an SQL/JSON path expression evaluated [ISO/IEC
//! 9075-2:2023 §9.46 GR11g ii 1 to 5]: the member, wildcard member, array
//! and wildcard array accessors, and the filter.
//!
//! [§4.48.5] Lax mode wraps or unwraps a base as the op needs, and takes
//! what it still cannot access for nothing; strict mode raises the
//! structural error. Implementation-defined and -dependent choices:
//! (IA071) an object of two members of one key raises no exception, and
//! (UA075) a member accessor takes the first of them; (UA058) a wildcard
//! member accessor deletes none of them, and (US040) gives the bound values
//! in the order the members are kept; (IA073) a subscript is truncated
//! toward zero.

use super::predicate::Truth;
use super::{Evaluator, Number, exception, singleton_number};
use crate::error::{Result, SqlState};
use crate::json::Item;
use crate::json::path::ast::{Expr, Predicate, Subscript};
use crate::value::{DecFloat, MAX_DECFLOAT_PRECISION, pow10};

/// Where a subscript, truncated, is in an array of some length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// Negative.
    Below,
    /// The subscript of an element.
    At(usize),
    /// The length or more.
    Beyond,
}

impl Evaluator<'_> {
    /// [§9.46 GR11g ii 1] `<JSON member accessor>`: the bound value of the
    /// member of key `key` of each object of `base`.
    pub(super) fn member(&self, base: Vec<Item>, key: &str) -> Result<Vec<Item>> {
        let base = self.lax_unwrap(base);
        let mut values = Vec::new();
        for item in &base {
            let bound = item.as_object().and_then(|members| members.iter().find(|member| member.key == key));
            let Some(member) = bound else {
                if self.strict() {
                    return Err(exception(
                        SqlState::SQL_JSON_MEMBER_NOT_FOUND,
                        &format!(
                            "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 1 B II] an SQL/JSON item is not an object with a member of the key {key}"
                        ),
                    ));
                }
                continue;
            };
            values.push(member.value.clone());
        }
        Ok(values)
    }

    /// [§9.46 GR11g ii 2] `<JSON wildcard member accessor>`: the bound
    /// values of every member of each object of `base`.
    pub(super) fn any_member(&self, base: Vec<Item>) -> Result<Vec<Item>> {
        let base = self.lax_unwrap(base);
        if self.strict() && base.iter().any(|item| item.as_object().is_none()) {
            return Err(exception(
                SqlState::SQL_JSON_OBJECT_NOT_FOUND,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 2 A II] an SQL/JSON item is not an object",
            ));
        }
        let members = base.iter().filter_map(Item::as_object).flatten();
        Ok(members.map(|member| member.value.clone()).collect())
    }

    /// [§9.46 GR11g ii 3] `<JSON array accessor>`: of each array of
    /// `base`, the elements the subscripts of `list` select, in turn.
    pub(super) fn subscripts(&self, base: Vec<Item>, list: &[Subscript]) -> Result<Vec<Item>> {
        let base = self.lax_wrap(base);
        let arrays = arrays(&base, "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 B] an SQL/JSON item is not an array")?;
        // [GR11g ii 3 E] Every subscript of every array is evaluated, with
        // the array current, before any is checked against its array.
        let mut bounds = Vec::with_capacity(arrays.len());
        for elements in &arrays {
            let within = self.with_array(elements.len());
            bounds.push(list.iter().map(|subscript| within.bounds(subscript, elements.len())).collect::<Result<Vec<_>>>()?);
        }
        let mut values = Vec::new();
        for (elements, each) in arrays.iter().zip(&bounds) {
            for &(from, to) in each {
                self.select(elements, from, to, &mut values)?;
            }
        }
        Ok(values)
    }

    /// [§9.46 GR11g ii 3 E II, E III, F II 1] The first and last
    /// subscripts of `subscript`, truncated, in an array of `len` elements.
    fn bounds(&self, subscript: &Subscript, len: usize) -> Result<(Position, Position)> {
        let from = self.position(&subscript.from, len)?;
        let to = match &subscript.to {
            Some(to) => self.position(to, len)?,
            None => from,
        };
        Ok((from, to))
    }

    /// The position of the subscript the wff `expr` gives, truncated
    /// (IA073), in an array of `len` elements. \[DESIGN\] An exception of
    /// the wff is the result, as a base's is \[GR11g i\].
    fn position(&self, expr: &Expr, len: usize) -> Result<Position> {
        let seq = self.wff(expr)?;
        let digits = singleton_number(&seq).and_then(digits).ok_or_else(|| {
            exception(
                SqlState::INVALID_SQL_JSON_SUBSCRIPT,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 E III] an SQL/JSON subscript is not a singleton numeric value",
            )
        })?;
        Ok(position(digits, len))
    }

    /// [§9.46 GR11g ii 3 F II 2, F III] The elements of `elements` from
    /// `from` to `to`, appended to `values`. In strict mode a subscript
    /// out of the array, or a first past the last, is an exception; in lax
    /// mode only the elements there are are taken.
    fn select(&self, elements: &[Item], from: Position, to: Position, values: &mut Vec<Item>) -> Result<()> {
        if self.strict() && !within(from, to) {
            return Err(exception(
                SqlState::INVALID_SQL_JSON_SUBSCRIPT,
                "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 3 F II 2 a] an SQL/JSON subscript is outside its array, or the first of a range past the last",
            ));
        }
        let len = elements.len();
        let start = match from {
            Position::Below => 0,
            Position::At(k) => k,
            Position::Beyond => len,
        };
        let end = match to {
            Position::Below => 0,
            Position::At(k) => k.saturating_add(1),
            Position::Beyond => len,
        };
        values.extend(elements.get(start..end).into_iter().flatten().cloned());
        Ok(())
    }

    /// [§9.46 GR11g ii 4] `<JSON wildcard array accessor>`: every element
    /// of each array of `base`.
    pub(super) fn any_element(&self, base: Vec<Item>) -> Result<Vec<Item>> {
        let base = self.lax_wrap(base);
        let arrays = arrays(&base, "[ISO/IEC 9075-2:2023 §9.46 GR11g ii 4 B] an SQL/JSON item is not an array")?;
        Ok(arrays.into_iter().flatten().cloned().collect())
    }

    /// [§9.46 GR11g ii 5] `<JSON filter expression>`: the items of `base`
    /// for which `predicate` is True, each the current item in turn.
    pub(super) fn filter(&self, base: Vec<Item>, predicate: &Predicate) -> Vec<Item> {
        let base = self.lax_unwrap(base);
        base.into_iter().filter(|item| self.with_current(item).truth(predicate) == Truth::True).collect()
    }
}

/// The elements of each item of `base`, all arrays.
///
/// # Errors
///
/// SQLSTATE 22039, its message `cited`, if one is not an array.
fn arrays<'b>(base: &'b [Item], cited: &str) -> Result<Vec<&'b [Item]>> {
    base.iter().map(|item| item.as_array().ok_or_else(|| exception(SqlState::SQL_JSON_ARRAY_NOT_FOUND, cited))).collect()
}

/// The number `n` as a coefficient and the power of ten it is multiplied
/// by; none if approximate and not finite.
fn digits(n: Number) -> Option<(i128, i64)> {
    match n {
        Number::Exact(d) => Some((d.units, i64::from(d.scale).saturating_neg())),
        Number::DecFloat(d) => Some((d.coefficient(), i64::from(d.exponent()))),
        // Rounded to more digits than an approximate number has, so that no
        // whole number is crossed.
        Number::Approx(x) => DecFloat::of_f64(x, MAX_DECFLOAT_PRECISION).map(|d| (d.coefficient(), i64::from(d.exponent()))),
    }
}

/// [§9.46 GR11g ii 3 F II 1] Where `c` × 10^`e`, truncated toward zero
/// (IA073), is in an array of `len` elements.
fn position((c, e): (i128, i64), len: usize) -> Position {
    let whole = match u32::try_from(e) {
        Ok(up) => pow10(up).and_then(|p| c.checked_mul(p)),
        // Below 1 in magnitude if no i128 holds the power of ten.
        Err(_) => Some(u32::try_from(e.saturating_neg()).ok().and_then(pow10).map_or(0, |p| c.checked_div(p).unwrap_or(0))),
    };
    let Some(whole) = whole else { return if c < 0 { Position::Below } else { Position::Beyond } };
    if whole < 0 {
        return Position::Below;
    }
    usize::try_from(whole).ok().filter(|k| *k < len).map_or(Position::Beyond, Position::At)
}

/// [§9.46 GR11g ii 3 F II 2] Whether a range from `from` to `to` has no
/// potential error: both within the array, the first not past the last.
fn within(from: Position, to: Position) -> bool {
    let (Position::At(first), Position::At(last)) = (from, to) else { return false };
    first <= last
}

#[cfg(test)]
mod tests;
