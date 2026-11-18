//! \[EXTENSION\] The views of the Information Schema of node and edge
//! tables (§4.2, RULES.md D109), each over its table of the
//! Definition Schema (`definition/native.rs`): GRAPH_TABLES, whether each
//! table is a node or an edge table; and CONNECTION_CONSTRAINTS, the
//! clauses of the CONNECTION constraints of edge tables. Each shows the
//! tables the current user, PUBLIC or an enabled role holds a privilege
//! on, as TABLES does.

use super::SystemView;

/// The views of node and edge tables, after those of property graphs.
pub(super) const VIEWS: &[SystemView] = &[
    // CONNECTION_CONSTRAINTS.
    SystemView {
        name: "CONNECTION_CONSTRAINTS",
        query: r"SELECT
       CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME,
       TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION,
       FROM_TABLE_CATALOG, FROM_TABLE_SCHEMA, FROM_TABLE_NAME,
       TO_TABLE_CATALOG, TO_TABLE_SCHEMA, TO_TABLE_NAME, DELETE_RULE
     FROM DEFINITION_SCHEMA.CONNECTION_CONSTRAINTS
         WHERE ( TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME )
           IN (
               SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                FROM DEFINITION_SCHEMA.TABLE_PRIVILEGES
                  WHERE ( GRANTEE IN ( 'PUBLIC', CURRENT_USER )
                          OR GRANTEE IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) )
               UNION
               SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                FROM DEFINITION_SCHEMA.COLUMN_PRIVILEGES
                  WHERE ( GRANTEE IN ( 'PUBLIC', CURRENT_USER )
                          OR GRANTEE IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) ) )
         AND TABLE_CATALOG
           = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // GRAPH_TABLES.
    SystemView {
        name: "GRAPH_TABLES",
        query: r"SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, GRAPH_TABLE_TYPE
     FROM DEFINITION_SCHEMA.GRAPH_TABLES
         WHERE ( TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME )
           IN (
               SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                FROM DEFINITION_SCHEMA.TABLE_PRIVILEGES
                  WHERE ( GRANTEE IN ( 'PUBLIC', CURRENT_USER )
                          OR GRANTEE IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) )
               UNION
               SELECT TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME
                FROM DEFINITION_SCHEMA.COLUMN_PRIVILEGES
                  WHERE ( GRANTEE IN ( 'PUBLIC', CURRENT_USER )
                          OR GRANTEE IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) ) )
         AND TABLE_CATALOG
           = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
];
