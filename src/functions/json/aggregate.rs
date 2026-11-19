//! [ISO/IEC 9075-2:2023 §10.11] JSON_OBJECTAGG and JSON_ARRAYAGG, the
//! `<JSON aggregate function>`s, as set functions; and JSON_ARRAY by query
//! [§6.34 GR4], the aggregate of its query's rows.
//!
//! Each call's argument is a row, which is never the null value, so the
//! null values the rules keep [GR4b ii 2 D I, GR5f ii 2 A] reach the
//! accumulator, and no null value eliminated is warned of: JSON_OBJECTAGG's
//! is the ROW of its name and value, JSON_ARRAYAGG's the ROW of its value
//! and its sort keys. JSON_OBJECTAGG gives each row to the library's state
//! (`JsonObjectAgg`) as it comes. JSON_ARRAYAGG holds its rows, each key as
//! its collation orders it, and orders them by their keys, stably, when it
//! is finished \[GR5c\], as ARRAY_AGG does; of no ORDER BY, it keeps the
//! order the rows came in (US044). It holds at most `MAX_ELEMENTS` rows of
//! a value not left out, and one more is 2203D, so what it holds is
//! bounded, as the library's states are.

use std::cmp::Ordering;
use std::sync::Arc;

use super::super::{Accumulator, AggregateFunction};
use super::construct::{JSON_ARRAYAGG, JSON_OBJECTAGG, keys_words, null_words};
use super::operands::{input_type, name_type};
use crate::ast::JsonFormat;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::json::{ArrayOptions, Input, JsonObjectAgg, Keys, MAX_ELEMENTS, ObjectOptions, OnNull, json_array_by_query};
use crate::params::ParameterType;
use crate::value::collation::CollationRef;
use crate::value::types::composite::unordered;
use crate::value::{DataType, SortOrder, Value};

/// [§10.11] JSON_OBJECTAGG of its clauses, explicit or implicit.
pub struct JsonObjectAggregate {
    /// The `<JSON constructor null clause>`.
    pub on_null: OnNull,
    /// The `<JSON key uniqueness constraint>`.
    pub keys: Keys,
    /// The type of the `<JSON output clause>`.
    pub returning: DataType,
    /// Its value's `<JSON input clause>`.
    pub format: Option<JsonFormat>,
}

/// The error of an aggregate given another argument than the binder gives
/// it.
fn not_bound(name: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {name} was bound without its row"))
}

impl AggregateFunction for JsonObjectAggregate {
    fn name(&self) -> &str {
        JSON_OBJECTAGG
    }
    /// [§10.11 SR4d] The type of its `<JSON output clause>`, of a ROW of a
    /// name, a character string [§6.34 F], and a value of a type an input
    /// may be [§10.12].
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let Some(DataType::Row(r)) = arg else { return Err(not_bound(JSON_OBJECTAGG)) };
        let [name, value] = r.fields.as_slice() else { return Err(not_bound(JSON_OBJECTAGG)) };
        name_type(&name.data_type, JSON_OBJECTAGG)?;
        input_type(&value.data_type, self.format, "the value of JSON_OBJECTAGG")?;
        Ok(self.returning.clone())
    }
    /// \[EXTENSION\] [§20.7 GR5a] As JSON_OBJECT's (`JsonConstruct`): a
    /// `?` as its name, or as its value FORMAT JSON, is CHARACTER VARYING;
    /// as a value without FORMAT JSON, of none \[GR5b\].
    fn parameter_type(&self, position: usize) -> Option<ParameterType> {
        (position == 0 || self.format.is_some()).then_some(ParameterType::Varying)
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        let options = ObjectOptions { on_null: self.on_null, keys: self.keys, returning: &self.returning };
        Box::new(Members { state: JsonObjectAgg::new(&options), format_json: self.format.is_some() })
    }
    /// Its clauses, so two calls of other clauses are computed apart.
    fn label_suffix(&self) -> String {
        let format = if self.format.is_some() { " FORMAT JSON" } else { "" };
        format!("{format} {} {} RETURNING {}", null_words(self.on_null), keys_words(self.keys), self.returning)
    }
}

/// The state of a JSON_OBJECTAGG, or the error of making it.
struct Members {
    state: Result<JsonObjectAgg>,
    format_json: bool,
}

impl Accumulator for Members {
    /// [§10.11 GR4b ii 2] The row of a name and a value.
    fn update(&mut self, v: &Value) -> Result<()> {
        let state = self.state.as_mut().map_err(|e| e.clone())?;
        let Value::Row(row) = v else { return Err(not_bound(JSON_OBJECTAGG)) };
        let [name, value] = &**row else { return Err(not_bound(JSON_OBJECTAGG)) };
        state.add(name, &Input { value, format_json: self.format_json })
    }
    /// [§10.11 GR4b, GR6b] Null of no rows, else the object's JSON text.
    fn finish(&self) -> Result<Value> {
        self.state.as_ref().map_err(Clone::clone)?.finish()
    }
}

/// [§10.11] JSON_ARRAYAGG, or [§6.34] JSON_ARRAY by query, of its clauses,
/// explicit or implicit, and its sort keys.
pub struct JsonArrayAggregate {
    /// The `<JSON constructor null clause>`.
    pub on_null: OnNull,
    /// The type of the `<JSON output clause>`.
    pub returning: DataType,
    /// Its value's `<JSON input clause>`.
    pub format: Option<JsonFormat>,
    /// Per sort key, its direction and null ordering.
    pub orders: Vec<SortOrder>,
    /// Per sort key, the collation it orders by, if not the default.
    pub collations: Vec<Option<CollationRef>>,
    /// Whether it is JSON_ARRAY by query, which makes an empty array of no
    /// rows [§6.34 GR4b i], where JSON_ARRAYAGG gives the null value
    /// [§10.11 GR5f i].
    pub by_query: bool,
}

impl JsonArrayAggregate {
    /// The name of the function it is.
    fn function(&self) -> &'static str {
        if self.by_query { super::construct::JSON_ARRAY } else { JSON_ARRAYAGG }
    }
}

impl AggregateFunction for JsonArrayAggregate {
    fn name(&self) -> &str {
        self.function()
    }
    /// [§6.34 SR7b, §10.11 SR5b] The type of its `<JSON output clause>`,
    /// of a ROW of a value of a type an input may be [§10.12] and its sort
    /// keys, none LOB-ordered or array-ordered [§9.14 SR2f, SR3].
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let function = self.function();
        let Some(DataType::Row(r)) = arg else { return Err(not_bound(function)) };
        let mut fields = r.fields.iter().map(|f| &f.data_type);
        let value = fields.next().ok_or_else(|| not_bound(function))?;
        input_type(value, self.format, &format!("the value of {function}"))?;
        if let Some((key, these)) = fields.find_map(|t| Some((t, unordered(t)?))) {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!(
                    "[ISO/IEC 9075-2:2023 §9.14 SR3] {function} orders by values of {key}, and {these} are compared for equality alone"
                ),
            ));
        }
        Ok(self.returning.clone())
    }
    /// \[EXTENSION\] [§20.7 GR5a] As JSON_ARRAY's (`JsonConstruct`): a `?`
    /// as its value FORMAT JSON is CHARACTER VARYING; without FORMAT JSON,
    /// or as a sort key, of none \[GR5b\].
    fn parameter_type(&self, position: usize) -> Option<ParameterType> {
        (position == 0 && self.format.is_some()).then_some(ParameterType::Varying)
    }
    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        Box::new(Elements {
            rows: Vec::new(),
            any: false,
            on_null: self.on_null,
            returning: self.returning.clone(),
            format_json: self.format.is_some(),
            orders: self.orders.clone(),
            collations: self.collations.clone(),
            by_query: self.by_query,
        })
    }
    /// Its clauses and its keys' directions, null orderings and
    /// collations, by their places in its row.
    fn label_suffix(&self) -> String {
        let keys = self.orders.iter().zip(&self.collations).enumerate().map(|(i, (order, collation))| {
            let collated = collation.as_ref().map_or_else(String::new, |c| format!(" COLLATE {}", c.collation.name()));
            let down = if order.descending { " DESC" } else { "" };
            format!("{}{collated}{down}{}", i.saturating_add(2), order.null_suffix())
        });
        let keys: Vec<String> = keys.collect();
        let order = if keys.is_empty() { String::new() } else { format!(" ORDER BY {}", keys.join(", ")) };
        let format = if self.format.is_some() { " FORMAT JSON" } else { "" };
        format!("{format}{order} {} RETURNING {}", null_words(self.on_null), self.returning)
    }
}

/// The rows of a JSON_ARRAYAGG so far, each its value then its keys, each
/// key as its collation orders it; and whether any row came, one of a
/// value left out too.
struct Elements {
    rows: Vec<Arc<[Value]>>,
    any: bool,
    on_null: OnNull,
    returning: DataType,
    format_json: bool,
    orders: Vec<SortOrder>,
    collations: Vec<Option<CollationRef>>,
    by_query: bool,
}

impl Elements {
    /// [§10.10] How two rows order by their keys.
    fn order(a: &[Value], b: &[Value], orders: &[SortOrder]) -> Ordering {
        let keys = a.iter().zip(b).skip(1).zip(orders);
        keys.map(|((x, y), order)| order.cmp(x, y)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
    }

    /// [§6.34 GR4b ii 2, §10.11 GR5g] The error of more elements than
    /// `MAX_ELEMENTS`.
    fn too_many(&self) -> SqlError {
        let rule = if self.by_query { "§6.34 GR4b ii 2" } else { "§10.11 GR5g" };
        SqlError::new(Code::Data, format!("[ISO/IEC 9075-2:2023 {rule}] the elements of a JSON array are more than {MAX_ELEMENTS}"))
            .with_state(SqlState::TOO_MANY_JSON_ARRAY_ELEMENTS)
    }
}

impl Accumulator for Elements {
    /// [§10.11 GR5b] The row of a value and its sort keys: held, unless its
    /// value is null and ABSENT ON NULL leaves it out \[GR5g ii\].
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(row) = v else { return Err(not_bound(self.function_name())) };
        self.any = true;
        let absent = self.on_null == OnNull::Absent && row.first().is_none_or(Value::is_null);
        if absent {
            return Ok(());
        }
        if self.rows.len() >= MAX_ELEMENTS {
            return Err(self.too_many());
        }
        let keyed = row.iter().enumerate().map(|(i, part)| {
            let collation = i.checked_sub(1).and_then(|k| self.collations.get(k)).and_then(Option::as_ref);
            collation.map_or_else(|| Ok(part.clone()), |c| c.sort_key(part))
        });
        self.rows.push(if self.collations.iter().any(Option::is_some) { keyed.collect::<Result<_>>()? } else { Arc::clone(row) });
        Ok(())
    }
    /// [§10.11 GR5c, GR5f, GR6b; §6.34 GR4b] Of no rows the null value, or
    /// of JSON_ARRAY by query an empty array; else the JSON text of the
    /// array of the values, ordered by their keys, stably.
    fn finish(&self) -> Result<Value> {
        if !self.any && !self.by_query {
            return Ok(Value::Null);
        }
        let mut rows: Vec<&Arc<[Value]>> = self.rows.iter().collect();
        rows.sort_by(|a, b| Elements::order(a, b, &self.orders));
        let values: Vec<Value> = rows.into_iter().map(|r| r.first().cloned().unwrap_or(Value::Null)).collect();
        json_array_by_query(&values, self.format_json, &ArrayOptions { on_null: self.on_null, returning: &self.returning })
    }
}

impl Elements {
    /// The name of the function whose state it is.
    fn function_name(&self) -> &'static str {
        if self.by_query { super::construct::JSON_ARRAY } else { JSON_ARRAYAGG }
    }
}
