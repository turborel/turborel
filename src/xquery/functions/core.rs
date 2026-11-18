//! The built-in functions the core of XQuery relies on \[XQueryFO\]:
//! fn:true and fn:false [§9.1], fn:not [§9.3.1], fn:boolean [§15.1.1],
//! fn:data [§2.4], fn:string [§2.3], fn:count [§15.4.1], fn:empty and
//! fn:exists [§15.1.4, §15.1.5], fn:position and fn:last [§16.1, §16.2],
//! fn:concat [§7.4.1], fn:root [§14.9] and fn:doc [§15.5.4], each a
//! `Builtin` of its name, signature and body.

use std::sync::Arc;

use super::super::ast::Occurrence;
use super::super::error::{ErrorCode, error};
use super::super::eval::{atomize, ebv, string_value};
use super::super::types::{ItemKind, NodeKind, SchemaType, SeqType};
use super::{Context, FN_NAMESPACE, XQueryFunction};
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, Item, Node, QName, Sequence};

/// What a built-in function computes of its context and arguments.
type Body = fn(&Context<'_>, &[Sequence]) -> Result<Sequence>;

/// A built-in function of the `fn` namespace.
struct Builtin {
    /// Its name.
    name: QName,
    /// Its parameters' types.
    parameters: Vec<SeqType>,
    /// Its result's type.
    result: SeqType,
    /// Whether it takes any number of arguments past its parameters'.
    variadic: bool,
    /// What it computes.
    body: Body,
}

impl XQueryFunction for Builtin {
    fn name(&self) -> &QName {
        &self.name
    }

    fn parameters(&self) -> &[SeqType] {
        &self.parameters
    }

    fn result(&self) -> &SeqType {
        &self.result
    }

    fn variadic(&self) -> bool {
        self.variadic
    }

    fn call(&self, context: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence> {
        (self.body)(context, &arguments)
    }
}

/// `item()*`.
const ITEMS: SeqType = SeqType::Of(ItemKind::Item, Occurrence::Any);
/// `item()?`.
const ITEM: SeqType = SeqType::Of(ItemKind::Item, Occurrence::Optional);
/// `xs:boolean`.
const BOOLEAN: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(AtomicType::Boolean)), Occurrence::One);
/// `xs:integer`.
const INTEGER: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(AtomicType::Integer)), Occurrence::One);
/// `xs:string`.
const STRING: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(AtomicType::String)), Occurrence::One);
/// `xs:string?`.
const OPTIONAL_STRING: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(AtomicType::String)), Occurrence::Optional);
/// `xdt:anyAtomicType?`.
const OPTIONAL_ATOMIC: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::AnyAtomicType), Occurrence::Optional);
/// `xdt:anyAtomicType*`.
const ATOMICS: SeqType = SeqType::Of(ItemKind::Atomic(SchemaType::AnyAtomicType), Occurrence::Any);
/// `node()?`.
const OPTIONAL_NODE: SeqType = SeqType::Of(ItemKind::Node(NodeKind::Any), Occurrence::Optional);
/// `node()`.
const NODE: SeqType = SeqType::Of(ItemKind::Node(NodeKind::Any), Occurrence::One);
/// `document-node()?`.
const OPTIONAL_DOCUMENT: SeqType = SeqType::Of(ItemKind::Node(NodeKind::Document(None)), Occurrence::Optional);

/// The built-in function `local` of `parameters`, `result` and `body`.
fn builtin(local: &str, parameters: Vec<SeqType>, result: SeqType, body: Body) -> Arc<dyn XQueryFunction> {
    let name = QName { prefix: None, uri: Some(FN_NAMESPACE.into()), local: local.into() };
    Arc::new(Builtin { name, parameters, result, variadic: false, body })
}

/// Every built-in function of the core.
pub(super) fn all() -> Vec<Arc<dyn XQueryFunction>> {
    let concat = Arc::new(Builtin {
        name: QName { prefix: None, uri: Some(FN_NAMESPACE.into()), local: "concat".into() },
        parameters: vec![OPTIONAL_ATOMIC, OPTIONAL_ATOMIC],
        result: STRING,
        variadic: true,
        body: concat,
    });
    vec![
        builtin("true", Vec::new(), BOOLEAN, |_, _| Ok(boolean(true))),
        builtin("false", Vec::new(), BOOLEAN, |_, _| Ok(boolean(false))),
        builtin("not", vec![ITEMS], BOOLEAN, |_, a| Ok(boolean(!ebv(first(a))?))),
        builtin("boolean", vec![ITEMS], BOOLEAN, |_, a| Ok(boolean(ebv(first(a))?))),
        builtin("data", vec![ITEMS], ATOMICS, |_, a| Ok(Sequence::new(atomize(first(a)).into_iter().map(Item::Atomic).collect()))),
        builtin("string", Vec::new(), STRING, |c, _| Ok(string(context_item(c)?.map_or_else(String::new, string_value)))),
        builtin("string", vec![ITEM], STRING, |_, a| Ok(string(first(a).first().map_or_else(String::new, string_value)))),
        builtin("count", vec![ITEMS], INTEGER, |_, a| Ok(integer(first(a).len()))),
        builtin("empty", vec![ITEMS], BOOLEAN, |_, a| Ok(boolean(first(a).is_empty()))),
        builtin("exists", vec![ITEMS], BOOLEAN, |_, a| Ok(boolean(!first(a).is_empty()))),
        builtin("position", Vec::new(), INTEGER, |c, _| context_item(c).map(|_| integer(c.position()))),
        builtin("last", Vec::new(), INTEGER, |c, _| context_item(c).map(|_| integer(c.size()))),
        builtin("root", Vec::new(), NODE, root_of_context),
        builtin("root", vec![OPTIONAL_NODE], OPTIONAL_NODE, |_, a| Ok(root(first(a).first()))),
        builtin("doc", vec![OPTIONAL_STRING], OPTIONAL_DOCUMENT, doc),
        concat,
    ]
}

/// The items of the first argument.
fn first(arguments: &[Sequence]) -> &[Item] {
    arguments.first().map_or(&[], Sequence::items)
}

/// The sequence of the `xs:boolean` `b`.
fn boolean(b: bool) -> Sequence {
    Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::Boolean, lexical: if b { "true" } else { "false" }.into() })])
}

/// The sequence of the `xs:integer` `n`.
fn integer(n: usize) -> Sequence {
    Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::Integer, lexical: n.to_string().into() })])
}

/// The sequence of the `xs:string` `s`.
fn string(s: String) -> Sequence {
    Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::String, lexical: s.into() })])
}

/// The context item [XQueryFO §2.3, §16.1]: err:FONC0001 if it is
/// undefined.
fn context_item<'a>(c: &'a Context<'_>) -> Result<Option<&'a Item>> {
    c.item().map(Some).ok_or_else(|| error(ErrorCode::FONC0001, "the context item is undefined [XQueryFO §16.1]"))
}

/// fn:root($arg) [XQueryFO §14.9]: the root of the tree of the node, none
/// of none.
fn root(node: Option<&Item>) -> Sequence {
    let Some(Item::Node(node)) = node else { return Sequence::default() };
    let tree = node.tree();
    let top = std::iter::successors(Some(node.id()), |&id| tree.parent(id)).last().unwrap_or(node.id());
    Sequence::new(vec![Item::Node(Node::at(node, top))])
}

/// fn:root() [XQueryFO §14.9]: of the context item, a node \[err:FOTY0011\].
fn root_of_context(c: &Context<'_>, _: &[Sequence]) -> Result<Sequence> {
    match context_item(c)? {
        Some(item @ Item::Node(_)) => Ok(root(Some(item))),
        Some(Item::Atomic(_)) | None => Err(error(ErrorCode::FOTY0011, "the context item of fn:root is no node [XQueryFO §14.9]")),
    }
}

/// fn:doc($uri) [XQueryFO §15.5.4]: none of none; else, as the available
/// documents are none, err:FODC0005 [XQuery Appendix C.2].
fn doc(_: &Context<'_>, arguments: &[Sequence]) -> Result<Sequence> {
    match first(arguments) {
        [] => Ok(Sequence::default()),
        [uri, ..] => {
            let why = format!("no document is available at '{}', as turborel makes none available [XQueryFO §15.5.4]", string_value(uri));
            Err(error(ErrorCode::FODC0005, &why))
        }
    }
}

/// fn:concat [XQueryFO §7.4.1]: each argument's string, none the
/// zero-length string, joined.
fn concat(_: &Context<'_>, arguments: &[Sequence]) -> Result<Sequence> {
    let joined: String = arguments.iter().flat_map(|a| a.items().iter().map(string_value)).collect();
    if joined.len() > crate::xml::MAX_TEXT {
        return Err(super::super::error::beyond("a string is longer than its limit"));
    }
    Ok(string(joined))
}
