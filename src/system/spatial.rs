//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §19, §20] The SQL/MM Spatial
//! Information Schema, ST_INFORMTN_SCHEMA, and the Definition Schema it
//! reads, ST_DEFINITION_SCHEMA (RULES.md D-SPATIAL-CATALOG).
//!
//! Clause 20's base tables are "only a model" [§20.1], so each is a system
//! table made from the definitions as a statement reads it, and, as the
//! Definition Schema of ISO/IEC 9075 is, read only by the views of its
//! Information Schema. Those views are the standard's definitions
//! (`SPATIAL_VIEWS`), bound as INFORMATION_SCHEMA binds its own, SELECT on
//! them PUBLIC's: ST_GEOMETRY_COLUMNS shows what INFORMATION_SCHEMA.COLUMNS
//! shows its user, of the columns the base table lists.
//!
//! * ST_GEOMETRY_COLUMNS lists each column of a table or view whose
//!   declared type is ST_Geometry or one of its subtypes [§20.2
//!   Description 2], with the spatial reference system its type names,
//!   `ST_POINT(4326)` say, and none of a type that names none [§20.2
//!   Description 3].
//! * ST_SPATIAL_REFERENCE_SYSTEMS holds the systems this implementation
//!   supplies (`spatial/systems.rs`), ST_UNITS_OF_MEASURE its units, and
//!   ST_SIZINGS each meta-variable of Annex A.1 with an integer value.

mod systems;

use super::{Defined, Rows, Seen, SystemTable, SystemView, catalog_name, identifier, text, text_column};
use crate::catalog::ColumnDesc;
use crate::storage::Row;
use crate::value::{DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

pub use systems::{SRS_SEED, SpatialRefSys};

/// [§19.1] The schema of the SQL/MM Spatial Information Schema.
pub const ST_INFORMTN_SCHEMA: &str = "ST_INFORMTN_SCHEMA";

/// [§20.1] The schema of the SQL/MM Spatial Definition Schema.
pub const ST_DEFINITION_SCHEMA: &str = "ST_DEFINITION_SCHEMA";

/// ST_MaxSRSNameLength [§20.2 DR1].
pub const SRS_NAME_LENGTH: u32 = 128;
/// ST_MaxOrganizationNameLength [§20.3 DR2].
pub const ORGANIZATION_LENGTH: u32 = 128;
/// ST_MaxSRSDefinitionLength [§20.3 DR3].
pub const DEFINITION_LENGTH: u32 = 2048;
/// ST_MaxDescriptionLength [§20.3 DR4].
pub const DESCRIPTION_LENGTH: u32 = 256;
/// ST_MaxUnitNameLength [§20.4 DR1].
pub const UNIT_NAME_LENGTH: u32 = 64;
/// ST_MaxUnitTypeLength [§20.4 DR2].
pub const UNIT_TYPE_LENGTH: u32 = 16;
/// ST_MaxVariableNameLength [§20.5 DR1].
pub const VARIABLE_NAME_LENGTH: u32 = 64;

/// The base tables of the Definition Schema [§20.2 to §20.5].
pub(super) fn tables() -> Vec<std::sync::Arc<dyn SystemTable>> {
    let table = |name, columns, rows| -> std::sync::Arc<dyn SystemTable> {
        std::sync::Arc::new(Defined { schema: ST_DEFINITION_SCHEMA, name, columns, rows: Rows::Infallible(rows) })
    };
    vec![
        table("ST_GEOMETRY_COLUMNS", geometry_columns_columns, geometry_columns),
        table("ST_SPATIAL_REFERENCE_SYSTEMS", systems_columns, systems),
        table("ST_UNITS_OF_MEASURE", units_columns, units),
        table("ST_SIZINGS", sizings_columns, sizings),
    ]
}

/// A column of INTEGER, NOT NULL if `not_null`.
fn integer(name: &str, not_null: bool) -> ColumnDesc {
    let data_type = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };
    ColumnDesc { data_type, ..text_column((name, 1), not_null) }
}

/// An INTEGER value.
fn int(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

// ---------------------------------------------------------------------------
// [§20.2] ST_GEOMETRY_COLUMNS
// ---------------------------------------------------------------------------

fn geometry_columns_columns() -> Vec<ColumnDesc> {
    let names = ["TABLE_CATALOG", "TABLE_SCHEMA", "TABLE_NAME", "COLUMN_NAME"];
    let mut all: Vec<ColumnDesc> = names.iter().map(|n| identifier(n, true)).collect();
    all.push(text_column(("SRS_NAME", SRS_NAME_LENGTH), false));
    all
}

/// Each column of a base table or view of a geometry type, with the name
/// of the system it is registered with (`ST_REGISTERGEOMETRYCOLUMN`); of
/// one not registered, of the system its type's SRID identifies, if one
/// is supplied or defined.
fn geometry_columns(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let spatial = seen.state.catalog.spatial();
    let tables = seen.state.catalog.tables().into_iter().map(|t| (&t.name, &t.columns));
    let views = seen.state.catalog.views().into_iter().map(|v| (&v.name, &v.columns));
    let mut rows = Vec::new();
    for (name, columns) in tables.chain(views) {
        for c in columns.iter().filter(|c| crate::geometry::of_data_type(&c.data_type).is_some()) {
            let registered = spatial.column(name, &c.name).map(|r| r.srs.as_deref());
            let typed = || crate::geometry::fixed_srid(&c.data_type).and_then(|id| spatial.system_name(id));
            let srs = registered.unwrap_or_else(typed).map_or(Value::Null, text);
            rows.push(vec![catalog.clone(), text(&name.auth), text(&name.ident), text(&c.name), srs]);
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// [§20.3] ST_SPATIAL_REFERENCE_SYSTEMS
// ---------------------------------------------------------------------------

fn systems_columns() -> Vec<ColumnDesc> {
    vec![
        text_column(("SRS_NAME", SRS_NAME_LENGTH), true),
        integer("SRS_ID", true),
        text_column(("ORGANIZATION", ORGANIZATION_LENGTH), false),
        integer("ORGANIZATION_COORDSYS_ID", false),
        text_column(("DEFINITION", DEFINITION_LENGTH), true),
        text_column(("DESCRIPTION", DESCRIPTION_LENGTH), false),
    ]
}

/// Each system supplied or defined (`ST_DEFINESRS`), in order of its
/// identifier.
fn systems(seen: &Seen<'_>) -> Vec<Row> {
    let supplied = SRS_SEED.iter().map(|s| {
        (
            s.id,
            vec![
                text(&s.name),
                int(i64::from(s.id)),
                text(s.organization),
                int(i64::from(s.id)),
                text(&s.definition),
                text(&s.description),
            ],
        )
    });
    let defined = seen.state.catalog.spatial().systems().iter().map(|s| {
        let (org, org_id) = s.organization.as_ref().map_or((Value::Null, Value::Null), |(o, i)| (text(o), int(i64::from(*i))));
        let description = s.description.as_deref().map_or(Value::Null, text);
        (s.id, vec![text(&s.name), int(i64::from(s.id)), org, org_id, text(&s.definition), description])
    });
    let mut all: Vec<(i32, Row)> = supplied.chain(defined).collect();
    all.sort_by_key(|(id, _)| *id);
    all.into_iter().map(|(_, row)| row).collect()
}

// ---------------------------------------------------------------------------
// [§20.4] ST_UNITS_OF_MEASURE
// ---------------------------------------------------------------------------

/// The units of measure supplied: name, type and the factor converting
/// to the base unit, METRE of LINEAR and RADIAN of ANGULAR [§20.4
/// Description 3].
pub const UNITS: [(&str, &str, f64, &str); 12] = [
    ("METRE", "LINEAR", 1.0_f64, "the metre, the base unit of length"),
    ("KILOMETRE", "LINEAR", 1000.0_f64, "a thousand metres"),
    ("FOOT", "LINEAR", 0.3048_f64, "the international foot"),
    ("US SURVEY FOOT", "LINEAR", 0.304_800_609_601_219_2_f64, "the US survey foot, 1200/3937 metre"),
    ("STATUTE MILE", "LINEAR", 1609.344_f64, "the international statute mile"),
    ("NAUTICAL MILE", "LINEAR", 1852.0_f64, "the international nautical mile"),
    ("RADIAN", "ANGULAR", 1.0_f64, "the radian, the base unit of angle"),
    ("MICRORADIAN", "ANGULAR", 1.0e-6_f64, "a millionth of a radian"),
    ("DEGREE", "ANGULAR", std::f64::consts::PI / 180.0_f64, "a 360th of a turn"),
    ("ARC-MINUTE", "ANGULAR", std::f64::consts::PI / 10_800.0_f64, "a 60th of a degree"),
    ("ARC-SECOND", "ANGULAR", std::f64::consts::PI / 648_000.0_f64, "a 3600th of a degree"),
    ("GRAD", "ANGULAR", std::f64::consts::PI / 200.0_f64, "a 400th of a turn"),
];

fn units_columns() -> Vec<ColumnDesc> {
    let double = DataType::Approximate { precision: crate::value::DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };
    vec![
        text_column(("UNIT_NAME", UNIT_NAME_LENGTH), true),
        text_column(("UNIT_TYPE", UNIT_TYPE_LENGTH), true),
        ColumnDesc { data_type: double, ..text_column(("CONVERSION_FACTOR", 1), true) },
        text_column(("DESCRIPTION", DESCRIPTION_LENGTH), false),
    ]
}

fn units(_seen: &Seen<'_>) -> Vec<Row> {
    UNITS.iter().map(|(name, kind, factor, about)| vec![text(name), text(kind), Value::Approx(*factor), text(about)]).collect()
}

// ---------------------------------------------------------------------------
// [§20.5] ST_SIZINGS
// ---------------------------------------------------------------------------

/// [Annex A.1] The meta-variables of what is not supported.
const UNSUPPORTED: [&str; 24] = [
    "ST_MaxAngleAsGML",
    "ST_MaxAngleAsText",
    "ST_MaxAngleString",
    "ST_MaxConstraintArrayElements",
    "ST_MaxConstraintLength",
    "ST_MaxDirectionAsGML",
    "ST_MaxDirectionAsText",
    "ST_MaxDirectionString",
    "ST_MaxFeatureIDLength",
    "ST_MaxGeometryAsGML",
    "ST_MaxLEMeasureArrayElements",
    "ST_MaxLRAsGML",
    "ST_MaxLRAsText",
    "ST_MaxLRMNameLength",
    "ST_MaxPositionExpArrayElements",
    "ST_MaxReferentArrayElements",
    "ST_MaxSRSAsText",
    "ST_MaxStartValueArrayElements",
    "ST_MaxTopologyName",
    "ST_MaxTopologyOrNetworkName",
    "ST_MaxVectorArrayElements",
    "ST_MaxVectorAsBinary",
    "ST_MaxVectorAsGML",
    "ST_MaxVectorAsText",
];

/// [Annex A.1] Each meta-variable of an integer value: its value, 0 of no
/// limit, none of what is not supported [§20.5 Description 2].
pub fn sizing_values() -> Vec<(&'static str, Option<i64>, &'static str)> {
    let bytes = i64::try_from(crate::geometry::MAX_BYTES).unwrap_or(0);
    let network = i64::try_from(crate::spatial::network::sql::MAX_NETWORK_NAME).unwrap_or(0);
    let mut all = vec![
        ("ST_MaxArrayElements", Some(0), "an array's cardinality is its declared type's"),
        ("ST_MaxDescriptionLength", Some(i64::from(DESCRIPTION_LENGTH)), "the characters of a description"),
        ("ST_MaxDoublePrecisionArrayElements", Some(0), "an array's cardinality is its declared type's"),
        (
            "ST_MaxGeometryArrayElements",
            Some(i64::from(crate::geometry::MAX_GEOMETRY_ARRAY_ELEMENTS)),
            "the elements of the array ST_Geometries returns",
        ),
        ("ST_MaxGeometryAsBinary", Some(bytes), "the octets of a well-known binary representation"),
        ("ST_MaxGeometryAsText", Some(bytes), "the characters of a well-known text representation"),
        ("ST_MaxIntegerArrayElements", Some(0), "an array's cardinality is its declared type's"),
        ("ST_MaxKnotArrayElements", Some(0), "the knots of a NURBS curve, limited by the representation's octets"),
        ("ST_MaxNetworkName", Some(network), "the characters of a network's name"),
        ("ST_MaxNURBSPointArrayElements", Some(0), "the control points of a NURBS curve, limited by the representation's octets"),
        ("ST_MaxOrganizationNameLength", Some(i64::from(ORGANIZATION_LENGTH)), "the characters of an organization's name"),
        ("ST_MaxSRSDefinitionLength", Some(i64::from(DEFINITION_LENGTH)), "the characters of a system's well-known text"),
        ("ST_MaxSRSNameLength", Some(i64::from(SRS_NAME_LENGTH)), "the characters of a system's name"),
        ("ST_MaxTypeNameLength", Some(32), "the characters of a type's name ST_GeometryType gives"),
        ("ST_MaxUnitNameLength", Some(i64::from(UNIT_NAME_LENGTH)), "the characters of a unit's name"),
        ("ST_MaxUnitTypeLength", Some(i64::from(UNIT_TYPE_LENGTH)), "the characters of a unit's type"),
        ("ST_MaxVariableNameLength", Some(i64::from(VARIABLE_NAME_LENGTH)), "the characters of a meta-variable's name"),
    ];
    all.extend(UNSUPPORTED.iter().map(|n| (*n, None, "not supported")));
    all.sort_unstable_by_key(|(name, ..)| *name);
    all
}

fn sizings_columns() -> Vec<ColumnDesc> {
    vec![
        text_column(("VARIABLE_NAME", VARIABLE_NAME_LENGTH), true),
        integer("SUPPORTED_VALUE", false),
        text_column(("DESCRIPTION", DESCRIPTION_LENGTH), false),
    ]
}

fn sizings(_seen: &Seen<'_>) -> Vec<Row> {
    sizing_values().into_iter().map(|(name, value, about)| vec![text(name), value.map_or(Value::Null, int), text(about)]).collect()
}

/// The name of the Information Schema view `v`, in its schema:
/// ST_INFORMTN_SCHEMA of one of `SPATIAL_VIEWS`, else INFORMATION_SCHEMA.
pub(crate) fn name_of(v: &SystemView) -> crate::catalog::QualifiedName {
    let spatial = SPATIAL_VIEWS.iter().any(|s| s.name == v.name);
    let auth = if spatial { ST_INFORMTN_SCHEMA } else { super::INFORMATION_SCHEMA };
    crate::catalog::QualifiedName { auth: auth.to_owned(), ident: v.name.to_owned() }
}

// ---------------------------------------------------------------------------
// [§19] The views
// ---------------------------------------------------------------------------

/// [§19.2 to §19.6] The views of ST_INFORMTN_SCHEMA. ST_GEOMETRY_COLUMNS
/// reads the columns the base table lists, not those of the types of
/// INFORMATION_SCHEMA.DIRECT_SUPERTYPES, as the geometry types are built in
/// and have no row there; the subquery of SRS_ID is the standard's.
pub const SPATIAL_VIEWS: &[SystemView] = &[
    SystemView {
        name: "ST_GEOMETRY_COLUMNS",
        query: r"SELECT c.TABLE_CATALOG, c.TABLE_SCHEMA, c.TABLE_NAME, c.COLUMN_NAME, g.SRS_NAME,
         ( SELECT s.SRS_ID FROM ST_DEFINITION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS AS s
            WHERE s.SRS_NAME = g.SRS_NAME ) AS SRS_ID
       FROM INFORMATION_SCHEMA.COLUMNS AS c
            JOIN ST_DEFINITION_SCHEMA.ST_GEOMETRY_COLUMNS AS g ON
              ( c.TABLE_CATALOG = g.TABLE_CATALOG AND c.TABLE_SCHEMA = g.TABLE_SCHEMA AND
                c.TABLE_NAME = g.TABLE_NAME AND c.COLUMN_NAME = g.COLUMN_NAME )",
    },
    SystemView {
        name: "ST_SPATIAL_REFERENCE_SYSTEMS",
        query: r"SELECT SRS_NAME, SRS_ID, ORGANIZATION, ORGANIZATION_COORDSYS_ID, DEFINITION, DESCRIPTION
       FROM ST_DEFINITION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS",
    },
    SystemView {
        name: "ST_UNITS_OF_MEASURE",
        query: r"SELECT UNIT_NAME, UNIT_TYPE, CONVERSION_FACTOR, DESCRIPTION FROM ST_DEFINITION_SCHEMA.ST_UNITS_OF_MEASURE",
    },
    SystemView { name: "ST_SIZINGS", query: r"SELECT VARIABLE_NAME, SUPPORTED_VALUE, DESCRIPTION FROM ST_DEFINITION_SCHEMA.ST_SIZINGS" },
    SystemView {
        name: "GEOMETRY_COLUMNS",
        query: r"SELECT TABLE_CATALOG AS F_TABLE_CATALOG, TABLE_SCHEMA AS F_TABLE_SCHEMA, TABLE_NAME AS F_TABLE_NAME,
         COLUMN_NAME AS F_GEOMETRY_COLUMN, SRS_NAME, SRS_ID AS SRID
       FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS",
    },
    SystemView {
        name: "SPATIAL_REF_SYS",
        query: r"SELECT SRS_NAME, SRS_ID AS SRID, ORGANIZATION AS AUTH_NAME, ORGANIZATION_COORDSYS_ID AS AUTH_ID,
         DEFINITION AS SRTEXT
       FROM ST_DEFINITION_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS",
    },
    SystemView {
        name: "ST_UNITS",
        query: r"SELECT UNIT_NAME, UNIT_TYPE, CONVERSION_FACTOR, DESCRIPTION FROM ST_DEFINITION_SCHEMA.ST_UNITS_OF_MEASURE",
    },
];
