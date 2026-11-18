//! XQuery 1.0, the language XMLQUERY evaluates [ISO/IEC 9075-14:2006
//! draft §6.18], read into an abstract syntax tree. \[XQuery\] is the W3C
//! Candidate Recommendation of 3 November 2005, the edition the draft
//! names [§2.2], cited by its section, `[XQuery §3.2]`, or its grammar's
//! productions, `[XQuery Appendix A.1 [68]]`.
//!
//! A query's text has its line ends normalized [XQuery Appendix A.2.3]
//! and is read by a lexer (`xquery/lexer.rs`) whose tokens the parser
//! (`xquery/parser.rs`) asks for one at a time, so the lexer reads each
//! part of the text in the state the grammar puts it in: expressions,
//! where whitespace and comments separate tokens [Appendix A.2.4.1], or
//! the direct constructors' tags, attribute values and content, where
//! whitespace is explicit and comments are text [Appendix A.2.4.2]. The
//! tree (`xquery/ast.rs`) is the grammar's, production by production, and
//! writes itself back as text (`xquery/display.rs`) that parses to the
//! same tree.
//!
//! \[DESIGN\] RULES.md D86: the parser descends recursively, one level
//! each expression and direct element constructor nested in another, and
//! refuses a query nested deeper than `MAX_NESTING`. A chain of operators
//! of one precedence is one node of the tree, so a long chain makes no
//! deep tree.
//!
//! The parser raises what the text alone shows: syntax errors
//! \[err:XPST0003\], and the static errors of the prolog and the direct
//! constructors that need no static context (`ErrorCode`). A module is then
//! analysed in its static context (`xquery/analyze.rs`) [XQuery §2.2.3.1]
//! into a plan (`xquery/plan.rs`), which is evaluated (`xquery/eval.rs`)
//! [XQuery §2.2.3.2]; its types are `xquery/types.rs`'s, its collations
//! `xquery/collation.rs`'s, the functions it calls a library of
//! `XQueryFunction`s (`xquery/functions.rs`). A compiled query is a
//! `Query`.

mod analyze;
mod ast;
mod collation;
mod display;
mod error;
mod eval;
mod functions;
mod keywords;
mod lexer;
mod parser;
mod plan;
#[cfg(test)]
mod testing;
mod types;

pub use analyze::{EMPTY_ORDER, Environment};
pub use ast::{AddOp, Flwor, GeneralComp, MulOp, NodeComp, PreserveMode, SetOp, Sign, ValueComp};
pub use ast::{
    AttributeTest, AttributeValuePart, AxisStep, BoundarySpace, CaseClause, Comparator, Comparison, ComputedName, Construction,
    Constructor, Declaration, DefaultNamespaceDecl, DirAttribute, DirContent, DirElement, DirPi, Direction, DocumentTest, ElementTest,
    EmptyOrder, Expr, FlworClause, ForBinding, ForwardAxis, FunctionDecl, If, Import, InheritMode, ItemType, KindTest, LetBinding, Literal,
    Module, ModuleBody, ModuleDecl, ModuleImport, NameOrWildcard, NameTest, NamespaceDecl, NamespaceKind, NodeTest, Occurrence, OptionDecl,
    OrderBy, OrderSpec, OrderingMode, Param, Path, PiName, PiTest, Pragma, Prolog, QName, Quantified, QuantifiedBinding, Quantifier,
    ReverseAxis, Root, SchemaImport, SchemaPrefix, Separator, SequenceType, Setter, Setup, SingleType, Step, StepExpr, Typeswitch,
    ValidationMode, VarDecl, VersionDecl, boundary_whitespace,
};
pub use collation::{CODEPOINT, Collations, Collator, SQL_COLLATION};
pub use error::{ErrorCode, error};
pub use eval::{Input, MAX_DEPTH};
pub use functions::{Context, FN_NAMESPACE, Library, XQueryFunction};
pub use types::{AttributeKind, ElementKind, ItemKind, NodeKind, SchemaType, SeqType, XDT_NAMESPACE};

use crate::error::Result;
use crate::xml::Version;

/// \[DESIGN\] How deep expressions, and direct element constructors, nest
/// at most in a query, so the parser's recursion, and every walk of the
/// tree it makes, is bounded (RULES.md R-CF-6, D86). \[XQuery\] sets no
/// limit, so a query nested deeper is SQLCODE -900 (D18). The value is
/// no measurement: it is past the nesting of the queries an application
/// writes, and the deepest query of each shape parses within half of a
/// thread's default stack.
pub const MAX_NESTING: usize = 64;

/// The module `text` is, read by the lexical rules of XML `version`
/// [XQuery Appendix A.2]: its names are `NCName`s of that version's
/// Namespaces in XML, its characters that version's `Char`s, its line
/// ends normalized as that version normalizes them. XMLQUERY reads by
/// XML 1.1's rules, as turborel supports Feature X211 [ISO/IEC
/// 9075-14:2006 draft §6.18 GR2a].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error [ISO/IEC 9075-14:2006 draft §6.18 GR3],
/// its message naming the static error \[XQuery\] raises and where:
/// err:XPST0003 if `text` is no module of the grammar [XQuery Appendix
/// A.1], or another static error the text alone shows (`ErrorCode`);
/// SQLCODE -900 if it nests deeper than `MAX_NESTING`.
pub fn parse(text: &str, version: Version) -> Result<Module> {
    parser::module(text, version)
}

/// A query, parsed and analysed in its static context [XQuery §2.2.3.1],
/// to be evaluated as often as it is given input [XQuery §2.2.3.2]. It is
/// shared by every evaluation of a statement, and changes in none.
#[derive(Debug)]
pub struct Query(plan::Plan);

impl Query {
    /// The query `text`, parsed by XML 1.1's lexical rules [ISO/IEC
    /// 9075-14:2006 draft §6.18 GR2a] and analysed in the static context
    /// `env` gives [§10.20].
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, XQuery error, of a static error \[XQuery\] raises
    /// [§6.18 GR3]; SQLCODE -900 of a query nested deeper than
    /// `MAX_NESTING`, or valid XQuery turborel does not process yet.
    pub fn compile(text: &str, env: &Environment<'_>) -> Result<Query> {
        let module = parse(text, Version::V11)?;
        analyze::compile(&module, env).map(Query)
    }

    /// Its value, given `input` [XQuery §2.2.3.2].
    ///
    /// # Errors
    ///
    /// SQLSTATE 10000, XQuery error, of a dynamic or type error \[XQuery\]
    /// raises [§6.18 GR3]; SQLCODE -900 of an evaluation past turborel's
    /// limits; SQLCODE -920 if the statement is cancelled.
    pub fn evaluate(&self, input: Input<'_>) -> Result<crate::xml::Sequence> {
        eval::evaluate(&self.0, input)
    }
}

#[cfg(test)]
mod tests;
