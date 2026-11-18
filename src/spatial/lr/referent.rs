//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.10.2 to §15.10.7, §15.11.2
//! to §15.11.5, §15.12.2 to §15.12.5, §15.13.2, §15.13.3] The
//! constructors, observers and mutators of ST_Referent and of the three
//! offset expressions.
//!
//! The three short constructors of ST_Referent [§15.10.2 Descriptions 2,
//! 4 and 6] are NEW ST_Referent of four arguments with NULL for those
//! they leave out: `Referent::new`.

use super::locate::same_place;
use super::model::{LatOffsetExp, LrMeasure, PositionExp, Referent, Vector, VectorOffsetExp, VerOffsetExp};
use super::{Catalog, failed, required};
use crate::error::{Result, SqlState};
use crate::geometry::model::Geom;

/// [§15.13.1] The cardinality of ST_PrivateVectors, an `ST_Vector ARRAY[3]`.
pub const MAX_VECTORS: usize = 3;

impl Referent {
    /// [§15.10.2 Description 8] ST_Referent(CHARACTER VARYING, CHARACTER
    /// VARYING, ST_Point, ST_PositionExp). Whether a position and a
    /// location are the same place is found as `same_place` finds it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 8a); 2FF83 (potentially
    /// incompatible referent position and location) (Description 8b).
    pub fn new(
        areferentname: Option<String>,
        areferenttype: Option<String>,
        aposition: Option<Geom>,
        alocation: Option<PositionExp>,
        catalog: &Catalog<'_>,
    ) -> Result<Referent> {
        const RULE: &str = "§15.10.2 Description 8a";
        let name = required(areferentname, RULE, "areferentname")?;
        let referent_type = required(areferenttype, RULE, "areferenttype")?;
        compatible(aposition.as_ref(), alocation.as_ref(), catalog, "§15.10.2 Description 8b")?;
        Ok(Referent { name, referent_type, position: aposition, location: alocation.map(Box::new) })
    }

    /// [§15.10.3 Description 2] ST_ReferentName().
    pub fn referent_name(&self) -> &str {
        &self.name
    }

    /// [§15.10.3 Description 4] ST_ReferentName(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_referent_name(this: Option<&Referent>, areferentname: Option<String>) -> Result<Option<Referent>> {
        let name = required(areferentname, "§15.10.3 Description 4a", "areferentname")?;
        Ok(this.map(|r| Referent { name, ..r.clone() }))
    }

    /// [§15.10.4 Description 2] ST_ReferentType().
    pub fn referent_type(&self) -> &str {
        &self.referent_type
    }

    /// [§15.10.4 Description 4] ST_ReferentType(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_referent_type(this: Option<&Referent>, areferenttype: Option<String>) -> Result<Option<Referent>> {
        let referent_type = required(areferenttype, "§15.10.4 Description 4a", "areferenttype")?;
        Ok(this.map(|r| Referent { referent_type, ..r.clone() }))
    }

    /// [§15.10.5 Description 2] ST_Position().
    pub fn position(&self) -> Option<&Geom> {
        self.position.as_ref()
    }

    /// [§15.10.5 Description 4] ST_Position(ST_Point).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF83 (Description 4b i).
    pub fn with_position(this: Option<&Referent>, aposition: Option<Geom>, catalog: &Catalog<'_>) -> Result<Option<Referent>> {
        let Some(r) = this else { return Ok(None) };
        compatible(aposition.as_ref(), r.location.as_deref(), catalog, "§15.10.5 Description 4b i")?;
        Ok(Some(Referent { position: aposition, ..r.clone() }))
    }

    /// [§15.10.6 Description 2] ST_Location().
    pub fn location(&self) -> Option<&PositionExp> {
        self.location.as_deref()
    }

    /// [§15.10.6 Description 4] ST_Location(ST_PositionExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF83 (Description 4b i).
    pub fn with_location(this: Option<&Referent>, alocation: Option<PositionExp>, catalog: &Catalog<'_>) -> Result<Option<Referent>> {
        let Some(r) = this else { return Ok(None) };
        compatible(r.position.as_ref(), alocation.as_ref(), catalog, "§15.10.6 Description 4b i")?;
        Ok(Some(Referent { location: alocation.map(Box::new), ..r.clone() }))
    }

    /// [§15.10.7 Description 2] ST_ChangePosAndLoc(ST_Point,
    /// ST_PositionExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF83 (Description 2b i).
    pub fn change_pos_and_loc(
        this: Option<&Referent>,
        aposition: Option<Geom>,
        alocation: Option<PositionExp>,
        catalog: &Catalog<'_>,
    ) -> Result<Option<Referent>> {
        let Some(r) = this else { return Ok(None) };
        compatible(aposition.as_ref(), alocation.as_ref(), catalog, "§15.10.7 Description 2b i")?;
        Ok(Some(Referent {
            name: r.name.clone(),
            referent_type: r.referent_type.clone(),
            position: aposition,
            location: alocation.map(Box::new),
        }))
    }
}

/// [§15.10.2 Description 8b and its peers] Checks that a position and a
/// location, if both are given, are known to be the same place.
fn compatible(position: Option<&Geom>, location: Option<&PositionExp>, catalog: &Catalog<'_>, rule: &str) -> Result<()> {
    let (Some(point), Some(at)) = (position, location) else { return Ok(()) };
    same_place(point, at, catalog).map_err(|e| {
        failed(
            SqlState::SPATIAL_INCOMPATIBLE_REFERENT_POSITION,
            rule,
            &format!("potentially incompatible referent position and location: {}", e.msg),
        )
    })
}

/// [§15.11.4 Description 4a, §15.12.4 Description 4a] Checks a feature
/// geometry may be set: not one beside SELF's description. As 4a
/// precedes 4b, SELF the null value has no description to refuse it.
fn geometry_allowed(description: Option<&String>, given: Option<&Geom>, clause: &str) -> Result<()> {
    if given.is_some() && description.is_some() {
        return Err(failed(
            SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_DESCRIPTION,
            &format!("{clause}.4 Description 4a"),
            "illegal with offset referent description",
        ));
    }
    Ok(())
}

/// [§15.11.5 Description 4a, §15.12.5 Description 4a] Checks a
/// description may be set: not one beside SELF's feature geometry.
fn description_allowed(geometry: Option<&Geom>, given: Option<&String>, clause: &str) -> Result<()> {
    if given.is_some() && geometry.is_some() {
        return Err(failed(
            SqlState::SPATIAL_ILLEGAL_WITH_OFFSET_REFERENT_GEOMETRY,
            &format!("{clause}.5 Description 4a"),
            "illegal with offset referent geometry",
        ));
    }
    Ok(())
}

impl LatOffsetExp {
    /// [§15.11.2 Description 2] ST_LatOffsetExp(ST_LRMeasure), null-call.
    pub fn new(anoffsetlateraldistance: LrMeasure) -> LatOffsetExp {
        LatOffsetExp { distance: Some(anoffsetlateraldistance), geometry: None, description: None }
    }

    /// [§15.11.2 Description 4] ST_LatOffsetExp(ST_LRMeasure,
    /// ST_Geometry), null-call.
    pub fn with_geometry(anoffsetlateraldistance: LrMeasure, afeaturegeometry: Geom) -> LatOffsetExp {
        LatOffsetExp { distance: Some(anoffsetlateraldistance), geometry: Some(afeaturegeometry), description: None }
    }

    /// [§15.11.2 Description 6] ST_LatOffsetExp(ST_LRMeasure, CHARACTER
    /// VARYING), null-call.
    pub fn with_description(anoffsetlateraldistance: LrMeasure, anoffsetreferentdescription: String) -> LatOffsetExp {
        LatOffsetExp { distance: Some(anoffsetlateraldistance), geometry: None, description: Some(anoffsetreferentdescription) }
    }

    /// [§15.11.3 Description 2] ST_OffsetLatDist().
    pub fn offset_lat_dist(&self) -> Option<&LrMeasure> {
        self.distance.as_ref()
    }

    /// [§15.11.3 Description 4] ST_OffsetLatDist(ST_LRMeasure).
    #[must_use]
    pub fn with_offset_lat_dist(this: Option<&LatOffsetExp>, anoffsetlateraldistance: Option<LrMeasure>) -> Option<LatOffsetExp> {
        this.map(|o| LatOffsetExp { distance: anoffsetlateraldistance, ..o.clone() })
    }

    /// [§15.11.4 Description 2] ST_FeatureGeometry().
    pub fn feature_geometry(&self) -> Option<&Geom> {
        self.geometry.as_ref()
    }

    /// [§15.11.4 Description 4] ST_FeatureGeometry(ST_Geometry). As 4a
    /// precedes 4b, SELF the null value has no description to refuse.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF94 (illegal with offset referent description) (Description 4a).
    pub fn with_feature_geometry(this: Option<&LatOffsetExp>, afeaturegeometry: Option<Geom>) -> Result<Option<LatOffsetExp>> {
        geometry_allowed(this.and_then(|o| o.description.as_ref()), afeaturegeometry.as_ref(), "§15.11")?;
        Ok(this.map(|o| LatOffsetExp { geometry: afeaturegeometry, ..o.clone() }))
    }

    /// [§15.11.5 Description 2] ST_OffsetRefDesc().
    pub fn offset_ref_desc(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// [§15.11.5 Description 4] ST_OffsetRefDesc(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF95 (illegal with offset referent geometry) (Description 4a).
    pub fn with_offset_ref_desc(this: Option<&LatOffsetExp>, anoffsetreferentdescription: Option<String>) -> Result<Option<LatOffsetExp>> {
        description_allowed(this.and_then(|o| o.geometry.as_ref()), anoffsetreferentdescription.as_ref(), "§15.11")?;
        Ok(this.map(|o| LatOffsetExp { description: anoffsetreferentdescription, ..o.clone() }))
    }
}

impl VerOffsetExp {
    /// [§15.12.2 Description 2] ST_VerOffsetExp(ST_LRMeasure), null-call.
    pub fn new(anoffsetverticaldistance: LrMeasure) -> VerOffsetExp {
        VerOffsetExp { distance: Some(anoffsetverticaldistance), geometry: None, description: None }
    }

    /// [§15.12.2 Description 4] ST_VerOffsetExp(ST_LRMeasure,
    /// ST_Geometry), null-call.
    pub fn with_geometry(anoffsetverticaldistance: LrMeasure, afeaturegeometry: Geom) -> VerOffsetExp {
        VerOffsetExp { distance: Some(anoffsetverticaldistance), geometry: Some(afeaturegeometry), description: None }
    }

    /// [§15.12.2 Description 6] ST_VerOffsetExp(ST_LRMeasure, CHARACTER
    /// VARYING), null-call.
    pub fn with_description(anoffsetverticaldistance: LrMeasure, anoffsetreferentdescription: String) -> VerOffsetExp {
        VerOffsetExp { distance: Some(anoffsetverticaldistance), geometry: None, description: Some(anoffsetreferentdescription) }
    }

    /// [§15.12.3 Description 2] ST_OffsetVerDist().
    pub fn offset_ver_dist(&self) -> Option<&LrMeasure> {
        self.distance.as_ref()
    }

    /// [§15.12.3 Description 4] ST_OffsetVerDist(ST_LRMeasure).
    #[must_use]
    pub fn with_offset_ver_dist(this: Option<&VerOffsetExp>, anoffsetverticaldistance: Option<LrMeasure>) -> Option<VerOffsetExp> {
        this.map(|o| VerOffsetExp { distance: anoffsetverticaldistance, ..o.clone() })
    }

    /// [§15.12.4 Description 2] ST_FeatureGeometry().
    pub fn feature_geometry(&self) -> Option<&Geom> {
        self.geometry.as_ref()
    }

    /// [§15.12.4 Description 4] ST_FeatureGeometry(ST_Geometry).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF94 (Description 4a).
    pub fn with_feature_geometry(this: Option<&VerOffsetExp>, afeaturegeometry: Option<Geom>) -> Result<Option<VerOffsetExp>> {
        geometry_allowed(this.and_then(|o| o.description.as_ref()), afeaturegeometry.as_ref(), "§15.12")?;
        Ok(this.map(|o| VerOffsetExp { geometry: afeaturegeometry, ..o.clone() }))
    }

    /// [§15.12.5 Description 2] ST_OffsetRefDesc().
    pub fn offset_ref_desc(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// [§15.12.5 Description 4] ST_OffsetRefDesc(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF95 (Description 4a).
    pub fn with_offset_ref_desc(this: Option<&VerOffsetExp>, anoffsetreferentdescription: Option<String>) -> Result<Option<VerOffsetExp>> {
        description_allowed(this.and_then(|o| o.geometry.as_ref()), anoffsetreferentdescription.as_ref(), "§15.12")?;
        Ok(this.map(|o| VerOffsetExp { description: anoffsetreferentdescription, ..o.clone() }))
    }
}

impl VectorOffsetExp {
    /// [§15.13.2 Description 2] ST_VectorOffsetExp(ST_Vector ARRAY),
    /// null-call.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2202F (array data, right truncation) of more vectors than
    /// the `ARRAY[3]` of §15.13.1 holds.
    pub fn new(avectorarray: Vec<Vector>) -> Result<VectorOffsetExp> {
        Ok(VectorOffsetExp { vectors: at_most_three(avectorarray, "§15.13.1 Definition")? })
    }

    /// [§15.13.3 Description 2] ST_Vectors().
    pub fn vectors(&self) -> &[Vector] {
        &self.vectors
    }

    /// [§15.13.3 Description 4] ST_Vectors(ST_Vector ARRAY).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a); 2202F as `new`.
    pub fn with_vectors(this: Option<&VectorOffsetExp>, avectorarray: Option<Vec<Vector>>) -> Result<Option<VectorOffsetExp>> {
        let vectors = at_most_three(required(avectorarray, "§15.13.3 Description 4a", "avectorarray")?, "§15.13.1 Definition")?;
        Ok(this.map(|_| VectorOffsetExp { vectors }))
    }
}

/// `vectors`, if the `ARRAY[3]` of ST_PrivateVectors holds them.
fn at_most_three(vectors: Vec<Vector>, rule: &str) -> Result<Vec<Vector>> {
    if vectors.len() > MAX_VECTORS {
        return Err(failed(
            SqlState::ARRAY_DATA_RIGHT_TRUNCATION,
            rule,
            &format!("ST_PrivateVectors, an ARRAY[3], holds at most three vectors, not {}", vectors.len()),
        ));
    }
    Ok(vectors)
}
