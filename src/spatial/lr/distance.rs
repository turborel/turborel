//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.9.2 to §15.9.10] The
//! constructors, observers and mutators of ST_DistanceExp.
//!
//! The sixteen constructors of §15.9.2 are null-call forms of two, the
//! seven-argument one of Description 30 and the six-argument one with a
//! vector offset of Description 32, called with NULL for what they leave
//! out: `DistanceExp::new` and `DistanceExp::new_vector`, with `along` for
//! the one-argument form. Descriptions 4 to 28 write "NEW ST_LRMeasure"
//! where they mean NEW ST_DistanceExp, the type they construct, and are
//! read so. Description 32 does not repeat the check of Description 30b,
//! and none is made of it, as the text gives.

use super::model::{DistanceExp, LatOffsetExp, LrMeasure, VectorOffsetExp, VerOffsetExp};
use super::{failed, required};
use crate::error::{Result, SqlState};

/// The "from" and "towards" referents a distance expression names, each
/// the null value or not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReferentNames {
    /// afromreferentfeatureID.
    pub from_feature: Option<String>,
    /// afromreferentname.
    pub from_name: Option<String>,
    /// atowardsreferentfeatureID.
    pub towards_feature: Option<String>,
    /// atowardsreferentname.
    pub towards_name: Option<String>,
}

impl DistanceExp {
    /// [§15.9.2 Description 4] ST_DistanceExp(ST_LRMeasure), null-call.
    pub fn along(adistancealong: LrMeasure) -> DistanceExp {
        DistanceExp {
            along: adistancealong,
            from_feature: None,
            from_name: None,
            towards_feature: None,
            towards_name: None,
            lateral: None,
            vertical: None,
            vector: None,
        }
    }

    /// [§15.9.2 Description 30] ST_DistanceExp(ST_LRMeasure, CHARACTER
    /// VARYING, CHARACTER VARYING, CHARACTER VARYING, CHARACTER VARYING,
    /// ST_LatOffsetExp, ST_VerOffsetExp), and the null-call forms of
    /// Descriptions 6 to 10, 14 to 20 and 24 to 28 through it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 30a); 2FF90 (towards referent
    /// requires a from referent) (Description 30b).
    pub fn new(
        adistancealong: Option<LrMeasure>,
        referents: ReferentNames,
        alateraloffsetexpression: Option<LatOffsetExp>,
        averticaloffsetexpression: Option<VerOffsetExp>,
    ) -> Result<DistanceExp> {
        let along = required(adistancealong, "§15.9.2 Description 30a", "adistancealong")?;
        if referents.towards_name.is_some() && referents.from_name.is_none() {
            return Err(towards_without_from("§15.9.2 Description 30b"));
        }
        Ok(DistanceExp { lateral: alateraloffsetexpression, vertical: averticaloffsetexpression, ..of_referents(along, referents) })
    }

    /// [§15.9.2 Description 32] ST_DistanceExp(ST_LRMeasure, CHARACTER
    /// VARYING, CHARACTER VARYING, CHARACTER VARYING, CHARACTER VARYING,
    /// ST_VectorOffsetExp), and the null-call forms of Descriptions 12
    /// and 22 through it.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 32a).
    pub fn new_vector(
        adistancealong: Option<LrMeasure>,
        referents: ReferentNames,
        avectoroffsetexpression: Option<VectorOffsetExp>,
    ) -> Result<DistanceExp> {
        let along = required(adistancealong, "§15.9.2 Description 32a", "adistancealong")?;
        Ok(DistanceExp { vector: avectoroffsetexpression, ..of_referents(along, referents) })
    }

    /// [§15.9.3 Description 2] ST_DistanceAlong().
    pub fn distance_along(&self) -> &LrMeasure {
        &self.along
    }

    /// [§15.9.3 Description 4] ST_DistanceAlong(ST_LRMeasure).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_distance_along(this: Option<&DistanceExp>, adistancealong: Option<LrMeasure>) -> Result<Option<DistanceExp>> {
        let along = required(adistancealong, "§15.9.3 Description 4a", "adistancealong")?;
        Ok(this.map(|d| DistanceExp { along, ..d.clone() }))
    }

    /// [§15.9.4 Description 2] ST_FromRefFeaID().
    pub fn from_ref_fea_id(&self) -> Option<&str> {
        self.from_feature.as_deref()
    }

    /// [§15.9.4 Description 4] ST_FromRefFeaID(CHARACTER VARYING).
    #[must_use]
    pub fn with_from_ref_fea_id(this: Option<&DistanceExp>, afromreferentfeature_id: Option<String>) -> Option<DistanceExp> {
        this.map(|d| DistanceExp { from_feature: afromreferentfeature_id, ..d.clone() })
    }

    /// [§15.9.5 Description 2] ST_FromRefName().
    pub fn from_ref_name(&self) -> Option<&str> {
        self.from_name.as_deref()
    }

    /// [§15.9.5 Description 4] ST_FromRefName(CHARACTER VARYING). The text
    /// makes no check here that a towards referent keeps its from
    /// referent, and none is made.
    #[must_use]
    pub fn with_from_ref_name(this: Option<&DistanceExp>, afromreferentname: Option<String>) -> Option<DistanceExp> {
        this.map(|d| DistanceExp { from_name: afromreferentname, ..d.clone() })
    }

    /// [§15.9.6 Description 2] ST_TowardsRefFeaID().
    pub fn towards_ref_fea_id(&self) -> Option<&str> {
        self.towards_feature.as_deref()
    }

    /// [§15.9.6 Description 4] ST_TowardsRefFeaID(CHARACTER VARYING).
    #[must_use]
    pub fn with_towards_ref_fea_id(this: Option<&DistanceExp>, atowardsreferentfeature_id: Option<String>) -> Option<DistanceExp> {
        this.map(|d| DistanceExp { towards_feature: atowardsreferentfeature_id, ..d.clone() })
    }

    /// [§15.9.7 Description 2] ST_TowardsRefName().
    pub fn towards_ref_name(&self) -> Option<&str> {
        self.towards_name.as_deref()
    }

    /// [§15.9.7 Description 4] ST_TowardsRefName(CHARACTER VARYING). As 4a
    /// precedes 4b, SELF the null value has no from referent, and a towards
    /// referent given to it raises the exception.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF90 (Description 4a).
    pub fn with_towards_ref_name(this: Option<&DistanceExp>, atowardsreferentname: Option<String>) -> Result<Option<DistanceExp>> {
        let has_from = this.is_some_and(|d| d.from_name.is_some());
        if atowardsreferentname.is_some() && !has_from {
            return Err(towards_without_from("§15.9.7 Description 4a"));
        }
        Ok(this.map(|d| DistanceExp { towards_name: atowardsreferentname, ..d.clone() }))
    }

    /// [§15.9.8 Description 2] ST_LatOffsetExp().
    pub fn lat_offset_exp(&self) -> Option<&LatOffsetExp> {
        self.lateral.as_ref()
    }

    /// [§15.9.8 Description 4] ST_LatOffsetExp(ST_LatOffsetExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF91 (illegal with vector offset) (Description 4a).
    pub fn with_lat_offset_exp(this: Option<&DistanceExp>, alateraloffsetexpression: Option<LatOffsetExp>) -> Result<Option<DistanceExp>> {
        if this.is_some_and(|d| d.vector.is_some()) {
            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET, "§15.9.8 Description 4a", "illegal with vector offset"));
        }
        Ok(this.map(|d| DistanceExp { lateral: alateraloffsetexpression, ..d.clone() }))
    }

    /// [§15.9.9 Description 2] ST_VerOffsetExp().
    pub fn ver_offset_exp(&self) -> Option<&VerOffsetExp> {
        self.vertical.as_ref()
    }

    /// [§15.9.9 Description 4] ST_VerOffsetExp(ST_VerOffsetExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF91 (Description 4a).
    pub fn with_ver_offset_exp(this: Option<&DistanceExp>, averticaloffsetexpression: Option<VerOffsetExp>) -> Result<Option<DistanceExp>> {
        if this.is_some_and(|d| d.vector.is_some()) {
            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_VECTOR_OFFSET, "§15.9.9 Description 4a", "illegal with vector offset"));
        }
        Ok(this.map(|d| DistanceExp { vertical: averticaloffsetexpression, ..d.clone() }))
    }

    /// [§15.9.10 Description 2] ST_VectorOffsetExp().
    pub fn vector_offset_exp(&self) -> Option<&VectorOffsetExp> {
        self.vector.as_ref()
    }

    /// [§15.9.10 Description 4] ST_VectorOffsetExp(ST_VectorOffsetExp).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF92 (illegal with lateral offset) (Description 4a); 2FF93 (illegal
    /// with vertical offset) (Description 4b).
    pub fn with_vector_offset_exp(
        this: Option<&DistanceExp>,
        avectoroffsetexpression: Option<VectorOffsetExp>,
    ) -> Result<Option<DistanceExp>> {
        if this.is_some_and(|d| d.lateral.is_some()) {
            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_LATERAL_OFFSET, "§15.9.10 Description 4a", "illegal with lateral offset"));
        }
        if this.is_some_and(|d| d.vertical.is_some()) {
            return Err(failed(SqlState::SPATIAL_ILLEGAL_WITH_VERTICAL_OFFSET, "§15.9.10 Description 4b", "illegal with vertical offset"));
        }
        Ok(this.map(|d| DistanceExp { vector: avectoroffsetexpression, ..d.clone() }))
    }

    /// [§15.9.1 Description 20, 21] Whether it has a "from" referent.
    pub fn has_referent(&self) -> bool {
        self.from_name.is_some()
    }

    /// [§15.9.1 Description 25] Whether it has an offset expression.
    pub fn has_offset(&self) -> bool {
        self.lateral.is_some() || self.vertical.is_some() || self.vector.is_some()
    }
}

/// A distance expression of `along` and `referents` alone.
fn of_referents(along: LrMeasure, referents: ReferentNames) -> DistanceExp {
    DistanceExp {
        from_feature: referents.from_feature,
        from_name: referents.from_name,
        towards_feature: referents.towards_feature,
        towards_name: referents.towards_name,
        ..DistanceExp::along(along)
    }
}

/// SQLSTATE 2FF90, cited as `rule`.
fn towards_without_from(rule: &str) -> crate::error::SqlError {
    failed(SqlState::SPATIAL_TOWARDS_REFERENT_REQUIRES_FROM, rule, "towards referent requires a from referent")
}
