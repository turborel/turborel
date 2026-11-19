//! What the library's tests share (RULES.md R-TEST-10): a focus at a fixed
//! instant, calls of a function by its name, values to call it with, and
//! the results and errors read back.

use std::sync::{Arc, OnceLock};

use super::super::collation::{Collations, Collator, SQL_COLLATION};
use super::super::functions::{Context, FN_NAMESPACE, Library, QueryContext};
use super::super::testing::collations;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::xml::{Atomic, AtomicType, Item, Moment, Node, NodeId, QName, Sequence, Shape, Syntax, Whitespace, parse};

/// The dynamic context a test calls a function in, owned: what a
/// `Context` borrows of it.
#[derive(Clone)]
pub struct Focus {
    /// The context item, if one is defined.
    pub item: Option<Item>,
    /// The context position.
    pub position: usize,
    /// The context size.
    pub size: usize,
    /// The current dateTime.
    pub now: Moment,
    /// The implicit timezone, in minutes east of UTC.
    pub timezone: i16,
    /// The default collation.
    pub collation: Collator,
    /// The base URI of the static context.
    pub base_uri: Option<String>,
}

impl Focus {
    /// It with the context item `item`, at `position` of `size`.
    pub fn at(&self, item: Item, position: usize, size: usize) -> Focus {
        Focus { item: Some(item), position, size, ..self.clone() }
    }
}

/// The collations of a database, made once.
pub fn known() -> &'static Collations {
    static KNOWN: OnceLock<Collations> = OnceLock::new();
    KNOWN.get_or_init(collations)
}

/// The built-in functions, made once.
fn library() -> &'static Library {
    static BUILT: OnceLock<Library> = OnceLock::new();
    BUILT.get_or_init(Library::builtin)
}

/// The database's collation `name`, by its URI.
pub fn collator(name: &str) -> Collator {
    Collator::named(&format!("{SQL_COLLATION}{name}"), known()).unwrap()
}

/// A focus with no context item at 2004-05-12T18:17:15.125Z, the instant
/// [XQueryFO §16.3.1] shows, an implicit time zone of -05:00, as several
/// of \[XQueryFO\]'s examples assume; the codepoint collation and no base
/// URI.
pub fn focus() -> Focus {
    let now = Moment::parse("2004-05-12T18:17:15.125Z", Shape::DateTime).unwrap().unwrap();
    Focus { item: None, position: 0, size: 0, now, timezone: -300, collation: Collator::codepoint(), base_uri: None }
}

/// `fn:local` of `args` in `focus`.
pub fn call_in(focus: &Focus, local: &str, args: &[Sequence]) -> Result<Sequence> {
    let name = QName { prefix: None, uri: Some(FN_NAMESPACE.into()), local: local.into() };
    let function = library()
        .find(&name, args.len())
        .ok_or_else(|| SqlError::new(Code::Data, format!("[DESIGN] fn:{local}#{} is not in the library", args.len())))?;
    in_context(focus, |context| function.call(context, args.to_vec()))
}

/// The value of `f` of the dynamic context `focus` is, no statement ever
/// cancelled.
pub fn in_context<T>(focus: &Focus, f: impl FnOnce(&Context<'_>) -> T) -> T {
    let never = || Ok(());
    let query = QueryContext {
        now: focus.now,
        timezone: focus.timezone,
        collation: &focus.collation,
        collations: known(),
        base_uri: focus.base_uri.as_deref(),
        interrupted: &never,
    };
    f(&Context { item: focus.item.as_ref(), position: focus.position, size: focus.size, query: &query })
}

/// `fn:local` of `args` in `focus()`.
pub fn call(local: &str, args: &[Sequence]) -> Result<Sequence> {
    call_in(&focus(), local, args)
}

/// The one value of `kind` written `text`.
pub fn atom(kind: AtomicType, text: &str) -> Sequence {
    Sequence::new(vec![Item::Atomic(Atomic::new(kind, text))])
}

/// The `xs:string` `text`.
pub fn s(text: &str) -> Sequence {
    atom(AtomicType::String, text)
}

/// The `xs:integer` `n`.
pub fn int(n: i64) -> Sequence {
    atom(AtomicType::Integer, &n.to_string())
}

/// The `xs:decimal` written `text`.
pub fn dec(text: &str) -> Sequence {
    atom(AtomicType::Decimal, text)
}

/// The `xs:double` written `text`.
pub fn dbl(text: &str) -> Sequence {
    atom(AtomicType::Double, text)
}

/// The sequence of each of `values`' items, in order.
pub fn seq(values: &[Sequence]) -> Sequence {
    Sequence::new(values.iter().flat_map(|v| v.items().iter().cloned()).collect())
}

/// The empty sequence.
pub fn empty() -> Sequence {
    Sequence::empty()
}

/// The lexical form of each atomic value of `value`, a node's string
/// value of each node.
pub fn texts(value: &Sequence) -> Vec<String> {
    value
        .items()
        .iter()
        .map(|item| match item {
            Item::Atomic(a) => a.lexical.to_string(),
            Item::Node(n) => n.tree().string_value(n.id()),
        })
        .collect()
}

/// The lexical form of the one atomic value of `result`.
pub fn text(result: Result<Sequence>) -> String {
    let value = result.unwrap();
    let mut all = texts(&value);
    assert_eq!(all.len(), 1, "one item: {all:?}");
    all.remove(0)
}

/// The type of the one atomic value of `result`, and its lexical form.
pub fn typed(result: Result<Sequence>) -> (AtomicType, String) {
    match result.unwrap().items() {
        [Item::Atomic(a)] => (a.kind, a.lexical.to_string()),
        other => panic!("one atomic value: {other:?}"),
    }
}

/// The XQuery error code of `result`'s error, its SQLSTATE 10000.
pub fn code(result: Result<Sequence>) -> String {
    let error = result.expect_err("an error");
    assert_eq!(error.sqlstate(), SqlState::XQUERY_ERROR, "{error}");
    let message = error.to_string();
    let (_, after) = message.split_once("err:").unwrap_or_else(|| panic!("an error code in {message}"));
    after.chars().take_while(char::is_ascii_alphanumeric).collect()
}

/// The document node of the XML text `xml`.
pub fn document(xml: &str) -> Node {
    let tree = parse(xml, Syntax::Content, Whitespace::Preserve).unwrap();
    Node::new(Arc::new(tree), NodeId::ROOT)
}

/// The nodes below `root`, its attributes too, that `pick` chooses, in
/// document order.
pub fn below(root: &Node, pick: impl Fn(&Node) -> bool) -> Vec<Node> {
    let tree = root.tree();
    let mut out = Vec::new();
    for id in tree.descendants(root.id()) {
        let node = root.at(id);
        if pick(&node) {
            out.push(node);
        }
        for attribute in tree.attributes(id) {
            let a = root.at(attribute);
            if pick(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// The elements below `root` named `local`, in document order.
pub fn elements(root: &Node, local: &str) -> Vec<Node> {
    below(root, |n| n.kind() == Some(crate::xml::Kind::Element) && n.tree().name(n.id()).is_some_and(|q| &*q.local == local))
}

/// The sequence of `node`.
pub fn one(node: &Node) -> Sequence {
    Sequence::new(vec![Item::Node(node.clone())])
}
