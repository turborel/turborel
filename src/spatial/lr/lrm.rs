//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §15.1.2 to §15.1.10] The
//! constructors, observers and mutators of ST_LRM.
//!
//! Two defects of the text are read as their neighbours show they are
//! meant. §15.1.2 Description 6g viii 2A tests apositivelateraloffsetdirection
//! where it sets the vertical direction: it is read as testing
//! apositiveverticaloffsetdirection, as vii tests the lateral one for the
//! lateral direction. §15.1.10 Description 4c sets
//! ST_PrivatePositiveLateralOffsetDirection to 'up': it is read as setting
//! ST_PrivatePositiveVerticalOffsetDirection, the attribute §15.1.10
//! mutates.

use super::model::Lrm;
use super::{Unit, failed, required, supported};
use crate::error::{Result, SqlState};

// ---------------------------------------------------------------------------
// [§15.1.1] Key words of the attribute values
// ---------------------------------------------------------------------------

/// [§15.1.1 Description 13] The LRM type measuring from the start.
pub const ABSOLUTE: &str = "absolute";
/// [§15.1.1 Description 13] The LRM type measuring from referents.
pub const RELATIVE: &str = "relative";
/// [§15.1.1 Description 13] The LRM type pro-rating the default measure.
pub const INTERPOLATIVE: &str = "interpolative";
/// [§15.1.1 Description 13] The LRM type interpolating m coordinates.
pub const LOCAL_INTERPOLATIVE: &str = "local interpolative";
/// [§15.1.1 Description 10, 14] The default positive lateral direction.
pub const RIGHT: &str = "right";
/// [§15.1.1 Description 14] The other positive lateral direction.
pub const LEFT: &str = "left";
/// [§15.1.1 Description 10, 15] The default positive vertical direction.
pub const UP: &str = "up";
/// [§15.1.1 Description 15] The other positive vertical direction.
pub const DOWN: &str = "down";

/// The arguments of ST_LRM's five-argument constructor, each the null
/// value or not.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LrmParts {
    /// anlrmid.
    pub id: Option<i32>,
    /// anlrmname.
    pub name: Option<String>,
    /// anlrmtype.
    pub lrm_type: Option<String>,
    /// aunitofmeasure.
    pub units: Option<String>,
    /// aconstraintarray, a null element kept.
    pub constraints: Option<Vec<Option<String>>>,
}

/// The three further arguments of ST_LRM's eight-argument constructor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OffsetParts {
    /// anoffsetunitofmeasure.
    pub units: Option<String>,
    /// apositivelateraloffsetdirection.
    pub lateral: Option<String>,
    /// apositiveverticaloffsetdirection.
    pub vertical: Option<String>,
}

impl LrmParts {
    /// [§15.1.2 Description 4a to e, 6a to e] The LRM the parts give, its
    /// offset attributes null, once none of the four required is null and
    /// the unit is supported. `at` names the Description.
    fn checked(self, units: &[Unit<'_>], at: &str) -> Result<Lrm> {
        let null_rule = format!("§15.1.2 Description {at}a");
        let id = required(self.id, &null_rule, "anlrmid")?;
        let name = required(self.name, &null_rule, "anlrmname")?;
        let lrm_type = required(self.lrm_type, &null_rule, "anlrmtype")?;
        let unit = required(self.units, &null_rule, "aunitofmeasure")?;
        supported(&unit, units, &format!("§15.1.2 Description {at}e"))?;
        Ok(Lrm {
            id,
            name,
            lrm_type,
            units: unit,
            constraints: self.constraints,
            offset_units: None,
            lateral_direction: None,
            vertical_direction: None,
        })
    }
}

impl Lrm {
    /// [§15.1.2 Description 4] ST_LRM(INTEGER, CHARACTER VARYING,
    /// CHARACTER VARYING, CHARACTER VARYING, CHARACTER VARYING ARRAY). SELF
    /// is never the null value of a constructor, so 4b does not apply.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (null argument) (Description 4a); 2FF14 (unsupported unit
    /// specified) (Description 4e).
    pub fn new(parts: LrmParts, units: &[Unit<'_>]) -> Result<Lrm> {
        parts.checked(units, "4")
    }

    /// [§15.1.2 Description 6] ST_LRM of eight arguments: with the offset
    /// unit and the positive lateral and vertical offset directions,
    /// 'right' and 'up' by default of an LRM allowing offsets [6g vii,
    /// viii, read as the module doc says].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 6a); 2FF14 (Description 6e, 6f iii).
    pub fn new_with_offsets(parts: LrmParts, offsets: OffsetParts, units: &[Unit<'_>]) -> Result<Lrm> {
        let mut lrm = parts.checked(units, "6")?;
        if let Some(unit) = &offsets.units {
            supported(unit, units, "§15.1.2 Description 6f iii")?;
            lrm.lateral_direction = Some(offsets.lateral.unwrap_or_else(|| RIGHT.to_owned()));
            lrm.vertical_direction = Some(offsets.vertical.unwrap_or_else(|| UP.to_owned()));
        }
        lrm.offset_units = offsets.units;
        Ok(lrm)
    }

    /// [§15.1.3 Description 2] ST_LRMID().
    pub fn lrm_id(&self) -> i32 {
        self.id
    }

    /// [§15.1.3 Description 4] ST_LRMID(INTEGER).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_lrm_id(this: Option<&Lrm>, anlrmid: Option<i32>) -> Result<Option<Lrm>> {
        let id = required(anlrmid, "§15.1.3 Description 4a", "anlrmid")?;
        Ok(this.map(|l| Lrm { id, ..l.clone() }))
    }

    /// [§15.1.4 Description 2] ST_LRMName().
    pub fn lrm_name(&self) -> &str {
        &self.name
    }

    /// [§15.1.4 Description 4] ST_LRMName(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_lrm_name(this: Option<&Lrm>, anlrmname: Option<String>) -> Result<Option<Lrm>> {
        let name = required(anlrmname, "§15.1.4 Description 4a", "anlrmname")?;
        Ok(this.map(|l| Lrm { name, ..l.clone() }))
    }

    /// [§15.1.5 Description 2] ST_LRMType().
    pub fn lrm_type(&self) -> &str {
        &self.lrm_type
    }

    /// [§15.1.5 Description 4] ST_LRMType(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a).
    pub fn with_lrm_type(this: Option<&Lrm>, anlrmtype: Option<String>) -> Result<Option<Lrm>> {
        let lrm_type = required(anlrmtype, "§15.1.5 Description 4a", "anlrmtype")?;
        Ok(this.map(|l| Lrm { lrm_type, ..l.clone() }))
    }

    /// [§15.1.6 Description 2] ST_UnitOfMeasure().
    pub fn unit_of_measure(&self) -> &str {
        &self.units
    }

    /// [§15.1.6 Description 4] ST_UnitOfMeasure(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF03 (Description 4a); 2FF14 (Description 4c iii).
    pub fn with_unit_of_measure(this: Option<&Lrm>, aunitofmeasure: Option<String>, units: &[Unit<'_>]) -> Result<Option<Lrm>> {
        let unit = required(aunitofmeasure, "§15.1.6 Description 4a", "aunitofmeasure")?;
        let Some(lrm) = this else { return Ok(None) };
        supported(&unit, units, "§15.1.6 Description 4c iii")?;
        Ok(Some(Lrm { units: unit, ..lrm.clone() }))
    }

    /// [§15.1.7 Description 2] ST_Constraints().
    pub fn constraints(&self) -> Option<&[Option<String>]> {
        self.constraints.as_deref()
    }

    /// [§15.1.7 Description 4] ST_Constraints(CHARACTER VARYING ARRAY).
    #[must_use]
    pub fn with_constraints(this: Option<&Lrm>, aconstraintarray: Option<Vec<Option<String>>>) -> Option<Lrm> {
        this.map(|l| Lrm { constraints: aconstraintarray, ..l.clone() })
    }

    /// [§15.1.8 Description 2] ST_OffsetMeasUnit().
    pub fn offset_meas_unit(&self) -> Option<&str> {
        self.offset_units.as_deref()
    }

    /// [§15.1.8 Description 4] ST_OffsetMeasUnit(CHARACTER VARYING).
    /// Reading: 4b i to iii check a unit that is not the null value, as
    /// 4b v gives the null value its own meaning.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF14 (Description 4b iii).
    pub fn with_offset_meas_unit(this: Option<&Lrm>, anoffsetunitofmeasure: Option<String>, units: &[Unit<'_>]) -> Result<Option<Lrm>> {
        let Some(lrm) = this else { return Ok(None) };
        let mut changed = lrm.clone();
        if let Some(unit) = &anoffsetunitofmeasure {
            supported(unit, units, "§15.1.8 Description 4b iii")?;
            changed.lateral_direction.get_or_insert_with(|| RIGHT.to_owned());
            changed.vertical_direction.get_or_insert_with(|| UP.to_owned());
        } else {
            changed.lateral_direction = None;
            changed.vertical_direction = None;
        }
        changed.offset_units = anoffsetunitofmeasure;
        Ok(Some(changed))
    }

    /// [§15.1.9 Description 2] ST_PosLatOffsetDir().
    pub fn pos_lat_offset_dir(&self) -> Option<&str> {
        self.lateral_direction.as_deref()
    }

    /// [§15.1.9 Description 4] ST_PosLatOffsetDir(CHARACTER VARYING).
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF89 (offset unit must be specified) (Description 4b).
    pub fn with_pos_lat_offset_dir(this: Option<&Lrm>, apositivelateraloffsetdirection: Option<String>) -> Result<Option<Lrm>> {
        let Some(lrm) = this else { return Ok(None) };
        let direction = offset_direction(lrm, apositivelateraloffsetdirection, RIGHT, "§15.1.9 Description 4b")?;
        Ok(Some(Lrm { lateral_direction: direction, ..lrm.clone() }))
    }

    /// [§15.1.10 Description 2] ST_PosVerOffsetDir().
    pub fn pos_ver_offset_dir(&self) -> Option<&str> {
        self.vertical_direction.as_deref()
    }

    /// [§15.1.10 Description 4] ST_PosVerOffsetDir(CHARACTER VARYING),
    /// 4c read as the module doc says.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF89 (Description 4b).
    pub fn with_pos_ver_offset_dir(this: Option<&Lrm>, apositiveverticaloffsetdirection: Option<String>) -> Result<Option<Lrm>> {
        let Some(lrm) = this else { return Ok(None) };
        let direction = offset_direction(lrm, apositiveverticaloffsetdirection, UP, "§15.1.10 Description 4b")?;
        Ok(Some(Lrm { vertical_direction: direction, ..lrm.clone() }))
    }

    /// [§15.1.1 Description 10] Whether it allows offsets: it has an
    /// offset unit.
    pub fn allows_offsets(&self) -> bool {
        self.offset_units.is_some()
    }
}

/// [§15.1.9 Description 4b to d, §15.1.10 Description 4b to d] The
/// direction a mutator sets: an error if one is given to an LRM of no
/// offset unit, `default` if none is given to one of an offset unit.
fn offset_direction(lrm: &Lrm, given: Option<String>, default: &str, rule: &str) -> Result<Option<String>> {
    match (given, lrm.allows_offsets()) {
        (Some(_), false) => Err(failed(SqlState::SPATIAL_OFFSET_UNIT_MUST_BE_SPECIFIED, rule, "offset unit must be specified")),
        (None, true) => Ok(Some(default.to_owned())),
        (Some(direction), true) => Ok(Some(direction)),
        (None, false) => Ok(None),
    }
}
