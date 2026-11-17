//! [ISO/IEC 9075:1992 §11.8 SR2; ISO/IEC 9075-2:2023 §11.8 SR4, SR18a i
//! 6] The columns a foreign key references: of a unique constraint of the
//! referenced table, WITHOUT OVERLAPS of the referenced period of a
//! temporal one and of none of another.

use super::{Described, Within, rule};
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::error::Result;

/// [ISO/IEC 9075:1992 §11.8 SR2] The positions of the columns a foreign
/// key, written at `at`, references of table `target`, `parent`: those
/// `written`, of a unique constraint of it [SR2a], or else its PRIMARY KEY
/// [SR2b]; of a temporal one, of a unique constraint WITHOUT OVERLAPS of
/// the period `keyed` [ISO/IEC 9075-2:2023 §11.8 SR18a i 6].
pub(super) fn referenced_columns(
    (target, parent): (&QualifiedName, &TableDesc),
    (written, keyed): (Option<&[String]>, Option<&str>),
    at: usize,
) -> Result<Vec<usize>> {
    // [ISO/IEC 9075-2:2023 §11.8 SR4] Of a unique constraint WITHOUT
    // OVERLAPS, none but a temporal one's: its columns alone are no key.
    let keys = parent.constraints.iter().filter_map(|c| match &c.kind {
        ConstraintKind::Unique { columns, primary, period, .. } => Some((columns, *primary, period.as_deref())),
        ConstraintKind::NotNull(_) | ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => None,
    });
    let Some(written) = written else {
        return match keys.into_iter().find(|k| k.1) {
            Some((columns, _, None)) => Ok(columns.clone()),
            Some((_, _, Some(_))) => Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.8 SR4b] the PRIMARY KEY of {target} is WITHOUT OVERLAPS of a period, which a foreign key with no column list does not reference"
            ))
            .or_at(at)),
            None => Err(rule(format!("[ISO/IEC 9075:1992 §11.8 SR2b] {target} has no PRIMARY KEY")).or_at(at)),
        };
    };
    let positions =
        Described::new((target, &target.auth, Within::Session), &parent.columns, Vec::new()).positions(written, "§11.8 SR2a")?;
    let mut keys = keys;
    let found = keys.any(|(u, _, period)| period == keyed && u.len() == positions.len() && u.iter().all(|c| positions.contains(c)));
    if !found {
        let (cited, of) = keyed.map_or(("ISO/IEC 9075:1992 §11.8 SR2a", String::new()), |p| {
            ("ISO/IEC 9075-2:2023 §11.8 SR18a i 6", format!(" WITHOUT OVERLAPS of period {p}"))
        });
        return Err(rule(format!("[{cited}] the columns referenced are not those of a unique constraint{of} of {target}")).or_at(at));
    }
    Ok(positions)
}
