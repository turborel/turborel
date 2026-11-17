//! [ISO/IEC 9075-2:2023 §9.46 SR10] The `<JSON datetime template>` of the
//! path language's `datetime()`: its implicit datetime data type, and the
//! conversion of §9.51 that reads a character string by it, made of the
//! code CAST with FORMAT uses.

use std::sync::Arc;

use super::template::{Field, Of, Part, Template, lexed};
use super::{ScalarFunction, formatted_cast, refused};
use crate::error::{Code, Result, SqlError};
use crate::value::datetime::{Clocked, MAX_TIME_PRECISION};
use crate::value::{CharUnits, DataType};

/// [§9.46 SR10] The conversion of a formatted character string to a
/// datetime [§9.51] by the `<JSON datetime template>` `template`, of the
/// SQL-session `now` gives: a function of one character string whose
/// return type is the template's implicit datetime data type \[SR10f\],
/// and which gives the DATETIME VALUE of §9.51's General Rules.
///
/// # Errors
///
/// SQLCODE -103 if `template` is not of the lexical grammar of a
/// `<datetime template>` [§9.52 SR2], is zoned and not timed \[SR10d\],
/// is neither dated nor timed \[SR10e\], or breaks a Syntax Rule of §9.52
/// for its implicit datetime data type \[SR10h\]; SQLCODE -900 if its
/// fractional seconds precision is more than turborel's maximum, 6
/// (RULES.md D18).
pub(crate) fn json_datetime_cast(template: &str, now: Clocked<'_>) -> Result<Arc<dyn ScalarFunction>> {
    // [SR10] The lexical grammar of a <datetime template>.
    let parts = lexed(template)?;
    let ofs: Vec<Of> = parts.iter().filter_map(|part| if let Part::Field(f) = part { Some(f.of()) } else { None }).collect();
    // [SR10a, SR10b, SR10c]
    let (dated, timed, zoned) = (ofs.contains(&Of::Date), ofs.contains(&Of::Time), ofs.contains(&Of::Zone));
    if zoned && !timed {
        return Err(refused("§9.46 SR10d", &format!("the <JSON datetime template> '{template}' is zoned, so it shall be timed")));
    }
    if !dated && !timed {
        return Err(refused("§9.46 SR10e", &format!("the <JSON datetime template> '{template}' shall be dated or timed or both")));
    }
    let idt = implicit_type(&parts, dated, (timed, zoned))?;
    // [SR10g] A variable-length character string of the maximum template
    // length [§9.52 SR17]; turborel's strings are all of Unicode.
    let most = parts.iter().map(|part| Template::max_length(*part)).fold(0_usize, usize::saturating_add);
    let cst = DataType::Varying { length: u32::try_from(most).unwrap_or(u32::MAX), units: CharUnits::Characters };
    // [SR10h]
    formatted_cast(&cst, &idt, template, now)
}

/// [§9.46 SR10b, SR10f] The implicit datetime data type of a template of
/// `parts`, `dated`, and `timed` and `zoned` or not; one that is dated or
/// timed, and timed if zoned.
///
/// # Errors
///
/// SQLCODE -900 if its fractional seconds precision is more than 6.
fn implicit_type(parts: &[Part], dated: bool, (timed, zoned): (bool, bool)) -> Result<DataType> {
    // [SR10b i, ii] FFn's n, else 0.
    let fsp = parts.iter().find_map(|part| if let Part::Field(Field::Fraction(n)) = part { Some(*n) } else { None });
    let precision = u32::try_from(fsp.unwrap_or(0)).unwrap_or(u32::MAX);
    if precision > MAX_TIME_PRECISION {
        return Err(SqlError::new(
            Code::Unsupported,
            format!(
                "[DESIGN] a <JSON datetime template> of FF{precision} has more digits of the second than {MAX_TIME_PRECISION} (RULES.md D18)"
            ),
        ));
    }
    // [SR10f i to v]
    Ok(if !timed {
        DataType::Date
    } else if dated {
        DataType::Timestamp { precision, zoned }
    } else {
        DataType::Time { precision, zoned }
    })
}

#[cfg(test)]
mod tests;
