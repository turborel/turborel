//! Particles and model groups read from `<all>`, `<choice>`, `<sequence>`,
//! `<group>`, `<element>` and `<any>` [Schema1 §3.7.2, §3.8.2, §3.9.2,
//! §3.10.2], and model group definitions, with their redefinitions
//! [Schema1 §4.2.2].

use super::super::component::{Compositor, Expanded, GroupDefId, Particle, Scope, Term};
use super::super::{check, invalid};
use super::attrs::At;
use super::build::{Builder, Place};
use super::element::Local;
use super::element::annotation;
use super::index::Space;
use crate::error::Result;

impl Builder<'_> {
    /// The particle `at`, an element of local name `which`, corresponds to
    /// [Schema1 §3.9.2]; none if its minOccurs and maxOccurs are 0, or it
    /// is no particle.
    ///
    /// # Errors
    ///
    /// As the reading of each kind.
    pub fn particle(&mut self, at: &At<'_>, which: &str, local: Local) -> Result<Option<Particle>> {
        match which {
            "element" => self.local_element(at, local),
            "group" => self.group_reference(at, local),
            "all" => self.model_group(at, Compositor::All, local),
            "choice" => self.model_group(at, Compositor::Choice, local),
            "sequence" => self.model_group(at, Compositor::Sequence, local),
            "any" => {
                let (min, max) = at.occurs()?;
                let wildcard = self.wildcard(at, local.doc)?;
                Ok((min != 0 || max != Some(0)).then_some(Particle { min, max, term: Term::Wildcard(wildcard) }))
            }
            _ => Ok(None),
        }
    }

    /// The particles of the children of `at`, each read in order [Schema1
    /// §3.8.2].
    ///
    /// # Errors
    ///
    /// As `particle`.
    fn particles(&mut self, at: &At<'_>, local: Local) -> Result<Vec<Particle>> {
        let local = Local { depth: Builder::deeper(local.depth)?, ..local };
        let mut out = Vec::new();
        for (child, which) in at.children()? {
            out.extend(self.particle(&child, which, local)?);
        }
        Ok(out)
    }

    /// The particle of a model group of `compositor`, `<all>`, `<choice>` or
    /// `<sequence>` `at` [Schema1 §3.8.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if an `<all>`'s minOccurs is other than 0 or 1, or
    /// its maxOccurs other than 1 [Schema1 Appendix A].
    fn model_group(&mut self, at: &At<'_>, compositor: Compositor, local: Local) -> Result<Option<Particle>> {
        let (min, max) = at.occurs()?;
        if compositor == Compositor::All && (min > 1 || max != Some(1)) {
            return Err(invalid("[Schema1 §3.8.2]", "an <all> occurs once, or optionally"));
        }
        let particles = self.particles(at, local)?;
        let group = self.new_group(compositor, particles);
        let annotation = annotation(local.doc, at)?;
        if let Some(g) = self.schema.groups.get_mut(group.0) {
            g.annotation = annotation;
        }
        Ok((min != 0 || max != Some(0)).then_some(Particle { min, max, term: Term::Group(group) }))
    }

    /// The particle of `<group>` `at` with a ref [Schema1 §3.7.2]: the
    /// model group of the definition it names.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has no ref, names no model group definition, or
    /// is a redefinition's reference to itself of other than one
    /// occurrence [Schema1 §4.2.2 src-redefine.6.1.2].
    fn group_reference(&mut self, at: &At<'_>, local: Local) -> Result<Option<Particle>> {
        let (min, max) = at.occurs()?;
        let name = self.qname(local.doc, at, "ref")?.ok_or_else(|| invalid("[Schema1 §3.7.2]", "a nested <group> has a ref"))?;
        if self.original_of(Space::Group, &name).is_some() && (min, max) != (1, Some(1)) {
            return Err(invalid("[Schema1 §4.2.2 src-redefine.6.1.2]", "a redefined group refers to itself once"));
        }
        let place = self.entry(Space::Group, &name).ok_or_else(|| Builder::unresolved(&name, "model group definition"))?;
        let def = self.group_def_at(place, local.depth)?;
        let group =
            self.schema.group_defs.get(def.0).map(|d| d.group).ok_or_else(|| Builder::unresolved(&name, "model group definition"))?;
        Ok((min != 0 || max != Some(0)).then_some(Particle { min, max, term: Term::Group(group) }))
    }

    /// Model group definition `id`'s properties read from `<group>` `place`
    /// [Schema1 §3.7.2]: its name, and its one `<all>`, `<choice>` or
    /// `<sequence>`, read into its model group.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has not one of them; if a redefinition, refers
    /// to itself more than once [Schema1 §4.2.2 src-redefine.6.1.1], or,
    /// never, does not restrict the one it redefines \[6.2.2\].
    pub fn read_group_def(&mut self, place: Place, id: GroupDefId, depth: usize) -> Result<()> {
        let at = self.at(place)?;
        let tns = self.doc(place.0)?.tns.clone();
        let name = Expanded::new(tns.as_deref(), &at.ncname("name")?.unwrap_or_default());
        let groups: Vec<(At<'_>, &str)> = at.children()?.into_iter().filter(|(_, l)| matches!(*l, "all" | "choice" | "sequence")).collect();
        let [(inner, which)] = groups.as_slice() else {
            return Err(invalid("[Schema1 §3.7.2]", "a model group definition has one <all>, <choice> or <sequence>"));
        };
        let compositor = match *which {
            "all" => Compositor::All,
            "choice" => Compositor::Choice,
            _ => Compositor::Sequence,
        };
        let local = Local { doc: place.0, scope: Scope::Absent, depth };
        let particles = self.particles(inner, local)?;
        let annotation = annotation(place.0, &at)?;
        let group = self.schema.group_defs.get(id.0).map(|d| d.group);
        if let Some(g) = group.and_then(|g| self.schema.groups.get_mut(g.0)) {
            g.compositor = compositor;
            g.particles = particles;
        }
        if let Some(d) = self.schema.group_defs.get_mut(id.0) {
            d.name = name;
            d.annotation = annotation;
        }
        self.redefined_group(place, id)
    }

    /// [Schema1 §4.2.2 src-redefine.6] Model group definition `id`, at
    /// `place`, if it redefines one: it refers to itself once at most
    /// \[6.1.1\], or, never, its model group restricts the one it
    /// redefines \[6.2.2\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it does neither.
    fn redefined_group(&mut self, place: Place, id: GroupDefId) -> Result<()> {
        let Some(context) = self.contexts.get(&place).cloned() else { return Ok(()) };
        if self.self_references > 1 {
            return Err(invalid("[Schema1 §4.2.2 src-redefine.6.1.1]", "a redefined group refers to itself at most once"));
        }
        if self.self_references == 1 {
            return Ok(());
        }
        let original = self.group_def_at(context.original, 0)?;
        let groups = (self.schema.group_defs.get(id.0).map(|d| d.group), self.schema.group_defs.get(original.0).map(|d| d.group));
        let (Some(new), Some(old)) = groups else { return Ok(()) };
        let particle = |g| Particle { min: 1, max: Some(1), term: Term::Group(g) };
        check::particle_restricts(&self.schema, &particle(new), &particle(old)).map_err(|why| {
            invalid("[Schema1 §4.2.2 src-redefine.6.2.2]", &format!("a redefined group restricts the one it redefines: {why}"))
        })
    }
}
