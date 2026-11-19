//! [OGC 06-104r4 §7.1] The geometry columns and spatial reference systems
//! tables of Simple feature access, SQL option, in its "SQL with Geometry
//! Types" environment: GEOMETRY_COLUMNS and SPATIAL_REF_SYS, each a view
//! of the schema SF_INFORMTN_SCHEMA, as §7.1.1 lets them be, of what
//! SQL/MM's Spatial Definition Schema holds (RULES.md D136).
//!
//! * SPATIAL_REF_SYS [§7.1.2.2] is each system supplied or defined
//!   (ST_DEFINESRS) under the columns §7.1.2.2 names.
//! * GEOMETRY_COLUMNS [§7.1.3.2] lists each column of a geometry type
//!   that INFORMATION_SCHEMA.COLUMNS shows its user, of a table or view,
//!   from the base table SF_GEOMETRY_COLUMNS of ST_DEFINITION_SCHEMA. Its
//!   feature table is its geometry table [§7.1.3.3 c]; its STORAGE_TYPE
//!   and MAX_PPR are null, of no normalized storage [§7.1.3.3 d, h]. Its
//!   SRID is that of the system the column is registered with
//!   (ST_REGISTERGEOMETRYCOLUMN), else the one its type names,
//!   `POINT(4326)` say, else 0, SQL/MM's unspecified system, which no
//!   row of SPATIAL_REF_SYS describes. COORD_DIMENSION is 3 in a
//!   geocentric system, else 2; GEOMETRY_TYPE the code of Table 4, or
//!   of Part 1's Table 7 [OGC 06-103r4 §8.2.3] it is a subset of, of the
//!   column's declared type, with 1000 of z in three coordinate
//!   dimensions; null of a type neither table codes.
//!
//! ANNOTATION_TEXT_METADATA [§6.2.9.2 Table 3] lists each column of type
//! ANNOTATION_TEXT [§7.2.20] the same way, from the base table
//! SF_ANNOTATION_TEXT_METADATA. Its defaults are null: no map base scale,
//! expression or attributes is kept beside a column, each text value
//! holding its own.
//!
//! An unqualified name of a view that no table of the default
//! schema has names it (`binder/from.rs`), as a client of Simple feature
//! access writes `FROM geometry_columns` [Annex C.3.3.3 T1].

use super::super::{Defined, Rows, Seen, SystemTable, SystemView, catalog_name, identifier, text, text_column};
use super::{SRS_SEED, ST_DEFINITION_SCHEMA, int, integer};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::storage::Row;
use crate::value::Value;

/// The schema of the views of Simple feature access.
pub const SF_INFORMTN_SCHEMA: &str = "SF_INFORMTN_SCHEMA";

/// The base table GEOMETRY_COLUMNS reads.
const BASE: &str = "SF_GEOMETRY_COLUMNS";
/// The key word a geocentric system's well-known text begins with, its
/// `<geocentric cs>` [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §14.1.8].
const GEOCENTRIC: &str = "GEOCCS";
/// COORD_DIMENSION of a geocentric system.
const THREE_DIMENSIONS: i64 = 3;
/// COORD_DIMENSION of any other.
const TWO_DIMENSIONS: i64 = 2;
/// [§7.1.3.3 Table 4] What a type's code adds of z.
const Z_CODE: i64 = 1000;

/// The base table ANNOTATION_TEXT_METADATA reads.
const TEXT_BASE: &str = "SF_ANNOTATION_TEXT_METADATA";

/// The base tables of GEOMETRY_COLUMNS and ANNOTATION_TEXT_METADATA.
pub(super) fn tables() -> [std::sync::Arc<dyn SystemTable>; 2] {
    [
        std::sync::Arc::new(Defined { schema: ST_DEFINITION_SCHEMA, name: BASE, columns, rows: Rows::Infallible(rows) }),
        std::sync::Arc::new(Defined {
            schema: ST_DEFINITION_SCHEMA,
            name: TEXT_BASE,
            columns: text_columns,
            rows: Rows::Infallible(text_rows),
        }),
    ]
}

/// [§6.2.9.2 Table 3] The columns of ANNOTATION_TEXT_METADATA.
fn text_columns() -> Vec<ColumnDesc> {
    let names = ["F_TABLE_CATALOG", "F_TABLE_SCHEMA", "F_TABLE_NAME", "F_TEXT_COLUMN"];
    let mut all: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
    let length = crate::geometry::support::MAX_ANNOTATION_SIZE;
    let defaults = ["A_TEXT_DEFAULT_MAP_BASE_SCALE", "A_TEXT_DEFAULT_EXPRESSION", "A_TEXT_DEFAULT_ATTRIBUTES"];
    all.extend(defaults.iter().map(|n| text_column((n, length), false)));
    all
}

/// Each column of a base table or view of type ANNOTATION_TEXT.
fn text_rows(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let tables = seen.state.catalog.tables().into_iter().map(|t| (&t.name, &t.columns));
    let views = seen.state.catalog.views().into_iter().map(|v| (&v.name, &v.columns));
    let text_type = Some(crate::geometry::support::Support::AnnotationText);
    let mut rows = Vec::new();
    for (name, columns) in tables.chain(views) {
        for c in columns.iter().filter(|c| crate::geometry::support::of_data_type(&c.data_type) == text_type) {
            rows.push(vec![catalog.clone(), text(&name.auth), text(&name.ident), text(&c.name), Value::Null, Value::Null, Value::Null]);
        }
    }
    rows
}

/// [§7.1.3.2] The columns of GEOMETRY_COLUMNS.
fn columns() -> Vec<ColumnDesc> {
    let names =
        ["F_TABLE_CATALOG", "F_TABLE_SCHEMA", "F_TABLE_NAME", "F_GEOMETRY_COLUMN", "G_TABLE_CATALOG", "G_TABLE_SCHEMA", "G_TABLE_NAME"];
    let mut all: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
    all.extend(["STORAGE_TYPE", "GEOMETRY_TYPE", "COORD_DIMENSION", "MAX_PPR"].iter().map(|n| integer(n, false)));
    all.push(integer("SRID", true));
    all
}

/// Each column of a base table or view of a geometry type.
fn rows(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let tables = seen.state.catalog.tables().into_iter().map(|t| (&t.name, &t.columns));
    let views = seen.state.catalog.views().into_iter().map(|v| (&v.name, &v.columns));
    let mut rows = Vec::new();
    for (name, columns) in tables.chain(views) {
        for c in columns {
            if let Some(kind) = crate::geometry::of_data_type(&c.data_type) {
                rows.push(row(seen, (&catalog, name), c, kind));
            }
        }
    }
    rows
}

/// The row of column `c` of `name`, of declared type `kind`.
fn row(seen: &Seen<'_>, (catalog, name): (&Value, &QualifiedName), c: &ColumnDesc, kind: crate::geometry::model::Kind) -> Row {
    let spatial = seen.state.catalog.spatial();
    let registered = spatial.column(name, &c.name).and_then(|r| r.srs.as_deref()).and_then(|srs| spatial.system_id(srs));
    let srid = registered.or_else(|| crate::geometry::fixed_srid(&c.data_type)).unwrap_or(0_i32);
    let dimension = if geocentric(seen, srid) { THREE_DIMENSIONS } else { TWO_DIMENSIONS };
    let z = if dimension == THREE_DIMENSIONS { Z_CODE } else { 0 };
    let code = crate::geometry::sfa::type_code(kind).map_or(Value::Null, |base| int(base.saturating_add(z)));
    let (table, column) = (text(&name.ident), text(&c.name));
    let feature = [catalog.clone(), text(&name.auth), table];
    [feature.to_vec(), vec![column], feature.to_vec(), vec![Value::Null, code, int(dimension), Value::Null, int(i64::from(srid))]].concat()
}

/// Whether the system of identifier `srid`, supplied or defined, is
/// geocentric: its text a `<geocentric cs>`.
fn geocentric(seen: &Seen<'_>, srid: i32) -> bool {
    let supplied = SRS_SEED.iter().find(|s| s.id == srid).map(|s| s.definition.as_str());
    let defined = || seen.state.catalog.spatial().systems().iter().find(|s| s.id == srid).map(|s| s.definition.as_str());
    supplied.or_else(defined).is_some_and(|d| d.trim_start().get(..GEOCENTRIC.len()).is_some_and(|w| w.eq_ignore_ascii_case(GEOCENTRIC)))
}

/// [§6.2.9.2, §7.1.2.2, §7.1.3.2] The views, of SF_INFORMTN_SCHEMA.
pub const VIEWS: &[SystemView] = &[
    SystemView {
        name: "GEOMETRY_COLUMNS",
        query: r"SELECT g.F_TABLE_CATALOG, g.F_TABLE_SCHEMA, g.F_TABLE_NAME, g.F_GEOMETRY_COLUMN,
         g.G_TABLE_CATALOG, g.G_TABLE_SCHEMA, g.G_TABLE_NAME, g.STORAGE_TYPE, g.GEOMETRY_TYPE,
         g.COORD_DIMENSION, g.MAX_PPR, g.SRID
       FROM INFORMATION_SCHEMA.COLUMNS AS c
            JOIN ST_DEFINITION_SCHEMA.SF_GEOMETRY_COLUMNS AS g ON
              ( c.TABLE_CATALOG = g.F_TABLE_CATALOG AND c.TABLE_SCHEMA = g.F_TABLE_SCHEMA AND
                c.TABLE_NAME = g.F_TABLE_NAME AND c.COLUMN_NAME = g.F_GEOMETRY_COLUMN )",
    },
    SystemView {
        name: "SPATIAL_REF_SYS",
        query: r"SELECT SRS_ID AS SRID, ORGANIZATION AS AUTH_NAME, ORGANIZATION_COORDSYS_ID AS AUTH_SRID,
         DEFINITION AS SRTEXT
       FROM ST_DEFINITION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS",
    },
    SystemView {
        name: "ANNOTATION_TEXT_METADATA",
        query: r"SELECT a.F_TABLE_CATALOG, a.F_TABLE_SCHEMA, a.F_TABLE_NAME, a.F_TEXT_COLUMN,
         a.A_TEXT_DEFAULT_MAP_BASE_SCALE, a.A_TEXT_DEFAULT_EXPRESSION, a.A_TEXT_DEFAULT_ATTRIBUTES
       FROM INFORMATION_SCHEMA.COLUMNS AS c
            JOIN ST_DEFINITION_SCHEMA.SF_ANNOTATION_TEXT_METADATA AS a ON
              ( c.TABLE_CATALOG = a.F_TABLE_CATALOG AND c.TABLE_SCHEMA = a.F_TABLE_SCHEMA AND
                c.TABLE_NAME = a.F_TABLE_NAME AND c.COLUMN_NAME = a.F_TEXT_COLUMN )",
    },
];

/// The view of SF_INFORMTN_SCHEMA an unqualified `ident` names, if it
/// names one.
pub fn public(ident: &str) -> Option<QualifiedName> {
    VIEWS.iter().any(|v| v.name == ident).then(|| QualifiedName { auth: SF_INFORMTN_SCHEMA.to_owned(), ident: ident.to_owned() })
}
