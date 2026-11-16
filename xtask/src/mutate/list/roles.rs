//! The mutations of roles (RULES.md R-TEST-9, D75). Of the cell: the
//! closure blind to PUBLIC, a user's granted roles enabled without SET
//! ROLE, the shortcut of no role authorization taken always, SESSION_USER
//! the cell's. Of SET ROLE: a role not granted taken, one taken in a
//! transaction. Of the statements: who creates a role not checked, a cycle
//! granted, a grant without the admin option, the role dependency graph's
//! grantor rule turned, RESTRICT not refusing, the settling of USAGE on a
//! sequence skipped, a dropped role's name kept. Of the log: a definition
//! replayed without its role, the right to create roles replayed as not
//! given. Of the parser: CURRENT_ROLE read as a column. Of the Information
//! Schema: a role granted to PUBLIC not enabled, a null current role
//! enabled, SCHEMATA blind to an enabled role, a role granted to PUBLIC
//! described with no grantee.

use super::{Mutation, Package};

/// The file of roles and the top cell.
const CELL: &str = "src/catalog/roles.rs";
/// The file of SET ROLE and the statements creating and granting roles.
const STATEMENTS: &str = "src/db/roles.rs";
/// The file of REVOKE of roles and DROP ROLE.
const REVOKE: &str = "src/db/roles/revoke.rs";
/// The test of who creates a role.
const CREATED: &str = "roles::the_owner_and_users_it_allows_create_roles_12_4";
/// The test of granting a role.
const GRANTED: &str = "roles::granting_a_role_takes_its_admin_option_12_5";
/// The test of a schema a role owns.
const OWNED: &str = "roles::a_role_owns_a_schema_and_defines_with_its_roles_12_4";
/// The test of a role applicable through another.
const CHAIN: &str = "roles_session::a_role_granted_to_a_role_is_applicable_4_42_4";
/// The test of SET ROLE.
const SET: &str = "roles_session::set_role_enables_a_role_granted_19_3";
/// The test of a trigger of a role's schema.
const TRIGGER: &str = "roles_session::a_trigger_of_a_role_runs_as_the_role_alone_15_21";
/// The test of a CURRENT_ROLE default.
const DEFAULT: &str = "roles_session::current_role_as_a_default_11_5";
/// The test of a privilege a role granted.
const FALLS: &str = "roles_revoke::a_privilege_a_role_granted_falls_with_its_role_12_7";
/// The test of ADMIN OPTION FOR.
const ADMIN: &str = "roles_revoke::admin_option_for_abandons_what_it_granted_12_7";
/// The test of DROP ROLE.
const DROPPED: &str = "roles_revoke::drop_role_takes_every_grant_naming_it_12_6";
/// The test of roles kept.
const KEPT: &str = "roles_revoke::roles_are_kept_as_the_database_opens_again";
/// The unit test of applicable roles.
const APPLICABLE: &str = "catalog::roles::tests::applicable_roles_follow_chains_and_public";
/// The unit test of the enabled identifiers.
const ENABLED: &str = "catalog::roles::tests::enabled_identifiers_of_each_cell";
/// The file of the Information Schema's views of roles.
const VIEWS: &str = "src/system/information/roles.rs";
/// The test of the views of roles.
const ROLE_VIEWS: &str = "role_views::enabled_applicable_and_administrable_roles_5_30";
/// The test of the other views as they read the enabled roles.
const READ: &str = "role_views::views_read_the_enabled_roles_5_61";

/// The mutations of roles.
pub const ROLES: [Mutation; 20] = [
    Mutation {
        name: "roles-closure-without-public",
        package: Package::Core,
        breaks: "a role granted to PUBLIC is applicable for no one",
        file: CELL,
        original: "if to.is_none_or(|g| of.contains(&g) || found.contains(g)) {",
        replacement: "if to.is_some_and(|g| of.contains(&g) || found.contains(g)) {",
        integration: &[],
        unit: &[APPLICABLE],
    },
    Mutation {
        name: "roles-user-enables-granted",
        package: Package::Core,
        breaks: "a user's cell enables the roles granted it without SET ROLE, and a definition a user owns takes them",
        file: CELL,
        original: "let Some(role) = self.current_role() else { return false };",
        replacement: "let role = self.current_role().unwrap_or(self.auth);",
        integration: &[OWNED, SET],
        unit: &[ENABLED],
    },
    Mutation {
        name: "roles-shortcut-always",
        package: Package::Core,
        breaks: "the shortcut of no role authorization is taken always, so no role's privileges are current",
        file: CELL,
        original: "if self.roles.authorizations.is_empty() {",
        replacement: "if self.roles.authorizations.len() < usize::MAX {",
        integration: &[CHAIN, OWNED],
        unit: &[ENABLED],
    },
    Mutation {
        name: "roles-session-user-of-cell",
        package: Package::Core,
        breaks: "SESSION_USER is the top cell's identifier, a trigger's owner's",
        file: CELL,
        original: "UserFunction::Session | UserFunction::System => Some(self.session),",
        replacement: "UserFunction::Session | UserFunction::System => Some(self.cell.auth),",
        integration: &[TRIGGER],
        unit: &[],
    },
    Mutation {
        name: "roles-set-role-ungranted",
        package: Package::Core,
        breaks: "SET ROLE takes a role granted neither to the user nor to PUBLIC",
        file: STATEMENTS,
        original: "Some(name) if roles.is_role(&name) && granted(&name) => {",
        replacement: "Some(name) if roles.is_role(&name) => {",
        integration: &[SET],
        unit: &[],
    },
    Mutation {
        name: "roles-set-role-in-transaction",
        package: Package::Core,
        breaks: "SET ROLE runs in a transaction",
        file: STATEMENTS,
        original: "if self.transaction().is_some() {",
        replacement: "if self.transaction().is_some() && false {",
        integration: &[SET],
        unit: &[],
    },
    Mutation {
        name: "roles-creator-unchecked",
        package: Package::Core,
        breaks: "any user creates a role",
        file: STATEMENTS,
        original: "if !cell.user().is_some_and(|u| st.users.creates_roles(u)) {",
        replacement: "if !cell.user().is_some() {",
        integration: &[CREATED],
        unit: &[],
    },
    Mutation {
        name: "roles-cycle-granted",
        package: Package::Core,
        breaks: "a role is granted to a role applicable for it, a cycle",
        file: STATEMENTS,
        original: "if grantee == role || roles.applicable(&[role]).contains(grantee.as_str()) {",
        replacement: "if grantee == role {",
        integration: &[GRANTED],
        unit: &[],
    },
    Mutation {
        name: "roles-grant-without-admin",
        package: Package::Core,
        breaks: "a role is granted by one not holding it WITH ADMIN OPTION",
        file: STATEMENTS,
        original: "if !roles.administers(a, role) {",
        replacement: "if roles.is_role(a) && !roles.administers(a, role) {",
        integration: &[GRANTED],
        unit: &[],
    },
    Mutation {
        name: "roles-graph-grantor",
        package: Package::Core,
        breaks: "a role authorization depends on one granted to another than its grantor, so what an admin option granted is never abandoned",
        file: REVOKE,
        original: "rd.grantee.as_deref() == Some(d.grantor.as_str())",
        replacement: "rd.grantee.as_deref() != Some(d.grantor.as_str())",
        integration: &[ADMIN],
        unit: &[],
    },
    Mutation {
        name: "roles-restrict-unrefused",
        package: Package::Core,
        breaks: "REVOKE of a role with RESTRICT abandons privileges and objects",
        file: REVOKE,
        original: "refuse_restricted(&revoked, strict)?;",
        replacement: "let _ = strict;",
        integration: &[FALLS],
        unit: &[],
    },
    Mutation {
        name: "roles-drop-keeps-name",
        package: Package::Core,
        breaks: "DROP ROLE keeps the role's name, which no role may take again",
        file: REVOKE,
        original: "names: before.names.iter().filter(|n| Some(n.as_str()) != identified.gone).cloned().collect(),",
        replacement: "names: before.names.clone(),",
        integration: &[DROPPED],
        unit: &[],
    },
    Mutation {
        name: "roles-settle-skips-sequences",
        package: Package::Core,
        breaks: "USAGE on a sequence generator a role granted stands once the role holds it no more",
        file: "src/db/roles/settle.rs",
        original: "retained(&mut s.usage, &|held, a| usage_held(held, Who::sole(roles, a), owner).1);",
        replacement: "let _ = owner;",
        integration: &[FALLS],
        unit: &[],
    },
    Mutation {
        name: "roles-replay-without-role",
        package: Package::Core,
        breaks: "a definition is replayed without the role it was made as, so one in a role's schema fails",
        file: "src/db/ddl.rs",
        original: "self.redefine(((auth, role.as_deref()), schema, *zone), sql, (placed.as_ref(), made, epochs))",
        replacement: "self.redefine(((auth, None), schema, *zone), sql, (placed.as_ref(), made, epochs))",
        integration: &[KEPT],
        unit: &[],
    },
    Mutation {
        name: "roles-creator-replayed-refused",
        package: Package::Core,
        breaks: "the right to create roles is replayed as taken away",
        file: "src/db/recovery.rs",
        original: "Change::RoleCreator { name, allowed } => st.users.replay_creator(name, *allowed),",
        replacement: "Change::RoleCreator { name, allowed } => st.users.replay_creator(name, !*allowed),",
        integration: &[KEPT],
        unit: &[],
    },
    Mutation {
        name: "roles-current-role-a-column",
        package: Package::Core,
        breaks: "CURRENT_ROLE alone is read as a column's name",
        file: "src/parser/literal.rs",
        original: "matches!(&t.tok, Tok::Identifier(w) if w == CURRENT_ROLE)",
        replacement: "matches!(&t.tok, Tok::Identifier(w) if w == \"CURRENT ROLE\")",
        integration: &[SET, DEFAULT],
        unit: &[],
    },
    Mutation {
        name: "roles-views-public-not-enabled",
        package: Package::Core,
        breaks: "ENABLED_ROLES lacks a role granted to PUBLIC, applicable for the current role",
        file: VIEWS,
        original: "WHERE RAD.GRANTEE = 'PUBLIC'",
        replacement: "WHERE RAD.GRANTEE = 'NOBODY'",
        integration: &[ROLE_VIEWS],
        unit: &[],
    },
    Mutation {
        name: "roles-views-null-role-enabled",
        package: Package::Core,
        breaks: "ENABLED_ROLES has a row of a null current role",
        file: VIEWS,
        original: "WHERE C.ROLE_NAME IS NOT NULL",
        replacement: "WHERE 1 = 1",
        integration: &[ROLE_VIEWS],
        unit: &[],
    },
    Mutation {
        name: "roles-views-schemata-blind",
        package: Package::Core,
        breaks: "SCHEMATA shows no schema an enabled role owns",
        file: "src/system/information.rs",
        original: "WHERE ( SCHEMA_OWNER = CURRENT_USER OR SCHEMA_OWNER IN ( SELECT ROLE_NAME FROM ENABLED_ROLES ) )\n         AND CATALOG_NAME",
        replacement: "WHERE SCHEMA_OWNER = CURRENT_USER\n         AND CATALOG_NAME",
        integration: &[READ],
        unit: &[],
    },
    Mutation {
        name: "roles-described-without-public",
        package: Package::Core,
        breaks: "a role granted to PUBLIC is described with an empty grantee, so no view finds it",
        file: "src/system/definition/roles.rs",
        original: "text(a.grantee.as_deref().unwrap_or(PUBLIC))",
        replacement: "text(a.grantee.as_deref().unwrap_or(\"\"))",
        integration: &[ROLE_VIEWS],
        unit: &["system::definition::tests::role_authorizations_are_described"],
    },
];
