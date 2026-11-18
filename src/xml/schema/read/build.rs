//! The components of a schema built from the elements of its documents
//! [Schema1 §3.x.2], each named one as it is first needed and every one
//! in the end; a reference resolved by QName resolution (Schema Document)
//! [Schema1 §3.15.3].
//!
//! \[DESIGN\] A named component gets its place in the schema's table when
//! first referred to, and is filled in when its properties are needed:
//! a complex type's when another derives from it or uses its content, an
//! element's when its substitution group needs its type. A component
//! needed while it is being filled is a circular definition. Element
//! value constraints, which may need the content type of a type still
//! being filled, are read once every type is.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::builtin;
use super::super::component::{
    AttributeGroupId, AttributeId, ComplexId, ComplexType, Compositor, ConstraintId, ContentType, ElementId, Expanded, GroupDefId, GroupId,
    ModelGroup, ModelGroupDef, Notation, NotationId, Schema, Simple, TypeDef, ValueKind,
};
use super::super::{MAX_DEPTH, beyond, invalid};
use super::attrs::At;
use super::documents::Doc;
use super::element::annotation;
use super::index::{Entry, Index, Space};
use crate::error::{Result, SqlError};
use crate::xml::{AtomicType, NodeId, XS_NAMESPACE};

/// Where a component is represented: a document's place and an element.
pub(super) type Place = (usize, NodeId);

/// How far a component is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// Its place is taken; its properties are not yet read.
    Pending,
    /// Its properties are being read.
    Filling,
    /// Built.
    Done,
}

/// An element declaration's value constraint, read once every type is
/// built.
#[derive(Debug, Clone)]
pub(super) struct PendingValue {
    /// The declaration.
    pub element: ElementId,
    /// default or fixed.
    pub kind: ValueKind,
    /// The attribute's value.
    pub text: String,
    /// The `<element>`, whose namespaces in scope resolve a QName.
    pub at: Place,
}

/// A component of a redefinition whose own name, referred to within it,
/// names the component it redefines [Schema1 §4.2.2 clause 1.2, 2].
#[derive(Debug, Clone)]
pub(super) struct Redefining {
    /// The symbol space.
    pub space: Space,
    /// The name.
    pub name: Expanded,
    /// The definition redefined.
    pub original: Place,
}

/// A schema being built.
#[derive(Debug)]
pub(super) struct Builder<'a> {
    /// The documents.
    pub docs: &'a [Doc],
    /// Their top-level components.
    pub index: &'a Index,
    /// The schema so far.
    pub schema: Schema,
    /// Each complex type definition's place, by its element.
    pub complex: BTreeMap<Place, ComplexId>,
    /// How far each complex type definition is built, by its place.
    pub complex_state: Vec<State>,
    /// Each top-level element declaration's place, by its element.
    pub elements: BTreeMap<Place, ElementId>,
    /// How far each element declaration is built, by its place.
    pub element_state: Vec<State>,
    /// Each top-level attribute declaration's place, by its element.
    pub attributes: BTreeMap<Place, AttributeId>,
    /// Each attribute group definition's place and state, by its element.
    pub attribute_groups: BTreeMap<Place, (AttributeGroupId, State)>,
    /// Each model group definition's place, by its element.
    pub group_defs: BTreeMap<Place, GroupDefId>,
    /// Each notation declaration's place, by its element.
    pub notations: BTreeMap<Place, NotationId>,
    /// Each identity-constraint definition's place, by its element.
    pub constraints: BTreeMap<Place, ConstraintId>,
    /// The redefinitions, by their element.
    pub contexts: BTreeMap<Place, Redefining>,
    /// The definitions a `<redefine>` replaces, which are anonymous
    /// [Schema1 §4.2.2 clause 1.1].
    pub originals: Vec<Place>,
    /// The redefinition being filled, if one is.
    pub redefining: Option<Redefining>,
    /// How many times the redefinition being filled has referred to its
    /// own name [Schema1 §4.2.2 src-redefine.6.1.1, 7.1].
    pub self_references: usize,
    /// Element value constraints yet to read.
    pub values: Vec<PendingValue>,
    /// Keyrefs and the name each refers to, yet to resolve.
    pub refers: Vec<(ConstraintId, Expanded)>,
}

impl<'a> Builder<'a> {
    /// A schema of `docs`, indexed as `index`, of `xs:anyType` alone so far.
    pub fn new(docs: &'a [Doc], index: &'a Index) -> Builder<'a> {
        let mut schema = Schema::default();
        schema.groups.push(ModelGroup { compositor: Compositor::Sequence, particles: vec![builtin::any_content()], annotation: None });
        schema.complex_types.push(builtin::any_type(GroupId(0)));
        schema.automata.push(None);
        schema.documents = docs.iter().map(|d| (Arc::clone(&d.tree), d.location.clone())).collect();
        let mut contexts = BTreeMap::new();
        let mut originals = Vec::new();
        for ((space, name), entry) in &index.entries {
            if let Some(original) = entry.original {
                contexts.insert((entry.doc, entry.node), Redefining { space: *space, name: name.clone(), original });
                originals.push(original);
            }
        }
        Builder {
            docs,
            index,
            schema,
            complex: BTreeMap::new(),
            complex_state: vec![State::Done],
            elements: BTreeMap::new(),
            element_state: Vec::new(),
            attributes: BTreeMap::new(),
            attribute_groups: BTreeMap::new(),
            group_defs: BTreeMap::new(),
            notations: BTreeMap::new(),
            constraints: BTreeMap::new(),
            contexts,
            originals,
            redefining: None,
            self_references: 0,
            values: Vec::new(),
            refers: Vec::new(),
        }
    }

    /// The schema: every top-level component built, then element value
    /// constraints read, keyrefs resolved, and the names of the top-level
    /// components recorded.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if a component breaks a constraint on its
    /// representation, or a reference resolves to nothing; SQLCODE -900
    /// past `MAX_DEPTH`.
    pub fn build(mut self) -> Result<Schema> {
        let entries: Vec<((Space, Expanded), Entry)> = self.index.entries.iter().map(|(k, e)| (k.clone(), *e)).collect();
        for ((space, name), entry) in &entries {
            self.top_level(*space, name, (entry.doc, entry.node))?;
        }
        if self.index.constraints.values().any(|place| !self.constraints.contains_key(place)) {
            return Err(invalid("[Schema1 §3.11.2]", "an identity-constraint definition is a child of an <element>"));
        }
        self.finish_values()?;
        self.finish_refers()?;
        for ((space, name), entry) in &entries {
            self.record(*space, name, (entry.doc, entry.node));
        }
        for (name, place) in &self.index.constraints {
            if let Some(&id) = self.constraints.get(place) {
                self.schema.constraint_names.insert(name.clone(), id);
            }
        }
        Ok(self.schema)
    }

    /// The top-level component at `place`, of `space` and named `name`,
    /// built.
    fn top_level(&mut self, space: Space, name: &Expanded, place: Place) -> Result<()> {
        match space {
            Space::Type => {
                if let TypeDef::Complex(id) = self.type_at(place)? {
                    self.complete(id, 0)?;
                }
            }
            Space::Element => {
                let id = self.element_named(name)?;
                self.complete_element(id, 0)?;
            }
            Space::Attribute => {
                self.attribute_named(name)?;
            }
            Space::AttributeGroup => {
                self.attribute_group_at(place, 0)?;
            }
            Space::Group => {
                self.group_def_at(place, 0)?;
            }
            Space::Notation => {
                self.notation_at(place)?;
            }
        }
        Ok(())
    }

    /// `read` run with the redefinition context of the component at
    /// `place`, none if it is no redefinition, the context before it kept.
    pub fn in_context<T>(&mut self, place: Place, read: impl FnOnce(&mut Builder<'a>) -> Result<T>) -> Result<T> {
        let context = self.contexts.get(&place).cloned();
        let saved = std::mem::replace(&mut self.redefining, context);
        let references = std::mem::take(&mut self.self_references);
        let out = read(self);
        self.redefining = saved;
        self.self_references = references;
        out
    }

    /// The document `doc`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if there is none, which no place built here names.
    pub fn doc(&self, doc: usize) -> Result<&'a Doc> {
        self.docs.get(doc).ok_or_else(|| invalid("[Schema1 §4.2]", "a schema document is missing"))
    }

    /// The element `place` as an `At`.
    ///
    /// # Errors
    ///
    /// As `doc`.
    pub fn at(&self, place: Place) -> Result<At<'a>> {
        Ok(At::new(&self.doc(place.0)?.tree, place.1))
    }

    /// `depth`, one deeper.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `MAX_DEPTH`.
    pub fn deeper(depth: usize) -> Result<usize> {
        let next = depth.saturating_add(1);
        if next > MAX_DEPTH {
            return Err(beyond("its declarations nest too deeply"));
        }
        Ok(next)
    }

    /// The QName of attribute `attr` of `at` in document `doc`, resolved
    /// by QName Interpretation [Schema1 §3.15.3]: an absent namespace of a
    /// document read into another's namespace is that namespace [Schema1
    /// §4.2.1 NOTE]. None if it has no such attribute.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if its prefix is not in scope [Schema1 §3.15.3
    /// clause 1.1], or its namespace is neither the document's target
    /// namespace nor one it imports [Schema1 §3.15.3 src-resolve.4].
    pub fn qname(&self, doc: usize, at: &At<'_>, attr: &str) -> Result<Option<Expanded>> {
        let Some(text) = at.token(attr) else { return Ok(None) };
        let d = self.doc(doc)?;
        let mut name = builtin::resolve(&text, &at.scope())
            .ok_or_else(|| invalid("[Schema1 §3.15.3]", &format!("'{text}' is no QName whose prefix is in scope")))?;
        if d.chameleon && name.uri.is_none() {
            name.uri.clone_from(&d.tns);
        }
        let known = name.uri == d.tns || name.uri.as_deref() == Some(XS_NAMESPACE) || d.imports.contains(&name.uri);
        if !known {
            return Err(invalid(
                "[Schema1 §3.15.3 src-resolve.4]",
                &format!("{} is of a namespace the document does not import", name.text()),
            ));
        }
        Ok(Some(name))
    }

    /// The error of `name` resolving to no component of `what`.
    pub fn unresolved(name: &Expanded, what: &str) -> SqlError {
        invalid("[Schema1 §3.15.3 src-resolve]", &format!("{} names no {what}", name.text()))
    }

    /// The definition redefined by the component being filled, if `name`
    /// of `space` is its own name [Schema1 §4.2.2].
    pub fn original_of(&self, space: Space, name: &Expanded) -> Option<Place> {
        self.redefining.as_ref().filter(|r| r.space == space && r.name == *name).map(|r| r.original)
    }

    /// The top-level component of `space` named `name`: the one it
    /// redefines if the redefinition being filled is named so, else the
    /// index's.
    pub fn entry(&mut self, space: Space, name: &Expanded) -> Option<Place> {
        if let Some(original) = self.original_of(space, name) {
            self.self_references = self.self_references.saturating_add(1);
            return Some(original);
        }
        self.index.get(space, name).map(|e| (e.doc, e.node))
    }

    /// The type definition named `name` [Schema1 §3.15.3]: a built-in one
    /// of the XML Schema namespace, else a top-level one.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if there is none; as `type_at`.
    pub fn type_named(&mut self, name: &Expanded) -> Result<TypeDef> {
        if name.uri.as_deref() == Some(XS_NAMESPACE) {
            return builtin::named(&name.local).ok_or_else(|| Builder::unresolved(name, "built-in type definition"));
        }
        let place = self.entry(Space::Type, name).ok_or_else(|| Builder::unresolved(name, "type definition"))?;
        self.type_at(place)
    }

    /// The type definition `<simpleType>` or `<complexType>` `place`
    /// represents: a complex one's place, taken if new.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 of a `<simpleType>` (`simple_types`).
    pub fn type_at(&mut self, place: Place) -> Result<TypeDef> {
        if self.at(place)?.xs_name() == Some("simpleType") {
            return Err(simple_types());
        }
        if let Some(&id) = self.complex.get(&place) {
            return Ok(TypeDef::Complex(id));
        }
        let id = ComplexId(self.schema.complex_types.len());
        let mut placeholder = builtin::any_type(GroupId(0));
        placeholder.name = None;
        self.schema.complex_types.push(placeholder);
        self.schema.automata.push(None);
        self.complex_state.push(State::Pending);
        self.complex.insert(place, id);
        Ok(TypeDef::Complex(id))
    }

    /// The simple type definition named `name`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it names a complex type or nothing [Schema1
    /// §3.2.2], or `xs:NOTATION`, used only through a type restricting it
    /// [Schema2 §3.2.19]; as `type_named`.
    pub fn simple_named(&mut self, name: &Expanded) -> Result<Simple> {
        match self.type_named(name)? {
            TypeDef::Simple(Simple::Atomic(AtomicType::Notation)) => {
                Err(invalid("[Schema2 §3.2.19]", "NOTATION is used only through a type restricting it by enumeration"))
            }
            TypeDef::Simple(s) => Ok(s),
            TypeDef::Complex(_) => Err(invalid("[Schema1 §3.2.2]", &format!("{} is no simple type definition", name.text()))),
        }
    }

    /// Complex type `id`, its properties read if not yet.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it is needed while being read, a circular
    /// definition [Schema1 §3.4.6 ct-props-correct.3]; as its reading.
    pub fn complete(&mut self, id: ComplexId, depth: usize) -> Result<()> {
        match self.complex_state.get(id.0).copied() {
            Some(State::Done) | None => Ok(()),
            Some(State::Filling) => Err(invalid("[Schema1 §3.4.6 ct-props-correct.3]", "a type definition derives from itself")),
            Some(State::Pending) => {
                let place = self.complex.iter().find(|(_, c)| **c == id).map(|(&p, _)| p);
                let place = place.ok_or_else(|| invalid("[Schema1 §3.4.2]", "a type definition has no element"))?;
                self.set_complex_state(id, State::Filling);
                let anonymous = self.originals.contains(&place);
                let deeper = Builder::deeper(depth)?;
                let def = self.in_context(place, |b| b.read_complex(place, id, anonymous, deeper))?;
                if let Some(slot) = self.schema.complex_types.get_mut(id.0) {
                    *slot = def;
                }
                self.set_complex_state(id, State::Done);
                Ok(())
            }
        }
    }

    /// Complex type `id`'s state set to `state`.
    fn set_complex_state(&mut self, id: ComplexId, state: State) {
        if let Some(s) = self.complex_state.get_mut(id.0) {
            *s = state;
        }
    }

    /// Complex type `id`, its properties read if not yet.
    ///
    /// # Errors
    ///
    /// As `complete`.
    pub fn complex_of(&mut self, id: ComplexId, depth: usize) -> Result<ComplexType> {
        self.complete(id, depth)?;
        self.schema.complex(id).cloned().ok_or_else(|| invalid("[Schema1 §3.4.1]", "a type definition is missing"))
    }

    /// The {content type} of type definition `t`: a simple type's itself.
    ///
    /// # Errors
    ///
    /// As `complete`.
    pub fn content_of(&mut self, t: TypeDef, depth: usize) -> Result<ContentType> {
        match t {
            TypeDef::Simple(s) => Ok(ContentType::Simple(s)),
            TypeDef::Complex(id) => Ok(self.complex_of(id, depth)?.content),
        }
    }

    /// The model group definition `<group>` `place` represents, its model
    /// group's place taken and the group read if new [Schema1 §3.7.2].
    ///
    /// # Errors
    ///
    /// As `read_group_def`.
    pub fn group_def_at(&mut self, place: Place, depth: usize) -> Result<GroupDefId> {
        if let Some(&id) = self.group_defs.get(&place) {
            return Ok(id);
        }
        let id = GroupDefId(self.schema.group_defs.len());
        let group = GroupId(self.schema.groups.len());
        self.schema.groups.push(ModelGroup { compositor: Compositor::Sequence, particles: Vec::new(), annotation: None });
        self.schema.group_defs.push(ModelGroupDef { name: Expanded::new(None, ""), group, annotation: None });
        self.group_defs.insert(place, id);
        let deeper = Builder::deeper(depth)?;
        self.in_context(place, |b| b.read_group_def(place, id, deeper))?;
        Ok(id)
    }

    /// The notation declaration `<notation>` `place` represents [Schema1
    /// §3.12.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 22000 if it has neither a public nor a system identifier
    /// [Schema1 §3.12.1].
    pub fn notation_at(&mut self, place: Place) -> Result<NotationId> {
        if let Some(&id) = self.notations.get(&place) {
            return Ok(id);
        }
        let at = self.at(place)?;
        let tns = self.doc(place.0)?.tns.as_deref();
        let name = at.ncname("name")?.unwrap_or_default();
        let public = at.attr("public").map(Into::into);
        let system = at.attr("system").map(Into::into);
        if public.is_none() && system.is_none() {
            return Err(invalid("[Schema1 §3.12.1]", &format!("notation {name} has a public or a system identifier")));
        }
        let annotation = annotation(place.0, &at)?;
        let id = NotationId(self.schema.notations.len());
        self.schema.notations.push(Notation { name: Expanded::new(tns, &name), system, public, annotation });
        self.notations.insert(place, id);
        Ok(id)
    }

    /// The name of the top-level component at `place`, of `space`,
    /// recorded in its symbol space [Schema1 §3.15.1].
    fn record(&mut self, space: Space, name: &Expanded, place: Place) {
        let schema = &mut self.schema;
        let name = name.clone();
        match space {
            Space::Type => {
                if let Some(&id) = self.complex.get(&place) {
                    schema.type_names.insert(name, TypeDef::Complex(id));
                }
            }
            Space::Element => {
                if let Some(&id) = self.elements.get(&place) {
                    schema.element_names.insert(name, id);
                }
            }
            Space::Attribute => {
                if let Some(&id) = self.attributes.get(&place) {
                    schema.attribute_names.insert(name, id);
                }
            }
            Space::AttributeGroup => {
                if let Some(&(id, _)) = self.attribute_groups.get(&place) {
                    schema.attribute_group_names.insert(name, id);
                }
            }
            Space::Group => {
                if let Some(&id) = self.group_defs.get(&place) {
                    schema.group_names.insert(name, id);
                }
            }
            Space::Notation => {
                if let Some(&id) = self.notations.get(&place) {
                    schema.notation_names.insert(name, id);
                }
            }
        }
    }
}

/// The error of a user-defined simple type definition: \[DESIGN\] they
/// wait for XML Schema Part 2's facets [Schema2 §4.1, §4.3].
pub(super) fn simple_types() -> SqlError {
    beyond("user-defined simple type definitions [Schema2 §4.1] are not yet supported")
}
