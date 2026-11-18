//! XMLCAST [ISO/IEC 9075-14:2006 draft §6.6] of an SQL value to an XML
//! type and of an XML value to an SQL type: the functions the binder
//! calls (`binder/xml/cast.rs`), through the mapping of SQL types to XML
//! Schema types [§9.5] and of SQL values to their lexical forms [§9.8].
//!
//! \[DESIGN\] RULES.md D86: \[§6.6 SR1b\] implementation-defined, in the
//! scope of no `<XML binary encoding>` [§11.3] the encoding is HEX
//! (`IMPLICIT_BINARY_ENCODING`), so a binary string is an `xs:hexBinary`;
//! in the scope of XMLBINARY USING BASE64 it is an `xs:base64Binary`
//! \[SR1a, SR2\] [§9.5 GR8b ii]. \[§9.5 GR8d iv\]
//! implementation-defined, INTEGER, SMALLINT and BIGINT are restrictions
//! of `xs:integer`; \[§9.5 GR8e viii, GR8f\] REAL and FLOAT of 24 bits or
//! fewer are `xs:float`, DOUBLE PRECISION and other FLOATs `xs:double`,
//! BOOLEAN `xs:boolean`. An XML value is cast to an SQL type through the
//! XML Schema type its type maps to, restricted by the facets §9.5 gives
//! it (`functions/xml/schema_type.rs`), so a value they refuse is an
//! XQuery error \[GR4j\]; SQL's CAST to the type then applies its own
//! rules \[GR4l\].

use std::sync::Arc;

use super::mapping::{BinaryEncoding, Mapping, xml_lexical, xml_mapped};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::value::datetime::{Field, MAX_TIME_PRECISION};
use crate::value::interval::{Interval, MAX_LEADING_PRECISION, Qualifier};
use crate::value::lob::LobKind;
use crate::value::xml::{XmlType, XmlValue};
use crate::value::{Binary, DataType, Decimal, Spelled, Value};
use crate::xml::{
    Atomic, AtomicType, Item, Kind, LAST_YEAR, Moment, NodeId, Primitive, Sequence, Shape, SimpleType, Syntax, Tree, Whitespace, base64,
    cast, duration_units, hex, parse, xquery_error,
};

/// [§6.6 SR1b, SR2, §6.14 GR1b, §6.15 GR1b, §6.19 GR3b]
/// implementation-defined: the `<XML binary encoding>` of XMLCAST,
/// XMLELEMENT, XMLFOREST and XMLTEXT outside the scope of one: HEX.
pub const IMPLICIT_BINARY_ENCODING: BinaryEncoding = BinaryEncoding::Hex;

/// The decimal digits an exact number's units hold at most.
const MAX_DIGITS: usize = 38;

/// [§9.5 GR8] The XML Schema built-in type the SQL type `t` maps to, in
/// binary strings' `encoding`, the type its value is cast to and from: of
/// a character string `xs:string`, of a binary string `xs:hexBinary`, or
/// `xs:base64Binary` of BASE64 [§9.5 GR8b ii], of NUMERIC and DECIMAL
/// `xs:decimal`, of INTEGER, SMALLINT and BIGINT `xs:integer`, of an
/// approximate number `xs:float` or `xs:double`, of BOOLEAN `xs:boolean`,
/// of DATE, TIME and TIMESTAMP `xs:date`, `xs:time` and `xs:dateTime`,
/// and of a year-month or day-time interval `xdt:yearMonthDuration` or
/// `xdt:dayTimeDuration` [§6.6 GR3d ii]; none of another type.
#[must_use]
pub fn xml_schema_type(t: &DataType, encoding: BinaryEncoding) -> Option<AtomicType> {
    let binary = binary_type(encoding);
    Some(match t.represented() {
        DataType::Character { .. } | DataType::Varying { .. } | DataType::Lob { kind: LobKind::Character, .. } => AtomicType::String,
        DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Lob { kind: LobKind::Binary, .. } => binary,
        DataType::Exact { spelled: Spelled::Integer | Spelled::Smallint | Spelled::Bigint, .. } => AtomicType::Integer,
        DataType::Exact { .. } => AtomicType::Decimal,
        DataType::Approximate { precision, .. } => {
            if *precision <= crate::value::REAL_PRECISION {
                AtomicType::Float
            } else {
                AtomicType::Double
            }
        }
        DataType::Boolean => AtomicType::Boolean,
        DataType::Date => AtomicType::Date,
        DataType::Time { .. } => AtomicType::Time,
        DataType::Timestamp { .. } => AtomicType::DateTime,
        DataType::Interval(q) => {
            if q.year_month() {
                AtomicType::YearMonthDuration
            } else {
                AtomicType::DayTimeDuration
            }
        }
        DataType::DecFloat { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Extension(_)
        | DataType::Xml(_) => return None,
    })
}

/// [§9.5 GR8b ii] The XML Schema type a binary string maps to in
/// `encoding`: `xs:hexBinary` of HEX, `xs:base64Binary` of BASE64.
pub(super) fn binary_type(encoding: BinaryEncoding) -> AtomicType {
    match encoding {
        BinaryEncoding::Hex => AtomicType::HexBinary,
        BinaryEncoding::Base64 => AtomicType::Base64Binary,
    }
}

/// [§6.6 GR3] XMLCAST of an SQL value to an XML type: its lexical form
/// [§9.8], binary strings in `encoding` \[SR1, SR2\], parsed as CONTENT
/// \[GR3a, GR3b\], and of XML(SEQUENCE) cast as the XML Schema type of its
/// SQL type \[GR3d\].
#[derive(Debug)]
pub struct XmlCastIn {
    /// The XML type cast to: CONTENT or SEQUENCE.
    pub target: XmlType,
    /// The XML Schema type the value's SQL type maps to, of XML(SEQUENCE)
    /// \[GR3d\].
    pub base: AtomicType,
    /// Of a value of a distinct type whose source is a collection type,
    /// cast to XML(CONTENT) alone, that type and how its value is mapped
    /// [§9.8 GR1a, GR9j]; none of a value of a predefined type.
    pub collection: Option<(DataType, Mapping)>,
    /// How binary strings are encoded, ENC of \[SR2\].
    pub encoding: BinaryEncoding,
}

impl ScalarFunction for XmlCastIn {
    fn name(&self) -> &'static str {
        "XMLCAST"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(self.target))
    }

    /// [§6.6 GR2] The null value of the null value; else \[GR3b\] XMLPARSE
    /// (CONTENT XMLV PRESERVE WHITESPACE) of its lexical form, characters
    /// mapped \[GR3a\], the value \[GR3c\], or of XML(SEQUENCE) the value
    /// of its string value cast as the XML Schema type \[GR3d vi\].
    fn call(&self, args: &[Value]) -> Result<Value> {
        let v = args.first().unwrap_or(&Value::Null);
        if v.is_null() {
            return Ok(Value::Null);
        }
        let mapped = match &self.collection {
            Some((t, m)) => xml_mapped(v, t, *m)?,
            None => xml_lexical(v, self.encoding, true)?,
        };
        let tree = parse(&mapped, Syntax::Content, Whitespace::Preserve)?;
        if self.target != XmlType::Sequence {
            return Ok(Value::Xml(XmlValue::of(Sequence::document(tree))));
        }
        let text = Atomic { kind: AtomicType::UntypedAtomic, lexical: tree.string_value(NodeId::ROOT).into() };
        Ok(Value::Xml(XmlValue::of(Sequence::new(vec![Item::Atomic(cast(&text, self.base)?)]))))
    }
}

/// [§6.6 GR4] XMLCAST of an XML value to an SQL type: its one atomic
/// value, after its document nodes are replaced by their children and it
/// is atomized \[GR4b, GR4c\], cast as XT, the XML Schema type the SQL
/// type maps to with its facets \[GR4e, GR4j\], as the SQL value that is
/// \[GR4k\], which the binder casts to the SQL type \[GR4l\].
#[derive(Debug)]
pub struct XmlCastOut {
    /// XT, the type §9.5 maps the SQL type to \[GR4e\].
    pub xmlt: Arc<SimpleType>,
    /// Of DATE, TIME and TIMESTAMP WITHOUT TIME ZONE, `xs:date`, `xs:time`
    /// or `xs:dateTime`, which the value is cast as and adjusted to UTC
    /// first, before it is cast as XT \[GR4j i to iii\].
    pub adjust: Option<AtomicType>,
    /// Of TIME(p) and TIMESTAMP(p), with or without time zone, p: the
    /// value is checked against XT written with p digits of fraction
    /// (`written`).
    pub fraction: Option<u32>,
    /// The type of the value it gives, A of \[GR4k\].
    pub given: DataType,
}

impl ScalarFunction for XmlCastOut {
    fn name(&self) -> &'static str {
        "XMLCAST"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.given.clone())
    }

    /// [§6.6 GR2] The null value of the null value, and \[GR4d\] of an
    /// empty sequence once atomized; else the value cast.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Xml(x)) = args.first() else { return Ok(Value::Null) };
        let Some(av) = atomized(&*x.sequence()?)? else { return Ok(Value::Null) };
        let bv = match (self.fraction, self.adjust) {
            (Some(digits), adjust) => self.written(&av, digits, adjust)?,
            (None, Some(kind)) => self.xmlt.cast(&in_utc(&av, kind)?)?,
            (None, None) => self.xmlt.cast(&av)?,
        };
        sql_value(&bv)
    }
}

impl XmlCastOut {
    /// `av` cast as XT, the type of TIME(`digits`) or TIMESTAMP(`digits`),
    /// adjusted to UTC first as `adjust` says \[GR4j i to iii\].
    /// \[DESIGN\] RULES.md D86: implementation-defined, as the patterns
    /// of §9.5 GR8h to GR8k ask for exactly `digits` digits of fraction,
    /// which no canonical form of a value has [XQueryFO §17.4], the
    /// pattern and the other facets are checked against the value written
    /// as §9.8 GR9f and GR9g write an SQL value of the type, its fraction
    /// padded with zeros to `digits` and its time zone ±hh:mm
    /// (`Moment::written`), so each value of the SQL type is taken. A
    /// value of more significant digits of fraction is not, nor text of
    /// more, zeros ending its fraction not counted, as they are no digits
    /// of its value.
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, err:FORG0001, of more digits of fraction, or a
    /// value XT does not take; as `cast` and `in_utc` do.
    fn written(&self, av: &Atomic, digits: u32, adjust: Option<AtomicType>) -> Result<Atomic> {
        let kind = self.xmlt.atomic().unwrap_or(AtomicType::DateTime);
        let places = usize::try_from(digits).unwrap_or(usize::MAX);
        if av.kind.primitive().textual() && significant_fraction(&av.lexical) > places {
            let why = format!("'{}' has more than {digits} digits of fraction for {}", av.lexical, self.xmlt.name());
            return Err(xquery_error(&why, "[XQueryFO §17.1.1] err:FORG0001"));
        }
        let typed = match adjust {
            Some(k) => in_utc(av, k)?,
            None => cast(av, kind)?,
        };
        let Some(m) = Shape::of(kind.primitive()).and_then(|shape| Moment::parse(&typed.lexical, shape).ok().flatten()) else {
            return self.xmlt.cast(&typed);
        };
        Ok(self.xmlt.validate(&m.written(places))?.pop().unwrap_or(typed))
    }
}

/// How many digits of fraction the seconds of the date and time or time
/// `text` has, zeros ending them not counted.
fn significant_fraction(text: &str) -> usize {
    let Some((_, after)) = text.split_once('.') else { return 0 };
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.trim_end_matches('0').len()
}

/// [§6.6 GR4j i to iii] `value` cast as `kind`, a date, a time or a date
/// and time, adjusted to UTC and its time zone removed, as
/// `fn:adjust-dateTime-to-timezone` to PT0H then to none adjusts it
/// [XQueryFO §10.7].
///
/// # Errors
///
/// As `cast`; SQLSTATE 22007 if its year in UTC is before 0001 \[GR4k vi
/// 1, GR4k vii 1\].
fn in_utc(value: &Atomic, kind: AtomicType) -> Result<Atomic> {
    let typed = cast(value, kind)?;
    let Some(m) = Shape::of(kind.primitive()).and_then(|shape| Moment::parse(&typed.lexical, shape).ok().flatten()) else {
        return Ok(typed);
    };
    let utc = m.in_utc();
    if m.shape != Shape::Time && utc.year() < 1 {
        let why = format!("'{}' is of the year {} in UTC", typed.lexical, utc.year());
        return Err(refused(&why, "GR4k vi 1, GR4k vii 1", SqlState::INVALID_DATETIME_FORMAT));
    }
    Ok(Atomic { kind, lexical: utc.text().into() })
}

/// [§6.6 GR4b, GR4c] The one atomic value of `value` with its document
/// nodes replaced by their children [§10.17], by `fn:data` [XQueryFO
/// §2.4]: of an element, attribute or text node its string value, untyped
/// [XQueryDM §6.2.3, §6.3.3, §6.7.3]; of a comment or processing
/// instruction its content, a string [XQueryDM §6.5.2, §6.6.2]; none if
/// it has none.
///
/// # Errors
///
/// SQLSTATE 10000, err:XPTY0004, if it has more than one, as `cast as`
/// casts one value [XQuery §3.12.3].
fn atomized(value: &Sequence) -> Result<Option<Atomic>> {
    let mut items = Vec::new();
    for item in value.items() {
        match item {
            Item::Atomic(a) => items.push(a.clone()),
            Item::Node(node) => {
                let tree = node.tree();
                let nodes: Vec<NodeId> =
                    if node.kind() == Some(Kind::Document) { tree.children(node.id()).collect() } else { vec![node.id()] };
                items.extend(nodes.into_iter().map(|id| typed_value(tree, id)));
            }
        }
    }
    if items.len() > 1 {
        let why = format!("XMLCAST casts one atomic value, not {}", items.len());
        return Err(xquery_error(&why, "[XQuery §3.12.3] err:XPTY0004"));
    }
    Ok(items.pop())
}

/// The typed value of node `id` of `tree`, which no schema typed
/// [XQueryDM §5.15].
fn typed_value(tree: &Tree, id: NodeId) -> Atomic {
    let kind = match tree.kind(id) {
        Some(Kind::Comment | Kind::ProcessingInstruction) => AtomicType::String,
        Some(Kind::Document | Kind::Element | Kind::Attribute | Kind::Text) | None => AtomicType::UntypedAtomic,
    };
    Atomic { kind, lexical: tree.string_value(id).into() }
}

/// The error of a value XMLCAST cannot give, `why`, by `rule` of §6.6,
/// with SQLSTATE `state`.
fn refused(why: &str, rule: &str, state: SqlState) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §6.6 {rule}] {why}")).with_state(state)
}

/// [§6.6 GR4k] The SQL value of `bv`, an atomic value of the XML Schema
/// type an SQL type maps to, XT, for that type: a
/// string \[GR4k i\], the octets of hex \[GR4k ii\], an exact number
/// \[GR4k iv\], an approximate one \[GR4k v\], a truth value \[GR4k
/// xi\], an interval \[GR4k ix, GR4k x\], or a datetime's literal's
/// string, CAST to the datetime type by the binder \[GR4k vi to viii\].
fn sql_value(bv: &Atomic) -> Result<Value> {
    let text = &*bv.lexical;
    Ok(match bv.kind.primitive() {
        // [GR4k i] a string; XT is none of the other types here, as §9.5
        // maps no SQL type to them.
        Primitive::String
        | Primitive::Untyped
        | Primitive::AnyUri
        | Primitive::QName
        | Primitive::Notation
        | Primitive::Duration
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth => Value::Char(text.to_owned()),
        Primitive::HexBinary => Value::Binary(Binary::new(hex(text).unwrap_or_default())),
        Primitive::Base64Binary => Value::Binary(Binary::new(base64(text).unwrap_or_default())),
        Primitive::Decimal | Primitive::Integer => Value::Exact(exact(text)?),
        Primitive::Float | Primitive::Double => Value::Approx(approximate(text)?),
        Primitive::Boolean => Value::Bool(text == "true"),
        Primitive::YearMonthDuration => interval(text, true)?,
        Primitive::DayTimeDuration => interval(text, false)?,
        Primitive::Date => Value::Char(datetime(text, Shape::Date)?),
        Primitive::Time => Value::Char(datetime(text, Shape::Time)?),
        Primitive::DateTime => Value::Char(datetime(text, Shape::DateTime)?),
    })
}

/// [§6.6 GR4k iv] The exact number of the decimal string `text`, digits
/// of its fraction past what `MAX_DIGITS` hold dropped.
///
/// # Errors
///
/// SQLSTATE 22003 if its whole part has more digits than that.
fn exact(text: &str) -> Result<Decimal> {
    let (negative, body) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let room = MAX_DIGITS.saturating_sub(whole.len());
    let fraction = fraction.get(..fraction.len().min(room)).unwrap_or_default();
    let units: i128 = format!("{whole}{fraction}").parse().map_err(|_| {
        let why = format!("{text} has more digits than an exact number holds");
        refused(&why, "GR4k iv", SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
    })?;
    let scale = u32::try_from(fraction.len()).unwrap_or(0);
    Ok(Decimal::new(if negative { units.saturating_neg() } else { units }, scale))
}

/// [§6.6 GR4k v] The approximate number of the float or double string
/// `text`.
///
/// # Errors
///
/// SQLSTATE 22003 of INF, -INF or NaN \[GR4k v 1\].
fn approximate(text: &str) -> Result<f64> {
    let x = text.parse::<f64>().ok().filter(|x| x.is_finite());
    x.ok_or_else(|| {
        refused(&format!("{text} is no number of an approximate numeric type"), "GR4k v 1", SqlState::NUMERIC_VALUE_OUT_OF_RANGE)
    })
}

/// [§6.6 GR4k ix, GR4k x] The interval of the canonical duration `text`:
/// months of YEAR(9) TO MONTH if `year_month`, else microseconds of
/// DAY(9) TO SECOND(6).
fn interval(text: &str, year_month: bool) -> Result<Value> {
    let units = duration_units(text, year_month)?;
    let qualifier = if year_month {
        Qualifier::new(Field::Year, Field::Month, MAX_LEADING_PRECISION, 0)
    } else {
        Qualifier::new(Field::Day, Field::Second, MAX_LEADING_PRECISION, MAX_TIME_PRECISION)
    };
    Ok(Value::Interval(Interval { units, qualifier }))
}

/// [§6.6 GR4j, GR4k vi to viii] The literal's string of the date, time or
/// date and time `text`, of `shape`, a value of XT: of a type WITH TIME
/// ZONE with its time zone, which XT's pattern requires [§9.5 GR8i,
/// GR8k], so \[GR4k vii 3A, GR4k viii 2A\] find none missing; of one
/// without, adjusted to UTC already \[GR4j i to iii\].
///
/// # Errors
///
/// SQLSTATE 22007 if its year in UTC is not from 0001 to 9999 \[GR4k vi
/// 1, GR4k vii 1\].
fn datetime(text: &str, shape: Shape) -> Result<String> {
    let invalid = SqlState::INVALID_DATETIME_FORMAT;
    let m = Moment::parse(text, shape)?.ok_or_else(|| refused(&format!("'{text}' is no date or time"), "GR4k", invalid))?;
    let year = m.in_utc().year();
    if shape != Shape::Time && !(1..=LAST_YEAR).contains(&year) {
        return Err(refused(&format!("'{text}' is of the year {year} in UTC"), "GR4k vi 1, GR4k vii 1", invalid));
    }
    Ok(m.sql_literal())
}
