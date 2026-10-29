//! \[DESIGN\] The conditions of a filter a scan is told (RULES.md D31,
//! `TableStorage::scan_where`), as the executor tells them and the planner
//! prices them (`TableStorage::scan_share`). Split from exec.rs (RULES.md
//! R-SIZE-2).

use super::{Cond, Expr};
use crate::index::Probe;
use crate::storage::{Condition, Conjunct, Conjunction};
use crate::value::{CompOp, Value, constant_prefix};

/// The conjuncts of `cond` comparing a column with a constant, testing it
/// IS NULL or IS NOT NULL, or IN a list of constants, if any, unless `cond`
/// can raise an error: every row `cond` keeps satisfies each, so a scan
/// tells its engine (`TableStorage::scan_where`), which may then leave out
/// the rows they do not match, skip the frozen chunks one cannot match, and
/// count only the rows they all match as read.
pub fn pushed_conjuncts(cond: &Cond) -> Option<Conjunction> {
    // A condition that can raise an error must see every row: a row the
    // conjunct rejects may still raise it, and whether it does may not
    // depend on the plan, nor on which rows another transaction changed.
    if cond.can_raise() {
        return None;
    }
    let mut found = Vec::new();
    conjuncts(cond, &mut found);
    if found.is_empty() {
        return None;
    }
    Some(Conjunction(found))
}

/// The conjuncts of `cond` a scan is told (`pushed_conjuncts`).
fn conjuncts(cond: &Cond, found: &mut Vec<Conjunct>) {
    match cond {
        Cond::And(a, b) => {
            conjuncts(a, found);
            conjuncts(b, found);
        }
        // An `<in predicate>` of constants, as the binder writes it; else an
        // OR of conjuncts of one column.
        Cond::Or(..) => found.extend(single(cond).or_else(|| alternatives(cond))),
        // NOT IN a list of constants, none null: a null in it leaves no row.
        Cond::Not(inner) => found.extend(
            in_list(inner)
                .filter(|(_, values)| !values.iter().any(Value::is_null))
                .map(|(column, values)| Conjunct::NotIn { column, values }),
        ),
        Cond::Like(Expr::Column(c), Expr::Const(pattern), escape) => found.extend(prefixed(*c, pattern, escape.as_ref())),
        Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::Const(_)
        | Cond::Is(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => found.extend(single(cond)),
    }
}

/// `cond` as one conjunct of one column, if it is: a comparison with a
/// constant, IS NULL or IS NOT NULL, or IN a list of constants.
fn single(cond: &Cond) -> Option<Conjunct> {
    if let Some((column, op, value)) = compared_with_constant(cond) {
        return Some(Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value: value.clone() } }));
    }
    if let Cond::IsNull(Expr::Column(column), not) = cond {
        return Some(Conjunct::Null { column: *column, not: *not });
    }
    in_list(cond).map(|(column, values)| Conjunct::In { column, values })
}

/// `cond`, an OR, as the conjunct any of its operands is true of, if each
/// is one conjunct (`single`) of one column, the same.
fn alternatives(cond: &Cond) -> Option<Conjunct> {
    let mut operands = Vec::new();
    disjuncts(cond, &mut operands);
    let each: Vec<Conjunct> = operands.into_iter().map(single).collect::<Option<_>>()?;
    let first = each.first()?.columns();
    (first.len() == 1 && each.iter().all(|c| c.columns() == first)).then_some(Conjunct::Any(each))
}

/// The operands of the ORs of `cond`.
fn disjuncts<'c>(cond: &'c Cond, out: &mut Vec<&'c Cond>) {
    if let Cond::Or(a, b) = cond {
        disjuncts(a, out);
        disjuncts(b, out);
    } else {
        out.push(cond);
    }
}

/// [X3.135-1986 §5.14] The bounds of the values of `column` LIKE the
/// constant `pattern`, with `escape`: above the characters its pattern
/// begins with, the last made one less, and below them, the last made one
/// more. Every string that begins with them is between, as it is first
/// apart from either at that character, whatever follows, spaces padding
/// it or not [X3.135-1986 §5.11 GR5]. None if they are none, or a bound has
/// no character.
fn prefixed(column: usize, pattern: &Value, escape: Option<&Expr>) -> Vec<Conjunct> {
    let escape = match escape {
        None => None,
        Some(Expr::Const(e)) => Some(e),
        Some(_) => return Vec::new(),
    };
    let Some(prefix) = constant_prefix(pattern, escape) else { return Vec::new() };
    let moved = |by: i64| {
        let mut chars: Vec<char> = prefix.chars().collect();
        let last = chars.pop()?;
        let code = i64::from(u32::from(last)).checked_add(by)?;
        chars.push(char::from_u32(u32::try_from(code).ok()?)?);
        Some(Value::Char(chars.into_iter().collect()))
    };
    let bound = |op: CompOp, value: Value| Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } });
    [moved(-1).map(|v| bound(CompOp::Gt, v)), moved(1).map(|v| bound(CompOp::Lt, v))].into_iter().flatten().collect()
}

/// The column and constant `cond` compares by `op`, either way round, the
/// operator turned around if the constant is first.
pub fn compared_with_constant(cond: &Cond) -> Option<(usize, CompOp, &Value)> {
    match cond {
        Cond::Compare(Expr::Column(c), op, Expr::Const(v)) => Some((*c, *op, v)),
        // "constant op column" is "column op' constant".
        Cond::Compare(Expr::Const(v), op, Expr::Column(c)) => Some((*c, op.flip(), v)),
        Cond::Compare(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => None,
    }
}

/// The column and constants of `cond` if it is an OR of equalities of one
/// column with constants, as an `<in predicate>` of constants is
/// (`binder/predicates.rs`).
pub fn in_list(cond: &Cond) -> Option<(usize, Vec<Value>)> {
    if let Cond::Or(a, b) = cond {
        let (c, mut list) = in_list(a)?;
        let (d, more) = in_list(b)?;
        list.extend(more);
        return (c == d).then_some((c, list));
    }
    let (c, op, v) = compared_with_constant(cond)?;
    (op == CompOp::Eq).then(|| (c, vec![v.clone()]))
}

// Unit tests: the conjuncts a scan is told, of LIKE, NOT IN and OR
// (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    fn text(s: &str) -> Expr {
        Expr::Const(Value::Char(s.into()))
    }

    fn like(pattern: &str, escape: Option<&str>) -> Cond {
        Cond::Like(Expr::Column(0), text(pattern), escape.map(text))
    }

    /// Whether a row of `value` in column 0 passes what `cond` tells a scan.
    fn passes(cond: &Cond, value: Value) -> bool {
        pushed_conjuncts(cond).is_none_or(|c| c.matches(&vec![value, int(0)]))
    }

    /// Verifies: [X3.135-1986 §5.14 GR5, §5.11 GR5] a LIKE of a constant
    /// pattern is told a scan as the bounds of the characters it begins
    /// with: every string beginning with them between, a tab after them
    /// and spaces padding them too; none else; a pattern beginning with `%`
    /// tells nothing; a pattern whose escape has no partitioning may raise
    /// an error, so nothing is told.
    #[test]
    fn a_like_is_told_as_the_bounds_of_its_prefix() {
        let prefix = like("abc%", None);
        assert_eq!(pushed_conjuncts(&prefix).map(|c| c.0.len()), Some(2));
        for matching in ["abc", "abc\t", "abcz", "abc  ", "abc\u{10FFFF}"] {
            assert!(passes(&prefix, Value::Char(matching.into())), "{matching:?}");
        }
        for not in ["abb", "abd", "ab", "b"] {
            assert!(!passes(&prefix, Value::Char(not.into())), "{not:?}");
        }
        assert!(pushed_conjuncts(&like("%abc", None)).is_none(), "no prefix");
        assert_eq!(pushed_conjuncts(&like("a!%b%", Some("!"))).map(|c| c.0.len()), Some(2), "an escaped % is the prefix's");
        assert!(passes(&like("a!%b%", Some("!")), Value::Char("a%bc".into())));
        assert!(!passes(&like("a!%b%", Some("!")), Value::Char("abc".into())));
        assert!(like("a!x%", Some("!")).can_raise(), "no partitioning");
        assert!(pushed_conjuncts(&like("a!x%", Some("!"))).is_none());
        assert!(!like("abc%", None).can_raise());
    }

    /// Verifies: [X3.135-1986 §5.13, §5.18] NOT IN a list of constants is
    /// told a scan, unless a null is among them; an OR of conjuncts of one
    /// column is told as any of them, of two columns not at all.
    #[test]
    fn not_in_and_or_are_told_of_one_column() {
        let eq = |c: usize, v: i128| Cond::Compare(Expr::Column(c), CompOp::Eq, Expr::Const(int(v)));
        let or = |a: Cond, b: Cond| Cond::Or(Box::new(a), Box::new(b));
        let not_in = Cond::Not(Box::new(or(eq(0, 1), eq(0, 2))));
        let told = pushed_conjuncts(&not_in).map(|c| c.0).unwrap_or_default();
        assert!(matches!(told.as_slice(), [Conjunct::NotIn { .. }]));
        assert!(passes(&not_in, int(3)) && !passes(&not_in, int(2)) && !passes(&not_in, Value::Null));
        let with_null = Cond::Not(Box::new(or(eq(0, 1), Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Null)))));
        assert!(pushed_conjuncts(&with_null).is_none(), "a null in the list");
        let below = Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Const(int(10)));
        let above = Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Const(int(90)));
        let either = or(or(below.clone(), above), Cond::IsNull(Expr::Column(0), false));
        assert!(passes(&either, int(5)) && passes(&either, int(95)) && passes(&either, Value::Null) && !passes(&either, int(50)));
        assert!(pushed_conjuncts(&or(below, eq(1, 3))).is_none(), "two columns");
    }
}
