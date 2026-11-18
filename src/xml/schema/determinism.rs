//! Whether a global element declaration of a schema is non-deterministic
//! [ISO/IEC 9075-14:2006 draft §4.2.6]: whether it contains or references a
//! wildcard whose {namespace constraint} is any or not and a namespace
//! name, and whose {process contents} is strict or lax, such as `<xs:any>`
//! or `<xs:anyAttribute>` of `##any` or `##other` [NOTE 6].

use std::collections::BTreeSet;

use super::component::{ComplexId, ElementId, NamespaceConstraint, Process, Schema, Term, TypeDef, Wildcard};

/// Whether `w` makes what contains it non-deterministic \[§4.2.6\].
fn open(w: &Wildcard) -> bool {
    !matches!(w.constraint, NamespaceConstraint::Set(_)) && w.process != Process::Skip
}

impl Schema {
    /// [ISO/IEC 9075-14:2006 draft §4.2.6] Whether element declaration
    /// `e` is non-deterministic: its type, the types of the elements its
    /// content holds, and theirs, at any depth, have such a wildcard. Each
    /// type is looked at once, so the walk ends.
    #[must_use]
    pub fn non_deterministic(&self, e: ElementId) -> bool {
        let mut seen: BTreeSet<ComplexId> = BTreeSet::new();
        let mut types: Vec<TypeDef> = self.element(e).map(|d| d.type_def).into_iter().collect();
        while let Some(t) = types.pop() {
            let TypeDef::Complex(c) = t else { continue };
            if !seen.insert(c) {
                continue;
            }
            let Some(def) = self.complex(c) else { continue };
            if def.wildcard.as_ref().is_some_and(open) {
                return true;
            }
            if self.content_open(def.content.particle(), &mut types) {
                return true;
            }
        }
        false
    }

    /// Whether content model `model` has such a wildcard at any depth, the
    /// types of its element declarations added to `types` to look at.
    fn content_open(&self, model: Option<&super::component::Particle>, types: &mut Vec<TypeDef>) -> bool {
        let mut particles: Vec<&super::component::Particle> = model.into_iter().collect();
        let mut groups = BTreeSet::new();
        while let Some(p) = particles.pop() {
            match &p.term {
                Term::Wildcard(w) => {
                    if open(w) {
                        return true;
                    }
                }
                Term::Element(d) => types.extend(self.element(*d).map(|d| d.type_def)),
                Term::Group(g) => {
                    if groups.insert(*g) {
                        particles.extend(self.group(*g).into_iter().flat_map(|m| &m.particles));
                    }
                }
            }
        }
        false
    }

    /// [ISO/IEC 9075-14:2006 draft §4.2.6] Whether namespace `ns`, none
    /// for absent, of the schema is non-deterministic: one of its global
    /// element declarations is.
    #[must_use]
    pub fn namespace_non_deterministic(&self, ns: Option<&str>) -> bool {
        self.element_names.iter().any(|(name, &e)| name.uri.as_deref() == ns && self.non_deterministic(e))
    }

    /// [ISO/IEC 9075-14:2006 draft §4.2.6] Whether the schema is
    /// non-deterministic: one of its namespaces is.
    #[must_use]
    pub fn is_non_deterministic(&self) -> bool {
        self.element_names.values().any(|&e| self.non_deterministic(e))
    }
}
