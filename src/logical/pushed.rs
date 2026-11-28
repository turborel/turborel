//! \[DESIGN\] The conditions of a filter a scan is told (RULES.md D31,
//! `TableStorage::scan_where`), as the executor tells them and the planner
//! prices them (`TableStorage::scan_share`).

use std::sync::Arc;

use super::{Cond, Expr};
use crate::index::Probe;
use crate::storage::{Condition, Conjunct, Conjunction, Liked};
use crate::value::{CompOp, Value, constant_pattern, constant_prefix};

/// The conjuncts of `cond` comparing a column with a constant, as strings
/// compare padded or, under a collation of NO PAD that orders them as their
/// octets do, unpadded (`compared_unpadded`), testing it
/// IS NULL or IS NOT NULL, IN or NOT IN a list of constants, LIKE a
/// constant pattern (as the bounds of its prefix, and, of a pattern of no
/// `_`, itself, as NOT LIKE one is, RULES.md D1251), or an OR of these of
/// one column, if any, unless `cond` can raise an error: every row `cond` keeps
/// satisfies each, so a scan tells its engine (`TableStorage::scan_where`),
/// which may then leave out the rows they do not match, skip the blocks of
/// rows one cannot match, and count only the rows they all match as read.
pub fn pushed_conjuncts(cond: &Cond) -> Option<Conjunction> {
    // A condition that can raise an error must see every row: a row the
    // conjunct rejects may still raise it, and whether it does may not
    // depend on the plan, nor on which rows another transaction changed.
    if cond.can_raise() {
        return None;
    }
    let (mut found, mut own) = (Vec::new(), Vec::new());
    conjuncts(cond, &mut found, &mut own);
    if found.is_empty() {
        return None;
    }
    Some(Conjunction(found))
}

/// \[DESIGN\] (RULES.md D1266) Each conjunct of `cond`, an operand of its
/// ANDs, in their order, with the place among the conjuncts a scan is
/// told (`pushed_conjuncts`) of the one that is that conjunct itself,
/// true of exactly the rows it is true of: a comparison with a constant,
/// IS NULL or IS NOT NULL, IN or NOT IN constants, an OR of these of one
/// column, a LIKE or NOT LIKE of a constant pattern. None of a conjunct a
/// scan is not told, nor of the bounds of a LIKE's prefix, which hold of
/// strings the LIKE is false of, and none at all of a condition that can
/// raise an error, of which a scan is told nothing. A scan that says it
/// judged the conjunct at that place exactly has judged the filter's own
/// conjunct, which the filter then need not judge again.
pub fn told_conjuncts(cond: &Cond) -> Vec<(&Cond, Option<usize>)> {
    let (mut found, mut own) = (Vec::new(), Vec::new());
    conjuncts(cond, &mut found, &mut own);
    if cond.can_raise() {
        for (_, told) in &mut own {
            *told = None;
        }
    }
    own
}

/// The conjuncts of `cond` a scan is told (`pushed_conjuncts`), added to
/// `found`, and each operand of its ANDs added to `own` with the place in
/// `found` of the conjunct told that is the operand itself, if one is
/// (`told_conjuncts`).
fn conjuncts<'c>(cond: &'c Cond, found: &mut Vec<Conjunct>, own: &mut Vec<(&'c Cond, Option<usize>)>) {
    let itself = match cond {
        Cond::And(a, b) => {
            conjuncts(a, found, own);
            conjuncts(b, found, own);
            return;
        }
        // An `<in predicate>` of constants, as the binder writes it; else an
        // OR of conjuncts of one column.
        Cond::Or(..) => single(cond).or_else(|| alternatives(cond)),
        // NOT IN a list of constants, none null: a null in it leaves no row.
        // Or NOT LIKE a constant pattern.
        Cond::Not(inner) => in_list(inner)
            .filter(|(_, values)| !values.iter().any(Value::is_null))
            .map(|(column, values)| Conjunct::NotIn { column, values })
            .or_else(|| liked(inner, true)),
        // The bounds of its prefix first, which are not the LIKE itself.
        Cond::Like(Expr::Column(c), pattern, escape) => {
            found.extend(prefixed(*c, pattern, escape.as_ref()));
            liked(cond, false)
        }
        Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::In(..)
        | Cond::IsNull(..)
        | Cond::Const(_)
        | Cond::Is(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::Value(_) => single(cond),
    };
    own.push((cond, itself.as_ref().map(|_| found.len())));
    found.extend(itself);
}

/// `cond` as one conjunct of one column, if it is: a comparison with a
/// constant, padded or unpadded, IS NULL or IS NOT NULL, or IN a list of
/// constants.
fn single(cond: &Cond) -> Option<Conjunct> {
    if let Some((column, op, octets)) = compared_unpadded(cond) {
        return Some(Conjunct::Unpadded { column, op, octets: octets.to_vec() });
    }
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
pub fn disjuncts<'c>(cond: &'c Cond, out: &mut Vec<&'c Cond>) {
    if let Cond::Or(a, b) = cond {
        disjuncts(a, out);
        disjuncts(b, out);
    } else {
        out.push(cond);
    }
}

/// The conjuncts a scan is told of `column` LIKE `pattern` with `escape`:
/// the comparisons `like_bounds` gives.
fn prefixed(column: usize, pattern: &Expr, escape: Option<&Expr>) -> Vec<Conjunct> {
    let bound = |(op, value)| Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } });
    like_bounds(pattern, escape).into_iter().map(bound).collect()
}

/// \[DESIGN\] (RULES.md D1251) The conjunct a scan is told of `cond`, a
/// LIKE of a column, or, if `not`, of NOT of it: of a constant pattern
/// and escape that raise nothing and are not null (`constant_pattern`),
/// the pattern of no `_` and of pieces short enough for a machine over
/// octets (`Liked::of`). A LIKE under a collation that folds or
/// contracts characters is not of a column as it is
/// (`binder/collation.rs`), so is none.
fn liked(cond: &Cond, not: bool) -> Option<Conjunct> {
    let Cond::Like(Expr::Column(column), Expr::Const(pattern), escape) = cond else { return None };
    let escape = match escape {
        None => None,
        Some(Expr::Const(e)) => Some(e),
        Some(_) => return None,
    };
    let like = Liked::of(&constant_pattern(pattern, escape)??, not)?;
    Some(Conjunct::Like { column: *column, like: Arc::new(like) })
}

/// [X3.135-1986 §5.14] The bounds of the values LIKE the constant
/// `pattern`, with the constant `escape` if any: above the characters its
/// pattern begins with, the last made one less, and below them, the last
/// made one more. Every string that begins with them is between, as it is
/// first apart from either at that character, whatever follows, spaces
/// padding it or not [X3.135-1986 §5.11 GR5]; so are some that do not
/// begin with them, and a filter still judges the LIKE (RULES.md D15).
/// None if they are none, a bound has no character, or the pattern or
/// escape is not a constant.
pub fn like_bounds(pattern: &Expr, escape: Option<&Expr>) -> Vec<(CompOp, Value)> {
    let escape = match escape {
        None => None,
        Some(Expr::Const(e)) => Some(e),
        Some(_) => return Vec::new(),
    };
    let Expr::Const(pattern) = pattern else { return Vec::new() };
    let Some(prefix) = constant_prefix(pattern, escape) else { return Vec::new() };
    let moved = |by: i64| {
        let mut chars: Vec<char> = prefix.chars().collect();
        let last = chars.pop()?;
        let code = i64::from(u32::from(last)).checked_add(by)?;
        chars.push(char::from_u32(u32::try_from(code).ok()?)?);
        Some(Value::Char(chars.into_iter().collect()))
    };
    [moved(-1).map(|v| (CompOp::Gt, v)), moved(1).map(|v| (CompOp::Lt, v))].into_iter().flatten().collect()
}

/// The column and constant `cond` compares by `op`, either way round, the
/// operator turned around if the constant is first.
pub fn compared_with_constant(cond: &Cond) -> Option<(usize, CompOp, &Value)> {
    match cond {
        Cond::Compare(Expr::Column(c), op, Expr::Const(v)) => Some((*c, *op, v)),
        // "constant op column" is "column op' constant".
        Cond::Compare(Expr::Const(v), op, Expr::Column(c)) => Some((*c, op.flip(), v)),
        Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Is(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => None,
    }
}

/// [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) The column, operator and
/// constant's octets of `cond` if it compares a column of character
/// strings with a constant under a collation of NO PAD that orders strings
/// as their octets do (`CollationRef::is_octets`): the binder marks the
/// column with the collation and makes the constant its sort key, the
/// text's octets, so the comparison is of the column's text in place,
/// unpadded, either way round.
pub fn compared_unpadded(cond: &Cond) -> Option<(usize, CompOp, &[u8])> {
    let Cond::Compare(a, op, b) = cond else { return None };
    if let Some(found) = marked(a, b) {
        return Some((found.0, *op, found.1));
    }
    marked(b, a).map(|(column, octets)| (column, op.flip(), octets))
}

/// The column `key` marks with such a collation and the octets of
/// `constant`, its sort key, if they are so.
fn marked<'c>(key: &Expr, constant: &'c Expr) -> Option<(usize, &'c [u8])> {
    let (Expr::SortKey(of, collation), Expr::Const(Value::Binary(octets))) = (key, constant) else { return None };
    let Expr::Column(column) = &**of else { return None };
    collation.is_octets().then(|| (*column, octets.bytes()))
}

/// The column and constants of `cond` if it is an OR of equalities of one
/// column with constants, as an `<in predicate>` of constants is
/// (`binder/predicates.rs`), or their set (`logical/sets.rs`).
pub fn in_list(cond: &Cond) -> Option<(usize, Vec<Value>)> {
    if let Cond::In(Expr::Column(c), set) = cond {
        return Some((*c, set.values().to_vec()));
    }
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
    /// with, beside the LIKE itself: every string beginning with them
    /// between, a tab after them and spaces padding them too; none else; a
    /// pattern beginning with `%` tells no bounds; a pattern whose escape
    /// has no partitioning may raise an error, so nothing is told.
    #[test]
    fn a_like_is_told_as_the_bounds_of_its_prefix() {
        let bounds = |cond: &Cond| {
            let told = pushed_conjuncts(cond).map(|c| c.0).unwrap_or_default();
            told.iter().filter(|c| matches!(c, Conjunct::Condition(_))).count()
        };
        let prefix = like("abc%", None);
        assert_eq!(bounds(&prefix), 2);
        for matching in ["abc", "abc\t", "abcz", "abc  ", "abc\u{10FFFF}"] {
            assert!(passes(&prefix, Value::Char(matching.into())), "{matching:?}");
        }
        for not in ["abb", "abd", "ab", "b"] {
            assert!(!passes(&prefix, Value::Char(not.into())), "{not:?}");
        }
        assert_eq!(bounds(&like("%abc", None)), 0, "no prefix");
        assert_eq!(bounds(&like("a_c%", None)), 2, "the characters before a `_`");
        assert_eq!(bounds(&like("a!%b%", Some("!"))), 2, "an escaped % is the prefix's");
        assert!(passes(&like("a!%b%", Some("!")), Value::Char("a%bc".into())));
        assert!(!passes(&like("a!%b%", Some("!")), Value::Char("abc".into())));
        assert!(like("a!x%", Some("!")).can_raise(), "no partitioning");
        assert!(pushed_conjuncts(&like("a!x%", Some("!"))).is_none());
        assert!(!like("abc%", None).can_raise());
    }

    /// Verifies: [X3.135-1986 §5.14 GR2a, GR4, GR5] (RULES.md D1251) a
    /// LIKE of a column and a constant pattern of no `_` is told a scan as
    /// itself, and NOT of one as NOT LIKE, each true of the strings the
    /// predicate is true of and of no null, with an ESCAPE that makes
    /// `%` a character of the pattern too, and of a pattern of no `%`, of
    /// a first piece alone and of `%` alone.
    #[test]
    fn a_like_of_a_constant_pattern_is_told_itself() {
        let not = |cond: Cond| Cond::Not(Box::new(cond));
        let char_of = |s: &str| Value::Char(s.into());
        // Whether `cond` is told as one LIKE of column 0, and whether NOT.
        let negated = |cond: &Cond| {
            let all = conjuncts_told(cond);
            let [Conjunct::Like { column: 0, like }] = all.as_slice() else { panic!("not one LIKE: {all:?}") };
            like.not()
        };
        let within = like("%b%d%", None);
        assert!(!negated(&within));
        assert!(passes(&within, char_of("abcd")) && !passes(&within, char_of("adcb")) && !passes(&within, Value::Null));
        let without = not(like("%b%d%", None));
        assert!(negated(&without));
        assert!(!passes(&without, char_of("abcd")) && passes(&without, char_of("adcb")) && !passes(&without, Value::Null));
        let escaped = like("%5!%%", Some("!"));
        assert_eq!(likes_told(&escaped), 1);
        assert!(passes(&escaped, char_of("a 5% b")) && !passes(&escaped, char_of("a 50 b")));
        assert_eq!((likes_told(&like("abc", None)), likes_told(&like("ab%", None)), likes_told(&like("%", None))), (1, 1, 1));
    }

    /// Verifies: [X3.135-1986 §5.14 GR2b] (RULES.md D1251) a scan is told
    /// no LIKE of a pattern with `_`, of pieces of more octets than a
    /// machine has states for, of a null pattern or escape, of a pattern
    /// or an escape not a constant, of a LIKE of an expression, as one
    /// under a collation that folds is, nor of one that may raise an
    /// error.
    #[test]
    fn a_like_with_no_machine_is_not_told() {
        let not = |cond: Cond| Cond::Not(Box::new(cond));
        let long = |octets: usize| like(&format!("%{}%", "x".repeat(octets)), None);
        assert_eq!((likes_told(&long(253)), likes_told(&long(254)), likes_told(&not(long(254)))), (1, 0, 0), "past a machine's states");
        for none in [like("a_c", None), like("%_%", None), not(like("_b%", None))] {
            assert_eq!(likes_told(&none), 0, "an arbitrary character: {none:?}");
        }
        let null = Expr::Const(Value::Null);
        let of_column = |pattern: Expr, escape: Option<Expr>| Cond::Like(Expr::Column(0), pattern, escape);
        assert_eq!(likes_told(&of_column(null.clone(), None)), 0, "a null pattern");
        assert_eq!(likes_told(&of_column(text("%a%"), Some(null))), 0, "a null escape");
        assert_eq!(likes_told(&of_column(Expr::Column(1), None)), 0, "a column's pattern");
        assert_eq!(likes_told(&of_column(text("%a%"), Some(Expr::Column(1)))), 0, "a column's escape");
        let folded = Cond::Like(Expr::Neg(Box::new(Expr::Column(0))), text("%a%"), None);
        assert_eq!((likes_told(&folded), likes_told(&not(folded))), (0, 0), "of an expression");
        assert!(conjuncts_told(&like("%a!x%", Some("!"))).is_empty(), "no partitioning");
    }

    /// The conjuncts a scan is told of `cond`.
    fn conjuncts_told(cond: &Cond) -> Vec<Conjunct> {
        pushed_conjuncts(cond).map(|c| c.0).unwrap_or_default()
    }

    /// How many LIKEs a scan is told of `cond`.
    fn likes_told(cond: &Cond) -> usize {
        conjuncts_told(cond).iter().filter(|c| matches!(c, Conjunct::Like { .. })).count()
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

    /// Verifies: [DESIGN] RULES.md D1266 each conjunct of a condition
    /// knows the place of the conjunct a scan is told that is it: a
    /// comparison, a LIKE after the two bounds of its prefix, which are
    /// not it, NOT IN, an OR of one column, IS NULL and NOT LIKE, each
    /// counted on from those told before; a LIKE with `_`, told as its
    /// bounds alone, an OR of two columns and a comparison of two
    /// columns have none; and of a condition that can raise an error,
    /// which is told nothing, none has.
    #[test]
    fn each_conjunct_knows_the_place_of_the_one_told_that_is_it() {
        let eq = |c: usize, v: i128| Cond::Compare(Expr::Column(c), CompOp::Eq, Expr::Const(int(v)));
        let or = |a: Cond, b: Cond| Cond::Or(Box::new(a), Box::new(b));
        let and = |a: Cond, b: Cond| Cond::And(Box::new(a), Box::new(b));
        let not = |c: Cond| Cond::Not(Box::new(c));
        let below = |v: i128| Cond::Compare(Expr::Column(1), CompOp::Lt, Expr::Const(int(v)));
        let conjuncts = [
            eq(1, 5),
            like("abc%", None),
            like("a_c%", None),
            not(or(eq(1, 1), eq(1, 2))),
            or(below(3), Cond::IsNull(Expr::Column(1), false)),
            or(eq(0, 1), eq(1, 1)),
            Cond::IsNull(Expr::Column(1), true),
            Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Column(1)),
            not(like("%z%", None)),
        ];
        let cond = conjuncts.iter().cloned().reduce(and).unwrap();
        let places: Vec<Option<usize>> = told_conjuncts(&cond).into_iter().map(|(_, place)| place).collect();
        assert_eq!(places, [Some(0), Some(3), None, Some(6), Some(7), None, Some(8), None, Some(9)]);
        let told = conjuncts_told(&cond);
        assert_eq!(told.len(), 10, "{told:?}");
        assert!(matches!((&told[1], &told[2], &told[3]), (Conjunct::Condition(_), Conjunct::Condition(_), Conjunct::Like { .. })));
        assert!(matches!((&told[4], &told[5]), (Conjunct::Condition(_), Conjunct::Condition(_))), "the bounds of `a_c%` alone");
        assert!(matches!((&told[6], &told[7], &told[8]), (Conjunct::NotIn { .. }, Conjunct::Any(_), Conjunct::Null { .. })));
        assert!(matches!(&told[9], Conjunct::Like { like, .. } if like.not()));
        let each: Vec<&Cond> = told_conjuncts(&cond).into_iter().map(|(conjunct, _)| conjunct).collect();
        assert_eq!(format!("{each:?}"), format!("{:?}", conjuncts.iter().collect::<Vec<_>>()), "every operand of the ANDs, in order");
        let raising = and(eq(1, 5), like("a!x%", Some("!")));
        assert!(raising.can_raise());
        assert_eq!(told_conjuncts(&raising).into_iter().map(|(_, place)| place).collect::<Vec<_>>(), [None, None]);
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) a
    /// comparison of a column marked with a collation of NO PAD by octets
    /// with a constant's key is told a scan as the column's octets compared
    /// unpadded, either way round, and an OR of them as any; a prefix is
    /// below, a trailing space tells apart; one marked with another
    /// collation, or of an expression, is told nothing.
    #[test]
    fn a_comparison_unpadded_is_told_as_octets() {
        let octets = crate::value::collation::unpadded_scalar();
        let key = |e: Expr| Expr::SortKey(Box::new(e), octets.clone());
        let constant = |s: &str| Expr::Const(Value::Binary(crate::value::Binary::of(s.as_bytes())));
        let below = Cond::Compare(key(Expr::Column(0)), CompOp::Lt, constant("a "));
        assert_eq!(compared_unpadded(&below), Some((0, CompOp::Lt, b"a ".as_slice())));
        let told = pushed_conjuncts(&below).unwrap();
        assert!(matches!(told.conjuncts(), [Conjunct::Unpadded { column: 0, op: CompOp::Lt, .. }]), "{told:?}");
        let char_of = |s: &str| Value::Char(s.into());
        assert!(passes(&below, char_of("a")) && passes(&below, char_of("a\t")) && passes(&below, char_of("")));
        assert!(!passes(&below, char_of("a ")) && !passes(&below, char_of("ab")) && !passes(&below, Value::Null));
        let flipped = Cond::Compare(constant("a"), CompOp::Lt, key(Expr::Column(0)));
        assert_eq!(compared_unpadded(&flipped), Some((0, CompOp::Gt, b"a".as_slice())));
        assert!(passes(&flipped, char_of("a ")) && !passes(&flipped, char_of("a")));
        let equal = |s: &str| Cond::Compare(key(Expr::Column(0)), CompOp::Eq, constant(s));
        let either = Cond::Or(Box::new(equal("a")), Box::new(equal("b ")));
        let any = pushed_conjuncts(&either).unwrap();
        let [Conjunct::Any(each)] = any.conjuncts() else { panic!("not an OR of conjuncts: {any:?}") };
        assert_eq!(each.len(), 2);
        assert!(passes(&either, char_of("b ")) && !passes(&either, char_of("b")) && !passes(&either, char_of("a ")));
        let padded = crate::value::collation::CollationRef { pad: crate::value::collation::Pad::Space, ..octets.clone() };
        let other = Cond::Compare(Expr::SortKey(Box::new(Expr::Column(0)), padded), CompOp::Eq, constant("a"));
        assert!(compared_unpadded(&other).is_none() && pushed_conjuncts(&other).is_none());
        let of_expression = Cond::Compare(key(Expr::Neg(Box::new(Expr::Column(0)))), CompOp::Eq, constant("a"));
        assert!(compared_unpadded(&of_expression).is_none() && pushed_conjuncts(&of_expression).is_none());
        let of_text = Cond::Compare(key(Expr::Column(0)), CompOp::Eq, Expr::Const(char_of("a")));
        assert!(compared_unpadded(&of_text).is_none());
    }
}
