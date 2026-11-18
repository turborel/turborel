//! Complex type definitions read from `<complexType>` [Schema1 §3.4.2]:
//! simple content, complex content, and the shorthand of complex content
//! restricting the ur-type; their attribute uses and attribute wildcards,
//! and attribute group definitions [Schema1 §3.6.2], whose uses and
//! wildcards they take in.

use super::super::component::{
    AnnotationRef, AttributeGroupDef, AttributeGroupId, AttributeUse, ComplexId, ComplexType, Compositor, ContentType, Derivations,
    Expanded, Method, ModelGroup, Particle, Process, Scope, Term, TypeDef, Wildcard,
};
use super::super::wildcard::namespace_constraint;
use super::super::{check, invalid};
use super::attrs::At;
use super::build::{Builder, Place, State, simple_types};
use super::element::annotation;
use super::element::{Local, UseRead};
use super::index::Space;
use crate::error::Result;

/// [Schema1 §3.4.2] The elements of the content models of complex
/// content.
pub(super) const GROUPS: [&str; 4] = ["group", "all", "choice", "sequence"];
/// [Schema1 §3.4.2] The children of `<restriction>` of simple content
/// that are facets [Schema2 §4.3].
const FACETS: [&str; 12] = [
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
];

/// The attributes a complex type definition or an attribute group
/// definition is given by its children [Schema1 §3.4.2, §3.6.2].
#[derive(Debug, Default)]
struct Attributes {
    /// The attribute uses of its `<attribute>` and `<attributeGroup>`
    /// children.
    uses: Vec<AttributeUse>,
    /// The names of its `<attribute>`s that are prohibited.
    prohibited: Vec<Expanded>,
    /// The wildcard of its `<anyAttribute>`, the local wildcard.
    local: Option<Wildcard>,
    /// The non-absent wildcards of the attribute groups it refers to.
    groups: Vec<Wildcard>,
}

/// How a `<restriction>` or `<extension>` derives.
#[derive(Debug, Clone, Copy)]
struct Deriving {
    /// The method.
    method: Method,
    /// The base type definition.
    base: TypeDef,
}

impl Builder<'_> {
    /// The complex type definition `<complexType>` `place` represents,
    /// complex type `id`; anonymous if `anonymous`, as a definition a
    /// `<redefine>` replaces is [Schema1 §4.2.2 clause 1.1].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it breaks a representation constraint [Schema1
    /// §3.4.3]; as the reading of its parts.
    pub fn read_complex(&mut self, place: Place, id: ComplexId, anonymous: bool, depth: usize) -> Result<ComplexType> {
        let at = self.at(place)?;
        let doc = self.doc(place.0)?;
        let name = if anonymous { None } else { at.ncname("name")?.map(|n| Expanded::new(doc.tns.as_deref(), &n)) };
        let both = Derivations::EXTENSION.with(Derivations::RESTRICTION);
        let prohibited = at.derivations("block", both, doc.block_default)?.within(both);
        let final_ = at.derivations("final", both, doc.final_default)?.within(both);
        let mixed = at.boolean("mixed", false)?;
        let mut annotations: Vec<AnnotationRef> = annotation(place.0, &at)?.into_iter().collect();
        let children: Vec<(At<'_>, &str)> = at.children()?.into_iter().filter(|(_, l)| *l != "annotation").collect();
        let local = Local { doc: place.0, scope: Scope::Local(id), depth };
        let mut def = ComplexType {
            name,
            namespace: doc.tns.clone(),
            base: TypeDef::Complex(ComplexId::ANY_TYPE),
            method: Method::Restriction,
            final_,
            abstract_: at.boolean("abstract", false)?,
            attribute_uses: Vec::new(),
            wildcard: None,
            content: ContentType::Empty,
            prohibited,
            annotations: Vec::new(),
        };
        match children.first() {
            Some((content, "simpleContent")) => {
                annotations.extend(annotation(place.0, content)?);
                self.simple_content(&mut def, content, local, &mut annotations)?;
            }
            Some((content, "complexContent")) => {
                annotations.extend(annotation(place.0, content)?);
                let mixed = if content.attr("mixed").is_some() { content.boolean("mixed", false)? } else { mixed };
                let (derivation, derived) = only_derivation(content)?;
                annotations.extend(annotation(place.0, &derivation)?);
                let base = self.base_of(&derivation, place.0, &def)?;
                let deriving = Deriving { method: method_of(derived), base };
                self.complex_content(&mut def, &derivation.children()?, deriving, (mixed, local))?;
            }
            _ => {
                let deriving = Deriving { method: Method::Restriction, base: TypeDef::Complex(ComplexId::ANY_TYPE) };
                self.complex_content(&mut def, &children, deriving, (mixed, local))?;
            }
        }
        def.annotations = annotations;
        Ok(def)
    }

    /// The base type definition named by the base attribute of
    /// `<restriction>` or `<extension>` `at` of document `doc`, of complex
    /// type `def`: of a redefinition, its own name [Schema1 §4.2.2
    /// src-redefine.5].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has no base, or a redefinition's base is not
    /// its own name.
    fn base_of(&mut self, at: &At<'_>, doc: usize, def: &ComplexType) -> Result<TypeDef> {
        let base = self.qname(doc, at, "base")?.ok_or_else(|| invalid("[Schema1 Appendix A]", &format!("<{}> has a base", at.local())))?;
        let redefines = self.redefining.as_ref().is_some_and(|r| r.space == Space::Type);
        if redefines && def.name.as_ref() != Some(&base) {
            return Err(invalid("[Schema1 §4.2.2 src-redefine.5]", "a redefined type derives from the type it redefines"));
        }
        self.type_named(&base)
    }

    /// `def`'s properties read from the `<restriction>` or `<extension>`
    /// of `<simpleContent>` `content` [Schema1 §3.4.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its base is neither a complex type of simple
    /// content nor, of an extension, a simple type [Schema1 §3.4.3
    /// src-ct.2]; SQLCODE -900 of facets or an anonymous simple type,
    /// which wait for Part 2's facets.
    fn simple_content(
        &mut self,
        def: &mut ComplexType,
        content: &At<'_>,
        local: Local,
        annotations: &mut Vec<AnnotationRef>,
    ) -> Result<()> {
        let (derivation, derived) = only_derivation(content)?;
        annotations.extend(annotation(local.doc, &derivation)?);
        let base = self.base_of(&derivation, local.doc, def)?;
        let method = method_of(derived);
        let base_content = self.content_of(base, local.depth)?;
        let restricts_simple = matches!(base, TypeDef::Simple(_)) && method == Method::Restriction;
        let simple = match base_content {
            ContentType::Simple(s) => (!restricts_simple).then_some(s),
            ContentType::Empty | ContentType::Mixed(_) | ContentType::ElementOnly(_) => None,
        };
        let simple = simple.ok_or_else(|| {
            invalid("[Schema1 §3.4.3 src-ct.2]", "simple content derives from a complex type of simple content, or extends a simple type")
        })?;
        let children = derivation.children()?;
        if children.iter().any(|(_, l)| *l == "simpleType" || FACETS.contains(l)) {
            return Err(simple_types());
        }
        let attributes = self.attributes(&children, local)?;
        def.base = base;
        def.method = method;
        def.content = ContentType::Simple(simple);
        self.attribute_properties(def, attributes, base, local)
    }

    /// `def`'s properties read from `children`, the content of a
    /// `<restriction>` or `<extension>` of complex content, or of a
    /// `<complexType>` written as the shorthand, derived as `deriving`,
    /// mixed if `mixed` [Schema1 §3.4.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its base is a simple type [Schema1 §3.4.3
    /// src-ct.1], or it extends a type of simple content by a content
    /// model [Schema1 §3.4.6 cos-ct-extends.1.4].
    fn complex_content(
        &mut self,
        def: &mut ComplexType,
        children: &[(At<'_>, &str)],
        deriving: Deriving,
        (mixed, local): (bool, Local),
    ) -> Result<()> {
        let TypeDef::Complex(base_id) = deriving.base else {
            return Err(invalid("[Schema1 §3.4.3 src-ct.1]", "complex content derives from a complex type"));
        };
        let base = self.complex_of(base_id, local.depth)?;
        let explicit = self.explicit_content(children, local)?;
        def.content = match (deriving.method, explicit) {
            (Method::Restriction, None) => ContentType::Empty,
            (Method::Restriction, Some(p)) => pair(mixed, p),
            (Method::Extension, None) => base.content.clone(),
            (Method::Extension, Some(p)) => match &base.content {
                ContentType::Empty => pair(mixed, p),
                ContentType::Mixed(b) | ContentType::ElementOnly(b) => {
                    let group = self.new_group(Compositor::Sequence, vec![b.clone(), p]);
                    pair(mixed, Particle { min: 1, max: Some(1), term: Term::Group(group) })
                }
                ContentType::Simple(_) => {
                    return Err(invalid(
                        "[Schema1 §3.4.6 cos-ct-extends.1.4]",
                        "a type of simple content is not extended by a content model",
                    ));
                }
            },
        };
        let attributes = self.attributes(children, local)?;
        def.base = deriving.base;
        def.method = deriving.method;
        self.attribute_properties(def, attributes, deriving.base, local)
    }

    /// The explicit content of `children` [Schema1 §3.4.2]: none if it is
    /// empty \[1.1\], as no particle, an empty `<all>` or `<sequence>`, or
    /// an empty `<choice>` of minOccurs 0 is; else its particle.
    ///
    /// # Errors
    ///
    /// As `particle`.
    fn explicit_content(&mut self, children: &[(At<'_>, &str)], local: Local) -> Result<Option<Particle>> {
        let Some((at, which)) = children.iter().find(|(_, l)| GROUPS.contains(l)) else { return Ok(None) };
        let own: Vec<(At<'_>, &str)> = at.children()?.into_iter().filter(|(_, l)| *l != "annotation").collect();
        let empty = match *which {
            "all" | "sequence" => own.is_empty(),
            "choice" => own.is_empty() && at.occurs()?.0 == 0,
            _ => false,
        };
        if empty {
            return Ok(None);
        }
        self.particle(at, which, local)
    }

    /// A new model group of `compositor` and `particles`.
    pub fn new_group(&mut self, compositor: Compositor, particles: Vec<Particle>) -> super::super::component::GroupId {
        let id = super::super::component::GroupId(self.schema.groups.len());
        self.schema.groups.push(ModelGroup { compositor, particles, annotation: None });
        id
    }

    /// `def`'s {attribute uses} and {attribute wildcard} from `attributes`,
    /// its own, and `base`'s [Schema1 §3.4.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if the intersection of the wildcards is not
    /// expressible [Schema1 §3.4.3 src-ct.4], or their union is not
    /// [Schema1 §3.4.2].
    fn attribute_properties(&mut self, def: &mut ComplexType, attributes: Attributes, base: TypeDef, local: Local) -> Result<()> {
        let complete = complete_wildcard(attributes.local.clone(), &attributes.groups, "[Schema1 §3.4.3 src-ct.4]")?;
        let base = match base {
            TypeDef::Complex(b) => Some(self.complex_of(b, local.depth)?),
            TypeDef::Simple(_) => None,
        };
        let mut uses = attributes.uses;
        if let Some(base) = &base {
            for inherited in &base.attribute_uses {
                let name = self.schema.attribute(inherited.decl).map(|a| a.name.clone());
                let overridden = name.as_ref().is_some_and(|n| {
                    attributes.prohibited.contains(n) || uses.iter().any(|u| self.schema.attribute(u.decl).is_some_and(|a| a.name == *n))
                });
                if def.method == Method::Extension || !overridden {
                    uses.push(inherited.clone());
                }
            }
        }
        def.attribute_uses = uses;
        def.wildcard = match (def.method, base.and_then(|b| b.wildcard)) {
            (Method::Restriction, _) | (Method::Extension, None) => complete,
            (Method::Extension, Some(b)) => match complete {
                None => Some(b),
                Some(c) => {
                    let constraint = c
                        .constraint
                        .union(&b.constraint)
                        .ok_or_else(|| invalid("[Schema1 §3.4.2]", "the union of an extension's attribute wildcards is expressible"))?;
                    Some(Wildcard { constraint, ..c })
                }
            },
        };
        Ok(())
    }

    /// The attributes of `children` [Schema1 §3.4.2]: their `<attribute>`
    /// and `<attributeGroup>` uses and `<anyAttribute>`.
    ///
    /// # Errors
    ///
    /// As `attribute_use`, `attribute_group_at` and `any_attribute`.
    fn attributes(&mut self, children: &[(At<'_>, &str)], local: Local) -> Result<Attributes> {
        let mut out = Attributes::default();
        for (at, which) in children {
            match *which {
                "attribute" => match self.attribute_use(at, local)? {
                    UseRead::Use(u) => out.uses.push(u),
                    UseRead::Prohibited(name) => out.prohibited.push(name),
                },
                "attributeGroup" => {
                    let name = self
                        .qname(local.doc, at, "ref")?
                        .ok_or_else(|| invalid("[Schema1 §3.6.2]", "a nested <attributeGroup> has a ref"))?;
                    let place = self.entry(Space::AttributeGroup, &name).ok_or_else(|| Builder::unresolved(&name, "attribute group"))?;
                    let id = self.attribute_group_at(place, local.depth)?;
                    let group = self.schema.attribute_groups.get(id.0).cloned();
                    if let Some(group) = group {
                        out.uses.extend(group.attribute_uses);
                        out.groups.extend(group.wildcard);
                    }
                }
                "anyAttribute" => out.local = Some(self.wildcard(at, local.doc)?),
                _ => {}
            }
        }
        Ok(out)
    }

    /// The wildcard `<any>` or `<anyAttribute>` `at` of document `doc`
    /// represents [Schema1 §3.10.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its namespace or processContents is not of its
    /// type [Schema1 Appendix A].
    pub fn wildcard(&self, at: &At<'_>, doc: usize) -> Result<Wildcard> {
        let tns = self.doc(doc)?.tns.as_deref();
        let constraint =
            namespace_constraint(at.attr("namespace"), tns).ok_or_else(|| at.bad("namespace", at.attr("namespace").unwrap_or("")))?;
        let process = match at.token("processContents").as_deref() {
            None | Some("strict") => Process::Strict,
            Some("lax") => Process::Lax,
            Some("skip") => Process::Skip,
            Some(other) => return Err(at.bad("processContents", other)),
        };
        Ok(Wildcard { constraint, process, annotation: annotation(doc, at)? })
    }

    /// The attribute group definition `<attributeGroup>` `place`
    /// represents [Schema1 §3.6.2], read if new.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it refers to itself outside a redefinition
    /// [Schema1 §3.6.3 src-attribute_group.3], the intersection of its
    /// wildcards is not expressible \[2\], or a redefinition of it breaks
    /// [Schema1 §4.2.2 src-redefine.7].
    pub fn attribute_group_at(&mut self, place: Place, depth: usize) -> Result<AttributeGroupId> {
        match self.attribute_groups.get(&place) {
            Some(&(id, State::Done)) => return Ok(id),
            Some((_, State::Filling | State::Pending)) => {
                return Err(invalid("[Schema1 §3.6.3 src-attribute_group.3]", "an attribute group refers to itself"));
            }
            None => {}
        }
        let id = AttributeGroupId(self.schema.attribute_groups.len());
        let at = self.at(place)?;
        let tns = self.doc(place.0)?.tns.clone();
        let name = Expanded::new(tns.as_deref(), &at.ncname("name")?.unwrap_or_default());
        let annotation = annotation(place.0, &at)?;
        self.schema.attribute_groups.push(AttributeGroupDef { name, attribute_uses: Vec::new(), wildcard: None, annotation });
        self.attribute_groups.insert(place, (id, State::Filling));
        let local = Local { doc: place.0, scope: Scope::Absent, depth: Builder::deeper(depth)? };
        let (attributes, references) = self.in_context(place, |b| {
            let attributes = b.attributes(&at.children()?, local)?;
            Ok((attributes, b.self_references))
        })?;
        let wildcard = complete_wildcard(attributes.local, &attributes.groups, "[Schema1 §3.6.3 src-attribute_group.2]")?;
        if let Some(group) = self.schema.attribute_groups.get_mut(id.0) {
            group.attribute_uses = attributes.uses;
            group.wildcard = wildcard;
        }
        self.attribute_groups.insert(place, (id, State::Done));
        self.redefined_attribute_group(place, id, references)?;
        Ok(id)
    }

    /// [Schema1 §4.2.2 src-redefine.7] Attribute group `id`, at `place`,
    /// if it redefines one: it refers to itself at most once \[7.1\], or,
    /// referring to itself never, restricts the one it redefines \[7.2\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it does neither.
    fn redefined_attribute_group(&mut self, place: Place, id: AttributeGroupId, references: usize) -> Result<()> {
        let Some(context) = self.contexts.get(&place).cloned() else { return Ok(()) };
        if references > 1 {
            return Err(invalid("[Schema1 §4.2.2 src-redefine.7.1]", "a redefined attribute group refers to itself at most once"));
        }
        if references == 1 {
            return Ok(());
        }
        let original = self.attribute_group_at(context.original, 0)?;
        let (Some(new), Some(old)) = (self.schema.attribute_groups.get(id.0), self.schema.attribute_groups.get(original.0)) else {
            return Ok(());
        };
        check::attributes_restrict(&self.schema, (&new.attribute_uses, new.wildcard.as_ref()), (&old.attribute_uses, old.wildcard.as_ref()))
            .map_err(|clause| {
                invalid(
                    "[Schema1 §4.2.2 src-redefine.7.2.2]",
                    &format!("a redefined attribute group restricts the one it redefines ({clause})"),
                )
            })
    }
}

/// The `<restriction>` or `<extension>` child of `<simpleContent>` or
/// `<complexContent>` `at`, and which.
///
/// # Errors
///
/// SQLSTATE 22000 if it has not one of them [Schema1 §3.4.2].
fn only_derivation<'t>(at: &At<'t>) -> Result<(At<'t>, &'t str)> {
    let derivations: Vec<(At<'t>, &'t str)> =
        at.children()?.into_iter().filter(|(_, l)| matches!(*l, "restriction" | "extension")).collect();
    match derivations.as_slice() {
        [(d, which)] => Ok((*d, *which)),
        _ => Err(invalid("[Schema1 §3.4.2]", &format!("<{}> has one <restriction> or <extension>", at.local()))),
    }
}

/// The derivation method of `<restriction>` or `<extension>`.
fn method_of(which: &str) -> Method {
    if which == "extension" { Method::Extension } else { Method::Restriction }
}

/// The content type of particle `p`, mixed if `mixed`.
fn pair(mixed: bool, p: Particle) -> ContentType {
    if mixed { ContentType::Mixed(p) } else { ContentType::ElementOnly(p) }
}

/// The complete wildcard [Schema1 §3.4.2] of local wildcard `local` and
/// the non-absent wildcards of attribute groups `groups`: the local one
/// if there are none; else of the local one's, or the first group's,
/// {process contents}, and the intersection of all their constraints.
///
/// # Errors
///
/// SQLSTATE 22000, citing `rule`, if the intersection is not expressible.
fn complete_wildcard(local: Option<Wildcard>, groups: &[Wildcard], rule: &str) -> Result<Option<Wildcard>> {
    let Some(first) = groups.first() else { return Ok(local) };
    let mut constraint = local.as_ref().map_or_else(|| first.constraint.clone(), |l| l.constraint.clone());
    for group in groups {
        constraint = constraint
            .intersection(&group.constraint)
            .ok_or_else(|| invalid(rule, "the intersection of the attribute wildcards is expressible"))?;
    }
    Ok(Some(match local {
        Some(l) => Wildcard { constraint, ..l },
        None => Wildcard { constraint, process: first.process, annotation: None },
    }))
}
