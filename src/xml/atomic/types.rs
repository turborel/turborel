//! The built-in datatypes of XML Schema [Schema2 §3.2, §3.3] and the
//! three atomic types XQuery adds [XQueryDM §2.6.2]: each type's name,
//! the type it is derived from, the primitive type a cast treats it as,
//! its whiteSpace facet, the facets it fixes (an integer type's bounds,
//! the productions a string type's literals match), and the fundamental
//! facets of its value space [Schema2 Appendix C.1].
//!
//! \[DESIGN\] RULES.md D106: the types are a table, one row each, so a type
//! is never described in two places.

use super::super::sequence::AtomicType;

/// The type a cast treats as primitive [XQueryFO §17.1]: the 19 primitive
/// types of [Schema2 §3.2], and `xdt:untypedAtomic`, `xs:integer` and the
/// two durations, treated as primitive there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Primitive {
    /// `xdt:untypedAtomic`.
    Untyped,
    /// `xs:string`.
    String,
    /// `xs:boolean`.
    Boolean,
    /// `xs:decimal`.
    Decimal,
    /// `xs:integer`.
    Integer,
    /// `xs:float`.
    Float,
    /// `xs:double`.
    Double,
    /// `xs:duration`.
    Duration,
    /// `xdt:yearMonthDuration`.
    YearMonthDuration,
    /// `xdt:dayTimeDuration`.
    DayTimeDuration,
    /// `xs:dateTime`.
    DateTime,
    /// `xs:time`.
    Time,
    /// `xs:date`.
    Date,
    /// `xs:gYearMonth`.
    GYearMonth,
    /// `xs:gYear`.
    GYear,
    /// `xs:gMonthDay`.
    GMonthDay,
    /// `xs:gDay`.
    GDay,
    /// `xs:gMonth`.
    GMonth,
    /// `xs:hexBinary`.
    HexBinary,
    /// `xs:base64Binary`.
    Base64Binary,
    /// `xs:anyURI`.
    AnyUri,
    /// `xs:QName`.
    QName,
    /// `xs:NOTATION`.
    Notation,
}

/// [Schema2 §4.3.6] The values of the whiteSpace facet, in order of what
/// they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WhiteSpace {
    /// No normalization.
    Preserve,
    /// Each tab, line feed and carriage return a space.
    Replace,
    /// As replace, then runs of spaces one space, none at either end.
    Collapse,
}

/// [Schema2 §4.2.2.1] The values of the ordered fundamental facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ordered {
    /// No order relation is defined.
    False,
    /// A partial order: two values may be incomparable.
    Partial,
    /// A total order.
    Total,
}

/// What the literals of a type derived from `xs:token` match besides its
/// base's [Schema2 Appendix A]: the patterns of `xs:language`,
/// `xs:NMTOKEN`, `xs:Name` and `xs:NCName`, each the production it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Production {
    /// Nothing more.
    None,
    /// [Schema2 §3.3.3] A language identifier.
    Language,
    /// [Schema2 §3.3.4] An Nmtoken.
    NmToken,
    /// [Schema2 §3.3.6] A Name.
    Name,
    /// [Schema2 §3.3.7] An NCName.
    NcName,
}

/// A row of the table of types.
#[derive(Debug, Clone, Copy)]
struct Row {
    kind: AtomicType,
    name: &'static str,
    base: Option<AtomicType>,
    primitive: Primitive,
    production: Production,
    /// The minInclusive and maxInclusive it fixes, of the integer types.
    bounds: (Option<&'static str>, Option<&'static str>),
}

/// [Schema2 §3.3.16] The bounds of `xs:long`.
const LONG: (Option<&str>, Option<&str>) = (Some("-9223372036854775808"), Some("9223372036854775807"));
/// [Schema2 §3.3.17] The bounds of `xs:int`.
const INT: (Option<&str>, Option<&str>) = (Some("-2147483648"), Some("2147483647"));
/// [Schema2 §3.3.18] The bounds of `xs:short`.
const SHORT: (Option<&str>, Option<&str>) = (Some("-32768"), Some("32767"));
/// [Schema2 §3.3.19] The bounds of `xs:byte`.
const BYTE: (Option<&str>, Option<&str>) = (Some("-128"), Some("127"));
/// [Schema2 §3.3.21] The bounds of `xs:unsignedLong`.
const UNSIGNED_LONG: (Option<&str>, Option<&str>) = (Some("0"), Some("18446744073709551615"));
/// [Schema2 §3.3.22] The bounds of `xs:unsignedInt`.
const UNSIGNED_INT: (Option<&str>, Option<&str>) = (Some("0"), Some("4294967295"));
/// [Schema2 §3.3.23] The bounds of `xs:unsignedShort`.
const UNSIGNED_SHORT: (Option<&str>, Option<&str>) = (Some("0"), Some("65535"));
/// [Schema2 §3.3.24] The bounds of `xs:unsignedByte`.
const UNSIGNED_BYTE: (Option<&str>, Option<&str>) = (Some("0"), Some("255"));
/// [Schema2 §3.3.14, §3.3.15, §3.3.20, §3.3.25] The bounds of
/// `xs:nonPositiveInteger`, `xs:negativeInteger`, `xs:nonNegativeInteger`
/// and `xs:positiveInteger`.
const NON_POSITIVE: (Option<&str>, Option<&str>) = (None, Some("0"));
/// See `NON_POSITIVE`.
const NEGATIVE: (Option<&str>, Option<&str>) = (None, Some("-1"));
/// See `NON_POSITIVE`.
const NON_NEGATIVE: (Option<&str>, Option<&str>) = (Some("0"), None);
/// See `NON_POSITIVE`.
const POSITIVE: (Option<&str>, Option<&str>) = (Some("1"), None);
/// No bounds.
const UNBOUNDED: (Option<&str>, Option<&str>) = (None, None);

/// A row of a type of no production and no bounds.
const fn plain(kind: AtomicType, name: &'static str, base: Option<AtomicType>, primitive: Primitive) -> Row {
    Row { kind, name, base, primitive, production: Production::None, bounds: UNBOUNDED }
}

/// A row of a type derived from `xs:token` by a production.
const fn token(kind: AtomicType, name: &'static str, base: AtomicType, production: Production) -> Row {
    Row { kind, name, base: Some(base), primitive: Primitive::String, production, bounds: UNBOUNDED }
}

/// A row of a type derived from `xs:integer` by bounds.
const fn integer(kind: AtomicType, name: &'static str, base: AtomicType, bounds: (Option<&'static str>, Option<&'static str>)) -> Row {
    Row { kind, name, base: Some(base), primitive: Primitive::Integer, production: Production::None, bounds }
}

/// [Schema2 §3.2, §3.3, Appendix A] [XQueryDM §2.6.2] Every atomic type:
/// its name, its base type, and what a cast treats it as.
const TYPES: [Row; 44] = [
    plain(AtomicType::String, "xs:string", None, Primitive::String),
    plain(AtomicType::UntypedAtomic, "xdt:untypedAtomic", None, Primitive::Untyped),
    plain(AtomicType::Boolean, "xs:boolean", None, Primitive::Boolean),
    plain(AtomicType::Decimal, "xs:decimal", None, Primitive::Decimal),
    plain(AtomicType::Integer, "xs:integer", Some(AtomicType::Decimal), Primitive::Integer),
    plain(AtomicType::Float, "xs:float", None, Primitive::Float),
    plain(AtomicType::Double, "xs:double", None, Primitive::Double),
    plain(AtomicType::Date, "xs:date", None, Primitive::Date),
    plain(AtomicType::Time, "xs:time", None, Primitive::Time),
    plain(AtomicType::DateTime, "xs:dateTime", None, Primitive::DateTime),
    plain(AtomicType::YearMonthDuration, "xdt:yearMonthDuration", Some(AtomicType::Duration), Primitive::YearMonthDuration),
    plain(AtomicType::DayTimeDuration, "xdt:dayTimeDuration", Some(AtomicType::Duration), Primitive::DayTimeDuration),
    plain(AtomicType::HexBinary, "xs:hexBinary", None, Primitive::HexBinary),
    plain(AtomicType::Duration, "xs:duration", None, Primitive::Duration),
    plain(AtomicType::GYearMonth, "xs:gYearMonth", None, Primitive::GYearMonth),
    plain(AtomicType::GYear, "xs:gYear", None, Primitive::GYear),
    plain(AtomicType::GMonthDay, "xs:gMonthDay", None, Primitive::GMonthDay),
    plain(AtomicType::GDay, "xs:gDay", None, Primitive::GDay),
    plain(AtomicType::GMonth, "xs:gMonth", None, Primitive::GMonth),
    plain(AtomicType::Base64Binary, "xs:base64Binary", None, Primitive::Base64Binary),
    plain(AtomicType::AnyUri, "xs:anyURI", None, Primitive::AnyUri),
    plain(AtomicType::QName, "xs:QName", None, Primitive::QName),
    plain(AtomicType::Notation, "xs:NOTATION", None, Primitive::Notation),
    plain(AtomicType::NormalizedString, "xs:normalizedString", Some(AtomicType::String), Primitive::String),
    plain(AtomicType::Token, "xs:token", Some(AtomicType::NormalizedString), Primitive::String),
    token(AtomicType::Language, "xs:language", AtomicType::Token, Production::Language),
    token(AtomicType::NmToken, "xs:NMTOKEN", AtomicType::Token, Production::NmToken),
    token(AtomicType::Name, "xs:Name", AtomicType::Token, Production::Name),
    token(AtomicType::NcName, "xs:NCName", AtomicType::Name, Production::NcName),
    token(AtomicType::Id, "xs:ID", AtomicType::NcName, Production::NcName),
    token(AtomicType::IdRef, "xs:IDREF", AtomicType::NcName, Production::NcName),
    token(AtomicType::Entity, "xs:ENTITY", AtomicType::NcName, Production::NcName),
    integer(AtomicType::NonPositiveInteger, "xs:nonPositiveInteger", AtomicType::Integer, NON_POSITIVE),
    integer(AtomicType::NegativeInteger, "xs:negativeInteger", AtomicType::NonPositiveInteger, NEGATIVE),
    integer(AtomicType::Long, "xs:long", AtomicType::Integer, LONG),
    integer(AtomicType::Int, "xs:int", AtomicType::Long, INT),
    integer(AtomicType::Short, "xs:short", AtomicType::Int, SHORT),
    integer(AtomicType::Byte, "xs:byte", AtomicType::Short, BYTE),
    integer(AtomicType::NonNegativeInteger, "xs:nonNegativeInteger", AtomicType::Integer, NON_NEGATIVE),
    integer(AtomicType::UnsignedLong, "xs:unsignedLong", AtomicType::NonNegativeInteger, UNSIGNED_LONG),
    integer(AtomicType::UnsignedInt, "xs:unsignedInt", AtomicType::UnsignedLong, UNSIGNED_INT),
    integer(AtomicType::UnsignedShort, "xs:unsignedShort", AtomicType::UnsignedInt, UNSIGNED_SHORT),
    integer(AtomicType::UnsignedByte, "xs:unsignedByte", AtomicType::UnsignedShort, UNSIGNED_BYTE),
    integer(AtomicType::PositiveInteger, "xs:positiveInteger", AtomicType::NonNegativeInteger, POSITIVE),
];

/// The row of a type not in the table, which every type is.
const MISSING: Row = plain(AtomicType::UntypedAtomic, "xdt:untypedAtomic", None, Primitive::Untyped);

/// \[DESIGN\] How many types deep the built-in types are derived at most,
/// `xs:byte` from `xs:decimal` the deepest, so a walk up from a type ends.
const MAX_DERIVATION: usize = 8;

impl AtomicType {
    /// Every atomic type, each once.
    pub fn all() -> impl Iterator<Item = AtomicType> {
        TYPES.iter().map(|row| row.kind)
    }

    /// Its row.
    fn row(self) -> Row {
        TYPES.iter().find(|row| row.kind == self).copied().unwrap_or(MISSING)
    }

    /// Its name, a QName of the namespaces `xs` and `xdt` [XQueryDM §2.6].
    #[must_use]
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// The type it is derived from by restriction [Schema2 §3.3, Appendix
    /// A] [XQueryDM §2.6.2]; none of a primitive type or
    /// `xdt:untypedAtomic`, whose base is no atomic type.
    #[must_use]
    pub fn base(self) -> Option<AtomicType> {
        self.row().base
    }

    /// What a cast treats it as [XQueryFO §17.1, §17.2].
    #[must_use]
    pub fn primitive(self) -> Primitive {
        self.row().primitive
    }

    /// The production its literals match besides its base's.
    #[must_use]
    pub fn production(self) -> Production {
        self.row().production
    }

    /// The literals of the minInclusive and maxInclusive facets it fixes,
    /// of a type derived from `xs:integer` by them [Schema2 Appendix A].
    #[must_use]
    pub fn bounds(self) -> (Option<&'static str>, Option<&'static str>) {
        self.row().bounds
    }

    /// Whether it is `other` or derived from it, at any remove.
    #[must_use]
    pub fn derives_from(self, other: AtomicType) -> bool {
        let mut at = Some(self);
        for _ in 0..=MAX_DERIVATION {
            let Some(t) = at else { return false };
            if t == other {
                return true;
            }
            at = t.base();
        }
        false
    }

    /// Its whiteSpace facet [Schema2 §4.3.6, Appendix A]: preserve of
    /// `xs:string` and `xdt:untypedAtomic`, which no validation normalizes
    /// [XQueryDM §2.6.4]; replace of `xs:normalizedString`; collapse of
    /// every other type, fixed but of those derived from `xs:string`.
    #[must_use]
    pub fn white_space(self) -> WhiteSpace {
        if self == AtomicType::String || self == AtomicType::UntypedAtomic {
            WhiteSpace::Preserve
        } else if self == AtomicType::NormalizedString {
            WhiteSpace::Replace
        } else {
            WhiteSpace::Collapse
        }
    }
}

impl Primitive {
    /// The primitive type of [Schema2 §3.2] it is or is derived from:
    /// `xs:decimal` of `xs:integer`, `xs:duration` of the two durations;
    /// `xdt:untypedAtomic` its own, as it is derived from none of them.
    #[must_use]
    pub fn schema(self) -> Primitive {
        match self {
            Primitive::Integer => Primitive::Decimal,
            Primitive::YearMonthDuration | Primitive::DayTimeDuration => Primitive::Duration,
            Primitive::Untyped
            | Primitive::String
            | Primitive::Boolean
            | Primitive::Decimal
            | Primitive::Float
            | Primitive::Double
            | Primitive::Duration
            | Primitive::DateTime
            | Primitive::Time
            | Primitive::Date
            | Primitive::GYearMonth
            | Primitive::GYear
            | Primitive::GMonthDay
            | Primitive::GDay
            | Primitive::GMonth
            | Primitive::HexBinary
            | Primitive::Base64Binary
            | Primitive::AnyUri
            | Primitive::QName
            | Primitive::Notation => self,
        }
    }

    /// Whether a value of it is cast as its string [XQueryFO §17.1.1]:
    /// `xs:string`, a type derived from it, or `xdt:untypedAtomic`.
    #[must_use]
    pub fn textual(self) -> bool {
        matches!(self, Primitive::String | Primitive::Untyped)
    }

    /// [Schema2 Appendix C.1, §4.2.2.1] Its ordered fundamental facet:
    /// total of the numbers, partial of the durations, dates and times,
    /// which a type derived from one inherits; false of the rest.
    #[must_use]
    pub fn ordered(self) -> Ordered {
        match self {
            Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double => Ordered::Total,
            Primitive::Duration
            | Primitive::YearMonthDuration
            | Primitive::DayTimeDuration
            | Primitive::DateTime
            | Primitive::Time
            | Primitive::Date
            | Primitive::GYearMonth
            | Primitive::GYear
            | Primitive::GMonthDay
            | Primitive::GDay
            | Primitive::GMonth => Ordered::Partial,
            Primitive::Untyped
            | Primitive::String
            | Primitive::Boolean
            | Primitive::HexBinary
            | Primitive::Base64Binary
            | Primitive::AnyUri
            | Primitive::QName
            | Primitive::Notation => Ordered::False,
        }
    }

    /// [Schema2 Appendix C.1, §4.2.5.1] Its numeric fundamental facet:
    /// true of the numbers alone.
    #[must_use]
    pub fn numeric(self) -> bool {
        matches!(self, Primitive::Decimal | Primitive::Integer | Primitive::Float | Primitive::Double)
    }
}
