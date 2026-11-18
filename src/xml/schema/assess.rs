//! Schema-validity assessment [Schema1 §3.3.4, §5.2] of a document or
//! element node against a schema, as XQuery's validate expression does it
//! [XQuery §3.13], which SQL/XML's XMLVALIDATE and IS VALID evaluate
//! [ISO/IEC 9075-14:2006 draft §10.22]; and the post-schema-validation
//! infoset it makes, mapped to the data model [XQueryDM §3.3]: a new tree,
//! its element and attribute nodes annotated with their types, their
//! values normalized, default attributes and element values added, nilled
//! elements marked.
//!
//! An element is assessed by the declaration its parent's content model
//! determines, or the one its name resolves to, or the type its
//! `xsi:type` names (`assess/element.rs`); its attributes by its type's
//! attribute uses and wildcard (`assess/attributes.rs`); its children
//! by its content model's automaton (`assess/content.rs`). Identity
//! constraints and the ID/IDREF table of the validation root are checked
//! once the tree is made (`assess/identity.rs`).
//!
//! \[DESIGN\] Nothing recurses: elements are assessed on a stack of those
//! open, each with the state of its content model, so the depth of what is
//! validated is the depth of the tree, which `MAX_NESTING` bounds. The
//! first failure ends the assessment, as any makes the validation root's
//! [validity] invalid [Schema1 §3.3.5], which XQuery's validate reports
//! [XQuery §3.13].

mod attributes;
mod content;
mod element;
pub(super) use element::{ANONYMOUS, ANONYMOUS_SIMPLE, simple_type_name, type_name};
mod identity;
mod run;

#[cfg(test)]
mod tests;

use super::component::Schema;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::xml::Node;

/// [XQuery §3.13] A validation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// strict: the validated element has a top-level declaration
    /// [Schema1 §5.2 item 2].
    Strict,
    /// lax: it is validated by its top-level declaration if it has one
    /// [Schema1 §5.2 item 3].
    Lax,
}

/// [XQuery §3.13] Why a document or element node is not valid: the
/// validation rule or the constraint it breaks, and XQuery's error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The rule broken, as cited: `[Schema1 §3.4.4 cvc-complex-type.2.4]`.
    pub rule: String,
    /// What breaks it.
    pub why: String,
    /// XQuery's error code: err:XQDY0027 of an invalid node,
    /// err:XQDY0084 of an element with no declaration in strict mode,
    /// err:XQDY0061 of a document node of other children, err:XQTY0030 of
    /// an operand neither a document nor an element node.
    pub code: &'static str,
}

/// [XQuery §3.13] The operand node is not valid.
pub const INVALID: &str = "err:XQDY0027";
/// [XQuery §3.13] Strict validation of an element with no top-level
/// declaration.
pub const UNDECLARED: &str = "err:XQDY0084";
/// [XQuery §3.13] A document node whose children are not one element and
/// comments and processing instructions.
pub const NOT_A_DOCUMENT: &str = "err:XQDY0061";
/// [XQuery §3.13] An operand that is no document or element node.
pub const NOT_A_NODE: &str = "err:XQTY0030";

impl Failure {
    /// The failure of `rule`, `why`, as err:XQDY0027.
    #[must_use]
    pub fn new(rule: &str, why: impl Into<String>) -> Failure {
        Failure { rule: rule.to_owned(), why: why.into(), code: INVALID }
    }

    /// [ISO/IEC 9075-14:2006 draft §6.20 GR6a iii 2 D, GR6b iv] The
    /// exception of XMLVALIDATE that meets it: data exception, validation
    /// failure, SQLSTATE 2201R.
    #[must_use]
    pub fn validation_failure(&self) -> SqlError {
        SqlError::new(Code::Data, format!("[ISO/IEC 9075-14:2006 draft §6.20 GR6b iv] validation failure: {} {}", self.rule, self.why))
            .with_state(SqlState::VALIDATION_FAILURE)
    }
}

/// What ends an assessment: an error of turborel's limits, or a failure.
#[derive(Debug)]
enum Stop {
    /// An error, as one past `MAX_NODES`.
    Error(SqlError),
    /// The node is not valid.
    Invalid(Failure),
}

impl From<SqlError> for Stop {
    fn from(e: SqlError) -> Stop {
        Stop::Error(e)
    }
}

impl From<Failure> for Stop {
    fn from(f: Failure) -> Stop {
        Stop::Invalid(f)
    }
}

/// The result of a step of an assessment.
type Step<T> = std::result::Result<T, Stop>;

/// [XQuery §3.13] [ISO/IEC 9075-14:2006 draft §10.22] `node`, a document
/// or element node, validated against `schema` in `mode`: a new node of
/// its kind, with no parent, of the post-schema-validation infoset's
/// types, values and defaults [XQueryDM §3.3]. Of an element node, the
/// identity constraints and the ID/IDREF table are not checked [XQuery
/// §3.13].
///
/// # Errors
///
/// The outer error is turborel's: SQLCODE -900 if the result would be
/// past a tree's limits. The inner `Failure` says why `node` is not
/// valid, or cannot be validated.
pub fn validate(schema: &Schema, node: &Node, mode: Mode) -> Result<std::result::Result<Node, Failure>> {
    match run::run(schema, node, mode) {
        Ok(out) => Ok(Ok(out)),
        Err(Stop::Invalid(f)) => Ok(Err(f)),
        Err(Stop::Error(e)) => Err(e),
    }
}
