//! Mutations of the Information Schema's short name views: that they are
//! there, and that each column reads its view's column of the same
//! meaning (RULES.md R-TEST-9, D37).

use super::{Mutation, Package};

/// Each short name view's definition broken alone, and the tests that
/// fail.
pub const SHORT_NAMES: [Mutation; 6] = [
    Mutation {
        name: "short-name-views-defined",
        package: Package::Core,
        breaks: "the short name views are not views of the Information Schema",
        file: "src/system/information.rs",
        original: "    let short = short_names::PARTS.as_slice();",
        replacement: "    let short: &[&[SystemView]] = &[];",
        integration: &[
            "short_name_views::short_name_views_of_sql_92_5_77",
            "short_name_views::short_name_views_of_types_and_routines_5_77",
            "short_name_views::short_name_views_of_roles_and_triggers_5_77",
            "short_name_views::short_name_views_of_modules_18_8",
            "short_name_views::short_name_views_of_registered_xml_schemas_20_15",
        ],
        unit: &[],
    },
    Mutation {
        name: "short-name-columns-column",
        package: Package::Core,
        breaks: "COLUMNS_S shows IS_IDENTITY as IS_SELF_REF",
        file: "src/system/information/short_names.rs",
        original: "           IS_SELF_REFERENCING AS IS_SELF_REF, IS_IDENTITY,",
        replacement: "           IS_IDENTITY AS IS_SELF_REF, IS_IDENTITY,",
        integration: &["short_name_views::short_name_views_of_sql_92_5_77"],
        unit: &[],
    },
    Mutation {
        name: "short-name-routines-xml",
        package: Package::Core,
        breaks: "ROUTINES_S shows XML_OPTION as XML_RETURN_MECH",
        file: "src/system/information/short_names/user_types.rs",
        original: "           XML_RETURN_MECHANISM AS XML_RETURN_MECH",
        replacement: "           XML_OPTION AS XML_RETURN_MECH",
        integration: &["short_name_views::short_name_views_of_types_and_routines_5_77"],
        unit: &[],
    },
    Mutation {
        name: "short-name-attributes-corrected",
        package: Package::Core,
        breaks: "ATTRIBUTES_S leaves out ATTRIBUTE_NAME, as the draft's select list does",
        file: "src/system/information/short_names/structured.rs",
        original: "           ATTRIBUTE_NAME, ORDINAL_POSITION, ATTRIBUTE_DEFAULT,",
        replacement: "           ORDINAL_POSITION, ATTRIBUTE_DEFAULT,",
        integration: &["short_name_views::short_name_views_of_types_and_routines_5_77"],
        unit: &[],
    },
    Mutation {
        name: "short-name-modules-schema",
        package: Package::Core,
        breaks: "MODULES_S shows the default schema's catalog as DEFAULT_SCHEMA",
        file: "src/system/information/short_names/modules.rs",
        original: "           DEFAULT_SCHEMA_NAME AS DEFAULT_SCHEMA, MODULE_DEFINITION,",
        replacement: "           DEFAULT_SCHEMA_CATALOG AS DEFAULT_SCHEMA, MODULE_DEFINITION,",
        integration: &["short_name_views::short_name_views_of_modules_18_8"],
        unit: &[],
    },
    Mutation {
        name: "short-name-xml-schemas",
        package: Package::Core,
        breaks: "XML_SCHEMAS_S shows whether a registered XML Schema is deterministic as XML_SCHEMA_IS_PERM",
        file: "src/system/information/short_names/xml_schemas.rs",
        original: "           XML_SCHEMA_IS_PERMANENT AS XML_SCHEMA_IS_PERM",
        replacement: "           XML_SCHEMA_IS_DETERMINISTIC AS XML_SCHEMA_IS_PERM",
        integration: &["short_name_views::short_name_views_of_registered_xml_schemas_20_15"],
        unit: &[],
    },
];
