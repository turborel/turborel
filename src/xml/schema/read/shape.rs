//! The structure the Schema for Schemas gives the elements of a schema
//! document [Schema1 §5.1, Appendix A]: the attributes of no namespace
//! each may have, and the order its children of the XML Schema namespace
//! come in, as each XML Representation Summary of [Schema1 §3] shows them.
//!
//! \[DESIGN\] A schema document is checked against this table, not
//! validated against the Schema for Schemas itself: each element's
//! children are a sequence of stages, each a set of names that may occur
//! once or any number of times. The contents of `<appinfo>` and
//! `<documentation>`, which may be anything, are not checked. What a stage
//! cannot say (one of `<restriction>` or `<extension>`, a field at least,
//! a facet's value) the readers of the components check.

use super::super::invalid;
use super::attrs::At;
use super::documents::Doc;
use crate::error::Result;
use crate::xml::{Kind, NodeId};

/// A stage of an element's children: the names that may occur in it, and
/// whether more than one may.
#[derive(Debug, Clone, Copy)]
struct Stage {
    names: &'static [&'static str],
    many: bool,
}

/// What an element may have: its attributes of no namespace, and the
/// stages of its children.
#[derive(Debug, Clone, Copy)]
struct Rule {
    attrs: &'static [&'static str],
    stages: &'static [Stage],
}

/// A stage of names occurring at most once.
const fn once(names: &'static [&'static str]) -> Stage {
    Stage { names, many: false }
}

/// A stage of names occurring any number of times.
const fn many(names: &'static [&'static str]) -> Stage {
    Stage { names, many: true }
}

/// `<annotation>`, first and optional.
const ANNOTATION: Stage = once(&["annotation"]);
/// The attributes and attribute wildcard of a type or attribute group.
const ATTRIBUTES: [Stage; 2] = [many(&["attribute", "attributeGroup"]), once(&["anyAttribute"])];
/// The facets of a simple content restriction [Schema2 §4.3].
const FACETS: Stage = many(&[
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "totalDigits",
    "fractionDigits",
    "length",
    "minLength",
    "maxLength",
    "enumeration",
    "whiteSpace",
    "pattern",
]);
/// The content model of complex content.
const MODEL: Stage = once(&["group", "all", "choice", "sequence"]);
/// The particles of a choice or sequence.
const PARTICLES: Stage = many(&["element", "group", "choice", "sequence", "any"]);
/// The children of `<annotation>` alone.
const ANNOTATION_ONLY: &[Stage] = &[ANNOTATION];

/// Stages of the children of an element of [Schema1 §3].
const STAGES_0: [Stage; 2] = [
    many(&["include", "import", "redefine", "annotation"]),
    many(&["simpleType", "complexType", "group", "attributeGroup", "element", "attribute", "notation", "annotation"]),
];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_1: [Stage; 1] = [many(&["annotation", "simpleType", "complexType", "group", "attributeGroup"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_2: [Stage; 1] = [many(&["appinfo", "documentation"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_3: [Stage; 3] = [ANNOTATION, once(&["simpleType", "complexType"]), many(&["unique", "key", "keyref"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_4: [Stage; 2] = [ANNOTATION, once(&["simpleType"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_5: [Stage; 2] = [ANNOTATION, once(&["restriction", "extension"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_6: [Stage; 3] = [ANNOTATION, ATTRIBUTES[0], ATTRIBUTES[1]];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_7: [Stage; 4] = [ANNOTATION, MODEL, ATTRIBUTES[0], ATTRIBUTES[1]];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_8: [Stage; 2] = [ANNOTATION, once(&["all", "choice", "sequence"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_9: [Stage; 2] = [ANNOTATION, many(&["element"])];
/// Stages of the children of an element of [Schema1 §3].
const STAGES_10: [Stage; 2] = [ANNOTATION, PARTICLES];

/// [Schema1 §3.15.2] `<schema>`.
const SCHEMA: Rule = Rule {
    attrs: &["attributeFormDefault", "blockDefault", "elementFormDefault", "finalDefault", "id", "targetNamespace", "version"],
    stages: &STAGES_0,
};

/// The rule of an XML Schema element `local` whose parent is `parent`,
/// top-level if `parent` is `<schema>` or `<redefine>` [Schema1 §3];
/// none of an element not checked.
fn rule(local: &str, parent: &str) -> Option<Rule> {
    let top = matches!(parent, "schema" | "redefine");
    if let Some(simple) = simple_rule(local, parent, top) {
        return Some(simple);
    }
    let simple_content = parent == "simpleContent";
    Some(match (local, top, simple_content) {
        ("schema", _, _) => SCHEMA,
        ("include", _, _) => Rule { attrs: &["id", "schemaLocation"], stages: ANNOTATION_ONLY },
        ("import", _, _) => Rule { attrs: &["id", "namespace", "schemaLocation"], stages: ANNOTATION_ONLY },
        ("redefine", _, _) => Rule { attrs: &["id", "schemaLocation"], stages: &STAGES_1 },
        ("annotation", _, _) => Rule { attrs: &["id"], stages: &STAGES_2 },
        ("element", true, _) => Rule {
            attrs: &["abstract", "block", "default", "final", "fixed", "id", "name", "nillable", "substitutionGroup", "type"],
            stages: &STAGES_3,
        },
        ("element", _, _) => Rule {
            attrs: &["block", "default", "fixed", "form", "id", "maxOccurs", "minOccurs", "name", "nillable", "ref", "type"],
            stages: &STAGES_3,
        },
        ("attribute", true, _) => Rule { attrs: &["default", "fixed", "id", "name", "type"], stages: &STAGES_4 },
        ("attribute", _, _) => Rule { attrs: &["default", "fixed", "form", "id", "name", "ref", "type", "use"], stages: &STAGES_4 },
        ("complexType", true, _) => Rule { attrs: &["abstract", "block", "final", "id", "mixed", "name"], stages: &COMPLEX },
        ("complexType", _, _) => Rule { attrs: &["id", "mixed"], stages: &COMPLEX },
        ("simpleContent", _, _) => Rule { attrs: &["id"], stages: &STAGES_5 },
        ("complexContent", _, _) => Rule { attrs: &["id", "mixed"], stages: &STAGES_5 },
        ("restriction", _, true) => Rule { attrs: &["base", "id"], stages: &SIMPLE_RESTRICTION },
        ("extension", _, true) => Rule { attrs: &["base", "id"], stages: &STAGES_6 },
        ("restriction", _, _) | ("extension", _, _) => Rule { attrs: &["base", "id"], stages: &STAGES_7 },
        ("attributeGroup", true, _) => Rule { attrs: &["id", "name"], stages: &STAGES_6 },
        ("attributeGroup", _, _) => Rule { attrs: &["id", "ref"], stages: ANNOTATION_ONLY },
        ("group", true, _) => Rule { attrs: &["id", "name"], stages: &STAGES_8 },
        ("group", _, _) => Rule { attrs: &["id", "maxOccurs", "minOccurs", "ref"], stages: ANNOTATION_ONLY },
        ("all", _, _) => Rule { attrs: &["id", "maxOccurs", "minOccurs"], stages: &STAGES_9 },
        ("choice", _, _) | ("sequence", _, _) => Rule { attrs: &["id", "maxOccurs", "minOccurs"], stages: &STAGES_10 },
        ("any", _, _) => Rule { attrs: &["id", "maxOccurs", "minOccurs", "namespace", "processContents"], stages: ANNOTATION_ONLY },
        ("anyAttribute", _, _) => Rule { attrs: &["id", "namespace", "processContents"], stages: ANNOTATION_ONLY },
        ("unique", _, _) | ("key", _, _) => Rule { attrs: &["id", "name"], stages: &IDENTITY },
        ("keyref", _, _) => Rule { attrs: &["id", "name", "refer"], stages: &IDENTITY },
        ("selector", _, _) | ("field", _, _) => Rule { attrs: &["id", "xpath"], stages: ANNOTATION_ONLY },
        ("notation", _, _) => Rule { attrs: &["id", "name", "public", "system"], stages: ANNOTATION_ONLY },
        _ => return None,
    })
}

/// [Schema2 §4.1.2] The children of `<simpleType>`.
const SIMPLE_TYPE: [Stage; 2] = [ANNOTATION, once(&["restriction", "list", "union"])];
/// [Schema2 §4.1.2.1] The children of a `<restriction>` of `<simpleType>`.
const SIMPLE_DERIVATION: [Stage; 3] = [ANNOTATION, once(&["simpleType"]), FACETS];
/// [Schema2 §4.1.2.3] The children of `<union>`.
const UNION: [Stage; 2] = [ANNOTATION, many(&["simpleType"])];

/// [Schema2 §4.1.2, Appendix A] The rule of `<simpleType>`, of the
/// children of its `<restriction>`, `<list>` or `<union>`, and of a facet,
/// `local`, whose parent is `parent`, top-level if `top`; none of another.
/// A local `<simpleType>` has no name and no final.
fn simple_rule(local: &str, parent: &str, top: bool) -> Option<Rule> {
    Some(match (local, parent, top) {
        ("simpleType", _, true) => Rule { attrs: &["final", "id", "name"], stages: &SIMPLE_TYPE },
        ("simpleType", _, false) => Rule { attrs: &["id"], stages: &SIMPLE_TYPE },
        ("restriction", "simpleType", _) => Rule { attrs: &["base", "id"], stages: &SIMPLE_DERIVATION },
        ("list", "simpleType", _) => Rule { attrs: &["id", "itemType"], stages: &STAGES_4 },
        ("union", "simpleType", _) => Rule { attrs: &["id", "memberTypes"], stages: &UNION },
        ("pattern", _, _) | ("enumeration", _, _) => Rule { attrs: &["id", "value"], stages: ANNOTATION_ONLY },
        (facet, _, _) => {
            if !FACETS.names.contains(&facet) {
                return None;
            }
            Rule { attrs: &["fixed", "id", "value"], stages: ANNOTATION_ONLY }
        }
    })
}

/// [Schema1 §3.4.2] The children of `<complexType>`.
const COMPLEX: [Stage; 4] =
    [ANNOTATION, once(&["simpleContent", "complexContent", "group", "all", "choice", "sequence"]), ATTRIBUTES[0], ATTRIBUTES[1]];
/// [Schema1 §3.4.2] The children of a simple content `<restriction>`.
const SIMPLE_RESTRICTION: [Stage; 5] = [ANNOTATION, once(&["simpleType"]), FACETS, ATTRIBUTES[0], ATTRIBUTES[1]];
/// [Schema1 §3.11.2] The children of an identity-constraint definition.
const IDENTITY: [Stage; 3] = [ANNOTATION, once(&["selector"]), many(&["field"])];

/// [Schema1 §5.1] Whether each element of `doc` has the attributes and
/// children its rule allows.
///
/// # Errors
///
/// SQLSTATE 22000 if one has an attribute of no namespace or a child its
/// rule does not allow, or a child out of order.
pub(super) fn check(doc: &Doc) -> Result<()> {
    let tree = &doc.tree;
    let mut stack: Vec<(NodeId, &str)> = vec![(doc.root, "")];
    while let Some((node, parent)) = stack.pop() {
        let at = At::new(tree, node);
        let Some(local) = at.xs_name() else { continue };
        let Some(rule) = rule(local, parent) else { continue };
        for attribute in tree.attributes(node) {
            let unqualified = tree.name(attribute).filter(|n| n.uri.is_none()).map(|n| &*n.local);
            if let Some(name) = unqualified.filter(|n| !rule.attrs.contains(n)) {
                return Err(invalid("[Schema1 Appendix A]", &format!("<{local}> has no attribute {name}")));
            }
        }
        let children = at.children()?;
        stages(local, rule.stages, &children)?;
        let next: Vec<(NodeId, &str)> = children.iter().map(|(c, _)| (c.node, local)).collect();
        stack.extend(next.into_iter().filter(|&(c, _)| tree.kind(c) == Some(Kind::Element)));
    }
    Ok(())
}

/// Whether `children` of `<local>` come in `stages`, in order, each of a
/// stage that is not `many` at most once.
///
/// # Errors
///
/// SQLSTATE 22000 if one does not.
fn stages(local: &str, stages: &[Stage], children: &[(At<'_>, &str)]) -> Result<()> {
    let mut stage = 0usize;
    let mut count = 0usize;
    for (_, child) in children {
        let found = stages.iter().enumerate().skip(stage).find(|(_, s)| s.names.contains(child)).map(|(i, s)| (i, *s));
        let Some((at, s)) = found else {
            return Err(invalid("[Schema1 Appendix A]", &format!("<{local}> has no <{child}> here")));
        };
        if at != stage {
            stage = at;
            count = 0;
        }
        count = count.saturating_add(1);
        if count > 1 && !s.many {
            return Err(invalid("[Schema1 Appendix A]", &format!("<{local}> has one <{child}> at most here")));
        }
    }
    Ok(())
}
