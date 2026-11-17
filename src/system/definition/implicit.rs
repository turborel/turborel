//! [ISO/IEC 9075-2:2023 §11.51 GR2, GR3, §11.52 GR5, GR6] The routines a
//! user-defined type's definition makes, as ROUTINES, PARAMETERS and
//! ROUTINE_PRIVILEGES show them (`definition/routines.rs`). Of a distinct
//! type, its two functions FNUDT and FNSDT [§11.51 GR2b]. Of a structured
//! type, the observer and mutator of each attribute it defines, SQL-invoked
//! methods [§11.52 GR5, GR6], and its constructor function if it is
//! instantiable [§11.51 GR3c].
//!
//! Each has the characteristics its implicit `<SQL-invoked routine>` gives
//! it or leaves implicit [§11.60 SR]. FNUDT and FNSDT are DETERMINISTIC
//! and their casts AS ASSIGNMENT [§11.51 GR2b]. An observer is
//! DETERMINISTIC and RETURNS NULL ON NULL INPUT, a mutator DETERMINISTIC,
//! CALLED ON NULL INPUT and SELF AS RESULT [§11.51 SR10g], so its SELF is
//! the RESULT parameter of a type-preserving function [§4.9.3.2]. The
//! constructor function is of none, NOT DETERMINISTIC and CALLED ON NULL
//! INPUT, the defaults. Each parameter's name is the one its rule writes,
//! and a method's first parameter is SELF [§11.60 SR6e].

use super::structured::ATTR;
use super::user_types::{functions, signature, specific};
use crate::ast::MethodKind;
use crate::catalog::{QualifiedName, StructuredDesc, UsageDesc};
use crate::system::Seen;
use crate::value::DataType;

/// The ROUTINE_TYPE of an SQL-invoked function that is no method
/// [ISO/IEC 9075-11:2006 draft §6.40 Description 5].
pub(super) const FUNCTION: &str = "FUNCTION";

/// The name of a method's subject parameter [ISO/IEC 9075-2:2023 §11.60
/// SR6e].
pub(super) const SELF: &str = "SELF";

/// A routine a type's definition makes.
pub(super) struct Implicit<'a> {
    /// Its specific name.
    pub(super) specific: QualifiedName,
    /// Its routine name.
    pub(super) name: QualifiedName,
    /// Of a method, its type.
    pub(super) method_of: Option<&'a QualifiedName>,
    /// Its ROUTINE_TYPE.
    pub(super) kind: String,
    /// Each parameter's name and type, and whether it is the RESULT
    /// parameter of a type-preserving function.
    pub(super) parameters: Vec<(&'static str, DataType, bool)>,
    /// Its result type.
    pub(super) result: DataType,
    /// DETERMINISTIC.
    pub(super) deterministic: bool,
    /// RETURNS NULL ON NULL INPUT.
    pub(super) null_call: bool,
    /// Of a user-defined cast function, whether it is implicitly
    /// invocable, AS ASSIGNMENT.
    pub(super) cast: Option<bool>,
    /// The type's schema, whose owner holds EXECUTE on it.
    pub(super) schema: &'a str,
    /// EXECUTE on it: each grant.
    pub(super) execute: &'a [UsageDesc],
}

/// The ROUTINE_TYPE of a method of kind `kind` [ISO/IEC 9075-11:2006 draft
/// §6.40 Description 5]: INSTANCE, STATIC or CONSTRUCTOR METHOD.
pub(super) fn method_type(kind: MethodKind) -> String {
    format!("{} METHOD", kind.name())
}

/// Every routine the types' definitions make: each distinct type's two
/// functions, then each structured type's.
pub(super) fn implicit<'a>(seen: &'a Seen<'_>) -> Vec<Implicit<'a>> {
    let mut all = Vec::new();
    for t in seen.state.catalog.user_types() {
        for f in functions(t) {
            let (parameter, source, result) = signature(f);
            all.push(Implicit {
                specific: specific(f),
                name: QualifiedName { auth: t.name.auth.clone(), ident: t.routine(f.1).name.clone() },
                method_of: None,
                kind: FUNCTION.to_owned(),
                parameters: vec![(parameter, source, false)],
                result,
                deterministic: true,
                null_call: false,
                cast: Some(true),
                schema: &t.name.auth,
                execute: &t.routine(f.1).execute,
            });
        }
    }
    for s in seen.state.catalog.structured_types() {
        structured(s, &mut all);
    }
    all
}

/// The routines of the structured type `s`: the observer and mutator of
/// each attribute it defines, then its constructor function.
fn structured<'a>(s: &'a StructuredDesc, all: &mut Vec<Implicit<'a>>) {
    let named = |ident: &str| QualifiedName { auth: s.name.auth.clone(), ident: ident.to_owned() };
    let method = |r: &'a crate::catalog::RoutineDesc, parameters, result| Implicit {
        specific: named(&r.specific),
        name: named(&r.name),
        method_of: Some(&s.name),
        kind: method_type(MethodKind::Instance),
        parameters,
        result,
        deterministic: true,
        null_call: false,
        cast: None,
        schema: &s.name.auth,
        execute: &r.execute,
    };
    for a in s.attributes.iter().filter(|a| a.defined_by == s.name) {
        let observer = method(&a.observer, vec![(SELF, s.data_type(), false)], a.data_type.clone());
        all.push(Implicit { null_call: true, ..observer });
        let mutator = vec![(SELF, s.data_type(), true), (ATTR, a.data_type.clone(), false)];
        all.push(method(&a.mutator, mutator, s.data_type()));
    }
    if let Some(c) = &s.constructor {
        let constructor = method(c, Vec::new(), s.data_type());
        all.push(Implicit { method_of: None, kind: FUNCTION.to_owned(), deterministic: false, ..constructor });
    }
}
