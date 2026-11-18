//! [ISO/IEC 9075-2:2023 §6.34, §10.11, §8.22] The SQL/JSON constructor
//! functions, the `<JSON aggregate function>`s and the `<JSON predicate>`
//! bound: JSON_OBJECT and JSON_ARRAY by enumeration each a call of a
//! function of its own (`JsonConstruct`); JSON_ARRAY by query the
//! aggregate of its query's rows, as ARRAY by query is
//! (`Binder::query_aggregate`); JSON_OBJECTAGG and JSON_ARRAYAGG set
//! functions of a row, as ARRAY_AGG is; IS JSON a BOOLEAN `Cond::Value`
//! judges. Each clause not written is made implicit by its Syntax Rule,
//! and each operand's declared type checked as the statement is bound.

use std::sync::Arc;

use super::super::{Binder, Scope};
use crate::ast::{
    Composite, IsJson, JsonAggregate, JsonConstructor, JsonFormat, JsonInput, JsonMember, JsonOutput, SetArg, SetFunctionSpecification,
    Subquery, ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::{
    AggregateFunction, AggregateRef, Construction, IsJsonCall, JSON_ARRAY, JSON_ARRAYAGG, JSON_OBJECT, JSON_OBJECTAGG, JsonArrayAggregate,
    JsonConstruct, JsonObjectAggregate, OPERANDS_PER_MEMBER, ScalarRef, input_type, json_text, name_type, output_type,
};
use crate::json::{JsonType, Keys, OnNull};
use crate::logical::{Cond, Expr};
use crate::value::{DataType, SortOrder};

/// The Syntax Rule an input of IS JSON of another type than a string's
/// breaks.
const IS_JSON_RULE: &str = "[ISO/IEC 9075-2:2023 §8.22 F] the input of IS JSON is a <string value expression>";

impl Binder<'_> {
    /// [§6.34] JSON_OBJECT or JSON_ARRAY `c`, and its declared type, the
    /// type of its `<JSON output clause>` \[SR6d, SR7b\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the returned type is no string type [§10.13 SR2], a
    /// name no character string [F], a value of no type it may be [§10.12],
    /// or a query of more columns than one \[SR7c\]; -900 of a binary
    /// string's JSON text or type.
    pub(in crate::binder) fn json_constructor(&self, c: &JsonConstructor, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match c {
            JsonConstructor::Object { members, on_null, keys, returning } => {
                // [§6.34 SR6a, SR6b] NULL ON NULL and WITHOUT UNIQUE KEYS if
                // not written.
                let construction = Construction::Object(on_null.unwrap_or(OnNull::Null), keys.unwrap_or(Keys::Any));
                self.object(members, construction, returning.as_ref(), scope)
            }
            JsonConstructor::Array { elements, on_null, returning } => {
                // [§6.34 SR7a] ABSENT ON NULL if not written.
                let construction = Construction::Array(on_null.unwrap_or(OnNull::Absent));
                self.array(elements, construction, returning.as_ref(), scope)
            }
            JsonConstructor::ArrayQuery { query, format, on_null, returning } => {
                self.array_by_query(query, (*format, on_null.unwrap_or(OnNull::Absent), returning.as_ref()), scope)
            }
        }
    }

    /// [§6.34 GR2] JSON_OBJECT of `members`, as `construction` makes it.
    fn object(
        &self,
        members: &[JsonMember],
        construction: Construction,
        returning: Option<&JsonOutput>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let output = output_type(returning, JSON_OBJECT)?;
        let formats: Vec<bool> = members.iter().map(|m| m.value.format.is_some()).collect();
        let operands: Vec<ValueExpression> = members.iter().flat_map(|m| [m.name.clone(), m.value.value.clone()]).collect();
        let f = ScalarRef(Arc::new(JsonConstruct::new(construction, formats.clone(), output)));
        let (exprs, types) = self.arguments(&*f.0, &operands, scope)?;
        let values: Vec<&DataType> = types.iter().skip(1).step_by(OPERANDS_PER_MEMBER).collect();
        let f = if implicit_json(returning, &values) {
            ScalarRef(Arc::new(JsonConstruct::new(construction, formats, DataType::Json)))
        } else {
            f
        };
        for (member, pair) in members.iter().zip(types.chunks(OPERANDS_PER_MEMBER)) {
            let mut pair = pair.iter();
            pair.next().into_iter().try_for_each(|name| name_type(name, JSON_OBJECT))?;
            pair.next().into_iter().try_for_each(|value| input_type(value, member.value.format, "a value of JSON_OBJECT"))?;
        }
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// [§6.34 GR3] JSON_ARRAY of `elements`, as `construction` makes it.
    fn array(
        &self,
        elements: &[JsonInput],
        construction: Construction,
        returning: Option<&JsonOutput>,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let output = output_type(returning, JSON_ARRAY)?;
        let formats: Vec<bool> = elements.iter().map(|e| e.format.is_some()).collect();
        let operands: Vec<ValueExpression> = elements.iter().map(|e| e.value.clone()).collect();
        let f = ScalarRef(Arc::new(JsonConstruct::new(construction, formats.clone(), output)));
        let (exprs, types) = self.arguments(&*f.0, &operands, scope)?;
        let f = if implicit_json(returning, &types.iter().collect::<Vec<_>>()) {
            ScalarRef(Arc::new(JsonConstruct::new(construction, formats, DataType::Json)))
        } else {
            f
        };
        for (element, t) in elements.iter().zip(&types) {
            input_type(t, element.format, "an element of JSON_ARRAY")?;
        }
        let t = f.0.return_type(&types)?;
        Ok((Expr::Call(f, exprs), t))
    }

    /// [§6.34 SR7c, GR4] JSON_ARRAY by query of `query`, its one column
    /// \[SR7c\] the elements, in the order of its rows; of no rows an
    /// empty array \[GR4b i\].
    fn array_by_query(
        &self,
        query: &Subquery,
        (format, on_null, returning): (Option<JsonFormat>, OnNull, Option<&JsonOutput>),
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let output = output_type(returning, JSON_ARRAY)?;
        self.query_aggregate(query, scope, ("§6.34 SR7c", JSON_ARRAY), |element, orders, collations| {
            input_type(element, format, "the query of JSON_ARRAY")?;
            let made = if implicit_json(returning, &[element]) { DataType::Json } else { output.clone() };
            let func = JsonArrayAggregate { on_null, returning: made.clone(), format, orders, collations, by_query: true };
            let label = format!("{JSON_ARRAY}({}{})", crate::logical::render_expr(&Expr::Column(0), &[]), func.label_suffix());
            Ok((AggregateRef(Arc::new(func)), made, label))
        })
    }

    /// [§8.22] IS JSON `p`, judged as the BOOLEAN it gives; of IS NOT JSON,
    /// its negation \[SR4\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the input is no character string \[F\], or ENCODING
    /// is written of one [§10.12 SR8]; -900 of a binary string.
    pub(in crate::binder) fn is_json(&self, p: &IsJson, scope: Scope<'_>) -> Result<Cond> {
        // [SR2, SR3] VALUE and WITHOUT UNIQUE KEYS if not written.
        let kind = p.kind.unwrap_or(JsonType::Value);
        let f = ScalarRef(Arc::new(IsJsonCall::new(p.not, kind, p.keys.unwrap_or(Keys::Any))));
        let (exprs, types) = self.arguments(&*f.0, std::slice::from_ref(&p.input.value), scope)?;
        types.iter().try_for_each(|t| json_text(t, p.input.format, IS_JSON_RULE))?;
        Ok(Cond::Value(Expr::Call(f, exprs)))
    }

    /// [§10.11] The `<JSON aggregate function>` `sf`, if it is one.
    pub(in crate::binder) fn json_aggregate(&self, sf: &SetFunctionSpecification, scope: Scope<'_>) -> Option<Result<(Expr, DataType)>> {
        let json = sf.json.as_deref()?;
        Some(if sf.name == JSON_OBJECTAGG { self.object_aggregate(sf, json, scope) } else { self.array_aggregate_json(sf, json, scope) })
    }

    /// [§10.11 SR4, GR4] JSON_OBJECTAGG: the aggregate of the ROW of its
    /// name and value, so a null value is kept.
    fn object_aggregate(&self, sf: &SetFunctionSpecification, json: &JsonAggregate, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let SetArg::All(row) = &sf.arg else { return Err(not_read()) };
        // [§10.11 SR1a] Its value, the ROW's second field.
        let value = if let ValueExpression::Composite(c, _) = &**row
            && let Composite::Row(parts) = &**c
        {
            parts.last()
        } else {
            None
        };
        let returning = self.aggregate_returning(json, value, JSON_OBJECTAGG)?;
        // [SR4a, SR4b] NULL ON NULL and WITHOUT UNIQUE KEYS if not written.
        let func = JsonObjectAggregate {
            on_null: json.on_null.unwrap_or(OnNull::Null),
            keys: json.keys.unwrap_or(Keys::Any),
            returning,
            format: json.format,
        };
        self.aggregate(AggregateRef(Arc::new(func)), (Some(row), false), sf.filter.as_deref(), scope)
    }

    /// [§10.11 SR5, GR5] JSON_ARRAYAGG ( V ORDER BY K1, ... ): the
    /// aggregate of the ROW ( V, K1, ... ), so a null V is kept, ordered
    /// by its keys, each as its collation orders it [§9.15], as ARRAY_AGG
    /// is.
    fn array_aggregate_json(&self, sf: &SetFunctionSpecification, json: &JsonAggregate, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let SetArg::All(value) = &sf.arg else { return Err(not_read()) };
        let returning = self.aggregate_returning(json, Some(value), JSON_ARRAYAGG)?;
        let mut parts = vec![(**value).clone()];
        let mut collations = Vec::new();
        for key in &sf.order_by {
            // A key not a character string is its own sort key.
            collations.push(self.collation_of(&[self.derived(&key.key)?])?);
            parts.push(key.key.clone());
        }
        let row = ValueExpression::Composite(Box::new(Composite::Row(parts)), sf.at);
        let orders = sf.order_by.iter().map(|k| SortOrder::of(k.descending, k.nulls_first)).collect();
        // [SR5a] ABSENT ON NULL if not written.
        let on_null = json.on_null.unwrap_or(OnNull::Absent);
        let func = JsonArrayAggregate { on_null, returning, format: json.format, orders, collations, by_query: false };
        self.sorted(|| self.aggregate(AggregateRef(Arc::new(func)), (Some(&row), false), sf.filter.as_deref(), scope))
    }
}

impl Binder<'_> {
    /// [§10.11 SR1] The type the `<JSON aggregate function>` `function`
    /// of `json` returns, its value `arg`: of its `<JSON output
    /// clause>`; without one, JSON if its value is of the JSON type
    /// \[SR1a\], which `arg` bound by itself, as its rows bind it, shows
    /// (Feature T801); else `IMPLICIT_RETURNING` \[SR1b\].
    ///
    /// # Errors
    ///
    /// As `output_type`.
    fn aggregate_returning(&self, json: &JsonAggregate, arg: Option<&ValueExpression>, function: &str) -> Result<DataType> {
        if json.returning.is_none() {
            let rows = Scope::Rows("[ISO/IEC 9075:1992 §6.5 SR4] a set function argument cannot contain a set function");
            let bound = arg.and_then(|v| self.value_expression(v, rows).ok());
            if matches!(bound, Some((_, DataType::Json))) {
                return Ok(DataType::Json);
            }
        }
        output_type(json.returning.as_ref(), function)
    }
}

/// [§6.34 SR2a, §10.11 SR1a] Whether RETURNING JSON is implicit: no
/// `<JSON output clause>` is `written`, and an input is of a type of
/// `types` that is JSON (Feature T801).
fn implicit_json(written: Option<&JsonOutput>, types: &[&DataType]) -> bool {
    written.is_none() && types.iter().any(|t| matches!(t, DataType::Json))
}

/// The error of a `<JSON aggregate function>` read without its argument.
fn not_read() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] a JSON aggregate function was read without its argument")
}
