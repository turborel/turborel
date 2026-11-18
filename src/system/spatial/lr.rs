//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21] The SQL/MM Linear
//! Referencing Information and Definition Schemas: the base tables
//! ST_LR_COLUMNS and ST_LRMS of ST_DEFINITION_SCHEMA, made from the
//! definitions as a statement reads them, as §20's are, and the views of
//! ST_INFORMTN_SCHEMA over them.
//!
//! * ST_LR_COLUMNS lists each column of a table or view whose declared
//!   type is a linear referencing type [§21.2.2 Description 2], with the
//!   method its registration names (`ST_REGISTERLRCOLUMN`), none if it
//!   has none [Description 3].
//! * ST_LRMS lists each method defined (`ST_DEFINELRM`).

use super::{int, integer};
use crate::catalog::ColumnDesc;
use crate::spatial::lr::LrKind;
use crate::spatial::lr::sql::{LRM_NAME_LENGTH, data_type_of, of_data_type, stored_value};
use crate::storage::Row;
use crate::system::{Seen, SystemView, catalog_name, identifier, text, text_column};
use crate::value::Value;

/// ST_MaxLRAsText [§21.2.3 DR3], the length of DEFINITION.
const DEFINITION_LENGTH: u32 = 16_777_216;

/// [§21.2.2] The columns of ST_LR_COLUMNS.
pub(super) fn lr_columns_columns() -> Vec<ColumnDesc> {
    let names = ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"];
    let mut all: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
    all.push(integer("LRM_ID", false));
    all
}

/// [§21.2.2] Each column of a base table or view of a linear referencing
/// type, with the identifier of the method it is registered with.
pub(super) fn lr_columns(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let spatial = seen.state.catalog.spatial();
    let tables = seen.state.catalog.tables().into_iter().map(|t| (&t.name, &t.columns));
    let views = seen.state.catalog.views().into_iter().map(|v| (&v.name, &v.columns));
    let mut rows = Vec::new();
    for (name, columns) in tables.chain(views) {
        for c in columns.iter().filter(|c| of_data_type(&c.data_type).is_some()) {
            let lrm = spatial.lr_column(name, &c.name).and_then(|r| r.lrm).map_or(Value::Null, |id| int(i64::from(id)));
            rows.push(vec![catalog.clone(), text(&name.auth), text(&name.ident), text(&c.name), lrm]);
        }
    }
    rows
}

/// [§21.2.3] The columns of ST_LRMS.
pub(super) fn lrms_columns() -> Vec<ColumnDesc> {
    vec![
        text_column(("LRM_NAME", LRM_NAME_LENGTH), true),
        integer("LRM_ID", true),
        text_column(("ORGANIZATION", super::ORGANIZATION_LENGTH), false),
        integer("ORGANIZATION_LRM_ID", false),
        text_column(("DEFINITION", DEFINITION_LENGTH), true),
        text_column(("DESCRIPTION", super::DESCRIPTION_LENGTH), false),
        ColumnDesc { data_type: data_type_of(LrKind::Lrm), ..text_column(("LRM", 1), false) },
    ]
}

/// [§21.2.3] Each method defined, in order of its identifier.
pub(super) fn lrms(seen: &Seen<'_>) -> Vec<Row> {
    let row = |l: &crate::catalog::LrmDesc| {
        let (org, org_id) = l.organization.as_ref().map_or((Value::Null, Value::Null), |(o, i)| (text(o), int(i64::from(*i))));
        let description = l.description.as_deref().map_or(Value::Null, text);
        let lrm = stored_value(&l.lrm).unwrap_or(Value::Null);
        vec![text(&l.name), int(i64::from(l.id)), org, org_id, text(&l.definition), description, lrm]
    };
    seen.state.catalog.spatial().lrms().iter().map(row).collect()
}

/// [§21.1.2] The view ST_LR_COLUMNS of ST_INFORMTN_SCHEMA. It shows what
/// INFORMATION_SCHEMA.COLUMNS shows its user, as §19's
/// ST_GEOMETRY_COLUMNS does.
pub(super) const LR_COLUMNS_VIEW: SystemView = SystemView {
    name: "ST_LR_COLUMNS",
    query: r"SELECT c.TABLE_CATALOG, c.TABLE_SCHEMA, c.TABLE_NAME, c.COLUMN_NAME, l.LRM_ID AS LRMID
       FROM INFORMATION_SCHEMA.COLUMNS AS c
            JOIN ST_DEFINITION_SCHEMA.ST_LR_COLUMNS AS l ON
              ( c.TABLE_CATALOG = l.TABLE_CATALOG AND c.TABLE_SCHEMA = l.TABLE_SCHEMA AND
                c.TABLE_NAME = l.TABLE_NAME AND c.COLUMN_NAME = l.COLUMN_NAME )",
};

/// [§21.1.3] The view ST_LRMS of ST_INFORMTN_SCHEMA. It reads
/// ORGANIZATION_LRM_ID, the base table's column, where the standard's
/// definition names an ORGANIZATION_COORDSYS_ID the base table does not
/// have.
pub(super) const LRMS_VIEW: SystemView = SystemView {
    name: "ST_LRMS",
    query: r"SELECT LRM_NAME, LRM_ID AS LRMID, ORGANIZATION AS AUTH_NAME, ORGANIZATION_LRM_ID AS AUTH_ID,
         DEFINITION AS WKT, DESCRIPTION, LRM
       FROM ST_DEFINITION_SCHEMA.ST_LRMS",
};
