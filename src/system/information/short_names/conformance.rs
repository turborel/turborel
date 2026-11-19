//! [ISO/IEC 9075-11:2006 draft §5.77] The short name views of the views of
//! what this implementation supports: SQL_IMPL_INFO and SQL_SIZING_PROFS.
//! The draft defines none of SQL_FEATURES, SQL_PACKAGES, SQL_PARTS or
//! SQL_SIZING.

use super::SystemView;

/// The views, in the draft's order.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-11:2006 draft §5.77] SQL_IMPL_INFO.
    SystemView {
        name: "SQL_IMPL_INFO",
        query: r"SELECT IMPLEMENTATION_INFO_ID AS IMPL_INFO_ID,
           IMPLEMENTATION_INFO_NAME AS IMPL_INFO_NAME, INTEGER_VALUE,
           CHARACTER_VALUE, COMMENTS
    FROM INFORMATION_SCHEMA.SQL_IMPLEMENTATION_INFO",
    },
    // [ISO/IEC 9075-11:2006 draft §5.77] SQL_SIZING_PROFS.
    SystemView {
        name: "SQL_SIZING_PROFS",
        query: r"SELECT SIZING_ID, SIZING_NAME, PROFILE_ID,
           PROFILE_NAME, REQUIRED_VALUE, COMMENTS
    FROM INFORMATION_SCHEMA.SQL_SIZING_PROFILES",
    },
];
