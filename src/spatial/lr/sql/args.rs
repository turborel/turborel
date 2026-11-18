//! The parameters and results of the routines of §15 as SQL declares
//! them, and the arguments of a call read as the engine takes them.

use std::cell::Cell;
use std::sync::Arc;

use super::super::{Catalog, LinearElement, LrKind, LrValue};
use super::kind::{data_type_of, kind_of_value, of_data_type, of_value, payload, to_value};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::geometry::model::{Geom, Kind as GeomKind};
use crate::value::{DataType, Decimal, Spelled, Value, pow10};

/// The type of a parameter or of a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P {
    /// INTEGER.
    Int,
    /// DOUBLE PRECISION.
    Double,
    /// CHARACTER VARYING of `n` characters.
    Text(u32),
    /// CHARACTER of one character.
    Char,
    /// CHARACTER LARGE OBJECT.
    Clob,
    /// CHARACTER VARYING ARRAY.
    TextArray,
    /// A linear referencing type.
    Lr(LrKind),
    /// An ARRAY of a linear referencing type.
    LrArray(LrKind),
    /// A geometry type.
    Geo(GeomKind),
    /// SELF's declared type, of a type-preserving method [ISO/IEC
    /// 9075-2:2023 §9.18 SR10] (a result alone).
    SelfType,
}

/// \[DESIGN\] The cardinality of the arrays the routines return and take:
/// ST_MaxConstraintArrayElements, ST_MaxStartValueArrayElements,
/// ST_MaxReferentArrayElements and ST_MaxPositionExpArrayElements.
pub const MAX_ARRAY: u32 = 1024;

/// INTEGER.
pub const INTEGER: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };

impl P {
    /// Whether an argument of declared type `t` is taken here. A
    /// geometry type is taken for any geometry parameter, its type
    /// checked as the call reads it, as the geometry routines check theirs.
    pub fn takes(self, t: &DataType) -> bool {
        match self {
            P::Int | P::Double => t.is_numeric(),
            P::Text(_) | P::Char | P::Clob => t.is_character(),
            P::TextArray => element_of(t).is_some_and(DataType::is_character),
            P::Lr(k) => of_data_type(t).is_some_and(|own| own.is_subtype_of(k)),
            P::LrArray(k) => element_of(t).and_then(of_data_type).is_some_and(|own| own.is_subtype_of(k)),
            P::Geo(_) => crate::geometry::of_data_type(t).is_some(),
            P::SelfType => false,
        }
    }

    /// Whether the non-null value `v` may be an argument here.
    pub fn holds(self, v: &Value) -> bool {
        match self {
            P::Int | P::Double => v.as_f64().is_some(),
            P::Text(_) | P::Char | P::Clob => matches!(v, Value::Char(_) | Value::Lob(_)),
            P::TextArray | P::LrArray(_) => matches!(v, Value::Array(_)),
            P::Lr(k) => kind_of_value(v).is_some_and(|own| own.is_subtype_of(k)),
            P::Geo(_) => payload(v).is_none() && matches!(v, Value::Extension(_)),
            P::SelfType => false,
        }
    }

    /// The SQL type of it, as a result; SELF's type `own` of `SelfType`.
    pub fn data_type(self, own: Option<&DataType>) -> DataType {
        match self {
            P::Int => INTEGER,
            P::Double => DataType::Approximate { precision: crate::value::DOUBLE_PRECISION, spelled: Spelled::DoublePrecision },
            P::Text(n) => DataType::varying(n),
            P::Char => DataType::character(1),
            P::Clob => DataType::Lob {
                kind: crate::value::lob::LobKind::Character,
                length: u64::try_from(super::super::MAX_TEXT).unwrap_or(u64::MAX),
                units: crate::value::CharUnits::Characters,
            },
            P::TextArray => DataType::array(DataType::varying(super::CONSTRAINT_LENGTH), MAX_ARRAY),
            P::Lr(k) => data_type_of(k),
            P::LrArray(k) => DataType::array(data_type_of(k), MAX_ARRAY),
            P::Geo(k) => crate::geometry::data_type_of(k),
            P::SelfType => own.cloned().unwrap_or_else(|| data_type_of(LrKind::LinearElement)),
        }
    }
}

/// The element type of `t`, if it is an array type.
fn element_of(t: &DataType) -> Option<&DataType> {
    if let DataType::Array(a) = t { Some(&a.element) } else { None }
}

/// The arguments of a call, and what the catalogs give it.
pub struct Args<'a> {
    /// The arguments, SELF first of a method.
    pub values: &'a [Value],
    /// The catalog the call reads.
    pub catalog: Catalog<'a>,
    /// The completion condition the call raises, if one.
    pub warning: Cell<Option<SqlState>>,
}

/// A data exception: argument `at` is not of its parameter's type.
fn not_of(at: usize, what: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 §9.2 SR2] argument {} is not {what}", at.saturating_add(1)))
}

/// The text `v` holds, of a character string or a character large object.
///
/// # Errors
///
/// As reading the large object.
pub fn text_of(v: &Value) -> Result<Option<String>> {
    if let Value::Char(s) = v {
        return Ok(Some(s.clone()));
    }
    let Value::Lob(lob) = v else { return Ok(None) };
    if !matches!(lob.kind(), crate::value::lob::LobKind::Character) {
        return Ok(None);
    }
    let most = u64::try_from(super::super::MAX_TEXT).unwrap_or(u64::MAX);
    let bytes = lob.bounded(most)?.ok_or_else(|| {
        SqlError::new(Code::Data, "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.1 DR2] a text longer than ST_MaxLRAsText")
    })?;
    String::from_utf8(bytes).map(Some).map_err(|_| SqlError::new(Code::Data, "[DESIGN] the text is not UTF-8"))
}

/// The INTEGER `n`.
pub fn int(n: i32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// The CHARACTER VARYING `s`.
pub fn text(s: &str) -> Value {
    Value::Char(s.to_owned())
}

/// `v`, or the null value.
pub fn or_null<T>(v: Option<T>, f: impl FnOnce(T) -> Value) -> Value {
    v.map_or(Value::Null, f)
}

/// The value of `v`, a linear referencing value, or the null value.
///
/// # Errors
///
/// As `to_value`.
pub fn lr_out(v: Option<LrValue>) -> Result<Value> {
    v.map_or(Ok(Value::Null), |x| to_value(&x))
}

/// The value of `e`, a linear element, or the null value.
///
/// # Errors
///
/// As `to_value`.
pub fn element_out(e: Option<LinearElement>) -> Result<Value> {
    lr_out(e.map(LrValue::LinearElement))
}

/// An ARRAY of the linear referencing values `vs`.
///
/// # Errors
///
/// As `to_value`.
pub fn lr_array(vs: Vec<LrValue>) -> Result<Value> {
    let out = vs.into_iter().map(|v| to_value(&v)).collect::<Result<Vec<_>>>()?;
    Ok(Value::Array(Arc::from(out)))
}

/// The geometry value of `g`, or the null value.
pub fn geom_out(g: Option<&Geom>) -> Value {
    or_null(g, Geom::to_value)
}

impl Args<'_> {
    /// Argument `at`.
    fn at(&self, at: usize) -> Option<&Value> {
        self.values.get(at).filter(|v| !v.is_null())
    }

    /// Argument `at` as an INTEGER.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is no exact number of scale 0; SQLSTATE 22003
    /// past 32 bits.
    pub fn int(&self, at: usize) -> Result<Option<i32>> {
        let Some(v) = self.at(at) else { return Ok(None) };
        let units = match v {
            Value::Exact(d) => pow10(d.scale).filter(|p| d.units.checked_rem(*p) == Some(0)).and_then(|p| d.units.checked_div(p)),
            Value::Null
            | Value::Char(_)
            | Value::Approx(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Bool(_)
            | Value::Datetime(_)
            | Value::Interval(_)
            | Value::Lob(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Extension(_) => None,
        };
        let n = units.ok_or_else(|| not_of(at, "an INTEGER"))?;
        i32::try_from(n).map(Some).map_err(|_| {
            SqlError::new(Code::Data, format!("[ISO/IEC 9075:1992 §4.4] {n} is out of the range of INTEGER"))
                .with_state(SqlState::of_bytes(*b"22003"))
        })
    }

    /// Argument `at` as a DOUBLE PRECISION.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is no number.
    pub fn double(&self, at: usize) -> Result<Option<f64>> {
        self.at(at).map(|v| v.as_f64().ok_or_else(|| not_of(at, "a number"))).transpose()
    }

    /// Argument `at` as text.
    ///
    /// # Errors
    ///
    /// As `text_of`.
    pub fn text(&self, at: usize) -> Result<Option<String>> {
        let Some(v) = self.at(at) else { return Ok(None) };
        text_of(v)?.map(|s| Some(s.trim_end_matches(' ').to_owned())).ok_or_else(|| not_of(at, "a character string"))
    }

    /// Argument `at` as one character.
    ///
    /// # Errors
    ///
    /// As `text`.
    pub fn character(&self, at: usize) -> Result<Option<char>> {
        Ok(self.text(at)?.map(|s| s.chars().next().unwrap_or(' ')))
    }

    /// Argument `at` as an ARRAY of text, its null elements empty.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is no array of character strings.
    pub fn texts(&self, at: usize) -> Result<Option<Vec<String>>> {
        let Some(v) = self.at(at) else { return Ok(None) };
        let Value::Array(items) = v else { return Err(not_of(at, "an array")) };
        let one = |item: &Value| -> Result<String> {
            if item.is_null() {
                return Ok(String::new());
            }
            text_of(item)?.ok_or_else(|| not_of(at, "an array of character strings"))
        };
        items.iter().map(one).collect::<Result<Vec<_>>>().map(Some)
    }

    /// Argument `at` as a linear referencing value.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is none.
    pub fn lr(&self, at: usize) -> Result<Option<LrValue>> {
        self.at(at).map(of_value).transpose()
    }

    /// Argument `at` as an ARRAY of linear referencing values, its null
    /// elements left out.
    ///
    /// # Errors
    ///
    /// SQLCODE -301 if it is no array of them.
    pub fn lrs(&self, at: usize) -> Result<Option<Vec<LrValue>>> {
        let Some(v) = self.at(at) else { return Ok(None) };
        let Value::Array(items) = v else { return Err(not_of(at, "an array")) };
        items.iter().filter(|i| !i.is_null()).map(of_value).collect::<Result<Vec<_>>>().map(Some)
    }

    /// Argument `at` as a geometry value of `kind` or a subtype.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF08 if it is of another type.
    pub fn geom(&self, at: usize, kind: GeomKind) -> Result<Option<Geom>> {
        let Some(v) = self.at(at) else { return Ok(None) };
        let g = Geom::from_value(v)?;
        if g.kind().is_subtype_of(kind) {
            return Ok(Some(g));
        }
        Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-2:2023 §9.2 SR2] argument {} is not a value of {}", at.saturating_add(1), kind.sql_name()),
        )
        .with_state(SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE))
    }

    /// Raises the completion condition `state`.
    pub fn warn(&self, state: SqlState) {
        self.warning.set(Some(state));
    }
}

/// Extractors of argument `at` as a value of one linear referencing type.
macro_rules! extractor {
    ($fn:ident, $variant:ident, $t:ty, $what:literal) => {
        impl Args<'_> {
            #[doc = concat!("Argument `at` as ", $what, ".")]
            ///
            /// # Errors
            ///
            /// SQLCODE -301 if it is of another type.
            pub fn $fn(&self, at: usize) -> Result<Option<$t>> {
                let Some(v) = self.lr(at)? else { return Ok(None) };
                let LrValue::$variant(x) = v else { return Err(not_of(at, $what)) };
                Ok(Some(x))
            }
        }
    };
}

extractor!(lrm, Lrm, super::super::Lrm, "an ST_LRM value");
extractor!(element, LinearElement, LinearElement, "an ST_LinearElement value");
extractor!(position, PositionExp, super::super::PositionExp, "an ST_PositionExp value");
extractor!(measure, LrMeasure, super::super::LrMeasure, "an ST_LRMeasure value");
extractor!(start_value, StartValue, super::super::StartValue, "an ST_StartValue value");
extractor!(distance, DistanceExp, super::super::DistanceExp, "an ST_DistanceExp value");
extractor!(referent, Referent, super::super::Referent, "an ST_Referent value");
extractor!(lateral, LatOffsetExp, super::super::LatOffsetExp, "an ST_LatOffsetExp value");
extractor!(vertical, VerOffsetExp, super::super::VerOffsetExp, "an ST_VerOffsetExp value");
extractor!(vector, VectorOffsetExp, super::super::VectorOffsetExp, "an ST_VectorOffsetExp value");

/// Extractors of argument `at` as a value of one subtype of
/// ST_LinearElement.
macro_rules! subtype {
    ($fn:ident, $variant:ident, $t:ty, $what:literal) => {
        impl Args<'_> {
            #[doc = concat!("Argument `at` as ", $what, ".")]
            ///
            /// # Errors
            ///
            /// SQLCODE -301 if it is of another type.
            pub fn $fn(&self, at: usize) -> Result<Option<$t>> {
                let Some(e) = self.element(at)? else { return Ok(None) };
                let LinearElement::$variant(x) = e else { return Err(not_of(at, $what)) };
                Ok(Some(x))
            }
        }
    };
}

subtype!(feature, Feature, super::super::LrFeature, "an ST_LRFeature value");
subtype!(curve, Curve, super::super::LrCurve, "an ST_LRCurve value");
subtype!(edge, Edge, super::super::LrDirectedEdge, "an ST_LRDirectedEdge value");

/// Extractors of argument `at` as an ARRAY of one linear referencing type.
macro_rules! arrays {
    ($fn:ident, $variant:ident, $t:ty, $what:literal) => {
        impl Args<'_> {
            #[doc = concat!("Argument `at` as an ARRAY of ", $what, ".")]
            ///
            /// # Errors
            ///
            /// SQLCODE -301 if an element is of another type.
            pub fn $fn(&self, at: usize) -> Result<Option<Vec<$t>>> {
                let Some(all) = self.lrs(at)? else { return Ok(None) };
                let one = |v: LrValue| if let LrValue::$variant(x) = v { Ok(x) } else { Err(not_of(at, $what)) };
                all.into_iter().map(one).collect::<Result<Vec<_>>>().map(Some)
            }
        }
    };
}

arrays!(start_values, StartValue, super::super::StartValue, "ST_StartValue values");
arrays!(referents, Referent, super::super::Referent, "ST_Referent values");
