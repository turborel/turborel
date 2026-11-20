//! The mutations of ALTER TABLE's column nullability, data type and table
//! RENAME TO (RULES.md R-TEST-9, D399 to D402): the rows SET NOT NULL
//! adds a constraint of left unchecked, a primary key's column let drop
//! NOT NULL, a shorter string taken as wider, the copy firing triggers,
//! a routine's body read only by its counting, a view that names a table
//! renamed passed over, and the generation a rename gives left out of its
//! log record.

use super::super::{Mutation, Package};

/// Mechanisms whose removal must fail the requirements of the forms.
pub const ALTERED_TABLES: [Mutation; 7] = [
    Mutation {
        name: "set-not-null-unchecked",
        package: Package::Core,
        breaks: "SET NOT NULL adds its constraint without checking the rows",
        file: "src/db/ddl.rs",
        original: "                None => validate_rows(&next, tx, qn, self.effects(&next)),\n            },\n            // [§11.12 GR1]",
        replacement: "                None => Ok(()),\n            },\n            // [§11.12 GR1]",
        integration: &["alter_columns::set_not_null_checks_the_rows_15"],
        unit: &[],
    },
    Mutation {
        name: "drop-not-null-key",
        package: Package::Core,
        breaks: "DROP NOT NULL is allowed of a column of the primary key",
        file: "src/db/alter/nullability.rs",
        original: "        if desc.primary_key().is_some_and(|k| k.contains(&at)) {",
        replacement: "        if desc.primary_key().is_some_and(|k| k.contains(&at) && false) {",
        integration: &["alter_columns::drop_not_null_and_its_refusals_16"],
        unit: &[],
    },
    Mutation {
        name: "retype-shorter-string",
        package: Package::Core,
        breaks: "SET DATA TYPE takes a shorter string as a wider one",
        file: "src/db/alter/retype/widening.rs",
        original: "        let kept = characters(d).is_some_and(|(ld, u)| u == units && ld >= ldtc);",
        replacement: "        let kept = characters(d).is_some_and(|(_, u)| u == units);",
        integration: &["alter_columns::set_data_type_refuses_what_narrows_19"],
        unit: &["db::alter::retype::widening::tests::strings_widen_to_longer_strings_of_their_kind"],
    },
    Mutation {
        name: "copy-fires-triggers",
        package: Package::Core,
        breaks: "the rows an ALTER TABLE copies fire the table's insert triggers",
        file: "src/db/ddl/copied.rs",
        original: "    effects.triggered = None;",
        replacement: "",
        integration: &["renamed_tables::a_table_renamed_keeps_all_but_its_name"],
        unit: &[],
    },
    Mutation {
        name: "retype-routine-counted",
        package: Package::Core,
        breaks: "SET DATA TYPE reads a routine's body only by its counting, missing a column it names",
        file: "src/db/alter/retype.rs",
        original: "            if super::rename::names_any(&r.body, [column, column], false) || self.routine_usage_columns(r)?.iter().any(is_it) {",
        replacement: "            if self.routine_usage_columns(r)?.iter().any(is_it) {",
        integration: &["alter_columns::set_data_type_refuses_what_references_the_column"],
        unit: &[],
    },
    Mutation {
        name: "rename-view-passed",
        package: Package::Core,
        breaks: "RENAME TO passes over a view that names the table",
        file: "src/db/alter/renamed/texts.rs",
        original: "        for v in self.catalog.views().into_iter().filter(|v| v.reads.contains(qn)) {",
        replacement: "        for v in self.catalog.views().into_iter().filter(|v| v.reads.contains(qn) && false) {",
        integration: &["renamed_tables::a_rename_waits_for_the_texts_that_name_the_table"],
        unit: &[],
    },
    Mutation {
        name: "rename-generation-unlogged",
        package: Package::Core,
        breaks: "a rename's log record leaves out the generation it gives the table",
        file: "src/db/alter/renamed.rs",
        original: "    Some(format!(\"RENAME TO {}.{} {GENERATION} {generation}\", super::rename::quoted(&to.auth), super::rename::quoted(&to.ident)))",
        replacement: "    let _ = generation;\n    Some(format!(\"RENAME TO {}.{}\", super::rename::quoted(&to.auth), super::rename::quoted(&to.ident)))",
        integration: &["renamed_tables::renames_are_kept_by_a_reopen", "renamed_tables::renames_are_recovered_after_a_crash"],
        unit: &[],
    },
];
