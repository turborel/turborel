//! An element assessed [Schema1 §3.3.4]: the declaration or type it is
//! assessed by (Schema-Validity Assessment (Element)), then Element
//! Locally Valid (Element) — abstract, `xsi:nil`, `xsi:type`, value
//! constraints — and Element Locally Valid (Type); an element of a simple
//! type or of simple content made whole at once, one of complex content
//! opened for its children. Its annotation is its type's [XQueryDM
//! §3.3.1.1].

use super::super::Variety;
use super::super::automaton::Automaton;
use super::super::builtin::{self, string_valid};
use super::super::check::type_derives;
use super::super::component::{ComplexId, ContentType, Derivations, ElementDecl, Expanded, Schema, Simple, TypeDef, Typed, ValueKind};
use super::attributes::Attrs;
use super::run::{Assess, Content, Frame, Run};
use super::{Failure, Mode, Step, UNDECLARED};
use crate::xml::{Annotation, AtomicType, ElementStart, Kind, NodeId, QName, Typing, XS_NAMESPACE, XSI_NAMESPACE};

/// [Schema1 §3.2.7] The local names of the built-in attributes of the
/// XML Schema instance namespace.
pub(super) const XSI_TYPE: &str = "type";
/// `xsi:nil`.
pub(super) const XSI_NIL: &str = "nil";
/// \[DESIGN\] The prefix of the local name an anonymous complex type
/// definition is annotated with, `#anonymous-` and its place, which no
/// NCName is, so no named type has it [XQueryDM §3.3].
const ANONYMOUS: &str = "#anonymous-";
/// \[DESIGN\] The prefix of the local name an anonymous simple type
/// definition is annotated with, `#anonymous-simple-` and its place, so no
/// anonymous complex type's clashes with it.
const ANONYMOUS_SIMPLE: &str = "#anonymous-simple-";

/// The expanded name of node `id` of `tree`.
pub(super) fn expanded(tree: &crate::xml::Tree, id: NodeId) -> Expanded {
    tree.name(id).map_or_else(|| Expanded::new(None, ""), |n| Expanded::new(n.uri.as_deref(), &n.local))
}

/// The name of type definition `t` of `schema` as a node's type-name
/// [XQueryDM §3.3.1.1], an anonymous one's made from its place.
pub(super) fn type_name(schema: &Schema, t: TypeDef) -> QName {
    let name = match t {
        TypeDef::Simple(Simple::Defined(id)) => schema.simple(id).map_or_else(
            || Expanded::new(None, ANONYMOUS_SIMPLE),
            |d| d.name.clone().unwrap_or_else(|| Expanded::new(d.namespace.as_deref(), &format!("{ANONYMOUS_SIMPLE}{}", id.0))),
        ),
        TypeDef::Simple(s) => builtin::simple_name(schema, s).unwrap_or_else(builtin::any_type_name),
        TypeDef::Complex(c) => schema.complex(c).map_or_else(builtin::any_type_name, |d| {
            d.name.clone().unwrap_or_else(|| Expanded::new(d.namespace.as_deref(), &format!("{ANONYMOUS}{}", c.0)))
        }),
    };
    QName { prefix: None, uri: name.uri, local: name.local }
}

/// How the typed value of a node of simple type `s` of `schema`, of value
/// `value` if it has one, is determined [XQueryDM §3.3.1.2]: by its
/// atomic type, or a list's atomic item type; else, as a union's member
/// varies from node to node, by the kind of the atomic values validating
/// the node gave, one of an atomic type, more of a list, none the empty
/// sequence. \[DESIGN\] A list whose items are of several kinds, of a
/// union, is typed as `xs:anySimpleType`, its typed value
/// `xdt:untypedAtomic`, as an annotation holds one item type.
pub(super) fn simple_typing(schema: &Schema, s: Simple, value: Option<&Typed>) -> Typing {
    let id = match s {
        Simple::AnySimple => return Typing::AnySimple,
        Simple::Atomic(a) => return Typing::Atomic(a),
        Simple::List(a) => return Typing::List(a),
        Simple::Defined(id) => id,
    };
    let def = schema.simple(id).map(|d| &d.def);
    let fixed = match def.map(|d| d.variety()) {
        Some(Variety::Atomic(a)) => Some(Typing::Atomic(*a)),
        Some(Variety::List(item)) => item.atomic().map(Typing::List),
        Some(Variety::Union(_)) | None => None,
    };
    fixed.unwrap_or_else(|| value.map_or(Typing::AnySimple, |v| typing_of(&v.atoms)))
}

/// [XQueryDM §3.3.1.2] The typing of a node whose typed value is `atoms`.
fn typing_of(atoms: &[crate::xml::Atomic]) -> Typing {
    match atoms {
        [] => Typing::Empty,
        [one] => Typing::Atomic(one.kind),
        [first, rest @ ..] => {
            if rest.iter().all(|a| a.kind == first.kind) {
                Typing::List(first.kind)
            } else {
                Typing::AnySimple
            }
        }
    }
}

/// The annotation of a node of type `t` [XQueryDM §3.3.1, §6.2.4], of
/// value `value` if it is of a simple type or simple content and has one.
pub(super) fn annotation(schema: &Schema, t: TypeDef, value: Option<&Typed>) -> Annotation {
    let typing = match t {
        TypeDef::Simple(s) => simple_typing(schema, s, value),
        TypeDef::Complex(c) => match schema.complex(c).map(|d| &d.content) {
            Some(ContentType::Simple(s)) => simple_typing(schema, *s, value),
            Some(ContentType::Empty) => Typing::Empty,
            Some(ContentType::ElementOnly(_)) => Typing::ElementOnly,
            Some(ContentType::Mixed(_)) | None => Typing::Mixed,
        },
    };
    Annotation { name: type_name(schema, t), typing }
}

/// What a declaration asks of an element it validates.
#[derive(Debug, Clone, Copy)]
pub(super) struct Declared<'s> {
    /// The declaration.
    pub decl: &'s ElementDecl,
    /// Whether the element is nilled [Schema1 §3.3.4 cvc-elt.3.2].
    pub nil: bool,
}

impl<'s> Run<'s> {
    /// The value of attribute `local` of the XML Schema instance namespace
    /// of element `id`, if it has one.
    pub(super) fn xsi(&self, id: NodeId, local: &str) -> Option<&'s str> {
        self.tree
            .attributes(id)
            .find(|&a| self.tree.name(a).is_some_and(|n| n.uri.as_deref() == Some(XSI_NAMESPACE) && &*n.local == local))
            .map(|a| self.tree.content(a))
    }

    /// The type definition the `xsi:type` of element `id` names, if it
    /// has one [Schema1 §3.3.4 cvc-elt.4.1, 4.2].
    fn xsi_type(&self, id: NodeId) -> Step<Option<TypeDef>> {
        let Some(text) = self.xsi(id, XSI_TYPE) else { return Ok(None) };
        let scope = self.tree.in_scope(id);
        let Some(name) = builtin::resolve(text.trim(), &scope) else {
            return Err(
                Failure::new("[Schema1 §3.3.4 cvc-elt.4.1]", format!("xsi:type '{text}' is no QName whose prefix is in scope")).into()
            );
        };
        let found = if name.uri.as_deref() == Some(XS_NAMESPACE) {
            builtin::named(&name.local)
        } else {
            self.schema.type_names.get(&name).copied()
        };
        let found = found.filter(|t| *t != TypeDef::Simple(Simple::Atomic(AtomicType::Notation)));
        found.map(Some).ok_or_else(|| {
            Failure::new("[Schema1 §3.3.4 cvc-elt.4.2]", format!("xsi:type {} names no type definition", name.text())).into()
        })
    }

    /// [Schema1 §3.3.4 cvc-assess-elt] Element `input` entered to be
    /// assessed as `assess` says; the frame of its children if it is open
    /// for them, none if it was made whole.
    pub(super) fn enter(&mut self, input: NodeId, assess: Assess) -> Step<Option<Frame<'s>>> {
        let name = expanded(self.tree, input);
        let global = self.schema.element_names.get(&name).copied();
        let decl = match assess {
            Assess::Skip => return self.unassessed(input, Assess::Skip),
            Assess::Decl(d) => Some(d),
            Assess::Strict | Assess::Lax | Assess::Root(Mode::Lax) => global,
            Assess::Root(Mode::Strict) => {
                let why = format!("{} has no top-level element declaration", name.text());
                Some(global.ok_or_else(|| Failure { rule: "[XQuery §3.13]".into(), why, code: UNDECLARED })?)
            }
        };
        let local = self.xsi_type(input)?;
        match (decl.and_then(|d| self.schema.element(d).map(|e| (d, e))), local) {
            (Some((id, decl)), _) => self.by_declaration(input, (id, decl), local),
            (None, Some(t)) => self.by_type(input, t, None),
            (None, None) => {
                if assess == Assess::Strict {
                    let why = format!("{} has no declaration and no xsi:type", name.text());
                    return Err(Failure::new("[Schema1 §3.3.4 cvc-assess-elt.1.1.1]", why).into());
                }
                self.unassessed(input, Assess::Lax)
            }
        }
    }

    /// [Schema1 §3.3.4 cvc-elt] Element Locally Valid (Element): element
    /// `input` by declaration `decl`, of `xsi:type` `local` if it has one.
    fn by_declaration(
        &mut self,
        input: NodeId,
        (id, decl): (super::super::component::ElementId, &'s ElementDecl),
        local: Option<TypeDef>,
    ) -> Step<Option<Frame<'s>>> {
        if decl.abstract_ {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.2]", format!("{} is declared abstract", decl.name.text())).into());
        }
        let nil = self.nil(input, decl)?;
        let actual = match local {
            Some(t) => {
                let prohibited = match decl.type_def {
                    TypeDef::Complex(c) => self.schema.complex(c).map_or(Derivations::NONE, |d| d.prohibited),
                    TypeDef::Simple(_) => Derivations::NONE,
                };
                let blocked = decl.disallowed.with(prohibited).within(Derivations::EXTENSION.with(Derivations::RESTRICTION));
                if !type_derives(self.schema, t, decl.type_def, blocked) {
                    return Err(
                        Failure::new("[Schema1 §3.3.4 cvc-elt.4.3]", "the xsi:type is not validly derived from the declared type").into()
                    );
                }
                t
            }
            None => decl.type_def,
        };
        self.decls_pending = Some(id);
        self.by_type(input, actual, Some(Declared { decl, nil }))
    }

    /// [Schema1 §3.3.4 cvc-elt.3] Whether element `input` is nilled by
    /// `decl`: its `xsi:nil` is true, which only a nillable declaration
    /// allows, and then it has no element or character child and no fixed
    /// value constraint.
    fn nil(&self, input: NodeId, decl: &ElementDecl) -> Step<bool> {
        let Some(text) = self.xsi(input, XSI_NIL) else { return Ok(false) };
        if !decl.nillable {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.3.1]", format!("{} is not nillable", decl.name.text())).into());
        }
        let Some(value) = string_valid(self.schema, Simple::Atomic(AtomicType::Boolean), text, &[])? else {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.3.2]", format!("xsi:nil '{text}' is no boolean")).into());
        };
        let nil = value.atoms.first().is_some_and(|a| &*a.lexical == "true");
        if !nil {
            return Ok(false);
        }
        let filled = self.tree.children(input).any(|c| matches!(self.tree.kind(c), Some(Kind::Element | Kind::Text)));
        if filled {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.3.2.1]", "a nilled element has no element or character child").into());
        }
        if decl.value.as_ref().is_some_and(|v| v.kind == ValueKind::Fixed) {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.3.2.2]", "an element of a fixed value is not nilled").into());
        }
        Ok(true)
    }

    /// [Schema1 §3.3.4 cvc-type] Element Locally Valid (Type): element
    /// `input` by type `t`, as declaration `declared` asks if it has one.
    pub(super) fn by_type(&mut self, input: NodeId, t: TypeDef, declared: Option<Declared<'s>>) -> Step<Option<Frame<'s>>> {
        let (content, abstract_) = match t {
            TypeDef::Simple(s) => (ContentType::Simple(s), false),
            TypeDef::Complex(c) => self.schema.complex(c).map_or((ContentType::Empty, false), |d| (d.content.clone(), d.abstract_)),
        };
        if abstract_ {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-type.2]", "an element is not validated by an abstract type").into());
        }
        let attrs = match t {
            TypeDef::Simple(_) => self.simple_attributes(input)?,
            TypeDef::Complex(c) => self.complex_attributes(input, c)?,
        };
        match content {
            ContentType::Simple(s) => self.simple_element(input, (t, s), attrs, declared).map(|()| None),
            ContentType::Empty | ContentType::Mixed(_) | ContentType::ElementOnly(_) => {
                self.complex_element(input, t, attrs, declared).map(Some)
            }
        }
    }

    /// The start of element `input` made, with `attrs`, annotated as of
    /// type `t` and nilled if `nil`, and recorded as validated by the
    /// declaration pending, if any.
    pub(super) fn start(&mut self, input: NodeId, attrs: Attrs, (annotation, nil): (Annotation, bool)) -> Step<NodeId> {
        let name = self.tree.name(input).cloned().unwrap_or_else(|| QName::local(""));
        let bindings = if self.out.depth() == 0 && self.tree.parent(input).is_some_and(|p| self.tree.kind(p) == Some(Kind::Element)) {
            let scope = self.tree.in_scope(input).into_iter().filter(|(p, _)| *p != Some(crate::xml::XML_PREFIX));
            scope.map(|(p, uri)| crate::xml::Binding { prefix: p.map(Into::into), uri: uri.into() }).collect()
        } else {
            self.tree.declared(input).to_vec()
        };
        let Attrs { specified, annotations } = attrs;
        self.out.start_element(ElementStart { name, bindings, attributes: specified })?;
        let element = self.out.current().ok_or_else(|| super::super::beyond("an element started is not open"))?;
        self.out.annotate(element, &annotation, (nil, id_typing(annotation.typing), idrefs_typing(annotation.typing)))?;
        for (i, a) in annotations.iter().enumerate() {
            let (Some(a), Some(at)) = (a, element.nth_attribute(i)) else { continue };
            self.out.annotate(at, a, (false, id_typing(a.typing), idrefs_typing(a.typing)))?;
        }
        if let Some(d) = self.decls_pending.take() {
            self.decls.push((element, d));
        }
        Ok(element)
    }

    /// Element `input` of simple type, or of complex type `t` of simple
    /// content `s`, made whole [Schema1 §3.3.4 cvc-type.3.1, §3.4.4
    /// cvc-complex-type.2.2]: no element child, its normalized value valid
    /// against `s`, or its default if it has no character child [Schema1
    /// §3.3.5], its fixed value matched [cvc-elt.5.2.2.2.2]; its children
    /// a text node of its schema normalized value then its comments and
    /// processing instructions [XQueryDM §6.2.4].
    fn simple_element(&mut self, input: NodeId, (t, s): (TypeDef, Simple), attrs: Attrs, declared: Option<Declared<'s>>) -> Step<()> {
        if self.tree.children(input).any(|c| self.tree.kind(c) == Some(Kind::Element)) {
            return Err(Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.2]", "an element of simple content has no element child").into());
        }
        let nil = declared.is_some_and(|d| d.nil);
        let value = if nil { None } else { Some(self.simple_value(input, s, declared)?) };
        if let Some(v) = &value {
            self.note_ids(v);
        }
        self.start(input, attrs, (annotation(self.schema, t, value.as_ref()), nil))?;
        if let Some(v) = value.filter(|v| !v.normalized.is_empty()) {
            self.out.text(&v.normalized)?;
        }
        for child in self.tree.children(input).filter(|&c| matches!(self.tree.kind(c), Some(Kind::Comment | Kind::ProcessingInstruction))) {
            self.copy_other(child)?;
        }
        self.out.end_element()?;
        Ok(())
    }

    /// The value of element `input` of simple content `s`: its default if
    /// it has no character child and its declaration has a value
    /// constraint [Schema1 §3.3.4 cvc-elt.5.1], else its initial value
    /// read against `s` and checked against a fixed value \[5.2\].
    fn simple_value(&self, input: NodeId, s: Simple, declared: Option<Declared<'s>>) -> Step<Typed> {
        let text: String =
            self.tree.children(input).filter(|&c| self.tree.kind(c) == Some(Kind::Text)).map(|c| self.tree.content(c)).collect();
        let filled = self.tree.children(input).any(|c| self.tree.kind(c) == Some(Kind::Text));
        let constraint = declared.and_then(|d| d.decl.value.as_ref());
        let scope = self.tree.in_scope(input);
        if let Some(c) = constraint.filter(|_| !filled) {
            let read = string_valid(self.schema, s, &c.value.canonical(), &scope)?;
            return read
                .ok_or_else(|| Failure::new("[Schema1 §3.3.4 cvc-elt.5.1.2]", "the default is no value of the element's type").into());
        }
        let value = string_valid(self.schema, s, &text, &scope)?
            .ok_or_else(|| Failure::new("[Schema1 §3.3.4 cvc-type.3.1.3]", format!("'{text}' is no value of the element's type")))?;
        if let Some(fixed) = constraint.filter(|c| c.kind == ValueKind::Fixed)
            && !value.equal(&fixed.value)
        {
            return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.5.2.2.2.2]", format!("'{text}' is not the element's fixed value")).into());
        }
        Ok(value)
    }

    /// Element `input` of complex type `t` and complex content started,
    /// with `attrs`; the frame of its children [Schema1 §3.4.4
    /// cvc-complex-type.2].
    fn complex_element(&mut self, input: NodeId, t: TypeDef, attrs: Attrs, declared: Option<Declared<'s>>) -> Step<Frame<'s>> {
        let nil = declared.is_some_and(|d| d.nil);
        self.start(input, attrs, (annotation(self.schema, t, None), nil))?;
        let def = match t {
            TypeDef::Complex(c) => self.schema.complex(c).map(|d| (c, d)),
            TypeDef::Simple(_) => None,
        };
        let (content, mixed) = match def.filter(|_| !nil) {
            Some((ComplexId::ANY_TYPE, _)) => (Content::Any(Assess::Lax), true),
            Some((c, d)) => (self.model(c), matches!(d.content, ContentType::Mixed(_))),
            None => (if nil { Content::Nil } else { Content::Empty }, false),
        };
        let value = declared.and_then(|d| d.decl.value.as_ref()).filter(|_| mixed);
        Ok(Frame { next: self.tree.first_child(input), content, mixed, value, text: String::new(), filled: false })
    }

    /// What the children of an element of complex type `c` are checked
    /// against.
    fn model(&self, c: ComplexId) -> Content<'s> {
        match self.schema.automaton(c) {
            Some(a @ Automaton::Dfa { .. }) => Content::Model(a, a.start()),
            Some(Automaton::All { leaves, optional }) => Content::All(leaves, *optional, vec![false; leaves.len()]),
            None => Content::Empty,
        }
    }

    /// Element `input` not assessed, if `how` is skip, or laxly assessed
    /// by the ur-type [Schema1 §3.3.4]: annotated `xs:anyType`, its
    /// attributes and children assessed as `how` says.
    fn unassessed(&mut self, input: NodeId, how: Assess) -> Step<Option<Frame<'s>>> {
        let attrs = self.other_attributes(input, how)?;
        self.start(input, attrs, (Annotation::any_type(), false))?;
        Ok(Some(Frame {
            next: self.tree.first_child(input),
            content: Content::Any(how),
            mixed: true,
            value: None,
            text: String::new(),
            filled: false,
        }))
    }
}

/// Whether a node of `typing` is an ID [XQueryDM §6.2.4, §6.3.4].
fn id_typing(typing: Typing) -> bool {
    match typing {
        Typing::Atomic(a) => a.derives_from(AtomicType::Id),
        Typing::List(_) | Typing::AnySimple | Typing::Mixed | Typing::Empty | Typing::ElementOnly => false,
    }
}

/// Whether a node of `typing` is an IDREF or IDREFS [XQueryDM §6.3.4].
fn idrefs_typing(typing: Typing) -> bool {
    match typing {
        Typing::Atomic(a) | Typing::List(a) => a.derives_from(AtomicType::IdRef),
        Typing::AnySimple | Typing::Mixed | Typing::Empty | Typing::ElementOnly => false,
    }
}
