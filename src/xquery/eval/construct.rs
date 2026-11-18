//! Element and document constructors evaluated [XQuery §3.7.1, §3.7.3.1,
//! §3.7.3.3]: the content read into pieces, attributes before the rest,
//! adjacent text one, each node of an enclosed expression copied by the
//! construction and copy-namespaces modes [XQuery §3.7.1.3], and the
//! element built with its bindings [XQuery §3.7.4]. The other nodes'
//! constructors are in `construct/nodes.rs`.
//!
//! \[DESIGN\] RULES.md D86: a direct element nested in another is built in
//! place, in the tree of the element it is in, so nothing is copied twice;
//! each is one level deeper, within `MAX_DEPTH`. The trees one evaluation
//! constructs hold `xml::MAX_STORED` octets together at most.

mod names;
mod nodes;

use super::super::error::{ErrorCode, beyond, error};
use super::super::plan::{ComputedElement, Content, DirectElement, NameOp, Op, Part};
use super::sequence::atomize;
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::Result;
use crate::xml::{
    Annotation, Binding, Builder, Construction, Copying, ElementStart, Item, Kind, MAX_STORED, Node, QName, Specified, Tree, is_xml_id,
};

/// The one separator of adjacent atomic values made a text node or an
/// attribute's value [XQuery §3.7.1.1, §3.7.1.3].
const SEPARATOR: &str = " ";

/// A piece of the content of an element or document [XQuery §3.7.1.3].
enum Piece<'p> {
    /// An attribute.
    Attribute(Specified),
    /// Text, adjacent text joined.
    Text(String),
    /// A node to copy: an element, comment or processing instruction.
    Copy(Node),
    /// A direct element constructor nested, built in place.
    Element(&'p DirectElement),
    /// A direct comment constructor nested.
    Comment(&'p str),
    /// A direct processing instruction constructor nested: its target and
    /// content.
    Pi(&'p str, &'p str),
}

/// `piece` added after `out`'s pieces: text to the text before it.
fn push<'p>(out: &mut Vec<Piece<'p>>, piece: Piece<'p>) {
    if let (Some(Piece::Text(before)), Piece::Text(more)) = (out.last_mut(), &piece) {
        before.push_str(more);
        return;
    }
    out.push(piece);
}

/// The strings of `items` atomized, a space apart [XQuery §3.7.1.1].
///
/// # Errors
///
/// As `atomize`.
pub(super) fn joined(items: &[Item]) -> Result<String> {
    Ok(atomize(items)?.iter().map(|a| &*a.lexical).collect::<Vec<_>>().join(SEPARATOR))
}

/// `value` of an attribute named `xml:id`, its spaces collapsed [XQuery
/// §3.7.1.1]: none at either end, a run of them one.
pub(super) fn id_value(name: &QName, value: String) -> String {
    if !is_xml_id(name) {
        return value;
    }
    value.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

impl Evaluator<'_> {
    /// The element just started in `builder` annotated `xs:anyType` in
    /// construction mode preserve; in strip it is `xdt:untyped` [XQuery
    /// §3.7.1.3, §3.7.3.1].
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the tree would be too big.
    fn annotated(&self, builder: &mut Builder) -> Result<()> {
        match builder.current().filter(|_| self.plan.settings.preserve) {
            Some(element) => builder.annotate(element, &Annotation::any_type(), (false, false, false)),
            None => Ok(()),
        }
    }

    /// How a node of an enclosed expression is copied [XQuery §3.7.1.3].
    fn copying(&self) -> Copying {
        let s = &self.plan.settings;
        let construction = if s.preserve { Construction::Preserve } else { Construction::Strip };
        Copying { construction, preserve: s.copy_preserve, inherit: s.copy_inherit }
    }

    /// The error if the trees constructed would hold more than
    /// `MAX_STORED` octets with `tree`; else `tree` counted.
    pub(super) fn constructed(&self, tree: &Tree) -> Result<()> {
        let held = self.constructed.get().saturating_add(tree.held_bytes());
        if held > MAX_STORED {
            return Err(beyond(&format!("the nodes it constructs hold more than {MAX_STORED} octets")));
        }
        self.constructed.set(held);
        Ok(())
    }

    /// The node `builder` built, alone, counted.
    fn built(&self, builder: Builder) -> Result<Seq> {
        let tree = builder.finish()?;
        self.constructed(&tree)?;
        Ok(vec![Item::Node(Node::root(tree))])
    }

    /// The pieces of the items `value` of an enclosed expression [XQuery
    /// §3.7.1.3], added to `out`: each run of atomic values a text of
    /// their strings a space apart; a document node its children; a text
    /// node its text; an attribute copied; another node to copy.
    fn pieces_of(&self, value: Seq, out: &mut Vec<Piece<'_>>) -> Result<()> {
        let mut run: Vec<String> = Vec::new();
        for item in value {
            let node = match item {
                Item::Atomic(a) => {
                    run.push(a.lexical.into_string());
                    continue;
                }
                Item::Node(node) => node,
            };
            if !run.is_empty() {
                push(out, Piece::Text(std::mem::take(&mut run).join(SEPARATOR)));
            }
            let ids: Vec<_> = if node.kind() == Some(Kind::Document) { node.tree().children(node.id()).collect() } else { vec![node.id()] };
            for id in ids {
                let tree = node.tree();
                let piece = match tree.kind(id) {
                    Some(Kind::Attribute) => Piece::Attribute(crate::xml::copied_attribute(tree, id, self.copying())?),
                    Some(Kind::Text) => Piece::Text(tree.content(id).to_owned()),
                    Some(Kind::Element | Kind::Comment | Kind::ProcessingInstruction | Kind::Document) | None => Piece::Copy(node.at(id)),
                };
                push(out, piece);
            }
        }
        if !run.is_empty() {
            push(out, Piece::Text(run.join(SEPARATOR)));
        }
        Ok(())
    }

    /// A direct element constructor [XQuery §3.7.1]: an element standing
    /// alone, declaring the bindings of the constructors it is within.
    pub(super) fn direct_element(&self, e: &DirectElement, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut builder = Builder::parentless();
        self.build_element(&mut builder, e, (frame, focus), None)?;
        self.built(builder)
    }

    /// `e` built in `builder` [XQuery §3.7.1]: in the scope `scope` of the
    /// element it is built in, or standing alone if none.
    fn build_element(
        &self,
        builder: &mut Builder,
        e: &DirectElement,
        (frame, focus): (&mut Frame, &Focus),
        scope: Option<&[Binding]>,
    ) -> Result<()> {
        let _deeper = self.deeper()?;
        let mut attributes = Vec::with_capacity(e.attributes.len());
        for (name, parts) in &e.attributes {
            let mut value = String::new();
            for part in parts {
                match part {
                    Part::Text(text) => value.push_str(text),
                    Part::Enclosed(op) => value.push_str(&joined(&self.eval(op, frame, focus)?)?),
                }
            }
            let is_id = is_xml_id(name);
            attributes.push(Specified { name: name.clone(), value: id_value(name, value), is_id, is_idrefs: false });
        }
        let mut pieces = Vec::new();
        for part in &e.content {
            match part {
                Content::Text(text) => push(&mut pieces, Piece::Text(text.to_string())),
                Content::Element(inner) => pieces.push(Piece::Element(inner)),
                Content::Comment(text) => pieces.push(Piece::Comment(text)),
                Content::Pi(target, content) => pieces.push(Piece::Pi(target, content)),
                Content::Enclosed(op) => self.pieces_of(self.eval(op, frame, focus)?, &mut pieces)?,
            }
        }
        let (declared, own_scope) = match scope {
            Some(outer) => (e.declared.clone(), outer.to_vec()),
            None => (overridden(&e.inherited, &e.declared), Vec::new()),
        };
        let children = Self::start(builder, (e.name.clone(), attributes, declared), pieces, &own_scope)?;
        self.annotated(builder)?;
        let inner_scope = [own_scope, children.1].concat();
        for piece in children.0 {
            self.add_piece(builder, piece, (frame, focus), &inner_scope)?;
        }
        builder.end_element()
    }

    /// The element of `name`, `attributes` and the bindings `declared`
    /// started in `builder` with the attributes among `pieces` [XQuery
    /// §3.7.1.3]: an attribute after another piece err:XQTY0024, two of
    /// one name err:XQDY0025; its names bound in `scope` [XQuery §3.7.4].
    /// The pieces left, and the bindings it declares.
    fn start<'p>(
        builder: &mut Builder,
        (name, mut attributes, mut declared): (QName, Vec<Specified>, Vec<Binding>),
        pieces: Vec<Piece<'p>>,
        scope: &[Binding],
    ) -> Result<(Vec<Piece<'p>>, Vec<Binding>)> {
        let mut children = Vec::new();
        for piece in pieces {
            match piece {
                // [XQuery §3.7.1.3] a text node of no characters is deleted.
                Piece::Text(text) => {
                    if !text.is_empty() {
                        children.push(Piece::Text(text));
                    }
                }
                Piece::Attribute(a) => {
                    if !children.is_empty() {
                        return Err(error(
                            ErrorCode::XQTY0024,
                            "an attribute follows a node that is not one in an element's content [XQuery §3.7.1.3]",
                        ));
                    }
                    attributes.push(a);
                }
                other @ (Piece::Copy(_) | Piece::Element(_) | Piece::Comment(_) | Piece::Pi(..)) => children.push(other),
            }
        }
        distinct(&attributes)?;
        let mut name = name;
        names::bind(scope, &mut declared, &mut name, false);
        for a in &mut attributes {
            names::bind(scope, &mut declared, &mut a.name, true);
        }
        builder.start_element(ElementStart { name, bindings: declared.clone(), attributes })?;
        Ok((children, declared))
    }

    /// `piece`, a child, added to the element open in `builder`, of the
    /// bindings `scope` in scope.
    fn add_piece(&self, builder: &mut Builder, piece: Piece<'_>, (frame, focus): (&mut Frame, &Focus), scope: &[Binding]) -> Result<()> {
        match piece {
            Piece::Text(text) => builder.text(&text),
            Piece::Copy(node) => builder.copy_as(node.tree(), node.id(), self.copying()),
            Piece::Element(e) => self.build_element(builder, e, (frame, focus), Some(scope)),
            Piece::Comment(text) => builder.comment(text),
            Piece::Pi(target, content) => builder.processing_instruction(target, content),
            Piece::Attribute(_) => Ok(()),
        }
    }

    /// A computed element constructor [XQuery §3.7.3.1]: its name, fixed
    /// or computed, and its content, as an enclosed expression's.
    pub(super) fn computed_element(&self, e: &ComputedElement, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let name = match &e.name {
            NameOp::Fixed(name) => name.clone(),
            NameOp::Computed(op, names) => names::computed(&self.eval(op, frame, focus)?, names)?,
        };
        let mut pieces = Vec::new();
        if let Some(content) = &e.content {
            self.pieces_of(self.eval(content, frame, focus)?, &mut pieces)?;
        }
        let mut builder = Builder::parentless();
        let declared = overridden(&e.inherited, &[]);
        let (children, _) = Self::start(&mut builder, (name, Vec::new(), declared), pieces, &[])?;
        self.annotated(&mut builder)?;
        for piece in children {
            self.add_piece(&mut builder, piece, (frame, focus), &[])?;
        }
        builder.end_element()?;
        self.built(builder)
    }

    /// `document` [XQuery §3.7.3.3]: a document node of its content's
    /// pieces, of no attribute \[err:XPTY0004\].
    pub(super) fn document(&self, content: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let mut pieces = Vec::new();
        self.pieces_of(self.eval(content, frame, focus)?, &mut pieces)?;
        let mut builder = Builder::document();
        for piece in pieces {
            if matches!(piece, Piece::Attribute(_)) {
                return Err(error(ErrorCode::XPTY0004, "a document node's content holds an attribute [XQuery §3.7.3.3]"));
            }
            self.add_piece(&mut builder, piece, (frame, focus), &[])?;
        }
        self.built(builder)
    }
}

/// The bindings of `outer` then `own`, a prefix bound by a later one kept
/// once, as that binds it [XQuery §3.7.4].
fn overridden(outer: &[Binding], own: &[Binding]) -> Vec<Binding> {
    let mut all: Vec<Binding> = Vec::with_capacity(outer.len().saturating_add(own.len()));
    for b in outer.iter().chain(own) {
        all.retain(|kept| kept.prefix != b.prefix);
        all.push(b.clone());
    }
    all
}

/// The error if two of `attributes` have one name [XQuery §3.7.1.3]
/// \[err:XQDY0025\].
fn distinct(attributes: &[Specified]) -> Result<()> {
    for (n, a) in attributes.iter().enumerate() {
        if attributes.iter().skip(n.saturating_add(1)).any(|b| b.name.same(&a.name)) {
            let why = format!("two attributes of a constructed element are named {} [XQuery §3.7.1.3]", a.name.local);
            return Err(error(ErrorCode::XQDY0025, &why));
        }
    }
    Ok(())
}
