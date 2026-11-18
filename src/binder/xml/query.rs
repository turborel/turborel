//! Binding the SQL/XML operators that evaluate XQuery [ISO/IEC
//! 9075-14:2006 draft §6.18, §8.4, §7.1]: their arguments bound and
//! passed [§6.18 SR5, GR1], their query analysed in the static context of
//! §10.20 as the statement is bound, each a call of its function
//! (`functions/xml/query.rs`).

use std::sync::Arc;

use super::super::{Binder, Scope, rule};
use super::DOCUMENT_CONTENT;
use crate::ast::{
    DefaultOption, OnEmpty, Passing, Returning, ValueExpression, ValueSpecification, XmlArguments, XmlQuery, XmlTable, XmlTableColumn,
};
use crate::error::Result;
use crate::functions::{Evaluation, RegularColumn, ScalarRef, TableColumn, XmlCopy, XmlDocument, XmlExistsOf, XmlQueryOf, XmlTableOf};
use crate::logical::Expr;
use crate::value::datetime::{MICROS_PER_DAY, MICROS_PER_MINUTE};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY};
use crate::value::xml::XmlType;
use crate::value::{DataType, Value};
use crate::xml::{Moment, Shape, Version, is_ncname};
use crate::xquery::{Environment, Query};

/// [§6.18 SR3] implementation-defined: the `<XML returning clause>` of an
/// XMLQUERY that writes none, RETURNING CONTENT, as XMLDOCUMENT's.
pub const IMPLICIT_QUERY_RETURNING: Returning = Returning::Content;

/// The arguments of an XMLQUERY, XMLEXISTS or XMLTABLE, bound: the
/// context item's value first, if passed, then each variable's; and the
/// variables' names.
struct Bound {
    /// The values passed.
    values: Vec<Expr>,
    /// Whether the first is the context item.
    context: bool,
    /// The variables' names, in order.
    names: Vec<String>,
    /// Whether the context item is a value of one node: of XML(DOCUMENT)
    /// or XML(CONTENT) [§4.2.2].
    one_node: bool,
}

impl Binder<'_> {
    /// [§6.18 SR5] The `<XML query argument>`s of `arguments`, if written:
    /// at most one context item [SR5b i]; each variable's name an XML 1.1
    /// NCName, none twice [SR5a i, ii]; each value of a type XMLCAST casts
    /// to XML(SEQUENCE) [SR5a iv, SR5b iii], a passing mechanism of an XML
    /// value alone [SR5a v, SR5b iv], an XML value passed BY VALUE a copy
    /// [GR1a ii 2 A I, GR1b i 2], another cast [GR1a ii 2 A II, GR1b i 3].
    fn xml_arguments(&self, arguments: Option<&XmlArguments>, scope: Scope<'_>) -> Result<Bound> {
        let Some(arguments) = arguments else {
            return Ok(Bound { values: Vec::new(), context: false, names: Vec::new(), one_node: false });
        };
        let (mut context, mut variables, mut names) = (None, Vec::new(), Vec::<String>::new());
        let mut one_node = false;
        for argument in &arguments.arguments {
            let (e, t) = self.value_expression(&argument.value, scope)?;
            let value = if matches!(t, DataType::Xml(_)) {
                let passing = argument.passing.unwrap_or(arguments.default);
                if passing == Passing::ByValue { Expr::Call(ScalarRef(Arc::new(XmlCopy)), vec![e]) } else { e }
            } else if argument.passing.is_some() {
                return Err(rule(format!(
                    "[ISO/IEC 9075-14:2006 draft §6.18 SR5a v] a value of {t} is passed with no <XML passing mechanism>"
                ))
                .or_at(argument.at));
            } else {
                self.cast_to_xml(e, &t, DataType::Xml(XmlType::Sequence)).map_err(|err| err.or_at(argument.at))?.0
            };
            match &argument.name {
                None => {
                    if context.replace(value).is_some() {
                        return Err(rule("[ISO/IEC 9075-14:2006 draft §6.18 SR5b i] one context item is passed at most").or_at(argument.at));
                    }
                    one_node = matches!(t, DataType::Xml(XmlType::Document(_) | XmlType::Content(_)));
                }
                Some(name) => {
                    if !is_ncname(Version::V11, name) || names.contains(name) {
                        let why = format!(
                            "[ISO/IEC 9075-14:2006 draft §6.18 SR5a i, ii] the variable {name} is no XML 1.1 NCName, or is passed twice"
                        );
                        return Err(rule(why).or_at(argument.at));
                    }
                    names.push(name.clone());
                    variables.push(value);
                }
            }
        }
        let has_context = context.is_some();
        Ok(Bound { values: context.into_iter().chain(variables).collect(), context: has_context, names, one_node })
    }

    /// The query `text` analysed in the static context §10.20 makes \[SR2\]:
    /// the namespaces declared in scope, the variables `names` passed, and
    /// the database's functions and collations.
    fn xquery(&self, text: &str, names: &[String]) -> Result<Arc<Query>> {
        let (namespaces, default_element) = self.namespaces_in_scope();
        let env = Environment {
            namespaces,
            default_element,
            variables: names.to_vec(),
            library: &self.db.xquery_functions,
            collations: &self.db.collations,
        };
        Query::compile(text, &env).map(Arc::new)
    }

    /// What evaluates `text` of the arguments `bound`, at the statement's
    /// instant and in its time zone [XQuery Appendix C.2].
    fn evaluation(&self, text: &str, bound: &Bound) -> Result<Evaluation> {
        let query = self.xquery(text, &bound.names)?;
        let now = self.now.instant();
        let local = now.utc.saturating_add(i64::from(now.zone).saturating_mul(MICROS_PER_MINUTE));
        let moment = Moment {
            shape: Shape::DateTime,
            days: local.div_euclid(MICROS_PER_DAY),
            micros: local.rem_euclid(MICROS_PER_DAY),
            zone: Some(now.zone),
        };
        Ok(Evaluation { query, context: bound.context, one_node: bound.one_node, now: moment, timezone: now.zone })
    }

    /// [§6.18] XMLQUERY: RETURNING CONTENT, implicit too, of no returning
    /// mechanism \[SR4a\], XMLDOCUMENT of its value of RETURNING SEQUENCE BY
    /// VALUE \[SR10a\]; RETURNING SEQUENCE of its mechanism, or its default
    /// passing mechanism, which it then has [SR4b, SR4c], of XML(SEQUENCE)
    /// \[SR10b\].
    pub(super) fn xml_query(&self, q: &XmlQuery, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let returning = q.returning.unwrap_or(IMPLICIT_QUERY_RETURNING);
        let mechanism = match (returning, q.mechanism, &q.arguments) {
            (Returning::Content, Some(_), _) => {
                return Err(rule("[ISO/IEC 9075-14:2006 draft §6.18 SR4a] RETURNING CONTENT has no <XML query returning mechanism>"));
            }
            (Returning::Content, None, _) => Passing::ByValue,
            (Returning::Sequence, Some(m), _) => m,
            (Returning::Sequence, None, Some(arguments)) => arguments.default,
            (Returning::Sequence, None, None) => {
                return Err(rule(
                    "[ISO/IEC 9075-14:2006 draft §6.18 SR4c] RETURNING SEQUENCE has an <XML query returning mechanism> or an <XML query argument list>",
                ));
            }
        };
        let bound = self.xml_arguments(q.arguments.as_ref(), scope)?;
        let evaluation = self.evaluation(&q.text, &bound)?;
        let f = XmlQueryOf { evaluation, by_value: mechanism == Passing::ByValue, null_on_empty: q.empty == Some(OnEmpty::Null) };
        let call = Expr::Call(ScalarRef(Arc::new(f)), bound.values);
        Ok(match returning {
            Returning::Sequence => (call, DataType::Xml(XmlType::Sequence)),
            Returning::Content => {
                let t = DataType::Xml(DOCUMENT_CONTENT);
                (Expr::Call(ScalarRef(Arc::new(XmlDocument(t.clone()))), vec![call]), t)
            }
        })
    }

    /// [§8.4] XMLEXISTS: whether the XMLQUERY of its expression and
    /// arguments RETURNING SEQUENCE EMPTY ON EMPTY is not empty, Unknown of
    /// the null value \[GR2\].
    pub(super) fn xml_exists(&self, q: &XmlQuery, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let bound = self.xml_arguments(q.arguments.as_ref(), scope)?;
        let evaluation = self.evaluation(&q.text, &bound)?;
        Ok((Expr::Call(ScalarRef(Arc::new(XmlExistsOf(evaluation))), bound.values), DataType::Boolean))
    }

    /// [§7.1] XMLTABLE, the collection of its table primary: its
    /// namespaces in scope within it \[SR4a\]; the rows of its row pattern,
    /// RETURNING SEQUENCE BY REF EMPTY ON EMPTY, of its arguments, BY REF of
    /// none \[SR4c\]; an array of rows of its columns \[SR4h\].
    pub(super) fn xml_table(&self, t: &XmlTable, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.in_namespaces(t.namespaces.as_deref(), || {
            let bound = self.xml_arguments(t.arguments.as_ref(), scope)?;
            let rows = self.evaluation(&t.pattern, &bound)?;
            let arguments = bound.values.len();
            let mut values = bound.values;
            let (mut columns, mut fields) = (Vec::new(), Vec::new());
            for column in &t.columns {
                let (c, field) = self.xml_table_column(column, t, &mut values)?;
                if fields.iter().any(|f: &Field| f.name == field.name) {
                    return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §7.1 SR4h] the column {} is named twice", field.name)));
                }
                columns.push(c);
                fields.push(field);
            }
            if columns.iter().filter(|c| matches!(c, TableColumn::Ordinality)).count() > 1 {
                return Err(rule("[ISO/IEC 9075-14:2006 draft §7.1 SR4d] an <XML table> has one ordinality column at most"));
            }
            let array = DataType::array(DataType::row(fields), MAX_ARRAY_CARDINALITY);
            let f = XmlTableOf { rows, arguments, columns, array: array.clone() };
            Ok((Expr::Call(ScalarRef(Arc::new(f)), values), array))
        })
    }

    /// A column of `t` [§7.1 SR4e], its default's value added to `values`:
    /// FOR ORDINALITY an exact number of scale 0; else of its type, an
    /// XML(SEQUENCE) one passed as written, or by the argument list's
    /// mechanism, of which it then has one [SR4e ii 2 A], another XML type
    /// its value made a document node and copied, another type cast by
    /// XMLCAST, no mechanism of either [SR4e ii 2 B, 7]; its pattern its
    /// PATH, or its name [SR4e ii 4]; its default NULL if none [SR4e ii 3].
    fn xml_table_column(&self, column: &XmlTableColumn, t: &XmlTable, values: &mut Vec<Expr>) -> Result<(TableColumn, Field)> {
        let c = match column {
            XmlTableColumn::Ordinality(name) => {
                let field = Field { name: name.clone(), data_type: super::super::composite::integer(), unnamed: false };
                return Ok((TableColumn::Ordinality, field));
            }
            XmlTableColumn::Regular(c) => c,
        };
        let (target, _) = super::resolved_type(&self.context(), &c.data_type, self.who())?;
        let input = (Expr::Column(0), DataType::Xml(DOCUMENT_CONTENT));
        let (content, cast) = match (&target, c.passing) {
            (DataType::Xml(XmlType::Sequence), passing) => {
                let Some(mechanism) = passing.or(t.arguments.as_ref().map(|a| a.default)) else {
                    return Err(rule("[ISO/IEC 9075-14:2006 draft §7.1 SR4e ii 2 A I] a column of XML(SEQUENCE) has a passing mechanism, as the XMLTABLE has no argument list").or_at(c.at));
                };
                (false, self.xml_cast((Expr::Column(0), target.clone()), target.clone(), mechanism)?.0)
            }
            (_, Some(_)) => {
                return Err(rule(format!(
                    "[ISO/IEC 9075-14:2006 draft §7.1 SR4e ii 2 B] a column of {target} has no <XML passing mechanism>"
                ))
                .or_at(c.at));
            }
            (DataType::Xml(_), None) => (true, self.xml_cast(input, target.clone(), Passing::ByValue)?.0),
            (_, None) => (true, self.cast_from_xml(Expr::Column(0), target.clone()).map_err(|e| e.or_at(c.at))?.0),
        };
        let query = self.xquery(c.path.as_deref().unwrap_or(&c.name), &[]).map_err(|e| e.or_at(c.at))?;
        values.push(self.xml_table_default(c.default.as_ref(), &target)?);
        let regular = RegularColumn { query, content, cast, default: values.len().saturating_sub(1) };
        Ok((TableColumn::Regular(regular), Field { name: c.name.clone(), data_type: target, unnamed: false }))
    }

    /// The value of a column's `<default option>` [§7.1 SR4e ii 3], NULL
    /// if none, cast to the column's type `target` as CASE gives it.
    fn xml_table_default(&self, default: Option<&DefaultOption>, target: &DataType) -> Result<Expr> {
        let value = match default {
            None | Some(DefaultOption::Null) => return Ok(Expr::Const(Value::Null)),
            Some(DefaultOption::Empty(_)) => {
                return Err(rule(format!("[ISO/IEC 9075-14:2006 draft §7.1 SR4e ii 8] an empty collection is no default of {target}")));
            }
            Some(DefaultOption::Literal(l)) => ValueExpression::Value(ValueSpecification::Literal(l.clone())),
            Some(DefaultOption::User(f)) => ValueExpression::Value(ValueSpecification::User(*f)),
            Some(DefaultOption::Datetime(f)) => ValueExpression::Datetime(*f),
        };
        let bound = self.value_expression(&value, Scope::Rows("[ISO/IEC 9075-14:2006 draft §7.1] a default has no set function"))?;
        Ok(self.cast_bound(bound, target.clone())?.0)
    }
}
