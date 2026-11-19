//! The expressions on sequence types evaluated [XQuery §3.12]: `instance
//! of`, `treat`, `cast` and `castable`, a cast by the rules of [XQueryFO
//! §17.1] (`xml::cast`); and the function conversion rules [XQuery
//! §3.1.5], which convert a function's arguments and result to their
//! declared types.

use super::super::error::{ErrorCode, error};
use super::super::plan::{CastTarget, Op};
use super::super::types::{ItemKind, SchemaType, SeqType};
use super::sequence::{atomize, boolean};
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::{Result, SqlError, SqlState};
use crate::xml::{Atomic, AtomicType, Item, Primitive, cast};

/// [ISO/IEC 9075-14:2006 draft §6.18 GR3] The rule an error of a cast
/// within a query breaks.
const QUERY_RULE: &str = "ISO/IEC 9075-14:2006 draft §6.18 GR3";

/// `a` cast as `to` [XQueryFO §17.1], its error, an XQuery error, raised
/// as the evaluation's [ISO/IEC 9075-14:2006 draft §6.18 GR3].
///
/// # Errors
///
/// The cast's: err:FORG0001, err:XPTY0004, err:FODT0001 or
/// err:FOCA0002.
pub(super) fn cast_atomic(a: &Atomic, to: AtomicType) -> Result<Atomic> {
    cast(a, to).map_err(within_query)
}

/// `e`, of an XQuery error, as the evaluation of a query raises it.
fn within_query(mut e: SqlError) -> SqlError {
    if e.state == Some(SqlState::XQUERY_ERROR) {
        e.rule = Some(QUERY_RULE.to_owned());
    }
    e
}

impl Evaluator<'_> {
    /// `instance of` [XQuery §3.12.1].
    pub(super) fn instance_of(&self, operand: &Op, t: &SeqType, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        Ok(boolean(t.matches(&self.eval(operand, frame, focus)?)))
    }

    /// `treat as` [XQuery §3.12.6]: the operand's value if it matches the
    /// type, else err:XPDY0050.
    pub(super) fn treat(&self, operand: &Op, t: &SeqType, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let value = self.eval(operand, frame, focus)?;
        if !t.matches(&value) {
            return Err(error(ErrorCode::XPDY0050, "the operand of treat does not match its type [XQuery §3.12.6]"));
        }
        Ok(value)
    }

    /// `cast as` [XQuery §3.12.3]: the operand atomized, one value cast as
    /// the target type, none allowed by `?` \[err:XPTY0004\].
    pub(super) fn cast(&self, operand: &Op, target: &CastTarget, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut atomized = atomize(&self.eval(operand, frame, focus)?)?;
        if atomized.len() > 1 {
            return Err(error(ErrorCode::XPTY0004, "the operand of a cast is more than one atomic value [XQuery §3.12.3]"));
        }
        match atomized.pop() {
            Some(a) => Ok(vec![Item::Atomic(cast_to(&a, target)?)]),
            None => {
                if target.optional {
                    Ok(Vec::new())
                } else {
                    Err(error(ErrorCode::XPTY0004, "the operand of a cast is empty, and its type has no '?' [XQuery §3.12.3]"))
                }
            }
        }
    }

    /// `castable as` [XQuery §3.12.4]: whether the cast would succeed.
    pub(super) fn castable(&self, operand: &Op, target: &CastTarget, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let atomized = atomize(&self.eval(operand, frame, focus)?)?;
        Ok(boolean(match atomized.as_slice() {
            [] => target.optional,
            [a] => cast_to(a, target).is_ok(),
            _ => false,
        }))
    }
}

/// `a` cast as `target` [XQuery §3.12.3]: as its built-in type, or as the
/// user-defined atomic type of an imported schema, whose facets the value
/// is checked against [XQueryFO §17.2, §17.4], made a value of that type.
///
/// # Errors
///
/// As `cast_atomic`; of a user-defined type, err:FORG0001 of a value its
/// facets refuse.
fn cast_to(a: &Atomic, target: &CastTarget) -> Result<Atomic> {
    let Some(defined) = &target.defined else { return cast_atomic(a, target.to) };
    defined.cast(a).map_err(within_query)
}

/// `value` converted to `expected` by the function conversion rules
/// [XQuery §3.1.5]: to a sequence of an atomic type, atomized, each
/// untyped value cast to the type, `xs:double` of a numeric one, and each
/// number promoted to it [XQuery Appendix B.1]; then matched against it
/// \[err:XPTY0004\].
pub(super) fn converted(value: Seq, expected: &SeqType, what: &str) -> Result<Seq> {
    let converted = match expected {
        SeqType::Of(ItemKind::Atomic(t), _) => {
            atomize(&value)?.into_iter().map(|a| to_atomic_type(a, t).map(Item::Atomic)).collect::<Result<Seq>>()?
        }
        SeqType::Of(ItemKind::Numeric, _) => {
            atomize(&value)?.into_iter().map(|a| untyped_to(a, AtomicType::Double).map(Item::Atomic)).collect::<Result<Seq>>()?
        }
        SeqType::Empty | SeqType::Of(ItemKind::Item | ItemKind::Node(_), _) => value,
    };
    if !expected.matches(&converted) {
        return Err(error(ErrorCode::XPTY0004, &format!("{what} does not match its declared type [XQuery §3.1.5]")));
    }
    Ok(converted)
}

/// `a` cast to `to` if it is untyped.
fn untyped_to(a: Atomic, to: AtomicType) -> Result<Atomic> {
    if a.kind == AtomicType::UntypedAtomic { cast_atomic(&a, to) } else { Ok(a) }
}

/// `a` converted to the atomic type `t` [XQuery §3.1.5]: an untyped value
/// cast to it, a user-defined type of an imported schema too; a number
/// promoted to `xs:float` or `xs:double`, an `xs:anyURI` to `xs:string`
/// [XQuery Appendix B.1]; any other kept, to be matched.
fn to_atomic_type(a: Atomic, t: &SchemaType) -> Result<Atomic> {
    if let (SchemaType::Defined(defined), AtomicType::UntypedAtomic) = (t, a.kind) {
        return defined.cast(&a).map_err(within_query);
    }
    let SchemaType::Atomic(to) = t else { return Ok(a) };
    let from = a.kind.primitive();
    let exact = matches!(from, Primitive::Integer | Primitive::Decimal);
    let numeric =
        (exact && (*to == AtomicType::Float || *to == AtomicType::Double)) || (from == Primitive::Float && *to == AtomicType::Double);
    let uri = from == Primitive::AnyUri && *to == AtomicType::String;
    if a.kind == AtomicType::UntypedAtomic || numeric || uri { cast_atomic(&a, *to) } else { Ok(a) }
}
