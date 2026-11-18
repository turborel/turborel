//! Binding what validates an XML value against a registered XML Schema
//! [ISO/IEC 9075-14:2006 draft §6.20, §8.5]: XMLVALIDATE, IS VALID, and
//! CAST BY VALUE to a type of XMLSCHEMA, which is XMLVALIDATE [§6.5 GR1b
//! i 1]. Each is a call of its function (`functions/xml/validate.rs`),
//! holding the schemas it validates by, as the statement is bound.
//!
//! \[DESIGN\] Implementation-defined [§6.20 GR6a iii 2 A II 2, GR6b i 2,
//! §8.5 GR7a iii 3 A II 2, GR7b i 2], RULES.md D86: a validation of no
//! `<XML valid according to clause>` validates an element by the
//! registered XML Schema of its namespace, the zero-length one of an
//! element in none, that the current user may use, of the least name,
//! schema then identifier, the permanently registered ones last; chosen
//! as the statement is bound, so again of the same registered XML
//! Schemas and the same value it is the same.

use std::sync::Arc;

use super::super::predicates::negated;
use super::super::{Binder, Scope, rule};
use super::pinned::pinned_according;
use crate::ast::{Dcs, RowValue, ValueExpression, XmlValidAccording};
use crate::catalog::QualifiedName;
use crate::catalog::xml_schemas::{PERMANENT, may_use};
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{According, Candidate, ScalarRef, ValidBy, XmlValid, XmlValidate};
use crate::logical::{Cond, Expr};
use crate::value::DataType;
use crate::value::types::XmlSchemaNamed;
use crate::value::xml::{Typing, XmlType};

impl Binder<'_> {
    /// [§6.20] XMLVALIDATE ( `dcs` `operand` `according` ): of an XML value
    /// [§6.9 SR1], of the declared type SR3 gives.
    pub(super) fn xml_validate(
        &self,
        dcs: Dcs,
        operand: &ValueExpression,
        according: Option<&XmlValidAccording>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (e, t) = self.value_expression(operand, scope)?;
        let DataType::Xml(xvt) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §6.9 SR1] XMLVALIDATE takes an XML value, not a value of {t}")));
        };
        let (by, named) = self.valid_by(according)?;
        let declared = DataType::Xml(validated_type(xvt, named));
        let f = XmlValidate { dcs, by, declared: declared.clone() };
        Ok((Expr::Call(ScalarRef(Arc::new(f)), vec![e]), declared))
    }

    /// [§8.5] `value` IS \[NOT\] VALID `dcs` `according`: of one `<XML
    /// value expression>` \[SR1\]; IS NOT the negation of IS \[SR3\].
    pub(in crate::binder) fn xml_valid_predicate(
        &self,
        value: &RowValue,
        (dcs, according): (Dcs, Option<&XmlValidAccording>),
        not: bool,
        scope: Scope<'_>,
    ) -> Result<Cond> {
        let [xve] = value.as_slice() else {
            return Err(rule("[ISO/IEC 9075-14:2006 draft §8.5 SR1] the operand of IS VALID is one value, not a row".to_owned()));
        };
        let (e, t) = self.value_expression(xve, scope)?;
        if !matches!(t, DataType::Xml(_)) {
            return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §8.5 SR1] IS VALID tests an XML value, not a value of {t}")));
        }
        let f = XmlValid { dcs, by: self.valid_by(according)?.0 };
        Ok(negated(Cond::Value(Expr::Call(ScalarRef(Arc::new(f)), vec![e])), not))
    }

    /// [§6.5 GR1b i 1] CAST BY VALUE of `cast`, an XML value cast BY REF to
    /// `target`, a type of XMLSCHEMA: XMLVALIDATE ( DCS VE ACCORDING TO S
    /// ), DCS DOCUMENT of a DOCUMENT type, else CONTENT \[A\], S what its
    /// descriptor indicates \[B\].
    pub(in crate::binder) fn validated_cast(&self, cast: Expr, target: &DataType) -> Result<Expr> {
        let (DataType::Xml(xml), Some(named)) = (target, xml_named(target)) else {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] {target} is no type of XMLSCHEMA")));
        };
        let dcs = if matches!(xml, XmlType::Document(_)) { Dcs::Document } else { Dcs::Content };
        let name = QualifiedName { auth: named.schema.clone(), ident: named.name.clone() };
        let desc = self
            .db
            .catalog
            .xml_schema(&name)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-14:2006 draft §11.6 SR1] there is no registered XML Schema {name}")))?;
        let schema =
            desc.schema.clone().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] registered XML Schema {name} is not read")))?;
        let according = According { name: name.to_string(), schema, namespace: named.namespace.clone(), element: named.element.clone() };
        let f = XmlValidate { dcs, by: ValidBy::According(according), declared: target.clone() };
        Ok(Expr::Call(ScalarRef(Arc::new(f)), vec![cast]))
    }

    /// [§6.20 SR2, GR6] [§8.5 SR4, GR7] How each element finds its
    /// registered XML Schema: the one `according` indicates, as the binder's
    /// cell may use it [§11.6 AR1]; else, data-driven, of each namespace the
    /// one chosen (this module's choice); and what a type's descriptor
    /// includes of the one indicated, if one is.
    fn valid_by(&self, according: Option<&XmlValidAccording>) -> Result<(ValidBy, Option<XmlSchemaNamed>)> {
        let Some(according) = according else { return Ok((ValidBy::Namespaces(self.candidates()), None)) };
        // [ISO/IEC 9075-14:2006 draft §4.2.6] Of a definition bound again,
        // the one it indicated as it was created; noted, for one created.
        let pinned = pinned_according(according, self.horizon.pins(), &*self.db.catalog);
        let indicated = self.db.indicated(&pinned, (self.schema, self.who())).or_at(according.at)?;
        if let Some(noted) = self.invoked {
            noted.indicated(&indicated.name);
        }
        let named = indicated.named();
        let by = According {
            name: indicated.name.to_string(),
            schema: indicated.schema,
            namespace: indicated.namespace,
            element: indicated.element,
        };
        Ok((ValidBy::According(by), Some(named)))
    }

    /// [§6.20 GR6a iii 2 A II 2] Of each target namespace, the registered
    /// XML Schema the binder's cell may use of the least name, the
    /// permanently registered ones after the others. The in-scope schema
    /// definitions of XQuery are chosen alike (`binder/xml/query.rs`).
    pub(super) fn candidates(&self) -> Vec<Candidate> {
        let catalog = &*self.db.catalog;
        let mut usable: Vec<_> = catalog.xml_schemas().into_iter().filter(|d| may_use(catalog, &d.name, self.who()).is_ok()).collect();
        usable.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out: Vec<Candidate> = Vec::new();
        for d in usable {
            if !out.iter().any(|c| c.namespace == d.target_namespace) {
                out.push(Candidate { namespace: d.target_namespace.clone(), name: d.name.to_string(), schema: d.schema.clone() });
            }
        }
        for p in &PERMANENT {
            if !out.iter().any(|c| c.namespace == p.target_namespace) {
                out.push(Candidate { namespace: p.target_namespace.to_owned(), name: p.qualified().to_string(), schema: None });
            }
        }
        out
    }
}

/// What the type `t`'s descriptor includes of a registered XML Schema, of
/// a type of XMLSCHEMA.
fn xml_named(t: &DataType) -> Option<&XmlSchemaNamed> {
    if let DataType::Xml(x) = t { x.schema() } else { None }
}

/// [§6.20 SR3] The declared type of XMLVALIDATE of a value of `xvt`: of a
/// DOCUMENT type, XML(DOCUMENT(XMLSCHEMA)) of `named`, what an `<XML valid
/// according to clause>` indicates, else XML(DOCUMENT(ANY)) \[SR3a\]; of a
/// CONTENT type, so of CONTENT \[SR3b\]; of XML(SEQUENCE), XML(SEQUENCE)
/// \[SR3c\].
fn validated_type(xvt: &XmlType, named: Option<XmlSchemaNamed>) -> XmlType {
    let typing = named.map_or(Typing::Any, |n| Typing::Schema(Arc::new(n)));
    match xvt {
        XmlType::Document(_) => XmlType::Document(typing),
        XmlType::Content(_) => XmlType::Content(typing),
        XmlType::Sequence => XmlType::Sequence,
    }
}
