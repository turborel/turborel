//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.2.2 to §15.2.6, §15.3.2 to
//! §15.3.4, §15.4.2, §15.4.3, §15.5.2 to §15.5.5] The constructors,
//! observers and mutators of ST_LinearElement and its subtypes.
//! ST_TranslateToInst and ST_TranslateToType are in `lr/translate.rs`,
//! ST_Point and ST_LRPosition in `lr/locate.rs`.

use super::model::{ElementBase, LinearElement, LrCurve, LrDirectedEdge, LrFeature, LrMeasure, Lrm, Referent, StartValue};
use super::{failed, required};
use crate::error::{Result, SqlState};
use crate::geometry::model::{Geom, Kind};

/// The ST_LinearElement arguments a subtype's constructor takes first,
/// each the null value or not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementParts {
    /// anleid.
    pub id: Option<i32>,
    /// anlrmid, the default LRM.
    pub default_lrm: Option<i32>,
    /// ameasure, the default measure.
    pub default_measure: Option<LrMeasure>,
    /// alinearelementtype.
    pub element_type: Option<String>,
    /// astartvaluearray.
    pub start_values: Option<Vec<StartValue>>,
}

/// The arguments ST_LRDirectedEdge's constructor takes after the
/// ST_LinearElement ones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeParts {
    /// atopologytype.
    pub topology_type: Option<char>,
    /// atopologyornetworkname.
    pub topology_name: Option<String>,
    /// anedgeorlinkID.
    pub edge_id: Option<i32>,
}

impl ElementParts {
    /// The attributes the parts give, once anleid, anlrmid and ameasure,
    /// which every subtype's constructor requires, are not null.
    fn checked(self, rule: &str) -> Result<ElementBase> {
        Ok(ElementBase {
            id: required(self.id, rule, "anleid")?,
            default_lrm: required(self.default_lrm, rule, "anlrmid")?,
            default_measure: required(self.default_measure, rule, "ameasure")?,
            element_type: self.element_type,
            start_values: self.start_values,
        })
    }
}

impl LrFeature {
    /// [§15.3.2 Description 4] ST_LRFeature(INTEGER, INTEGER,
    /// ST_LRMeasure, CHARACTER VARYING, ST_StartValue ARRAY, CHARACTER
    /// VARYING). Its referents keep their default, ARRAY[] [§15.3.1].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 4a).
    pub fn new(parts: ElementParts, afeature_id: Option<String>) -> Result<LrFeature> {
        feature(parts, afeature_id, Some(Vec::new()), "§15.3.2 Description 4a")
    }

    /// [§15.3.2 Description 6] ST_LRFeature of seven arguments, with the
    /// referents.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 6a).
    pub fn new_with_referents(
        parts: ElementParts,
        afeature_id: Option<String>,
        areferentarray: Option<Vec<Referent>>,
    ) -> Result<LrFeature> {
        feature(parts, afeature_id, areferentarray, "§15.3.2 Description 6a")
    }

    /// [§15.3.3 Description 2] ST_FeatureID().
    pub fn feature_id(&self) -> &str {
        &self.feature_id
    }

    /// [§15.3.3 Description 4] ST_FeatureID(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_feature_id(this: Option<&LrFeature>, afeature_id: Option<String>) -> Result<Option<LrFeature>> {
        let feature_id = required(afeature_id, "§15.3.3 Description 4a", "afeatureID")?;
        Ok(this.map(|f| LrFeature { feature_id, ..f.clone() }))
    }

    /// [§15.3.4 Description 2] ST_Referents().
    pub fn referents(&self) -> Option<&[Referent]> {
        self.referents.as_deref()
    }

    /// [§15.3.4 Description 4] ST_Referents(ST_Referent ARRAY).
    #[must_use]
    pub fn with_referents(this: Option<&LrFeature>, areferentarray: Option<Vec<Referent>>) -> Option<LrFeature> {
        this.map(|f| LrFeature { referents: areferentarray, ..f.clone() })
    }
}

impl LrCurve {
    /// [§15.4.2 Description 4] ST_LRCurve(INTEGER, INTEGER, ST_LRMeasure,
    /// CHARACTER VARYING, ST_StartValue ARRAY, ST_Curve).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a); 2FF08 (element is not a valid type) of a
    /// geometry that is no ST_Curve, which the SQL layer's typing
    /// otherwise prevents.
    pub fn new(parts: ElementParts, acurve: Option<Geom>) -> Result<LrCurve> {
        const RULE: &str = "§15.4.2 Description 4a";
        let base = parts.checked(RULE)?;
        let curve = curve_value(required(acurve, RULE, "acurve")?)?;
        Ok(LrCurve { base, curve })
    }

    /// [§15.4.3 Description 2] ST_Curve().
    pub fn curve(&self) -> &Geom {
        &self.curve
    }

    /// [§15.4.3 Description 4] ST_Curve(ST_Curve).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a); 2FF08 as `new`.
    pub fn with_curve(this: Option<&LrCurve>, acurve: Option<Geom>) -> Result<Option<LrCurve>> {
        let curve = curve_value(required(acurve, "§15.4.3 Description 4a", "acurve")?)?;
        Ok(this.map(|c| LrCurve { curve, ..c.clone() }))
    }
}

/// [§15.4.1] `g`, if it is a value of ST_Curve, the type of
/// ST_PrivateCurve.
pub(crate) fn curve_value(g: Geom) -> Result<Geom> {
    if g.kind().is_subtype_of(Kind::Curve) {
        return Ok(g);
    }
    Err(failed(
        SqlState::SPATIAL_ELEMENT_IS_NOT_A_VALID_TYPE,
        "§15.4.1",
        &format!("the curve of an ST_LRCurve is not an ST_Curve but an {}", g.kind().sql_name()),
    ))
}

impl LrDirectedEdge {
    /// [§15.5.2 Description 4] ST_LRDirectedEdge(INTEGER, INTEGER,
    /// ST_LRMeasure, CHARACTER VARYING, ST_StartValue ARRAY, CHARACTER,
    /// CHARACTER VARYING, INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn new(parts: ElementParts, edge: EdgeParts) -> Result<LrDirectedEdge> {
        const RULE: &str = "§15.5.2 Description 4a";
        let base = parts.checked(RULE)?;
        Ok(LrDirectedEdge {
            base,
            topology_type: required(edge.topology_type, RULE, "atopologytype")?,
            topology_name: required(edge.topology_name, RULE, "atopologyornetworkname")?,
            edge_id: required(edge.edge_id, RULE, "anedgeorlinkID")?,
        })
    }

    /// [§15.5.3 Description 2] ST_TopologyType().
    pub fn topology_type(&self) -> char {
        self.topology_type
    }

    /// [§15.5.3 Description 4] ST_TopologyType(CHARACTER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_topology_type(this: Option<&LrDirectedEdge>, atopologytype: Option<char>) -> Result<Option<LrDirectedEdge>> {
        let topology_type = required(atopologytype, "§15.5.3 Description 4a", "atopologytype")?;
        Ok(this.map(|e| LrDirectedEdge { topology_type, ..e.clone() }))
    }

    /// [§15.5.4 Description 2] ST_TopoOrNetName().
    pub fn topo_or_net_name(&self) -> &str {
        &self.topology_name
    }

    /// [§15.5.4 Description 4] ST_TopoOrNetName(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_topo_or_net_name(this: Option<&LrDirectedEdge>, atopologyornetworkname: Option<String>) -> Result<Option<LrDirectedEdge>> {
        let topology_name = required(atopologyornetworkname, "§15.5.4 Description 4a", "atopologyornetworkname")?;
        Ok(this.map(|e| LrDirectedEdge { topology_name, ..e.clone() }))
    }

    /// [§15.5.5 Description 2] ST_EdgeOrLinkID().
    pub fn edge_or_link_id(&self) -> i32 {
        self.edge_id
    }

    /// [§15.5.5 Description 4] ST_EdgeOrLinkID(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_edge_or_link_id(this: Option<&LrDirectedEdge>, anedgeorlink_id: Option<i32>) -> Result<Option<LrDirectedEdge>> {
        let edge_id = required(anedgeorlink_id, "§15.5.5 Description 4a", "anedgeorlinkID")?;
        Ok(this.map(|e| LrDirectedEdge { edge_id, ..e.clone() }))
    }
}

impl LinearElement {
    /// [§15.2.2 Description 2] ST_LinearElementID().
    pub fn linear_element_id(&self) -> i32 {
        self.base().id
    }

    /// [§15.2.2 Description 4] ST_LinearElementID(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_linear_element_id(this: Option<&LinearElement>, anleid: Option<i32>) -> Result<Option<LinearElement>> {
        let id = required(anleid, "§15.2.2 Description 4a", "anleid")?;
        Ok(this.map(|e| changed(e, |b| b.id = id)))
    }

    /// [§15.2.3 Description 2] ST_DefaultLRM().
    pub fn default_lrm(&self) -> i32 {
        self.base().default_lrm
    }

    /// [§15.2.3 Description 4] ST_DefaultLRM(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_default_lrm(this: Option<&LinearElement>, anlrmid: Option<i32>) -> Result<Option<LinearElement>> {
        let lrm = required(anlrmid, "§15.2.3 Description 4a", "anlrmid")?;
        Ok(this.map(|e| changed(e, |b| b.default_lrm = lrm)))
    }

    /// [§15.2.4 Description 2] ST_DefaultMeasure().
    pub fn default_measure(&self) -> &LrMeasure {
        &self.base().default_measure
    }

    /// [§15.2.4 Description 4] ST_DefaultMeasure(ST_LRMeasure): the
    /// changed value with the completion condition 4c i raises, SQLSTATE
    /// 01F82, for the SQL layer to raise as a warning.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_default_measure(this: Option<&LinearElement>, ameasure: Option<LrMeasure>) -> Result<Option<(LinearElement, SqlState)>> {
        let measure = required(ameasure, "§15.2.4 Description 4a", "ameasure")?;
        Ok(this.map(|e| (changed(e, |b| b.default_measure = measure), SqlState::SPATIAL_CHANGING_DEFAULT_MEASURE)))
    }

    /// [§15.2.5 Description 2] ST_LEType().
    pub fn le_type(&self) -> Option<&str> {
        self.base().element_type.as_deref()
    }

    /// [§15.2.5 Description 4] ST_LEType(CHARACTER VARYING).
    #[must_use]
    pub fn with_le_type(this: Option<&LinearElement>, alinearelementtype: Option<String>) -> Option<LinearElement> {
        this.map(|e| changed(e, |b| b.element_type = alinearelementtype))
    }

    /// [§15.2.6 Description 2] ST_StartValue(INTEGER): the measure at its
    /// start of the LRM `anlrmid`, one of `lrms`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 2a); 2FF81 (invalid LRM) (Description 2b).
    pub fn start_value(&self, anlrmid: Option<i32>, lrms: &[Lrm]) -> Result<LrMeasure> {
        let id = required(anlrmid, "§15.2.6 Description 2a", "anlrmid")?;
        let Some(lrm) = lrms.iter().find(|l| l.id == id) else {
            return Err(failed(
                SqlState::SPATIAL_INVALID_LRM,
                "§15.2.6 Description 2b",
                &format!("invalid LRM: no LRM has the lrmid {id}"),
            ));
        };
        let found = self.base().start_values.iter().flatten().find(|s| s.lrm == id);
        Ok(found.map_or_else(|| LrMeasure { measure: 0.0_f64, units: Some(lrm.units.clone()) }, |s| s.measure.clone()))
    }

    /// [§15.2.6 Description 4] ST_StartValue(INTEGER, ST_LRMeasure): the
    /// start value of `anlrmid` set to `ameasure`, or one added. SELF the
    /// null value, the method being of SELF, gives the null value.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a, 4b).
    pub fn with_start_value(
        this: Option<&LinearElement>,
        anlrmid: Option<i32>,
        ameasure: Option<LrMeasure>,
    ) -> Result<Option<LinearElement>> {
        let id = required(anlrmid, "§15.2.6 Description 4a", "anlrmid")?;
        let measure = required(ameasure, "§15.2.6 Description 4b", "ameasure")?;
        Ok(this.map(|e| {
            changed(e, |b| {
                let values = b.start_values.get_or_insert_with(Vec::new);
                match values.iter_mut().find(|s| s.lrm == id) {
                    Some(found) => found.measure = measure,
                    None => values.push(StartValue { lrm: id, measure }),
                }
            })
        }))
    }
}

/// `e` with its ST_LinearElement attributes changed by `change`.
fn changed(e: &LinearElement, change: impl FnOnce(&mut ElementBase)) -> LinearElement {
    let mut copy = e.clone();
    change(copy.base_mut());
    copy
}

/// [§15.3.2 Description 4c, 6c] The feature of `parts`, `afeature_id`
/// and `referents`, once its null checks, cited as `rule`, pass.
fn feature(parts: ElementParts, afeature_id: Option<String>, referents: Option<Vec<Referent>>, rule: &str) -> Result<LrFeature> {
    let base = parts.checked(rule)?;
    let feature_id = required(afeature_id, rule, "afeatureID")?;
    Ok(LrFeature { base, feature_id, referents })
}
