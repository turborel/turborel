//! The validate expression evaluated [XQuery §3.13]: its operand, one
//! document or element node, validated in its mode (`xml::schema::validate`)
//! against its effective schema, of the in-scope schema definitions
//! [ISO/IEC 9075-14:2006 draft §10.20 SR2e].
//!
//! \[DESIGN\] RULES.md D86: the in-scope schema definitions are registered
//! XML Schemas, each read alone, one of each target namespace
//! (`InScopeSchema`). The effective schema of a validation is the one of
//! the namespace of the element validated, the operand or a document's
//! element child, with whatever it imports; of none, a schema of no
//! components, by which strict validation finds no top-level declaration
//! \[err:XQDY0084\] and lax validation assesses the element by `xs:anyType`.
//! The tree a validation makes counts among those the evaluation
//! constructs.

use super::super::analyze::InScopeSchema;
use super::super::error::{ErrorCode, error};
use super::super::plan::Op;
use super::{Evaluator, Focus, Frame, Seq};
use crate::error::{Result, SqlError};
use crate::xml::schema::{self, Failure, Mode, NOT_A_DOCUMENT, NOT_A_NODE, Schema, UNDECLARED};
use crate::xml::{Item, Kind, Node};

impl Evaluator<'_> {
    /// `validate` in `mode` of `operand` [XQuery §3.13]: its value one
    /// document or element node \[err:XQTY0030\], validated against its
    /// effective schema; a new node of the type annotations, values and
    /// defaults the validation gives, or the error XQuery names.
    pub(super) fn validate(&self, mode: Mode, operand: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let value = self.eval(operand, frame, focus)?;
        let [Item::Node(node)] = value.as_slice() else {
            return Err(error(ErrorCode::XQTY0030, "a validate expression's operand is one document or element node [XQuery §3.13]"));
        };
        let empty = Schema::default();
        let effective = self.effective(node);
        match schema::validate(effective.map_or(&empty, |s| &*s.schema), node, mode)? {
            Ok(valid) => {
                self.constructed(valid.tree())?;
                Ok(vec![Item::Node(valid)])
            }
            Err(failure) => Err(failed(&failure, effective)),
        }
    }

    /// The registered XML Schema of the in-scope schema definitions whose
    /// target namespace is the namespace of the element `node` validates:
    /// itself, or of a document node its first element child; none if
    /// there is no such element or schema.
    fn effective(&self, node: &Node) -> Option<&InScopeSchema> {
        let tree = node.tree();
        let element = match node.kind() {
            Some(Kind::Element) => node.id(),
            Some(Kind::Document) => tree.children(node.id()).find(|&c| tree.kind(c) == Some(Kind::Element))?,
            Some(Kind::Attribute | Kind::Text | Kind::Comment | Kind::ProcessingInstruction) | None => return None,
        };
        let namespace = tree.name(element).and_then(|n| n.uri.as_deref()).unwrap_or_default();
        self.plan.schemas.iter().find(|s| s.namespace == namespace)
    }
}

/// [XQuery §3.13] The XQuery error of `failure`, a validation against
/// `effective`, which the message names: err:XQDY0084 of an element no
/// top-level declaration matches in strict mode, err:XQDY0061 of a document
/// node of other children, err:XQTY0030 of an operand of another kind, and
/// err:XQDY0027 of a node not valid.
fn failed(failure: &Failure, effective: Option<&InScopeSchema>) -> SqlError {
    let code = match failure.code {
        UNDECLARED => ErrorCode::XQDY0084,
        NOT_A_DOCUMENT => ErrorCode::XQDY0061,
        NOT_A_NODE => ErrorCode::XQTY0030,
        _ => ErrorCode::XQDY0027,
    };
    let against = effective.map_or_else(
        || "no registered XML Schema of its namespace is in scope".to_owned(),
        |s| format!("against registered XML Schema {}", s.name),
    );
    error(code, &format!("{} {} ({against})", failure.rule, failure.why))
}
