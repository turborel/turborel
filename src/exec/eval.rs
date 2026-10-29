//! Evaluation of bound expressions and conditions against a row: `<value
//! expression>` [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.6-§6.11] and
//! `<search condition>` with three-valued logic [X3.135-1986 §5.18; ISO/IEC
//! 9075:1992 §8.12], the subqueries among them run by `Subqueries`. Split
//! from exec.rs (RULES.md R-SIZE-2).

use super::{Subqueries, broken};
use crate::catalog::MatchType;
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::logical::{Cond, DomainChecks, Expr, Quantifier, Subquery, Truth};
use crate::spill::norm_row;
use crate::storage::Row;
use crate::value::{CompOp, Value, arith, cast, compare, like};

/// The value of column `i` of `row`.
fn column(row: &Row, i: usize) -> Result<&Value> {
    row.get(i).ok_or_else(|| broken(&format!("column {i} is not in a row of {} columns", row.len())))
}

/// [X3.135-1986 §5.9] Evaluates a `<value expression>` against a row, its
/// subqueries run by `subqueries`.
///
/// # Errors
///
/// An arithmetic, cast or function error; a scalar subquery with more than
/// one row [ISO/IEC 9075:1992 §7.11 GR1]; a column the row does not have
/// (SQLCODE -910).
pub fn eval(e: &Expr, row: &Row, subqueries: &dyn Subqueries) -> Result<Value> {
    match e {
        Expr::Column(i) => column(row, *i).cloned(),
        Expr::Const(v) => Ok(v.clone()),
        Expr::Neg(x) => Ok(eval(x, row, subqueries)?.negate()),
        Expr::Arith(a, op, b) => arith(*op, &eval(a, row, subqueries)?, &eval(b, row, subqueries)?),
        Expr::Call(f, args) => {
            let values = args.iter().map(|a| eval(a, row, subqueries)).collect::<Result<Vec<_>>>()?;
            f.0.call(&values)
        }
        Expr::Outer(_, name) => Err(broken(&format!("the outer reference {name} has no value"))),
        // [ISO/IEC 9075:1992 §6.9 GR2] the first WHEN true gives the result,
        // the others not evaluated; else the ELSE.
        Expr::Case(whens, otherwise) => {
            for (cond, result) in whens {
                if eval_cond(cond, row, subqueries)? == Truth::True {
                    return eval(result, row, subqueries);
                }
            }
            eval(otherwise, row, subqueries)
        }
        Expr::Cast(x, to) => cast(eval(x, row, subqueries)?, to),
        Expr::Domain(x, d) => checked(eval(x, row, subqueries)?, d, subqueries),
        // [ISO/IEC 9075:1992 §7.11 GR1, GR2] the value of its one row, null
        // if it has no row.
        Expr::Subquery(s, at) => {
            let rows = subqueries.rows(s, &outer_values(s, row, subqueries)?, Some(2))?.first(2)?;
            match rows.as_slice() {
                [] => Ok(Value::Null),
                [one] => one.get(*at).cloned().ok_or_else(|| broken("a subquery gave a row shorter than its degree")),
                _ => fail(Code::Cardinality, "[ISO/IEC 9075:1992 §7.11 GR1] a <scalar subquery> or <row subquery> gives more than one row"),
            }
        }
    }
}

/// The values of `s`'s outer expressions for `row`.
/// [ISO/IEC 9075:1992 §6.10 GR13] `v`, CAST to the domain `d`, if none of
/// its constraints is false of it [§4.10.3].
fn checked(v: Value, d: &DomainChecks, subqueries: &dyn Subqueries) -> Result<Value> {
    let row = vec![v];
    for (name, cond) in &d.checks {
        if eval_cond(cond, &row, subqueries)? == Truth::False {
            return fail(
                Code::Constraint,
                format!("[ISO/IEC 9075:1992 §6.10 GR13] CHECK {name} of domain {} is false for a value CAST to it", d.domain),
            );
        }
    }
    Ok(row.into_iter().next().unwrap_or(Value::Null))
}

fn outer_values(s: &Subquery, row: &Row, subqueries: &dyn Subqueries) -> Result<Vec<Value>> {
    s.outer.iter().map(|e| eval(e, row, subqueries)).collect()
}

/// The values of `exprs` for `row`.
fn values(exprs: &[Expr], row: &Row, subqueries: &dyn Subqueries) -> Result<Vec<Value>> {
    exprs.iter().map(|e| eval(e, row, subqueries)).collect()
}

/// True if `holds`, else false.
fn truth(holds: bool) -> Truth {
    if holds { Truth::True } else { Truth::False }
}

/// [X3.135-1986 §5.18] Evaluates a `<search condition>` with three-valued
/// logic. Both sides of AND and OR are evaluated, so a run-time error is
/// never skipped. Its subqueries are run by `subqueries`.
///
/// # Errors
///
/// As for [`eval`], for any `<value expression>` in the condition; a
/// subquery's errors.
pub fn eval_cond(c: &Cond, row: &Row, subqueries: &dyn Subqueries) -> Result<Truth> {
    use Truth::{False, True, Unknown};
    Ok(match c {
        Cond::Const(t) => *t,
        // [X3.135-1986 §5.18 GR2] truth table for OR
        Cond::Or(a, b) => match (eval_cond(a, row, subqueries)?, eval_cond(b, row, subqueries)?) {
            (True, _) | (_, True) => True,
            (False, False) => False,
            _ => Unknown,
        },
        // [X3.135-1986 §5.18 GR2] truth table for AND
        Cond::And(a, b) => match (eval_cond(a, row, subqueries)?, eval_cond(b, row, subqueries)?) {
            (False, _) | (_, False) => False,
            (True, True) => True,
            _ => Unknown,
        },
        Cond::Not(a) => match eval_cond(a, row, subqueries)? {
            True => False,
            False => True,
            Unknown => Unknown,
        },
        // [ISO/IEC 9075:1992 §8.12 GR2, Table 4] IS is never unknown.
        Cond::Is(a, value, not) => truth((eval_cond(a, row, subqueries)? == *value) != *not),
        // [X3.135-1986 §5.11 GR2, GR3]
        Cond::Compare(a, op, b) => compared(&eval(a, row, subqueries)?, *op, &eval(b, row, subqueries)?),
        // [X3.135-1986 §5.15 GR2-GR4]: IS NULL is never unknown.
        Cond::IsNull(e, not) => truth(eval(e, row, subqueries)?.is_null() != *not),
        Cond::Like(e, pattern, escape) => {
            let escape = escape.as_ref().map(|x| eval(x, row, subqueries)).transpose()?;
            like_truth(&eval(e, row, subqueries)?, &eval(pattern, row, subqueries)?, escape)?
        }
        // [X3.135-1986 §5.17 GR2, GR3] true if and only if S is not empty.
        Cond::Exists(s) => truth(!subqueries.rows(s, &outer_values(s, row, subqueries)?, Some(1))?.first(1)?.is_empty()),
        Cond::Quantified(xs, op, q, s) => {
            let rows = subqueries.rows(s, &outer_values(s, row, subqueries)?, None)?;
            rows.quantified(&values(xs, row, subqueries)?, (*op, *q))?
        }
        Cond::Unique(s) => truth(subqueries.rows(s, &outer_values(s, row, subqueries)?, None)?.unique()?),
        Cond::Match { values: xs, unique, match_type, subquery } => {
            let rows = subqueries.rows(subquery, &outer_values(subquery, row, subqueries)?, None)?;
            matched(&values(xs, row, subqueries)?, (*unique, *match_type), rows.rows()?)?
        }
    })
}

/// [X3.135-1986 §5.11 GR2, GR3] "x op y".
pub(super) fn compared(x: &Value, op: CompOp, y: &Value) -> Truth {
    compare(x, y).map_or(Truth::Unknown, |ord| truth(op.holds(ord)))
}

/// [ISO/IEC 9075:1992 §8.2 GR7] "Rx op Ry", for rows of values of the same
/// degree: = true if each pair is equal, false if one is not; <> the
/// reverse; < true if the pairs are equal up to one that is less, false if
/// Rx >= Ry is true; and so on; else unknown. For one value it is "x op y"
/// \[GR1\].
pub fn compare_rows(xs: &[Value], op: CompOp, ys: &[Value]) -> Truth {
    let pairs = || xs.iter().zip(ys).map(|(x, y)| compare(x, y));
    let unequal = pairs().any(|o| o.is_some_and(std::cmp::Ordering::is_ne));
    let equal = pairs().all(|o| o.is_some_and(std::cmp::Ordering::is_eq));
    // Ordered at the first pair not equal, unless a null comes before it.
    let mut first = Some(std::cmp::Ordering::Equal);
    for o in pairs() {
        match o {
            None => {
                first = None;
                break;
            }
            Some(std::cmp::Ordering::Equal) => {}
            Some(decided) => {
                first = Some(decided);
                break;
            }
        }
    }
    match op {
        CompOp::Eq => decided((equal, unequal)),
        CompOp::Ne => decided((unequal, equal)),
        CompOp::Lt | CompOp::Gt | CompOp::Le | CompOp::Ge => first.map_or(Truth::Unknown, |ord| truth(op.holds(ord))),
    }
}

/// True if `holds`, false if `fails`, else unknown.
fn decided((holds, fails): (bool, bool)) -> Truth {
    if holds {
        Truth::True
    } else if fails {
        Truth::False
    } else {
        Truth::Unknown
    }
}

/// "R op q S", S the rows `rows`, as wide as R: [X3.135-1986 §5.16 GR2;
/// ISO/IEC 9075:1992 §8.7] for ALL and SOME, and for one row [X3.135-1986
/// §5.11 GR1, GR2].
///
/// # Errors
///
/// SQLCODE -811 if a comparison's subquery gives more than one row.
pub fn quantified(xs: &[Value], (op, quantifier): (CompOp, Quantifier), rows: impl IntoIterator<Item = Result<Row>>) -> Result<Truth> {
    let mut rows = rows.into_iter();
    if quantifier == Quantifier::One {
        let Some(only) = rows.next().transpose()? else { return Ok(Truth::Unknown) };
        if rows.next().transpose()?.is_some() {
            return fail(Code::Cardinality, "[X3.135-1986 §5.11 GR1] the <subquery> of a comparison gives more than one value");
        }
        return Ok(compare_rows(xs, op, &only));
    }
    // Whether a row gave true, false, unknown.
    let (mut some_true, mut some_false, mut some_unknown) = (false, false, false);
    for r in rows {
        match compare_rows(xs, op, &r?) {
            Truth::True => some_true = true,
            Truth::False => some_false = true,
            Truth::Unknown => some_unknown = true,
        }
    }
    Ok(match quantifier {
        // [GR2a, b, e] true if true for every row (or none), false if false for one.
        Quantifier::All => decided((!some_false && !some_unknown, some_false)),
        // [GR2c, d, e] true if true for one, false if false for every row (or none).
        Quantifier::Some | Quantifier::One => decided((some_true, !some_true && !some_unknown)),
    })
}

/// [ISO/IEC 9075:1992 §8.9 GR2] UNIQUE: true unless two of `rows`, neither
/// with a null, are equal; `first` tells whether a row's key is seen the
/// first time, and keeps it.
///
/// # Errors
///
/// The rows', and `first`'s.
pub(super) fn unique(rows: impl IntoIterator<Item = Result<Row>>, first: &mut dyn FnMut(Vec<u8>) -> Result<bool>) -> Result<bool> {
    for r in rows {
        let row = r?;
        if row.iter().any(Value::is_null) {
            continue;
        }
        if !first(norm_row(&row))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// [ISO/IEC 9075:1992 §8.10 GR2-GR4] "R MATCH \[UNIQUE\] \[PARTIAL | FULL\]
/// S", S the rows `rows`: with none, true if R has a null; with PARTIAL or
/// FULL, true if all of R is null, and with FULL false if some is; else
/// true if a row (a unique one, with UNIQUE) equals R, with PARTIAL in the
/// values of R not null.
///
/// # Errors
///
/// The rows'.
fn matched(xs: &[Value], (unique, match_type): (bool, MatchType), rows: impl IntoIterator<Item = Result<Row>>) -> Result<Truth> {
    let nulls = xs.iter().filter(|v| v.is_null()).count();
    if nulls == xs.len() && match_type != MatchType::Simple {
        return Ok(Truth::True);
    }
    if nulls > 0 && match_type != MatchType::Partial {
        return Ok(truth(match_type == MatchType::Simple));
    }
    let equal = |r: &Row| xs.iter().zip(r).all(|(x, y)| x.is_null() || compare(x, y) == Some(std::cmp::Ordering::Equal));
    let mut count = 0_usize;
    for r in rows {
        if equal(&r?) {
            count = count.saturating_add(1);
            // Past two, the answer is decided.
            if count > 1 {
                break;
            }
        }
    }
    Ok(truth(if unique { count == 1 } else { count > 0 }))
}

/// [X3.135-1986 §5.14 GR4, GR5] "x LIKE y": unknown if x, y, or the escape
/// character is null [ISO/IEC 9075:1992 §8.5 GR1, GR2]; an escape
/// character not of one character a data exception \[GR3a i\].
fn like_truth(x: &Value, pattern: &Value, escape: Option<Value>) -> Result<Truth> {
    let escape = match escape {
        None => None,
        Some(Value::Char(e)) => {
            let mut chars = e.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => {
                    return fail_with(
                        Code::Data,
                        SqlState::INVALID_ESCAPE_CHARACTER,
                        "[ISO/IEC 9075:1992 §8.5 GR3a i] the <escape character> is one character",
                    );
                }
            }
        }
        Some(Value::Null) => return Ok(Truth::Unknown),
        Some(Value::Exact(_) | Value::Approx(_) | Value::Bits(_) | Value::Datetime(_) | Value::Interval(_) | Value::Extension(_)) => {
            return fail_with(
                Code::Data,
                SqlState::INVALID_ESCAPE_CHARACTER,
                "[X3.135-1986 §5.14 SR3] the <escape character> is a character string",
            );
        }
    };
    Ok(match (x, pattern) {
        (Value::Null, _) | (_, Value::Null) => Truth::Unknown,
        (Value::Char(x), Value::Char(y)) => truth(like(x, y, escape)?),
        _ => return fail(Code::Data, "[X3.135-1986 §5.14 SR1, SR2] LIKE matches character strings"),
    })
}

// Unit tests: comparisons of rows of values (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn n(v: i128) -> Value {
        Value::Exact(Decimal::new(v, 0))
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.2 GR7] rows compare equal only if
    /// every pair does, unequal if one pair does not; in order by the first
    /// pair not equal, unknown if a null comes first.
    #[test]
    fn rows_compare_pairwise() {
        use Truth::{False, True, Unknown};
        let row = |a: Option<i128>, b: Option<i128>| vec![a.map_or(Value::Null, n), b.map_or(Value::Null, n)];
        assert_eq!(compare_rows(&row(Some(1), Some(2)), CompOp::Eq, &row(Some(1), Some(2))), True);
        assert_eq!(compare_rows(&row(None, Some(2)), CompOp::Eq, &row(Some(1), Some(3))), False);
        assert_eq!(compare_rows(&row(None, Some(2)), CompOp::Eq, &row(Some(1), Some(2))), Unknown);
        assert_eq!(compare_rows(&row(None, Some(2)), CompOp::Ne, &row(Some(1), Some(3))), True);
        assert_eq!(compare_rows(&row(Some(1), Some(2)), CompOp::Ne, &row(Some(1), Some(2))), False);
        assert_eq!(compare_rows(&row(Some(1), None), CompOp::Lt, &row(Some(2), Some(0))), True);
        assert_eq!(compare_rows(&row(None, Some(1)), CompOp::Lt, &row(Some(2), Some(0))), Unknown);
        assert_eq!(compare_rows(&row(Some(1), Some(2)), CompOp::Le, &row(Some(1), Some(2))), True);
        assert_eq!(compare_rows(&row(Some(2), Some(0)), CompOp::Le, &row(Some(1), Some(9))), False);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §8.9 GR2, §8.10 GR2-GR4] UNIQUE counts
    /// no row with a null; MATCH by its type.
    #[test]
    fn unique_and_match() {
        let rows_of = |rows: &[Row]| rows.iter().cloned().map(Ok).collect::<Vec<Result<Row>>>();
        let unique_of = |rows: &[Row]| {
            let mut seen = std::collections::HashSet::new();
            unique(rows_of(rows), &mut |key| Ok(seen.insert(key))).unwrap()
        };
        assert!(!unique_of(&[vec![n(1)], vec![n(1)]]));
        assert!(unique_of(&[vec![n(1), Value::Null], vec![n(1), Value::Null]]));
        let rows = [vec![n(1), n(2)], vec![n(1), n(3)]];
        let matches = |xs: &[Value], how: (bool, MatchType)| matched(xs, how, rows_of(&rows)).unwrap();
        let simple = (false, MatchType::Simple);
        assert_eq!(matches(&[n(1), Value::Null], simple), Truth::True);
        assert_eq!(matches(&[n(1), Value::Null], (false, MatchType::Full)), Truth::False);
        assert_eq!(matches(&[Value::Null, Value::Null], (false, MatchType::Full)), Truth::True);
        assert_eq!(matches(&[n(1), Value::Null], (false, MatchType::Partial)), Truth::True);
        assert_eq!(matches(&[n(1), Value::Null], (true, MatchType::Partial)), Truth::False);
        assert_eq!(matches(&[n(1), n(2)], (true, MatchType::Simple)), Truth::True);
        assert_eq!(matches(&[n(2), n(2)], simple), Truth::False);
    }
}
