//! \[EXTENSION\] The rules of each object a manifest declares, by its kind
//! (RULES.md D168), that the grammar cannot hold.

use super::{BOUND, Declared, NON_NEGATIVE, ObjectKind, REWRITTEN, SERVED_PARTS, STANDARD_AGGREGATES, SYMMETRIC, TRIANGLE};
use super::{EQUALITY, NEAREST, RANGE};
use crate::error::Result;
use crate::plugin::errors::invalid_manifest;
use crate::vector::Element;

/// The rules of one declaration.
pub(super) fn check_object(d: &Declared) -> Result<()> {
    let named = |what: &str| invalid_manifest(&format!("{} {} {what}", d.kind.word(), d.name));
    if d.batch == Some(0) {
        return Err(named("has a BATCH of 0"));
    }
    match d.kind {
        ObjectKind::Aggregate => {
            if d.parameters.len() != 1 {
                return Err(named("takes one argument"));
            }
            if STANDARD_AGGREGATES.contains(&d.name.as_str()) {
                return Err(named("would replace a standard set function"));
            }
            Ok(())
        }
        ObjectKind::Metric => {
            d.metric_formats()?;
            let known = [SYMMETRIC, NON_NEGATIVE, TRIANGLE];
            match d.properties.iter().find(|p| !known.contains(&p.as_str())) {
                Some(p) => Err(named(&format!("has no property {p}"))),
                None => Ok(()),
            }
        }
        ObjectKind::Embedding => {
            if d.dimensions == Some(0) {
                return Err(named("has no dimensions"));
            }
            match d.element.as_deref().filter(|e| Element::named(e).is_none()) {
                Some(e) => Err(named(&format!("has no element format {e}"))),
                None => Ok(()),
            }
        }
        ObjectKind::IndexMethod => check_supports(d),
        ObjectKind::Rule => check_rule(d),
        ObjectKind::Language => check_language(d),
        ObjectKind::Scalar
        | ObjectKind::Table
        | ObjectKind::Procedure
        | ObjectKind::Authenticator
        | ObjectKind::Target
        | ObjectKind::Partition
        | ObjectKind::Ranking
        | ObjectKind::Quantizer
        | ObjectKind::Collation
        | ObjectKind::SystemTable
        | ObjectKind::XQuery
        | ObjectKind::Type
        | ObjectKind::Engine
        | ObjectKind::Credentials
        | ObjectKind::Diagnostics => Ok(()),
    }
}

/// That an optimizer rule's PHASE is one there is, and its PASSES not 0.
fn check_rule(d: &Declared) -> Result<()> {
    if let Some(p) = d.phase.as_deref().filter(|p| ![BOUND, REWRITTEN].contains(p)) {
        return Err(invalid_manifest(&format!("OPTIMIZER RULE {} has no phase {p}", d.name)));
    }
    if d.passes == Some(0) {
        return Err(invalid_manifest(&format!("OPTIMIZER RULE {} makes no pass", d.name)));
    }
    Ok(())
}

/// That a language SERVES at least one part, each one known, is named as
/// no built-in language is, and is LIKE a built-in one, if written.
fn check_language(d: &Declared) -> Result<()> {
    let built_in = |name: &str| crate::fulltext::Language::all().any(|l| l.name().eq_ignore_ascii_case(name));
    let fault = if let Some(s) = d.supports.iter().find(|s| !SERVED_PARTS.contains(&s.as_str())) {
        format!("serves no part {s}")
    } else if built_in(&d.name) {
        "is named as a built-in language, which wins".to_owned()
    } else if let Some(like) = d.like.as_deref().filter(|like| !built_in(like)) {
        format!("is LIKE {like}, no built-in language")
    } else if d.supports.is_empty() {
        "serves nothing".to_owned()
    } else {
        return Ok(());
    };
    Err(invalid_manifest(&format!("LANGUAGE {} {fault}", d.name)))
}

/// That an index method SUPPORTS at least one search, each one known.
fn check_supports(d: &Declared) -> Result<()> {
    let known = [EQUALITY, RANGE, NEAREST];
    if let Some(s) = d.supports.iter().find(|s| !known.contains(&s.as_str())) {
        return Err(invalid_manifest(&format!("INDEX METHOD {} supports no search {s}", d.name)));
    }
    if d.supports.is_empty() {
        return Err(invalid_manifest(&format!("INDEX METHOD {} supports no search", d.name)));
    }
    Ok(())
}
