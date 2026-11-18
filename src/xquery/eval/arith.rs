//! Arithmetic and the sequence operators evaluated [XQuery §3.4, §3.3.1,
//! §3.3.3]: each arithmetic operand atomized, one value or none, an
//! untyped one cast to `xs:double`, then the operator of \[XQueryFO\] the
//! operator mapping names [XQuery Appendix B.2], the library's (`op`);
//! ranges of integers by `op:to`; and the union, intersection and
//! difference of node sequences by `op:union`, `op:intersect` and
//! `op:except`.

use super::super::error::{ErrorCode, error};
use super::super::library::op;
use super::super::plan::{Arith, Op};
use super::sequence::atomize;
use super::types::cast_atomic;
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Primitive, Sequence};

/// What an arithmetic operand is: none, or one atomic value.
type Operand = Option<Atomic>;

/// An operator of two atomic values.
type Binary = fn(&Atomic, &Atomic) -> Result<Atomic>;

/// What an operand of arithmetic is, by the operator mapping [XQuery
/// Appendix B.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A number of any numeric type.
    Number,
    /// An `xs:date`.
    Date,
    /// An `xs:time`.
    Time,
    /// An `xs:dateTime`.
    DateTime,
    /// An `xdt:yearMonthDuration`.
    YearMonth,
    /// An `xdt:dayTimeDuration`.
    DayTime,
    /// Any other value, of no arithmetic.
    Other,
}

/// The kind of `a`.
fn kind(a: &Atomic) -> Kind {
    let p = a.kind.primitive();
    if p.numeric() {
        return Kind::Number;
    }
    match p {
        Primitive::Date => Kind::Date,
        Primitive::Time => Kind::Time,
        Primitive::DateTime => Kind::DateTime,
        Primitive::YearMonthDuration => Kind::YearMonth,
        Primitive::DayTimeDuration => Kind::DayTime,
        Primitive::Untyped
        | Primitive::String
        | Primitive::Boolean
        | Primitive::Decimal
        | Primitive::Integer
        | Primitive::Float
        | Primitive::Double
        | Primitive::Duration
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth
        | Primitive::HexBinary
        | Primitive::Base64Binary
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation => Kind::Other,
    }
}

/// The operator of `op` on numbers [XQueryFO §6.2].
fn numeric(op: Arith) -> Binary {
    match op {
        Arith::Add => op::numeric_add,
        Arith::Subtract => op::numeric_subtract,
        Arith::Multiply => op::numeric_multiply,
        Arith::Divide => op::numeric_divide,
        Arith::IntegerDivide => op::numeric_integer_divide,
        Arith::Modulo => op::numeric_mod,
    }
}

/// The operator `+` maps operands of `x` and `y` to [XQuery Appendix
/// B.2], and whether it takes them the other way round.
fn addition(x: Kind, y: Kind) -> Option<(Binary, bool)> {
    let (left, right, swapped) = if matches!(x, Kind::YearMonth | Kind::DayTime) && !matches!(y, Kind::YearMonth | Kind::DayTime) {
        (y, x, true)
    } else {
        (x, y, false)
    };
    let f: Binary = match (left, right) {
        (Kind::Date, Kind::YearMonth) => op::add_year_month_duration_to_date,
        (Kind::Date, Kind::DayTime) => op::add_day_time_duration_to_date,
        (Kind::Time, Kind::DayTime) => op::add_day_time_duration_to_time,
        (Kind::DateTime, Kind::YearMonth) => op::add_year_month_duration_to_date_time,
        (Kind::DateTime, Kind::DayTime) => op::add_day_time_duration_to_date_time,
        (Kind::YearMonth, Kind::YearMonth) => op::add_year_month_durations,
        (Kind::DayTime, Kind::DayTime) => op::add_day_time_durations,
        _ => return None,
    };
    Some((f, swapped))
}

/// The operator `-` maps operands of `x` and `y` to [XQuery Appendix
/// B.2], those of dates and times given the implicit timezone.
fn subtraction(x: Kind, y: Kind) -> Option<Binary> {
    Some(match (x, y) {
        (Kind::Date, Kind::YearMonth) => op::subtract_year_month_duration_from_date,
        (Kind::Date, Kind::DayTime) => op::subtract_day_time_duration_from_date,
        (Kind::Time, Kind::DayTime) => op::subtract_day_time_duration_from_time,
        (Kind::DateTime, Kind::YearMonth) => op::subtract_year_month_duration_from_date_time,
        (Kind::DateTime, Kind::DayTime) => op::subtract_day_time_duration_from_date_time,
        (Kind::YearMonth, Kind::YearMonth) => op::subtract_year_month_durations,
        (Kind::DayTime, Kind::DayTime) => op::subtract_day_time_durations,
        _ => return None,
    })
}

/// The operator `*` or `div` maps operands of `x` and `y` to [XQuery
/// Appendix B.2], and whether it takes them the other way round.
fn scaling(divide: bool, x: Kind, y: Kind) -> Option<(Binary, bool)> {
    let found: (Binary, bool) = match (divide, x, y) {
        (false, Kind::YearMonth, Kind::Number) => (op::multiply_year_month_duration, false),
        (false, Kind::Number, Kind::YearMonth) => (op::multiply_year_month_duration, true),
        (false, Kind::DayTime, Kind::Number) => (op::multiply_day_time_duration, false),
        (false, Kind::Number, Kind::DayTime) => (op::multiply_day_time_duration, true),
        (true, Kind::YearMonth, Kind::Number) => (op::divide_year_month_duration, false),
        (true, Kind::DayTime, Kind::Number) => (op::divide_day_time_duration, false),
        (true, Kind::YearMonth, Kind::YearMonth) => (op::divide_year_month_duration_by_year_month_duration, false),
        (true, Kind::DayTime, Kind::DayTime) => (op::divide_day_time_duration_by_day_time_duration, false),
        _ => return None,
    };
    Some(found)
}

impl Evaluator<'_> {
    /// An arithmetic chain [XQuery §3.4], left to right.
    pub(super) fn arithmetic(&self, first: &Op, rest: &[(Arith, Op)], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut value = operand(&self.eval(first, frame, focus)?)?;
        for (op, next) in rest {
            let right = operand(&self.eval(next, frame, focus)?)?;
            value = match (value, right) {
                (Some(a), Some(b)) => Some(self.binary(*op, &a, &b)?),
                (None | Some(_), None) | (None, Some(_)) => None,
            };
        }
        Ok(value.map(Item::Atomic).into_iter().collect())
    }

    /// `a op b` by the operator the operator mapping names [XQuery
    /// Appendix B.2].
    ///
    /// # Errors
    ///
    /// err:XPTY0004 if the mapping has no such combination; the
    /// operator's own.
    fn binary(&self, op: Arith, left: &Atomic, right: &Atomic) -> Result<Atomic> {
        let (lk, rk) = (kind(left), kind(right));
        if lk == Kind::Number && rk == Kind::Number {
            return numeric(op)(left, right);
        }
        let zone = i32::from(self.context.timezone);
        let found = match op {
            Arith::Add => addition(lk, rk),
            Arith::Subtract => match (lk, rk) {
                (Kind::Date, Kind::Date) => return op::subtract_dates(left, right, zone),
                (Kind::Time, Kind::Time) => return op::subtract_times(left, right, zone),
                (Kind::DateTime, Kind::DateTime) => return op::subtract_date_times(left, right, zone),
                _ => subtraction(lk, rk).map(|function| (function, false)),
            },
            Arith::Multiply => scaling(false, lk, rk),
            Arith::Divide => scaling(true, lk, rk),
            Arith::IntegerDivide | Arith::Modulo => None,
        };
        let Some((function, swapped)) = found else {
            let why = format!("{} and {} have no arithmetic operator so [XQuery Appendix B.2]", left.kind.name(), right.kind.name());
            return Err(error(ErrorCode::XPTY0004, &why));
        };
        if swapped { function(right, left) } else { function(left, right) }
    }

    /// A unary operator [XQuery §3.4]: of a number, `-` op:numeric-unary-
    /// minus and `+` op:numeric-unary-plus [XQueryFO §6.2.7, §6.2.8].
    pub(super) fn unary(&self, negate: bool, operand_op: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let Some(a) = operand(&self.eval(operand_op, frame, focus)?)? else { return Ok(Vec::new()) };
        if kind(&a) != Kind::Number {
            let why = format!("a unary operator is of a number, not {} [XQuery Appendix B.2]", a.kind.name());
            return Err(error(ErrorCode::XPTY0004, &why));
        }
        let result = if negate { op::numeric_unary_minus(&a)? } else { op::numeric_unary_plus(&a)? };
        Ok(vec![Item::Atomic(result)])
    }

    /// A range [XQuery §3.3.1]: each operand converted as an argument of
    /// type `xs:integer?`; none if either is none; else op:to of them
    /// [XQueryFO §15.5.1].
    pub(super) fn range(&self, operands: &[Op; 2], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let [from, to] = operands;
        let from = range_operand(&self.eval(from, frame, focus)?)?;
        let to = range_operand(&self.eval(to, frame, focus)?)?;
        let (Some(from), Some(to)) = (from, to) else { return Ok(Vec::new()) };
        Ok(op::to(&from, &to)?.into_items())
    }

    /// `union` [XQuery §3.3.3]: op:union of its operands, left to right
    /// [XQueryFO §15.3.2].
    pub(super) fn union(&self, operands: &[Op], frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut value = Sequence::default();
        for operand in operands {
            let next = Sequence::new(self.eval(operand, frame, focus)?);
            value = op::union(&value, &next)?;
        }
        Ok(value.into_items())
    }

    /// `intersect` and `except` [XQuery §3.3.3], left to right: op:intersect
    /// and op:except [XQueryFO §15.3.3, §15.3.4].
    pub(super) fn intersect_except(
        &self,
        first: &Op,
        rest: &[(super::super::ast::SetOp, Op)],
        frame: &mut Frame,
        focus: &Focus,
    ) -> Result<Seq> {
        let mut value = op::union(&Sequence::new(self.eval(first, frame, focus)?), &Sequence::default())?;
        for (set, next) in rest {
            let other = Sequence::new(self.eval(next, frame, focus)?);
            value = if *set == super::super::ast::SetOp::Intersect { op::intersect(&value, &other)? } else { op::except(&value, &other)? };
        }
        Ok(value.into_items())
    }
}

/// An arithmetic operand [XQuery §3.4]: atomized, none or one value
/// \[err:XPTY0004\], an untyped one cast to `xs:double` \[err:FORG0001\].
fn operand(items: &[Item]) -> Result<Operand> {
    let mut atomized = atomize(items);
    if atomized.len() > 1 {
        return Err(error(ErrorCode::XPTY0004, "an arithmetic operand is more than one atomic value [XQuery §3.4]"));
    }
    let Some(a) = atomized.pop() else { return Ok(None) };
    if a.kind == AtomicType::UntypedAtomic { cast_atomic(&a, AtomicType::Double).map(Some) } else { Ok(Some(a)) }
}

/// An operand of a range [XQuery §3.3.1]: atomized, none or one value
/// \[err:XPTY0004\], which op:to converts as an `xs:integer`.
fn range_operand(items: &[Item]) -> Result<Operand> {
    let mut atomized = atomize(items);
    if atomized.len() > 1 {
        return Err(error(ErrorCode::XPTY0004, "an operand of a range is more than one atomic value [XQuery §3.3.1]"));
    }
    Ok(atomized.pop())
}
