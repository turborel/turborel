//! [ISO/IEC 9075-2:2023 §4.26.3] The known functional dependencies of a
//! base table: of its keys, and of its generated columns.

use super::{Attrs, Known, Walk};
use crate::binder::generations;
use crate::catalog::{ConstraintKind, QualifiedName, TableDesc};
use crate::logical::expr_columns;

/// [§4.26.3] The base table `table`, of its first `width` columns, those
/// the query sees. A non-deferrable PRIMARY KEY, and a non-deferrable
/// UNIQUE of columns known not null, determine every column, a BPK-set and
/// a BUC-set; so does a non-deferrable UNIQUE NULLS NOT DISTINCT of any
/// columns (IE015), a BUC-set [§4.26.2], as no two rows are alike in them. A generated column is determined by its
/// parametric columns. A column is known not null of a non-deferrable NOT
/// NULL or PRIMARY KEY. A table the catalog does not have, a declared
/// local temporary one, has nothing known.
pub(super) fn base_table(w: &Walk<'_>, table: &QualifiedName, width: usize) -> Known {
    let mut known = w.fresh(width);
    let Some(desc) = w.db.catalog.table(table) else { return known };
    let not_null = not_null(desc);
    known.not_null = not_null.iter().filter_map(|&c| known.attribute_of(c)).collect();
    let every: Attrs = known.columns.iter().copied().collect();
    for c in desc.constraints.iter().filter(|c| !c.attributes.deferrable) {
        let ConstraintKind::Unique { columns, primary, nulls_distinct, period } = &c.kind else { continue };
        if !(*primary || !*nulls_distinct || columns.iter().all(|k| not_null.contains(k))) {
            continue;
        }
        // [ISO/IEC 9075-2:2023 §11.7 SR6d i] Of one WITHOUT OVERLAPS, its
        // columns with the period's start and end, which no two rows share.
        if let Some(key) = known.attributes_of(&desc.key_columns(columns, period.as_deref())) {
            known.depends(key.clone(), every.clone());
            known.keys.push((key, *primary));
        }
    }
    if w.strict {
        known.types =
            desc.columns.iter().enumerate().filter_map(|(c, col)| Some((known.attribute_of(c)?, col.data_type.clone()))).collect();
    }
    generated(w, desc, &mut known);
    known.collations = desc
        .columns
        .iter()
        .enumerate()
        .filter_map(|(c, col)| Some((known.attribute_of(c)?, w.db.collation_named(col.collation.as_ref()?)?)))
        .collect();
    known
}

/// The columns of `desc` known not null [§4.26.3, §4.18]: of a
/// non-deferrable NOT NULL, or of a non-deferrable PRIMARY KEY.
fn not_null(desc: &TableDesc) -> Vec<usize> {
    let mut columns = Vec::new();
    for c in desc.constraints.iter().filter(|c| !c.attributes.deferrable) {
        match &c.kind {
            ConstraintKind::NotNull(at) => columns.push(*at),
            ConstraintKind::Unique { columns: key, primary: true, .. } => columns.extend(key),
            ConstraintKind::Unique { primary: false, .. }
            | ConstraintKind::ForeignKey(_)
            | ConstraintKind::Check { .. }
            | ConstraintKind::Connection(_) => {}
        }
    }
    columns
}

/// [§4.26.3] Of each generated column of `desc` the query sees, that its
/// parametric columns, those its generation expression references,
/// determine it. A generation expression that no longer binds gives
/// nothing: the table cannot be written, and nothing is known of it.
fn generated(w: &Walk<'_>, desc: &TableDesc, known: &mut Known) {
    let Ok(bound) = generations(w.db, desc) else { return };
    for (at, e) in bound {
        let mut read = Vec::new();
        expr_columns(&e, &mut read);
        let from = known.attributes_of(&read).filter(|from| known.told_alike(from));
        if let (Some(from), Some(c)) = (from, known.attribute_of(at)) {
            known.depends(from, Attrs::from([c]));
        }
    }
}
