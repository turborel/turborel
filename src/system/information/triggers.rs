//! The views of the Information Schema of triggers [ISO/IEC 9075-11:2006
//! draft §5.64 to §5.69]: TRIGGERS, each trigger on a table the current
//! user or an enabled role holds a privilege but SELECT on, its condition
//! and statement shown to the owner of its schema alone;
//! TRIGGERED_UPDATE_COLUMNS, each column of a trigger column list, of a
//! column so held; and TRIGGER_TABLE_USAGE, TRIGGER_COLUMN_USAGE,
//! TRIGGER_ROUTINE_USAGE and TRIGGER_SEQUENCE_USAGE, what the triggers of
//! the schemas the current user or an enabled role owns name. Each reads
//! its table of the Definition Schema (`definition/triggers.rs`), as the
//! draft defines it.

use super::SystemView;

/// The views of triggers, after those of roles.
pub(super) const VIEWS: &[SystemView] = &[
    // [ISO/IEC 9075-11:2006 draft §5.69] TRIGGERS.
    SystemView {
        name: "TRIGGERS",
        query: r"SELECT TRIGGER_CATALOG, TRIGGER_SCHEMA, TRIGGER_NAME,
            EVENT_MANIPULATION,
            EVENT_OBJECT_CATALOG, EVENT_OBJECT_SCHEMA, EVENT_OBJECT_TABLE,
            ACTION_ORDER,
            CASE
              WHEN EXISTS
                 ( SELECT *
                   FROM DEFINITION_SCHEMA.SCHEMATA AS S
                   WHERE ( TRIGGER_CATALOG, TRIGGER_SCHEMA )
                       = ( S.CATALOG_NAME, S.SCHEMA_NAME )
                     AND
                          ( S.SCHEMA_OWNER = CURRENT_USER
                         OR
                            S.SCHEMA_OWNER IN
                            ( SELECT ROLE_NAME
                              FROM ENABLED_ROLES ) ) )
              THEN ACTION_CONDITION
              ELSE NULL
            END AS ACTION_CONDITION,
            CASE
              WHEN EXISTS
                 ( SELECT *
                   FROM DEFINITION_SCHEMA.SCHEMATA AS S
                   WHERE ( TRIGGER_CATALOG, TRIGGER_SCHEMA )
                       = ( S.CATALOG_NAME, S.SCHEMA_NAME )
                     AND
                          ( S.SCHEMA_OWNER = CURRENT_USER
                         OR
                            S.SCHEMA_OWNER IN
                            ( SELECT ROLE_NAME
                              FROM ENABLED_ROLES ) ) )
              THEN ACTION_STATEMENT
              ELSE NULL
            END AS ACTION_STATEMENT,
            ACTION_ORIENTATION, ACTION_TIMING,
            ACTION_REFERENCE_OLD_TABLE, ACTION_REFERENCE_NEW_TABLE,
            ACTION_REFERENCE_OLD_ROW, ACTION_REFERENCE_NEW_ROW,
            CREATED
     FROM DEFINITION_SCHEMA.TRIGGERS
     WHERE ( EVENT_OBJECT_CATALOG, EVENT_OBJECT_SCHEMA, EVENT_OBJECT_TABLE ) IN
             ( SELECT TP.TABLE_CATALOG, TP.TABLE_SCHEMA, TP.TABLE_NAME
               FROM DEFINITION_SCHEMA.TABLE_PRIVILEGES AS TP
               WHERE TP.PRIVILEGE_TYPE <> 'SELECT'
                  AND
                        ( TP.GRANTEE IN
                          ( 'PUBLIC', CURRENT_USER )
                       OR
                          TP.GRANTEE IN
                          ( SELECT ROLE_NAME
                            FROM ENABLED_ROLES ) )
              UNION
                SELECT CP.TABLE_CATALOG, CP.TABLE_SCHEMA, CP.TABLE_NAME
                FROM DEFINITION_SCHEMA.COLUMN_PRIVILEGES AS CP
                 WHERE CP.PRIVILEGE_TYPE <> 'SELECT'
                   AND
                       ( CP.GRANTEE IN
                         ( 'PUBLIC', CURRENT_USER )
                      OR
                         CP.GRANTEE IN
                         ( SELECT ROLE_NAME
                           FROM ENABLED_ROLES ) ) )
       AND
            TRIGGER_CATALOG
          = ( SELECT CATALOG_NAME
              FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.64] TRIGGERED_UPDATE_COLUMNS.
    SystemView {
        name: "TRIGGERED_UPDATE_COLUMNS",
        query: r"SELECT TRIGGER_CATALOG, TRIGGER_SCHEMA, TRIGGER_NAME,
            EVENT_OBJECT_CATALOG, EVENT_OBJECT_SCHEMA, EVENT_OBJECT_TABLE,
            EVENT_OBJECT_COLUMN
     FROM DEFINITION_SCHEMA.TRIGGERED_UPDATE_COLUMNS
     WHERE ( EVENT_OBJECT_CATALOG, EVENT_OBJECT_SCHEMA,
             EVENT_OBJECT_TABLE, EVENT_OBJECT_COLUMN ) IN
             ( SELECT CP.TABLE_CATALOG, CP.TABLE_SCHEMA,
                       CP.TABLE_NAME, CP.COLUMN_NAME
               FROM DEFINITION_SCHEMA.COLUMN_PRIVILEGES AS CP
               WHERE CP.PRIVILEGE_TYPE <> 'SELECT'
                 AND
                     ( CP.GRANTEE IN
                        ( 'PUBLIC', CURRENT_USER )
                    OR
                        CP.GRANTEE IN
                        ( SELECT ROLE_NAME
                          FROM ENABLED_ROLES ) ) )
       AND
           TRIGGER_CATALOG
         = ( SELECT CATALOG_NAME
             FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.68] TRIGGER_TABLE_USAGE.
    SystemView {
        name: "TRIGGER_TABLE_USAGE",
        query: r"SELECT TTU.TRIGGER_CATALOG, TTU.TRIGGER_SCHEMA, TTU.TRIGGER_NAME,
             TTU.TABLE_CATALOG, TTU.TABLE_SCHEMA, TTU.TABLE_NAME
     FROM DEFINITION_SCHEMA.TRIGGER_TABLE_USAGE AS TTU
       JOIN
           DEFINITION_SCHEMA.SCHEMATA AS S
         ON ( ( TTU.TRIGGER_CATALOG, TTU.TRIGGER_SCHEMA )
             = ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
            OR
               S.SCHEMA_OWNER IN
               ( SELECT ER.ROLE_NAME
                 FROM ENABLED_ROLES AS ER ) )
       AND
            TTU.TRIGGER_CATALOG
         = ( SELECT ISCN.CATALOG_NAME
               FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.65] TRIGGER_COLUMN_USAGE.
    SystemView {
        name: "TRIGGER_COLUMN_USAGE",
        query: r"SELECT TCU.TRIGGER_CATALOG, TCU.TRIGGER_SCHEMA, TCU.TRIGGER_NAME,
             TCU.TABLE_CATALOG, TCU.TABLE_SCHEMA, TCU.TABLE_NAME, TCU.COLUMN_NAME
     FROM DEFINITION_SCHEMA.TRIGGER_COLUMN_USAGE AS TCU
       JOIN
          DEFINITION_SCHEMA.SCHEMATA AS S
         ON ( ( TCU.TRIGGER_CATALOG, TCU.TRIGGER_SCHEMA )
             = ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
           OR
              S.SCHEMA_OWNER IN
              ( SELECT ER.ROLE_NAME
                FROM ENABLED_ROLES AS ER ) )
       AND TCU.TRIGGER_CATALOG
            = ( SELECT ISCN.CATALOG_NAME
                FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.66] TRIGGER_ROUTINE_USAGE.
    SystemView {
        name: "TRIGGER_ROUTINE_USAGE",
        query: r"SELECT TRU.TRIGGER_CATALOG, TRU.TRIGGER_SCHEMA, TRU.TRIGGER_NAME,
             TRU.SPECIFIC_CATALOG, TRU.SPECIFIC_SCHEMA, TRU.SPECIFIC_NAME
     FROM DEFINITION_SCHEMA.TRIGGER_ROUTINE_USAGE AS TRU
       JOIN
           DEFINITION_SCHEMA.SCHEMATA AS S
         ON ( ( TRU.TRIGGER_CATALOG, TRU.TRIGGER_SCHEMA )
             = ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
            OR
               S.SCHEMA_OWNER IN
               ( SELECT ER.ROLE_NAME
                 FROM ENABLED_ROLES AS ER ) )
       AND
            TRU.SPECIFIC_CATALOG
         = ( SELECT ISCN.CATALOG_NAME
               FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
    // [ISO/IEC 9075-11:2006 draft §5.67] TRIGGER_SEQUENCE_USAGE.
    SystemView {
        name: "TRIGGER_SEQUENCE_USAGE",
        query: r"SELECT TSU.TRIGGER_CATALOG, TSU.TRIGGER_SCHEMA, TSU.TRIGGER_NAME,
             TSU.SEQUENCE_CATALOG, TSU.SEQUENCE_SCHEMA, TSU.SEQUENCE_NAME
     FROM DEFINITION_SCHEMA.TRIGGER_SEQUENCE_USAGE AS TSU
       JOIN
           DEFINITION_SCHEMA.SCHEMATA AS S
         ON ( ( TSU.TRIGGER_CATALOG, TSU.TRIGGER_SCHEMA )
             = ( S.CATALOG_NAME, S.SCHEMA_NAME ) )
     WHERE ( S.SCHEMA_OWNER = CURRENT_USER
          OR
              S.SCHEMA_OWNER IN
              ( SELECT ER.ROLE_NAME
                FROM ENABLED_ROLES AS ER ) )
       AND
            TSU.SEQUENCE_CATALOG
         = ( SELECT ISCN.CATALOG_NAME
              FROM INFORMATION_SCHEMA_CATALOG_NAME AS ISCN )",
    },
];
