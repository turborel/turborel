//! XMLCAST [ISO/IEC 9075-14:2006 draft §6.6] of an SQL value to an XML
//! type and of an XML value to an SQL type: the functions the binder
//! calls (`binder/xml/cast.rs`), through the mapping of SQL types to XML
//! Schema types [§9.5] and of SQL values to their lexical forms [§9.8].
//!
//! \[DESIGN\] RULES.md D86: \[§6.6 SR1b\] implementation-defined, with no
//! `<XML lexically scoped options>` the `<XML binary encoding>` is HEX
//! (`IMPLICIT_BINARY_ENCODING`), so a binary string is an `xs:hexBinary`. \[§9.5 GR8d iv\]
//! implementation-defined, INTEGER, SMALLINT and BIGINT are restrictions
//! of `xs:integer`; \[§9.5 GR8e viii, GR8f\] REAL and FLOAT of 24 bits or
//! fewer are `xs:float`, DOUBLE PRECISION and other FLOATs `xs:double`,
//! BOOLEAN `xs:boolean`. An XML value is cast to an SQL type through the
//! XML Schema type its type maps to, without the facets that type
//! restricts it by; SQL's CAST to the type applies its own length,
//! precision and range \[GR4l\].

use super::mapping::{BinaryEncoding, Mapping, xml_lexical, xml_mapped};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::functions::ScalarFunction;
use crate::value::datetime::{Field, MAX_TIME_PRECISION};
use crate::value::interval::{Interval, MAX_LEADING_PRECISION, Qualifier};
use crate::value::lob::LobKind;
use crate::value::xml::{XmlType, XmlValue};
use crate::value::{Binary, DataType, Decimal, Spelled, Value};
use crate::xml::{
    Atomic, AtomicType, Item, Kind, LAST_YEAR, Moment, NodeId, Sequence, Shape, Syntax, Tree, Whitespace, cast, duration_units, hex, parse,
    xquery_error,
};

/// [§6.6 SR1b, SR2, §6.14 GR1b, §6.15 GR1b, §6.19 GR3b]
/// implementation-defined: the `<XML binary encoding>` of XMLCAST,
/// XMLELEMENT, XMLFOREST and XMLTEXT outside the scope of one: HEX.
pub const IMPLICIT_BINARY_ENCODING: BinaryEncoding = BinaryEncoding::Hex;

/// The decimal digits an exact number's units hold at most.
const MAX_DIGITS: usize = 38;

/// [§9.5 GR8] The XML Schema built-in type the SQL type `t` maps to,
/// the type its value is cast to and from: of a character string
/// `xs:string`, of a binary string `xs:hexBinary`, of NUMERIC and DECIMAL
/// `xs:decimal`, of INTEGER, SMALLINT and BIGINT `xs:integer`, of an
/// approximate number `xs:float` or `xs:double`, of BOOLEAN `xs:boolean`,
/// of DATE, TIME and TIMESTAMP `xs:date`, `xs:time` and `xs:dateTime`,
/// and of a year-month or day-time interval `xdt:yearMonthDuration` or
/// `xdt:dayTimeDuration` [§6.6 GR3d ii]; none of another type.
#[must_use]
pub fn xml_schema_type(t: &DataType) -> Option<AtomicType> {
    Some(match t.represented() {
        DataType::Character { .. } | DataType::Varying { .. } | DataType::Lob { kind: LobKind::Character, .. } => AtomicType::String,
        DataType::Binary { .. } | DataType::VarBinary { .. } | DataType::Lob { kind: LobKind::Binary, .. } => AtomicType::HexBinary,
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

/// [§6.6 GR3] XMLCAST of an SQL value to an XML type: its lexical form
/// [§9.8], parsed as CONTENT \[GR3a, GR3b\], and of XML(SEQUENCE) cast as
/// the XML Schema type of its SQL type \[GR3d\].
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
            None => xml_lexical(v, IMPLICIT_BINARY_ENCODING, true)?,
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
/// is atomized \[GR4b, GR4c\], cast as the XML Schema type the SQL type
/// maps to \[GR4j\], as the SQL value that is \[GR4k\], which the binder
/// casts to the SQL type \[GR4l\].
#[derive(Debug)]
pub struct XmlCastOut {
    /// The XML Schema type the SQL type maps to.
    pub base: AtomicType,
    /// Whether the SQL type is a datetime type WITH TIME ZONE.
    pub zoned: bool,
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
        let bv = cast(&av, self.base)?;
        sql_value(&bv, self.zoned)
    }
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
/// type an SQL type maps to, for that type, WITH TIME ZONE if `zoned`: a
/// string \[GR4k i\], the octets of hex \[GR4k ii\], an exact number
/// \[GR4k iv\], an approximate one \[GR4k v\], a truth value \[GR4k
/// xi\], an interval \[GR4k ix, GR4k x\], or a datetime's literal's
/// string, CAST to the datetime type by the binder \[GR4k vi to viii\].
fn sql_value(bv: &Atomic, zoned: bool) -> Result<Value> {
    let text = &*bv.lexical;
    Ok(match bv.kind {
        AtomicType::String | AtomicType::UntypedAtomic => Value::Char(text.to_owned()),
        AtomicType::HexBinary => Value::Binary(Binary::new(hex(text).unwrap_or_default())),
        AtomicType::Decimal | AtomicType::Integer => Value::Exact(exact(text)?),
        AtomicType::Float | AtomicType::Double => Value::Approx(approximate(text)?),
        AtomicType::Boolean => Value::Bool(text == "true"),
        AtomicType::YearMonthDuration => interval(text, true)?,
        AtomicType::DayTimeDuration => interval(text, false)?,
        AtomicType::Date => Value::Char(datetime(text, Shape::Date, zoned)?),
        AtomicType::Time => Value::Char(datetime(text, Shape::Time, zoned)?),
        AtomicType::DateTime => Value::Char(datetime(text, Shape::DateTime, zoned)?),
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
/// date and time `text`, of `shape`, for a type WITH TIME ZONE if `zoned`;
/// for one without, adjusted to UTC and its time zone removed \[GR4j i
/// to iii\].
///
/// # Errors
///
/// SQLSTATE 22007 if its year is not from 0001 to 9999 \[GR4k vi 1, GR4k
/// vii 1\], or, for a type WITH TIME ZONE, it has no time zone \[GR4k vii
/// 3A, GR4k viii 2A\].
fn datetime(text: &str, shape: Shape, zoned: bool) -> Result<String> {
    let invalid = SqlState::INVALID_DATETIME_FORMAT;
    let m = Moment::parse(text, shape)?.ok_or_else(|| refused(&format!("'{text}' is no date or time"), "GR4k", invalid))?;
    if zoned && m.zone.is_none() {
        return Err(refused(&format!("'{text}' has no time zone"), "GR4k vii 3A, GR4k viii 2A", invalid));
    }
    let m = if zoned { m } else { m.in_utc() };
    let year = m.year();
    if shape != Shape::Time && !(1..=LAST_YEAR).contains(&year) {
        return Err(refused(&format!("'{text}' is of the year {year} in UTC"), "GR4k vi 1, GR4k vii 1", invalid));
    }
    Ok(m.sql_literal())
}
