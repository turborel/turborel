//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15] Linear referencing: the
//! pure engine under the SQL types of §15, with no SQL dependency.
//!
//! It holds a value of each of the thirteen types (`LrKind`, `LrValue`,
//! the structs of `lr/model.rs`), each constructor, observer and mutator
//! of §15.1 to §15.13 as one Rust function implementing its Description
//! (`lr/lrm.rs`, `lr/element.rs`, `lr/position.rs`, `lr/distance.rs`,
//! `lr/referent.rs`), the stored form (`lr/codec.rs`), the well-known text
//! of §15.14 (`lr/wkt.rs`), a GML representation (`lr/gml.rs`), locating
//! a position along a curve (`lr/measure.rs`, `lr/locate.rs`,
//! `lr/offset.rs`) and translating between linear elements
//! (`lr/translate.rs`).
//!
//! How the SQL layer calls it:
//!
//! - A mutator is an associated function taking `this: Option<&T>`, SELF
//!   or the null value, and its argument as `Option`, so each null rule of
//!   its Description is applied in the order the Description gives. It
//!   returns `None` where the Description returns the null value.
//! - A constructor taking more than five arguments takes a `...Parts`
//!   struct of them. A null-call constructor takes its arguments as plain
//!   values, since the SQL layer returns the null value before calling it.
//! - "a supported `<unit name>`" [§15.1.1 Description 16] is checked
//!   against a slice of `Unit`s, the LINEAR rows of ST_UNITS_OF_MEASURE
//!   (`linear_units` gives the seeded ones).
//! - What the catalogs hold (the LRMs of ST_LRMS, the linear elements a
//!   position expression names by id) comes in a `Catalog`.
//! - `encode`, `as_text` and `as_gml` return `Result`, since a value nested
//!   past `MAX_DEPTH` is refused as R-CF-6 requires.
//! - Reading text or GML takes the `Catalog` too, as the constructors the
//!   text calls check units and referents against it, and GML takes the
//!   reference systems (`spatial::formats::gml::Srs`) a geometry's
//!   srsName names.

mod codec;
mod distance;
mod element;
mod gml;
mod locate;
mod lrm;
mod measure;
mod model;
mod offset;
mod position;
mod referent;
pub mod sql;
mod translate;
mod wkt;

pub use distance::ReferentNames;
pub use element::{EdgeParts, ElementParts};
pub use gml::{as_gml, from_gml};
pub use lrm::{ABSOLUTE, DOWN, INTERPOLATIVE, LEFT, LOCAL_INTERPOLATIVE, LrmParts, OffsetParts, RELATIVE, RIGHT, UP};
pub use measure::RELATIVE_TOLERANCE;
pub use model::{
    DistanceExp, ElementBase, LatOffsetExp, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrMeasure, Lrm, PositionExp, Referent,
    StartValue, Vector, VectorOffsetExp, VerOffsetExp,
};
pub use referent::MAX_VECTORS;
pub use translate::MAX_POSITION_EXPS;
pub use wkt::{as_text, from_text};

use crate::error::{Code, Result, SqlError, SqlState};
use crate::spatial::formats::gml::Srs;

/// The standard every citation of this module is to, as it is printed.
pub(crate) const STANDARD: &str = "ISO/IEC 13249-3:2016 (DIS, SC32 N2593)";

/// \[DESIGN\] How deep linear referencing values may nest, one level per
/// value inside another: a position expression's linear element, its
/// referents, their locations and so on (RULES.md R-CF-6). Decoding,
/// reading text or GML and writing them each refuse a deeper value.
pub const MAX_DEPTH: usize = 32;

/// \[DESIGN\] ST_MaxLRAsText and ST_MaxLRAsGML, implementation-defined
/// [§15.1.1 Definitional Rule 2, §15.1.12 Definitional Rule 1]: the
/// length any spatial text may have (RULES.md D85).
pub const MAX_TEXT: usize = crate::geometry::MAX_BYTES;

// ---------------------------------------------------------------------------
// [§15] The types
// ---------------------------------------------------------------------------

/// [§15] A linear referencing type, in the order §15 defines them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LrKind {
    /// [§15.1] ST_LRM.
    Lrm,
    /// [§15.2] ST_LinearElement.
    LinearElement,
    /// [§15.3] ST_LRFeature.
    LrFeature,
    /// [§15.4] ST_LRCurve.
    LrCurve,
    /// [§15.5] ST_LRDirectedEdge.
    LrDirectedEdge,
    /// [§15.6] ST_PositionExp.
    PositionExp,
    /// [§15.7] ST_LRMeasure.
    LrMeasure,
    /// [§15.8] ST_StartValue.
    StartValue,
    /// [§15.9] ST_DistanceExp.
    DistanceExp,
    /// [§15.10] ST_Referent.
    Referent,
    /// [§15.11] ST_LatOffsetExp.
    LatOffsetExp,
    /// [§15.12] ST_VerOffsetExp.
    VerOffsetExp,
    /// [§15.13] ST_VectorOffsetExp.
    VectorOffsetExp,
}

/// The number of linear referencing types [§15].
pub const KINDS: usize = 13;

/// [§15] Every linear referencing type, in the order §15 defines them.
pub const ALL: [LrKind; KINDS] = [
    LrKind::Lrm,
    LrKind::LinearElement,
    LrKind::LrFeature,
    LrKind::LrCurve,
    LrKind::LrDirectedEdge,
    LrKind::PositionExp,
    LrKind::LrMeasure,
    LrKind::StartValue,
    LrKind::DistanceExp,
    LrKind::Referent,
    LrKind::LatOffsetExp,
    LrKind::VerOffsetExp,
    LrKind::VectorOffsetExp,
];

impl LrKind {
    /// The type's name as the standard spells it: `ST_LRM`.
    pub const fn sql_name(self) -> &'static str {
        match self {
            LrKind::Lrm => "ST_LRM",
            LrKind::LinearElement => "ST_LinearElement",
            LrKind::LrFeature => "ST_LRFeature",
            LrKind::LrCurve => "ST_LRCurve",
            LrKind::LrDirectedEdge => "ST_LRDirectedEdge",
            LrKind::PositionExp => "ST_PositionExp",
            LrKind::LrMeasure => "ST_LRMeasure",
            LrKind::StartValue => "ST_StartValue",
            LrKind::DistanceExp => "ST_DistanceExp",
            LrKind::Referent => "ST_Referent",
            LrKind::LatOffsetExp => "ST_LatOffsetExp",
            LrKind::VerOffsetExp => "ST_VerOffsetExp",
            LrKind::VectorOffsetExp => "ST_VectorOffsetExp",
        }
    }

    /// The type's name as an SQL `<identifier>` folds it: `ST_LRFEATURE`.
    pub const fn type_name(self) -> &'static str {
        match self {
            LrKind::Lrm => "ST_LRM",
            LrKind::LinearElement => "ST_LINEARELEMENT",
            LrKind::LrFeature => "ST_LRFEATURE",
            LrKind::LrCurve => "ST_LRCURVE",
            LrKind::LrDirectedEdge => "ST_LRDIRECTEDEDGE",
            LrKind::PositionExp => "ST_POSITIONEXP",
            LrKind::LrMeasure => "ST_LRMEASURE",
            LrKind::StartValue => "ST_STARTVALUE",
            LrKind::DistanceExp => "ST_DISTANCEEXP",
            LrKind::Referent => "ST_REFERENT",
            LrKind::LatOffsetExp => "ST_LATOFFSETEXP",
            LrKind::VerOffsetExp => "ST_VEROFFSETEXP",
            LrKind::VectorOffsetExp => "ST_VECTOROFFSETEXP",
        }
    }

    /// The type named `name`, in either case.
    pub fn named(name: &str) -> Option<LrKind> {
        ALL.into_iter().find(|k| k.type_name().eq_ignore_ascii_case(name))
    }

    /// [§15.3.1, §15.4.1, §15.5.1] Its supertype: ST_LinearElement of
    /// ST_LRFeature, ST_LRCurve and ST_LRDirectedEdge; none of the others.
    pub const fn supertype(self) -> Option<LrKind> {
        match self {
            LrKind::LrFeature | LrKind::LrCurve | LrKind::LrDirectedEdge => Some(LrKind::LinearElement),
            LrKind::Lrm
            | LrKind::LinearElement
            | LrKind::PositionExp
            | LrKind::LrMeasure
            | LrKind::StartValue
            | LrKind::DistanceExp
            | LrKind::Referent
            | LrKind::LatOffsetExp
            | LrKind::VerOffsetExp
            | LrKind::VectorOffsetExp => None,
        }
    }

    /// Whether it is `other` or a subtype of it.
    pub fn is_subtype_of(self, other: LrKind) -> bool {
        self == other || self.supertype() == Some(other)
    }

    /// [§15.2.1] Whether values of it are made: all but ST_LinearElement,
    /// the one CREATE TYPE of §15 that is NOT INSTANTIABLE.
    pub const fn is_instantiable(self) -> bool {
        !matches!(self, LrKind::LinearElement)
    }

    /// \[DESIGN\] Its code in the stored form: its position in `ALL`.
    pub(crate) fn code(self) -> u8 {
        let at = ALL.iter().position(|k| *k == self).unwrap_or_default();
        u8::try_from(at).unwrap_or_default()
    }

    /// The type of stored code `code`.
    pub(crate) fn of_code(code: u8) -> Option<LrKind> {
        ALL.get(usize::from(code)).copied()
    }
}

/// A value of any instantiable linear referencing type.
#[derive(Debug, Clone, PartialEq)]
pub enum LrValue {
    /// [§15.1] An ST_LRM value.
    Lrm(Lrm),
    /// [§15.3, §15.4, §15.5] A value of a subtype of ST_LinearElement.
    LinearElement(LinearElement),
    /// [§15.6] An ST_PositionExp value.
    PositionExp(PositionExp),
    /// [§15.7] An ST_LRMeasure value.
    LrMeasure(LrMeasure),
    /// [§15.8] An ST_StartValue value.
    StartValue(StartValue),
    /// [§15.9] An ST_DistanceExp value.
    DistanceExp(DistanceExp),
    /// [§15.10] An ST_Referent value.
    Referent(Referent),
    /// [§15.11] An ST_LatOffsetExp value.
    LatOffsetExp(LatOffsetExp),
    /// [§15.12] An ST_VerOffsetExp value.
    VerOffsetExp(VerOffsetExp),
    /// [§15.13] An ST_VectorOffsetExp value.
    VectorOffsetExp(VectorOffsetExp),
}

impl LrValue {
    /// Its most specific type.
    pub fn kind(&self) -> LrKind {
        match self {
            LrValue::Lrm(_) => LrKind::Lrm,
            LrValue::LinearElement(e) => e.kind(),
            LrValue::PositionExp(_) => LrKind::PositionExp,
            LrValue::LrMeasure(_) => LrKind::LrMeasure,
            LrValue::StartValue(_) => LrKind::StartValue,
            LrValue::DistanceExp(_) => LrKind::DistanceExp,
            LrValue::Referent(_) => LrKind::Referent,
            LrValue::LatOffsetExp(_) => LrKind::LatOffsetExp,
            LrValue::VerOffsetExp(_) => LrKind::VerOffsetExp,
            LrValue::VectorOffsetExp(_) => LrKind::VectorOffsetExp,
        }
    }

    /// \[DESIGN\] Its stored form: the version octet 1, its kind's code,
    /// then its attributes (`lr/codec.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a value nested past `MAX_DEPTH`.
    pub fn encode(&self) -> Result<Vec<u8>> {
        codec::encode(self)
    }

    /// The value `encode` wrote.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 of bytes `encode` did not write.
    pub fn decode(bytes: &[u8]) -> Result<LrValue> {
        codec::decode(bytes)
    }
}

/// [§15.1.2 Description 2, §15.3.2 Description 2 and their peers] The
/// value of a CHARACTER LARGE OBJECT a one-argument constructor takes:
/// its GML representation if it holds an XML element, else its
/// well-known text. Implementation-defined reading: it holds one if its
/// first character other than a space is `<`, which no well-known text
/// begins with.
///
/// # Errors
///
/// As `from_gml` or `from_text`.
pub fn from_text_or_gml(text: &str, expected: LrKind, catalog: &Catalog<'_>, srs: &dyn Srs) -> Result<LrValue> {
    if text.trim_start().starts_with('<') { from_gml(text, expected, catalog, srs) } else { from_text(text, expected, catalog) }
}

// ---------------------------------------------------------------------------
// [§15.1.1 Description 16] Supported units
// ---------------------------------------------------------------------------

/// A linear unit of measure: its UNIT_NAME and how many metres one is,
/// its CONVERSION_FACTOR [§20.4].
pub type Unit<'a> = (&'a str, f64);

/// [§15.1.1 Description 16] The UNIT_TYPE of the units a linear
/// referencing value may name.
pub const LINEAR: &str = "LINEAR";

/// [§15.1.1 Description 16] The units ST_UNITS_OF_MEASURE is seeded with
/// whose UNIT_TYPE is 'LINEAR', read from the seed so they are written
/// once (`crate::system::UNITS`).
pub fn linear_units() -> Vec<Unit<'static>> {
    crate::system::UNITS.iter().filter(|&&(_, kind, _, _)| kind == LINEAR).map(|&(name, _, factor, _)| (name, factor)).collect()
}

/// The metres in one `unit` of `units`. Implementation-defined reading:
/// a unit name matches a UNIT_NAME in any case, as the seeded names are
/// upper case and the standard's text writes them in lower case.
pub(crate) fn metres_in(unit: &str, units: &[Unit<'_>]) -> Option<f64> {
    units.iter().find(|(name, _)| name.eq_ignore_ascii_case(unit)).map(|&(_, factor)| factor)
}

/// [§15.1.1 Description 16] Checks that `unit` is a supported `<unit name>`.
///
/// # Errors
///
/// SQLSTATE 2FF14 (unsupported unit specified), cited as `rule`.
pub(crate) fn supported(unit: &str, units: &[Unit<'_>], rule: &str) -> Result<()> {
    match metres_in(unit, units) {
        Some(_) => Ok(()),
        None => Err(failed(SqlState::SPATIAL_UNSUPPORTED_UNIT_SPECIFIED, rule, &format!("unsupported unit specified: {unit}"))),
    }
}

// ---------------------------------------------------------------------------
// What the catalogs supply
// ---------------------------------------------------------------------------

/// What the SQL layer resolves from its catalogs for the routines that
/// need more than their arguments.
#[derive(Debug, Clone, Copy)]
pub struct Catalog<'a> {
    /// [§15.1.1 Description 16] The supported linear units.
    pub units: &'a [Unit<'a>],
    /// [§21.2.3] The LRMs of ST_LRMS, found by their ST_PrivateLRMID.
    pub lrms: &'a [Lrm],
    /// The linear elements a position expression or a referent may name
    /// by id or feature id: the candidates the SQL layer found.
    pub elements: &'a [LinearElement],
    /// How many metres one unit of a curve's coordinates is: the linear
    /// unit of its spatial reference system, 1 if it has none.
    pub metres_per_unit: f64,
}

impl<'a> Catalog<'a> {
    /// A catalog of `units` alone, of no LRM and no element, its
    /// coordinates in metres.
    pub fn of_units(units: &'a [Unit<'a>]) -> Catalog<'a> {
        Catalog { units, lrms: &[], elements: &[], metres_per_unit: 1.0_f64 }
    }

    /// The LRM whose ST_PrivateLRMID is `id`.
    pub fn lrm(&self, id: i32) -> Option<&'a Lrm> {
        self.lrms.iter().find(|l| l.id == id)
    }

    /// The linear element whose ST_PrivateLinearElementID is `id`.
    pub fn element(&self, id: i32) -> Option<&'a LinearElement> {
        self.elements.iter().find(|e| e.base().id == id)
    }

    /// The feature whose ST_PrivateFeatureID is `id`.
    pub fn feature(&self, id: &str) -> Option<&'a LrFeature> {
        self.elements.iter().filter_map(LinearElement::as_feature).find(|f| f.feature_id == id)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An exception condition of §22 Table 16: SQLSTATE `state`, citing
/// `rule` of this standard.
pub(crate) fn failed(state: SqlState, rule: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[{STANDARD} {rule}] {detail}")).with_state(state)
}

/// `arg`, or SQLSTATE 2FF03 (null argument) if it is the null value.
pub(crate) fn required<T>(arg: Option<T>, rule: &str, name: &str) -> Result<T> {
    arg.ok_or_else(|| failed(SqlState::SPATIAL_NULL_ARGUMENT, rule, &format!("null argument: {name}")))
}

/// SQLCODE -900: a value nested past `MAX_DEPTH` (R-CF-6).
pub(crate) fn too_deep() -> SqlError {
    SqlError::new(Code::Unsupported, "[DESIGN] a linear referencing value is nested past its depth limit (RULES.md R-CF-6)")
}

/// The depth below `depth`, or `too_deep` past `MAX_DEPTH`.
pub(crate) fn deeper(depth: usize) -> Result<usize> {
    let next = depth.saturating_add(1);
    if next > MAX_DEPTH {
        return Err(too_deep());
    }
    Ok(next)
}

#[cfg(test)]
mod tests;
