//! \[EXTENSION\] The `<XML Schema document>` of CREATE XML SCHEMA [ISO/IEC
//! 9075-14:2006 draft §4.2.6] (RULES.md D86): a `<value expression>` of a
//! character string, a binary string or an XML type, evaluated once, as a
//! row of no table is, so its subqueries run and its parameters are read.
//!
//! The statement is then made, and logged, as its text with the document
//! written in as a literal: replay, and recovery, read that text alone,
//! with nothing evaluated again. A character string is the document as
//! written; a binary string is parsed as XMLPARSE DOCUMENT parses one
//! [§10.16], and an XML value must be a document [§10.15 GR3], each then
//! written as its serialization.
//!
//! \[DESIGN\] RULES.md D86: every document is written as a binary string
//! literal of its text's UTF-8 octets, which the definition reads back as
//! the same text, octet for octet (`definitions/xml_schemas.rs`). A
//! character string literal cannot hold every document, as a `<newline>`
//! is no `<character>` of one [X3.135-1986 §5.1 SR1]; one form for all is
//! the simplest that holds each. The octets are the text's, not a binary
//! string document's, so they are not parsed as one is, by the encoding
//! the document declares: a character string keeps the declaration it was
//! written with, which may name another encoding than UTF-8.

use super::super::super::compiled::plan;
use super::super::super::{Completion, Session, State, StatementContext};
use crate::ast::{Literal, SchemaStatement, TableName, ValueExpression, ValueSpecification, XmlSchemaDefinition};
use crate::binder::{self, qualify};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::execute;
use crate::functions::{ScalarFunction, XmlParse, xml_text};
use crate::logical::LogicalPlan;
use crate::params::{ParameterType, Params};
use crate::value::lob::{LobKind, MAX_LOB_LENGTH};
use crate::value::{Binary, CharUnits, DataType, Value};
use crate::xml::{Parameters, Syntax, Version, Whitespace, serialize};

/// [Serialization §5.1.2] The encoding a serialized document declares:
/// it is kept as a character string.
const UTF8: &str = "UTF-8";

/// \[EXTENSION\] [ISO/IEC 9075:1992 §17.6 GR4] The type a `?` written
/// alone as the document takes: CHARACTER LARGE OBJECT of the greatest
/// length, as a document is text of any length.
const DOCUMENT_PARAMETER: DataType = DataType::Lob { kind: LobKind::Character, length: MAX_LOB_LENGTH, units: CharUnits::Characters };

impl Session {
    /// Schema statement `statement`, written `text`, in `cx`, of
    /// parameters `params`: CREATE XML SCHEMA as `xml_schema_statement`
    /// makes it, any other as it is written.
    ///
    /// # Errors
    ///
    /// As `xml_schema_statement`, or the statement's own.
    pub(in crate::db) fn schema_statement_in(
        &self,
        cx: StatementContext<'_>,
        (statement, text): (&SchemaStatement, &str),
        params: &Params,
    ) -> Result<Completion> {
        if let SchemaStatement::XmlSchema(x) = statement {
            return self.xml_schema_statement(cx, x, params);
        }
        self.schema_statement(cx.st, cx.tx, statement, text)
    }

    /// \[EXTENSION\] CREATE XML SCHEMA `x` in `cx`, of parameters
    /// `params`: its document evaluated, then the statement, the document
    /// written in, made and logged as its text.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the document is of a type other than a character
    /// string, a binary string or XML; SQLSTATE 2200L if it is the null
    /// value or an XML value that is no document; 2200M if a binary
    /// string is no document; the evaluation's own; as the definition's.
    fn xml_schema_statement(&self, cx: StatementContext<'_>, x: &XmlSchemaDefinition, params: &Params) -> Result<Completion> {
        let at = x.name.at;
        let value = self.document_value(cx, &x.document, params)?;
        let document = document_text(&value)?;
        let qn = qualify(&x.name, &self.default_schema());
        let location = x.location.as_deref().map(|l| format!(" LOCATION {}", literal(l))).unwrap_or_default();
        let (written, logged) = written(document);
        let text = format!("CREATE XML SCHEMA {}.{}{location} AS {logged}", quoted(&qn.auth), quoted(&qn.ident));
        let name = TableName { auth: Some(qn.auth), ident: qn.ident, at };
        let written = ValueExpression::Value(ValueSpecification::Literal(written));
        let definition = XmlSchemaDefinition { name, location: x.location.clone(), document: written };
        self.schema_statement(cx.st, cx.tx, &SchemaStatement::XmlSchema(Box::new(definition)), &text)
    }

    /// The document `document` bound in `st`, of parameters `params`, as a
    /// statement prepared is, so each `?` takes its type: one written alone
    /// `DOCUMENT_PARAMETER`.
    ///
    /// # Errors
    ///
    /// As its binding.
    pub(in crate::db) fn document_only(&self, st: &State, document: &ValueExpression, params: &Params) -> Result<()> {
        if let ValueExpression::Value(v @ ValueSpecification::Dynamic { .. }) = document {
            return self.value_in(st, v, params, Some(ParameterType::Of(DOCUMENT_PARAMETER))).map(drop);
        }
        self.bound(st, params, |b| binder::bind_value(b, document)).map(drop)
    }

    /// The value of `document`, in `cx`, of parameters `params`: of a `?`
    /// written alone, its value as `DOCUMENT_PARAMETER`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is of a type other than a character string, a
    /// binary string or XML; as its binding and evaluation.
    fn document_value(&self, cx: StatementContext<'_>, document: &ValueExpression, params: &Params) -> Result<Value> {
        if let ValueExpression::Value(v @ ValueSpecification::Dynamic { .. }) = document {
            return Ok(self.value_in(cx.st, v, params, Some(ParameterType::Of(DOCUMENT_PARAMETER)))?.0);
        }
        let (e, t) = self.bound(cx.st, params, |b| binder::bind_value(b, document))?;
        if !(t.is_character() || t.is_binary() || matches!(t, DataType::Xml(_))) {
            let msg = format!("[EXTENSION] an XML Schema document is a character string, a binary string or an XML value, not {t}");
            return Err(SqlError::new(Code::SyntaxRule, msg));
        }
        let (optimized, _, physical) = plan(cx.st, LogicalPlan::Values { rows: vec![vec![e]], types: vec![t] });
        for table in optimized.tables() {
            cx.st.read_lock(cx.tx, &table)?;
        }
        let row = execute(physical, cx.st, cx.tx.id).next().transpose()?.unwrap_or_default();
        Ok(row.into_iter().next().unwrap_or(Value::Null))
    }
}

/// The text of the schema document `value`: a character string as it is,
/// a large object too; a binary string parsed as a document and an XML
/// document, serialized as XML 1.1, which writes every document either
/// version reads, with its declaration.
///
/// # Errors
///
/// SQLSTATE 2200L if it is the null value, or an XML value that is no
/// document; 2200M if a binary string is no document; SQLCODE -900 if a
/// character string is longer than XMLPARSE reads.
fn document_text(value: &Value) -> Result<String> {
    let not_a_document = |what: &str| {
        let msg = format!("[EXTENSION] [ISO/IEC 9075-14:2006 draft §10.15 GR3] an XML Schema document is an XML document, not {what}");
        SqlError::new(Code::Data, msg).with_state(SqlState::NOT_AN_XML_DOCUMENT)
    };
    if value.is_null() {
        return Err(not_a_document("the null value"));
    }
    if matches!(value, Value::Char(_)) || is_text(value) {
        return xml_text(value)?.ok_or_else(|| not_a_document("the null value"));
    }
    let parse = XmlParse { syntax: Syntax::Document, whitespace: Whitespace::Preserve, binary: true };
    let parsed = if let Value::Xml(_) = value { value.clone() } else { parse.call(std::slice::from_ref(value))? };
    let Value::Xml(x) = parsed else { return Err(not_a_document("a value of no XML type")) };
    let sequence = x.sequence()?;
    if !sequence.is_document() {
        return Err(not_a_document("an XML value of no single document node"));
    }
    serialize(&sequence, &Parameters { version: Version::V11, declaration: true, encoding: UTF8, undeclare: true })
}

/// Whether `v` is a character large object, a character string.
fn is_text(v: &Value) -> bool {
    let Value::Lob(l) = v else { return false };
    l.kind() == LobKind::Character
}

/// The document `text` as a literal, and as written in a statement: a
/// binary string literal of its UTF-8 octets, whatever it holds.
fn written(text: String) -> (Literal, String) {
    let octets = Binary::new(text.into_bytes());
    let logged = Value::Binary(octets.clone()).to_literal();
    (Literal::Binary(octets), logged)
}

/// `s` as a character string literal, its quotes doubled.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The name `name` written delimited, its quotes doubled.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
