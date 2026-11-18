//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.6.2 to §15.6.7, §15.7.2
//! to §15.7.4, §15.8.2 to §15.8.4] The constructors, observers and
//! mutators of ST_PositionExp, ST_LRMeasure and ST_StartValue.
//! ST_Equals is in `lr/locate.rs`.
//!
//! A defect of the text read as meant: ST_StartValue's CREATE TYPE names
//! its observer of ST_PrivateMeasure ST_LRMeasure() [§15.8.1], where its
//! Description 1d and §15.8.4 name it ST_Measure(); it is `measure` here.

use super::model::{DistanceExp, LinearElement, LrMeasure, Lrm, PositionExp, StartValue};
use super::{Unit, required, supported};
use crate::error::Result;

impl PositionExp {
    /// [§15.6.2 Description 4] ST_PositionExp(INTEGER, INTEGER,
    /// ST_DistanceExp), null-call.
    pub fn of_ids(anleid: i32, anlrmid: i32, adistanceexpression: DistanceExp) -> PositionExp {
        PositionExp { element_id: anleid, element: None, lrm_id: anlrmid, lrm: None, distance: adistanceexpression }
    }

    /// [§15.6.2 Description 6] ST_PositionExp(INTEGER, ST_LRM,
    /// ST_DistanceExp), null-call.
    pub fn of_id_and_lrm(anleid: i32, anlrm: Lrm, adistanceexpression: DistanceExp) -> PositionExp {
        PositionExp { element_id: anleid, element: None, lrm_id: anlrm.id, lrm: Some(anlrm), distance: adistanceexpression }
    }

    /// [§15.6.2 Description 8] ST_PositionExp(ST_LinearElement, INTEGER,
    /// ST_DistanceExp), null-call.
    pub fn of_element_and_id(alinearelement: LinearElement, anlrmid: i32, adistanceexpression: DistanceExp) -> PositionExp {
        PositionExp {
            element_id: alinearelement.base().id,
            element: Some(Box::new(alinearelement)),
            lrm_id: anlrmid,
            lrm: None,
            distance: adistanceexpression,
        }
    }

    /// [§15.6.2 Description 10] ST_PositionExp(ST_LinearElement, ST_LRM,
    /// ST_DistanceExp), null-call. Description 10 names the signature of
    /// Description 8 by a slip; its items a to e are of this one.
    pub fn of_element_and_lrm(alinearelement: LinearElement, anlrm: Lrm, adistanceexpression: DistanceExp) -> PositionExp {
        PositionExp {
            element_id: alinearelement.base().id,
            element: Some(Box::new(alinearelement)),
            lrm_id: anlrm.id,
            lrm: Some(anlrm),
            distance: adistanceexpression,
        }
    }

    /// [§15.6.3 Description 2] ST_LinearElementID(). Its Definition
    /// returns SELF.ST_PrivateLinearElement by a slip; Description 2,
    /// followed here, returns ST_PrivateLinearElementID.
    pub fn linear_element_id(&self) -> i32 {
        self.element_id
    }

    /// [§15.6.3 Description 4] ST_LinearElementID(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 4a).
    pub fn with_linear_element_id(this: Option<&PositionExp>, anleid: Option<i32>) -> Result<Option<PositionExp>> {
        let element_id = required(anleid, "§15.6.3 Description 4a", "anleid")?;
        Ok(this.map(|p| PositionExp { element_id, ..p.clone() }))
    }

    /// [§15.6.4 Description 2] ST_LinearElement().
    pub fn linear_element(&self) -> Option<&LinearElement> {
        self.element.as_deref()
    }

    /// [§15.6.4 Description 4] ST_LinearElement(ST_LinearElement): a
    /// linear element sets the leid too (Description 4b ii); the null value leaves it
    /// (Description 4a ii).
    #[must_use]
    pub fn with_linear_element(this: Option<&PositionExp>, alinearelement: Option<LinearElement>) -> Option<PositionExp> {
        let p = this?;
        let element_id = alinearelement.as_ref().map_or(p.element_id, |e| e.base().id);
        Some(PositionExp { element_id, element: alinearelement.map(Box::new), ..p.clone() })
    }

    /// [§15.6.5 Description 2] ST_LRMID().
    pub fn lrm_id(&self) -> i32 {
        self.lrm_id
    }

    /// [§15.6.5 Description 4] ST_LRMID(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_lrm_id(this: Option<&PositionExp>, anlrmid: Option<i32>) -> Result<Option<PositionExp>> {
        let lrm_id = required(anlrmid, "§15.6.5 Description 4a", "anlrmid")?;
        Ok(this.map(|p| PositionExp { lrm_id, ..p.clone() }))
    }

    /// [§15.6.6 Description 2] ST_LRM().
    pub fn lrm(&self) -> Option<&Lrm> {
        self.lrm.as_ref()
    }

    /// [§15.6.6 Description 4] ST_LRM(ST_LRM): an LRM sets the lrmid too
    /// (Description 4b ii); the null value leaves it (Description 4a ii).
    #[must_use]
    pub fn with_lrm(this: Option<&PositionExp>, anlrm: Option<Lrm>) -> Option<PositionExp> {
        let p = this?;
        let lrm_id = anlrm.as_ref().map_or(p.lrm_id, |l| l.id);
        Some(PositionExp { lrm_id, lrm: anlrm, ..p.clone() })
    }

    /// [§15.6.7 Description 2] ST_DistanceExp().
    pub fn distance_exp(&self) -> &DistanceExp {
        &self.distance
    }

    /// [§15.6.7 Description 4] ST_DistanceExp(ST_DistanceExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_distance_exp(this: Option<&PositionExp>, adistanceexpression: Option<DistanceExp>) -> Result<Option<PositionExp>> {
        let distance = required(adistanceexpression, "§15.6.7 Description 4a", "adistanceexpression")?;
        Ok(this.map(|p| PositionExp { distance, ..p.clone() }))
    }
}

impl LrMeasure {
    /// [§15.7.2 Description 2] ST_LRMeasure(DOUBLE PRECISION), null-call:
    /// NEW ST_LRMeasure(ameasure, NULL), of no unit.
    pub fn new(ameasure: f64) -> LrMeasure {
        LrMeasure { measure: ameasure, units: None }
    }

    /// [§15.7.2 Description 4] ST_LRMeasure(DOUBLE PRECISION, CHARACTER
    /// VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a); 2FF14 (unsupported unit specified) (Description 4c iii).
    pub fn new_with_units(ameasure: Option<f64>, aunitofmeasure: Option<String>, units: &[Unit<'_>]) -> Result<LrMeasure> {
        let measure = required(ameasure, "§15.7.2 Description 4a", "ameasure")?;
        if let Some(unit) = &aunitofmeasure {
            supported(unit, units, "§15.7.2 Description 4c iii")?;
        }
        Ok(LrMeasure { measure, units: aunitofmeasure })
    }

    /// [§15.7.3 Description 2] ST_Measure().
    pub fn measure(&self) -> f64 {
        self.measure
    }

    /// [§15.7.3 Description 4] ST_Measure(DOUBLE PRECISION).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_measure(this: Option<&LrMeasure>, ameasure: Option<f64>) -> Result<Option<LrMeasure>> {
        let measure = required(ameasure, "§15.7.3 Description 4a", "ameasure")?;
        Ok(this.map(|m| LrMeasure { measure, ..m.clone() }))
    }

    /// [§15.7.4 Description 2] ST_UnitOfMeasure().
    pub fn unit_of_measure(&self) -> Option<&str> {
        self.units.as_deref()
    }

    /// [§15.7.4 Description 4] ST_UnitOfMeasure(CHARACTER VARYING).
    /// Reading: the unit is optional [§15.7.1 Description 3], so the null
    /// value is set without the check 4b i to iii makes of a unit.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF14 (Description 4b iii).
    pub fn with_unit_of_measure(this: Option<&LrMeasure>, aunitofmeasure: Option<String>, units: &[Unit<'_>]) -> Result<Option<LrMeasure>> {
        let Some(m) = this else { return Ok(None) };
        if let Some(unit) = &aunitofmeasure {
            supported(unit, units, "§15.7.4 Description 4b iii")?;
        }
        Ok(Some(LrMeasure { measure: m.measure, units: aunitofmeasure }))
    }
}

impl StartValue {
    /// [§15.8.2 Description 2] ST_StartValue(INTEGER, ST_LRMeasure),
    /// null-call.
    pub fn new(anlrmid: i32, ameasure: LrMeasure) -> StartValue {
        StartValue { lrm: anlrmid, measure: ameasure }
    }

    /// [§15.8.3 Description 2] ST_LRM().
    pub fn lrm(&self) -> i32 {
        self.lrm
    }

    /// [§15.8.3 Description 4] ST_LRM(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_lrm(this: Option<&StartValue>, anlrmid: Option<i32>) -> Result<Option<StartValue>> {
        let lrm = required(anlrmid, "§15.8.3 Description 4a", "anlrmid")?;
        Ok(this.map(|s| StartValue { lrm, ..s.clone() }))
    }

    /// [§15.8.4 Description 2] ST_Measure().
    pub fn measure(&self) -> &LrMeasure {
        &self.measure
    }

    /// [§15.8.4 Description 4] ST_Measure(ST_LRMeasure).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_measure(this: Option<&StartValue>, ameasure: Option<LrMeasure>) -> Result<Option<StartValue>> {
        let measure = required(ameasure, "§15.8.4 Description 4a", "ameasure")?;
        Ok(this.map(|s| StartValue { lrm: s.lrm, measure }))
    }
}
