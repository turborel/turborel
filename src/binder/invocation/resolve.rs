//! [ISO/IEC 9075-2:2023 §9.18 SR5a, SR6, SR9] The subject routine of a
//! `<call statement>`'s `<routine invocation>`: the procedure of its name
//! of as many parameters as it has arguments, of which a schema holds one
//! at most [§11.60 SR20a] (Feature T341). Of a definition, a procedure
//! body's or a triggered action's, the one it invoked as it was defined
//! [§4.35.2] (RULES.md D73). A function's is chosen by the types of its
//! arguments (`candidates.rs`).
//!
//! A name not qualified is of the schema the statement's names are of,
//! the SQL-path [SR9c] (ID047).

use super::super::{Context, Invoked, qualify, rule};
use super::candidates::Candidate;
use crate::ast::TableName;
use crate::catalog::{QualifiedName, RoutineKind, SqlRoutineDesc};
use crate::error::{Code, Result, SqlError};

/// Notes the specific name `specific` of a routine invoked in `invoked`,
/// if anywhere [§11.60 GR1, §4.35.2].
pub(in crate::binder) fn note(invoked: Option<&Invoked>, specific: &QualifiedName) {
    if let Some(i) = invoked {
        i.borrow_mut().push(specific.clone());
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR5, SR6, SR9] The subject routine of a
/// CALL of `name` with `arity` arguments under the authorization
/// identifier of `cx`: of the procedures of the name, of a definition
/// those it invoked as it was defined, the one of as many parameters (no
/// parameter has a default value). Its specific name is noted where `cx`
/// notes the routines it invokes.
///
/// # Errors
///
/// SQLCODE -103 if no procedure is of the name \[SR9e\] or none takes
/// `arity` arguments \[SR9b\]; -104 if the authorization identifier holds no
/// EXECUTE on the one that does \[SR6\].
pub fn resolve_procedure(cx: Context<'_>, name: &TableName, arity: usize) -> Result<SqlRoutineDesc> {
    let qn = qualify(name, cx.schema);
    let mut named: Vec<&SqlRoutineDesc> =
        cx.db.catalog.routines_named(&qn).into_iter().filter(|r| r.kind == RoutineKind::Procedure).collect();
    let horizon = if cx.params.horizoned() { cx.params.horizon() } else { cx.horizon };
    if let Some(fixed) = horizon.subjects() {
        named.retain(|r| fixed.contains(&r.specific));
    }
    if named.is_empty() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR9e] there is no procedure {qn}")));
    }
    let Some(r) = named.iter().copied().find(|r| r.parameters.len() == arity) else {
        let mut arities: Vec<String> = named.iter().map(|r| r.parameters.len().to_string()).collect();
        arities.sort_unstable();
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.18 SR9b] no procedure {qn} has {arity} parameters; those of the name have {}",
            arities.join(" or ")
        )));
    };
    if !Candidate::Sql(r).executable(cx.db, cx.who()) {
        let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds no EXECUTE on procedure {qn}", cx.auth);
        return Err(SqlError::new(Code::Privilege, msg));
    }
    note(cx.invoked, &r.specific);
    Ok(r.clone())
}
