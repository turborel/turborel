//! The constructor functions [XQueryFO §5]: one for each built-in atomic
//! type but `xs:NOTATION`, which is abstract [XQueryFO §5.1], each
//! `$arg cast as xs:TYPE?` [XQueryFO §17] by `xml::cast`; and `fn:dateTime`
//! [XQueryFO §5.2], a date and time of a date and a time.
//!
//! \[DESIGN\] RULES.md D86: `xs:QName`'s constructor takes a literal alone
//! [XQueryFO §5.1], whose prefix the static context's namespaces resolve,
//! so an evaluator constructs one of a literal itself; called with a value
//! here, it casts as `xml::cast` does, a string refused, err:XPST0083.

use std::ops::RangeInclusive;

use super::args::{arg, one_of, optional, optional_atomic, single};
use super::codes::{FORG0008, fail};
use super::{Builtin, Focus, LibraryFunction, XDT_NAMESPACE};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Moment, Sequence, Shape, XS_NAMESPACE, XS_PREFIX, cast};

/// The functions of [XQueryFO §5.2].
pub(super) const FUNCTIONS: [Builtin; 1] = [Builtin::function("dateTime", 2, 2, date_time)];

/// The constructor function of a built-in atomic type [XQueryFO §5.1].
#[derive(Debug, Clone, Copy)]
pub struct Constructor {
    kind: AtomicType,
}

impl Constructor {
    /// The constructor of `kind`.
    const fn of(kind: AtomicType) -> Constructor {
        Constructor { kind }
    }

    /// The type it constructs.
    #[must_use]
    pub fn kind(&self) -> AtomicType {
        self.kind
    }
}

impl LibraryFunction for Constructor {
    fn name(&self) -> (&'static str, &'static str) {
        let name = self.kind.name();
        let (prefix, local) = name.split_once(':').unwrap_or((XS_PREFIX, name));
        (if prefix == XS_PREFIX { XS_NAMESPACE } else { XDT_NAMESPACE }, local)
    }

    fn arity(&self) -> RangeInclusive<usize> {
        1..=1
    }

    fn call(&self, _: &Focus, args: &[Sequence]) -> Result<Sequence> {
        construct(self.kind, arg(args, 0))
    }
}

/// [XQueryFO §5.1] The constructors of every built-in atomic type but
/// `xs:NOTATION`.
pub(super) static CONSTRUCTORS: [Constructor; 43] = [
    Constructor::of(AtomicType::String),
    Constructor::of(AtomicType::UntypedAtomic),
    Constructor::of(AtomicType::Boolean),
    Constructor::of(AtomicType::Decimal),
    Constructor::of(AtomicType::Integer),
    Constructor::of(AtomicType::Float),
    Constructor::of(AtomicType::Double),
    Constructor::of(AtomicType::Date),
    Constructor::of(AtomicType::Time),
    Constructor::of(AtomicType::DateTime),
    Constructor::of(AtomicType::YearMonthDuration),
    Constructor::of(AtomicType::DayTimeDuration),
    Constructor::of(AtomicType::HexBinary),
    Constructor::of(AtomicType::Duration),
    Constructor::of(AtomicType::GYearMonth),
    Constructor::of(AtomicType::GYear),
    Constructor::of(AtomicType::GMonthDay),
    Constructor::of(AtomicType::GDay),
    Constructor::of(AtomicType::GMonth),
    Constructor::of(AtomicType::Base64Binary),
    Constructor::of(AtomicType::AnyUri),
    Constructor::of(AtomicType::QName),
    Constructor::of(AtomicType::NormalizedString),
    Constructor::of(AtomicType::Token),
    Constructor::of(AtomicType::Language),
    Constructor::of(AtomicType::NmToken),
    Constructor::of(AtomicType::Name),
    Constructor::of(AtomicType::NcName),
    Constructor::of(AtomicType::Id),
    Constructor::of(AtomicType::IdRef),
    Constructor::of(AtomicType::Entity),
    Constructor::of(AtomicType::NonPositiveInteger),
    Constructor::of(AtomicType::NegativeInteger),
    Constructor::of(AtomicType::Long),
    Constructor::of(AtomicType::Int),
    Constructor::of(AtomicType::Short),
    Constructor::of(AtomicType::Byte),
    Constructor::of(AtomicType::NonNegativeInteger),
    Constructor::of(AtomicType::UnsignedLong),
    Constructor::of(AtomicType::UnsignedInt),
    Constructor::of(AtomicType::UnsignedShort),
    Constructor::of(AtomicType::UnsignedByte),
    Constructor::of(AtomicType::PositiveInteger),
];

/// [XQueryFO §5.1] `xs:TYPE($arg as xdt:anyAtomicType?) as xs:TYPE?`:
/// `value` atomized and cast as `to`; the empty sequence of the empty
/// sequence.
///
/// # Errors
///
/// err:XPTY0004 of more than one value; the errors of casting
/// [XQueryFO §17], err:FORG0001 of a value that is none of `to`.
pub fn construct(to: AtomicType, value: &Sequence) -> Result<Sequence> {
    Ok(optional(optional_atomic(value)?.map(|a| cast(&a, to)).transpose()?))
}

/// The date, time or date and time of `shape` that `a`, of its type, is.
fn moment(a: &Atomic, shape: Shape) -> Result<Moment> {
    Moment::parse(&a.lexical, shape)?.ok_or_else(|| super::args::wrong_type("a date or time", &format!("'{}' is none", a.lexical)))
}

/// [XQueryFO §5.2] `fn:dateTime($arg1 as xs:date, $arg2 as xs:time) as
/// xs:dateTime`: the date and time of the date of `$arg1` and the time of
/// `$arg2`, of the time zone either has, or none if neither has.
///
/// # Errors
///
/// err:XPTY0004 of an argument not a date or a time; err:FORG0008 if they
/// have different time zones.
fn date_time(_: &Focus, args: &[Sequence]) -> Result<Sequence> {
    let date = moment(&one_of(arg(args, 0), AtomicType::Date)?, Shape::Date)?;
    let time = moment(&one_of(arg(args, 1), AtomicType::Time)?, Shape::Time)?;
    let zone = match (date.zone, time.zone) {
        (Some(d), Some(t)) => {
            if d != t {
                return Err(fail(FORG0008, "the date and the time have different time zones [XQueryFO §5.2]"));
            }
            Some(d)
        }
        (d, t) => d.or(t),
    };
    let joined = Moment { shape: Shape::DateTime, days: date.days, micros: time.micros, zone };
    Ok(single(Atomic::new(AtomicType::DateTime, joined.text())))
}

#[cfg(test)]
mod tests;
