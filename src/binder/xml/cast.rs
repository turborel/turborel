//! Binding XMLCAST [ISO/IEC 9075-14:2006 draft §6.6]: of an XML value to
//! an XML type, CAST with a passing mechanism [SR10b ii]; of an SQL value
//! to an XML type, or of an XML value to an SQL type, a call of its
//! function (`functions/xml/cast.rs`), the second then CAST to the SQL
//! type [GR4l].

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use crate::ast::{Passing, TypeName, ValueExpression, ValueSpecification};
use crate::error::{Code, Result, SqlError};
use crate::functions::{Mapping, ScalarRef, XmlCastIn, XmlCastOut, xml_schema_restriction, xml_schema_type};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::datetime::{Field, MAX_TIME_PRECISION};
use crate::value::interval::{MAX_LEADING_PRECISION, Qualifier};
use crate::value::lob::{LobKind, MAX_LOB_LENGTH};
use crate::value::xml::XmlType;
use crate::value::{
    CharUnits, DOUBLE_PRECISION, DataType, MAX_BINARY_LENGTH, MAX_CHARACTER_LENGTH, MAX_EXACT_PRECISION, REAL_PRECISION, Spelled, Value,
};
use crate::xml::{AtomicType, Primitive};

/// [§6.6 SR5, SR10b i] Whether `t` is a collection, row, structured or
/// reference type, which XMLCAST takes neither to nor from; a distinct
/// type is none of them, whatever its source.
fn unmappable(t: &DataType) -> bool {
    matches!(t, DataType::Array(_) | DataType::Multiset(_) | DataType::Row(_) | DataType::Structured(_) | DataType::Reference(_))
}

/// Whether `v` is a `<dynamic parameter specification>`, perhaps in
/// parentheses [§6.6 SR10a].
fn is_parameter(v: &ValueExpression) -> bool {
    let mut at = v;
    while let ValueExpression::Nested(inner) = at {
        at = inner;
    }
    matches!(at, ValueExpression::Value(ValueSpecification::Parameter(_)))
}

/// The type of the SQL value XMLCAST gives of an XML value of the XML
/// Schema type `base`, before it is CAST to `sqlt` \[GR4k\]: a string,
/// binary string or large object of the most characters or octets; an
/// exact number of the most digits; REAL or DOUBLE PRECISION; BOOLEAN; a
/// datetime's literal's string; or an interval of the most leading digits
/// and, of seconds, microseconds.
fn given_type(base: AtomicType, sqlt: &DataType) -> DataType {
    let lob = |kind| DataType::Lob { kind, length: MAX_LOB_LENGTH, units: CharUnits::default() };
    let varying = DataType::Varying { length: MAX_CHARACTER_LENGTH, units: CharUnits::default() };
    let large = matches!(sqlt, DataType::Lob { .. });
    match base.primitive() {
        Primitive::String | Primitive::Untyped | Primitive::AnyUri | Primitive::QName | Primitive::Notation => {
            if large {
                lob(LobKind::Character)
            } else {
                varying
            }
        }
        Primitive::HexBinary | Primitive::Base64Binary => {
            if large {
                lob(LobKind::Binary)
            } else {
                DataType::VarBinary { length: MAX_BINARY_LENGTH }
            }
        }
        Primitive::Decimal | Primitive::Integer => DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        Primitive::Float => DataType::Approximate { precision: REAL_PRECISION, spelled: Spelled::Real },
        Primitive::Double => DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision },
        Primitive::Boolean => DataType::Boolean,
        Primitive::YearMonthDuration => DataType::Interval(Qualifier::new(Field::Year, Field::Month, MAX_LEADING_PRECISION, 0)),
        Primitive::DayTimeDuration => {
            DataType::Interval(Qualifier::new(Field::Day, Field::Second, MAX_LEADING_PRECISION, MAX_TIME_PRECISION))
        }
        Primitive::Date
        | Primitive::Time
        | Primitive::DateTime
        | Primitive::Duration
        | Primitive::GYearMonth
        | Primitive::GYear
        | Primitive::GMonthDay
        | Primitive::GDay
        | Primitive::GMonth => varying,
    }
}

impl Binder<'_> {
    /// [§6.6] XMLCAST ( `operand`, or NULL if none, AS `named` `passing` ):
    /// of an XML type or to one \[SR4\], neither of nor to a collection,
    /// row, structured or reference type \[SR5, SR10b i\], a passing
    /// mechanism of XML types alone \[SR9\]; a `?`, or NULL, CAST to the
    /// type \[SR10a, SR11\].
    pub(in crate::binder) fn xml_cast_specification(
        &self,
        operand: Option<&ValueExpression>,
        named: &TypeName,
        passing: Option<Passing>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        // [SR3, SR13, AR3] a domain's data type, USAGE on it held.
        let (target, _) = self.db.resolve_type(named, (self.schema, self.who()))?;
        if unmappable(&target) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.6 SR5] XMLCAST casts to no value of {target}")));
        }
        let to_xml = matches!(target, DataType::Xml(_));
        let Some(v) = operand.filter(|v| !is_parameter(v)) else {
            if !to_xml {
                return Err(rule(format!(
                    "[ISO/IEC 9075-14:2006 draft §6.6 SR4] XMLCAST of NULL or a parameter casts to an XML type, not {target}"
                )));
            }
            let Some(v) = operand else { return Ok((Expr::Const(Value::Null), target)) };
            let bound = self.typed(v, ParameterType::Of(target.clone()), scope)?;
            return super::xml_cast(bound, target, passing.unwrap_or(Passing::ByRef));
        };
        let (e, source) = self.value_expression(v, scope)?;
        if unmappable(&source) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.6 SR10b i] XMLCAST casts no value of {source}")));
        }
        let from_xml = matches!(source, DataType::Xml(_));
        if passing.is_some() && !(from_xml && to_xml) {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.6 SR9b] XMLCAST of {source} to {target} has no <XML passing mechanism>"
            )));
        }
        match (from_xml, to_xml) {
            // [SR10b ii] CAST ( VE AS TD XPM ), BY REF implicit.
            (true, true) => super::xml_cast((e, source), target, passing.unwrap_or(Passing::ByRef)),
            (false, true) => self.cast_to_xml(e, &source, target),
            (true, false) => self.cast_from_xml(e, target),
            (false, false) => {
                Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.6 SR4] XMLCAST of {source} to {target}: neither is an XML type")))
            }
        }
    }

    /// [§6.6 GR3] XMLCAST of `e`, of the SQL type `source`, to `target`, an
    /// XML type: not DOCUMENT \[SR7\]; of a type §9.5 maps, binary strings
    /// in the `<XML binary encoding>` in scope \[SR1, SR2\]; a datetime
    /// without time zone as the SQL-session shows it [§9.8 GR7].
    pub(super) fn cast_to_xml(&self, e: Expr, source: &DataType, target: DataType) -> Result<(Expr, DataType)> {
        let DataType::Xml(xml) = target else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.6 SR4] {target} is no XML type")));
        };
        if matches!(xml, XmlType::Document(_)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.6 SR7] XMLCAST to {target} casts an XML value or NULL, not a value of {source}"
            )));
        }
        if matches!(source.represented(), DataType::Array(_) | DataType::Multiset(_)) {
            return Ok((self.collection_to_xml(e, source, xml)?, target));
        }
        // [SR1, SR2] binary strings in the encoding in scope.
        let encoding = self.binary_encoding();
        let Some(base) = xml_schema_type(source, encoding) else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §9.5 GR8] {source} maps to no XML Schema type")));
        };
        let shown = super::super::datetime::shown(e, source.represented().clone(), self.now);
        Ok((Expr::Call(ScalarRef(Arc::new(XmlCastIn { target: xml, base, collection: None, encoding })), vec![shown]), target))
    }

    /// [§6.6 GR3, §9.8 GR1a, GR9j] XMLCAST of `e`, of `source`, a distinct
    /// type whose source is a collection type, which SR10b i does not
    /// refuse, to the XML type `xml`: to CONTENT, its value mapped by its
    /// type \[GR3c\]; to SEQUENCE, SQLCODE -900, as GR3d casts to an
    /// atomic type and §9.5 maps no collection to one \[GR3d ii\].
    fn collection_to_xml(&self, e: Expr, source: &DataType, xml: XmlType) -> Result<Expr> {
        if xml == XmlType::Sequence {
            return Err(SqlError::new(
                Code::Unsupported,
                format!("[ISO/IEC 9075-14:2006 draft §6.6 GR3d ii] XMLCAST of {source} to XML(SEQUENCE) casts to no atomic type"),
            ));
        }
        if !super::element::mapped_inside(source.represented(), super::element::MAPPED_LEVELS) {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §9.8 GR9j, §4.10.8] {source} is based on a type mapped to no XML: XML(SEQUENCE), a structured or a reference type, or one §9.5 does not map"
            )));
        }
        // [SR1, SR2] The encoding in scope; [GR3a] characters mapped.
        let encoding = self.binary_encoding();
        let mapping = Mapping { encoding, charmapping: true, zone: self.now.zone(), nil: false };
        // The atomic type is not read, as the value is cast to CONTENT.
        let cast = XmlCastIn { target: xml, base: AtomicType::UntypedAtomic, collection: Some((source.clone(), mapping)), encoding };
        Ok(Expr::Call(ScalarRef(Arc::new(cast)), vec![e]))
    }

    /// [§6.6 GR4] XMLCAST of `e`, an XML value, to `target`, an SQL type:
    /// a domain's data type, or a distinct type's source type, SQLT
    /// \[GR4a\], of a type §9.5 maps, cast as the type it maps to with its
    /// facets, a binary string's in the encoding in scope \[GR4e, GR4j,
    /// SR1\]; the SQL value given CAST to SQLT \[GR4l\], of the declared
    /// type `target` \[SR6\].
    pub(super) fn cast_from_xml(&self, e: Expr, target: DataType) -> Result<(Expr, DataType)> {
        let sqlt = target.represented().clone();
        let Some(xmlt) = xml_schema_restriction(&sqlt, self.binary_encoding())? else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §9.5 GR8] {target} maps to no XML Schema type")));
        };
        let base = xmlt.atomic().unwrap_or(AtomicType::String);
        // [GR4j i to iii] a date or time without time zone adjusted first.
        let adjust = if matches!(sqlt, DataType::Timestamp { zoned: false, .. }) {
            Some(AtomicType::DateTime)
        } else if matches!(sqlt, DataType::Time { zoned: false, .. }) {
            Some(AtomicType::Time)
        } else {
            (sqlt == DataType::Date).then_some(AtomicType::Date)
        };
        let given = given_type(base, &sqlt);
        let fraction =
            if let DataType::Time { precision, .. } | DataType::Timestamp { precision, .. } = sqlt { Some(precision) } else { None };
        let function = XmlCastOut { xmlt: Arc::new(xmlt), adjust, fraction, given: given.clone() };
        let (cast, _) = self.cast_bound((Expr::Call(ScalarRef(Arc::new(function)), vec![e]), given), sqlt)?;
        Ok((cast, target))
    }
}
