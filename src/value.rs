//! Data types and values: [X3.135-1986 §4.2, §5.5, §5.9] (arithmetic), [X3.135-1986 §5.11] (comparison).

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::error::{Code, Result, SqlState, fail_with};

mod binary;
pub(crate) mod cast;
pub(crate) mod codec;
pub mod collation;
pub(crate) mod comparison;
pub mod datetime;
pub mod decfloat;
mod decimal;
mod distinct;
pub mod interval;
pub mod json;
mod like;
pub mod lob;
pub mod multisets;
pub(crate) mod nested;
mod operators;
mod order;
pub mod precedence;
mod spelled;
pub mod types;
mod units;
pub mod xml;
pub use units::CharUnits;

pub use binary::{BYTE_BITS, Binary};
pub use cast::{cast, cast_warned};
pub use comparison::{Bucket, LobEquality, any_lob, bucket, compare, compare_checked, compare_strings, lob_proxies, proxies, row_proxies};
pub use datetime::Datetime;
pub use decfloat::DecFloat;
pub use decimal::{Decimal, bound_of, pow10};
pub use distinct::{distinct, key_bucket, key_equal};
pub use like::{
    LikePattern, LikePatterns, OctetPattern, collated as like_collated, constant_pattern, constant_prefix, constant_rest_is_any, like,
};
pub use operators::{ArithOp, CompOp, arith};
pub use order::SortOrder;
pub use spelled::{Spelled, spelling};
pub use types::{Descriptor, SqlType};

// ---------------------------------------------------------------------------
// Implementor-defined choices. The standard leaves each of these to us.
// ---------------------------------------------------------------------------

/// [ISO/IEC 9075-2:2023 §6.1 SR27a, §6.30 SR1c] Largest exact numeric
/// precision (decimal digits) this implementation supports (IL043), of a
/// declared type and of every exact result (IV136): the most digits whose units
/// an `i128` always holds (RULES.md D695).
pub const MAX_EXACT_PRECISION: u32 = 38;
/// \[DESIGN\] (RULES.md D695) The most digits whose units an `i64` always
/// holds: a column of no more is stored in 64 bits, as a batch computes
/// it, and one of more in 128.
pub const NARROW_EXACT_PRECISION: u32 = 18;
/// [X3.135-1986 §5.5 SR8] INTEGER has implementor-defined precision and scale 0.
pub const INTEGER_PRECISION: u32 = 10;
/// [X3.135-1986 §5.5 SR9] SMALLINT precision is not larger than INTEGER's.
pub const SMALLINT_PRECISION: u32 = 5;
/// [ISO/IEC 9075-2:2023 §6.1 SR30] BIGINT's precision is not smaller than
/// INTEGER's, and decimal as theirs is (IV032): the most digits an exact
/// value of 64 bits holds, which a row keeps in 64 bits.
pub const BIGINT_PRECISION: u32 = NARROW_EXACT_PRECISION;
/// [X3.135-1986 §5.5 SR3] precision of NUMERIC / DECIMAL when omitted.
pub const DEFAULT_EXACT_PRECISION: u32 = 18;
/// [X3.135-1986 §5.5 SR11] REAL precision, in binary digits (IEEE single).
pub const REAL_PRECISION: u32 = 24;
/// [X3.135-1986 §5.5 SR12] DOUBLE PRECISION, in binary digits (IEEE double).
pub const DOUBLE_PRECISION: u32 = 53;
/// [ISO/IEC 9075-2:2023 §6.1 SR23] The precisions of DECFLOAT, 1 to this
/// (IA021): decimal128's digits, the precision of its arithmetic (IV134).
pub const MAX_DECFLOAT_PRECISION: u32 = 34;
/// [ISO/IEC 9075-2:2023 §6.1 SR22] DECFLOAT's precision when omitted (ID062).
pub const DEFAULT_DECFLOAT_PRECISION: u32 = 34;
/// [X3.135-1986 §8.3 GR3b vii] "The order of the null value relative to nonnull values is
/// implementor-defined, but shall be either greater than or less than all
/// nonnull values." We choose greater.
pub const NULLS_SORT_HIGH: bool = true;

/// [X3.135-1986 §5.6 SR2] "The specific `<exact numeric type>` of indicator
/// parameters is implementor-defined": exact numeric of this precision, scale 0.
pub const INDICATOR_PRECISION: u32 = 9;

// ---------------------------------------------------------------------------
// Limits the standard does not set. A type beyond one is valid SQL-86 that this
// implementation does not process: SQLCODE -900 (RULES.md D18).
// ---------------------------------------------------------------------------

/// \[DESIGN\] Longest BINARY(`<length>`) and BINARY VARYING, in octets:
/// the implementation-defined maxima IL003 and IL007 [ISO/IEC 9075-2:2023
/// §6.32 SR6a], the longest character string's at one octet to a
/// character.
pub const MAX_BINARY_LENGTH: u32 = 32_767;

/// \[DESIGN\] Longest `CHARACTER(<length>)`. [X3.135-1986 §5.5 SR2] only requires a
/// length greater than 0; values are padded to their length
/// [X3.135-1986 §8.7 GR7b], so this bounds one value to 32,767 characters.
pub const MAX_CHARACTER_LENGTH: u32 = 32_767;

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.5] <data type>
// ---------------------------------------------------------------------------

/// A `<data type>` [X3.135-1986 §5.5].
#[derive(Debug, Clone, PartialEq)]
pub enum DataType {
    /// CHARACTER(length)
    Character {
        /// The length, in `units`.
        length: u32,
        /// [ISO/IEC 9075-2:2023 §6.1 SR16] What the length counts.
        units: CharUnits,
    },
    /// [ISO/IEC 9075:1992 §6.1] CHARACTER VARYING(length), CHAR VARYING,
    /// VARCHAR: a character string of variable length.
    Varying {
        /// The maximum length, in `units`.
        length: u32,
        /// [ISO/IEC 9075-2:2023 §6.1 SR16] What the length counts.
        units: CharUnits,
    },
    /// NUMERIC, DECIMAL, INTEGER, SMALLINT.
    Exact {
        /// Decimal digits in all.
        precision: u32,
        /// Decimal digits after the point.
        scale: u32,
        /// How it was written.
        spelled: Spelled,
    },
    /// FLOAT, REAL, DOUBLE PRECISION.
    Approximate {
        /// Binary digits of the mantissa.
        precision: u32,
        /// How it was written.
        spelled: Spelled,
    },
    /// [ISO/IEC 9075-2:2023 §6.1] DECFLOAT(precision): decimal floating
    /// point.
    DecFloat {
        /// Decimal digits of the coefficient.
        precision: u32,
    },
    /// [ISO/IEC 9075-2:2023 §6.1] BINARY(length): a binary string of
    /// fixed length.
    Binary {
        /// The length in octets.
        length: u32,
    },
    /// [ISO/IEC 9075-2:2023 §6.1] BINARY VARYING(length) or
    /// VARBINARY(length): a binary string of variable length.
    VarBinary {
        /// The maximum length in octets.
        length: u32,
    },
    /// [ISO/IEC 9075-2:2023 §6.1] CHARACTER LARGE OBJECT(length), CLOB,
    /// NATIONAL CHARACTER LARGE OBJECT or NCLOB, and BINARY LARGE
    /// OBJECT(length) or BLOB: a large object string (RULES.md D79).
    Lob {
        /// Which: character or binary.
        kind: lob::LobKind,
        /// The maximum length, in `units` of a character one, else octets.
        length: u64,
        /// [ISO/IEC 9075-2:2023 §6.1 SR16] What a character one's length
        /// counts.
        units: CharUnits,
    },
    /// [ISO/IEC 9075-2:2023 §6.1] BOOLEAN.
    Boolean,
    /// [ISO/IEC 9075:1992 §6.1] DATE.
    Date,
    /// [ISO/IEC 9075:1992 §6.1] TIME(precision) \[WITH TIME ZONE\].
    Time {
        /// The digits of the second's fraction.
        precision: u32,
        /// Whether WITH TIME ZONE.
        zoned: bool,
    },
    /// [ISO/IEC 9075:1992 §6.1] TIMESTAMP(precision) \[WITH TIME ZONE\].
    Timestamp {
        /// The digits of the second's fraction.
        precision: u32,
        /// Whether WITH TIME ZONE.
        zoned: bool,
    },
    /// [ISO/IEC 9075:1992 §6.1] INTERVAL `<interval qualifier>`.
    Interval(interval::Qualifier),
    /// [ISO/IEC 9075-2:2023 §6.1] `<data type>` ARRAY\[n\], an array type.
    Array(Arc<types::ArrayType>),
    /// [ISO/IEC 9075-2:2023 §6.1] `<data type>` MULTISET, a multiset
    /// type.
    Multiset(Arc<types::MultisetType>),
    /// [ISO/IEC 9075-2:2023 §6.1] ROW(field type, ...): a row type.
    Row(Arc<types::RowType>),
    /// [ISO/IEC 9075-2:2023 §4.9.2] A distinct type, a user-defined type of
    /// a source type (RULES.md D69).
    Distinct(Arc<types::DistinctType>),
    /// [ISO/IEC 9075-2:2023 §4.9.3] A structured type, known by its name
    /// (RULES.md D81).
    Structured(Arc<types::StructuredType>),
    /// [ISO/IEC 9075-2:2023 §4.11] REF(T), a reference type.
    Reference(Arc<types::RefType>),
    /// [ISO/IEC 9075-2:2023 §4.8, §6.1] JSON, the JSON type.
    Json,
    /// [ISO/IEC 9075-14:2006 draft §6.1] XML(...), an XML type (RULES.md
    /// D106).
    Xml(xml::XmlType),
    /// A type beyond the standard, a plug-in's (RULES.md D53).
    Extension(types::ExtensionType),
}

impl DataType {
    /// CHARACTER(`length`), in characters.
    pub const fn character(length: u32) -> DataType {
        DataType::Character { length, units: CharUnits::Characters }
    }
    /// CHARACTER VARYING(`length`), in characters.
    pub const fn varying(length: u32) -> DataType {
        DataType::Varying { length, units: CharUnits::Characters }
    }
    /// Whether this is a character string type: CHARACTER, CHARACTER
    /// VARYING or CHARACTER LARGE OBJECT [ISO/IEC 9075-2:2023 §4.3.1].
    pub fn is_character(&self) -> bool {
        matches!(self, DataType::Character { .. } | DataType::Varying { .. } | DataType::Lob { kind: lob::LobKind::Character, .. })
    }
    /// Whether this is a binary string type: BINARY, BINARY VARYING or
    /// BINARY LARGE OBJECT [ISO/IEC 9075-2:2023 §4.4.1].
    pub fn is_binary(&self) -> bool {
        matches!(self, DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Lob { kind: lob::LobKind::Binary, .. })
    }
    /// Whether this is a large object string type [ISO/IEC 9075-2:2023
    /// §4.3.1, §4.4.1].
    pub fn is_lob(&self) -> bool {
        matches!(self, DataType::Lob { .. })
    }
    /// Whether this is a datetime type.
    pub fn is_datetime(&self) -> bool {
        matches!(self, DataType::Date | DataType::Time { .. } | DataType::Timestamp { .. })
    }
    /// Whether this is an interval type.
    pub fn is_interval(&self) -> bool {
        matches!(self, DataType::Interval(_))
    }
    /// Whether this is a numeric type.
    pub fn is_numeric(&self) -> bool {
        matches!(self, DataType::Exact { .. } | DataType::Approximate { .. } | DataType::DecFloat { .. })
    }
    /// [X3.135-1986 §4.2] Whether its values compare with those of `other`,
    /// as its kind says: arrays, or multisets, whose element types do
    /// [ISO/IEC 9075-2:2023 §4.12.4], rows of one degree whose fields do
    /// [§4.10].
    pub fn comparable(&self, other: &DataType) -> bool {
        if let (DataType::Array(a), DataType::Array(b)) = (self, other) {
            return a.element.comparable(&b.element);
        }
        if let (DataType::Multiset(a), DataType::Multiset(b)) = (self, other) {
            return a.element.comparable(&b.element);
        }
        if let (DataType::Row(a), DataType::Row(b)) = (self, other) {
            return a.fields.len() == b.fields.len() && a.fields.iter().zip(&b.fields).all(|(x, y)| x.data_type.comparable(&y.data_type));
        }
        if let (DataType::Distinct(_), _) | (_, DataType::Distinct(_)) = (self, other) {
            return self == other && self.kind().comparable(other);
        }
        self.kind().comparable(other)
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.kind().written(self))
    }
}

// ---------------------------------------------------------------------------
// Values [X3.135-1986 §4.2] null, character string, or number; and
// [ISO/IEC 9075:1992 §4.5] datetime or interval; [ISO/IEC 9075-2:2023
// §4.4, §4.6] binary string or truth value
// ---------------------------------------------------------------------------

/// A value [X3.135-1986 §4.2].
#[derive(Debug, Clone)]
pub enum Value {
    /// The null value [X3.135-1986 §4.2].
    Null,
    /// A character string.
    Char(String),
    /// An exact number.
    Exact(Decimal),
    /// An approximate number.
    Approx(f64),
    /// [ISO/IEC 9075-2:2023 §4.5.2] A decimal floating-point number.
    DecFloat(DecFloat),
    /// [ISO/IEC 9075-2:2023 §4.4.1] A binary string.
    Binary(Binary),
    /// [ISO/IEC 9075-2:2023 §4.6] A truth value, True or False; Unknown is
    /// `Null` [§4.6.1].
    Bool(bool),
    /// [ISO/IEC 9075:1992 §4.5.1] A datetime.
    Datetime(Datetime),
    /// [ISO/IEC 9075:1992 §4.5.2] An interval.
    Interval(interval::Interval),
    /// [ISO/IEC 9075-2:2023 §4.3, §4.4] A large object string, read a
    /// chunk at a time (RULES.md D79).
    Lob(lob::Lob),
    /// [ISO/IEC 9075-2:2023 §4.12.2] An array: its elements, in order.
    Array(Arc<[Value]>),
    /// [ISO/IEC 9075-2:2023 §4.12.3] A multiset: its elements, in no
    /// order that means anything.
    Multiset(Arc<[Value]>),
    /// [ISO/IEC 9075-2:2023 §4.10] A row: its fields' values, in order.
    Row(Arc<[Value]>),
    /// [ISO/IEC 9075-2:2023 §4.9.3] A value of a structured type: its most
    /// specific type and its attributes' values.
    Structured(Arc<types::Structured>),
    /// [ISO/IEC 9075-2:2023 §4.11] A reference: its octets.
    Reference(Binary),
    /// [ISO/IEC 9075-2:2023 §4.8] A value of the JSON type, an SQL/JSON
    /// item in its stored form.
    Json(json::Json),
    /// [ISO/IEC 9075-14:2006 draft §4.2.3] An XML value: an XQuery
    /// sequence (RULES.md D106).
    Xml(xml::XmlValue),
    /// A value of a type beyond the standard, a plug-in's (RULES.md D53).
    Extension(types::ExtensionValue),
}

impl Value {
    /// Whether this is the null value.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// A number as its nearest `f64`; `None` for null or any other value.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Exact(d) => Some(d.to_f64()),
            Value::Approx(f) => Some(*f),
            Value::DecFloat(d) => Some(d.to_f64()),
            Value::Null
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => None,
        }
    }

    /// [X3.135-1986 §5.9 GR4] monadic minus reverses the sign.
    #[must_use]
    pub fn negate(&self) -> Value {
        match self {
            Value::Exact(d) => Value::Exact(d.negated()),
            Value::Approx(f) => Value::Approx(-f),
            Value::DecFloat(d) => Value::DecFloat(d.negated()),
            Value::Interval(i) => Value::Interval(interval::Interval { units: i.units.saturating_neg(), ..*i }),
            Value::Null
            | Value::Char(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Row(_)
            | Value::Json(_)
            | Value::Extension(_)
            | Value::Xml(_) => self.clone(),
        }
    }

    /// How this value is written as a `<literal>`, as its kind writes it
    /// ([X3.135-1986 §4.2] "the logical representation of a value is a
    /// `<literal>`").
    pub fn to_literal(&self) -> String {
        self.kind().map_or_else(|| "NULL".to_owned(), |k| k.literal(self))
    }

    /// A hashable form under which two values are equal exactly when they are
    /// "identical" per [X3.135-1986 §5.11] (trailing spaces ignored, 1.50 = 1.5, NULL = NULL
    /// as needed for DISTINCT by [X3.135-1986 §5.11 GR7]).
    pub fn norm(&self) -> NormKey {
        match self {
            Value::Null => NormKey::Null,
            Value::Char(s) => NormKey::Char(s.trim_end_matches(' ').to_string()),
            Value::Exact(d) => {
                let (u, s) = d.normalized();
                NormKey::Exact(u, s)
            }
            Value::Approx(f) => NormKey::Approx(if *f == 0.0 { 0f64.to_bits() } else { f.to_bits() }),
            Value::DecFloat(d) => NormKey::DecFloat(*d),
            Value::Binary(b) => NormKey::Binary(b.clone()),
            // [DESIGN] No large object is grouped or made distinct [ISO/IEC
            // 9075-2:2023 §9.12 SR3]: its form is its identity, for a cache
            // keyed by values (`lob::LobIdentity`).
            Value::Lob(l) => NormKey::Lob(lob::LobIdentity(l.clone())),
            Value::Bool(b) => NormKey::Bool(*b),
            Value::Datetime(d) => NormKey::Instant(d.utc),
            Value::Interval(i) => NormKey::Span(i.units),
            Value::Array(p) | Value::Row(p) => types::composite::norm_parts(p),
            Value::Multiset(p) => multisets::norm_bag(p),
            Value::Structured(s) => types::structured::norm_structured(s),
            Value::Reference(r) => NormKey::Binary(r.clone()),
            // [ISO/IEC 9075-2:2023 §8.2 GR10] Equivalent values share their
            // comparison key; a damaged one has none, so its stored form.
            Value::Json(j) => NormKey::Json(j.key().unwrap_or_else(|_| j.bytes().to_vec()).into()),
            // Equal values of a kind beyond the standard have the same bytes
            // (`SqlType`'s contract).
            Value::Extension(e) => NormKey::Extension(Box::new((e.kind.name().to_owned(), e.bytes.to_vec()))),
            // No XML value is grouped [ISO/IEC 9075-14:2006 draft §10.9 SR1]:
            // its stored form, for a key it is never given.
            Value::Xml(x) => NormKey::Extension(Box::new((types::XML_NAME.to_owned(), x.octets()))),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Char(s) => write!(f, "{s}"),
            Value::Xml(_) => f.write_str(&types::XML.text(self)),
            Value::Exact(d) => write!(f, "{d}"),
            // Written as an <approximate numeric literal>: <mantissa>E<exponent>.
            Value::Approx(x) => write!(f, "{x:E}"),
            Value::DecFloat(d) => write!(f, "{d}"),
            Value::Binary(b) => write!(f, "{b}"),
            Value::Lob(l) => write!(f, "{l}"),
            // [ISO/IEC 9075-2:2023 §5.3] Written as a <boolean literal>.
            Value::Bool(b) => f.write_str(if *b { crate::lexer::kw::TRUE } else { crate::lexer::kw::FALSE }),
            Value::Datetime(d) => write!(f, "{d}"),
            Value::Interval(i) => write!(f, "{i}"),
            Value::Array(_) | Value::Multiset(_) | Value::Structured(_) | Value::Reference(_) | Value::Row(_) => {
                f.write_str(&self.to_literal())
            }
            Value::Json(j) => write!(f, "{j}"),
            Value::Extension(e) => f.write_str(&e.kind.text(self)),
        }
    }
}

/// The form of a value that `Value::norm` gives: equal exactly when the
/// values are identical.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NormKey {
    /// The null value.
    Null,
    /// A string without trailing spaces.
    Char(String),
    /// Units and scale with no trailing fractional zeros.
    Exact(i128, u32),
    /// The bits of an `f64`, with -0 as +0.
    Approx(u64),
    /// A decimal floating-point number, in its one form.
    DecFloat(DecFloat),
    /// A binary string.
    Binary(Binary),
    /// A large object's identity.
    Lob(lob::LobIdentity),
    /// A truth value.
    Bool(bool),
    /// A datetime's instant, in UTC.
    Instant(i64),
    /// An interval's units.
    Span(i128),
    /// [ISO/IEC 9075-2:2023 §8.2 GR10] A value of the JSON type's
    /// comparison key.
    Json(Box<[u8]>),
    /// A kind beyond the standard's name, and the value's bytes: boxed, so
    /// a key of the standard's types stays small.
    Extension(Box<(String, Vec<u8>)>),
    /// An array's or a row's parts, each's form.
    Composite(Vec<NormKey>),
    /// A multiset's elements' forms, in any order.
    Bag(multisets::BagKey),
}

/// [ISO/IEC 9075:1992 §9.2 GR2, GR3] Store assignment of `v` to a column,
/// parameter or target of type `target`, as the rule `rule` (a citation,
/// e.g. "ISO/IEC 9075:1992 §13.8 GR4b iii") applies it, as the type's kind
/// assigns (`SqlType::assign`): null stays null. \[EXTENSION\] A character
/// string assigned to a type beyond the standard whose kind takes one
/// (`SqlType::assigns_text`) is first CAST to it (RULES.md D53).
/// Retrieval assignment [§9.1 GR3] is the same but for strings, which its
/// callers truncate first.
///
/// # Errors
///
/// SQLSTATE 22001 if a string's characters beyond the length are not all
/// spaces; 22003 if a number has no approximation in the type; 22005 if
/// the value is of a type that cannot be assigned to it \[SR2\], as a
/// plug-in function might give; the CAST's errors, of a character string
/// assigned to a type beyond the standard.
pub fn assign(v: Value, target: &DataType, rule: &str) -> Result<Value> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let kind = target.kind();
    // [ISO/IEC 9075-2:2023 §9.2 GR3b, GR3e] A large object to a string type
    // of no large object: what fits, read.
    if let Value::Lob(l) = &v
        && let Some((lowered, _)) = types::lob::lowered(l.clone(), target, types::lob::Lowering::Assign(rule))?
    {
        return kind.assign(lowered, target, rule);
    }
    let from_text = matches!((&v, target), (Value::Char(_), DataType::Extension(_))) && kind.assigns_text();
    let given = if from_text { kind.cast(v, target)? } else { v };
    // [DESIGN] the type assigned to, for a client's structured failure (RULES.md D186).
    kind.assign(given, target, rule).map_err(|e| e.or_of_type(target))
}

/// The citation of the rule `rule` that assigns a value, and of the
/// clause `clause` that it applies, of ISO/IEC 9075:1992 unless it names
/// its standard: that standard written once if `rule` is of it too.
pub(crate) fn assignment_rule(rule: &str, clause: &str) -> String {
    if clause.starts_with("ISO/IEC") {
        return format!("{rule}; {clause}");
    }
    match rule.strip_prefix("ISO/IEC 9075:1992 ") {
        Some(_) => format!("{rule}, {clause}"),
        None => format!("{rule}; ISO/IEC 9075:1992 {clause}"),
    }
}

/// [ISO/IEC 9075:1992 §9.2 SR2] `v` cannot be assigned to `t`.
///
/// # Errors
///
/// Always: SQLSTATE 22005.
pub(crate) fn unassignable(v: &Value, t: &DataType, rule: &str) -> Result<Value> {
    fail_with(
        Code::Data,
        SqlState::ERROR_IN_ASSIGNMENT,
        format!("[{}] the value {v} cannot be assigned to {t}", assignment_rule(rule, "§9.2 SR2")),
    )
}

/// `x` rounded to the nearest IEEE single-precision value, the precision of
/// REAL ([X3.135-1986 §8.7 GR7d] "the approximate value of v").
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "RULES.md R-DEV-3: rounding to single precision is the intent, and std has no f64-to-f32 conversion function"
)]
pub(crate) fn round_to_single(x: f64) -> f64 {
    f64::from(x as f32)
}

/// [X3.135-1986 §8.3 GR3b] The ORDER BY ordering of two values (ascending),
/// nulls placed by `NULLS_SORT_HIGH` [X3.135-1986 §8.3 GR3b vii].
pub fn sort_cmp(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => {
            if NULLS_SORT_HIGH {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (false, true) => {
            if NULLS_SORT_HIGH {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (false, false) => match (a, b) {
            // [ISO/IEC 9075-2:2023 §10.10 GR2] Rows field by field, each
            // null placed as a value is.
            (Value::Row(x), Value::Row(y)) | (Value::Array(x), Value::Array(y)) => {
                x.iter().zip(y.iter()).map(|(p, q)| sort_cmp(p, q)).find(|o| o.is_ne()).unwrap_or_else(|| x.len().cmp(&y.len()))
            }
            _ => compare(a, b).unwrap_or(Ordering::Equal),
        },
    }
}

// Unit tests: the decisions of arithmetic and assignment, one test per
// decision, with MC/DC cases written out (RULES.md R-TEST-1, R-COV-3).
#[cfg(test)]
mod tests;
