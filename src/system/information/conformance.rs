//! [ISO/IEC 9075-11:2006 draft §5.52 to §5.57] The views of what this
//! implementation supports: SQL_FEATURES, SQL_IMPLEMENTATION_INFO,
//! SQL_PACKAGES, SQL_PARTS, SQL_SIZING and SQL_SIZING_PROFILES, as the
//! draft defines them, over the Definition Schema's tables of them
//! (`definition/conformance.rs`). Their short name views, SQL_IMPL_INFO
//! and SQL_SIZING_PROFS [§5.77], are `short_names.rs`'s.

use super::SystemView;

/// The views, in the draft's order.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-11:2006 draft §5.52] SQL_FEATURES.
    SystemView {
        name: "SQL_FEATURES",
        query: r"SELECT ID AS FEATURE_ID, NAME AS FEATURE_NAME, SUB_ID AS SUB_FEATURE_ID,
           SUB_NAME AS SUB_FEATURE_NAME, IS_SUPPORTED, IS_VERIFIED_BY, COMMENTS
    FROM DEFINITION_SCHEMA.SQL_CONFORMANCE
    WHERE TYPE IN
          ( 'FEATURE', 'SUBFEATURE' )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.53] SQL_IMPLEMENTATION_INFO.
    SystemView {
        name: "SQL_IMPLEMENTATION_INFO",
        query: r"SELECT IMPLEMENTATION_INFO_ID, IMPLEMENTATION_INFO_NAME,
           INTEGER_VALUE, CHARACTER_VALUE, COMMENTS
    FROM DEFINITION_SCHEMA.SQL_IMPLEMENTATION_INFO",
    },
    // [ISO/IEC 9075-11:2006 draft §5.54] SQL_PACKAGES.
    SystemView {
        name: "SQL_PACKAGES",
        query: r"SELECT ID, NAME, IS_SUPPORTED, IS_VERIFIED_BY, COMMENTS
    FROM DEFINITION_SCHEMA.SQL_CONFORMANCE
    WHERE TYPE = 'PACKAGE'",
    },
    // [ISO/IEC 9075-11:2006 draft §5.55] SQL_PARTS.
    SystemView {
        name: "SQL_PARTS",
        query: r"SELECT ID AS PART, NAME, IS_SUPPORTED, IS_VERIFIED_BY, COMMENTS
    FROM DEFINITION_SCHEMA.SQL_CONFORMANCE
    WHERE TYPE = 'PART'",
    },
    // [ISO/IEC 9075-11:2006 draft §5.56] SQL_SIZING.
    SystemView {
        name: "SQL_SIZING",
        query: r"SELECT SIZING_ID, SIZING_NAME, SUPPORTED_VALUE, COMMENTS
    FROM DEFINITION_SCHEMA.SQL_SIZING",
    },
    // [ISO/IEC 9075-11:2006 draft §5.57] SQL_SIZING_PROFILES.
    SystemView {
        name: "SQL_SIZING_PROFILES",
        query: r"SELECT SIZING_ID, SS.SIZING_NAME, SSP.PROFILE_ID,
           SSP.PROFILE_NAME, SSP.REQUIRED_VALUE, SSP.COMMENTS
    FROM DEFINITION_SCHEMA.SQL_SIZING_PROFILES AS SSP
      JOIN
         DEFINITION_SCHEMA.SQL_SIZING AS SS
      USING ( SIZING_ID )",
    },
];
