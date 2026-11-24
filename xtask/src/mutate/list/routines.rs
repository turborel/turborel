//! The mutations of SQL-invoked routines (RULES.md R-TEST-9). Of a
//! function's invocation: its arguments not stored in their parameters,
//! its result not stored in its type, a null-call function run on a null,
//! a body that returns nothing given a null, its body bound as the caller,
//! EXECUTE unchecked, a modifying function taken in a query and a function
//! not deterministic taken in a CHECK. Of CALL: an output parameter not
//! null at first, its value not assigned to its target, an input's
//! argument not stored, and procedures nested past the limit. Of their
//! definitions: the data access indication unchecked and the owner's
//! EXECUTE grantable whatever the body needs. Of their drops and revokes:
//! DROP ROUTINE RESTRICT not refusing, CASCADE keeping a routine that
//! invokes it, a routine kept past the drop of a table it reads or of a
//! type it is typed by, kept past a revoke of what its body needs or of
//! EXECUTE on a routine it invokes, the owner's EXECUTE lost to a revoke
//! of another's, and routines lost from a checkpoint's snapshot.

use super::{Mutation, Package};

/// The file of a function's invocation, evaluated.
const INVOKED: &str = "src/exec/invoked.rs";
/// The file of CALL.
const CALL: &str = "src/db/routines/call.rs";
/// The file of CREATE and DROP of a routine.
const DEFINE: &str = "src/db/routines/define.rs";
/// The file of a routine's body, bound as it is defined.
const BODY: &str = "src/db/routines/body.rs";
/// The file of EXECUTE on a routine, granted and revoked.
const PRIVILEGES: &str = "src/db/routines/privileges.rs";
/// The test of a CALL's parameters.
const BY_MODE: &str = "routines_call::call_passes_parameters_by_mode_16_1";
/// The test of what a dropped routine takes with it.
const INVOKERS: &str = "routines_drops::what_invokes_a_routine_goes_with_it_11_62";

/// The mutations of SQL-invoked routines.
pub const ROUTINES: [Mutation; 22] = [
    Mutation {
        name: "routines-argument-not-stored",
        package: Package::Core,
        breaks: "an argument too long for its parameter is passed whole, not refused with SQLSTATE 22001",
        file: INVOKED,
        original: "let assigned = assign(arg, t, \"ISO/IEC 9075-2:2023 §9.18 GR4a ii\");",
        replacement: "let assigned: Result<Value> = { let _ = t; Ok(arg) };",
        integration: &["routines::arguments_and_results_are_stored_9_2"],
        unit: &[],
    },
    Mutation {
        name: "routines-result-not-stored",
        package: Package::Core,
        breaks: "a function's value out of its result type's range is returned, not refused with SQLSTATE 22003",
        file: INVOKED,
        original: "assign(returned.unwrap_or(Value::Null), &i.result, \"ISO/IEC 9075-2:2023 §9.18 GR10a iii\")",
        replacement: "Ok(returned.unwrap_or(Value::Null))",
        integration: &["routines::arguments_and_results_are_stored_9_2"],
        unit: &[],
    },
    Mutation {
        name: "routines-null-call-ignored",
        package: Package::Core,
        breaks: "a RETURNS NULL ON NULL INPUT function given a null runs its body",
        file: INVOKED,
        original: "if i.null_call && values.iter().any(Value::is_null) {",
        replacement: "if i.null_call && values.iter().any(Value::is_null) && false {",
        integration: &["routines::a_null_call_function_returns_null_9_18"],
        unit: &[],
    },
    Mutation {
        name: "routines-no-return-lost",
        package: Package::Core,
        breaks: "a function whose body is no RETURN gives a null, not SQLSTATE 2F005",
        file: INVOKED,
        original: "let Some(body) = &i.body else { return Err(no_return(i)) };",
        replacement: "let Some(body) = &i.body else { return Err(no_return(i)).or(Ok(Value::Null)) };",
        integration: &["routines::a_function_that_returns_nothing_raises_2f005"],
        unit: &[],
    },
    Mutation {
        name: "routines-body-as-the-caller",
        package: Package::Core,
        breaks: "a function's body is bound with the caller's privileges, not its owner's",
        file: "src/binder/invocation/body.rs",
        original: "        auth: owner,\n        role: None,",
        replacement: "        auth: cx.auth,\n        role: cx.role,",
        integration: &["routines::execute_is_needed_and_the_body_runs_as_its_owner_12_3"],
        unit: &[],
    },
    Mutation {
        name: "routines-execute-unchecked",
        package: Package::Core,
        breaks: "a routine is invoked by one who holds no EXECUTE on it",
        file: "src/binder/invocation/candidates.rs",
        original: "Candidate::Sql(r) => who.includes(owner_of(&*db.catalog, &r.name.auth)) || held(db, r, who),",
        replacement: "Candidate::Sql(r) => { let _ = (r, db, who); true }",
        integration: &[
            "routines_privileges::execute_is_granted_and_revoked_12_3",
            "routines_call::call_needs_execute_and_runs_as_the_owner_12_3",
        ],
        unit: &[],
    },
    Mutation {
        name: "routines-modifying-in-a-query",
        package: Package::Core,
        breaks: "a function that possibly modifies SQL-data is invoked in a query, a WHERE or a CASE abbreviation",
        file: "src/binder/invocation.rs",
        original: "(self.purpose == Purpose::Query && self.hosting.get())",
        replacement: "(self.purpose == Purpose::Query)",
        integration: &["routines::invocations_and_bodies_are_checked_9_18", "psm_functions::a_function_that_modifies_sql_data_4_35_2"],
        unit: &[],
    },
    Mutation {
        name: "routines-nondeterministic-in-a-check",
        package: Package::Core,
        breaks: "a CHECK invokes a function that is not deterministic",
        file: "src/binder/invocation.rs",
        original: "    if !desc.deterministic {\n        return Err(nondeterministic_source(",
        replacement: "    if !desc.deterministic && false {\n        return Err(nondeterministic_source(",
        integration: &["routines::constraints_take_deterministic_functions_9_16"],
        unit: &[],
    },
    Mutation {
        name: "routines-out-not-null-at-first",
        package: Package::Core,
        breaks: "an output parameter its body sets not is given 7, not the null value",
        file: CALL,
        original: "            outputs.push(cx, (k, p), to)?;\n            Ok(Expr::Const(Value::Null))",
        replacement: "            outputs.push(cx, (k, p), to)?;\n            Ok(Expr::Const(Value::Exact(crate::value::Decimal::new(7, 0))))",
        integration: &[BY_MODE],
        unit: &[],
    },
    Mutation {
        name: "routines-out-not-written-back",
        package: Package::Core,
        breaks: "an output parameter's value is not assigned to its target, which is given the null value",
        file: CALL,
        original: ".map(|(k, _)| out.get(*k).cloned().unwrap_or(Value::Null))",
        replacement: ".map(|(k, _)| out.get(*k).cloned().filter(|_| false).unwrap_or(Value::Null))",
        integration: &[BY_MODE],
        unit: &[],
    },
    Mutation {
        name: "routines-call-argument-not-stored",
        package: Package::Core,
        breaks: "a CALL's argument out of its parameter's range is passed, not refused with SQLSTATE 22003",
        file: CALL,
        original: "values.push(assign(v, &p.data_type, \"ISO/IEC 9075-2:2023 §9.18 GR4c i\").map_err(|e| e.or_about(about()))?);",
        replacement: "values.push({ let _ = about; v });",
        integration: &["routines_call::diagnostics_of_a_call_9_18"],
        unit: &[],
    },
    Mutation {
        name: "routines-nesting-unchecked",
        package: Package::Core,
        breaks: "a chain of seventeen procedures each calling the next runs, past the nesting limit",
        file: "src/db/nested.rs",
        original: "if self.nesting.depth() >= MAX_NESTING {",
        replacement: "if self.nesting.depth() >= MAX_NESTING.saturating_mul(4) {",
        integration: &["routines_call::procedures_nest_at_most_sixteen_deep"],
        unit: &[],
    },
    Mutation {
        name: "routines-access-unchecked",
        package: Package::Core,
        breaks: "a CONTAINS SQL body reads SQL-data, and a READS SQL DATA one modifies it",
        file: BODY,
        original: "within(desc, access)?;",
        replacement: "let _ = within(desc, access);",
        integration: &["routines::the_data_access_indication_is_checked_11_60"],
        unit: &[],
    },
    Mutation {
        name: "routines-execute-always-grantable",
        package: Package::Core,
        breaks: "the owner's EXECUTE is grantable though a privilege its body needs is held not grantable",
        file: DEFINE,
        original: "grantable: bound.grantable }];",
        replacement: "grantable: bound.grantable || true }];",
        integration: &["routines_privileges::the_owners_execute_is_grantable_as_the_body_allows_11_60"],
        unit: &[],
    },
    Mutation {
        name: "routines-drop-restrict-not-refusing",
        package: Package::Core,
        breaks: "DROP ROUTINE RESTRICT drops a routine a view, a CHECK or another routine invokes",
        file: DEFINE,
        original: "if !cascade && !abandoned.is_empty() {",
        replacement: "if !cascade && !abandoned.is_empty() && false {",
        integration: &[INVOKERS],
        unit: &[],
    },
    Mutation {
        name: "routines-cascade-keeps-invokers",
        package: Package::Core,
        breaks: "DROP ROUTINE CASCADE keeps the routines that invoke it",
        file: DEFINE,
        original: "let (gone, modules) = self.module_closure(self.invokers_of(own));",
        replacement: "let (gone, modules) = self.module_closure(own.to_vec());",
        integration: &[INVOKERS],
        unit: &[],
    },
    Mutation {
        name: "routines-kept-past-a-table",
        package: Package::Core,
        breaks: "DROP TABLE RESTRICT drops a table a routine's body reads, the routine kept",
        file: "src/db/routines/dependents.rs",
        original: "let naming = |r: &SqlRoutineDesc| r.reads.iter().find(|n| gone.contains(n)).cloned();",
        replacement: "let naming = |r: &SqlRoutineDesc| r.reads.iter().find(|n| gone.contains(n) && false).cloned();",
        integration: &["routines_drops::a_table_a_body_reads_takes_it_11_31"],
        unit: &[],
    },
    Mutation {
        name: "routines-kept-past-a-type",
        package: Package::Core,
        breaks: "DROP TYPE RESTRICT drops a type a routine's parameter is of, the routine kept",
        file: "src/db/user_types/uses.rs",
        original: "let routines = self.routines_lost_with(&gone);",
        replacement: "let routines: Vec<QualifiedName> = { let _ = self.routines_lost_with(&gone); Vec::new() };",
        integration: &["routines_drops::a_type_a_routine_is_typed_by_takes_it_11_59"],
        unit: &[],
    },
    Mutation {
        name: "routines-revoke-of-execute-not-abandoning",
        package: Package::Core,
        breaks: "REVOKE EXECUTE RESTRICT takes EXECUTE on a routine another routine invokes, the invoker kept",
        file: PRIVILEGES,
        original: "abandoned.routines = routines.filter(|r| gone(after.routine_binds(r))).map(|r| r.specific.clone()).collect();",
        replacement: "abandoned.routines = routines.filter(|r| gone(after.routine_binds(r)) && false).map(|r| r.specific.clone()).collect();",
        integration: &["routines_privileges::a_revoke_of_execute_abandons_an_invoker_12_7"],
        unit: &[],
    },
    Mutation {
        name: "routines-owners-execute-revoked",
        package: Package::Core,
        breaks: "a REVOKE of EXECUTE from another takes the owner's EXECUTE from _SYSTEM too",
        file: PRIVILEGES,
        original: ".retain(|u| u.grantor == SYSTEM_GRANTOR || privilege.of(&*self.catalog, &kept, sole(&*self.catalog, &u.grantor)).1);",
        replacement: ".retain(|u| (u.grantor == SYSTEM_GRANTOR && false) || privilege.of(&*self.catalog, &kept, sole(&*self.catalog, &u.grantor)).1);",
        integration: &[
            "routines_privileges::execute_is_granted_and_revoked_12_3",
            "routines_privileges::all_privileges_on_a_routine_is_execute_12_3",
        ],
        unit: &[],
    },
    Mutation {
        name: "routines-not-in-the-snapshot",
        package: Package::Core,
        breaks: "a checkpoint's snapshot holds no routines, so they are lost on reopening",
        file: "src/db/snapshot.rs",
        original: "routines: st.catalog.routines().into_iter().cloned().collect(),",
        replacement: "routines: Vec::new(),",
        integration: &["routines_drops::routines_are_kept_across_reopening"],
        unit: &["db::snapshot::tests::a_snapshot_reads_back_as_written"],
    },
    Mutation {
        name: "routines-revoke-not-abandoning",
        package: Package::Core,
        breaks: "REVOKE SELECT RESTRICT takes a privilege a routine's body needs, the routine kept",
        file: "src/db/revoke.rs",
        original: "            if lost(bound.routine_binds(r)) {",
        replacement: "            if lost(bound.routine_binds(r)) && false {",
        integration: &["routines_privileges::a_revoke_abandons_a_routine_12_7"],
        unit: &[],
    },
];
