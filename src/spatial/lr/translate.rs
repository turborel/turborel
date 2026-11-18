//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.7, §15.2.8]
//! ST_TranslateToInst and ST_TranslateToType: a position along one linear
//! element as a position along others.
//!
//! The text leaves when a translation is possible to the implementation.
//! Implementation-defined choices:
//!
//! - A translation is possible when the source position is located (an
//!   ST_LRCurve of the catalog, or SELF if it names SELF) and the target
//!   is an ST_LRCurve of the same SRID whose LRM is in the catalog.
//! - The equivalent position is the one along the target nearest the
//!   source's place [§15.4.5 Description 5]. If the place is off the
//!   target, it is equivalent only by an offset, so the target LRM must
//!   allow offsets; else the translation is not possible.
//! - ST_TranslateToType takes, of the catalog's elements whose linear
//!   element type equals the one given, the ST_LRCurves the source place
//!   lies on, within their tolerance, each in the target LRM or else its
//!   own default LRM [§15.2.8 Description 2b]. None found is "cannot
//!   translate".
//!
//! A defect of the text: §15.2.8 Description 2 numbers an item "b+1)"
//! that repeats item c. It is read once.

use super::locate::{Located, distance_to, locate, locate_on};
use super::model::{DistanceExp, LinearElement, PositionExp};
use super::{Catalog, failed, required};
use crate::error::{Result, SqlError, SqlState};

/// [§15.2.8 Definitional Rule 1] ST_MaxPositionExpArrayElements,
/// implementation-defined: the most position expressions
/// ST_TranslateToType returns. \[DESIGN\] As many as a catalog's elements
/// would give past any real use, so the bound is reached only by error.
pub const MAX_POSITION_EXPS: usize = 1024;

impl LinearElement {
    /// [§15.2.7 Description 2] ST_TranslateToInst(ST_PositionExp,
    /// ST_LinearElement, INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 2a); 2FF87 (cannot translate) (Description 2c).
    pub fn translate_to_inst(
        &self,
        asourcepositionexpression: Option<&PositionExp>,
        atargetlinearelement: Option<&LinearElement>,
        atarget_lrm: Option<i32>,
        catalog: &Catalog<'_>,
    ) -> Result<DistanceExp> {
        const RULE: &str = "§15.2.7 Description 2a";
        let source = required(asourcepositionexpression, RULE, "asourcepositionexpression")?;
        let target = required(atargetlinearelement, RULE, "atargetlinearelement")?;
        let lrm_id = atarget_lrm.unwrap_or(target.base().default_lrm);
        let rule = "§15.2.7 Description 2c";
        let place = self.source_place(source, catalog).map_err(|e| cannot(rule, &e.msg))?;
        let curve = target.as_curve().ok_or_else(|| cannot(rule, "the target is not an ST_LRCurve"))?;
        if curve.curve.srid != place.srid {
            return Err(cannot(rule, "the target is of another spatial reference system"));
        }
        let lrm = catalog.lrm(lrm_id).ok_or_else(|| cannot(rule, &format!("no LRM has the lrmid {lrm_id}")))?;
        let (d, off) = distance_to(curve, lrm, &place.at, catalog).map_err(|e| cannot(rule, &e.msg))?;
        if off > 0.0_f64 && !lrm.allows_offsets() {
            return Err(cannot(rule, "the place is off the target, and the target LRM allows no offsets"));
        }
        Ok(d)
    }

    /// [§15.2.8 Description 2] ST_TranslateToType(ST_PositionExp,
    /// CHARACTER VARYING, INTEGER), of the candidates in
    /// `catalog.elements`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 2a); 2FF87 (Description 2c); 2202F (array data, right
    /// truncation) past `MAX_POSITION_EXPS`.
    pub fn translate_to_type(
        &self,
        asourcepositionexpression: Option<&PositionExp>,
        atargetlinearelementtype: Option<&str>,
        atarget_lrm: Option<i32>,
        catalog: &Catalog<'_>,
    ) -> Result<Vec<PositionExp>> {
        const RULE: &str = "§15.2.8 Description 2a";
        let source = required(asourcepositionexpression, RULE, "asourcepositionexpression")?;
        let element_type = required(atargetlinearelementtype, RULE, "atargetlinearelementtype")?;
        let rule = "§15.2.8 Description 2c";
        let place = self.source_place(source, catalog).map_err(|e| cannot(rule, &e.msg))?;
        let of_type = catalog.elements.iter().filter(|e| e.base().element_type.as_deref() == Some(element_type));
        let found: Vec<PositionExp> = of_type.filter_map(|e| on_element(e, &place, atarget_lrm, catalog)).collect();
        if found.is_empty() {
            return Err(cannot(rule, &format!("no linear element of type {element_type} holds the place")));
        }
        if found.len() > MAX_POSITION_EXPS {
            return Err(failed(
                SqlState::ARRAY_DATA_RIGHT_TRUNCATION,
                "§15.2.8 Definitional Rule 1",
                &format!("the translation gives {} positions, past ST_MaxPositionExpArrayElements", found.len()),
            ));
        }
        Ok(found)
    }

    /// The place `source` locates: along SELF if it names SELF and SELF is
    /// a curve, else along the curve it names.
    fn source_place(&self, source: &PositionExp, catalog: &Catalog<'_>) -> Result<Located> {
        let own = self.as_curve().filter(|c| source.element_id == c.base.id && source.element.is_none());
        match own {
            Some(curve) => locate_on(curve, source, catalog, 0),
            None => locate(source, catalog, 0),
        }
    }
}

/// The position along `e`, if it is an ST_LRCurve of the place's SRID
/// that holds `place`, in `lrm_id` or else its default LRM.
fn on_element(e: &LinearElement, place: &Located, lrm_id: Option<i32>, catalog: &Catalog<'_>) -> Option<PositionExp> {
    let curve = e.as_curve().filter(|c| c.curve.srid == place.srid)?;
    let lrm = catalog.lrm(lrm_id.unwrap_or(curve.base.default_lrm))?;
    let (d, off) = distance_to(curve, lrm, &place.at, catalog).ok()?;
    (off <= 0.0_f64).then(|| PositionExp::of_ids(curve.base.id, lrm.id, d))
}

/// SQLSTATE 2FF87, cited as `rule`, saying why.
fn cannot(rule: &str, why: &str) -> SqlError {
    failed(SqlState::SPATIAL_CANNOT_TRANSLATE, rule, &format!("cannot translate: {why}"))
}
