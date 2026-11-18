//! Mutations of XQuery's Module Feature: the modules a module import
//! finds, each module's own static context, a module analysed once into a
//! query, the depth imports nest to, and the static errors of library
//! modules (RULES.md R-TEST-9, D106).

use super::{Mutation, Package};

/// Each mechanism of library modules and module imports broken alone, and
/// the tests that fail.
pub const XQUERY_MODULES: [Mutation; 7] = [
    Mutation {
        name: "xquery-module-target-namespace",
        package: Package::Core,
        breaks: "a library module declares a function or variable outside its target namespace",
        file: "src/xquery/analyze/prolog.rs",
        original: "        if name.uri.as_deref() == Some(target) {",
        replacement: "        if true || name.uri.as_deref() == Some(target) {",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::the_static_errors_of_modules"],
    },
    Mutation {
        name: "xquery-module-own-settings",
        package: Package::Core,
        breaks: "a library module's function is evaluated in the static context of the module calling it",
        file: "src/xquery/eval/call.rs",
        original: "        let _within = self.within(function.module);",
        replacement: "",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::each_module_has_its_own_static_context"],
    },
    Mutation {
        name: "xquery-module-statement-namespaces",
        package: Package::Core,
        breaks: "a library module sees the namespaces the SQL statement declares",
        file: "src/xquery/analyze.rs",
        original: "        prefixes.extend(env.namespaces.iter().filter(|_| main).map(|(p, u)| (p.as_str().into(), u.as_str().into())));",
        replacement: "        prefixes.extend(env.namespaces.iter().filter(|_| main || true).map(|(p, u)| (p.as_str().into(), u.as_str().into())));",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::each_module_has_its_own_static_context"],
    },
    Mutation {
        name: "xquery-module-location-hints",
        package: Package::Core,
        breaks: "a module import's location hints choose no module",
        file: "src/xquery/analyze/modules.rs",
        original: "        hints.iter().any(|h| *h == self.location || *h == self.name || *h == self.ident)",
        replacement: "        hints.is_empty() && hints.iter().any(|h| *h == self.location || *h == self.name || *h == self.ident)",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::location_hints_choose_among_modules_of_a_namespace"],
    },
    Mutation {
        name: "xquery-module-analysed-once",
        package: Package::Core,
        breaks: "a module imported by two others is analysed into the query twice",
        file: "src/xquery/analyze/modules.rs",
        original: "        if let Some(exports) = self.linked.exports(&found.name) {",
        replacement: "        if let Some(exports) = self.linked.exports(&found.name).filter(|_| false) {",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::a_module_imported_twice_is_analysed_once"],
    },
    Mutation {
        name: "xquery-module-namespace-imported-twice",
        package: Package::Core,
        breaks: "a prolog imports the modules of one target namespace twice",
        file: "src/xquery/analyze/modules.rs",
        original: "        if self.imported_modules.iter().any(|m| m.namespace() == Some(uri)) {",
        replacement: "        if false && self.imported_modules.iter().any(|m| m.namespace() == Some(uri)) {",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::the_static_errors_of_modules"],
    },
    Mutation {
        name: "xquery-module-import-depth",
        package: Package::Core,
        breaks: "modules import modules without bound",
        file: "src/xquery/analyze/linked.rs",
        original: "        if depth > MAX_IMPORT_DEPTH {",
        replacement: "        if false && depth > MAX_IMPORT_DEPTH {",
        integration: &[],
        unit: &["xquery::analyze::modules::tests::cycles_and_chains_of_imports"],
    },
];
