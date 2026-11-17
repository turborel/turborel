//! Binding the SQL/XML operators [ISO/IEC 9075-14:2006 draft §6]:
//! XMLCAST [§6.6] (`binder/xml/cast.rs`), XMLDOCUMENT [§6.13], XMLPARSE
//! [§6.16], XMLSERIALIZE [§6.8] and the publishing operators
//! (`publish`, `element`), and the document and content predicates [§8.2,
//! §8.3], each a call of its function (`functions/xml.rs`).

mod aggregate;
mod cast;
mod element;
mod namespaces;
mod publish;

use std::sync::Arc;

use super::predicates::negated;
use super::{Binder, Scope, rule};
use crate::ast::{Passing, Returning, RowValue, ValueExpression, XmlOperator};
use crate::error::Result;
use crate::functions::{ScalarRef, Serialization, XmlCopy, XmlDocument, XmlIs, XmlParse, XmlSerialize};
use crate::logical::{Cond, Expr};
use crate::value::DataType;
use crate::value::xml::{Typing, XmlType};
use crate::xml::{Syntax, VERSION_10, VERSION_11, Version, Whitespace};

/// [§6.8 SR2d, SR4g] implementation-defined: the version a serialization
/// writes if it names none, XML 1.0, which every processor reads.
pub const IMPLICIT_VERSION: Version = Version::V10;

/// [§6.8 SR4d, SR4e] implementation-defined: the `<XML encoding name>`s of
/// a binary serialization, UTF8 and UTF16, and the one it writes if it
/// names none, UTF8.
pub const ENCODINGS: [&str; 2] = ["UTF8", "UTF16"];

/// [§6.13 SR1] implementation-defined: the `<XML returning clause>` of an
/// XMLDOCUMENT that writes none, RETURNING CONTENT, as its value is one
/// document node.
pub const IMPLICIT_RETURNING: Returning = Returning::Content;

/// [§6.13 SR3a] implementation-defined: the declared type of XMLDOCUMENT
/// RETURNING CONTENT, XML(CONTENT(UNTYPED)), as no node turborel holds has
/// a type of an XML Schema, and §10.19 makes its value so \[GR9a\].
pub const DOCUMENT_CONTENT: XmlType = XmlType::Content(Typing::Untyped);

impl Binder<'_> {
    /// An SQL/XML operator: bound, and type-checked.
    pub(super) fn xml_operator(&self, x: &XmlOperator, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match x {
            XmlOperator::Cast { operand, target, passing } => self.xml_cast_specification(operand.as_ref(), target, *passing, scope),
            XmlOperator::Document { operand, returning } => {
                let operand = self.xml_value_expression(operand, scope, "XMLDOCUMENT")?;
                // [§6.13 SR3] CONTENT's type, or XML(SEQUENCE).
                let result = match returning.unwrap_or(IMPLICIT_RETURNING) {
                    Returning::Content => DataType::Xml(DOCUMENT_CONTENT),
                    Returning::Sequence => DataType::Xml(XmlType::Sequence),
                };
                Ok((Expr::Call(ScalarRef(Arc::new(XmlDocument(result.clone()))), vec![operand]), result))
            }
            XmlOperator::Comment { operand, returning } => self.xml_comment(operand, *returning, scope),
            XmlOperator::Concat { operands, returning } => self.xml_concatenation(operands, *returning, scope),
            XmlOperator::Pi { target, operand, returning } => self.xml_pi(target, operand.as_ref(), *returning, scope),
            XmlOperator::Text { operand, returning } => self.xml_text(operand, *returning, scope),
            XmlOperator::Element(element) => self.xml_element(element, scope),
            XmlOperator::Forest(forest) => self.xml_forest(forest, scope),
            XmlOperator::Parse { syntax, operand, whitespace } => self.xml_parse(*syntax, operand, *whitespace, scope),
            XmlOperator::Serialize { syntax, operand, target, encoding, version, declaration } => {
                let operand = self.xml_value_expression(operand, scope, "XMLSERIALIZE")?;
                let serialization = serialization(*syntax, target, encoding.as_deref(), version.as_deref(), *declaration)?;
                let call = Expr::Call(ScalarRef(Arc::new(XmlSerialize(serialization))), vec![operand]);
                Ok((call, target.clone()))
            }
        }
    }

    /// [§6.16] XMLPARSE: a character or binary string parsed, its declared
    /// type \[SR1\] implementation-defined: XML(DOCUMENT(UNTYPED)) or
    /// XML(CONTENT(UNTYPED)), as every node a parse makes is untyped
    /// [§10.16 GR9] [XQueryDM §6.2.3].
    fn xml_parse(&self, syntax: Syntax, operand: &ValueExpression, whitespace: Whitespace, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (expr, t) = self.value_expression(operand, scope)?;
        if !t.is_character() && !t.is_binary() {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.16] XMLPARSE parses a character or binary string, not a value of {t}"
            )));
        }
        let result = match syntax {
            Syntax::Document => XmlType::Document(Typing::Untyped),
            Syntax::Content => XmlType::Content(Typing::Untyped),
        };
        let call = Expr::Call(ScalarRef(Arc::new(XmlParse { syntax, whitespace, binary: t.is_binary() })), vec![expr]);
        Ok((call, DataType::Xml(result)))
    }

    /// [§6.5 SR1] CAST of `operand` to `target`, `passing` written: of a
    /// value expression, not NULL, of an XML type, to one.
    pub(super) fn xml_cast_passing(
        &self,
        operand: Option<&ValueExpression>,
        target: DataType,
        passing: Passing,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let Some(v) = operand else {
            return Err(rule("[ISO/IEC 9075-14:2006 draft §6.5 SR1] CAST of NULL has no <XML passing mechanism>"));
        };
        let bound = self.value_expression(v, scope)?;
        if !matches!(bound.1, DataType::Xml(_)) || !matches!(target, DataType::Xml(_)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.5 SR1] CAST of {} to {target} has no <XML passing mechanism>",
                bound.1
            )));
        }
        xml_cast(bound, target, passing)
    }

    /// [§8.2, §8.3] `value` IS \[NOT\] DOCUMENT or CONTENT, as `syntax`
    /// says: of one `<XML value expression>` \[SR1\]; IS NOT the negation
    /// of IS \[SR2\].
    pub(super) fn xml_predicate(&self, value: &RowValue, syntax: Syntax, not: bool, scope: Scope<'_>) -> Result<Cond> {
        let (what, clause) = match syntax {
            Syntax::Document => ("IS DOCUMENT", "§8.3 SR1"),
            Syntax::Content => ("IS CONTENT", "§8.2 SR1"),
        };
        let [xve] = value.as_slice() else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] the operand of {what} is one value, not a row")));
        };
        let (e, t) = self.value_expression(xve, scope)?;
        if !matches!(t, DataType::Xml(_)) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft {clause}] {what} tests an XML value, not a value of {t}")));
        }
        Ok(negated(Cond::Value(Expr::Call(ScalarRef(Arc::new(XmlIs(syntax))), vec![e])), not))
    }

    /// [§6.9] An `<XML value expression>` of `what`: of an XML type \[SR1\].
    fn xml_value_expression(&self, operand: &ValueExpression, scope: Scope<'_>, what: &str) -> Result<Expr> {
        let (expr, t) = self.value_expression(operand, scope)?;
        if !matches!(t, DataType::Xml(_)) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.9 SR1] {what} takes an XML value, not a value of {t}")));
        }
        Ok(expr)
    }
}

/// [§6.5] CAST of `e`, of type `t`, to `target`, one of them an XML type,
/// passed as `passing` says: of an XML type to an XML type alone \[SR3\];
/// BY REF, to an UNTYPED type of an UNTYPED one \[SR4a\], its nodes
/// themselves; BY VALUE, a copy of them [§10.18] \[GR1b i\].
///
/// # Errors
///
/// SQLCODE -103 if it is not valid.
pub(super) fn xml_cast((e, t): (Expr, DataType), target: DataType, passing: Passing) -> Result<(Expr, DataType)> {
    let (DataType::Xml(source), DataType::Xml(to)) = (&t, &target) else {
        return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.5 SR3] CAST of {t} to {target} is not valid")));
    };
    if passing == Passing::ByRef && !to.takes(*source) {
        return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.5 SR4a] CAST BY REF of {t} to {target} is not valid")));
    }
    let cast = Expr::Cast(Box::new(e), target.clone());
    let passed = match passing {
        Passing::ByRef => cast,
        Passing::ByValue => Expr::Call(ScalarRef(Arc::new(XmlCopy)), vec![cast]),
    };
    Ok((passed, target))
}

/// [§10.3 SR1] The result type of values of `types`, one an XML type:
/// all XML types \[SR1a\]; XML(SEQUENCE) if one is \[SR1b i\]; the one
/// type, if one \[SR1b ii 2 A\]; XML(DOCUMENT(UNTYPED)) if all are
/// \[SR1b ii 2 B I\]; XML(CONTENT(UNTYPED)) if all are UNTYPED \[SR1b ii
/// 2 B II\]; else XML(CONTENT(ANY)) \[SR1b ii 2 B IX\], no type of
/// XMLSCHEMA being one turborel has. `clause` is the rule to cite.
///
/// # Errors
///
/// SQLCODE -103 if one is not an XML type.
pub(super) fn xml_set_type(types: &[DataType], clause: &str) -> Result<DataType> {
    let mut xml = Vec::with_capacity(types.len());
    for t in types {
        let DataType::Xml(x) = t else {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §10.3 SR1a] {clause}: XML values are brought together with values of {t}"
            )));
        };
        xml.push(*x);
    }
    let untyped = |x: &XmlType| matches!(x, XmlType::Document(Typing::Untyped) | XmlType::Content(Typing::Untyped));
    let combined = if xml.is_empty() || xml.contains(&XmlType::Sequence) {
        XmlType::Sequence
    } else if let [only] = xml.as_slice() {
        *only
    } else if xml.iter().all(|x| *x == XmlType::Document(Typing::Untyped)) {
        XmlType::Document(Typing::Untyped)
    } else if xml.iter().all(untyped) {
        XmlType::Content(Typing::Untyped)
    } else {
        XmlType::Content(Typing::Any)
    };
    Ok(DataType::Xml(combined))
}

/// [§6.8 SR2, SR4] What XMLSERIALIZE writes: as `syntax` says, to
/// `target`, a character or a binary string type, in `encoding` if a
/// binary one, of `version`, '1.0' or '1.1', with a declaration as
/// `declaration` says.
fn serialization(
    syntax: Syntax,
    target: &DataType,
    encoding: Option<&str>,
    version: Option<&str>,
    declaration: Option<bool>,
) -> Result<Serialization> {
    let version = match version {
        None => IMPLICIT_VERSION,
        Some(VERSION_10) => Version::V10,
        Some(VERSION_11) => Version::V11,
        Some(other) => {
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.8 SR2c, SR4f] the version serialized is '1.0' or '1.1', not '{other}'"
            )));
        }
    };
    let binary = target.is_binary();
    if !binary && !target.is_character() {
        return Err(rule(format!(
            "[ISO/IEC 9075-14:2006 draft §6.8 SR2b, SR4b] XMLSERIALIZE makes a character or binary string, not a value of {target}"
        )));
    }
    if !binary && encoding.is_some() {
        return Err(rule("[ISO/IEC 9075-14:2006 draft §6.8] ENCODING names the encoding of a binary string alone"));
    }
    let utf16 = match encoding.map(str::to_ascii_uppercase).as_deref() {
        None | Some("UTF8") => false,
        Some("UTF16") => true,
        Some(other) => {
            let names = ENCODINGS.join(" and ");
            return Err(rule(format!(
                "[ISO/IEC 9075-14:2006 draft §6.8 SR4e] implementation-defined: the encodings are {names}, not {other}"
            )));
        }
    };
    Ok(Serialization { syntax, target: target.clone(), binary, utf16, version, declaration })
}
