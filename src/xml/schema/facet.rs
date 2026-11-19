//! The constraining facets [Schema2 §4.3]: those a derivation step
//! specifies, the set a type has, its own merged with its base's
//! [Schema2 §4.1.1], and which apply to which types [Schema2 §4.1.5].

use std::fmt;

use crate::regex::Regex;
use crate::xml::{Atomic, AtomicType, Primitive, WhiteSpace};

use super::Variety;

/// A constraining facet as a derivation step specifies it [Schema2
/// §4.1.2.1]: its value, a literal of the base type for those whose
/// values are of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Facet {
    /// [Schema2 §4.3.1] length.
    Length(u64),
    /// [Schema2 §4.3.2] minLength.
    MinLength(u64),
    /// [Schema2 §4.3.3] maxLength.
    MaxLength(u64),
    /// [Schema2 §4.3.4] pattern: a regular expression.
    Pattern(Box<str>),
    /// [Schema2 §4.3.5] enumeration: one value.
    Enumeration(Box<str>),
    /// [Schema2 §4.3.6] whiteSpace.
    WhiteSpace(WhiteSpace),
    /// [Schema2 §4.3.7] maxInclusive.
    MaxInclusive(Box<str>),
    /// [Schema2 §4.3.8] maxExclusive.
    MaxExclusive(Box<str>),
    /// [Schema2 §4.3.9] minExclusive.
    MinExclusive(Box<str>),
    /// [Schema2 §4.3.10] minInclusive.
    MinInclusive(Box<str>),
    /// [Schema2 §4.3.11] totalDigits.
    TotalDigits(u64),
    /// [Schema2 §4.3.12] fractionDigits.
    FractionDigits(u64),
}

/// A facet specified, and whether it is fixed, so no type derived from
/// this one gives it another value [Schema2 §4.3.1.1 and the like].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetSpec {
    /// The facet and its value.
    pub facet: Facet,
    /// Its {fixed}.
    pub fixed: bool,
}

impl FacetSpec {
    /// `facet`, not fixed.
    #[must_use]
    pub fn new(facet: Facet) -> FacetSpec {
        FacetSpec { facet, fixed: false }
    }
}

/// Which facet one is, apart from its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetKind {
    /// length.
    Length,
    /// minLength.
    MinLength,
    /// maxLength.
    MaxLength,
    /// pattern.
    Pattern,
    /// enumeration.
    Enumeration,
    /// whiteSpace.
    WhiteSpace,
    /// maxInclusive.
    MaxInclusive,
    /// maxExclusive.
    MaxExclusive,
    /// minExclusive.
    MinExclusive,
    /// minInclusive.
    MinInclusive,
    /// totalDigits.
    TotalDigits,
    /// fractionDigits.
    FractionDigits,
}

impl Facet {
    /// Which facet it is.
    #[must_use]
    pub fn kind(&self) -> FacetKind {
        match self {
            Facet::Length(_) => FacetKind::Length,
            Facet::MinLength(_) => FacetKind::MinLength,
            Facet::MaxLength(_) => FacetKind::MaxLength,
            Facet::Pattern(_) => FacetKind::Pattern,
            Facet::Enumeration(_) => FacetKind::Enumeration,
            Facet::WhiteSpace(_) => FacetKind::WhiteSpace,
            Facet::MaxInclusive(_) => FacetKind::MaxInclusive,
            Facet::MaxExclusive(_) => FacetKind::MaxExclusive,
            Facet::MinExclusive(_) => FacetKind::MinExclusive,
            Facet::MinInclusive(_) => FacetKind::MinInclusive,
            Facet::TotalDigits(_) => FacetKind::TotalDigits,
            Facet::FractionDigits(_) => FacetKind::FractionDigits,
        }
    }
}

impl fmt::Display for FacetKind {
    /// Its name, as the element of its XML representation is named.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FacetKind::Length => "length",
            FacetKind::MinLength => "minLength",
            FacetKind::MaxLength => "maxLength",
            FacetKind::Pattern => "pattern",
            FacetKind::Enumeration => "enumeration",
            FacetKind::WhiteSpace => "whiteSpace",
            FacetKind::MaxInclusive => "maxInclusive",
            FacetKind::MaxExclusive => "maxExclusive",
            FacetKind::MinExclusive => "minExclusive",
            FacetKind::MinInclusive => "minInclusive",
            FacetKind::TotalDigits => "totalDigits",
            FacetKind::FractionDigits => "fractionDigits",
        })
    }
}

/// [Schema2 §4.1.5] The facets of a list, and of the types derived from
/// `xs:string`, the URIs, the names and the binary types.
const OF_LENGTH: [FacetKind; 6] =
    [FacetKind::Length, FacetKind::MinLength, FacetKind::MaxLength, FacetKind::Pattern, FacetKind::Enumeration, FacetKind::WhiteSpace];
/// [Schema2 §4.1.5] The facets of a union.
const OF_UNION: [FacetKind; 2] = [FacetKind::Pattern, FacetKind::Enumeration];
/// [Schema2 §4.1.5] The facets of `xs:boolean`.
const OF_BOOLEAN: [FacetKind; 2] = [FacetKind::Pattern, FacetKind::WhiteSpace];
/// [Schema2 §4.1.5] The facets of the ordered types but `xs:decimal`.
const OF_ORDER: [FacetKind; 7] = [
    FacetKind::Pattern,
    FacetKind::Enumeration,
    FacetKind::WhiteSpace,
    FacetKind::MaxInclusive,
    FacetKind::MaxExclusive,
    FacetKind::MinInclusive,
    FacetKind::MinExclusive,
];
/// [Schema2 §4.1.5] The facets of `xs:decimal`.
const OF_DECIMAL: [FacetKind; 9] = [
    FacetKind::TotalDigits,
    FacetKind::FractionDigits,
    FacetKind::Pattern,
    FacetKind::WhiteSpace,
    FacetKind::Enumeration,
    FacetKind::MaxInclusive,
    FacetKind::MaxExclusive,
    FacetKind::MinInclusive,
    FacetKind::MinExclusive,
];

/// [Schema2 §4.1.5 applicable facets] Whether `kind` may be among the
/// facets of a type of `variety`: by the table of its {base type
/// definition}'s primitive type, of a list or union its own row. No facet
/// applies to `xdt:untypedAtomic`, from which no type is derived
/// [XQueryDM §2.6.2].
#[must_use]
pub fn applicable(variety: &Variety, kind: FacetKind) -> bool {
    let kinds: &[FacetKind] = match variety {
        Variety::List(_) => &OF_LENGTH,
        Variety::Union(_) => &OF_UNION,
        Variety::Atomic(t) => match t.primitive().schema() {
            Primitive::String
            | Primitive::AnyUri
            | Primitive::QName
            | Primitive::Notation
            | Primitive::HexBinary
            | Primitive::Base64Binary => &OF_LENGTH,
            Primitive::Boolean => &OF_BOOLEAN,
            Primitive::Decimal | Primitive::Integer => &OF_DECIMAL,
            Primitive::Float
            | Primitive::Double
            | Primitive::Duration
            | Primitive::YearMonthDuration
            | Primitive::DayTimeDuration
            | Primitive::DateTime
            | Primitive::Time
            | Primitive::Date
            | Primitive::GYearMonth
            | Primitive::GYear
            | Primitive::GMonthDay
            | Primitive::GDay
            | Primitive::GMonth => &OF_ORDER,
            Primitive::Untyped => &[],
        },
    };
    kinds.contains(&kind)
}

/// A facet's value in a type's set of facets, and whether it is fixed.
#[derive(Debug, Clone, PartialEq)]
pub struct Held<T> {
    /// Its {value}.
    pub value: T,
    /// Its {fixed}.
    pub fixed: bool,
}

/// A pattern facet's value: its regular expression, and that compiled.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// The regular expression as written.
    pub text: Box<str>,
    /// It compiled as XML Schema reads it (`Regex::schema`).
    pub regex: Regex,
}

/// A type's set of facets, its own merged with its base's [Schema2
/// §4.1.1, §4.1.2.1]. The patterns are kept by derivation step: those of
/// a step are ORed, those of steps ANDed [Schema2 §4.3.4.3]. The
/// enumeration is its last step's, each value a list of atomic values,
/// of one atomic value but of a list type.
#[derive(Debug, Clone, Default)]
pub struct Facets {
    /// length.
    pub length: Option<Held<u64>>,
    /// minLength.
    pub min_length: Option<Held<u64>>,
    /// maxLength.
    pub max_length: Option<Held<u64>>,
    /// The patterns of each step that gave any.
    pub patterns: Vec<Vec<Pattern>>,
    /// enumeration.
    pub enumeration: Option<Vec<Vec<Atomic>>>,
    /// whiteSpace.
    pub white_space: Option<Held<WhiteSpace>>,
    /// maxInclusive.
    pub max_inclusive: Option<Held<Atomic>>,
    /// maxExclusive.
    pub max_exclusive: Option<Held<Atomic>>,
    /// minExclusive.
    pub min_exclusive: Option<Held<Atomic>>,
    /// minInclusive.
    pub min_inclusive: Option<Held<Atomic>>,
    /// totalDigits.
    pub total_digits: Option<Held<u64>>,
    /// fractionDigits.
    pub fraction_digits: Option<Held<u64>>,
}

impl Facets {
    /// The facets the built-in type `t` has [Schema2 Appendix A]: its
    /// whiteSpace, fixed but of `xs:string` and the types derived from it;
    /// of `xs:integer` and the types derived from it, fractionDigits 0,
    /// fixed, and the bounds it fixes. The patterns of the types derived
    /// from `xs:token` are part of their lexical spaces (`xml/atomic.rs`).
    #[must_use]
    pub fn built_in(t: AtomicType) -> Facets {
        let fixed = !t.derives_from(AtomicType::String);
        let mut facets = Facets { white_space: Some(Held { value: t.white_space(), fixed }), ..Facets::default() };
        if t.derives_from(AtomicType::Integer) {
            facets.fraction_digits = Some(Held { value: 0, fixed: true });
            let bound = |literal: Option<&str>| literal.map(|l| Held { value: Atomic::new(AtomicType::Integer, l), fixed: false });
            let (least, most) = t.bounds();
            facets.min_inclusive = bound(least);
            facets.max_inclusive = bound(most);
        }
        facets
    }

    /// The bound `kind` names: maxInclusive, maxExclusive, minExclusive or
    /// minInclusive; none of another facet.
    #[must_use]
    pub fn bound(&self, kind: FacetKind) -> Option<&Held<Atomic>> {
        match kind {
            FacetKind::MaxInclusive => self.max_inclusive.as_ref(),
            FacetKind::MaxExclusive => self.max_exclusive.as_ref(),
            FacetKind::MinExclusive => self.min_exclusive.as_ref(),
            FacetKind::MinInclusive => self.min_inclusive.as_ref(),
            FacetKind::Length
            | FacetKind::MinLength
            | FacetKind::MaxLength
            | FacetKind::Pattern
            | FacetKind::Enumeration
            | FacetKind::WhiteSpace
            | FacetKind::TotalDigits
            | FacetKind::FractionDigits => None,
        }
    }
}
