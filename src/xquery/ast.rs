//! The abstract syntax tree of an XQuery module [XQuery Appendix A.1]:
//! modules and their prologs here [XQuery §4], expressions in
//! `ast/expr.rs` [XQuery §3], constructors in `ast/direct.rs` [XQuery
//! §3.7], and sequence types and node tests in `ast/types.rs` [XQuery
//! §2.5.3, §3.2.1.2].
//!
//! A node is named after the production it stands for. What the text
//! writes in more than one way with one meaning, `union` and `|`, `<a/>`
//! and `<a></a>`, a string literal's quotes and references, is one node;
//! what it writes once is kept as written, so a tree written back as text
//! (`xquery/display.rs`) parses to the same tree.

mod direct;
mod expr;
mod types;

pub use direct::{AttributeValuePart, ComputedName, Constructor, DirAttribute, DirContent, DirElement, DirPi, PiName, boundary_whitespace};
pub use expr::{
    AddOp, AxisStep, CaseClause, Comparator, Comparison, Direction, Expr, Flwor, FlworClause, ForBinding, ForwardAxis, GeneralComp, If,
    LetBinding, Literal, MulOp, NodeComp, OrderBy, OrderSpec, Path, Pragma, Quantified, QuantifiedBinding, Quantifier, ReverseAxis, Root,
    Separator, SetOp, Sign, Step, StepExpr, Typeswitch, ValidationMode, ValueComp,
};
pub use types::{
    AttributeTest, DocumentTest, ElementTest, ItemType, KindTest, NameOrWildcard, NameTest, NodeTest, Occurrence, PiTest, SequenceType,
    SingleType,
};

/// A lexical QName [XQuery Appendix A.2.1 \[154\]] [Namespaces 1.0 §3 \[6\]] [Namespaces 1.1 §4 \[6\]]: a
/// local name, and the prefix before its colon if it has one. Its
/// namespace is resolved by the static context, which the parser does not
/// have [XQuery §2.1.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QName {
    /// The prefix, an `NCName`.
    pub prefix: Option<String>,
    /// The local part, an `NCName`.
    pub local: String,
}

impl QName {
    /// The QName with no prefix and the local part `local`.
    #[must_use]
    pub fn local(local: &str) -> QName {
        QName { prefix: None, local: local.to_owned() }
    }

    /// Whether it has no prefix and the local part `local`: whether it is
    /// the keyword `local` where a keyword may be [XQuery Appendix A.2].
    #[must_use]
    pub fn is(&self, local: &str) -> bool {
        self.prefix.is_none() && self.local == local
    }
}

/// `Module` [XQuery Appendix A.1 \[1\]]: a main or library module, after
/// a version declaration if it has one [XQuery §4].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// `VersionDecl` \[2\].
    pub version: Option<VersionDecl>,
    /// What follows it.
    pub body: ModuleBody,
}

/// What a module is after its version declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleBody {
    /// `MainModule` [XQuery Appendix A.1 \[3\]]: a prolog, then the query
    /// body, an `Expr` \[30\].
    Main(Prolog, Expr),
    /// `LibraryModule` \[4\]: a module declaration, then a prolog. The
    /// Module Feature is not supported [XQuery §5.2], so a library module
    /// is parsed and refused by `Module::unsupported`.
    Library(ModuleDecl, Prolog),
}

/// `VersionDecl` [XQuery Appendix A.1 \[2\]] [XQuery §4.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionDecl {
    /// The version, "1.0" as the parser accepts \[err:XQST0031\].
    pub version: String,
    /// The encoding declaration's `EncName`, if it has one.
    pub encoding: Option<String>,
}

/// `ModuleDecl` [XQuery Appendix A.1 \[5\]] [XQuery §4.2].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDecl {
    /// The prefix the module's target namespace is bound to.
    pub prefix: String,
    /// The target namespace, a `URILiteral` \[140\].
    pub uri: String,
}

/// `Prolog` [XQuery Appendix A.1 \[6\]]: its setters, namespace
/// declarations and imports, then its variable, function and option
/// declarations, each in the order written [XQuery §4].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prolog {
    /// The first part.
    pub setup: Vec<Setup>,
    /// The second part.
    pub declarations: Vec<Declaration>,
}

/// One of the first part of a prolog [XQuery Appendix A.1 \[6\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setup {
    /// `DefaultNamespaceDecl` \[12\].
    DefaultNamespace(DefaultNamespaceDecl),
    /// `Setter` \[7\].
    Setter(Setter),
    /// `NamespaceDecl` \[10\].
    Namespace(NamespaceDecl),
    /// `Import` \[8\].
    Import(Import),
}

/// `DefaultNamespaceDecl` [XQuery Appendix A.1 \[12\]] [XQuery §4.13].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultNamespaceDecl {
    /// Whether it is the default element/type or function namespace.
    pub kind: NamespaceKind,
    /// The namespace, the zero-length string for none.
    pub uri: String,
}

/// Which default namespace a `DefaultNamespaceDecl` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceKind {
    /// `element`: of element and type names.
    Element,
    /// `function`: of function names.
    Function,
}

/// `NamespaceDecl` [XQuery Appendix A.1 \[10\]] [XQuery §4.12].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceDecl {
    /// The prefix, an `NCName`.
    pub prefix: String,
    /// The namespace.
    pub uri: String,
}

/// `Setter` [XQuery Appendix A.1 \[7\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setter {
    /// `BoundarySpaceDecl` \[11\] [XQuery §4.3].
    BoundarySpace(BoundarySpace),
    /// `DefaultCollationDecl` \[19\] [XQuery §4.4]: its `URILiteral`.
    DefaultCollation(String),
    /// `BaseURIDecl` \[20\] [XQuery §4.5]: its `URILiteral`.
    BaseUri(String),
    /// `ConstructionDecl` \[25\] [XQuery §4.6].
    Construction(Construction),
    /// `OrderingModeDecl` \[14\] [XQuery §4.7].
    OrderingMode(OrderingMode),
    /// `EmptyOrderDecl` \[15\] [XQuery §4.8].
    EmptyOrder(EmptyOrder),
    /// `CopyNamespacesDecl` \[16\] [XQuery §4.9].
    CopyNamespaces(PreserveMode, InheritMode),
}

/// The boundary-space policy [XQuery Appendix A.1 \[11\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundarySpace {
    /// `preserve`.
    Preserve,
    /// `strip`.
    Strip,
}

/// The construction mode [XQuery Appendix A.1 \[25\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Construction {
    /// `strip`.
    Strip,
    /// `preserve`.
    Preserve,
}

/// The ordering mode [XQuery Appendix A.1 \[14\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingMode {
    /// `ordered`.
    Ordered,
    /// `unordered`.
    Unordered,
}

/// Where the empty sequence orders: of an `EmptyOrderDecl` [XQuery
/// Appendix A.1 \[15\]] and of an `OrderModifier` \[41\].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyOrder {
    /// `empty greatest`.
    Greatest,
    /// `empty least`.
    Least,
}

/// `PreserveMode` [XQuery Appendix A.1 \[17\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreserveMode {
    /// `preserve`.
    Preserve,
    /// `no-preserve`.
    NoPreserve,
}

/// `InheritMode` [XQuery Appendix A.1 \[18\]].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InheritMode {
    /// `inherit`.
    Inherit,
    /// `no-inherit`.
    NoInherit,
}

/// `Import` [XQuery Appendix A.1 \[8\]]. A schema import imports a
/// registered XML Schema (`analyze/prolog.rs`); the Module Feature is not
/// supported [XQuery §5.2], so a module import is parsed and refused by
/// `Module::unsupported`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Import {
    /// `SchemaImport` \[21\] [XQuery §4.10].
    Schema(SchemaImport),
    /// `ModuleImport` \[23\] [XQuery §4.11].
    Module(ModuleImport),
}

/// `SchemaImport` [XQuery Appendix A.1 \[21\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaImport {
    /// `SchemaPrefix` \[22\], if it has one.
    pub prefix: Option<SchemaPrefix>,
    /// The target namespace.
    pub uri: String,
    /// The location hints after `at`.
    pub at: Vec<String>,
}

/// `SchemaPrefix` [XQuery Appendix A.1 \[22\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaPrefix {
    /// `namespace NCName =`.
    Namespace(String),
    /// `default element namespace`.
    DefaultElement,
}

/// `ModuleImport` [XQuery Appendix A.1 \[23\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleImport {
    /// The prefix bound to the target namespace, if one is.
    pub prefix: Option<String>,
    /// The target namespace, of nonzero length \[err:XQST0088\].
    pub uri: String,
    /// The location hints after `at`.
    pub at: Vec<String>,
}

/// One of the second part of a prolog [XQuery Appendix A.1 \[6\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    /// `VarDecl` \[24\].
    Variable(VarDecl),
    /// `FunctionDecl` \[26\].
    Function(FunctionDecl),
    /// `OptionDecl` \[13\].
    Option(OptionDecl),
}

/// `VarDecl` [XQuery Appendix A.1 \[24\]] [XQuery §4.14].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarDecl {
    /// The variable's name.
    pub name: QName,
    /// Its `TypeDeclaration` \[118\], if it has one.
    pub declared: Option<SequenceType>,
    /// Its initializing expression, or none if it is `external`.
    pub value: Option<Expr>,
}

/// `FunctionDecl` [XQuery Appendix A.1 \[26\]] [XQuery §4.15].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    /// The function's name.
    pub name: QName,
    /// Its `ParamList` \[27\], each name once \[err:XQST0039\].
    pub params: Vec<Param>,
    /// The sequence type after `as`, if it has one.
    pub returns: Option<SequenceType>,
    /// Its body, an `EnclosedExpr` \[29\], or none if it is `external`.
    pub body: Option<Expr>,
}

/// `Param` [XQuery Appendix A.1 \[28\]].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// The parameter's name.
    pub name: QName,
    /// Its `TypeDeclaration`, if it has one.
    pub declared: Option<SequenceType>,
}

/// `OptionDecl` [XQuery Appendix A.1 \[13\]] [XQuery §4.16].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionDecl {
    /// The option's name.
    pub name: QName,
    /// Its value, a `StringLiteral`'s.
    pub value: String,
}

impl Module {
    /// The prolog.
    #[must_use]
    pub fn prolog(&self) -> &Prolog {
        match &self.body {
            ModuleBody::Main(prolog, _) | ModuleBody::Library(_, prolog) => prolog,
        }
    }

    /// The static error \[XQuery\] requires of an implementation without
    /// the Module Feature, which turborel does not support [XQuery §5.2],
    /// if the module needs it: err:XQST0016 for a library module or a
    /// module import [XQuery Appendix F].
    #[must_use]
    pub fn unsupported(&self) -> Option<crate::xquery::ErrorCode> {
        if matches!(self.body, ModuleBody::Library(..)) {
            return Some(crate::xquery::ErrorCode::XQST0016);
        }
        self.prolog().setup.iter().find_map(|setup| match setup {
            Setup::Import(Import::Module(_)) => Some(crate::xquery::ErrorCode::XQST0016),
            Setup::Import(Import::Schema(_)) | Setup::DefaultNamespace(_) | Setup::Setter(_) | Setup::Namespace(_) => None,
        })
    }
}
