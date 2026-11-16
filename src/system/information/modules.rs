//! The views of the Information Schema of SQL-server modules [ISO/IEC
//! 9075-4:2006 draft §18.1 to §18.4, §18.6] (Feature P003): MODULES, each
//! module the current user or an enabled role holds EXECUTE on, its
//! definition shown to the owner of its schema alone; MODULE_PRIVILEGES and
//! ROLE_MODULE_GRANTS, each privilege on one granted or granted by them, or
//! an enabled role; and MODULE_TABLE_USAGE and MODULE_COLUMN_USAGE, the
//! tables and columns of the schemas they own that modules reference. Each
//! reads its table of the Definition Schema (`definition/modules.rs`), as
//! the draft defines it.
//!
//! Two slips of the draft are corrected. MODULES shows CREATED and
//! LAST_ALTERED as MODULE_CREATED and MODULE_LAST_ALTERED, the names its
//! Conformance Rules give them. MODULE_TABLE_USAGE names
//! INFORMATION_SCHEMA_CATALOG_NAME ISCN, as its condition does, where the
//! draft's FROM writes ICSN.

use super::SystemView;

/// The views of modules, after those of triggers.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-4:2006 draft §18.1] MODULE_COLUMN_USAGE.
    SystemView {
        name: "MODULE_COLUMN_USAGE",
        query: r"SELECT MCU.MODULE_CATALOG, MCU.MODULE_SCHEMA, MCU.MODULE_NAME,
            MCU.TABLE_CATALOG, MCU.TABLE_SCHEMA, MCU.TABLE_NAME, MCU.COLUMN_NAME
     FROM DEFINITION_SCHEMA.MODULE_COLUMN_USAGE AS MCU
       JOIN
          DEFINITION_SCHEMA.SCHEMATA AS S
       ON ( ( MCU.TABLE_CATALOG, MCU.TABLE_SCHEMA ) =
            ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
          OR
             S.SCHEMA_OWNER IN
             ( SELECT ER.ROLE_NAME
               FROM ENABLED_ROLES AS ER ) )
       AND
           MCU.MODULE_CATALOG =
           ( SELECT ISCN.CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
    // [ISO/IEC 9075-4:2006 draft §18.2] MODULE_PRIVILEGES.
    SystemView {
        name: "MODULE_PRIVILEGES",
        query: r"SELECT
       GRANTOR, GRANTEE, MODULE_CATALOG, MODULE_SCHEMA, MODULE_NAME,
       PRIVILEGE_TYPE, IS_GRANTABLE
     FROM DEFINITION_SCHEMA.MODULE_PRIVILEGES
     WHERE ( GRANTEE IN
             ( 'PUBLIC', CURRENT_USER )
          OR
             GRANTEE IN
               ( SELECT ROLE_NAME
                 FROM ENABLED_ROLES )
          OR
             GRANTOR = CURRENT_USER
          OR
             GRANTOR IN
               ( SELECT ROLE_NAME
                 FROM ENABLED_ROLES ) )
        AND
           MODULE_CATALOG =
           ( SELECT CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-4:2006 draft §18.3] MODULE_TABLE_USAGE.
    SystemView {
        name: "MODULE_TABLE_USAGE",
        query: r"SELECT MTU.MODULE_CATALOG, MTU.MODULE_SCHEMA, MTU.MODULE_NAME,
            MTU.TABLE_CATALOG, MTU.TABLE_SCHEMA, MTU.TABLE_NAME
     FROM DEFINITION_SCHEMA.MODULE_TABLE_USAGE AS MTU
       JOIN
          DEFINITION_SCHEMA.SCHEMATA AS S
       ON ( ( MTU.TABLE_CATALOG, MTU.TABLE_SCHEMA ) =
            ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
          OR
             S.SCHEMA_OWNER IN
             ( SELECT ER.ROLE_NAME
               FROM ENABLED_ROLES AS ER ) )
       AND
           MTU.MODULE_CATALOG =
           ( SELECT ISCN.CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
    // [ISO/IEC 9075-4:2006 draft §18.4] MODULES.
    SystemView {
        name: "MODULES",
        query: r"SELECT MODULE_CATALOG, MODULE_SCHEMA, MODULE_NAME,
            DEFAULT_CHARACTER_SET_CATALOG, DEFAULT_CHARACTER_SET_SCHEMA,
            DEFAULT_CHARACTER_SET_NAME,
            DEFAULT_SCHEMA_CATALOG, DEFAULT_SCHEMA_NAME,
            CASE
              WHEN EXISTS (
                SELECT *
                FROM DEFINITION_SCHEMA.SCHEMATA AS S
                WHERE ( MODULE_CATALOG, MODULE_SCHEMA ) =
                       ( S.CATALOG_NAME, S.SCHEMA_NAME )
                  AND
                       ( S.SCHEMA_OWNER = CURRENT_USER
                    OR
                       S.SCHEMA_OWNER IN
                       ( SELECT ROLE_NAME
                         FROM ENABLED_ROLES ) ) )
              THEN MODULE_DEFINITION
              ELSE NULL
            END AS MODULE_DEFINITION,
            MODULE_AUTHORIZATION, SQL_PATH,
            CREATED AS MODULE_CREATED, LAST_ALTERED AS MODULE_LAST_ALTERED
     FROM DEFINITION_SCHEMA.MODULES
     WHERE ( MODULE_CATALOG, MODULE_SCHEMA, MODULE_NAME ) IN
             ( SELECT MP.MODULE_CATALOG, MP.MODULE_SCHEMA, MP.MODULE_NAME
               FROM DEFINITION_SCHEMA.MODULE_PRIVILEGES AS MP
               WHERE ( MP.GRANTEE IN
                        ( 'PUBLIC', CURRENT_USER )
                   OR
                      MP.GRANTEE IN
                      ( SELECT ROLE_NAME
                        FROM ENABLED_ROLES ) ) )
       AND
           MODULE_CATALOG =
           ( SELECT CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-4:2006 draft §18.6] ROLE_MODULE_GRANTS.
    SystemView {
        name: "ROLE_MODULE_GRANTS",
        query: r"SELECT GRANTOR, GRANTEE, MODULE_CATALOG,
            MODULE_SCHEMA, MODULE_NAME, PRIVILEGE_TYPE,
            IS_GRANTABLE
     FROM DEFINITION_SCHEMA.MODULE_PRIVILEGES
     WHERE ( GRANTEE IN
             ( SELECT ROLE_NAME
               FROM ENABLED_ROLES )
          OR
             GRANTOR IN
             ( SELECT ROLE_NAME
               FROM ENABLED_ROLES ) )
       AND
           MODULE_CATALOG =
           ( SELECT CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
];
