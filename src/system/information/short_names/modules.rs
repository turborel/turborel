//! [ISO/IEC 9075-4:2006 draft §18.8] The short name views of the views of
//! SQL-server modules (Feature P003): MODULE_COL_USAGE and MODULES_S.
//!
//! Two slips of the draft are corrected. MODULE_COL_USAGE has no `<from
//! clause>` there; it reads MODULE_COLUMN_USAGE, the view whose columns
//! it names. MODULES_S selects DEFAULT_SCHEMA, which MODULES calls
//! DEFAULT_SCHEMA_NAME [§18.4]. A Conformance Rule calls MODULE_COL_USAGE
//! MOD_COL_USAGE.

use super::SystemView;

/// The views, in the order of the views they read.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-4:2006 draft §18.8] MODULE_COL_USAGE.
    SystemView {
        name: "MODULE_COL_USAGE",
        query: r"SELECT MODULE_CATALOG, MODULE_SCHEMA, MODULE_NAME,
           TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME,
           COLUMN_NAME
    FROM INFORMATION_SCHEMA.MODULE_COLUMN_USAGE",
    },
    // [ISO/IEC 9075-4:2006 draft §18.8] MODULES_S.
    SystemView {
        name: "MODULES_S",
        query: r"SELECT MODULE_CATALOG, MODULE_SCHEMA, MODULE_NAME,
           DEFAULT_CHARACTER_SET_CATALOG AS DEF_CHAR_SET_CAT,
           DEFAULT_CHARACTER_SET_SCHEMA AS DEF_CHAR_SET_SCH,
           DEFAULT_CHARACTER_SET_NAME AS DEF_CHAR_SET_NAME,
           DEFAULT_SCHEMA_CATALOG AS DEF_SCHEMA_CATALOG,
           DEFAULT_SCHEMA_NAME AS DEFAULT_SCHEMA, MODULE_DEFINITION,
           MODULE_AUTHORIZATION AS MODULE_AUTH, SQL_PATH, MODULE_CREATED AS CREATED,
           MODULE_LAST_ALTERED AS ALTERED
    FROM INFORMATION_SCHEMA.MODULES",
    },
];
