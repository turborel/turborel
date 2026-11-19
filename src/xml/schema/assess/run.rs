//! An assessment run: the validation root and its children walked in
//! document order on a stack of the elements open, the tree of the
//! post-schema-validation infoset built as they are [XQueryDM §3.3].

use super::super::automaton::{Automaton, StateId};
use super::super::component::{ElementId, Schema, ValueConstraint};
use super::identity::Ids;
use super::{Failure, Mode, NOT_A_DOCUMENT, NOT_A_NODE, Step};
use crate::xml::{Builder, Kind, Node, NodeId, Tree};

/// How an element is to be assessed: by the declaration its context
/// determines, or as its context's wildcard says, or as the validation
/// root [Schema1 §3.3.4, §3.10.4, §5.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Assess {
    /// By a context-determined declaration.
    Decl(ElementId),
    /// mustFind: by its top-level declaration, or its `xsi:type`, one of
    /// which it must have [Schema1 §3.10.4 clause 2].
    Strict,
    /// By its top-level declaration or `xsi:type` if it has one, else laxly
    /// [Schema1 §3.10.4 clause 1].
    Lax,
    /// Not at all [Schema1 §3.10.4 clause 3].
    Skip,
    /// The validation root, in a validation mode [XQuery §3.13].
    Root(Mode),
}

/// What an open element's children are checked against.
#[derive(Debug)]
pub(super) enum Content<'s> {
    /// A content model's automaton, and its state.
    Model(&'s Automaton, StateId),
    /// An all group: its element declarations, each with its {min occurs},
    /// whether the group may be absent, and which have come.
    All(&'s [(ElementId, u64)], bool, Vec<bool>),
    /// Empty content: no element and no character child [Schema1 §3.4.4
    /// clause 2.1].
    Empty,
    /// A nilled element: no element and no character child [Schema1
    /// §3.3.4 cvc-elt.3.2.1].
    Nil,
    /// The ur-type's content, laxly assessed or skipped: any element and
    /// text [Schema1 §3.4.7].
    Any(Assess),
}

/// An element open: the next of its children to read, what they are
/// checked against, whether text may be among them, and what its value
/// constraint asks of it.
#[derive(Debug)]
pub(super) struct Frame<'s> {
    /// Its next child to read.
    pub next: Option<NodeId>,
    /// What its children are checked against.
    pub content: Content<'s>,
    /// Whether its content is mixed: text other than white space may be
    /// among its children.
    pub mixed: bool,
    /// Its declaration's value constraint, of a mixed element [Schema1
    /// §3.3.4 cvc-elt.5].
    pub value: Option<&'s ValueConstraint>,
    /// Its initial value, the text of its children so far [Schema1
    /// §3.1.4].
    pub text: String,
    /// Whether it has had an element or character child.
    pub filled: bool,
}

/// An assessment under way.
#[derive(Debug)]
pub(super) struct Run<'s> {
    /// The schema.
    pub schema: &'s Schema,
    /// The tree of the node validated.
    pub tree: &'s Tree,
    /// The tree being made.
    pub out: Builder,
    /// The IDs and IDREFs met [Schema1 §3.15.5].
    pub ids: Ids,
    /// Whether the ID/IDREF table and identity constraints are checked:
    /// not of an element node [XQuery §3.13].
    pub whole: bool,
    /// Each element made, by its place in the tree made, and the
    /// declaration that validated it.
    pub decls: Vec<(NodeId, ElementId)>,
    /// The declaration validating the element about to be made.
    pub decls_pending: Option<ElementId>,
}

/// [XQuery §3.13] The assessment of `node` against `schema` in `mode`.
pub(super) fn run(schema: &Schema, node: &Node, mode: Mode) -> Step<Node> {
    let tree = node.tree();
    let id = node.id();
    match tree.kind(id) {
        Some(Kind::Document) => {
            let elements = tree.children(id).filter(|&c| tree.kind(c) == Some(Kind::Element)).count();
            let others =
                tree.children(id).any(|c| !matches!(tree.kind(c), Some(Kind::Element | Kind::Comment | Kind::ProcessingInstruction)));
            if elements != 1 || others {
                let why = "a document node validated has one element and comments and processing instructions as its children";
                return Err(Failure { rule: "[XQuery §3.13]".into(), why: why.into(), code: NOT_A_DOCUMENT }.into());
            }
            let mut r =
                Run { schema, tree, out: Builder::document(), ids: Ids::default(), whole: true, decls: Vec::new(), decls_pending: None };
            for child in tree.children(id) {
                match tree.kind(child) {
                    Some(Kind::Element) => r.walk(child, Assess::Root(mode))?,
                    Some(Kind::Comment | Kind::ProcessingInstruction | Kind::Text | Kind::Attribute | Kind::Document) | None => {
                        r.copy_other(child)?;
                    }
                }
            }
            r.finish()
        }
        Some(Kind::Element) => {
            let mut r =
                Run { schema, tree, out: Builder::parentless(), ids: Ids::default(), whole: false, decls: Vec::new(), decls_pending: None };
            r.walk(id, Assess::Root(mode))?;
            r.finish()
        }
        Some(Kind::Attribute | Kind::Text | Kind::Comment | Kind::ProcessingInstruction) | None => {
            let why = "the operand of validate is one document or element node";
            Err(Failure { rule: "[XQuery §3.13]".into(), why: why.into(), code: NOT_A_NODE }.into())
        }
    }
}

impl<'s> Run<'s> {
    /// Element `root` and its descendants assessed as `assess` says, each
    /// element on a stack of those open.
    fn walk(&mut self, root: NodeId, assess: Assess) -> Step<()> {
        let mut stack: Vec<Frame<'s>> = Vec::new();
        stack.extend(self.enter(root, assess)?);
        while let Some(frame) = stack.last_mut() {
            let Some(child) = frame.next else {
                if let Some(done) = stack.pop() {
                    self.leave(&done)?;
                }
                continue;
            };
            frame.next = self.tree.next_sibling(child);
            match self.tree.kind(child) {
                Some(Kind::Element) => {
                    let how = self.child(frame, child)?;
                    stack.extend(self.enter(child, how)?);
                }
                Some(Kind::Text) => self.text(frame, child)?,
                Some(Kind::Comment | Kind::ProcessingInstruction) => self.copy_other(child)?,
                Some(Kind::Attribute | Kind::Document) | None => {}
            }
        }
        Ok(())
    }

    /// Comment or processing instruction `id` copied to the tree made.
    pub fn copy_other(&mut self, id: NodeId) -> Step<()> {
        match self.tree.kind(id) {
            Some(Kind::Comment) => self.out.comment(self.tree.content(id))?,
            Some(Kind::ProcessingInstruction) => {
                let target = self.tree.name(id).map_or("", |n| &*n.local);
                self.out.processing_instruction(target, self.tree.content(id))?;
            }
            Some(Kind::Element | Kind::Attribute | Kind::Text | Kind::Document) | None => {}
        }
        Ok(())
    }

    /// The tree made, once the validation root's ID/IDREF table and
    /// identity constraints are shown satisfied, if they are checked.
    fn finish(self) -> Step<Node> {
        if self.whole {
            self.ids.check()?;
        }
        let tree = self.out.finish()?;
        if self.whole {
            super::identity::check(self.schema, &tree, &self.decls)?;
        }
        Ok(Node::root(tree))
    }
}
