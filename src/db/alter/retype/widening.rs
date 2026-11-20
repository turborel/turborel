//! [ISO/IEC 9075-2:2023 §11.19 SR13 to SR15] Which declared types a
//! column may be given in place of its own: predefined types both, and
//! the new one of the old one's kind, as long or as precise as it at
//! least, so every value the column holds is one of the new type too.

use crate::value::interval::Qualifier;
use crate::value::lob::LobKind;
use crate::value::{CharUnits, DataType};

/// [§11.19 SR13, SR14] Whether `t` is a predefined type [§4.1.2]: no
/// constructed type, no user-defined type and no type beyond the
/// standard.
pub(super) fn predefined(t: &DataType) -> bool {
    match t {
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Json
        | DataType::Xml(_) => true,
        DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => false,
    }
}

/// The length or maximum length of a character string type, and its
/// units.
fn characters(t: &DataType) -> Option<(u64, CharUnits)> {
    if let DataType::Character { length, units } | DataType::Varying { length, units } = t {
        return Some((u64::from(*length), *units));
    }
    if let DataType::Lob { kind: LobKind::Character, length, units } = t {
        return Some((*length, *units));
    }
    None
}

/// The length or maximum length of a binary string type.
fn octets(t: &DataType) -> Option<u64> {
    if let DataType::Binary { length } | DataType::VarBinary { length } = t {
        return Some(u64::from(*length));
    }
    if let DataType::Lob { kind: LobKind::Binary, length, .. } = t {
        return Some(*length);
    }
    None
}

/// [§11.19 SR15h, SR15i] Whether interval qualifier `d` has the fields of
/// `dtc`, and precisions as great.
fn interval_widened(dtc: Qualifier, d: Qualifier) -> bool {
    let fields = d.start == dtc.start && d.end == dtc.end;
    fields && d.leading >= dtc.leading && d.fraction >= dtc.fraction
}

/// [§11.19 SR15c] Whether exact numeric precision and scale `d` hold
/// every value of `dtc`: no less precision, scale or digits before the
/// point.
fn exact_widened((pdtc, sdtc): (u32, u32), (pd, sd): (u32, u32)) -> bool {
    pd >= pdtc && sd >= sdtc && pd.saturating_sub(sd) >= pdtc.saturating_sub(sdtc)
}

/// [§11.19 SR15] The item of SR15 that `dtc`, the column's declared type,
/// is of and `d`, the type it is given, breaks; none if `d` may take its
/// place.
pub(super) fn narrowed(dtc: &DataType, d: &DataType) -> Option<&'static str> {
    if let Some((ldtc, units)) = characters(dtc) {
        // [SR15a] A character string type as long; of the same units, as
        // the lengths are compared.
        let kept = characters(d).is_some_and(|(ld, u)| u == units && ld >= ldtc);
        return (!kept).then_some("SR15a");
    }
    if let Some(ldtc) = octets(dtc) {
        return octets(d).is_none_or(|ld| ld < ldtc).then_some("SR15b");
    }
    let (kept, clause) = match (dtc, d) {
        (DataType::Exact { precision: p, scale: s, .. }, DataType::Exact { precision, scale, .. }) => {
            (exact_widened((*p, *s), (*precision, *scale)), "SR15c")
        }
        (DataType::Exact { .. }, _) => (false, "SR15c"),
        (DataType::Approximate { precision: p, .. }, DataType::Approximate { precision, .. }) => (precision >= p, "SR15d"),
        (DataType::Approximate { .. }, _) => (false, "SR15d"),
        (DataType::Date, _) => (*d == DataType::Date, "SR15e"),
        (DataType::Time { precision: p, zoned: z }, DataType::Time { precision, zoned }) => (z == zoned && precision >= p, "SR15f"),
        (DataType::Time { .. }, _) => (false, "SR15f"),
        (DataType::Timestamp { precision: p, zoned: z }, DataType::Timestamp { precision, zoned }) => {
            (z == zoned && precision >= p, "SR15g")
        }
        (DataType::Timestamp { .. }, _) => (false, "SR15g"),
        (DataType::Interval(q), DataType::Interval(r)) => (interval_widened(*q, *r), "SR15h, SR15i"),
        (DataType::Interval(_), _) => (false, "SR15h, SR15i"),
        (DataType::Boolean, _) => (*d == DataType::Boolean, "SR15j"),
        // [SR15k] Of any other, the type it is.
        _ => (d == dtc, "SR15k"),
    };
    (!kept).then_some(clause)
}

#[cfg(test)]
mod tests;
