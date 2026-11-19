//! The operators of XQuery typed [XQueryFS §4.4, §4.5, Appendix B.2]:
//! arithmetic and value and general comparisons, after atomization
//! [XQueryFS §7.2.6] and `fs:convert-operand` [XQueryFS §7.1.1], each pair
//! of atomic types the operands may be of typed apart [XQueryFS Appendix
//! B.2], by the operator mapping of [XQuery Appendix B.2] and the type
//! promotion of [XQuery Appendix B.1].
//!
//! \[DESIGN\] RULES.md D106: \[XQueryFS\] Appendix B.2 restates XQuery's
//! operator mapping but leaves rows out (`xs:duration`'s equality, `idiv`
//! of numbers but integers, `xs:anyURI`); the evaluation follows XQuery's,
//! so the static typing does too. `fs:convert-operand`'s static rules type
//! an untyped operand and a numeric one; another atomic operand it returns
//! unchanged, as its dynamic semantics says [XQueryFS §7.1.1].

use super::super::ast::Comparator;
use super::super::plan::Arith;
use super::type_error;
use super::types::{Atom, Item, Type, prime, quantifier};
use crate::error::Result;
use crate::xml::{AtomicType, Primitive};

/// An operand's atomic types: each alternative of a union of optional
/// atomic types [XQueryFS Appendix B.2], none of the empty type; and
/// whether it may be the empty sequence.
pub struct Split {
    /// The atomic types, none if the operand is `empty`.
    pub atoms: Vec<Atom>,
    /// Whether the operand may be the empty sequence.
    pub optional: bool,
}

/// `t` as a union of optional atomic types [XQueryFS Appendix B.2], an
/// `fs:numeric` one each of its three; none if it is not one, as of more
/// than one item or of a node.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn split(t: &Type) -> Result<Option<Split>> {
    let q = quantifier(t)?;
    if q.many() {
        return Ok(None);
    }
    let mut atoms = Vec::new();
    for item in prime(t)? {
        match item {
            Item::Atomic(Atom::Numeric) => atoms.extend([AtomicType::Decimal, AtomicType::Float, AtomicType::Double].map(Atom::Of)),
            Item::Atomic(a) => atoms.push(a),
            Item::Element(_) | Item::Attribute(_) | Item::Document(_) | Item::Text | Item::Comment | Item::Pi => return Ok(None),
        }
    }
    Ok(Some(Split { optional: q.zero(), atoms }))
}

/// What `fs:convert-operand` makes of an atomic operand of type `a`, the
/// other of type `other` [XQueryFS §7.1.1]: an untyped one a string if
/// the other is untyped, a double if it is numeric, else of the other's
/// type; any other unchanged.
#[must_use]
pub fn convert(a: &Atom, other: &Atom) -> Atom {
    if *a != Atom::Of(AtomicType::UntypedAtomic) {
        return a.clone();
    }
    if *other == Atom::Of(AtomicType::UntypedAtomic) {
        Atom::Of(AtomicType::String)
    } else if *other == Atom::Numeric || rank(other).is_some() {
        Atom::Of(AtomicType::Double)
    } else {
        other.clone()
    }
}

/// [XQuery Appendix B.2] The numeric types, in the order the type of an
/// operator of numbers is the first of that both operands convert to.
const NUMERIC: [AtomicType; 4] = [AtomicType::Integer, AtomicType::Decimal, AtomicType::Float, AtomicType::Double];

/// Where `a` ranks among `NUMERIC`, by the primitive type it, or the
/// built-in type a user-defined one restricts, is derived from; none of a
/// type that is no number.
#[must_use]
pub fn rank(a: &Atom) -> Option<usize> {
    let t = a.built_in()?;
    NUMERIC.iter().position(|n| n.primitive() == t.primitive())
}

/// The type of `NUMERIC` at `at`.
#[must_use]
pub fn numeric_at(at: usize) -> AtomicType {
    NUMERIC.get(at).copied().unwrap_or(AtomicType::Double)
}

/// The primitive type a row of the operator mapping matches `a` by
/// [XQuery Appendix B.2]: of `xs:anyURI` `xs:string`, to which it is
/// promoted [XQuery Appendix B.1]; of a user-defined type, the built-in
/// type's it restricts, as operators take its values [XQuery §2.5.4.2];
/// none of `xdt:anyAtomicType` and `fs:numeric`.
fn primitive(a: &Atom) -> Option<Primitive> {
    let t = a.built_in()?;
    let p = t.primitive();
    Some(if p == Primitive::AnyUri { Primitive::String } else { p })
}

/// An operand of a row of [XQuery Appendix B.2].
#[derive(Clone, Copy)]
enum Operand {
    /// Of this primitive type or one derived from it.
    Of(Primitive),
    /// numeric.
    Numeric,
}

/// The rows of [XQuery Appendix B.2] of dates, times and durations, each
/// its operator, operands and type.
const TEMPORAL: [(Arith, Operand, Operand, AtomicType); 24] = {
    use AtomicType as T;
    use Operand::{Numeric as N, Of};
    use Primitive::{Date as D, DateTime as Dt, DayTimeDuration as Dd, Time as Ti, YearMonthDuration as Ym};
    [
        (Arith::Add, Of(D), Of(Ym), T::Date),
        (Arith::Add, Of(Ym), Of(D), T::Date),
        (Arith::Add, Of(D), Of(Dd), T::Date),
        (Arith::Add, Of(Dd), Of(D), T::Date),
        (Arith::Add, Of(Ti), Of(Dd), T::Time),
        (Arith::Add, Of(Dd), Of(Ti), T::Time),
        (Arith::Add, Of(Dt), Of(Ym), T::DateTime),
        (Arith::Add, Of(Ym), Of(Dt), T::DateTime),
        (Arith::Add, Of(Dt), Of(Dd), T::DateTime),
        (Arith::Add, Of(Dd), Of(Dt), T::DateTime),
        (Arith::Add, Of(Ym), Of(Ym), T::YearMonthDuration),
        (Arith::Add, Of(Dd), Of(Dd), T::DayTimeDuration),
        (Arith::Subtract, Of(D), Of(D), T::DayTimeDuration),
        (Arith::Subtract, Of(D), Of(Ym), T::Date),
        (Arith::Subtract, Of(D), Of(Dd), T::Date),
        (Arith::Subtract, Of(Ti), Of(Ti), T::DayTimeDuration),
        (Arith::Subtract, Of(Ti), Of(Dd), T::Time),
        (Arith::Subtract, Of(Dt), Of(Dt), T::DayTimeDuration),
        (Arith::Subtract, Of(Dt), Of(Ym), T::DateTime),
        (Arith::Subtract, Of(Dt), Of(Dd), T::DateTime),
        (Arith::Subtract, Of(Ym), Of(Ym), T::YearMonthDuration),
        (Arith::Subtract, Of(Dd), Of(Dd), T::DayTimeDuration),
        (Arith::Multiply, Of(Ym), N, T::YearMonthDuration),
        (Arith::Multiply, N, Of(Ym), T::YearMonthDuration),
    ]
};

/// More rows of [XQuery Appendix B.2] of durations.
const DURATIONS: [(Arith, Operand, Operand, AtomicType); 6] = {
    use AtomicType as T;
    use Operand::{Numeric as N, Of};
    use Primitive::{DayTimeDuration as Dd, YearMonthDuration as Ym};
    [
        (Arith::Multiply, Of(Dd), N, T::DayTimeDuration),
        (Arith::Multiply, N, Of(Dd), T::DayTimeDuration),
        (Arith::Divide, Of(Ym), N, T::YearMonthDuration),
        (Arith::Divide, Of(Dd), N, T::DayTimeDuration),
        (Arith::Divide, Of(Ym), Of(Ym), T::Decimal),
        (Arith::Divide, Of(Dd), Of(Dd), T::Decimal),
    ]
};

/// Whether `a` matches row operand `o`.
fn matches(o: Operand, a: &Atom) -> bool {
    match o {
        Operand::Of(p) => primitive(a) == Some(p),
        Operand::Numeric => rank(a).is_some(),
    }
}

/// [XQuery Appendix B.2] The type of `op` of operands of `a` and `b`, if
/// the operator mapping has a row of them: of two numbers the first
/// numeric type both convert to, `xs:decimal` of `div` of two integers
/// and `xs:integer` of `idiv`.
#[must_use]
pub fn arithmetic(op: Arith, a: &Atom, b: &Atom) -> Option<AtomicType> {
    if let (Some(x), Some(y)) = (rank(a), rank(b)) {
        let wider = x.max(y);
        return Some(match op {
            Arith::IntegerDivide => AtomicType::Integer,
            Arith::Divide => numeric_at(wider.max(1)),
            Arith::Add | Arith::Subtract | Arith::Multiply | Arith::Modulo => numeric_at(wider),
        });
    }
    TEMPORAL.iter().chain(DURATIONS.iter()).find(|(o, x, y, _)| *o == op && matches(*x, a) && matches(*y, b)).map(|r| r.3)
}

/// [XQuery Appendix B.2] The primitive types `lt`, `le`, `gt` and `ge`
/// compare, numbers aside.
const ORDERED: [Primitive; 7] = [
    Primitive::Boolean,
    Primitive::String,
    Primitive::Date,
    Primitive::Time,
    Primitive::DateTime,
    Primitive::YearMonthDuration,
    Primitive::DayTimeDuration,
];

/// [XQuery Appendix B.2] The primitive types `eq` and `ne` compare too,
/// each with its own type alone.
const EQUAL: [Primitive; 9] = [
    Primitive::GYearMonth,
    Primitive::GYear,
    Primitive::GMonthDay,
    Primitive::GDay,
    Primitive::GMonth,
    Primitive::HexBinary,
    Primitive::Base64Binary,
    Primitive::QName,
    Primitive::Notation,
];

/// [XQuery Appendix B.2] Whether the operator mapping compares `a` and `b`
/// by `equality` alone, `eq` and `ne`, or by every value comparison.
#[must_use]
pub fn comparable(a: &Atom, b: &Atom, equality: bool) -> bool {
    if rank(a).is_some() && rank(b).is_some() {
        return true;
    }
    let (Some(p), Some(q)) = (primitive(a), primitive(b)) else { return false };
    let durations = [Primitive::Duration, Primitive::YearMonthDuration, Primitive::DayTimeDuration];
    let same = p == q && (ORDERED.contains(&p) || (equality && EQUAL.contains(&p)));
    same || (equality && durations.contains(&p) && durations.contains(&q))
}

/// Whether `op` is `eq` or `ne`, or `=` or `!=`.
#[must_use]
pub fn is_equality(op: Comparator) -> bool {
    use super::super::ast::{GeneralComp, ValueComp};
    matches!(op, Comparator::Value(ValueComp::Eq | ValueComp::Ne) | Comparator::General(GeneralComp::Eq | GeneralComp::Ne))
}

/// The type of a binary operator of operand types `left` and `right`, each a
/// union of optional atomic types [XQueryFS Appendix B.2]: of each pair of
/// their atomic types, `typed`'s type, the empty type if either is
/// `empty`, optional if either may be empty.
///
/// # Errors
///
/// err:XPTY0004 if an operand is no union of optional atomic types, or
/// `typed` refuses a pair, `what` naming the operator.
pub fn binary(left: &Type, right: &Type, what: &str, typed: &dyn Fn(&Atom, &Atom) -> Option<AtomicType>) -> Result<Type> {
    let (Some(lefts), Some(rights)) = (split(left)?, split(right)?) else {
        return Err(type_error(&format!(
            "an operand of {what} is of {left} and {right}, not each of one atomic value or none [XQueryFS Appendix B.2]"
        )));
    };
    // [XQueryFS §8.4] Of an operand of no value at all, `none`, as of
    // fn:error, none; else of one of the empty type, the empty type.
    let never = |s: &Split| s.atoms.is_empty() && !s.optional;
    if never(&lefts) || never(&rights) {
        return Ok(Type::None);
    }
    if lefts.atoms.is_empty() || rights.atoms.is_empty() {
        return Ok(Type::Empty);
    }
    let mut out = Vec::new();
    for l in &lefts.atoms {
        for r in &rights.atoms {
            let Some(result) = typed(l, r) else {
                let why = format!(
                    "{what} has no operands of {} and {} [XQuery Appendix B.2] [XQueryFS Appendix B.2]",
                    Item::Atomic(l.clone()),
                    Item::Atomic(r.clone())
                );
                return Err(type_error(&why));
            };
            let item = Item::Atomic(Atom::Of(result));
            if !out.contains(&item) {
                out.push(item);
            }
        }
    }
    let occ = if lefts.optional || rights.optional { super::types::Occ::Optional } else { super::types::Occ::One };
    Ok(Type::of_prime(out, occ))
}
