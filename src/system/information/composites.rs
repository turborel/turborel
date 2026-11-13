//! The views of the Information Schema of arrays and row types, as
//! ISO/IEC 9075-11 names them: ELEMENT_TYPES, each array type's element
//! type, and FIELDS, each row type's fields. Each reads its table of the
//! Definition Schema (`definition/descriptors.rs`). Their text is this
//! implementation's, as ISO/IEC 9075:1992 has neither. Each shows the
//! descriptors subordinate to those of the tables the current user has a
//! privilege on, as TABLES chooses them [ISO/IEC 9075:1992 §21.2.7], of
//! the domains DOMAINS shows [§21.2.5], and of the user-defined types and
//! routines USER_DEFINED_TYPES and ROUTINES show
//! (`information/user_types.rs`).

use super::SystemView;

/// The views, after those of ISO/IEC 9075:1992.
pub(super) const VIEWS: &[SystemView] = &[
    SystemView {
        name: "ELEMENT_TYPES",
        query: r"SELECT
       E.OBJECT_CATALOG, E.OBJECT_SCHEMA, E.OBJECT_NAME, E.OBJECT_TYPE,
       E.COLLECTION_TYPE_IDENTIFIER, E.DATA_TYPE,
       E.CHARACTER_MAXIMUM_LENGTH, E.CHARACTER_OCTET_LENGTH,
       S.CHARACTER_SET_CATALOG, S.CHARACTER_SET_SCHEMA, S.CHARACTER_SET_NAME,
       E.COLLATION_CATALOG, E.COLLATION_SCHEMA, E.COLLATION_NAME,
       E.NUMERIC_PRECISION, E.NUMERIC_PRECISION_RADIX, E.NUMERIC_SCALE,
       E.DATETIME_PRECISION,
       E.USER_DEFINED_TYPE_CATALOG AS UDT_CATALOG,
       E.USER_DEFINED_TYPE_SCHEMA AS UDT_SCHEMA,
       E.USER_DEFINED_TYPE_NAME AS UDT_NAME,
       E.MAXIMUM_CARDINALITY, E.DTD_IDENTIFIER
     FROM DEFINITION_SCHEMA.ELEMENT_TYPES AS E
         LEFT JOIN
           DEFINITION_SCHEMA.COLLATIONS AS S
           ON
           ( ( S.COLLATION_CATALOG, S.COLLATION_SCHEMA, S.COLLATION_NAME )
           = ( E.COLLATION_CATALOG, E.COLLATION_SCHEMA, E.COLLATION_NAME ) )
         WHERE ( ( E.OBJECT_TYPE = 'TABLE'
                   AND ( E.OBJECT_CATALOG, E.OBJECT_SCHEMA, E.OBJECT_NAME )
                     IN ( SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                            FROM TABLES ) )
                 OR
                 ( E.OBJECT_TYPE = 'DOMAIN'
                   AND ( E.OBJECT_CATALOG, E.OBJECT_SCHEMA, E.OBJECT_NAME )
                     IN ( SELECT DOMAIN_CATALOG, DOMAIN_SCHEMA, DOMAIN_NAME
                            FROM DOMAINS ) )
                 OR
                 ( E.OBJECT_TYPE = 'USER-DEFINED TYPE'
                   AND ( E.OBJECT_CATALOG, E.OBJECT_SCHEMA, E.OBJECT_NAME )
                     IN ( SELECT USER_DEFINED_TYPE_CATALOG,
                                 USER_DEFINED_TYPE_SCHEMA, USER_DEFINED_TYPE_NAME
                            FROM USER_DEFINED_TYPES ) )
                 OR
                 ( E.OBJECT_TYPE = 'ROUTINE'
                   AND ( E.OBJECT_CATALOG, E.OBJECT_SCHEMA, E.OBJECT_NAME )
                     IN ( SELECT SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME
                            FROM ROUTINES ) ) )
         AND E.OBJECT_CATALOG
           = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    SystemView {
        name: "FIELDS",
        query: r"SELECT
       F.OBJECT_CATALOG, F.OBJECT_SCHEMA, F.OBJECT_NAME, F.OBJECT_TYPE,
       F.ROW_IDENTIFIER, F.FIELD_NAME, F.ORDINAL_POSITION, F.DATA_TYPE,
       F.CHARACTER_MAXIMUM_LENGTH, F.CHARACTER_OCTET_LENGTH,
       S.CHARACTER_SET_CATALOG, S.CHARACTER_SET_SCHEMA, S.CHARACTER_SET_NAME,
       F.COLLATION_CATALOG, F.COLLATION_SCHEMA, F.COLLATION_NAME,
       F.NUMERIC_PRECISION, F.NUMERIC_PRECISION_RADIX, F.NUMERIC_SCALE,
       F.DATETIME_PRECISION,
       F.USER_DEFINED_TYPE_CATALOG AS UDT_CATALOG,
       F.USER_DEFINED_TYPE_SCHEMA AS UDT_SCHEMA,
       F.USER_DEFINED_TYPE_NAME AS UDT_NAME,
       F.MAXIMUM_CARDINALITY, F.DTD_IDENTIFIER
     FROM DEFINITION_SCHEMA.FIELDS AS F
         LEFT JOIN
           DEFINITION_SCHEMA.COLLATIONS AS S
           ON
           ( ( S.COLLATION_CATALOG, S.COLLATION_SCHEMA, S.COLLATION_NAME )
           = ( F.COLLATION_CATALOG, F.COLLATION_SCHEMA, F.COLLATION_NAME ) )
         WHERE ( ( F.OBJECT_TYPE = 'TABLE'
                   AND ( F.OBJECT_CATALOG, F.OBJECT_SCHEMA, F.OBJECT_NAME )
                     IN ( SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                            FROM TABLES ) )
                 OR
                 ( F.OBJECT_TYPE = 'DOMAIN'
                   AND ( F.OBJECT_CATALOG, F.OBJECT_SCHEMA, F.OBJECT_NAME )
                     IN ( SELECT DOMAIN_CATALOG, DOMAIN_SCHEMA, DOMAIN_NAME
                            FROM DOMAINS ) )
                 OR
                 ( F.OBJECT_TYPE = 'USER-DEFINED TYPE'
                   AND ( F.OBJECT_CATALOG, F.OBJECT_SCHEMA, F.OBJECT_NAME )
                     IN ( SELECT USER_DEFINED_TYPE_CATALOG,
                                 USER_DEFINED_TYPE_SCHEMA, USER_DEFINED_TYPE_NAME
                            FROM USER_DEFINED_TYPES ) )
                 OR
                 ( F.OBJECT_TYPE = 'ROUTINE'
                   AND ( F.OBJECT_CATALOG, F.OBJECT_SCHEMA, F.OBJECT_NAME )
                     IN ( SELECT SPECIFIC_CATALOG, SPECIFIC_SCHEMA, SPECIFIC_NAME
                            FROM ROUTINES ) ) )
         AND F.OBJECT_CATALOG
           = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
];
