//! Mutations of registered XML Schemas in the catalog: USAGE on them as
//! an Access Rule checks it, granted and revoked (RULES.md R-TEST-9, D86).

use super::{Mutation, Package};

/// Each mechanism of registered XML Schemas' USAGE broken alone, and the
/// tests that fail.
pub const XML_SCHEMA_CATALOG: [Mutation; 6] = [
    Mutation {
        name: "xml-schema-usage-checked",
        package: Package::Core,
        breaks: "a cell holding no USAGE may use a registered XML Schema",
        file: "src/catalog/xml_schemas/privileges.rs",
        original: "    if desc.usage_of(who, owner_of(catalog, &name.auth)).0 {",
        replacement: "    if true {",
        integration: &[],
        unit: &["catalog::xml_schemas::privileges::tests::usage_is_the_owners_and_the_grantees"],
    },
    Mutation {
        name: "xml-schema-usage-grantable",
        package: Package::Core,
        breaks: "USAGE held without grant option is granted on",
        file: "src/db/usage/xml_schemas.rs",
        original: "        if !grantable {",
        replacement: "        if false && !grantable {",
        integration: &["xml_schema_catalog::usage_is_granted_by_those_holding_it_grantably"],
        unit: &[],
    },
    Mutation {
        name: "xml-schema-usage-abandoned",
        package: Package::Core,
        breaks: "a grant made by a grant option revoked stays",
        file: "src/db/usage/xml_schemas.rs",
        original: "            desc.usage.retain(|u| held.usage_of(sole(&*self.catalog, &u.grantor), owner).1);",
        replacement: "            desc.usage.retain(|_| true);",
        integration: &["xml_schema_catalog::revoke_takes_the_grants_it_abandons"],
        unit: &[],
    },
    Mutation {
        name: "xml-schema-usage-dependents",
        package: Package::Core,
        breaks: "a registered XML Schema importing one whose USAGE its owner lost stays",
        file: "src/db/usage/xml_schemas.rs",
        original: "        let dependents = self.xml_schema_dependents(&qn, &loses);",
        replacement: "        let dependents = self.xml_schema_dependents(&qn, &|_| false);",
        integration: &["xml_schema_catalog::revoke_drops_the_registered_xml_schemas_it_abandons"],
        unit: &[],
    },
    Mutation {
        name: "xml-schema-usage-abandoned-objects",
        package: Package::Core,
        breaks: "a view validating by a registered XML Schema whose USAGE its owner lost stays",
        file: "src/db/usage/xml_schemas.rs",
        original: "        let (dropped, locks) = self.destroy(Arc::from(catalog), &dependents.abandoned)?;",
        replacement: "        let (dropped, locks) = self.destroy(Arc::from(catalog), &super::super::revoke::Abandoned::default())?;",
        integration: &["xml_schema_catalog::revoke_abandons_what_names_a_registered_xml_schema"],
        unit: &[],
    },
    Mutation {
        name: "xml-schema-usage-typed-refused",
        package: Package::Core,
        breaks: "a revoke with CASCADE leaves a column of an XML type its owner may no longer use",
        file: "src/db/usage/xml_schemas.rs",
        original: "    let what = if let Some(t) = dependents.typed.first() {",
        replacement: "    let what = if let Some(t) = None::<&QualifiedName> {",
        integration: &["xml_schema_catalog::revoke_abandons_what_names_a_registered_xml_schema"],
        unit: &[],
    },
];
