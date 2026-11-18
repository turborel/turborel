//! SQL data types and domains mapped to XML Names [ISO/IEC 9075-14:2006
//! draft §9.4] and to XML Schema data types [§9.5], each named [§9.6], and
//! a collection of them written once each [§9.7], as the XML text of
//! `xs:simpleType` and `xs:complexType` definitions an XML Schema document
//! holds.
//!
//! \[§9.5 GR8\] implementation-defined: no annotation is written, so ANN,
//! ANNT, ANNL, ANNCS, ANNCO, ANNP, ANNS, ANNUP, ANNMIN and ANNMAX are each
//! the zero-length string, and no type uses the namespace of `sqlxml`.
//! \[§9.5 GR8d iv\] implementation-defined: INTEGER, SMALLINT and BIGINT
//! are restrictions of `xs:integer` by their greatest and least values, as
//! XMLCAST casts them (`functions/xml/cast.rs`). \[§9.5 GR8e viii, GR8f\]
//! implementation-defined: an approximate type is `xs:float` or
//! `xs:double` itself, and BOOLEAN `xs:boolean`, each named by a
//! restriction of no facet [§9.6 GR6b iv]. SQL_TEXT is Unicode, so the
//! mapping of a character set to Unicode (CSM) is the identity [§9.1
//! GR3]: homomorphic of a length in characters, a string of N characters
//! of N; of a length in octets not, as N octets hold at most N characters.
//!
//! Rows, arrays and multisets are mapped too (`map/composites.rs`).
//!
//! \[EXTENSION\] Part 14's 2006 draft names no BINARY, BINARY VARYING or
//! interval type [§9.4], which §9.5 maps \[GR8b, GR8l\]: they are named
//! as §9.4 names BLOB \[GR3\], `BINARY_n` and `VARBINARY_n`, and
//! `INTERVAL_` with their fields, leading precision and, of SECOND, their
//! fractional seconds precision, as `INTERVAL_DAY_TO_SECOND_2_6`.

use std::fmt::Write;

use super::super::super::BinaryEncoding;
use super::Nulls;
use super::composites::{array_name, complex, complex_type, multiset_name, parts_of, row_name};
use crate::value::datetime::Field;
use crate::value::interval::Qualifier;
use crate::value::lob::LobKind;
use crate::value::types::MAX_TYPE_NESTING;
use crate::value::xml::XmlType;
use crate::value::{CharUnits, DataType, Spelled};
use crate::xml::name_of_parts;

/// [§4.9 Table 2] The prefix and namespace of XQuery's data types,
/// `xdt:yearMonthDuration` and `xdt:dayTimeDuration` among them.
pub const XDT_PREFIX: &str = "xdt";
/// [§4.9 Table 2] The namespace of `xdt`.
pub const XDT_NAMESPACE: &str = "http://www.w3.org/2005/xpath-datatypes";

/// [§9.2] The first part of the XML Name of a domain [§9.4 GR18].
const DOMAIN_PART: &str = "Domain";
/// [§9.2] The first part of the XML Name of a distinct type [§9.4 GR20].
const UDT_PART: &str = "UDT";
/// [§9.4 GR23] The XML Name of an XML type.
const XML_NAME: &str = "XML";
/// [§9.4 GR2f] The designator of CHARACTER's XML Name.
const CHAR: &str = "CHAR";
/// [§9.4 GR2f] CHARACTER VARYING's.
const VARCHAR: &str = "VARCHAR";
/// [§9.4 GR2f] CHARACTER LARGE OBJECT's.
const CLOB: &str = "CLOB";
/// The radix of SQL's exact numbers' precision.
const RADIX: i128 = 10;

/// [§9.5 GR8d i] The greatest value an exact type of scale 0 and
/// `precision` digits holds: its digits each 9.
fn greatest(precision: u32) -> i128 {
    RADIX.checked_pow(precision).map_or(i128::MAX, |p| p.saturating_sub(1))
}

/// What a column is declared of [§9.7 GR3]: a data type, or a domain.
#[derive(Debug, Clone, PartialEq)]
pub enum Declared {
    /// A data type.
    Type(DataType),
    /// A domain: its schema, its name, and its data type.
    Domain {
        /// The `<authorization identifier>` of its schema.
        schema: String,
        /// Its `<identifier>`.
        name: String,
        /// Its data type.
        data_type: DataType,
    },
}

impl Declared {
    /// What a column of `data_type`, of the domain `domain` if it is of
    /// one, is declared of. \[§9.5 GR8m\] A domain of an XML type is
    /// declared of that type, as §9.5 restricts a domain's type as a
    /// simple type, which an XML type's complex type is not.
    #[must_use]
    pub fn of(data_type: &DataType, domain: Option<(&str, &str)>) -> Declared {
        let Some((schema, name)) = domain.filter(|_| !matches!(data_type, DataType::Xml(_))) else {
            return Declared::Type(data_type.clone());
        };
        Declared::Domain { schema: schema.to_owned(), name: name.to_owned(), data_type: data_type.clone() }
    }

    /// Its data type: a domain's, or itself.
    #[must_use]
    pub fn data_type(&self) -> &DataType {
        match self {
            Declared::Type(t) | Declared::Domain { data_type: t, .. } => t,
        }
    }
}

/// Whether a column of `t` is mapped: \[§4.10.8\] its type is no XML
/// unmappable data type, a structured type, a reference type or
/// XML(SEQUENCE), nor based on one; \[EXTENSION\] nor DECFLOAT or a type
/// of a plug-in, which §9.5 maps to no XML Schema type, nor based on one.
#[must_use]
pub fn mappable(t: &DataType) -> bool {
    type_name(t, "").is_some()
}

/// [§9.4] The XML Name of `t`, of the catalog `catalog`, if it is
/// mappable; a distinct type's \[GR20\] that of the multi-part name
/// "UDT", its catalog, schema and name [§9.2], if its source type is
/// mappable.
#[must_use]
pub fn type_name(t: &DataType, catalog: &str) -> Option<String> {
    named(t, catalog, 0)
}

/// `type_name` of `t`, inside `depth` types. \[DESIGN\] R-CF-6: no type
/// nests deeper than `MAX_TYPE_NESTING` (RULES.md D68), so none is mapped
/// past it.
pub(super) fn named(t: &DataType, catalog: &str, depth: usize) -> Option<String> {
    if depth > MAX_TYPE_NESTING {
        return None;
    }
    match t {
        DataType::Distinct(d) => {
            named(&d.source, catalog, depth.saturating_add(1)).map(|_| name_of_parts(&[UDT_PART, catalog, &d.schema, &d.name]))
        }
        DataType::Xml(XmlType::Sequence) => None,
        DataType::Xml(XmlType::Document(_) | XmlType::Content(_)) => Some(XML_NAME.to_owned()),
        DataType::Row(r) => row_name(r, catalog, depth),
        DataType::Array(a) => array_name(a, catalog, depth),
        DataType::Multiset(m) => multiset_name(&m.element, catalog, depth),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => predefined_name(t),
    }
}

/// [§9.4] The XML Name of what a column is declared of: a domain's
/// \[GR18\] that of "Domain", its catalog, schema and name [§9.2].
#[must_use]
pub fn declared_name(d: &Declared, catalog: &str) -> Option<String> {
    match d {
        Declared::Type(t) => type_name(t, catalog),
        Declared::Domain { schema, name, data_type } => mappable(data_type).then(|| name_of_parts(&[DOMAIN_PART, catalog, schema, name])),
    }
}

/// [§9.4] The XML Name of `t`, a predefined type other than XML, if it
/// has one.
fn predefined_name(t: &DataType) -> Option<String> {
    Some(match t {
        DataType::Character { length, units } => character_name(CHAR, u64::from(*length), *units),
        DataType::Varying { length, units } => character_name(VARCHAR, u64::from(*length), *units),
        DataType::Lob { kind: LobKind::Character, length, units } => character_name(CLOB, *length, *units),
        // [GR3] BLOB_XN; [EXTENSION] BINARY_XN and VARBINARY_XN.
        DataType::Lob { kind: LobKind::Binary, length, .. } => format!("BLOB_{length}"),
        DataType::Binary { length } => format!("BINARY_{length}"),
        DataType::VarBinary { length } => format!("VARBINARY_{length}"),
        DataType::Exact { precision, scale, spelled } => exact_name(*precision, *scale, *spelled),
        DataType::Approximate { precision, spelled } => approximate_name(*precision, *spelled),
        DataType::Boolean => "BOOLEAN".to_owned(),
        DataType::Date => "DATE".to_owned(),
        // [GR13, GR14] TIME_XTP and TIME_WTZ_XTP.
        DataType::Time { precision, zoned } => format!("TIME_{}{precision}", if *zoned { "WTZ_" } else { "" }),
        // [GR15, GR16] TIMESTAMP_XTSP and TIMESTAMP_WTZ_XTSP.
        DataType::Timestamp { precision, zoned } => format!("TIMESTAMP_{}{precision}", if *zoned { "WTZ_" } else { "" }),
        DataType::Interval(q) => interval_name(*q),
        DataType::DecFloat { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Xml(_)
        | DataType::Extension(_) => return None,
    })
}

/// [§9.4 GR2f] The XML Name of a character string type designated
/// `designator` of `length` in `units`: of a length in characters, CSM
/// homomorphic and MAXCSL N, `designator_MLIT` \[i\]; of one in octets,
/// \[iii\] VARCHAR_NLIT_MLIT, or CLOB_NLIT_MLIT of a CLOB, MAXCSL N.
fn character_name(designator: &str, length: u64, units: CharUnits) -> String {
    match units {
        CharUnits::Characters => format!("{designator}_{length}"),
        CharUnits::Octets => {
            let varying = if designator == CLOB { CLOB } else { VARCHAR };
            format!("{varying}_{length}_{length}")
        }
    }
}

/// [§9.4 GR4 to GR8] NUMERIC_XP_XS, DECIMAL_XP_XS, INTEGER, SMALLINT or
/// BIGINT, as the type was written; a type computed is NUMERIC.
fn exact_name(precision: u32, scale: u32, spelled: Spelled) -> String {
    match spelled {
        Spelled::Decimal => format!("DECIMAL_{precision}_{scale}"),
        Spelled::Integer => "INTEGER".to_owned(),
        Spelled::Smallint => "SMALLINT".to_owned(),
        Spelled::Bigint => "BIGINT".to_owned(),
        Spelled::Numeric | Spelled::Float | Spelled::Real | Spelled::DoublePrecision => format!("NUMERIC_{precision}_{scale}"),
    }
}

/// [§9.4 GR9 to GR11] FLOAT_XP, REAL or DOUBLE, as the type was written;
/// a type computed is FLOAT.
fn approximate_name(precision: u32, spelled: Spelled) -> String {
    match spelled {
        Spelled::Real => "REAL".to_owned(),
        Spelled::DoublePrecision => "DOUBLE".to_owned(),
        Spelled::Float | Spelled::Numeric | Spelled::Decimal | Spelled::Integer | Spelled::Smallint | Spelled::Bigint => {
            format!("FLOAT_{precision}")
        }
    }
}

/// The word of `field` in an interval qualifier.
fn field_word(field: Field) -> &'static str {
    match field {
        Field::Year => "YEAR",
        Field::Month => "MONTH",
        Field::Day => "DAY",
        Field::Hour => "HOUR",
        Field::Minute => "MINUTE",
        Field::Second => "SECOND",
    }
}

/// \[EXTENSION\] The XML Name of an interval type of the qualifier `q`:
/// INTERVAL, its start field, TO and its end field if it has two, its
/// leading precision and, of SECOND, its fractional seconds precision,
/// joined by underscores.
fn interval_name(q: Qualifier) -> String {
    let mut name = format!("INTERVAL_{}", field_word(q.start));
    if q.end != q.start {
        name.push_str("_TO_");
        name.push_str(field_word(q.end));
    }
    // Writing to a String cannot fail.
    let _ = write!(name, "_{}", q.leading);
    if q.end == Field::Second {
        let _ = write!(name, "_{}", q.fraction);
    }
    name
}

/// [§9.6] The XML Schema data type `d` maps to, named by its XML Name
/// \[GR2\], binary strings encoded as `encoding` says \[GR4\], a row's
/// null fields as `nulls` says \[GR3\]; none if `d` is not mappable.
#[must_use]
pub fn named_type(d: &Declared, catalog: &str, encoding: BinaryEncoding, nulls: Nulls) -> Option<String> {
    let name = declared_name(d, catalog)?;
    let t = match d {
        // [§9.5 GR8m] A domain restricts its data type, by its XML Name.
        Declared::Domain { data_type, .. } => return derived(&name, data_type, catalog),
        Declared::Type(t) => t,
    };
    Some(match t {
        // [§9.5 GR8o] A distinct type restricts its source type.
        DataType::Distinct(distinct) => derived(&name, &distinct.source, catalog)?,
        // [§9.5 GR8r, §9.6 GR6b i] Mixed content, any element unvalidated.
        DataType::Xml(_) => format!(r#"<xs:complexType name="{name}" mixed="true">{XML_CONTENT}</xs:complexType>"#),
        DataType::Row(_) | DataType::Array(_) | DataType::Multiset(_) => complex_type(t, &name, catalog, nulls)?,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => {
            format!(r#"<xs:simpleType name="{name}">{}</xs:simpleType>"#, restriction(t, encoding)?)
        }
    })
}

/// [§9.5 GR8m, GR8o] The type named `name` of a domain or a distinct type
/// of `base`: a restriction of it, by its XML Name. Of a complex type
/// \[GR8m vi, GR8o vi\] write a simple type restricting a complex one,
/// which no XML Schema has [Schema1 §3.14.6, Derivation Valid
/// (Restriction, Simple)], so it is read as a complex
/// type that extends it by nothing; of an XML type, one of its content,
/// mixed, as a mixed type's extension would need to be.
fn derived(name: &str, base: &DataType, catalog: &str) -> Option<String> {
    let of = type_name(base, catalog)?;
    if matches!(base.represented(), DataType::Xml(_)) {
        return Some(format!(r#"<xs:complexType name="{name}" mixed="true">{XML_CONTENT}</xs:complexType>"#));
    }
    if complex(base) {
        return Some(format!(
            r#"<xs:complexType name="{name}"><xs:complexContent><xs:extension base="{of}"/></xs:complexContent></xs:complexType>"#
        ));
    }
    Some(format!(r#"<xs:simpleType name="{name}"><xs:restriction base="{of}"/></xs:simpleType>"#))
}

/// [§9.5 GR8r ii] The content of an XML type's complex type: a sequence
/// of any elements, none validated. \[DESIGN\] The draft writes
/// `name="element"` on `xs:any`, which takes no name [Schema1 §3.10.2],
/// so a schema document that kept it would not be one: it is left out.
const XML_CONTENT: &str = r#"<xs:sequence><xs:any minOccurs="0" maxOccurs="unbounded" processContents="skip"/></xs:sequence>"#;

/// [§9.5 GR8a to GR8l] The `xs:restriction` of `t`, a predefined type
/// other than XML, of no annotation; none if §9.5 maps no such type.
fn restriction(t: &DataType, encoding: BinaryEncoding) -> Option<String> {
    Some(match t {
        DataType::Character { length, units } => characters(u64::from(*length), *units == CharUnits::Characters),
        DataType::Varying { length, .. } => characters(u64::from(*length), false),
        DataType::Lob { kind: LobKind::Character, length, .. } => characters(*length, false),
        DataType::Lob { kind: LobKind::Binary, length, .. } => binary(*length, encoding),
        DataType::Binary { length } | DataType::VarBinary { length } => binary(u64::from(*length), encoding),
        DataType::Exact { precision, scale, spelled } => exact(*precision, *scale, *spelled),
        // [GR8e viii] xs:float or xs:double itself, named [§9.6 GR6b iv].
        DataType::Approximate { precision, .. } => {
            let base = if *precision <= crate::value::REAL_PRECISION { "xs:float" } else { "xs:double" };
            format!(r#"<xs:restriction base="{base}"/>"#)
        }
        DataType::Boolean => r#"<xs:restriction base="xs:boolean"/>"#.to_owned(),
        DataType::Date => format!(r#"<xs:restriction base="xs:date"><xs:pattern value="{DATE_PATTERN}"/></xs:restriction>"#),
        DataType::Time { precision, zoned } => moment("xs:time", TIME_PATTERN, *precision, *zoned),
        DataType::Timestamp { precision, zoned } => moment("xs:dateTime", DATETIME_PATTERN, *precision, *zoned),
        DataType::Interval(q) => super::durations::duration(*q),
        DataType::DecFloat { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Xml(_)
        | DataType::Extension(_) => return None,
    })
}

/// [§9.5 GR8a, §9.6 GR5e] A character string of `most` characters:
/// exactly so many, `xs:length`, of a CHARACTER of a length in characters,
/// `exact`, as CSM is homomorphic; else at most, `xs:maxLength`.
fn characters(most: u64, exact: bool) -> String {
    let facet = if exact { "length" } else { "maxLength" };
    format!(r#"<xs:restriction base="xs:string"><xs:{facet} value="{most}"/></xs:restriction>"#)
}

/// [§9.5 GR8b] A binary string of at most `most` octets, in hex or base64
/// as `encoding` says.
fn binary(most: u64, encoding: BinaryEncoding) -> String {
    let base = match encoding {
        BinaryEncoding::Hex => "hexBinary",
        BinaryEncoding::Base64 => "base64Binary",
    };
    format!(r#"<xs:restriction base="xs:{base}"><xs:maxLength value="{most}"/></xs:restriction>"#)
}

/// [§9.5 GR8c, GR8d] An exact number: of INTEGER, SMALLINT or BIGINT,
/// `xs:integer` between its least and greatest values \[GR8d iv\]; else
/// `xs:decimal` of its precision and scale \[GR8c vi\].
fn exact(precision: u32, scale: u32, spelled: Spelled) -> String {
    match spelled {
        Spelled::Integer | Spelled::Smallint | Spelled::Bigint => {
            let most = greatest(precision);
            let least = most.saturating_neg();
            format!(
                r#"<xs:restriction base="xs:integer"><xs:maxInclusive value="{most}"/><xs:minInclusive value="{least}"/></xs:restriction>"#
            )
        }
        Spelled::Numeric | Spelled::Decimal | Spelled::Float | Spelled::Real | Spelled::DoublePrecision => format!(
            r#"<xs:restriction base="xs:decimal"><xs:totalDigits value="{precision}"/><xs:fractionDigits value="{scale}"/></xs:restriction>"#
        ),
    }
}

/// [§9.5 GR8g ii] A date's pattern, which allows no time zone.
const DATE_PATTERN: &str = r"\p{Nd}{4}-\p{Nd}{2}-\p{Nd}{2}";
/// [§9.5 GR8h ii] A time's pattern, before its fraction.
const TIME_PATTERN: &str = r"\p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2}";
/// [§9.5 GR8j ii] DATETIME, a timestamp's pattern before its fraction.
const DATETIME_PATTERN: &str = r"\p{Nd}{4}-\p{Nd}{2}-\p{Nd}{2}T\p{Nd}{2}:\p{Nd}{2}:\p{Nd}{2}";
/// [§9.5 GR8i ii, GR8k iii] TZ, a time zone displacement's pattern.
const TZ_PATTERN: &str = r"(+|-)\p{Nd}{2}:\p{Nd}{2}";

/// [§9.5 GR8h to GR8k] A time or a timestamp, its base `base` and its
/// pattern `pattern`, then `.` and `precision` digits if it has a
/// fraction, then TZ if `zoned`. The draft writes `.`, which an XML
/// Schema pattern reads as any character.
fn moment(base: &str, pattern: &str, precision: u32, zoned: bool) -> String {
    let fraction = if precision > 0 { format!(r".\p{{Nd}}{{{precision}}}") } else { String::new() };
    let zone = if zoned { TZ_PATTERN } else { "" };
    format!(r#"<xs:restriction base="{base}"><xs:pattern value="{pattern}{fraction}{zone}"/></xs:restriction>"#)
}

/// [§9.7] The XML Schema data types of `declared`, the collection of a
/// mapping's columns' data types and domains: it augmented by each
/// domain's data type, each row type's field types, each distinct type's
/// source type and each collection type's element type \[GR3\], mapped
/// by §9.6 \[GR6c\], each written once, by its XML Name \[GR6d, GR6e\],
/// in order. Whether a type is of `xdt`'s namespace, an interval type,
/// which the schema document then declares and imports.
#[must_use]
pub fn definitions(declared: &[Declared], catalog: &str, (encoding, nulls): (BinaryEncoding, Nulls)) -> (String, bool) {
    let mut written: Vec<String> = Vec::new();
    let (mut text, mut xdt) = (String::new(), false);
    for d in augmented(declared) {
        let (Some(name), Some(definition)) = (declared_name(&d, catalog), named_type(&d, catalog, encoding, nulls)) else { continue };
        if written.contains(&name) {
            continue;
        }
        xdt |= matches!(d, Declared::Type(DataType::Interval(_)));
        text.push_str(&definition);
        written.push(name);
    }
    (text, xdt)
}

/// [§9.7 GR3] `declared`, each followed by the types it is made of: a
/// domain's data type \[GR3a\], a row type's field types \[GR3b\], a
/// distinct type's source type \[GR3c\] and a collection type's element
/// type \[GR3d\], and theirs, in turn, depth first. Each type is a tree
/// of the types it is made of, no deeper than `MAX_TYPE_NESTING`, so the
/// stack of those to come ends.
fn augmented(declared: &[Declared]) -> Vec<Declared> {
    let mut all = Vec::with_capacity(declared.len());
    for d in declared {
        all.push(d.clone());
        let mut next: Vec<DataType> = match d {
            Declared::Domain { data_type, .. } => vec![data_type.clone()],
            Declared::Type(t) => parts_of(t).into_iter().rev().collect(),
        };
        while let Some(t) = next.pop() {
            next.extend(parts_of(&t).into_iter().rev());
            all.push(Declared::Type(t));
        }
    }
    all
}

#[cfg(test)]
mod tests;
