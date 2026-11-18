//! The schema component constraints of a schema read [Schema1 §3.x.6,
//! §5.1]: of particles and model groups [Schema1 §3.8.6, §3.9.6], of
//! complex type definitions and their derivations [Schema1 §3.4.6]
//! (`check/derive.rs`), of particles restricting others [Schema1 §3.9.6]
//! (`check/restrict.rs`), of element declarations' substitution groups
//! [Schema1 §3.3.6] and of attribute groups [Schema1 §3.6.6]; and each
//! content model compiled to its automaton (`schema/automaton.rs`).

mod derive;
mod restrict;

pub use derive::{substitutable, type_derives};
pub use restrict::particle_restricts;

use super::automaton::compile;
use super::builtin;
use super::component::{AttributeUse, ComplexId, Compositor, Expanded, Particle, Schema, Term, TypeDef, Wildcard};
use super::{MAX_DEPTH, beyond, invalid};
use crate::error::Result;

/// `schema`, once it is shown to satisfy every schema component
/// constraint, with the automaton of each content model.
///
/// # Errors
///
/// SQLSTATE 22000 naming the constraint a component breaks; SQLCODE
/// -900 of a content model past `MAX_POSITIONS` or `MAX_STATES`.
pub fn schema(mut schema: Schema) -> Result<Schema> {
    for group in &schema.groups {
        for p in &group.particles {
            particle_correct(p)?;
            if let Term::Group(g) = &p.term
                && schema.group(*g).is_some_and(|inner| inner.compositor == Compositor::All)
            {
                return Err(invalid("[Schema1 §3.8.6 cos-all-limited.1]", "an all group is the whole of a content model"));
            }
        }
    }
    for g in 0..schema.groups.len() {
        not_circular(&schema, super::component::GroupId(g))?;
    }
    for id in (1..schema.complex_types.len()).map(ComplexId) {
        derive::complex_type(&schema, id)?;
        let automaton = match schema.complex(id).and_then(|c| c.content.particle()) {
            Some(p) => {
                particle_correct(p)?;
                consistent(&schema, p)?;
                Some(compile(&schema, p)?)
            }
            None => None,
        };
        if let Some(slot) = schema.automata.get_mut(id.0) {
            *slot = automaton;
        }
    }
    for e in (0..schema.elements.len()).map(super::component::ElementId) {
        derive::substitution(&schema, e)?;
    }
    for group in &schema.attribute_groups {
        attribute_uses_correct(
            &schema,
            &group.attribute_uses,
            "[Schema1 §3.6.6 ag-props-correct.2]",
            "[Schema1 §3.6.6 ag-props-correct.3]",
        )?;
    }
    Ok(schema)
}

/// [Schema1 §3.9.6 p-props-correct.2] Whether `p`'s {min occurs} is at
/// most its {max occurs}, which is at least 1, if bounded.
///
/// # Errors
///
/// SQLSTATE 22000 if not.
fn particle_correct(p: &Particle) -> Result<()> {
    if p.max.is_some_and(|max| max < 1 || p.min > max) {
        return Err(invalid("[Schema1 §3.9.6 p-props-correct.2]", "a particle's minOccurs is at most its maxOccurs, which is at least 1"));
    }
    Ok(())
}

/// [Schema1 §3.8.6 mg-props-correct.2] Whether model group `start` holds,
/// at no depth, a particle whose term is itself.
///
/// # Errors
///
/// SQLSTATE 22000 if it does; SQLCODE -900 if its groups nest deeper than
/// `MAX_DEPTH`.
fn not_circular(schema: &Schema, start: super::component::GroupId) -> Result<()> {
    let mut stack: Vec<(super::component::GroupId, usize)> = vec![(start, 0)];
    let mut steps = 0usize;
    while let Some((g, depth)) = stack.pop() {
        steps = steps.saturating_add(1);
        if depth > MAX_DEPTH || steps > super::MAX_POSITIONS {
            return Err(beyond("its model groups nest too deeply"));
        }
        for p in schema.group(g).map(|m| m.particles.as_slice()).unwrap_or_default() {
            if let Term::Group(inner) = &p.term {
                if *inner == start {
                    return Err(invalid("[Schema1 §3.8.6 mg-props-correct.2]", "a model group holds itself"));
                }
                stack.push((*inner, depth.saturating_add(1)));
            }
        }
    }
    Ok(())
}

/// [Schema1 §3.8.6 cos-element-consistent] Whether the element
/// declaration particles content model `p` holds, directly, indirectly or
/// implicitly, of one name, are all of one named top-level type.
///
/// # Errors
///
/// SQLSTATE 22000 if two of one name are of different types, or one of
/// them of an anonymous type \[1, 2, 3\].
fn consistent(schema: &Schema, p: &Particle) -> Result<()> {
    let mut seen: Vec<(&Expanded, super::component::ElementId, TypeDef)> = Vec::new();
    let mut stack: Vec<(&Particle, usize)> = vec![(p, 0)];
    while let Some((q, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(beyond("its model groups nest too deeply"));
        }
        match &q.term {
            Term::Group(g) => stack.extend(schema.group(*g).into_iter().flat_map(|m| &m.particles).map(|r| (r, depth.saturating_add(1)))),
            Term::Wildcard(_) => {}
            Term::Element(e) => {
                for member in super::automaton::substitution_group(schema, *e) {
                    let Some(decl) = schema.element(member) else { continue };
                    let clash =
                        seen.iter().any(|(n, id, t)| *n == &decl.name && *id != member && !same_named_type(schema, *t, decl.type_def));
                    if clash {
                        return Err(invalid(
                            "[Schema1 §3.8.6 cos-element-consistent]",
                            &format!("elements named {} in one content model are of one named type", decl.name.text()),
                        ));
                    }
                    seen.push((&decl.name, member, decl.type_def));
                }
            }
        }
    }
    Ok(())
}

/// Whether `a` and `b` are one type definition with a name [Schema1
/// §3.8.6 cos-element-consistent.1-3].
fn same_named_type(schema: &Schema, a: TypeDef, b: TypeDef) -> bool {
    a == b && type_name(schema, a).is_some()
}

/// The name of type definition `t`, none if anonymous.
#[must_use]
pub fn type_name(schema: &Schema, t: TypeDef) -> Option<Expanded> {
    match t {
        TypeDef::Simple(s) => Some(builtin::simple_name(s)),
        TypeDef::Complex(c) => schema.complex(c).and_then(|c| c.name.clone()),
    }
}

/// [Schema1 §3.4.6 ct-props-correct.4, 5] [Schema1 §3.6.6
/// ag-props-correct.2, 3] Whether no two of `uses` declare one name, and
/// at most one is of a type that is or is derived from ID.
///
/// # Errors
///
/// SQLSTATE 22000, citing `names` or `ids`, if not.
pub fn attribute_uses_correct(schema: &Schema, uses: &[AttributeUse], names: &str, ids: &str) -> Result<()> {
    let decls: Vec<_> = uses.iter().filter_map(|u| schema.attribute(u.decl)).collect();
    for (i, a) in decls.iter().enumerate() {
        if decls.iter().skip(i.saturating_add(1)).any(|b| b.name == a.name) {
            return Err(invalid(names, &format!("two attribute uses declare {}", a.name.text())));
        }
    }
    if decls.iter().filter(|a| builtin::is_id(a.type_def)).count() > 1 {
        return Err(invalid(ids, "two attribute uses are of ID types"));
    }
    Ok(())
}

/// [Schema1 §3.9.6 cos-group-emptiable] Whether particle `p` is
/// emptiable: its {min occurs} is 0, or its term is a group whose
/// effective total range's minimum is 0.
#[must_use]
pub fn emptiable(schema: &Schema, p: &Particle) -> bool {
    range(schema, p, 0).0 == 0
}

/// [Schema1 §3.8.6 cos-seq-range, cos-choice-range] The effective total
/// range of particle `p`: of an element declaration or wildcard, its own
/// occurrence range; of a group, its range times the sum (all, sequence)
/// or the least and greatest (choice) of its particles' ranges. A group
/// nested past `MAX_DEPTH` counts as emptiable and unbounded.
#[must_use]
pub fn range(schema: &Schema, p: &Particle, depth: usize) -> (u64, Option<u64>) {
    let Term::Group(g) = &p.term else { return (p.min, p.max) };
    let Some(group) = schema.group(*g).filter(|_| depth < MAX_DEPTH) else { return (0, None) };
    let ranges: Vec<(u64, Option<u64>)> = group.particles.iter().map(|q| range(schema, q, depth.saturating_add(1))).collect();
    let (min, max) = match group.compositor {
        Compositor::All | Compositor::Sequence => {
            let min = ranges.iter().fold(0u64, |s, r| s.saturating_add(r.0));
            let max = ranges.iter().try_fold(0u64, |s, r| r.1.map(|m| s.saturating_add(m)));
            (min, max)
        }
        Compositor::Choice => {
            let min = ranges.iter().map(|r| r.0).min().unwrap_or(0);
            let max = ranges.iter().try_fold(0u64, |s, r| r.1.map(|m| s.max(m)));
            (min, max)
        }
    };
    let total_max = match (max, p.max) {
        (Some(0), _) => Some(0),
        (Some(m), Some(n)) => Some(m.saturating_mul(n)),
        (None, _) | (Some(_), None) => None,
    };
    (p.min.saturating_mul(min), total_max)
}

/// [Schema1 §3.4.6 derivation-ok-restriction.2, 3, 4] Whether attribute
/// uses `uses` and wildcard `wildcard` restrict `base_uses` and
/// `base_wildcard`, as a redefined attribute group must [Schema1 §4.2.2
/// src-redefine.7.2.2]; else the clause broken.
///
/// # Errors
///
/// The clause broken, if one is.
pub fn attributes_restrict(
    schema: &Schema,
    (uses, wildcard): (&[AttributeUse], Option<&Wildcard>),
    (base_uses, base_wildcard): (&[AttributeUse], Option<&Wildcard>),
) -> std::result::Result<(), &'static str> {
    derive::uses_restrict(schema, uses, base_uses, base_wildcard)?;
    match (wildcard, base_wildcard) {
        (None, _) => Ok(()),
        (Some(_), None) => Err("derivation-ok-restriction.4.1"),
        (Some(w), Some(b)) => w.constraint.subset_of(&b.constraint).then_some(()).ok_or("derivation-ok-restriction.4.2"),
    }
}
