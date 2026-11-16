//! The views of the Information Schema of roles [ISO/IEC 9075-11:2006
//! draft §5.8, §5.9, §5.30, §5.38 to §5.40, §5.42, §5.43]: the roles
//! enabled and applicable, the role authorizations administrable, and the
//! privileges granted to or by an enabled role. Each reads
//! DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS
//! (`definition/roles.rs`) or a privilege table, and the other views read
//! ENABLED_ROLES as the draft's do, so what a role brings a session is
//! shown as its privileges are current [ISO/IEC 9075-2:2023 §4.42.4].
//!
//! A RECURSIVE view is written as the `<query expression>` it is
//! equivalent to [ISO/IEC 9075-2:2023 §11.32 SR9e], each correction noted.

use super::SystemView;

/// The views of roles, after those of user-defined types.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-11:2006 draft §5.30] ENABLED_ROLES; corrected to
    // ISO/IEC 9075-2:2023 §4.42.4: no row of a null current role, which
    // enables none, and the roles granted to PUBLIC, applicable for the
    // current role.
    SystemView {
        name: "ENABLED_ROLES",
        query: r"WITH RECURSIVE ENABLED_ROLES ( ROLE_NAME ) AS
       ( SELECT C.ROLE_NAME
           FROM ( VALUES ( CURRENT_ROLE ) ) AS C ( ROLE_NAME )
           WHERE C.ROLE_NAME IS NOT NULL
       UNION
         SELECT RAD.ROLE_NAME
           FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS RAD
           WHERE RAD.GRANTEE = 'PUBLIC'
             AND CURRENT_ROLE IS NOT NULL
       UNION
         SELECT RAD.ROLE_NAME
           FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS RAD
             JOIN
                ENABLED_ROLES R
               ON RAD.GRANTEE = R.ROLE_NAME )
     SELECT ROLE_NAME FROM ENABLED_ROLES",
    },
    // [ISO/IEC 9075-11:2006 draft §5.9] APPLICABLE_ROLES.
    SystemView {
        name: "APPLICABLE_ROLES",
        query: r"WITH RECURSIVE APPLICABLE_ROLES ( GRANTEE, ROLE_NAME, IS_GRANTABLE ) AS
       ( ( SELECT GRANTEE, ROLE_NAME, IS_GRANTABLE
           FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS
           WHERE ( GRANTEE IN
                    ( CURRENT_USER, 'PUBLIC' )
                OR
                    GRANTEE IN
                    ( SELECT ROLE_NAME
                      FROM ENABLED_ROLES ) ) )
         UNION
         ( SELECT RAD.GRANTEE, RAD.ROLE_NAME, RAD.IS_GRANTABLE
           FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS RAD
             JOIN
                APPLICABLE_ROLES R
               ON
                   RAD.GRANTEE = R.ROLE_NAME ) )
     SELECT GRANTEE, ROLE_NAME, IS_GRANTABLE FROM APPLICABLE_ROLES",
    },
    // [ISO/IEC 9075-11:2006 draft §5.8] ADMINISTRABLE_ROLE_AUTHORIZATIONS.
    SystemView {
        name: "ADMINISTRABLE_ROLE_AUTHORIZATIONS",
        query: r"SELECT GRANTEE, ROLE_NAME, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.ROLE_AUTHORIZATION_DESCRIPTORS
     WHERE ROLE_NAME IN
           ( SELECT ROLE_NAME
             FROM INFORMATION_SCHEMA.APPLICABLE_ROLES
             WHERE IS_GRANTABLE = 'YES' )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.40] ROLE_TABLE_GRANTS; without
    // WITH_HIERARCHY, of a table of a structured type, there being none, as
    // TABLE_PRIVILEGES is.
    SystemView {
        name: "ROLE_TABLE_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME,
            PRIVILEGE_TYPE, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.TABLE_PRIVILEGES
     WHERE ( GRANTEE IN
              ( SELECT ROLE_NAME
                FROM ENABLED_ROLES )
           OR
              GRANTOR IN
              ( SELECT ROLE_NAME
                FROM ENABLED_ROLES ) )
       AND TABLE_CATALOG
         = ( SELECT CATALOG_NAME
              FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.38] ROLE_COLUMN_GRANTS.
    SystemView {
        name: "ROLE_COLUMN_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME,
            COLUMN_NAME, PRIVILEGE_TYPE, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.COLUMN_PRIVILEGES
     WHERE ( GRANTEE IN
              ( SELECT ROLE_NAME
                FROM ENABLED_ROLES )
           OR
                GRANTOR IN
              ( SELECT ROLE_NAME
                FROM ENABLED_ROLES ) )
       AND TABLE_CATALOG
         = ( SELECT CATALOG_NAME
              FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.42] ROLE_USAGE_GRANTS.
    SystemView {
        name: "ROLE_USAGE_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE,
             OBJECT_CATALOG, OBJECT_SCHEMA, OBJECT_NAME, OBJECT_TYPE,
             'USAGE' AS PRIVILEGE_TYPE, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.USAGE_PRIVILEGES
     WHERE ( GRANTEE IN
               ( SELECT ROLE_NAME
                 FROM ENABLED_ROLES )
            OR
                GRANTOR IN
                ( SELECT ROLE_NAME
                  FROM ENABLED_ROLES ) )
       AND
             OBJECT_CATALOG
           = ( SELECT CATALOG_NAME
                FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.43] ROLE_UDT_GRANTS; corrected: the
    // privileges read from DEFINITION_SCHEMA.UDT_PRIVILEGES, as
    // UDT_PRIVILEGES reads them, whose columns already have the names the
    // draft gives.
    SystemView {
        name: "ROLE_UDT_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE, UDT_CATALOG, UDT_SCHEMA, UDT_NAME,
             PRIVILEGE_TYPE, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.UDT_PRIVILEGES
     WHERE ( GRANTEE IN
               ( SELECT ROLE_NAME
                 FROM ENABLED_ROLES )
            OR
                GRANTOR IN
                ( SELECT ROLE_NAME
                  FROM ENABLED_ROLES ) )
       AND
             UDT_CATALOG
           = ( SELECT CATALOG_NAME
                FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.39] ROLE_ROUTINE_GRANTS; corrected:
    // the source's last subquery closed as "FROM
    // INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )", and the routine's name
    // read from DEFINITION_SCHEMA.ROUTINE_PRIVILEGES, which holds it, as
    // ROUTINE_PRIVILEGES reads it, not joined from ROUTINES.
    SystemView {
        name: "ROLE_ROUTINE_GRANTS",
        query: r"SELECT RP.GRANTOR, RP.GRANTEE,
             RP.SPECIFIC_CATALOG, RP.SPECIFIC_SCHEMA, RP.SPECIFIC_NAME,
             RP.ROUTINE_CATALOG, RP.ROUTINE_SCHEMA, RP.ROUTINE_NAME,
             RP.PRIVILEGE_TYPE, RP.IS_GRANTABLE
     FROM DEFINITION_SCHEMA.ROUTINE_PRIVILEGES AS RP
     WHERE ( RP.GRANTEE IN
               ( SELECT ER.ROLE_NAME
                 FROM ENABLED_ROLES AS ER )
            OR
               RP.GRANTOR IN
               ( SELECT ER.ROLE_NAME
                 FROM ENABLED_ROLES AS ER ) )
       AND
            RP.SPECIFIC_CATALOG
         = ( SELECT ISCN.CATALOG_NAME
              FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
];
