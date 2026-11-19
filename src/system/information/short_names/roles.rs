//! [ISO/IEC 9075-11:2006 draft §5.77] The short name views of the views of
//! roles: ADMIN_ROLE_AUTHS, ROLE_ROUT_GRANTS and ROL_TAB_METH_GRNTS,
//! which a Conformance Rule of the draft calls ROLE_TAB_METH_GRNTS.

use super::SystemView;

/// The views, in the order of the views they read.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-11:2006 draft §5.77] ADMIN_ROLE_AUTHS.
    SystemView {
        name: "ADMIN_ROLE_AUTHS",
        query: r"SELECT GRANTEE, ROLE_NAME, IS_GRANTABLE
    FROM INFORMATION_SCHEMA.ADMINISTRABLE_ROLE_AUTHORIZATIONS",
    },
    // [ISO/IEC 9075-11:2006 draft §5.77] ROLE_ROUT_GRANTS.
    SystemView {
        name: "ROLE_ROUT_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE, SPECIFIC_CATALOG,
           SPECIFIC_SCHEMA, SPECIFIC_NAME, ROUTINE_CATALOG,
           ROUTINE_SCHEMA, ROUTINE_NAME, PRIVILEGE_TYPE,
           IS_GRANTABLE
    FROM INFORMATION_SCHEMA.ROLE_ROUTINE_GRANTS",
    },
    // [ISO/IEC 9075-11:2006 draft §5.77] ROL_TAB_METH_GRNTS.
    SystemView {
        name: "ROL_TAB_METH_GRNTS",
        query: r"SELECT GRANTOR, GRANTEE, TABLE_CATALOG,
           TABLE_SCHEMA, TABLE_NAME, SPECIFIC_CATALOG,
           SPECIFIC_SCHEMA, SPECIFIC_NAME, IS_GRANTABLE
    FROM INFORMATION_SCHEMA.ROLE_TABLE_METHOD_GRANTS",
    },
];
