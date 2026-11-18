//! \[EXTENSION\] The views of the Information Schema of Mode C of the
//! property graph implementation standard (§4.3, RULES.md D110), each
//! over its table of the Definition Schema
//! (`definition/graph_types.rs`): GRAPH_TYPES, each graph type, its
//! definition shown to the owner of its schema alone, as VIEWS shows a
//! view's [ISO/IEC 9075:1992 §21.2.7]; and GQL_GRAPHS, each GQL graph the
//! current user or an enabled role holds SELECT on, as PROPERTY_GRAPHS
//! shows it, and its graph type.

use super::SystemView;

/// The views of Mode C, after those of property graphs.
pub(super) const VIEWS: &[SystemView] = &[
    // GQL_GRAPHS.
    SystemView {
        name: "GQL_GRAPHS",
        query: r"SELECT GRAPH_CATALOG, GRAPH_SCHEMA, GRAPH_NAME, GRAPH_TYPE_SCHEMA, GRAPH_TYPE_NAME
     FROM DEFINITION_SCHEMA.GQL_GRAPHS
     WHERE ( GRAPH_CATALOG, GRAPH_SCHEMA, GRAPH_NAME ) IN
             ( SELECT GP.PROPERTY_GRAPH_CATALOG, GP.PROPERTY_GRAPH_SCHEMA, GP.PROPERTY_GRAPH_NAME
               FROM DEFINITION_SCHEMA.PROPERTY_GRAPH_PRIVILEGES AS GP
               WHERE ( GP.GRANTEE IN ( 'PUBLIC', CURRENT_USER )
                    OR GP.GRANTEE IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) ) )
       AND GRAPH_CATALOG = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
    // GRAPH_TYPES.
    SystemView {
        name: "GRAPH_TYPES",
        query: r"SELECT GRAPH_TYPE_CATALOG, GRAPH_TYPE_SCHEMA, GRAPH_TYPE_NAME,
            CASE WHEN ( GRAPH_TYPE_CATALOG, GRAPH_TYPE_SCHEMA )
                        IN ( SELECT CATALOG_NAME, SCHEMA_NAME
                             FROM DEFINITION_SCHEMA.SCHEMATA
                             WHERE ( SCHEMA_OWNER = CURRENT_USER
                                  OR SCHEMA_OWNER IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) ) )
                 THEN GRAPH_TYPE_DEFINITION
                 ELSE NULL
            END AS GRAPH_TYPE_DEFINITION
     FROM DEFINITION_SCHEMA.GRAPH_TYPES
     WHERE GRAPH_TYPE_CATALOG = ( SELECT CATALOG_NAME FROM INFORMATION_SCHEMA_CATALOG_NAME )",
    },
];
