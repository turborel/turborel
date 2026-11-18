//! Binding SQL/XML's publishing operators [ISO/IEC 9075-14:2006 draft
//! §6]: XMLCOMMENT [§6.11], XMLCONCAT [§6.12], XMLPI [§6.17] and XMLTEXT
//! [§6.19], each a call of its function (`functions/xml/publish.rs`), and
//! RETURNING CONTENT a call of XMLDOCUMENT's of that.

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use super::{DOCUMENT_CONTENT, xml_cast};
use crate::ast::{Passing, Returning, ValueExpression};
use crate::error::Result;
use crate::functions::{ScalarRef, XmlComment, XmlConcat, XmlDocument, XmlPi, XmlText};
use crate::logical::Expr;
use crate::value::DataType;
use crate::value::xml::{Typing, XmlType};
use crate::xml::{Version, is_ncname};

/// [§6.11 SR1, §6.12 SR1, §6.17 SR4, §6.19 SR1] implementation-defined:
/// the `<XML returning clause>` of a publishing operator that writes
/// none, RETURNING CONTENT, as XMLDOCUMENT's is, so its value is a
/// document node an XML(CONTENT) or XML(DOCUMENT) column takes.
pub const IMPLICIT_PUBLISHING: Returning = Returning::Content;

/// `call`, of a sequence, as `returning` says \[§6.11 SR3, §6.17 SR7,
/// §6.19 SR3\]: of XML(SEQUENCE); or, RETURNING CONTENT, XMLDOCUMENT of
/// it RETURNING CONTENT, of `DOCUMENT_CONTENT` [§6.13 SR3a].
pub(super) fn returned(call: Expr, returning: Option<Returning>) -> (Expr, DataType) {
    match returning.unwrap_or(IMPLICIT_PUBLISHING) {
        Returning::Sequence => (call, DataType::Xml(XmlType::Sequence)),
        Returning::Content => {
            let document = DataType::Xml(DOCUMENT_CONTENT);
            (Expr::Call(ScalarRef(Arc::new(XmlDocument(document.clone()))), vec![call]), document)
        }
    }
}

/// [§6.12 SR2a ii 2] The declared type of XMLCONCAT RETURNING CONTENT of
/// values of `types`, two or more: XML(CONTENT(UNTYPED)) if each is
/// XML(DOCUMENT(UNTYPED)) or XML(CONTENT(UNTYPED)) \[A\], else
/// XML(CONTENT(ANY)) \[E\], as no type of XMLSCHEMA is one turborel has
/// \[B to D\]. The grammar takes two values or more, so a type of one
/// \[1\] never is.
fn concatenation_type(types: &[XmlType]) -> XmlType {
    let untyped = |t: &XmlType| matches!(t, XmlType::Document(Typing::Untyped) | XmlType::Content(Typing::Untyped));
    if types.iter().all(untyped) { XmlType::Content(Typing::Untyped) } else { XmlType::Content(Typing::Any) }
}

impl Binder<'_> {
    /// A `<character value expression>` of `what`, `clause` the rule to
    /// cite.
    fn xml_characters(&self, operand: &ValueExpression, scope: Scope<'_>, what: &str, clause: &str) -> Result<Expr> {
        let (expr, t) = self.value_expression(operand, scope)?;
        if !t.is_character() {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] {what} takes a character string, not a value of {t}")));
        }
        Ok(expr)
    }

    /// [§6.11] XMLCOMMENT of `operand`, a character string.
    pub(super) fn xml_comment(
        &self,
        operand: &ValueExpression,
        returning: Option<Returning>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let e = self.xml_characters(operand, scope, "XMLCOMMENT", "§6.11")?;
        Ok(returned(Expr::Call(ScalarRef(Arc::new(XmlComment)), vec![e]), returning))
    }

    /// [§6.19] XMLTEXT of `operand`, a character string, of the `<XML
    /// binary encoding>` in scope \[GR3, GR4\].
    pub(super) fn xml_text(&self, operand: &ValueExpression, returning: Option<Returning>, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let e = self.xml_characters(operand, scope, "XMLTEXT", "§6.19")?;
        Ok(returned(Expr::Call(ScalarRef(Arc::new(XmlText(self.binary_encoding()))), vec![e]), returning))
    }

    /// [§6.17] XMLPI of `target`, an XML 1.1 NCName \[SR1, SR2\] but `xml`
    /// in any case \[SR3\], and `operand`, a character string, if
    /// written.
    pub(super) fn xml_pi(
        &self,
        target: &str,
        operand: Option<&ValueExpression>,
        returning: Option<Returning>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        if !is_ncname(Version::V11, target) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.17 SR2] the target {target} is not an XML 1.1 NCName")));
        }
        if target.eq_ignore_ascii_case("xml") {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.17 SR3] the target of a processing instruction is not {target}")));
        }
        let args = operand.map(|o| self.xml_characters(o, scope, "XMLPI", "§6.17")).transpose()?.into_iter().collect();
        Ok(returned(Expr::Call(ScalarRef(Arc::new(XmlPi(target.into()))), args), returning))
    }

    /// [§6.12] XMLCONCAT of `operands`, XML values: RETURNING SEQUENCE,
    /// of XML(SEQUENCE) \[SR2b\]; RETURNING CONTENT, XMLDOCUMENT of that,
    /// cast BY REF to the type the operands' make \[SR2a, SR4\].
    pub(super) fn xml_concatenation(
        &self,
        operands: &[ValueExpression],
        returning: Option<Returning>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let mut args = Vec::with_capacity(operands.len());
        let mut types = Vec::with_capacity(operands.len());
        for operand in operands {
            let (e, t) = self.value_expression(operand, scope)?;
            let DataType::Xml(x) = t else {
                return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.9 SR1] XMLCONCAT takes XML values, not a value of {t}")));
            };
            args.push(e);
            types.push(x);
        }
        let call = Expr::Call(ScalarRef(Arc::new(XmlConcat)), args);
        let (e, t) = returned(call, returning);
        let target = DataType::Xml(concatenation_type(&types));
        if returning.unwrap_or(IMPLICIT_PUBLISHING) == Returning::Sequence || t == target {
            return Ok((e, t));
        }
        xml_cast((e, t), target, Passing::ByRef)
    }
}
