//! Particle Valid (Restriction) [Schema1 §3.9.6 cos-particle-restrict]:
//! whether a particle restricts another, after the heads of substitution
//! groups are read as choices of their members \[2.1\] and pointless
//! groups are dropped \[2.2\], by the case its kind and the base's name in
//! the table of [Schema1 §3.9.6].

use super::super::automaton::substitution_group;
use super::super::component::{Compositor, Derivations, ElementId, Particle, Schema, Scope, Term, ValueKind, Wildcard};
use super::super::{MAX_DEPTH, invalid};
use super::type_derives;
use crate::error::Result;

/// The verdict of a check: the constraint broken, if one is.
type Verdict = std::result::Result<(), String>;

/// A particle read for the check: its occurrence range and kind.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    min: u64,
    max: Option<u64>,
    kind: Kind,
}

/// The kind of a `Node`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// An element declaration particle.
    Elt(ElementId),
    /// A wildcard particle.
    Any(Wildcard),
    /// A group particle.
    Group(Compositor, Vec<Node>),
}

/// Whether particle `r` is a valid restriction of particle `b` [Schema1
/// §3.9.6 cos-particle-restrict].
///
/// # Errors
///
/// The name of the case broken, if `r` does not restrict `b`; also if
/// their groups nest past `MAX_DEPTH`.
pub fn particle_restricts(schema: &Schema, r: &Particle, b: &Particle) -> Verdict {
    if r == b {
        return Ok(());
    }
    let read = |p| top(schema, p).map_err(|e| e.to_string());
    match (read(r)?, read(b)?) {
        (Some(r), Some(b)) => valid(schema, &r, &b, 0),
        (None, b) => {
            b.is_none_or(|b| emptiable(&b)).then_some(()).ok_or_else(|| "cos-particle-restrict.2: the base is not emptiable".into())
        }
        (Some(_), None) => Err("cos-particle-restrict.2: nothing is restricted to content".into()),
    }
}

/// `p` read for the check, with its pointless groups dropped; none if
/// nothing is left.
fn top(schema: &Schema, p: &Particle) -> Result<Option<Node>> {
    let mut nodes = node(schema, p, None, 0)?;
    Ok(match nodes.len() {
        1 => nodes.pop(),
        0 => None,
        _ => Some(Node { min: 1, max: Some(1), kind: Kind::Group(Compositor::Sequence, nodes) }),
    })
}

/// `p`, within a group of `parent` if any, read for the check: what is
/// left of it once pointless groups are dropped [Schema1 §3.9.6 clause
/// 2.2], and a substitution group's head as a choice of its members
/// \[2.1\].
fn node(schema: &Schema, p: &Particle, parent: Option<Compositor>, depth: usize) -> Result<Vec<Node>> {
    if depth > MAX_DEPTH {
        return Err(super::super::beyond("its model groups nest too deeply"));
    }
    let kind = match &p.term {
        Term::Wildcard(w) => Kind::Any(w.clone()),
        Term::Element(e) => {
            let members = substitution_group(schema, *e);
            if members.len() > 1 && schema.element(*e).is_some_and(|d| d.scope == Scope::Global) {
                let each = members.into_iter().map(|m| Node { min: 1, max: Some(1), kind: Kind::Elt(m) }).collect();
                Kind::Group(Compositor::Choice, each)
            } else {
                Kind::Elt(*e)
            }
        }
        Term::Group(g) => {
            let group = schema.group(*g).ok_or_else(|| invalid("[Schema1 §3.8.1]", "a model group is missing"))?;
            let mut children = Vec::new();
            for q in &group.particles {
                children.extend(node(schema, q, Some(group.compositor), depth.saturating_add(1))?);
            }
            if pointless(p, group.compositor, children.len(), parent) {
                return Ok(children);
            }
            Kind::Group(group.compositor, children)
        }
    };
    Ok(vec![Node { min: p.min, max: p.max, kind }])
}

/// [Schema1 §3.9.6 clause 2.2] Whether a group particle `p` of
/// `compositor` with `count` particles, within a group of `parent`, is
/// pointless.
fn pointless(p: &Particle, compositor: Compositor, count: usize, parent: Option<Compositor>) -> bool {
    let once = p.min == 1 && p.max == Some(1);
    match compositor {
        Compositor::Sequence => count == 0 || (once && (count == 1 || parent == Some(Compositor::Sequence))),
        Compositor::All => count <= 1,
        Compositor::Choice => (count == 0 && p.min == 0) || (once && (count == 1 || parent == Some(Compositor::Choice))),
    }
}

/// [Schema1 §3.9.6 range-ok] Whether `r`'s occurrence range is within
/// `b`'s.
fn range_ok(r: (u64, Option<u64>), b: (u64, Option<u64>)) -> bool {
    r.0 >= b.0 && b.1.is_none_or(|bm| r.1.is_some_and(|rm| rm <= bm))
}

/// The effective total range of `n` [Schema1 §3.8.6 cos-seq-range,
/// cos-choice-range].
fn range(n: &Node) -> (u64, Option<u64>) {
    let Kind::Group(c, children) = &n.kind else { return (n.min, n.max) };
    let ranges: Vec<(u64, Option<u64>)> = children.iter().map(range).collect();
    let (min, max) = match c {
        Compositor::All | Compositor::Sequence => {
            (ranges.iter().fold(0u64, |s, r| s.saturating_add(r.0)), ranges.iter().try_fold(0u64, |s, r| r.1.map(|m| s.saturating_add(m))))
        }
        Compositor::Choice => (ranges.iter().map(|r| r.0).min().unwrap_or(0), ranges.iter().try_fold(0u64, |s, r| r.1.map(|m| s.max(m)))),
    };
    let total = match (max, n.max) {
        (Some(0), _) => Some(0),
        (Some(m), Some(k)) => Some(m.saturating_mul(k)),
        (None, _) | (Some(_), None) => None,
    };
    (n.min.saturating_mul(min), total)
}

/// [Schema1 §3.9.6 cos-group-emptiable] Whether `n` is emptiable.
fn emptiable(n: &Node) -> bool {
    range(n).0 == 0
}

/// Whether `r` restricts `b`, by the case of the table of [Schema1
/// §3.9.6] for their kinds.
fn valid(schema: &Schema, r: &Node, b: &Node, depth: usize) -> Verdict {
    if depth > MAX_DEPTH {
        return Err("cos-particle-restrict: the groups nest too deeply".into());
    }
    if r == b {
        return Ok(());
    }
    let deeper = depth.saturating_add(1);
    match (&r.kind, &b.kind) {
        (Kind::Elt(re), Kind::Elt(be)) => name_and_type(schema, (r, *re), (b, *be)),
        (Kind::Elt(re), Kind::Any(w)) => ns_compat(schema, (r, *re), (b, w)),
        (Kind::Elt(_), Kind::Group(c, _)) => {
            let group = Node { min: 1, max: Some(1), kind: Kind::Group(*c, vec![r.clone()]) };
            valid(schema, &group, b, deeper)
        }
        (Kind::Any(rw), Kind::Any(bw)) => {
            let ok = range_ok((r.min, r.max), (b.min, b.max)) && rw.constraint.subset_of(&bw.constraint);
            ok.then_some(()).ok_or_else(|| "cos-particle-restrict (NSSubset): a wildcard restricts a wildcard".into())
        }
        (Kind::Group(_, rs), Kind::Any(_)) => {
            rs.iter().try_for_each(|q| valid(schema, q, b, deeper))?;
            let ok = range_ok(range(r), (b.min, b.max));
            ok.then_some(()).ok_or_else(|| "cos-particle-restrict (NSRecurseCheckCardinality): a group restricts a wildcard".into())
        }
        (Kind::Group(Compositor::All, rs), Kind::Group(Compositor::All, bs))
        | (Kind::Group(Compositor::Sequence, rs), Kind::Group(Compositor::Sequence, bs)) => {
            ordered(schema, (r, rs), (b, bs), (true, deeper))
        }
        (Kind::Group(Compositor::Choice, rs), Kind::Group(Compositor::Choice, bs)) => ordered(schema, (r, rs), (b, bs), (false, deeper)),
        (Kind::Group(Compositor::Sequence, rs), Kind::Group(Compositor::All, bs)) => unordered(schema, (r, rs), (b, bs), deeper),
        (Kind::Group(Compositor::Sequence, rs), Kind::Group(Compositor::Choice, bs)) => map_and_sum(schema, (r, rs), (b, bs), deeper),
        (Kind::Any(_), Kind::Elt(_) | Kind::Group(..))
        | (Kind::Group(Compositor::All | Compositor::Choice, _), Kind::Elt(_))
        | (Kind::Group(Compositor::Sequence, _), Kind::Elt(_))
        | (Kind::Group(Compositor::All, _), Kind::Group(Compositor::Choice | Compositor::Sequence, _))
        | (Kind::Group(Compositor::Choice, _), Kind::Group(Compositor::All | Compositor::Sequence, _)) => {
            Err("cos-particle-restrict: the table forbids this restriction".into())
        }
    }
}

/// [Schema1 §3.9.6 rcase-NameAndTypeOK] Whether element particle `r`
/// restricts element particle `b`.
fn name_and_type(schema: &Schema, (r, re): (&Node, ElementId), (b, be): (&Node, ElementId)) -> Verdict {
    let fail = |clause: &str| Err(format!("rcase-NameAndTypeOK.{clause}"));
    if !range_ok((r.min, r.max), (b.min, b.max)) {
        return fail("3");
    }
    if re == be {
        return Ok(());
    }
    let (Some(rd), Some(bd)) = (schema.element(re), schema.element(be)) else { return fail("1") };
    if rd.name != bd.name {
        return fail("1");
    }
    if rd.nillable && !bd.nillable {
        return fail("2");
    }
    if let Some(fixed) = bd.value.as_ref().filter(|v| v.kind == ValueKind::Fixed)
        && !rd.value.as_ref().is_some_and(|v| v.kind == ValueKind::Fixed && v.value.equal(&fixed.value))
    {
        return fail("4");
    }
    let names = |d: &super::super::component::ElementDecl| -> Vec<_> {
        d.constraints.iter().filter_map(|c| schema.constraint(*c)).map(|c| c.name.clone()).collect()
    };
    let base_names = names(bd);
    if !names(rd).iter().all(|n| base_names.contains(n)) {
        return fail("5");
    }
    if rd.disallowed.within(bd.disallowed) != bd.disallowed {
        return fail("6");
    }
    let blocked = Derivations::EXTENSION.with(Derivations::LIST).with(Derivations::UNION);
    if !type_derives(schema, rd.type_def, bd.type_def, blocked) {
        return fail("7");
    }
    Ok(())
}

/// [Schema1 §3.9.6 rcase-NSCompat] Whether element particle `r` restricts
/// wildcard particle `b`.
fn ns_compat(schema: &Schema, (r, re): (&Node, ElementId), (b, w): (&Node, &Wildcard)) -> Verdict {
    let allowed = schema.element(re).is_some_and(|d| w.constraint.allows(d.name.uri.as_deref()));
    let ok = allowed && range_ok((r.min, r.max), (b.min, b.max));
    ok.then_some(()).ok_or_else(|| "rcase-NSCompat: an element restricts a wildcard".into())
}

/// [Schema1 §3.9.6 rcase-Recurse, rcase-RecurseLax] Whether group `r`
/// restricts group `b` of the same compositor by an order-preserving
/// mapping of its particles to `b`'s; with `strict`, those of `b` mapped
/// to by none emptiable.
fn ordered(schema: &Schema, (r, rs): (&Node, &[Node]), (b, bs): (&Node, &[Node]), (strict, depth): (bool, usize)) -> Verdict {
    let case = if strict { "rcase-Recurse" } else { "rcase-RecurseLax" };
    if !range_ok((r.min, r.max), (b.min, b.max)) {
        return Err(format!("{case}.1"));
    }
    let mut at = 0usize;
    for q in rs {
        loop {
            let Some(base) = bs.get(at) else { return Err(format!("{case}.2")) };
            at = at.saturating_add(1);
            if valid(schema, q, base, depth).is_ok() {
                break;
            }
            if strict && !emptiable(base) {
                return Err(format!("{case}.2.2"));
            }
        }
    }
    let rest_ok = !strict || bs.iter().skip(at).all(emptiable);
    rest_ok.then_some(()).ok_or_else(|| format!("{case}.2.2"))
}

/// [Schema1 §3.9.6 rcase-RecurseUnordered] Whether sequence `r` restricts
/// all group `b`.
fn unordered(schema: &Schema, (r, rs): (&Node, &[Node]), (b, bs): (&Node, &[Node]), depth: usize) -> Verdict {
    if !range_ok((r.min, r.max), (b.min, b.max)) {
        return Err("rcase-RecurseUnordered.1".into());
    }
    let mut mapped = vec![false; bs.len()];
    for q in rs {
        let found =
            bs.iter().enumerate().find(|(i, base)| !mapped.get(*i).copied().unwrap_or(true) && valid(schema, q, base, depth).is_ok());
        let Some((i, _)) = found else { return Err("rcase-RecurseUnordered.2".into()) };
        if let Some(m) = mapped.get_mut(i) {
            *m = true;
        }
    }
    let rest_ok = bs.iter().zip(&mapped).all(|(base, m)| *m || emptiable(base));
    rest_ok.then_some(()).ok_or_else(|| "rcase-RecurseUnordered.2.3".into())
}

/// [Schema1 §3.9.6 rcase-MapAndSum] Whether sequence `r` restricts choice
/// `b`: each of its particles restricts one of `b`'s, and its range
/// times its length is within `b`'s.
fn map_and_sum(schema: &Schema, (r, rs): (&Node, &[Node]), (b, bs): (&Node, &[Node]), depth: usize) -> Verdict {
    if !rs.iter().all(|q| bs.iter().any(|base| valid(schema, q, base, depth).is_ok())) {
        return Err("rcase-MapAndSum.1".into());
    }
    let length = u64::try_from(rs.len()).unwrap_or(u64::MAX);
    let total = (r.min.saturating_mul(length), r.max.map(|m| m.saturating_mul(length)));
    range_ok(total, (b.min, b.max)).then_some(()).ok_or_else(|| "rcase-MapAndSum.2".into())
}
