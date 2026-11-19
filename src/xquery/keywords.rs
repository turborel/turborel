//! The keywords of XQuery [XQuery Appendix A.1, A.2.2], each a name the
//! parser recognizes where the grammar has it and the tree is written
//! back with. None is reserved [Appendix A.2]: elsewhere each is a name.

// [XQuery Appendix A.1] The keywords, as the productions write them.
/// `xquery`.
pub(super) const XQUERY: &str = "xquery";
/// `version`.
pub(super) const VERSION: &str = "version";
/// `encoding`.
pub(super) const ENCODING: &str = "encoding";
/// `module`.
pub(super) const MODULE: &str = "module";
/// `namespace`.
pub(super) const NAMESPACE: &str = "namespace";
/// `declare`.
pub(super) const DECLARE: &str = "declare";
/// `boundary-space`.
pub(super) const BOUNDARY_SPACE: &str = "boundary-space";
/// `preserve`.
pub(super) const PRESERVE: &str = "preserve";
/// `strip`.
pub(super) const STRIP: &str = "strip";
/// `default`.
pub(super) const DEFAULT: &str = "default";
/// `element`.
pub(super) const ELEMENT: &str = "element";
/// `function`.
pub(super) const FUNCTION: &str = "function";
/// `option`.
pub(super) const OPTION: &str = "option";
/// `ordering`.
pub(super) const ORDERING: &str = "ordering";
/// `ordered`.
pub(super) const ORDERED: &str = "ordered";
/// `unordered`.
pub(super) const UNORDERED: &str = "unordered";
/// `order`.
pub(super) const ORDER: &str = "order";
/// `empty`.
pub(super) const EMPTY: &str = "empty";
/// `copy-namespaces`.
pub(super) const COPY_NAMESPACES: &str = "copy-namespaces";
/// `no-preserve`.
pub(super) const NO_PRESERVE: &str = "no-preserve";
/// `inherit`.
pub(super) const INHERIT: &str = "inherit";
/// `no-inherit`.
pub(super) const NO_INHERIT: &str = "no-inherit";
/// `collation`.
pub(super) const COLLATION: &str = "collation";
/// `base-uri`.
pub(super) const BASE_URI: &str = "base-uri";
/// `import`.
pub(super) const IMPORT: &str = "import";
/// `schema`.
pub(super) const SCHEMA: &str = "schema";
/// `at`.
pub(super) const AT: &str = "at";
/// `variable`.
pub(super) const VARIABLE: &str = "variable";
/// `construction`.
pub(super) const CONSTRUCTION: &str = "construction";
/// `as`.
pub(super) const AS: &str = "as";
/// `external`.
pub(super) const EXTERNAL: &str = "external";
/// `return`.
pub(super) const RETURN: &str = "return";
/// `for`.
pub(super) const FOR: &str = "for";
/// `in`.
pub(super) const IN: &str = "in";
/// `let`.
pub(super) const LET: &str = "let";
/// `where`.
pub(super) const WHERE: &str = "where";
/// `by`.
pub(super) const BY: &str = "by";
/// `stable`.
pub(super) const STABLE: &str = "stable";
/// `ascending`.
pub(super) const ASCENDING: &str = "ascending";
/// `descending`.
pub(super) const DESCENDING: &str = "descending";
/// `greatest`.
pub(super) const GREATEST: &str = "greatest";
/// `least`.
pub(super) const LEAST: &str = "least";
/// `some`.
pub(super) const SOME: &str = "some";
/// `every`.
pub(super) const EVERY: &str = "every";
/// `satisfies`.
pub(super) const SATISFIES: &str = "satisfies";
/// `typeswitch`.
pub(super) const TYPESWITCH: &str = "typeswitch";
/// `case`.
pub(super) const CASE: &str = "case";
/// `if`.
pub(super) const IF: &str = "if";
/// `then`.
pub(super) const THEN: &str = "then";
/// `else`.
pub(super) const ELSE: &str = "else";
/// `or`.
pub(super) const OR: &str = "or";
/// `and`.
pub(super) const AND: &str = "and";
/// `eq`.
pub(super) const EQ: &str = "eq";
/// `ne`.
pub(super) const NE: &str = "ne";
/// `lt`.
pub(super) const LT: &str = "lt";
/// `le`.
pub(super) const LE: &str = "le";
/// `gt`.
pub(super) const GT: &str = "gt";
/// `ge`.
pub(super) const GE: &str = "ge";
/// `is`.
pub(super) const IS: &str = "is";
/// `to`.
pub(super) const TO: &str = "to";
/// `div`.
pub(super) const DIV: &str = "div";
/// `idiv`.
pub(super) const IDIV: &str = "idiv";
/// `mod`.
pub(super) const MOD: &str = "mod";
/// `union`.
pub(super) const UNION: &str = "union";
/// `intersect`.
pub(super) const INTERSECT: &str = "intersect";
/// `except`.
pub(super) const EXCEPT: &str = "except";
/// `instance`.
pub(super) const INSTANCE: &str = "instance";
/// `of`.
pub(super) const OF: &str = "of";
/// `treat`.
pub(super) const TREAT: &str = "treat";
/// `castable`.
pub(super) const CASTABLE: &str = "castable";
/// `cast`.
pub(super) const CAST: &str = "cast";
/// `validate`.
pub(super) const VALIDATE: &str = "validate";
/// `lax`.
pub(super) const LAX: &str = "lax";
/// `strict`.
pub(super) const STRICT: &str = "strict";
/// `child`.
pub(super) const CHILD: &str = "child";
/// `descendant`.
pub(super) const DESCENDANT: &str = "descendant";
/// `attribute`.
pub(super) const ATTRIBUTE: &str = "attribute";
/// `self`.
pub(super) const SELF_AXIS: &str = "self";
/// `descendant-or-self`.
pub(super) const DESCENDANT_OR_SELF: &str = "descendant-or-self";
/// `following-sibling`.
pub(super) const FOLLOWING_SIBLING: &str = "following-sibling";
/// `following`.
pub(super) const FOLLOWING: &str = "following";
/// `parent`.
pub(super) const PARENT: &str = "parent";
/// `ancestor`.
pub(super) const ANCESTOR: &str = "ancestor";
/// `preceding-sibling`.
pub(super) const PRECEDING_SIBLING: &str = "preceding-sibling";
/// `preceding`.
pub(super) const PRECEDING: &str = "preceding";
/// `ancestor-or-self`.
pub(super) const ANCESTOR_OR_SELF: &str = "ancestor-or-self";
/// `document`.
pub(super) const DOCUMENT: &str = "document";
/// `text`.
pub(super) const TEXT: &str = "text";
/// `comment`.
pub(super) const COMMENT: &str = "comment";
/// `processing-instruction`.
pub(super) const PROCESSING_INSTRUCTION: &str = "processing-instruction";
/// `empty-sequence`.
pub(super) const EMPTY_SEQUENCE: &str = "empty-sequence";
/// `item`.
pub(super) const ITEM: &str = "item";
/// `node`.
pub(super) const NODE: &str = "node";
/// `document-node`.
pub(super) const DOCUMENT_NODE: &str = "document-node";
/// `schema-attribute`.
pub(super) const SCHEMA_ATTRIBUTE: &str = "schema-attribute";
/// `schema-element`.
pub(super) const SCHEMA_ELEMENT: &str = "schema-element";
