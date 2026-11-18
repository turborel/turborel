//! The functions of the SQL/XML operators that evaluate XQuery [ISO/IEC
//! 9075-14:2006 draft §6.18, §7.1, §8.4]: XMLQUERY and XMLEXISTS, each a
//! query compiled as the statement is bound, evaluated of each row's
//! context item and variables; and XMLTABLE, the rows its row pattern
//! gives, each column of its own query, as an array of rows.
//!
//! \[DESIGN\] RULES.md D44, D106: each is `in_statement`, so an evaluation
//! looks whether its statement was cancelled as it goes.

use std::sync::Arc;

use super::super::{ScalarFunction, ScalarRef};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::{NoSubqueries, Subqueries, eval};
use crate::logical::Expr;
use crate::value::xml::XmlValue;
use crate::value::{DataType, Decimal, Value};
use crate::xml::{Item, Moment, Sequence, decode, document, encode};
use crate::xquery::{Input, Query, Reduction};

/// What evaluates a query, as XMLQUERY gives its arguments.
#[derive(Debug, Clone)]
pub struct Evaluation {
    /// The query.
    pub query: Arc<Query>,
    /// Whether the first argument is the context item.
    pub context: bool,
    /// Whether the context item is a value of one node, of XML(DOCUMENT)
    /// or XML(CONTENT) [§4.2.2]: what a query is reduced to an XML
    /// index's lookup of (`XmlFilter`).
    pub one_node: bool,
    /// The statement's current dateTime [XQuery Appendix C.2].
    pub now: Moment,
    /// Its implicit timezone, in minutes east of UTC.
    pub timezone: i16,
}

/// The error of a context item of more than one item [§6.18 GR1a ii 2 B
/// III]: SQLSTATE 2200V.
fn invalid_context_item() -> SqlError {
    SqlError::new(Code::Data, "[ISO/IEC 9075-14:2006 draft §6.18 GR1a ii 2 B III] the context item passed is more than one XQuery item")
        .with_state(SqlState::INVALID_XQUERY_CONTEXT_ITEM)
}

/// The sequence of the XML value `v`, the empty sequence of the null value
/// [§6.18 GR1b i 1].
fn sequence_of(v: &Value) -> Result<Sequence> {
    if let Value::Xml(x) = v {
        return Ok((*x.sequence()?).clone());
    }
    if v.is_null() {
        return Ok(Sequence::default());
    }
    Err(SqlError::new(Code::Api, format!("[DESIGN] an XQuery evaluation was passed {v}, not an XML value")))
}

impl Evaluation {
    /// The value of the query of `args` [§6.18 GR1, GR2]: none, the null
    /// value, if the context item passed is null [GR1a ii 1]; no context
    /// item of an empty one, of more than one item SQLSTATE 2200V [GR1a ii 2
    /// B]; each variable's value, the empty sequence of the null value
    /// \[GR1b\].
    ///
    /// # Errors
    ///
    /// SQLSTATE 2200V of a context item of more than one item; the
    /// query's, as SQLSTATE 10000 \[GR3\]; SQLCODE -920 if the statement is
    /// cancelled.
    pub fn evaluate(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Option<Sequence>> {
        let (context_item, variables) = match (self.context, args.split_first()) {
            (true, Some((first, rest))) => {
                if first.is_null() {
                    return Ok(None);
                }
                let mut items = sequence_of(first)?.into_items();
                if items.len() > 1 {
                    return Err(invalid_context_item());
                }
                (items.pop(), rest)
            }
            (true, None) | (false, _) => (None, args),
        };
        let variables = variables.iter().map(sequence_of).collect::<Result<Vec<_>>>()?;
        self.with(context_item, variables, statement).map(Some)
    }

    /// The value of the query of the context item `item` and `variables`.
    fn with(&self, context_item: Option<Item>, variables: Vec<Sequence>, statement: &dyn Subqueries) -> Result<Sequence> {
        let interrupted = || statement.interrupted();
        self.query.evaluate(Input { context_item, variables, now: self.now, timezone: self.timezone, interrupted: &interrupted })
    }
}

/// `sequence` as an XML value: of BY VALUE, a copy of it [§10.18].
fn passed(sequence: Sequence, by_value: bool) -> Result<Value> {
    let value = if by_value { decode(&encode(&sequence))? } else { sequence };
    Ok(Value::Xml(XmlValue::of(value)))
}

/// [§6.18] XMLQUERY RETURNING SEQUENCE: its query's value, the null value
/// of the empty sequence if NULL ON EMPTY \[GR4a\], a copy of it if its
/// returning mechanism is BY VALUE [GR4b ii].
#[derive(Debug)]
pub struct XmlQueryOf {
    /// What evaluates the query.
    pub evaluation: Evaluation,
    /// Whether the returning mechanism is BY VALUE.
    pub by_value: bool,
    /// Whether NULL ON EMPTY is written.
    pub null_on_empty: bool,
}

impl ScalarFunction for XmlQueryOf {
    fn name(&self) -> &'static str {
        "XMLQUERY"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Xml(crate::value::xml::XmlType::Sequence))
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_in_statement(args, &NoSubqueries)
    }

    fn in_statement(&self) -> bool {
        true
    }

    fn call_in_statement(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        let Some(sequence) = self.evaluation.evaluate(args, statement)? else { return Ok(Value::Null) };
        if sequence.items().is_empty() && self.null_on_empty {
            return Ok(Value::Null);
        }
        passed(sequence, self.by_value)
    }

    fn xml_filter(&self) -> Option<XmlFilter> {
        reduction_of(&self.evaluation).filter(|_| self.null_on_empty).map(XmlFilter::NullOfEmpty)
    }
}

/// \[EXTENSION\] RULES.md D106: what a function's value says of whether a
/// query's value is empty, its context item the function's first
/// argument, the query reduced to an XML index's lookup
/// (`ScalarFunction::xml_filter`).
#[derive(Debug, Clone)]
pub enum XmlFilter {
    /// False or null exactly when the query's value is empty, or its
    /// context item null: XMLEXISTS [§8.4 GR2].
    Exists(Reduction),
    /// As `Exists`, a filter the planner adds where it is implied, which
    /// no row is checked by: XMLTABLE's row pattern (`XmlRowsOf`).
    Implied(Reduction),
    /// Null exactly when the query's value is empty, or its context item
    /// null: XMLQUERY NULL ON EMPTY [§6.18 GR4a].
    NullOfEmpty(Reduction),
    /// No row exactly when the query's value is empty, or its context item
    /// null: XMLTABLE [§7.1 SR4h], and its row pattern as an implied filter
    /// of the rows it is of.
    NoRows(Reduction, ScalarRef),
    /// Null exactly when its one argument is: XMLDOCUMENT.
    KeepsNull,
    /// A copy of its one argument, null of null: an XML value passed BY
    /// VALUE [§10.18].
    Copy,
}

/// The reduction of `evaluation`'s query, of a context item passed of
/// one node.
fn reduction_of(evaluation: &Evaluation) -> Option<Reduction> {
    (evaluation.context && evaluation.one_node).then(|| evaluation.query.reduced()).flatten()
}

/// The value of XMLEXISTS of `evaluation` and `args` [§8.4 GR2].
fn exists(evaluation: &Evaluation, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
    Ok(match evaluation.evaluate(args, statement)? {
        Some(sequence) => Value::Bool(!sequence.items().is_empty()),
        None => Value::Null,
    })
}

/// [§8.4] XMLEXISTS: Unknown if the context item passed is null, False if
/// its query's value is the empty sequence, else True \[GR2\].
#[derive(Debug)]
pub struct XmlExistsOf(pub Evaluation);

impl ScalarFunction for XmlExistsOf {
    fn name(&self) -> &'static str {
        "XMLEXISTS"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_in_statement(args, &NoSubqueries)
    }

    fn in_statement(&self) -> bool {
        true
    }

    fn call_in_statement(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        exists(&self.0, args, statement)
    }

    fn xml_filter(&self) -> Option<XmlFilter> {
        reduction_of(&self.0).map(XmlFilter::Exists)
    }

    fn raise_free(&self) -> bool {
        reduction_of(&self.0).is_some_and(|r| r.raise_free)
    }
}

/// \[EXTENSION\] RULES.md D106: XMLTABLE's row pattern as a filter of the
/// rows it is of, which keeps those it gives a row of: XMLEXISTS of it.
/// The planner adds it where an XML index may find them, and checks no
/// row by it, as XMLTABLE gives no row of the others.
#[derive(Debug)]
pub struct XmlRowsOf(pub Evaluation);

impl ScalarFunction for XmlRowsOf {
    fn name(&self) -> &'static str {
        "XMLTABLE"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_in_statement(args, &NoSubqueries)
    }

    fn in_statement(&self) -> bool {
        true
    }

    fn call_in_statement(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        exists(&self.0, args, statement)
    }

    fn xml_filter(&self) -> Option<XmlFilter> {
        reduction_of(&self.0).map(XmlFilter::Implied)
    }
}

/// A column of an XMLTABLE [§7.1 SR4e].
#[derive(Debug, Clone)]
pub enum TableColumn {
    /// FOR ORDINALITY: the row's item's place, from 1.
    Ordinality,
    /// A regular column.
    Regular(RegularColumn),
}

/// A regular column of an XMLTABLE [§7.1 SR4e ii].
#[derive(Debug, Clone)]
pub struct RegularColumn {
    /// Its column pattern's query, of the row's item as the context item.
    pub query: Arc<Query>,
    /// Whether its value is RETURNING CONTENT, a document node made of
    /// the query's value [§6.18 SR10a]: of a type not XML(SEQUENCE).
    pub content: bool,
    /// The XMLCAST of that value, `Expr::Column(0)`, to the column's type.
    pub cast: Expr,
    /// Its default's place among the arguments.
    pub default: usize,
}

/// [§7.1] XMLTABLE: of each item of its row pattern's value, a row of each
/// column: the ordinal, or the XMLCAST of its column pattern's value of
/// the item if it is not empty, else its default [SR4e ii 8]; the rows an
/// array, as the collection of a collection derived table.
#[derive(Debug)]
pub struct XmlTableOf {
    /// What evaluates the row pattern.
    pub rows: Evaluation,
    /// How many arguments the row pattern takes, the defaults after them.
    pub arguments: usize,
    /// The columns.
    pub columns: Vec<TableColumn>,
    /// The type of the array of rows.
    pub array: DataType,
}

impl ScalarFunction for XmlTableOf {
    fn name(&self) -> &'static str {
        "XMLTABLE"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.array.clone())
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_in_statement(args, &NoSubqueries)
    }

    fn in_statement(&self) -> bool {
        true
    }

    fn call_in_statement(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        let passed_args = args.get(..self.arguments).unwrap_or_default();
        let Some(items) = self.rows.evaluate(passed_args, statement)? else { return Ok(Value::Array(Arc::from(Vec::new()))) };
        let mut rows = Vec::with_capacity(items.items().len());
        for (n, item) in items.into_items().into_iter().enumerate() {
            statement.interrupted()?;
            let mut row = Vec::with_capacity(self.columns.len());
            for column in &self.columns {
                row.push(self.column_value(column, (&item, n), args, statement)?);
            }
            rows.push(Value::Row(Arc::from(row)));
        }
        Ok(Value::Array(Arc::from(rows)))
    }

    /// Of a row pattern of the context item alone.
    fn xml_filter(&self) -> Option<XmlFilter> {
        let reduction = reduction_of(&self.rows).filter(|_| self.arguments == 1)?;
        Some(XmlFilter::NoRows(reduction, ScalarRef(Arc::new(XmlRowsOf(self.rows.clone())))))
    }
}

impl XmlTableOf {
    /// The value of `column` of the `n`th item, `item`, of the rows [§7.1
    /// SR4e ii 8]: CASE WHEN XMLEXISTS of its pattern THEN the XMLCAST of
    /// XMLQUERY of it ELSE its default END.
    fn column_value(&self, column: &TableColumn, (item, n): (&Item, usize), args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        let column = match column {
            TableColumn::Ordinality => {
                return Ok(Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX).saturating_add(1), 0)));
            }
            TableColumn::Regular(c) => c,
        };
        let evaluation = Evaluation { query: Arc::clone(&column.query), context: true, ..self.rows.clone() };
        let value = evaluation.with(Some(item.clone()), Vec::new(), statement)?;
        if value.items().is_empty() {
            return Ok(args.get(column.default).cloned().unwrap_or(Value::Null));
        }
        let value = if column.content { document(&value)? } else { value };
        eval(&column.cast, &vec![Value::Xml(XmlValue::of(value))], statement)
    }
}

#[cfg(test)]
mod tests;
