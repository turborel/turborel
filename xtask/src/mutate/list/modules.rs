//! The mutations of SQL-server modules (RULES.md R-TEST-9): the privilege
//! that invokes a module's routines, the module's path, a drop taking the
//! whole module, the owner's grant option, the declared tables kept out of
//! the Information Schema, and a subquery's identity, which keeps two
//! functions' rows apart.

use super::{Mutation, Package};

/// The mutations of modules, in the order they run.
pub const MODULES: [Mutation; 6] = [
    Mutation {
        name: "module-execute-ignored",
        package: Package::Core,
        breaks: "a routine of a module is invoked without EXECUTE on its module [ISO/IEC 9075-4:2006 draft §8.1 SR1]",
        file: "src/binder/invocation/candidates.rs",
        original: "        Some(m) => db.catalog.module(&m).is_some_and(|m| m.execute_of(who).0),",
        replacement: "        Some(_) => true,",
        integration: &["modules_privileges::execute_on_the_module_invokes_its_routines_8_1"],
        unit: &[],
    },
    Mutation {
        name: "module-path-order-ignored",
        package: Package::Core,
        breaks: "a function a module's routine names is of the last schema of its path, not the first [ISO/IEC 9075-4:2006 draft §9.21 SR6]",
        file: "src/binder/invocation/candidates.rs",
        original: "    let Some(first) = subjects.iter().filter_map(rank).min() else { return subjects };",
        replacement: "    let Some(first) = subjects.iter().filter_map(rank).max() else { return subjects };",
        integration: &["modules::the_module_path_qualifies_routine_names_9_21"],
        unit: &[],
    },
    Mutation {
        name: "module-drop-partial",
        package: Package::Core,
        breaks: "a drop of a routine a module's routine invokes leaves the module's other routines [ISO/IEC 9075-4:2006 draft §10.3 GR7, §9.2 GR2a]",
        file: "src/db/modules.rs",
        original: "                gone.extend(routines.filter(|s| !gone.contains(s)).collect::<Vec<_>>());",
        replacement: "                gone.extend(routines.filter(|_| false).collect::<Vec<_>>());",
        integration: &["modules::drop_module_restrict_and_cascade_9_22"],
        unit: &[],
    },
    Mutation {
        name: "module-grant-never-upgraded",
        package: Package::Core,
        breaks: "the owner's EXECUTE on a module stays ungrantable once it holds grantably what its routines need [ISO/IEC 9075-4:2006 draft §10.1 GR1]",
        file: "src/db/definitions.rs",
        original: "                self.module_grants_upgraded();",
        replacement: "",
        integration: &["modules_privileges::the_owner_s_execute_becomes_grantable_10_1"],
        unit: &[],
    },
    Mutation {
        name: "module-tables-shown",
        package: Package::Core,
        breaks: "a module's declared local temporary tables are shown in the Information Schema [ISO/IEC 9075:1992 §13.11 GR5]",
        file: "src/db/declared.rs",
        original: "        let declared = |t: &&TableDesc| t.name.auth == kw::MODULE || module_of_schema(&t.name.auth).is_some();",
        replacement: "        let declared = |t: &&TableDesc| t.name.auth == kw::MODULE || module_of_schema(&t.name.auth).is_some_and(|_| false);",
        integration: &["modules::a_module_s_declared_tables_are_each_session_s_12_8"],
        unit: &[],
    },
    Mutation {
        name: "subquery-identity-shared",
        package: Package::Core,
        breaks: "subqueries of different plans share an identity, so a runner gives one the rows it kept of another [DESIGN]",
        file: "src/logical/subquery.rs",
        original: "        Subquery { plan, outer, id: NEXT_ID.fetch_add(1, Ordering::Relaxed) }",
        replacement: "        Subquery { plan, outer, id: NEXT_ID.load(Ordering::Relaxed) }",
        integration: &["routines::functions_of_subquery_bodies_give_their_own_values"],
        unit: &[],
    },
];
