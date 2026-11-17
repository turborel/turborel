//! [ISO/IEC 9075-2:2023 §9.5 SR3j, SR3l] The result type of values of
//! structured types or of reference types brought together: the minimal
//! common supertype of their subtype family (RULES.md D81).

use super::super::rule;
use crate::error::Result;
use crate::value::DataType;
use crate::value::types::StructuredType;

/// The minimal common supertype of `types`, each its name then its proper
/// supertypes, nearest first: of the first, the nearest that each is or
/// has as a supertype; none if they are of no one subtype family.
fn minimal_common(types: &[&StructuredType]) -> Option<StructuredType> {
    let first = types.first()?;
    let chain = std::iter::once(&first.name).chain(&first.supertypes);
    for (i, candidate) in chain.enumerate() {
        if types.iter().all(|t| t.name == *candidate || t.supertypes.contains(candidate)) {
            let supertypes = first.supertypes.get(i..).unwrap_or_default().to_vec();
            return Some(StructuredType { name: candidate.clone(), supertypes });
        }
    }
    None
}

/// [ISO/IEC 9075-2:2023 §9.5 SR3j, SR3l] Of `types`, if any is a reference
/// type or a structured type, their result type: of references, REF of
/// their referenced types' minimal common supertype, of their one scope if
/// each has it [SR3j i]; of structured types, their minimal common
/// supertype. None if none is either. `clause` is the rule to cite.
///
/// # Errors
///
/// SQLCODE -103 if they are not each of one subtype family.
pub(in crate::binder) fn user_defined_set_type(types: &[DataType], clause: &str) -> Option<Result<DataType>> {
    let family =
        |what: &str| rule(format!("[ISO/IEC 9075-2:2023 {clause}, §9.5 {what}] values of no one subtype family are brought together"));
    if types.iter().any(|t| matches!(t, DataType::Reference(_))) {
        let refs: Vec<_> = types.iter().filter_map(|t| if let DataType::Reference(r) = t { Some(r) } else { None }).collect();
        if refs.len() != types.len() {
            return Some(Err(family("SR3j")));
        }
        let referenced: Vec<&StructuredType> = refs.iter().map(|r| r.referenced.as_ref()).collect();
        let Some(common) = minimal_common(&referenced) else { return Some(Err(family("SR3j"))) };
        let scope = refs.first().and_then(|r| r.scope.clone()).filter(|s| refs.iter().all(|r| r.scope.as_ref() == Some(s)));
        return Some(Ok(DataType::reference(common, scope)));
    }
    if !types.iter().any(|t| matches!(t, DataType::Structured(_))) {
        return None;
    }
    let structured: Vec<&StructuredType> = types.iter().filter_map(DataType::structured_type).collect();
    if structured.len() != types.len() {
        return Some(Err(family("SR3l")));
    }
    Some(minimal_common(&structured).map(|t| DataType::Structured(std::sync::Arc::new(t))).ok_or_else(|| family("SR3l")))
}
