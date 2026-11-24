//! [ISO/IEC 9075-2:2023 §9.18 SR5a, SR6, SR9] The subject routine of a
//! `<call statement>`'s `<routine invocation>`: of the procedures of its
//! name that take its arguments, each parameter past those written, or
//! named by none, of a default value (Features T522 and T523), those of
//! the schema first in the path, and of them the one of fewest
//! parameters, of which a schema holds one at most [SR9b, SR9d to SR9g,
//! §11.60 SR20a] (Feature T341). Of a definition, a procedure
//! body's or a triggered action's, the one it invoked as it was defined
//! [§4.35.2] (RULES.md D73). A function's is chosen by the types of its
//! arguments (`candidates.rs`).
//!
//! A name not qualified is of the schema the statement's names are of,
//! the SQL-path [SR9c] (ID047).

use std::sync::Arc;

use super::super::named::placed;
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
/// CALL of `name` with arguments named by `names`, one each, under the
/// authorization identifier of `cx`: of the procedures of the name, of a
/// definition those it invoked as it was defined, those that take the
/// arguments (`placed`), of them those of the schema first in the path
/// [§9.18 SR9e], and of them the one of fewest parameters [§9.18 SR9f, SR9g]. Its
/// specific name is noted where `cx` notes the routines it invokes.
///
/// # Errors
///
/// SQLCODE -103 if no procedure is of the name \[SR9e\] or none takes the
/// arguments \[SR9b, SR9d\]; -104 if the authorization identifier holds no
/// EXECUTE on the one that does \[SR6\].
pub fn resolve_procedure(cx: Context<'_>, name: &TableName, names: &[Option<String>]) -> Result<Arc<SqlRoutineDesc>> {
    let path = cx.params.routine_path();
    let schemas = if name.auth.is_none() && !path.is_empty() { path.to_vec() } else { vec![qualify(name, cx.schema).auth] };
    let qn = QualifiedName { auth: schemas.first().cloned().unwrap_or_default(), ident: name.ident.clone() };
    let of_schema = |s: &String| cx.db.catalog.routines_named(&QualifiedName { auth: s.clone(), ident: name.ident.clone() });
    let mut named: Vec<&SqlRoutineDesc> = schemas.iter().flat_map(of_schema).filter(|r| r.kind == RoutineKind::Procedure).collect();
    let horizon = if cx.params.horizoned() { cx.params.horizon() } else { cx.horizon };
    if let Some(fixed) = horizon.subjects() {
        named.retain(|r| fixed.contains(&r.specific));
    }
    if named.is_empty() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR9e] there is no procedure {qn}")));
    }
    // [ISO/IEC 9075-4:2006 draft §8.1 SR2] of the schema first in the path.
    let rank = |r: &&SqlRoutineDesc| schemas.iter().position(|s| *s == r.name.auth);
    let fewest = |r: &&SqlRoutineDesc| (rank(r), r.parameters.len());
    let Some(r) = named.iter().copied().filter(|r| takes(r, names)).min_by_key(fewest) else {
        return Err(taken_by_none(&qn, &named, names));
    };
    if !Candidate::Sql(r).executable(cx.db, cx.who()) {
        let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds no EXECUTE on procedure {qn}", cx.auth);
        return Err(SqlError::new(Code::Privilege, msg));
    }
    note(cx.invoked, &r.specific);
    // Shared with the catalog, not copied: a procedure's text and body
    // may be long, and a CALL binds it each time it runs.
    Ok(cx.db.catalog.routine_shared(&r.specific).unwrap_or_else(|| Arc::new(r.clone())))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR9b, SR9d] Whether the procedure `r` takes
/// the arguments `names` names, one each: whether it is a candidate
/// routine of them.
pub(crate) fn takes(r: &SqlRoutineDesc, names: &[Option<String>]) -> bool {
    let shape: Vec<(&str, bool)> = r.parameters.iter().map(|p| (p.name.as_str(), p.default.is_some())).collect();
    placed(&shape, names).is_some()
}

/// [§9.18 SR9b, SR9d] The refusal of a CALL of the arguments `names`
/// names, which none of the procedures `named` of the name `qn` takes.
fn taken_by_none(qn: &QualifiedName, named: &[&SqlRoutineDesc], names: &[Option<String>]) -> SqlError {
    let mut arities: Vec<String> = named.iter().map(|r| r.parameters.len().to_string()).collect();
    arities.sort_unstable();
    arities.dedup();
    let arity = names.len();
    if names.iter().any(Option::is_some) {
        return rule(format!(
            "[ISO/IEC 9075-2:2023 §9.18 SR9d] no procedure {qn} has a parameter of each name the CALL's {arity} arguments name, and a \
             default value of each parameter they do not"
        ));
    }
    rule(format!(
        "[ISO/IEC 9075-2:2023 §9.18 SR9b] no procedure {qn} takes {arity} arguments, each parameter past them of a default value; those \
         of the name have {} parameters",
        arities.join(" or ")
    ))
}
