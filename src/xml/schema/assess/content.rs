//! The children of an element of complex content assessed [Schema1 §3.4.4
//! cvc-complex-type.2]: each element child matched to a particle of the
//! content model's automaton, whose particle determines how it is
//! assessed [Schema1 §3.9.4 cvc-particle]; character children allowed by
//! mixed content alone, or white space by element-only content; and, as
//! the element closes, its content complete, and a mixed element's value
//! constraint met [Schema1 §3.3.4 cvc-elt.5].

use super::super::automaton::Leaf;
use super::super::check::substitutable;
use super::super::component::{ElementId, Process, Simple, Typed, ValueKind};
use super::element::expanded;
use super::run::{Assess, Content, Frame, Run};
use super::{Failure, Step};
use crate::xml::{AtomicType, NodeId, is_space};

impl<'s> Run<'s> {
    /// How element `child` of the element of `frame` is assessed, its
    /// content model's state moved past it [Schema1 §3.9.4 cvc-particle].
    pub(super) fn child(&mut self, frame: &mut Frame<'s>, child: NodeId) -> Step<Assess> {
        frame.filled = true;
        let name = expanded(self.tree, child);
        let unexpected = || Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.4]", format!("element {} is not expected here", name.text()));
        match &mut frame.content {
            Content::Model(automaton, state) => {
                let (leaf, next) = automaton.step(*state, &name).ok_or_else(unexpected)?;
                *state = next;
                match leaf {
                    Leaf::Element(d) => Ok(Assess::Decl(self.context_declaration(*d, &name).ok_or_else(unexpected)?)),
                    Leaf::Wildcard(w) => Ok(match w.process {
                        Process::Skip => Assess::Skip,
                        Process::Lax => Assess::Lax,
                        Process::Strict => Assess::Strict,
                    }),
                }
            }
            Content::All(leaves, _, seen) => {
                let found = leaves.iter().enumerate().find_map(|(i, (d, _))| self.context_declaration(*d, &name).map(|c| (i, c)));
                let (i, decl) = found.ok_or_else(unexpected)?;
                let slot = seen.get_mut(i).ok_or_else(unexpected)?;
                if *slot {
                    return Err(unexpected().into());
                }
                *slot = true;
                Ok(Assess::Decl(decl))
            }
            Content::Any(how) => Ok(*how),
            Content::Empty | Content::Nil => {
                Err(Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.1]", "an element of empty content has no child").into())
            }
        }
    }

    /// [Schema1 §3.9.4 cvc-particle.2.3] The declaration that validates an
    /// element named `name` matched to element declaration particle `d`:
    /// `d`, of that name and not abstract; else a top-level declaration of
    /// that name that may stand for `d` [Schema1 §3.3.6
    /// cos-equiv-derived-ok-rec].
    fn context_declaration(&self, d: ElementId, name: &super::super::component::Expanded) -> Option<ElementId> {
        let decl = self.schema.element(d)?;
        if decl.name == *name && !decl.abstract_ {
            return Some(d);
        }
        let member = *self.schema.element_names.get(name)?;
        substitutable(self.schema, member, d, decl.disallowed).then_some(member)
    }

    /// Text node `text` among the children of the element of `frame`:
    /// copied, if its content allows it [Schema1 §3.4.4 cvc-complex-type.2.1,
    /// 2.3].
    pub(super) fn text(&mut self, frame: &mut Frame<'s>, text: NodeId) -> Step<()> {
        let content = self.tree.content(text);
        let blank = content.chars().all(is_space);
        match frame.content {
            Content::Empty | Content::Nil => {
                return Err(
                    Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.1]", "an element of empty content has no character child").into()
                );
            }
            Content::Model(..) | Content::All(..) => {
                if !frame.mixed && !blank {
                    let why = "an element of element-only content has no character child";
                    return Err(Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.3]", why).into());
                }
            }
            Content::Any(_) => {}
        }
        frame.filled = true;
        frame.text.push_str(content);
        self.out.text(content)?;
        Ok(())
    }

    /// The element of `frame` closed: its content complete [Schema1 §3.4.4
    /// cvc-complex-type.2.4], a mixed element's fixed value met
    /// [Schema1 §3.3.4 cvc-elt.5.2.2.2.1] or its default added [Schema1
    /// §3.3.5].
    pub(super) fn leave(&mut self, frame: &Frame<'s>) -> Step<()> {
        let incomplete = || Failure::new("[Schema1 §3.4.4 cvc-complex-type.2.4]", "the element's content is incomplete");
        match &frame.content {
            Content::Model(automaton, state) => {
                if !automaton.accepts(*state) {
                    return Err(incomplete().into());
                }
            }
            Content::All(leaves, optional, seen) => {
                let none = !seen.iter().any(|s| *s);
                let complete = leaves.iter().zip(seen).all(|((_, min), s)| *s || *min == 0);
                if !(complete || (none && *optional)) {
                    return Err(incomplete().into());
                }
            }
            Content::Empty | Content::Nil | Content::Any(_) => {}
        }
        if let Some(value) = frame.value {
            if !frame.filled {
                self.out.text(&value.value.canonical())?;
            } else if value.kind == ValueKind::Fixed && frame.text != value.value.canonical() {
                return Err(Failure::new("[Schema1 §3.3.4 cvc-elt.5.2.2.2.1]", "a mixed element's text is its fixed value").into());
            }
        }
        self.out.end_element()?;
        Ok(())
    }

    /// The IDs and IDREFs of `value`, of simple type `s`, noted for the
    /// validation root's ID/IDREF table [Schema1 §3.15.5].
    pub(super) fn note_ids(&mut self, s: Simple, value: &Typed) {
        let item = match s {
            Simple::Atomic(t) | Simple::List(t) => t,
            Simple::AnySimple => return,
        };
        for atom in &value.atoms {
            if item.derives_from(AtomicType::Id) {
                self.ids.id(&atom.lexical);
            } else if item.derives_from(AtomicType::IdRef) {
                self.ids.idref(&atom.lexical);
            }
        }
    }
}
