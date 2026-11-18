//! Element and attribute declarations and attribute uses read from
//! `<element>` and `<attribute>` [Schema1 §3.2.2, §3.3.2, §3.5.2], with
//! their representation constraints [Schema1 §3.2.3, §3.3.3] and the
//! constraints on their components that need only them [Schema1 §3.2.6,
//! §3.5.6]; and annotations [Schema1 §3.13.2].

use super::super::builtin::{self, string_valid};
use super::super::component::{
    AnnotationRef, AttributeDecl, AttributeId, AttributeUse, ComplexId, ContentType, Derivations, ElementDecl, ElementId, Expanded,
    Particle, Scope, Simple, Term, TypeDef, ValueConstraint, ValueKind,
};
use super::super::{check, invalid};
use super::attrs::At;
use super::build::{Builder, PendingValue, Place, State, simple_types};
use super::documents::form;
use super::index::Space;
use crate::error::Result;
use crate::xml::{AtomicType, XSI_NAMESPACE};

/// [Schema1 §3.3.3 src-element.2.2] What an `<element>` with ref has not.
const NOT_WITH_REF: [&str; 7] = ["nillable", "default", "fixed", "form", "block", "type", "substitutionGroup"];
/// [Schema1 §3.2.6 no-xmlns] The name no attribute declaration has.
const XMLNS: &str = "xmlns";
/// [Schema1 §3.2.2] The value of use of a required attribute.
const REQUIRED: &str = "required";
/// [Schema1 §3.2.2] The value of use of a prohibited attribute.
const PROHIBITED: &str = "prohibited";
/// [Schema1 §3.2.2] The value of use of an optional attribute.
const OPTIONAL: &str = "optional";
/// [Schema1 §3.11.2] The elements of identity-constraint definitions.
const IDENTITY: [&str; 3] = ["key", "keyref", "unique"];

/// Where a local declaration is read: its document, the {scope} its
/// declarations get, and how deeply it is nested.
#[derive(Debug, Clone, Copy)]
pub(super) struct Local {
    /// The document.
    pub doc: usize,
    /// The scope of declarations read here: the complex type definition
    /// being read, or absent within a named group [Schema1 §3.2.2, §3.3.2].
    pub scope: Scope,
    /// The nesting so far.
    pub depth: usize,
}

/// What an `<attribute>` within a complex type or attribute group
/// corresponds to [Schema1 §3.2.2].
#[derive(Debug, Clone)]
pub(super) enum UseRead {
    /// An attribute use.
    Use(AttributeUse),
    /// Nothing, as it is prohibited; the name it would have had, which a
    /// restriction does not inherit [Schema1 §3.4.2 clause 3.2].
    Prohibited(Expanded),
}

/// The annotation of `at` of document `doc` [Schema1 §3.13.2]: its
/// `<annotation>` child, if it has one.
///
/// # Errors
///
/// As `At::children`.
pub(super) fn annotation(doc: usize, at: &At<'_>) -> Result<Option<AnnotationRef>> {
    Ok(at.children()?.into_iter().find(|(_, l)| *l == "annotation").map(|(a, _)| AnnotationRef { document: doc, node: a.node }))
}

impl Builder<'_> {
    /// The top-level element declaration named `name`, its place taken if
    /// new.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if there is none [Schema1 §3.15.3 src-resolve].
    pub fn element_named(&mut self, name: &Expanded) -> Result<ElementId> {
        let place = self.entry(Space::Element, name).ok_or_else(|| Builder::unresolved(name, "element declaration"))?;
        if let Some(&id) = self.elements.get(&place) {
            return Ok(id);
        }
        let id = self.new_element(name.clone(), Scope::Global);
        self.elements.insert(place, id);
        Ok(id)
    }

    /// A new element declaration named `name`, of `scope`, its other
    /// properties yet to read.
    fn new_element(&mut self, name: Expanded, scope: Scope) -> ElementId {
        let id = ElementId(self.schema.elements.len());
        self.schema.elements.push(ElementDecl {
            name,
            type_def: TypeDef::Complex(ComplexId::ANY_TYPE),
            scope,
            value: None,
            nillable: false,
            constraints: Vec::new(),
            substitution_group: None,
            exclusions: Derivations::NONE,
            disallowed: Derivations::NONE,
            abstract_: false,
            annotation: None,
        });
        self.element_state.push(State::Pending);
        id
    }

    /// Top-level element declaration `id`, its properties read if not
    /// yet [Schema1 §3.3.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its substitution group affiliations are circular
    /// and it has no type [Schema1 §3.3.2], or as `read_element`.
    pub fn complete_element(&mut self, id: ElementId, depth: usize) -> Result<()> {
        match self.element_state.get(id.0).copied() {
            Some(State::Done) | None => Ok(()),
            Some(State::Filling) => Err(invalid("[Schema1 §3.3.2]", "an element's substitution group affiliations are circular")),
            Some(State::Pending) => {
                let place = self.elements.iter().find(|(_, e)| **e == id).map(|(&p, _)| p);
                let place = place.ok_or_else(|| invalid("[Schema1 §3.3.2]", "an element declaration has no element"))?;
                self.set_element_state(id, State::Filling);
                let local = Local { doc: place.0, scope: Scope::Global, depth: Builder::deeper(depth)? };
                self.read_element(id, place, local)?;
                self.set_element_state(id, State::Done);
                Ok(())
            }
        }
    }

    /// Element declaration `id`'s state set to `state`.
    fn set_element_state(&mut self, id: ElementId, state: State) {
        if let Some(s) = self.element_state.get_mut(id.0) {
            *s = state;
        }
    }

    /// The properties of element declaration `id` read from `<element>`
    /// `place` [Schema1 §3.3.2], its name and scope set already.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if both default and fixed are given [Schema1 §3.3.3
    /// src-element.1], or both a type and an anonymous type \[3\].
    fn read_element(&mut self, id: ElementId, place: Place, local: Local) -> Result<()> {
        let at = self.at(place)?;
        let doc = self.doc(place.0)?;
        let children = at.children()?;
        let anonymous = children.iter().find(|(_, l)| matches!(*l, "simpleType" | "complexType")).map(|(a, _)| a.node);
        let named = self.qname(place.0, &at, "type")?;
        if anonymous.is_some() && named.is_some() {
            return Err(invalid("[Schema1 §3.3.3 src-element.3]", "an element has a type or an anonymous type, not both"));
        }
        let head = match self.qname(place.0, &at, "substitutionGroup")? {
            Some(name) => Some(self.element_named(&name)?),
            None => None,
        };
        let type_def = match (anonymous, named, head) {
            (Some(node), _, _) => self.anonymous_type((place.0, node), local)?,
            (None, Some(name), _) => self.type_named(&name)?,
            (None, None, Some(h)) => {
                self.complete_element(h, local.depth)?;
                self.schema.element(h).map_or(TypeDef::Complex(ComplexId::ANY_TYPE), |e| e.type_def)
            }
            (None, None, None) => TypeDef::Complex(ComplexId::ANY_TYPE),
        };
        self.pend_value(id, &at, place)?;
        let mut constraints = Vec::new();
        for (child, _) in children.iter().filter(|(_, l)| IDENTITY.contains(l)) {
            constraints.push(self.read_constraint((place.0, child.node))?);
        }
        let all = Derivations::EXTENSION.with(Derivations::RESTRICTION).with(Derivations::SUBSTITUTION);
        let exclusions = at.derivations("final", Derivations::EXTENSION.with(Derivations::RESTRICTION), doc.final_default)?;
        let disallowed = at.derivations("block", all, doc.block_default)?;
        let annotation = annotation(place.0, &at)?;
        let (nillable, abstract_) = (at.boolean("nillable", false)?, at.boolean("abstract", false)?);
        if let Some(e) = self.schema.elements.get_mut(id.0) {
            e.type_def = type_def;
            e.nillable = nillable;
            e.constraints = constraints;
            e.substitution_group = head;
            e.exclusions = exclusions.within(Derivations::EXTENSION.with(Derivations::RESTRICTION));
            e.disallowed = disallowed.within(all);
            e.abstract_ = abstract_;
            e.annotation = annotation;
        }
        Ok(())
    }

    /// The type of anonymous `<simpleType>` or `<complexType>` `place`,
    /// read.
    ///
    /// # Errors
    ///
    /// As `type_at` and `complete`.
    fn anonymous_type(&mut self, place: Place, local: Local) -> Result<TypeDef> {
        let t = self.type_at(place)?;
        if let TypeDef::Complex(c) = t {
            self.complete(c, local.depth)?;
        }
        Ok(t)
    }

    /// The default or fixed of `<element>` `at`, to read once every type
    /// is built.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if both are given [Schema1 §3.3.3 src-element.1].
    fn pend_value(&mut self, id: ElementId, at: &At<'_>, place: Place) -> Result<()> {
        let value = match (at.attr("default"), at.attr("fixed")) {
            (Some(_), Some(_)) => {
                return Err(invalid("[Schema1 §3.3.3 src-element.1]", "an element has a default or a fixed value, not both"));
            }
            (Some(text), None) => Some((ValueKind::Default, text)),
            (None, Some(text)) => Some((ValueKind::Fixed, text)),
            (None, None) => None,
        };
        if let Some((kind, text)) = value {
            self.values.push(PendingValue { element: id, kind, text: text.to_owned(), at: place });
        }
        Ok(())
    }

    /// The particle a local `<element>` `at` corresponds to [Schema1
    /// §3.3.2]: of a local declaration, or of the top-level one its ref
    /// names; none if minOccurs and maxOccurs are 0.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has both or neither of name and ref [Schema1
    /// §3.3.3 src-element.2.1], or ref and what a reference cannot have
    /// \[2.2\]; as `read_element`.
    pub fn local_element(&mut self, at: &At<'_>, local: Local) -> Result<Option<Particle>> {
        let (min, max) = at.occurs()?;
        let reference = self.qname(local.doc, at, "ref")?;
        let name = at.ncname("name")?;
        let term = match (reference, name) {
            (Some(r), None) => {
                let has_content = at.children()?.iter().any(|(_, l)| *l != "annotation");
                if has_content || NOT_WITH_REF.iter().any(|a| at.attr(a).is_some()) {
                    return Err(invalid("[Schema1 §3.3.3 src-element.2.2]", "an element with ref has no content or other properties"));
                }
                self.element_named(&r)?
            }
            (None, Some(n)) => {
                let doc = self.doc(local.doc)?;
                let qualified = if at.attr("form").is_some() { form(at, "form")? } else { doc.elements_qualified };
                let tns = if qualified { doc.tns.as_deref() } else { None };
                let id = self.new_element(Expanded::new(tns, &n), local.scope);
                self.set_element_state(id, State::Filling);
                self.read_element(id, (local.doc, at.node), Local { depth: Builder::deeper(local.depth)?, ..local })?;
                self.set_element_state(id, State::Done);
                id
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid("[Schema1 §3.3.3 src-element.2.1]", "a local element has one of name and ref"));
            }
        };
        Ok((min != 0 || max != Some(0)).then_some(Particle { min, max, term: Term::Element(term) }))
    }

    /// The top-level attribute declaration named `name`, read if new
    /// [Schema1 §3.2.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if there is none [Schema1 §3.15.3 src-resolve], or as
    /// `read_attribute`.
    pub fn attribute_named(&mut self, name: &Expanded) -> Result<AttributeId> {
        let place = self.entry(Space::Attribute, name).ok_or_else(|| Builder::unresolved(name, "attribute declaration"))?;
        if let Some(&id) = self.attributes.get(&place) {
            return Ok(id);
        }
        let at = self.at(place)?;
        let decl = self.read_attribute(&at, place.0, name.clone(), (Scope::Global, true))?;
        let id = AttributeId(self.schema.attributes.len());
        self.schema.attributes.push(decl);
        self.attributes.insert(place, id);
        Ok(id)
    }

    /// The attribute declaration `<attribute>` `at` of document `doc`
    /// represents, named `name`, of `scope`; with its value constraint if
    /// `valued`, as a top-level one has [Schema1 §3.2.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has a type and an anonymous type [Schema1
    /// §3.2.3 src-attribute.4], is named xmlns [Schema1 §3.2.6 no-xmlns],
    /// is of the xsi namespace \[no-xsi\], or its value constraint breaks
    /// a-props-correct; SQLCODE -900 of an anonymous type.
    fn read_attribute(&mut self, at: &At<'_>, doc: usize, name: Expanded, (scope, valued): (Scope, bool)) -> Result<AttributeDecl> {
        let anonymous = at.children()?.iter().any(|(_, l)| *l == "simpleType");
        let named = self.qname(doc, at, "type")?;
        if anonymous && named.is_some() {
            return Err(invalid("[Schema1 §3.2.3 src-attribute.4]", "an attribute has a type or an anonymous type, not both"));
        }
        if anonymous {
            return Err(simple_types());
        }
        let type_def = match named {
            Some(t) => self.simple_named(&t)?,
            None => Simple::AnySimple,
        };
        if &*name.local == XMLNS {
            return Err(invalid("[Schema1 §3.2.6 no-xmlns]", "no attribute declaration is named xmlns"));
        }
        if name.uri.as_deref() == Some(XSI_NAMESPACE) {
            return Err(invalid("[Schema1 §3.2.6 no-xsi]", "no attribute declaration is of the XML Schema instance namespace"));
        }
        let value = if valued { attribute_value(at, type_def)? } else { None };
        Ok(AttributeDecl { name, type_def, scope, value, annotation: annotation(doc, at)? })
    }

    /// What `<attribute>` `at` within a complex type or attribute group
    /// corresponds to [Schema1 §3.2.2]: an attribute use, of a local
    /// declaration or of the top-level one its ref names, or nothing if
    /// prohibited.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has a default and a use other than optional
    /// [Schema1 §3.2.3 src-attribute.2], both or neither of ref and name
    /// \[3.1\], a ref and what a reference cannot have \[3.2\], or a value
    /// constraint other than its declaration's fixed one [Schema1 §3.5.6
    /// au-props-correct.2]; as `read_attribute`.
    pub fn attribute_use(&mut self, at: &At<'_>, local: Local) -> Result<UseRead> {
        let use_ = at.token("use");
        let required = use_.as_deref() == Some(REQUIRED);
        if at.attr("default").is_some() && use_.as_deref().is_some_and(|u| u != OPTIONAL) {
            return Err(invalid("[Schema1 §3.2.3 src-attribute.2]", "an attribute with a default is optional"));
        }
        let decl = match (self.qname(local.doc, at, "ref")?, at.ncname("name")?) {
            (Some(r), None) => {
                let blocked =
                    ["form", "type"].iter().any(|a| at.attr(a).is_some()) || at.children()?.iter().any(|(_, l)| *l == "simpleType");
                if blocked {
                    return Err(invalid("[Schema1 §3.2.3 src-attribute.3.2]", "an attribute with ref has no form, type or anonymous type"));
                }
                self.attribute_named(&r)?
            }
            (None, Some(n)) => {
                let doc = self.doc(local.doc)?;
                let qualified = if at.attr("form").is_some() { form(at, "form")? } else { doc.attributes_qualified };
                let tns = if qualified { doc.tns.as_deref() } else { None };
                let decl = self.read_attribute(at, local.doc, Expanded::new(tns, &n), (local.scope, false))?;
                let id = AttributeId(self.schema.attributes.len());
                self.schema.attributes.push(decl);
                id
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid("[Schema1 §3.2.3 src-attribute.3.1]", "a local attribute has one of name and ref"));
            }
        };
        let declared =
            self.schema.attribute(decl).cloned().ok_or_else(|| invalid("[Schema1 §3.5.1]", "an attribute declaration is missing"))?;
        if use_.as_deref() == Some(PROHIBITED) {
            return Ok(UseRead::Prohibited(declared.name));
        }
        let value = attribute_value(at, declared.type_def)?;
        if let Some(fixed) = declared.value.as_ref().filter(|v| v.kind == ValueKind::Fixed) {
            let same = value.as_ref().is_none_or(|v| v.kind == ValueKind::Fixed && v.value.equal(&fixed.value));
            if !same {
                return Err(invalid("[Schema1 §3.5.6 au-props-correct.2]", "a use of an attribute of a fixed value fixes the same value"));
            }
        }
        Ok(UseRead::Use(AttributeUse { required, decl, value }))
    }

    /// Each element declaration's value constraint read against its type
    /// [Schema1 §3.3.2], once every type is built.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if one is not valid for its type [Schema1 §3.3.6
    /// e-props-correct.2, cos-valid-default], or its type is or is derived
    /// from ID \[4\].
    pub fn finish_values(&mut self) -> Result<()> {
        for pending in std::mem::take(&mut self.values) {
            let at = self.at(pending.at)?;
            let Some(element) = self.schema.element(pending.element) else { continue };
            let content = match element.type_def {
                TypeDef::Simple(s) => ContentType::Simple(s),
                TypeDef::Complex(c) => self.schema.complex(c).map_or(ContentType::Empty, |t| t.content.clone()),
            };
            let simple = match &content {
                ContentType::Simple(s) => Some(*s),
                ContentType::Mixed(p) => check::emptiable(&self.schema, p).then_some(Simple::Atomic(AtomicType::String)),
                ContentType::ElementOnly(_) | ContentType::Empty => None,
            };
            let simple = simple.ok_or_else(|| {
                invalid("[Schema1 §3.3.6 cos-valid-default.2]", "an element with a value constraint has simple or emptiable mixed content")
            })?;
            if builtin::is_id(simple) {
                return Err(invalid("[Schema1 §3.3.6 e-props-correct.4]", "an element of an ID type has no value constraint"));
            }
            let value = string_valid(simple, &pending.text, &at.scope())?.ok_or_else(|| {
                invalid("[Schema1 §3.3.6 e-props-correct.2]", &format!("'{}' is no value of the element's type", pending.text))
            })?;
            if let Some(e) = self.schema.elements.get_mut(pending.element.0) {
                e.value = Some(ValueConstraint { kind: pending.kind, value });
            }
        }
        Ok(())
    }
}

/// The value constraint of `<attribute>` `at`, its default or fixed read
/// against `type_def` [Schema1 §3.2.2].
///
/// # Errors
///
/// SQLSTATE 22000 if it has both [Schema1 §3.2.3 src-attribute.1], its
/// type is or is derived from ID [Schema1 §3.2.6 a-props-correct.3], or
/// the value is not valid for it \[2\].
fn attribute_value(at: &At<'_>, type_def: Simple) -> Result<Option<ValueConstraint>> {
    let (kind, text) = match (at.attr("default"), at.attr("fixed")) {
        (Some(_), Some(_)) => {
            return Err(invalid("[Schema1 §3.2.3 src-attribute.1]", "an attribute has a default or a fixed value, not both"));
        }
        (Some(text), None) => (ValueKind::Default, text),
        (None, Some(text)) => (ValueKind::Fixed, text),
        (None, None) => return Ok(None),
    };
    if builtin::is_id(type_def) {
        return Err(invalid("[Schema1 §3.2.6 a-props-correct.3]", "an attribute of an ID type has no value constraint"));
    }
    let value = string_valid(type_def, text, &at.scope())?
        .ok_or_else(|| invalid("[Schema1 §3.2.6 a-props-correct.2]", &format!("'{text}' is no value of the attribute's type")))?;
    Ok(Some(ValueConstraint { kind, value }))
}
