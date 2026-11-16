//! [ISO/IEC 9075-2:2023 §9.18 SR5a, SR6, SR9] The subject routine of a
//! `<call statement>`'s `<routine invocation>`: the procedure of its name
//! of as many parameters as it has arguments, of which a schema holds one
//! at most [§11.60 SR20a] (Feature T341). Of a definition, a procedure
//! body's or a triggered action's, the one it invoked as it was defined
//! [§4.35.2] (RULES.md D73).
//!
//! A name not qualified is of the schema the statement's names are of,
//! the SQL-path [SR9c] (ID047).

use super::super::{Context, Invoked, qualify, rule};
use crate::ast::TableName;
use crate::catalog::{QualifiedName, RoutineKind, SqlRoutineDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};

/// [ISO/IEC 9075-2:2023 §9.18 SR5a, SR5f] The routines of kind `kind`
/// named `name`: the possibly candidate routines of an invocation of it,
/// by CALL of a procedure, else of a function.
pub(super) fn possibly_candidates<'a>(db: &'a State, name: &QualifiedName, kind: RoutineKind) -> Vec<&'a SqlRoutineDesc> {
    db.catalog.routines_named(name).into_iter().filter(|r| r.kind == kind).collect()
}

/// [ISO/IEC 9075-2:2023 §9.18 SR6, §12.3] Whether `auth` holds EXECUTE on
/// `r` of `db`: the owner of its schema does [§11.60 GR1]; another holds
/// it if it was granted it or PUBLIC.
pub(super) fn holds_execute(db: &State, auth: &str, r: &SqlRoutineDesc) -> bool {
    auth == owner_of(&*db.catalog, &r.name.auth) || r.execute.iter().any(|u| u.grantee.as_deref().is_none_or(|g| g == auth))
}

/// Notes the specific name `specific` of a routine invoked in `invoked`,
/// if anywhere [§11.60 GR1, §4.35.2].
pub(in crate::binder) fn note(invoked: Option<&Invoked>, specific: &QualifiedName) {
    if let Some(i) = invoked {
        i.borrow_mut().push(specific.clone());
    }
}

/// The word for a routine of kind `kind`, in messages.
fn word(kind: RoutineKind) -> &'static str {
    match kind {
        RoutineKind::Function => "function",
        RoutineKind::Procedure => "procedure",
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR6, SR9b, SR10a] Of `named`, the routines
/// of kind `kind` named `name`, the subject routine of an invocation of
/// `arity` arguments under `auth`: the one whose parameters are as many
/// (no parameter has a default value), on which `auth` holds EXECUTE.
///
/// # Errors
///
/// SQLCODE -103 if none takes `arity` arguments; -104 if `auth` holds no
/// EXECUTE on the one that does.
pub(super) fn subject<'a>(
    (db, auth): (&State, &str),
    named: &[&'a SqlRoutineDesc],
    (name, kind): (&QualifiedName, RoutineKind),
    arity: usize,
) -> Result<&'a SqlRoutineDesc> {
    let rule_of = match kind {
        RoutineKind::Function => "§9.18 SR10a",
        RoutineKind::Procedure => "§9.18 SR9b",
    };
    let Some(r) = named.iter().copied().find(|r| r.parameters.len() == arity) else {
        let takes = named.first().map_or(0, |r| r.parameters.len());
        return Err(rule(format!("[ISO/IEC 9075-2:2023 {rule_of}] {} {name} takes {takes} arguments, not {arity}", word(kind))));
    };
    if !holds_execute(db, auth, r) {
        let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {auth} holds no EXECUTE on {} {name}", word(kind));
        return Err(SqlError::new(Code::Privilege, msg));
    }
    Ok(r)
}

/// [ISO/IEC 9075-2:2023 §9.18 SR5, SR6, SR9, SR10] The subject routine of
/// an invocation of `name`, of kind `kind`, with `arity` arguments, under
/// the authorization identifier of `cx`: a CALL's of a procedure, or a
/// function's. Its specific name is noted where `cx` notes the routines
/// it invokes.
///
/// # Errors
///
/// SQLCODE -103 if no routine of the kind is of the name [§9.18 SR9e,
/// SR10h iv] or none takes `arity` arguments [SR9b, SR10a]; -104 if the
/// authorization identifier holds no EXECUTE on it \[SR6\].
pub fn resolve_routine(cx: Context<'_>, name: &TableName, kind: RoutineKind, arity: usize) -> Result<SqlRoutineDesc> {
    let qn = qualify(name, cx.schema);
    let mut named = possibly_candidates(cx.db, &qn, kind);
    let horizon = if cx.params.horizoned() { cx.params.horizon() } else { cx.horizon };
    if let Some(fixed) = horizon.subjects() {
        named.retain(|r| fixed.contains(&r.specific));
    }
    if named.is_empty() {
        let rule_of = match kind {
            RoutineKind::Function => "§9.18 SR10h iv",
            RoutineKind::Procedure => "§9.18 SR9e",
        };
        return Err(rule(format!("[ISO/IEC 9075-2:2023 {rule_of}] there is no {} {qn}", word(kind))));
    }
    let r = subject((cx.db, cx.auth), &named, (&qn, kind), arity)?;
    note(cx.invoked, &r.specific);
    Ok(r.clone())
}
