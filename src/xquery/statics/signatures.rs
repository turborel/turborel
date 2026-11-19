//! The signatures of the functions of XQuery 1.0 Functions and Operators
//! \[XQueryFO\], as its sections print them, which the static typing of a
//! call reads [XQueryFS §4.1.5]: each function's local name in the
//! namespace `fn`, its parameters' types and its result's, and whether it
//! takes any number of arguments past its parameters, as fn:concat does
//! [XQueryFO §7.4.1].
//!
//! \[DESIGN\] RULES.md D106: a built-in function declares its parameters
//! `item()*` to the evaluation, as it converts its arguments itself
//! (`functions/builtin.rs`); the static typing needs the types its
//! signature declares, so they are a table here, a row of each signature,
//! written as \[XQueryFO\] writes it.

use super::super::ast::Occurrence;
use super::super::types::{ElementKind, Imports, ItemKind, NodeKind, SchemaType, SeqType};
use crate::xml::AtomicType;

/// A function's signature [XQueryFO §1.4].
pub struct Signature {
    /// Its local name in the namespace `fn`.
    pub local: &'static str,
    /// Its parameters' types.
    pub parameters: &'static [&'static str],
    /// Its result's type.
    pub result: &'static str,
    /// Whether it takes more arguments of its last parameter's type.
    pub variadic: bool,
}

/// [XQueryFO] Every signature of a function of the namespace `fn`.
pub const SIGNATURES: [Signature; 150] = [
    Signature { local: "QName", parameters: &["xs:string?", "xs:string"], result: "xs:QName", variadic: false },
    Signature { local: "abs", parameters: &["numeric?"], result: "numeric?", variadic: false },
    Signature { local: "adjust-date-to-timezone", parameters: &["xs:date?"], result: "xs:date?", variadic: false },
    Signature { local: "adjust-date-to-timezone", parameters: &["xs:date?", "xdt:dayTimeDuration?"], result: "xs:date?", variadic: false },
    Signature { local: "adjust-dateTime-to-timezone", parameters: &["xs:dateTime?"], result: "xs:dateTime?", variadic: false },
    Signature {
        local: "adjust-dateTime-to-timezone",
        parameters: &["xs:dateTime?", "xdt:dayTimeDuration?"],
        result: "xs:dateTime?",
        variadic: false,
    },
    Signature { local: "adjust-time-to-timezone", parameters: &["xs:time?"], result: "xs:time?", variadic: false },
    Signature { local: "adjust-time-to-timezone", parameters: &["xs:time?", "xdt:dayTimeDuration?"], result: "xs:time?", variadic: false },
    Signature { local: "avg", parameters: &["xdt:anyAtomicType*"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "base-uri", parameters: &[], result: "xs:anyURI?", variadic: false },
    Signature { local: "base-uri", parameters: &["node()?"], result: "xs:anyURI?", variadic: false },
    Signature { local: "boolean", parameters: &["item()*"], result: "xs:boolean", variadic: false },
    Signature { local: "ceiling", parameters: &["numeric?"], result: "numeric?", variadic: false },
    Signature { local: "codepoint-equal", parameters: &["xs:string?", "xs:string?"], result: "xs:boolean?", variadic: false },
    Signature { local: "codepoints-to-string", parameters: &["xs:integer*"], result: "xs:string", variadic: false },
    Signature { local: "collection", parameters: &[], result: "node()*", variadic: false },
    Signature { local: "collection", parameters: &["xs:string?"], result: "node()*", variadic: false },
    Signature { local: "compare", parameters: &["xs:string?", "xs:string?"], result: "xs:integer?", variadic: false },
    Signature { local: "compare", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:integer?", variadic: false },
    Signature { local: "concat", parameters: &["xdt:anyAtomicType?", "xdt:anyAtomicType?"], result: "xs:string", variadic: true },
    Signature { local: "contains", parameters: &["xs:string?", "xs:string?"], result: "xs:boolean", variadic: false },
    Signature { local: "contains", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "count", parameters: &["item()*"], result: "xs:integer", variadic: false },
    Signature { local: "current-date", parameters: &[], result: "xs:date", variadic: false },
    Signature { local: "current-dateTime", parameters: &[], result: "xs:dateTime", variadic: false },
    Signature { local: "current-time", parameters: &[], result: "xs:time", variadic: false },
    Signature { local: "data", parameters: &["item()*"], result: "xdt:anyAtomicType*", variadic: false },
    Signature { local: "dateTime", parameters: &["xs:date", "xs:time"], result: "xs:dateTime", variadic: false },
    Signature { local: "day-from-date", parameters: &["xs:date?"], result: "xs:integer?", variadic: false },
    Signature { local: "day-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:integer?", variadic: false },
    Signature { local: "days-from-duration", parameters: &["xdt:dayTimeDuration?"], result: "xs:integer?", variadic: false },
    Signature { local: "deep-equal", parameters: &["item()*", "item()*"], result: "xs:boolean", variadic: false },
    Signature { local: "deep-equal", parameters: &["item()*", "item()*", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "default-collation", parameters: &[], result: "xs:string", variadic: false },
    Signature { local: "distinct-values", parameters: &["xdt:anyAtomicType*"], result: "xdt:anyAtomicType*", variadic: false },
    Signature { local: "distinct-values", parameters: &["xdt:anyAtomicType*", "xs:string"], result: "xdt:anyAtomicType*", variadic: false },
    Signature { local: "doc", parameters: &["xs:string?"], result: "document-node()?", variadic: false },
    Signature { local: "doc-available", parameters: &["xs:string?"], result: "xs:boolean", variadic: false },
    Signature { local: "document-uri", parameters: &["node()?"], result: "xs:anyURI?", variadic: false },
    Signature { local: "empty", parameters: &["item()*"], result: "xs:boolean", variadic: false },
    Signature { local: "encode-for-uri", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "ends-with", parameters: &["xs:string?", "xs:string?"], result: "xs:boolean", variadic: false },
    Signature { local: "ends-with", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "error", parameters: &[], result: "none", variadic: false },
    Signature { local: "error", parameters: &["xs:QName"], result: "none", variadic: false },
    Signature { local: "error", parameters: &["xs:QName?", "xs:string"], result: "none", variadic: false },
    Signature { local: "error", parameters: &["xs:QName?", "xs:string", "item()*"], result: "none", variadic: false },
    Signature { local: "escape-html-uri", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "exactly-one", parameters: &["item()*"], result: "item()", variadic: false },
    Signature { local: "exists", parameters: &["item()*"], result: "xs:boolean", variadic: false },
    Signature { local: "false", parameters: &[], result: "xs:boolean", variadic: false },
    Signature { local: "floor", parameters: &["numeric?"], result: "numeric?", variadic: false },
    Signature { local: "hours-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:integer?", variadic: false },
    Signature { local: "hours-from-duration", parameters: &["xdt:dayTimeDuration?"], result: "xs:integer?", variadic: false },
    Signature { local: "hours-from-time", parameters: &["xs:time?"], result: "xs:integer?", variadic: false },
    Signature { local: "id", parameters: &["xs:string*"], result: "element()*", variadic: false },
    Signature { local: "id", parameters: &["xs:string*", "node()"], result: "element()*", variadic: false },
    Signature { local: "idref", parameters: &["xs:string*"], result: "node()*", variadic: false },
    Signature { local: "idref", parameters: &["xs:string*", "node()"], result: "node()*", variadic: false },
    Signature { local: "implicit-timezone", parameters: &[], result: "xdt:dayTimeDuration", variadic: false },
    Signature { local: "in-scope-prefixes", parameters: &["element()"], result: "xs:string*", variadic: false },
    Signature { local: "index-of", parameters: &["xdt:anyAtomicType*", "xdt:anyAtomicType"], result: "xs:integer*", variadic: false },
    Signature {
        local: "index-of",
        parameters: &["xdt:anyAtomicType*", "xdt:anyAtomicType", "xs:string"],
        result: "xs:integer*",
        variadic: false,
    },
    Signature { local: "insert-before", parameters: &["item()*", "xs:integer", "item()*"], result: "item()*", variadic: false },
    Signature { local: "iri-to-uri", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "lang", parameters: &["xs:string?"], result: "xs:boolean", variadic: false },
    Signature { local: "lang", parameters: &["xs:string?", "node()"], result: "xs:boolean", variadic: false },
    Signature { local: "last", parameters: &[], result: "xs:integer", variadic: false },
    Signature { local: "local-name", parameters: &[], result: "xs:string", variadic: false },
    Signature { local: "local-name", parameters: &["node()?"], result: "xs:string", variadic: false },
    Signature { local: "local-name-from-QName", parameters: &["xs:QName?"], result: "xs:NCName?", variadic: false },
    Signature { local: "lower-case", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "matches", parameters: &["xs:string?", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "matches", parameters: &["xs:string?", "xs:string", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "max", parameters: &["xdt:anyAtomicType*"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "max", parameters: &["xdt:anyAtomicType*", "xs:string"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "min", parameters: &["xdt:anyAtomicType*"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "min", parameters: &["xdt:anyAtomicType*", "xs:string"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "minutes-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:integer?", variadic: false },
    Signature { local: "minutes-from-duration", parameters: &["xdt:dayTimeDuration?"], result: "xs:integer?", variadic: false },
    Signature { local: "minutes-from-time", parameters: &["xs:time?"], result: "xs:integer?", variadic: false },
    Signature { local: "month-from-date", parameters: &["xs:date?"], result: "xs:integer?", variadic: false },
    Signature { local: "month-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:integer?", variadic: false },
    Signature { local: "months-from-duration", parameters: &["xdt:yearMonthDuration?"], result: "xs:integer?", variadic: false },
    Signature { local: "name", parameters: &[], result: "xs:string", variadic: false },
    Signature { local: "name", parameters: &["node()?"], result: "xs:string", variadic: false },
    Signature { local: "namespace-uri", parameters: &[], result: "xs:anyURI", variadic: false },
    Signature { local: "namespace-uri", parameters: &["node()?"], result: "xs:anyURI", variadic: false },
    Signature { local: "namespace-uri-for-prefix", parameters: &["xs:string?", "element()"], result: "xs:anyURI?", variadic: false },
    Signature { local: "namespace-uri-from-QName", parameters: &["xs:QName?"], result: "xs:anyURI?", variadic: false },
    Signature { local: "nilled", parameters: &["node()?"], result: "xs:boolean?", variadic: false },
    Signature { local: "node-name", parameters: &["node()?"], result: "xs:QName?", variadic: false },
    Signature { local: "normalize-space", parameters: &[], result: "xs:string", variadic: false },
    Signature { local: "normalize-space", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "normalize-unicode", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "normalize-unicode", parameters: &["xs:string?", "xs:string"], result: "xs:string", variadic: false },
    Signature { local: "not", parameters: &["item()*"], result: "xs:boolean", variadic: false },
    Signature { local: "number", parameters: &[], result: "xs:double", variadic: false },
    Signature { local: "number", parameters: &["xdt:anyAtomicType?"], result: "xs:double", variadic: false },
    Signature { local: "one-or-more", parameters: &["item()*"], result: "item()+", variadic: false },
    Signature { local: "position", parameters: &[], result: "xs:integer", variadic: false },
    Signature { local: "prefix-from-QName", parameters: &["xs:QName?"], result: "xs:NCName?", variadic: false },
    Signature { local: "remove", parameters: &["item()*", "xs:integer"], result: "item()*", variadic: false },
    Signature { local: "replace", parameters: &["xs:string?", "xs:string", "xs:string"], result: "xs:string", variadic: false },
    Signature {
        local: "replace",
        parameters: &["xs:string?", "xs:string", "xs:string", "xs:string"],
        result: "xs:string",
        variadic: false,
    },
    Signature { local: "resolve-QName", parameters: &["xs:string?", "element()"], result: "xs:QName?", variadic: false },
    Signature { local: "resolve-uri", parameters: &["xs:string?"], result: "xs:anyURI?", variadic: false },
    Signature { local: "resolve-uri", parameters: &["xs:string?", "xs:string"], result: "xs:anyURI?", variadic: false },
    Signature { local: "reverse", parameters: &["item()*"], result: "item()*", variadic: false },
    Signature { local: "root", parameters: &[], result: "node()", variadic: false },
    Signature { local: "root", parameters: &["node()?"], result: "node()?", variadic: false },
    Signature { local: "round", parameters: &["numeric?"], result: "numeric?", variadic: false },
    Signature { local: "round-half-to-even", parameters: &["numeric?"], result: "numeric?", variadic: false },
    Signature { local: "round-half-to-even", parameters: &["numeric?", "xs:integer"], result: "numeric?", variadic: false },
    Signature { local: "seconds-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:decimal?", variadic: false },
    Signature { local: "seconds-from-duration", parameters: &["xdt:dayTimeDuration?"], result: "xs:decimal?", variadic: false },
    Signature { local: "seconds-from-time", parameters: &["xs:time?"], result: "xs:decimal?", variadic: false },
    Signature { local: "starts-with", parameters: &["xs:string?", "xs:string?"], result: "xs:boolean", variadic: false },
    Signature { local: "starts-with", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:boolean", variadic: false },
    Signature { local: "static-base-uri", parameters: &[], result: "xs:anyURI?", variadic: false },
    Signature { local: "string", parameters: &[], result: "xs:string", variadic: false },
    Signature { local: "string", parameters: &["item()?"], result: "xs:string", variadic: false },
    Signature { local: "string-join", parameters: &["xs:string*", "xs:string"], result: "xs:string", variadic: false },
    Signature { local: "string-length", parameters: &[], result: "xs:integer", variadic: false },
    Signature { local: "string-length", parameters: &["xs:string?"], result: "xs:integer", variadic: false },
    Signature { local: "string-to-codepoints", parameters: &["xs:string?"], result: "xs:integer*", variadic: false },
    Signature { local: "subsequence", parameters: &["item()*", "xs:double"], result: "item()*", variadic: false },
    Signature { local: "subsequence", parameters: &["item()*", "xs:double", "xs:double"], result: "item()*", variadic: false },
    Signature { local: "substring", parameters: &["xs:string?", "xs:double"], result: "xs:string", variadic: false },
    Signature { local: "substring", parameters: &["xs:string?", "xs:double", "xs:double"], result: "xs:string", variadic: false },
    Signature { local: "substring-after", parameters: &["xs:string?", "xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "substring-after", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:string", variadic: false },
    Signature { local: "substring-before", parameters: &["xs:string?", "xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "substring-before", parameters: &["xs:string?", "xs:string?", "xs:string"], result: "xs:string", variadic: false },
    Signature { local: "sum", parameters: &["xdt:anyAtomicType*"], result: "xdt:anyAtomicType", variadic: false },
    Signature { local: "sum", parameters: &["xdt:anyAtomicType*", "xdt:anyAtomicType?"], result: "xdt:anyAtomicType?", variadic: false },
    Signature { local: "timezone-from-date", parameters: &["xs:date?"], result: "xdt:dayTimeDuration?", variadic: false },
    Signature { local: "timezone-from-dateTime", parameters: &["xs:dateTime?"], result: "xdt:dayTimeDuration?", variadic: false },
    Signature { local: "timezone-from-time", parameters: &["xs:time?"], result: "xdt:dayTimeDuration?", variadic: false },
    Signature { local: "tokenize", parameters: &["xs:string?", "xs:string"], result: "xs:string*", variadic: false },
    Signature { local: "tokenize", parameters: &["xs:string?", "xs:string", "xs:string"], result: "xs:string*", variadic: false },
    Signature { local: "trace", parameters: &["item()*", "xs:string"], result: "item()*", variadic: false },
    Signature { local: "translate", parameters: &["xs:string?", "xs:string", "xs:string"], result: "xs:string", variadic: false },
    Signature { local: "true", parameters: &[], result: "xs:boolean", variadic: false },
    Signature { local: "unordered", parameters: &["item()*"], result: "item()*", variadic: false },
    Signature { local: "upper-case", parameters: &["xs:string?"], result: "xs:string", variadic: false },
    Signature { local: "year-from-date", parameters: &["xs:date?"], result: "xs:integer?", variadic: false },
    Signature { local: "year-from-dateTime", parameters: &["xs:dateTime?"], result: "xs:integer?", variadic: false },
    Signature { local: "years-from-duration", parameters: &["xdt:yearMonthDuration?"], result: "xs:integer?", variadic: false },
    Signature { local: "zero-or-one", parameters: &["item()*"], result: "item()?", variadic: false },
];

/// The signature of `fn:local` of `arity` arguments, if \[XQueryFO\] has
/// one.
#[must_use]
pub fn signature(local: &str, arity: usize) -> Option<&'static Signature> {
    SIGNATURES.iter().find(|s| s.local == local && (s.parameters.len() == arity || (s.variadic && arity >= s.parameters.len())))
}

/// The item types the signatures write by name, as they write them.
const ITEM: &str = "item()";
/// `node()`.
const NODE: &str = "node()";
/// [XQueryFO §1.4] `numeric`.
const NUMERIC: &str = "numeric";
/// [XQueryFO §3] `none`, fn:error's.
const NONE: &str = "none";
/// `element()`.
const ELEMENT: &str = "element()";
/// `document-node()`.
const DOCUMENT: &str = "document-node()";
/// `xdt:anyAtomicType`.
const ANY_ATOMIC: &str = "xdt:anyAtomicType";

/// The occurrence indicators a signature writes, each with what it is.
const OCCURRENCES: [(char, Occurrence); 3] = [('?', Occurrence::Optional), ('*', Occurrence::Any), ('+', Occurrence::Many)];

/// The sequence type a signature writes as `text`: `item()`, `node()`,
/// `element()`, `document-node()`, `numeric` or an atomic type's name,
/// with its occurrence indicator; none of `none`, the type of no value,
/// and of a name of no type.
#[must_use]
pub fn sequence_type(text: &str) -> Option<SeqType> {
    let found = OCCURRENCES.iter().find_map(|(c, o)| text.strip_suffix(*c).map(|item| (item, *o)));
    let (item, occurrence) = found.unwrap_or((text, Occurrence::One));
    let kind = if item == ITEM {
        ItemKind::Item
    } else if item == NODE {
        ItemKind::Node(NodeKind::Any)
    } else if item == NUMERIC {
        ItemKind::Numeric
    } else if item == ELEMENT {
        let any = ElementKind { name: None, substitutes: Vec::new(), annotation: None, nillable: true, imports: Imports::default() };
        ItemKind::Node(NodeKind::Element(any))
    } else if item == DOCUMENT {
        ItemKind::Node(NodeKind::Document(None))
    } else if item == ANY_ATOMIC {
        ItemKind::Atomic(SchemaType::AnyAtomicType)
    } else if item == NONE {
        return None;
    } else {
        ItemKind::Atomic(SchemaType::Atomic(AtomicType::all().find(|t| t.name() == item)?))
    };
    Some(SeqType::Of(kind, occurrence))
}
