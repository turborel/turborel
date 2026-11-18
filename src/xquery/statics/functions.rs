//! Function calls typed [XQueryFS §4.1.5]: each argument converted by its
//! parameter's type, atomized and its untyped values cast if the
//! parameter is atomic, then of a type that can be promoted to it
//! [XQueryFS §8.5.1]; the call of the function's result type. The
//! functions [XQueryFS §7.2] gives rules of their own are typed by them.
//!
//! A declared function's parameters and result are its declaration's; a
//! built-in function's its signature's (`statics/signatures.rs`); a
//! plug-in's those it registers (`XQueryFunction::parameters`).

use super::super::functions::{FN_NAMESPACE, XQueryFunction};
use super::super::plan::{CallOp, Callee, Op};
use super::operators::{numeric_at, rank};
use super::sequence::{any_attribute, any_document, any_element, normalized};
use super::signatures::{sequence_type, signature};
use super::subtype::{can_promote, subtype};
use super::types::{Atom, Item, Occ, Type, factored, prime, quantifier};
use super::values::{boolean, convert_untyped, data};
use super::{Frame, Statics, type_error};
use crate::error::Result;
use crate::xml::AtomicType;

/// The functions of [XQueryFS §7.2] with typing rules of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Special {
    /// fn:position and fn:last [XQueryFS §7.2.1, §7.2.2].
    Focus,
    /// fn:abs, fn:ceiling, fn:floor, fn:round, fn:round-half-to-even
    /// [XQueryFS §7.2.3].
    Rounding,
    /// fn:boolean [XQueryFS §7.2.4].
    Boolean,
    /// fn:collection [XQueryFS §7.2.5].
    Collection,
    /// fn:doc [XQueryFS §7.2.5].
    Doc,
    /// fn:data [XQueryFS §7.2.6].
    Data,
    /// fn:distinct-values [XQueryFS §7.2.7].
    Distinct,
    /// fn:unordered and fn:reverse [XQueryFS §7.2.8, §7.2.12].
    Same,
    /// fn:error [XQueryFS §7.2.9].
    Error,
    /// fn:min and fn:max [XQueryFS §7.2.10].
    MinMax,
    /// fn:avg.
    Avg,
    /// fn:sum.
    Sum,
    /// fn:remove [XQueryFS §7.2.11].
    Remove,
    /// fn:subsequence [XQueryFS §7.2.13].
    Subsequence,
    /// fn:insert-before [XQueryFS §7.2.15].
    InsertBefore,
    /// fn:zero-or-one, fn:one-or-more, fn:exactly-one [XQueryFS §7.2.16],
    /// of the occurrence each gives.
    Cardinality(Occ),
}

/// [XQueryFS §7.2] The functions of rules of their own, by local name.
const SPECIALS: [(&str, Special); 23] = [
    (POSITION, Special::Focus),
    (LAST, Special::Focus),
    ("abs", Special::Rounding),
    ("ceiling", Special::Rounding),
    ("floor", Special::Rounding),
    ("round", Special::Rounding),
    ("round-half-to-even", Special::Rounding),
    ("boolean", Special::Boolean),
    ("collection", Special::Collection),
    ("doc", Special::Doc),
    (DATA, Special::Data),
    ("distinct-values", Special::Distinct),
    ("unordered", Special::Same),
    ("reverse", Special::Same),
    ("error", Special::Error),
    ("min", Special::MinMax),
    ("max", Special::MinMax),
    ("avg", Special::Avg),
    ("sum", Special::Sum),
    ("remove", Special::Remove),
    ("subsequence", Special::Subsequence),
    ("insert-before", Special::InsertBefore),
    ("exactly-one", Special::Cardinality(Occ::One)),
];

/// [XQueryFS §7.2.6] fn:data.
pub const DATA: &str = "data";
/// [XQueryFS §7.2.1] fn:last.
pub const LAST: &str = "last";
/// [XQueryFS §7.2.2] fn:position.
pub const POSITION: &str = "position";

/// [XQueryFS §7.2.16] The other two cardinality functions.
const CARDINALITY: [(&str, Occ); 2] = [("zero-or-one", Occ::Optional), ("one-or-more", Occ::Plus)];

/// [XQueryFS §7.2.10] The types fn:min and fn:max give.
const MIN_MAX: [AtomicType; 10] = [
    AtomicType::String,
    AtomicType::Integer,
    AtomicType::Decimal,
    AtomicType::Float,
    AtomicType::Double,
    AtomicType::Date,
    AtomicType::Time,
    AtomicType::DateTime,
    AtomicType::YearMonthDuration,
    AtomicType::DayTimeDuration,
];
/// [XQueryFS §7.2.10] The types fn:avg gives.
const AVG: [AtomicType; 5] =
    [AtomicType::Decimal, AtomicType::Float, AtomicType::Double, AtomicType::YearMonthDuration, AtomicType::DayTimeDuration];
/// [XQueryFS §7.2.10] The types fn:sum gives, `xdt:dayTimeDuration` among
/// them as its text says.
const SUM: [AtomicType; 6] = [
    AtomicType::Integer,
    AtomicType::Decimal,
    AtomicType::Float,
    AtomicType::Double,
    AtomicType::YearMonthDuration,
    AtomicType::DayTimeDuration,
];

/// The local name of `f`, if it is a function of the namespace `fn`.
fn builtin_name(f: &dyn XQueryFunction) -> Option<&str> {
    let name = f.name();
    (name.uri.as_deref() == Some(FN_NAMESPACE)).then_some(&*name.local)
}

/// Whether `op` calls the built-in function `local`.
pub fn calls(op: &Op, local: &str) -> bool {
    let Op::Call(call) = op else { return false };
    let Callee::Library(f) = &call.callee else { return false };
    builtin_name(&**f) == Some(local)
}

impl Statics<'_> {
    /// [XQueryFS §4.1.5] A function call's type.
    pub(super) fn call(&mut self, call: &CallOp, frame: &mut Frame) -> Result<Type> {
        let mut args = Vec::with_capacity(call.arguments.len());
        for a in &call.arguments {
            args.push(self.expr(a, frame)?);
        }
        match &call.callee {
            Callee::Declared(at) => {
                let Some((parameters, result)) = self.functions.get(*at).cloned() else { return Ok(Type::None) };
                converted(&args, &parameters, "a declared function")?;
                Ok(result)
            }
            Callee::Library(f) => {
                if let Some(local) = builtin_name(&**f) {
                    return builtin(local, call, &args, frame);
                }
                let parameters = extended(f.parameters().iter().map(normalized).collect(), args.len());
                converted(&args, &parameters, &f.name().local)?;
                Ok(normalized(f.result()))
            }
        }
    }
}

/// A built-in function's call of `args`, by its rule of [XQueryFS §7.2]
/// or its signature.
fn builtin(local: &str, call: &CallOp, args: &[Type], frame: &Frame) -> Result<Type> {
    let special = SPECIALS.iter().find(|(n, _)| *n == local).map(|(_, s)| *s);
    let special = special.or_else(|| CARDINALITY.iter().find(|(n, _)| *n == local).map(|(_, o)| Special::Cardinality(*o)));
    let first = args.first().cloned().unwrap_or(Type::Empty);
    Ok(match special {
        Some(Special::Focus) => {
            if frame.focus.is_none() {
                return Err(super::no_focus(local));
            }
            Type::atomic(AtomicType::Integer)
        }
        Some(Special::Rounding) => {
            by_signature(local, args)?;
            rounded(&data(&first)?)?
        }
        Some(Special::Boolean) => {
            boolean(&first)?;
            Type::boolean()
        }
        Some(Special::Collection) => {
            Type::of_prime(vec![any_element(), any_attribute(), Item::Pi, Item::Text, Item::Comment, any_document()], Occ::Star)
        }
        Some(Special::Doc) => Type::Item(any_document()),
        Some(Special::Data) => data(&first)?,
        Some(Special::Distinct) => {
            by_signature(local, args)?;
            factored(&data(&first)?)?
        }
        Some(Special::Same) => {
            by_signature(local, args)?;
            factored(&first)?
        }
        Some(Special::Error) => {
            by_signature(local, args)?;
            Type::None
        }
        Some(Special::MinMax | Special::Avg | Special::Sum) => aggregate(special, local, args)?,
        Some(Special::Remove) => {
            by_signature(local, args)?;
            Type::occur(factored(&first)?, Occ::Optional)
        }
        Some(Special::Subsequence) => subsequence(call, args, local)?,
        Some(Special::InsertBefore) => {
            by_signature(local, args)?;
            factored(&Type::seq(vec![first, args.get(2).cloned().unwrap_or(Type::Empty)]))?
        }
        Some(Special::Cardinality(occ)) => Type::of_prime(prime(&first)?, occ),
        None => by_signature(local, args)?,
    })
}

/// [XQueryFS §7.2.10] An aggregate's type: its argument atomized, its
/// untyped values doubles, each promoted to the first type of the
/// function's list all can be; of fn:sum's second argument too.
fn aggregate(special: Option<Special>, local: &str, args: &[Type]) -> Result<Type> {
    by_signature(local, args)?;
    let first = data(args.first().unwrap_or(&Type::Empty))?;
    let q = quantifier(&first)?;
    let items = prime(&convert_untyped(&Type::of_prime(prime(&first)?, Occ::One), &Type::atomic(AtomicType::Double))?)?;
    let list: &[AtomicType] = match special {
        Some(Special::Avg) => &AVG,
        Some(Special::Sum) => &SUM,
        Some(_) | None => &MIN_MAX,
    };
    let summed = special == Some(Special::Sum);
    let zero = args.get(1).filter(|_| summed).map(data).transpose()?;
    for to in list {
        let target = Type::atomic(*to);
        let zero_promoted = match &zero {
            Some(z) => can_promote(z, &Type::occur(target.clone(), Occ::Optional))?,
            None => true,
        };
        if !zero_promoted || !all_promoted(&items, &target)? {
            continue;
        }
        // [XQueryFS §7.2.10] aggregate_quantifier: optional of ? and *.
        let aggregated = if q.zero() { Occ::Optional } else { Occ::One };
        // fn:sum of one argument gives the integer 0 of no values.
        if summed && zero.is_none() && q.zero() {
            return Ok(Type::choice(vec![target, Type::atomic(AtomicType::Integer)]));
        }
        return Ok(Type::occur(target, aggregated));
    }
    Err(type_error(&format!("fn:{local} of values of {first} is of no type [XQueryFS §7.2.10]")))
}

/// [XQueryFS §7.2.13] fn:subsequence: of its first item, by the literal
/// 1, of a non-empty sequence its prime type; of an item by a numeric
/// literal, fn:last() or fn:position(), of length 1, optional; else
/// optional and of the input's quantifier.
fn subsequence(call: &CallOp, args: &[Type], local: &str) -> Result<Type> {
    by_signature(local, args)?;
    let first = args.first().cloned().unwrap_or(Type::Empty);
    let at = call.arguments.get(1);
    let one = call.arguments.get(2).is_some_and(is_one);
    let picked = at.is_some_and(|a| is_numeric_literal(a) || calls(a, LAST) || calls(a, POSITION));
    pick(
        &first,
        if one && at.is_some_and(is_one) {
            Pick::First
        } else if one && picked {
            Pick::One
        } else {
            Pick::Any
        },
    )
}

/// [XQueryFS §7.2.13] The type of an item picked of `t`: of the first of
/// a sequence of one or more, `first`, its prime type; of any one,
/// `single`, optional; else of `t`'s quantifier, optional.
///
/// # Errors
///
/// SQLCODE -900 past `MAX_TYPE_DEPTH`.
pub fn pick(t: &Type, picked: Pick) -> Result<Type> {
    let q = quantifier(t)?;
    if picked == Pick::First && !q.zero() {
        return Ok(Type::of_prime(prime(t)?, Occ::One));
    }
    Ok(Type::of_prime(prime(t)?, if picked == Pick::Any { q.times(Occ::Optional) } else { Occ::Optional }))
}

/// What fn:subsequence, or a predicate, picks of a sequence [XQueryFS
/// §7.2.13].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// The first item, by the literal 1.
    First,
    /// One item, by a numeric literal, fn:last() or fn:position().
    One,
    /// Any number of items.
    Any,
}

/// Whether `op` is a numeric literal.
pub fn is_numeric_literal(op: &Op) -> bool {
    let Op::Atomic(a) = op else { return false };
    a.kind.primitive().numeric()
}

/// Whether `op` is the literal 1.
pub fn is_one(op: &Op) -> bool {
    let Op::Atomic(a) = op else { return false };
    a.kind == AtomicType::Integer && &*a.lexical == ONE
}

/// The literal 1, of fn:subsequence's first item [XQueryFS §7.2.13].
const ONE: &str = "1";

/// [XQueryFS §7.2.3] A rounding function's type of an argument atomized to
/// `t`: an optional atomic value, its untyped values doubles, promoted to
/// the first of `xs:integer`, `xs:decimal`, `xs:float` and `xs:double` all
/// can be, of `t`'s quantifier.
fn rounded(t: &Type) -> Result<Type> {
    if quantifier(t)?.many() {
        return Err(type_error(&format!("a rounding function is given values of {t}, not one or none [XQueryFS §7.2.3]")));
    }
    let items = prime(&convert_untyped(t, &Type::atomic(AtomicType::Double))?)?;
    let ranks: Vec<Option<usize>> = items.iter().map(|i| if let Item::Atomic(a) = i { rank(*a) } else { None }).collect();
    let mut widest = 0_usize;
    for r in ranks {
        let Some(r) = r else {
            return Err(type_error(&format!("a rounding function is given a value of {t}, no number [XQueryFS §7.2.3]")));
        };
        widest = widest.max(r);
    }
    Ok(Type::occur(Type::atomic(numeric_at(widest)), quantifier(t)?))
}

/// Whether each of `items` can be promoted to `target`.
fn all_promoted(items: &[Item], target: &Type) -> Result<bool> {
    for i in items {
        if !can_promote(&Type::Item(i.clone()), target)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `parameters` with the last repeated for each argument past them, of a
/// variadic function.
fn extended(mut parameters: Vec<Type>, arity: usize) -> Vec<Type> {
    if let Some(last) = parameters.last().cloned() {
        while parameters.len() < arity {
            parameters.push(last.clone());
        }
    }
    parameters
}

/// The call of the built-in `fn:local` of `args` by its signature
/// [XQueryFS §4.1.5]: its arguments converted, its result type.
fn by_signature(local: &str, args: &[Type]) -> Result<Type> {
    let Some(s) = signature(local, args.len()) else {
        return Err(type_error(&format!("fn:{local} has no signature of {} arguments [XQueryFS §4.1.5]", args.len())));
    };
    let parameters: Vec<Type> = s.parameters.iter().map(|p| sequence_type(p).map_or(Type::None, |t| normalized(&t))).collect();
    converted(args, &extended(parameters, args.len()), local)?;
    Ok(sequence_type(s.result).map_or(Type::None, |t| normalized(&t)))
}

/// [XQueryFS §4.1.5] Each of `args` converted by its parameter of
/// `parameters`, `[Expr]FunctionArgument(SequenceType)`: of an atomic
/// parameter, atomized and its untyped values cast to the parameter's
/// type; then of a type that can be promoted to the parameter's.
///
/// # Errors
///
/// err:XPTY0004 of an argument that cannot be, `function` naming the
/// function.
fn converted(args: &[Type], parameters: &[Type], function: &str) -> Result<()> {
    let atomic_star = Type::occur(Type::Item(Item::Atomic(Atom::Any)), Occ::Star);
    for (at, (a, p)) in args.iter().zip(parameters).enumerate() {
        let a = if subtype(p, &atomic_star)? { convert_untyped(&data(a)?, &target_of(p)?)? } else { a.clone() };
        if !can_promote(&a, p)? {
            let why = format!(
                "argument {} of {function} is of {a}, which cannot be promoted to {p} [XQueryFS §4.1.5, §8.5.1]",
                at.saturating_add(1)
            );
            return Err(type_error(&why));
        }
    }
    Ok(())
}

/// [XQueryFS §4.1.5, §7.1.2] What an untyped value is cast to for an
/// atomic parameter of type `p`: its atomic type, `xs:double` of a
/// numeric one, itself of `xdt:anyAtomicType`.
fn target_of(p: &Type) -> Result<Type> {
    let items = prime(p)?;
    Ok(match items.as_slice() {
        [Item::Atomic(Atom::Numeric)] => Type::atomic(AtomicType::Double),
        [Item::Atomic(Atom::Of(t))] => Type::atomic(*t),
        _ => Type::atomic(AtomicType::UntypedAtomic),
    })
}
