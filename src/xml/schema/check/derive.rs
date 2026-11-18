//! Derivations of type definitions [Schema1 §3.4.6, §3.14.6]: Type
//! Derivation OK (Complex) and (Simple), the constraints on complex types
//! derived by extension and by restriction, and substitution groups
//! [Schema1 §3.3.6].
//!
//! \[DESIGN\] Implementation-defined: every type definition is validly
//! derived from `xs:anyType`, the ur-type, from which every type is
//! derived [Schema1 §2.2.1.1], though Type Derivation OK (Simple) takes a
//! built-in primitive type's base to be the simple ur-type alone. Clause
//! 1.5 of Derivation Valid (Extension), which asks that a type be
//! derivable by one extension and one restriction from the ancestor
//! derived from the ur-type, is not checked.

use super::super::builtin::simple_base;
use super::super::component::{
    AttributeUse, ComplexId, ComplexType, ContentType, Derivations, ElementId, Method, Schema, TypeDef, ValueKind, Wildcard,
};
use super::super::{MAX_CHAIN, invalid};
use super::{attribute_uses_correct, emptiable, particle_restricts};
use crate::error::Result;

/// The derivation method as a member of a derivation set.
fn as_set(method: Method) -> Derivations {
    match method {
        Method::Extension => Derivations::EXTENSION,
        Method::Restriction => Derivations::RESTRICTION,
    }
}

/// One step up from type definition `t`: its derivation method and base;
/// none of the ur-type.
fn step(schema: &Schema, t: TypeDef) -> Option<(Method, TypeDef)> {
    match t {
        TypeDef::Complex(ComplexId::ANY_TYPE) => None,
        TypeDef::Complex(c) => schema.complex(c).map(|c| (c.method, c.base)),
        TypeDef::Simple(s) => Some((Method::Restriction, simple_base(s).map_or(TypeDef::Complex(ComplexId::ANY_TYPE), TypeDef::Simple))),
    }
}

/// Type Derivation OK (Complex) and (Simple) [Schema1 §3.4.6, §3.14.6]:
/// whether `d` is validly derived from `b` given the derivation methods
/// `blocked`: `d` is `b`; or no step of the chain from `d` up to `b` is
/// by a method in `blocked`, a list's base being the simple ur-type
/// \[2.2.3\]; or `b` is the ur-type.
#[must_use]
pub fn type_derives(schema: &Schema, d: TypeDef, b: TypeDef, blocked: Derivations) -> bool {
    if d == b {
        return true;
    }
    if b == TypeDef::Complex(ComplexId::ANY_TYPE) {
        return matches!(d, TypeDef::Simple(_)) || chain_avoids(schema, d, b, blocked);
    }
    chain_avoids(schema, d, b, blocked)
}

/// Whether the chain of bases from `d` reaches `b` within `MAX_CHAIN`
/// steps, none by a method in `blocked`.
fn chain_avoids(schema: &Schema, d: TypeDef, b: TypeDef, blocked: Derivations) -> bool {
    let mut at = d;
    for _ in 0..MAX_CHAIN {
        if at == b {
            return true;
        }
        let Some((method, base)) = step(schema, at) else { return false };
        if blocked.has(as_set(method)) {
            return false;
        }
        at = base;
    }
    false
}

/// [Schema1 §3.3.6 cos-equiv-derived-ok-rec] Substitution Group OK
/// (Transitive): whether element declaration `d` may stand for `c` given
/// blocking constraint `blocking`: no substitution blocked, `c` reached by
/// `d`'s substitution group affiliations, and no method of the
/// derivation of `d`'s type from `c`'s in `blocking`, `c`'s type's
/// {prohibited substitutions} or an intermediate type's.
#[must_use]
pub fn substitutable(schema: &Schema, d: ElementId, c: ElementId, blocking: Derivations) -> bool {
    if blocking.has(Derivations::SUBSTITUTION) {
        return false;
    }
    let mut head = Some(d);
    let mut reached = false;
    for _ in 0..MAX_CHAIN {
        head = head.and_then(|h| schema.element(h)).and_then(|e| e.substitution_group);
        reached = head == Some(c);
        if reached || head.is_none() {
            break;
        }
    }
    let (Some(dd), Some(cd)) = (schema.element(d), schema.element(c)) else { return false };
    reached && derived_avoiding(schema, dd.type_def, cd.type_def, blocking)
}

/// The {prohibited substitutions} of `t`: of a simple type, none.
fn prohibited(schema: &Schema, t: TypeDef) -> Derivations {
    match t {
        TypeDef::Complex(c) => schema.complex(c).map_or(Derivations::NONE, |c| c.prohibited),
        TypeDef::Simple(_) => Derivations::NONE,
    }
}

/// [Schema1 §3.3.6 cos-equiv-derived-ok-rec.3] Whether `d` is derived
/// from `c` by no method in `blocking`, `c`'s prohibited substitutions or
/// those of a type between them.
fn derived_avoiding(schema: &Schema, d: TypeDef, c: TypeDef, blocking: Derivations) -> bool {
    let mut blocked = blocking.with(prohibited(schema, c));
    let mut at = d;
    for _ in 0..MAX_CHAIN {
        if at == c {
            return true;
        }
        let Some((method, base)) = step(schema, at) else { return c == TypeDef::Complex(ComplexId::ANY_TYPE) };
        if blocked.has(as_set(method)) {
            return false;
        }
        if base != c {
            blocked = blocked.with(prohibited(schema, base));
        }
        at = base;
    }
    false
}

/// [Schema1 §3.3.6 e-props-correct.3] Whether element declaration `e`'s
/// type is validly derived from its substitution group head's, given the
/// head's {substitution group exclusions}.
///
/// # Errors
///
/// SQLSTATE 22000 if not.
pub fn substitution(schema: &Schema, e: ElementId) -> Result<()> {
    let Some(decl) = schema.element(e) else { return Ok(()) };
    let Some(head) = decl.substitution_group.and_then(|h| schema.element(h)) else { return Ok(()) };
    if !type_derives(schema, decl.type_def, head.type_def, head.exclusions) {
        return Err(invalid(
            "[Schema1 §3.3.6 e-props-correct.3]",
            &format!("the type of {} is derived from that of its substitution group's head", decl.name.text()),
        ));
    }
    Ok(())
}

/// The constraints on complex type definition `id` [Schema1 §3.4.6]:
/// ct-props-correct, and Derivation Valid (Extension) or (Restriction,
/// Complex).
///
/// # Errors
///
/// SQLSTATE 22000 naming the constraint broken.
pub fn complex_type(schema: &Schema, id: ComplexId) -> Result<()> {
    let Some(def) = schema.complex(id) else { return Ok(()) };
    attribute_uses_correct(schema, &def.attribute_uses, "[Schema1 §3.4.6 ct-props-correct.4]", "[Schema1 §3.4.6 ct-props-correct.5]")?;
    match def.method {
        Method::Extension => extension(schema, def),
        Method::Restriction => restriction(schema, def),
    }
}

/// [Schema1 §3.4.6 cos-ct-extends] Derivation Valid (Extension).
///
/// # Errors
///
/// SQLSTATE 22000 naming the clause broken.
fn extension(schema: &Schema, def: &ComplexType) -> Result<()> {
    let base = match def.base {
        TypeDef::Simple(s) => {
            let same = def.content == ContentType::Simple(s);
            return same
                .then_some(())
                .ok_or_else(|| invalid("[Schema1 §3.4.6 cos-ct-extends.2.1]", "an extension of a simple type has it as content"));
        }
        TypeDef::Complex(b) => schema.complex(b).ok_or_else(|| invalid("[Schema1 §3.4.1]", "a base type is missing"))?,
    };
    if base.final_.has(Derivations::EXTENSION) {
        return Err(invalid("[Schema1 §3.4.6 cos-ct-extends.1.1]", "a type final for extension is not extended"));
    }
    let kept = base.attribute_uses.iter().all(|b| {
        let decl = schema.attribute(b.decl);
        def.attribute_uses
            .iter()
            .filter_map(|u| schema.attribute(u.decl))
            .any(|a| decl.is_some_and(|d| d.name == a.name && d.type_def == a.type_def))
    });
    if !kept {
        return Err(invalid("[Schema1 §3.4.6 cos-ct-extends.1.2]", "an extension keeps its base's attribute uses"));
    }
    if let Some(b) = &base.wildcard
        && !def.wildcard.as_ref().is_some_and(|w| b.constraint.subset_of(&w.constraint))
    {
        return Err(invalid("[Schema1 §3.4.6 cos-ct-extends.1.3]", "an extension's attribute wildcard allows what its base's does"));
    }
    let content_ok = match (&base.content, &def.content) {
        (ContentType::Simple(a), ContentType::Simple(b)) => a == b,
        (ContentType::Empty, ContentType::Empty | ContentType::Mixed(_) | ContentType::ElementOnly(_)) => true,
        (ContentType::Mixed(b), ContentType::Mixed(p)) | (ContentType::ElementOnly(b), ContentType::ElementOnly(p)) => {
            valid_extension(schema, p, b)
        }
        (ContentType::Simple(_) | ContentType::Empty | ContentType::Mixed(_) | ContentType::ElementOnly(_), _) => false,
    };
    content_ok.then_some(()).ok_or_else(|| invalid("[Schema1 §3.4.6 cos-ct-extends.1.4]", "an extension's content extends its base's"))
}

/// [Schema1 §3.9.6 cos-particle-extend] Whether particle `e` is a valid
/// extension of `b`: the same, or a sequence once whose first particle is
/// `b`.
fn valid_extension(schema: &Schema, e: &super::super::component::Particle, b: &super::super::component::Particle) -> bool {
    if e == b {
        return true;
    }
    let super::super::component::Term::Group(g) = &e.term else { return false };
    e.min == 1
        && e.max == Some(1)
        && schema.group(*g).is_some_and(|m| m.compositor == super::super::component::Compositor::Sequence && m.particles.first() == Some(b))
}

/// [Schema1 §3.4.6 derivation-ok-restriction] Derivation Valid
/// (Restriction, Complex).
///
/// # Errors
///
/// SQLSTATE 22000 naming the clause broken.
fn restriction(schema: &Schema, def: &ComplexType) -> Result<()> {
    let fail = |clause: &str, why: &str| invalid(&format!("[Schema1 §3.4.6 derivation-ok-restriction.{clause}]"), why);
    let TypeDef::Complex(b) = def.base else { return Err(fail("1", "a restriction's base is a complex type")) };
    let base = schema.complex(b).ok_or_else(|| fail("1", "a restriction's base is a complex type"))?;
    if base.final_.has(Derivations::RESTRICTION) {
        return Err(fail("1", "a type final for restriction is not restricted"));
    }
    super::attributes_restrict(schema, (&def.attribute_uses, def.wildcard.as_ref()), (&base.attribute_uses, base.wildcard.as_ref()))
        .map_err(|clause| invalid(&format!("[Schema1 §3.4.6 {clause}]"), "a restriction's attributes restrict its base's"))?;
    let ok = match (&def.content, &base.content) {
        (ContentType::Simple(s), ContentType::Simple(t)) => s == t,
        (ContentType::Simple(_), ContentType::Mixed(p)) | (ContentType::Empty, ContentType::Mixed(p) | ContentType::ElementOnly(p)) => {
            emptiable(schema, p)
        }
        (ContentType::Empty, ContentType::Empty) => true,
        (ContentType::Mixed(rp), ContentType::Mixed(bp))
        | (ContentType::ElementOnly(rp), ContentType::Mixed(bp) | ContentType::ElementOnly(bp)) => {
            return particle_restricts(schema, rp, bp)
                .map_err(|why| fail("5.3", &format!("a restriction's content model restricts its base's: {why}")));
        }
        (ContentType::Simple(_) | ContentType::Empty | ContentType::Mixed(_) | ContentType::ElementOnly(_), _) => false,
    };
    ok.then_some(()).ok_or_else(|| fail("5", "a restriction's content restricts its base's"))
}

/// [Schema1 §3.4.6 derivation-ok-restriction.2, 3] Whether each of `uses`
/// restricts the use of its name among `base_uses`, or its name is allowed
/// by `base_wildcard`, and each required base use is kept required; else
/// the clause broken.
///
/// # Errors
///
/// The clause broken.
pub fn uses_restrict(
    schema: &Schema,
    uses: &[AttributeUse],
    base_uses: &[AttributeUse],
    base_wildcard: Option<&Wildcard>,
) -> std::result::Result<(), &'static str> {
    let name = |u: &AttributeUse| schema.attribute(u.decl).map(|a| a.name.clone());
    for r in uses {
        let Some(rn) = name(r) else { continue };
        match base_uses.iter().find(|b| name(b).as_ref() == Some(&rn)) {
            Some(b) => use_restricts(schema, r, b)?,
            None => {
                if !base_wildcard.is_some_and(|w| w.constraint.allows(rn.uri.as_deref())) {
                    return Err("derivation-ok-restriction.2.2");
                }
            }
        }
    }
    for b in base_uses.iter().filter(|b| b.required) {
        if !uses.iter().any(|r| r.required && name(r) == name(b)) {
            return Err("derivation-ok-restriction.3");
        }
    }
    Ok(())
}

/// [Schema1 §3.4.6 derivation-ok-restriction.2.1] Whether attribute use
/// `r` restricts `b`, of the same name.
fn use_restricts(schema: &Schema, r: &AttributeUse, b: &AttributeUse) -> std::result::Result<(), &'static str> {
    if b.required && !r.required {
        return Err("derivation-ok-restriction.2.1.1");
    }
    let (Some(rd), Some(bd)) = (schema.attribute(r.decl), schema.attribute(b.decl)) else { return Ok(()) };
    if !type_derives(schema, TypeDef::Simple(rd.type_def), TypeDef::Simple(bd.type_def), Derivations::NONE) {
        return Err("derivation-ok-restriction.2.1.2");
    }
    let effective = |u: &AttributeUse, decl: &super::super::component::AttributeDecl| u.value.clone().or_else(|| decl.value.clone());
    let Some(fixed) = effective(b, bd).filter(|v| v.kind == ValueKind::Fixed) else { return Ok(()) };
    let same = effective(r, rd).is_some_and(|v| v.kind == ValueKind::Fixed && v.value.equal(&fixed.value));
    same.then_some(()).ok_or("derivation-ok-restriction.2.1.3")
}
