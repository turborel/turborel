//! The mutations of CHECK conditions calling registered functions
//! (RULES.md R-TEST-9, D74, D133): a call not deterministic taken in a
//! constraint or a generation expression, and ALTER TABLE taking a call
//! its session's SQL.FUNCTION_CALLS refuses.

use super::{Mutation, Package};

/// The test of a registered function not deterministic.
const NONDETERMINISTIC: &str = "check_rebind::a_registered_function_not_deterministic_is_refused_in_a_check";

/// The mutations of CHECK conditions calling registered functions, in the order they run.
pub const CHECK_REBIND: [Mutation; 3] = [
    Mutation {
        name: "check-rebind-constraint-nondeterministic",
        package: Package::Core,
        breaks: "a CHECK or an assertion calls a registered function that is not deterministic",
        file: "src/binder/invocation.rs",
        original: "Purpose::Constraint | Purpose::UnderConstraint => Err(nondeterministic_source(\"am\", &what)),",
        replacement: "Purpose::Constraint | Purpose::UnderConstraint => Ok(()),",
        integration: &[NONDETERMINISTIC],
        unit: &[],
    },
    Mutation {
        name: "check-rebind-generation-nondeterministic",
        package: Package::Core,
        breaks: "a generation expression or a key part calls a registered function that is not deterministic",
        file: "src/binder/invocation.rs",
        original: "Err(rule(format!(\"[ISO/IEC 9075-2:2023 §11.4 SR10d, §9.16 SR1am] a generation expression shall not contain {what}\")))",
        replacement: "Ok(())",
        integration: &[NONDETERMINISTIC],
        unit: &[],
    },
    Mutation {
        name: "check-rebind-alter-takes-calls",
        package: Package::Core,
        breaks: "ALTER TABLE adds a CHECK or a generated column calling a registered function while its session refuses such calls",
        file: "src/db/ddl.rs",
        original: "let _calls = Parser::refusing_calls(text, st.settings.is_on(FUNCTION_CALLS));",
        replacement: "let _calls = Parser::refusing_calls(text, true);",
        integration: &["check_rebind::alter_keeps_geometry_checks_and_a_new_one_needs_the_setting"],
        unit: &[],
    },
];
