//! Constructors typed [XQueryFS §4.7]: a direct constructor as the
//! computed constructor it is normalized to [XQueryFS §4.7.1, §4.7.2].
//!
//! An element's content is `fs:item-sequence-to-node-sequence` of its
//! attributes and content [XQueryFS §4.7.3.1, §7.1.5]: attributes, then
//! elements, text, comments, processing instructions and atomic values,
//! of which it makes `attribute*, (element | text | processing-instruction
//! | comment)*`; the element is of its name and of type `xs:anyType` in
//! construction mode preserve, `xdt:untyped` in strip. A document's
//! content is that too, of no attribute [XQueryFS §4.7.3.3]. A computed
//! name is one `xs:QName`, `xs:string` or `xdt:untypedAtomic`, of a
//! processing instruction `xs:NCName` for the first [XQueryFS §4.7.3.1,
//! §4.7.3.2, §4.7.3.5].
//!
//! \[DESIGN\] RULES.md D106: a direct element of one element-content unit
//! is normalized to a computed element of that unit alone [XQueryFS
//! §4.7.1], which the computed element's normalization then gives
//! `fs:item-sequence-to-node-sequence` too [XQueryFS §4.7.3.1], so every
//! element's content is converted alike, as the evaluation converts it
//! [XQuery §3.7.1.3].

use super::super::plan::{ComputedElement, Content, DirectElement, NameOp, Op, Part, PiOp};
use super::super::types::SchemaType;
use super::sequence::any_element;
use super::subtype::subtype;
use super::types::{Atom, Item, Name, Node, Occ, Type};
use super::values::data;
use super::{Frame, Statics, type_error};
use crate::error::Result;
use crate::xml::{AtomicType, QName};

impl Statics<'_> {
    /// [XQueryFS §4.7.3.1] A computed element constructor.
    pub(super) fn computed_element(&mut self, e: &ComputedElement, frame: &mut Frame) -> Result<Type> {
        let name = self.name(&e.name, frame, AtomicType::QName)?;
        let content = match &e.content {
            Some(c) => self.expr(c, frame)?,
            None => Type::Empty,
        };
        self.element(name, &content)
    }

    /// [XQueryFS §4.7.3.2] A computed attribute constructor: of
    /// `xdt:untypedAtomic`, its content `fs:item-sequence-to-untypedAtomic`
    /// of any items.
    pub(super) fn attribute(&mut self, name: &NameOp, content: Option<&Op>, frame: &mut Frame) -> Result<Type> {
        let name = self.name(name, frame, AtomicType::QName)?;
        if let Some(c) = content {
            self.expr(c, frame)?;
        }
        let of = SchemaType::Atomic(AtomicType::UntypedAtomic);
        Ok(Type::Item(Item::Attribute(Box::new(Node { name, nillable: false, of }))))
    }

    /// [XQueryFS §4.7.3.3] A document constructor: its content's nodes
    /// elements, text, processing instructions and comments alone.
    pub(super) fn document(&mut self, content: &Op, frame: &mut Frame) -> Result<Type> {
        let t = items_to_nodes(&self.expr(content, frame)?)?;
        let wanted = Type::of_prime(vec![any_element(), Item::Text, Item::Pi, Item::Comment], Occ::Star);
        if !subtype(&t, &wanted)? {
            return Err(type_error(&format!("the content of a document constructor is of {t}, not of {wanted} [XQueryFS §4.7.3.3]")));
        }
        Ok(Type::Item(Item::Document(Box::new(t))))
    }

    /// [XQueryFS §4.7.3.5] A processing instruction constructor: a computed
    /// target one `xs:NCName`, `xs:string` or `xdt:untypedAtomic`.
    pub(super) fn pi(&mut self, pi: &PiOp, frame: &mut Frame) -> Result<Type> {
        if let Err(target) = &pi.target {
            let t = data(&self.expr(target, frame)?)?;
            one_name(&t, AtomicType::NcName)?;
        }
        if let Err(Some(content)) = &pi.content {
            self.expr(content, frame)?;
        }
        Ok(Type::Item(Item::Pi))
    }

    /// [XQueryFS §4.7.1] A direct element constructor: its attributes, each
    /// of `xdt:untypedAtomic`, then its content, the element they make.
    pub(super) fn direct(&mut self, e: &DirectElement, frame: &mut Frame) -> Result<Type> {
        let mut parts = Vec::new();
        for (name, value) in &e.attributes {
            for part in value {
                if let Part::Enclosed(op) = part {
                    self.expr(op, frame)?;
                }
            }
            let of = SchemaType::Atomic(AtomicType::UntypedAtomic);
            parts.push(Type::Item(Item::Attribute(Box::new(Node { name: name_of(name), nillable: false, of }))));
        }
        for c in &e.content {
            parts.push(match c {
                Content::Text(_) => Type::Item(Item::Text),
                Content::Element(inner) => self.direct(inner, frame)?,
                Content::Comment(_) => Type::Item(Item::Comment),
                Content::Pi(..) => Type::Item(Item::Pi),
                Content::Enclosed(op) => self.expr(op, frame)?,
            });
        }
        self.element(name_of(&e.name), &Type::seq(parts))
    }

    /// [XQueryFS §4.7.3.1] The element named `name` of `content`: its
    /// content converted to nodes, attributes before the rest; of type
    /// `xs:anyType` in construction mode preserve, else `xdt:untyped`.
    fn element(&self, name: Name, content: &Type) -> Result<Type> {
        items_to_nodes(content)?;
        let of = if self.plan.context(self.module).settings.preserve { SchemaType::AnyType } else { SchemaType::Untyped };
        Ok(Type::Item(Item::Element(Box::new(Node { name, nillable: false, of }))))
    }

    /// A computed element's or attribute's name: of a QName its own; of an
    /// expression, one `xs:QName`, `xs:string` or `xdt:untypedAtomic`
    /// atomized \[XQueryFS §4.7.3.1\], of any name.
    fn name(&mut self, name: &NameOp, frame: &mut Frame, kind: AtomicType) -> Result<Name> {
        match name {
            NameOp::Fixed(q) => Ok(name_of(q)),
            NameOp::Computed(op, _) => {
                let t = data(&self.expr(op, frame)?)?;
                one_name(&t, kind)?;
                Ok(Name::ANY)
            }
        }
    }
}

/// The name of type of `q`.
fn name_of(q: &QName) -> Name {
    Name::new(q.uri.as_deref(), &q.local)
}

/// [XQueryFS §4.7.3.1, §4.7.3.5] The error unless `t` is one `kind`,
/// `xs:string` or `xdt:untypedAtomic`.
fn one_name(t: &Type, kind: AtomicType) -> Result<()> {
    let names = Type::of_prime(
        [kind, AtomicType::String, AtomicType::UntypedAtomic].into_iter().map(|a| Item::Atomic(Atom::Of(a))).collect(),
        Occ::One,
    );
    if subtype(t, &names)? {
        return Ok(());
    }
    Err(type_error(&format!("a computed name is of {t}, not of {names} [XQueryFS §4.7.3.1]")))
}

/// [XQueryFS §7.1.5] `fs:item-sequence-to-node-sequence` of `t`, of type
/// `attribute*, (element | text | processing-instruction | comment |
/// xs:string | ... | xs:NOTATION)*`: `attribute*, (element | text |
/// processing-instruction | comment)*`, its attributes and its other
/// nodes of `t`'s, its atomic values text.
///
/// # Errors
///
/// err:XPTY0004 of a `t` of any other type; err:XQTY0024 of attributes
/// after other nodes [XQuery §3.7.1.3].
fn items_to_nodes(t: &Type) -> Result<Type> {
    let attribute = Type::occur(Type::Item(super::sequence::any_attribute()), Occ::Star);
    let others = Type::of_prime(vec![any_element(), Item::Text, Item::Pi, Item::Comment, Item::Atomic(Atom::Any)], Occ::Star);
    if subtype(t, &Type::Seq(vec![attribute.clone(), others.clone()]))? {
        return super::subtype::map_items(t, 0, &mut |i| Ok(Type::Item(if let Item::Atomic(_) = i { Item::Text } else { i.clone() })));
    }
    if subtype(
        t,
        &Type::occur(
            Type::of_prime(
                vec![super::sequence::any_attribute(), any_element(), Item::Text, Item::Pi, Item::Comment, Item::Atomic(Atom::Any)],
                Occ::One,
            ),
            Occ::Star,
        ),
    )? {
        return Err(super::super::error::error(
            super::super::error::ErrorCode::XQTY0024,
            &format!("an element's content of {t} may hold an attribute after another node [XQueryFS §4.7.3.1] [XQuery §3.7.1.3]"),
        ));
    }
    Err(type_error(&format!("an element's content is of {t}, of a node no element holds [XQueryFS §7.1.5]")))
}
