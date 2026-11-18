//! The validation root's ID/IDREF table [Schema1 §3.15.5, §3.3.4
//! cvc-id], and identity constraints [Schema1 §3.11.4
//! cvc-identity-constraint], checked over the tree validation made: each
//! constraint's selector and fields evaluated from each element whose
//! declaration has it, the fields' typed values [XQueryDM §3.3.1.2] its
//! key-sequences.

use std::collections::BTreeMap;

use super::super::component::{ConstraintId, ElementId, IdentityCategory, IdentityConstraint, Schema};
use super::super::xpath::{Path, Step as PathStep};
use super::{Failure, Step};
use crate::xml::{Atomic, Kind, NodeId, Tree, Typing, equal};

/// The IDs and IDREFs of what was validated [Schema1 §3.15.5]: how many
/// times each ID came, and each IDREF.
#[derive(Debug, Default)]
pub(super) struct Ids {
    ids: BTreeMap<String, usize>,
    refs: Vec<String>,
}

impl Ids {
    /// ID `value` met.
    pub fn id(&mut self, value: &str) {
        let n = self.ids.entry(value.to_owned()).or_default();
        *n = n.saturating_add(1);
    }

    /// IDREF `value` met.
    pub fn idref(&mut self, value: &str) {
        self.refs.push(value.to_owned());
    }

    /// [Schema1 §3.3.4 cvc-id] Validation Root Valid (ID/IDREF): every
    /// IDREF an ID \[1\], and no ID twice \[2\].
    pub fn check(&self) -> Step<()> {
        if let Some(r) = self.refs.iter().find(|r| !self.ids.contains_key(*r)) {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-id.1]", format!("IDREF '{r}' is no ID")).into());
        }
        if let Some((id, _)) = self.ids.iter().find(|(_, n)| **n > 1) {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-id.2]", format!("ID '{id}' is given twice")).into());
        }
        Ok(())
    }
}

/// A key-sequence [Schema1 §3.11.4 clause 3]: the typed value of each
/// field.
type KeySequence = Vec<Vec<Atomic>>;

/// Whether `a` and `b` are equal member for member, as Equal in [Schema2
/// §4.2.1] compares values.
fn same(a: &KeySequence, b: &KeySequence) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| equal(p, q)))
}

/// [Schema1 §3.11.4 cvc-identity-constraint] Every identity constraint of
/// each element of `tree` validated by a declaration, `decls`, satisfied.
pub(super) fn check(schema: &Schema, tree: &Tree, decls: &[(NodeId, ElementId)]) -> Step<()> {
    for &(element, decl) in decls {
        let Some(d) = schema.element(decl) else { continue };
        for &c in &d.constraints {
            let Some(constraint) = schema.constraint(c) else { continue };
            satisfied(schema, (tree, decls), element, constraint)?;
        }
    }
    Ok(())
}

/// [Schema1 §3.11.4] Whether `element` satisfies `constraint`.
fn satisfied(
    schema: &Schema,
    (tree, decls): (&Tree, &[(NodeId, ElementId)]),
    element: NodeId,
    constraint: &IdentityConstraint,
) -> Step<()> {
    let fail = |clause: &str, why: &str| {
        Failure::new(&format!("[Schema1 §3.11.4 cvc-identity-constraint.{clause}]"), format!("{} {why}", constraint.name.text()))
    };
    let (targets, qualified) = node_sets(schema, (tree, decls), element, constraint)?;
    match constraint.category {
        IdentityCategory::Unique => unique(&qualified).then_some(()).ok_or_else(|| fail("4.1", "is not unique").into()),
        IdentityCategory::Key => {
            if qualified.len() != targets {
                return Err(fail("4.2.1", "has a target with a field missing").into());
            }
            unique(&qualified).then_some(()).ok_or_else(|| fail("4.2.2", "is not unique").into())
        }
        IdentityCategory::KeyRef => {
            let Some(key) = constraint.refer else { return Ok(()) };
            let table = node_table(schema, (tree, decls), element, key)?;
            let Some(table) = table else { return Err(fail("4.3", "refers to a key not in scope").into()) };
            let found = qualified.iter().all(|(_, k)| table.iter().any(|(_, t)| same(k, t)));
            found.then_some(()).ok_or_else(|| fail("4.3", "refers to a key-sequence its key does not have").into())
        }
    }
}

/// Whether no two of `qualified` have equal key-sequences.
fn unique(qualified: &[(NodeId, KeySequence)]) -> bool {
    qualified.iter().enumerate().all(|(i, (_, a))| qualified.iter().skip(i.saturating_add(1)).all(|(_, b)| !same(a, b)))
}

/// [Schema1 §3.11.5] The node table of key or unique `key` at `element`:
/// the qualified nodes of it at `element` and at each element below
/// declared with it, an entry of one key-sequence but two nodes dropped;
/// none if no such element is.
fn node_table(
    schema: &Schema,
    (tree, decls): (&Tree, &[(NodeId, ElementId)]),
    element: NodeId,
    key: ConstraintId,
) -> Step<Option<Vec<(NodeId, KeySequence)>>> {
    let Some(definition) = schema.constraint(key) else { return Ok(None) };
    let end = tree.descendants(element).last().unwrap_or(element);
    let mut table: Option<Vec<(NodeId, KeySequence)>> = None;
    for &(e, d) in decls.iter().filter(|(e, _)| *e >= element && *e <= end) {
        if schema.element(d).is_none_or(|d| !d.constraints.contains(&key)) {
            continue;
        }
        let (_, qualified) = node_sets(schema, (tree, decls), e, definition)?;
        let entries = table.get_or_insert_with(Vec::new);
        for (node, k) in qualified {
            match entries.iter().position(|(n, t)| same(t, &k) && *n != node) {
                Some(i) => {
                    entries.remove(i);
                }
                None => entries.push((node, k)),
            }
        }
    }
    Ok(table)
}

/// The target node set's size, and the qualified node set with each
/// one's key-sequence, of `constraint` at `element` [Schema1 §3.11.4
/// clauses 1 to 4].
fn node_sets(
    schema: &Schema,
    (tree, decls): (&Tree, &[(NodeId, ElementId)]),
    element: NodeId,
    constraint: &IdentityConstraint,
) -> Step<(usize, Vec<(NodeId, KeySequence)>)> {
    let targets = select(tree, element, &constraint.selector);
    if targets.contains(&element) {
        let why = "selects the element itself, not one below it";
        return Err(Failure::new("[Schema1 §3.11.4 cvc-identity-constraint.2]", format!("{} {why}", constraint.name.text())).into());
    }
    let mut qualified = Vec::new();
    for &target in &targets {
        if let Some(k) = key_sequence(schema, (tree, decls), target, constraint)? {
            qualified.push((target, k));
        }
    }
    Ok((targets.len(), qualified))
}

/// The key-sequence of `target`: each field's one node's typed value;
/// none if a field selects no node.
fn key_sequence(
    schema: &Schema,
    (tree, decls): (&Tree, &[(NodeId, ElementId)]),
    target: NodeId,
    constraint: &IdentityConstraint,
) -> Step<Option<KeySequence>> {
    let fail = |clause: &str, why: &str| {
        Failure::new(&format!("[Schema1 §3.11.4 cvc-identity-constraint.{clause}]"), format!("{} {why}", constraint.name.text()))
    };
    let mut out = Vec::new();
    for field in &constraint.fields {
        let nodes = select(tree, target, field);
        let node = match nodes.as_slice() {
            [] => return Ok(None),
            [one] => *one,
            _ => return Err(fail("3", "has a field selecting more than one node").into()),
        };
        let simple = tree.kind(node) == Some(Kind::Attribute)
            || tree.annotation(node).is_some_and(|a| matches!(a.typing, Typing::Atomic(_) | Typing::List(_) | Typing::AnySimple));
        if !simple {
            return Err(fail("3", "has a field selecting a node of no simple type").into());
        }
        let nillable = decls
            .binary_search_by_key(&node, |(n, _)| *n)
            .ok()
            .and_then(|i| decls.get(i))
            .and_then(|(_, d)| schema.element(*d))
            .is_some_and(|d| d.nillable);
        if nillable && constraint.category == IdentityCategory::Key {
            return Err(fail("4.2.3", "has a field validated by a nillable declaration").into());
        }
        out.push(tree.typed_value(node)?);
    }
    Ok(Some(out))
}

/// The nodes `paths` select from `context`, in document order, each once.
fn select(tree: &Tree, context: NodeId, paths: &[Path]) -> Vec<NodeId> {
    let mut out = Vec::new();
    for path in paths {
        let mut at: Vec<NodeId> = if path.descendants {
            std::iter::once(context).chain(tree.descendants(context).filter(|&d| tree.kind(d) == Some(Kind::Element))).collect()
        } else {
            vec![context]
        };
        for step in &path.steps {
            at = match step {
                PathStep::Itself => at,
                PathStep::Child(test) => at
                    .iter()
                    .flat_map(|&e| tree.children(e))
                    .filter(|&c| {
                        tree.kind(c) == Some(Kind::Element) && tree.name(c).is_some_and(|n| test.matches(n.uri.as_deref(), &n.local))
                    })
                    .collect(),
            };
        }
        if let Some(test) = &path.attribute {
            at = at
                .iter()
                .flat_map(|&e| tree.attributes(e))
                .filter(|&a| tree.name(a).is_some_and(|n| test.matches(n.uri.as_deref(), &n.local)))
                .collect();
        }
        out.extend(at);
    }
    out.sort_unstable();
    out.dedup();
    out
}
