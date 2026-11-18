//! [ISO/IEC 9075-2:2023 §11.32 GR1b, GR2b, §9.38, §7.6 GR3b] Referenceable
//! views where a query names one. Its own rows, its original `<query
//! expression>`, are its query's, its self-referencing column first: of a
//! user-defined reference representation, the REF its first column casts
//! its leaf column to; of a derived one, the REF its derivational
//! attributes' values make, IDV \[GR1b i\]. Named without ONLY, it is its
//! hierarchical `<query expression>`: its own rows and, after them, each
//! proper subview's own rows, cut to its columns by name, as UNION ALL
//! CORRESPONDING does \[§9.38 GR3\]; named with ONLY, its own rows alone
//! \[GR2b ii\]. A data change through one is refused (`views/referenceable.rs`).
//!
//! \[DESIGN\] A user-defined reference's octets are its representation
//! type's value as `value::codec` writes it, and a derived one's those of
//! its derivational attributes' values as a row's (UV101), as a typed
//! table's are (`value::types::reference`, RULES.md D81).

use std::sync::Arc;

use super::Context;
use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::views::{Reader, Source};
use crate::ast::{QueryExpression, RefGeneration, SelectList, SelectSublist, ValueExpression};
use crate::catalog::{QualifiedName, RefRepresentation, ViewDesc, owner_of, proper_subtables};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::{Expr, LogicalPlan, SetOp};
use crate::params::Params;
use crate::parser::Parser;
use crate::value::codec::{put_row, put_value};
use crate::value::types::REF_OCTETS;
use crate::value::{Binary, DataType, Value, cast};

/// The name a self-referencing column's value is shown with.
const REFERENCE: &str = "REF";
/// \[DESIGN\] The most casts in one another a self-referencing column's
/// cast is searched through for its leaf column (RULES.md R-CF-6).
const MAX_CASTS: usize = 256;

/// An internal error: a referenceable view not as its definition made it.
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

/// The REF value `octets` make.
///
/// # Errors
///
/// SQLCODE -900 if they are more than `REF_OCTETS`.
fn reference(octets: Vec<u8>) -> Result<Value> {
    if octets.len() > REF_OCTETS {
        let msg = format!("[DESIGN] a reference has at most {REF_OCTETS} octets, and a view row's takes {} (RULES.md D81)", octets.len());
        return Err(SqlError::new(Code::Unsupported, msg));
    }
    Ok(Value::Reference(Binary::new(octets)))
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23s i, §6.13] The REF of a user-defined
/// representation its leaf column's value is cast to: null of null, and a
/// reference of a reference.
#[derive(Debug)]
struct UserReference {
    /// The representation type.
    representation: DataType,
    /// The self-referencing column's type.
    result: DataType,
}

impl ScalarFunction for UserReference {
    fn name(&self) -> &str {
        REFERENCE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(v) = args.first().filter(|v| !matches!(v, Value::Null)) else { return Ok(Value::Null) };
        // A reference already, its own value.
        if let Value::Reference(_) = v {
            return Ok(v.clone());
        }
        let mut octets = Vec::new();
        put_value(&mut octets, &cast(v.clone(), &self.representation)?);
        reference(octets)
    }
}

/// [ISO/IEC 9075-2:2023 §11.32 GR1b i] IDV: the REF of a row of a view of
/// derived references, made of its derivational attributes' values.
#[derive(Debug)]
struct DerivedReference {
    /// The self-referencing column's type.
    result: DataType,
}

impl ScalarFunction for DerivedReference {
    fn name(&self) -> &str {
        REFERENCE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let mut octets = Vec::new();
        put_row(&mut octets, &args.to_vec());
        reference(octets)
    }
}

/// [ISO/IEC 9075-2:2023 §11.32 GR1b, GR1c] The own rows of the
/// referenceable view `view`, its original `<query expression>`: its
/// query, bound as its owner's for `reader`, its self-referencing column's
/// values first.
///
/// # Errors
///
/// Its query breaks a rule now; SQLCODE -910 if it is not as its
/// definition made it.
pub(super) fn own_rows(db: &State, view: &ViewDesc, reader: Reader<'_>) -> Result<LogicalPlan> {
    let typed = view.typed.as_ref().ok_or_else(|| broken(&format!("view {} is not referenceable", view.name)))?;
    let st = db.catalog.structured_type(&typed.udt).ok_or_else(|| broken(&format!("there is no type {}", typed.udt)))?;
    let maximal = db.catalog.structured_type(&st.maximal()).unwrap_or(st);
    let result = view.columns.first().map(|c| c.data_type.clone()).ok_or_else(|| broken("a referenceable view has no columns"))?;
    let mut query = Parser::for_stored(&view.query)?.view_query()?;
    let by_user = matches!(typed.generation, RefGeneration::User);
    let uncast = if by_user { uncast_first(&mut query) } else { false };
    let (first, after) = match (typed.generation, &maximal.reference) {
        (RefGeneration::User, RefRepresentation::User(representation)) => {
            let f = UserReference { representation: representation.clone(), result };
            // [GR1b ii] Of a subview, a first column of a value of its own.
            let value = if uncast { Expr::Call(ScalarRef(Arc::new(f)), vec![Expr::Column(0)]) } else { Expr::Column(0) };
            (value, 1)
        }
        (RefGeneration::Derived, RefRepresentation::Derived(names)) => {
            let at: Option<Vec<Expr>> = names.iter().map(|n| st.attribute(n).map(|(i, _)| Expr::Column(i))).collect();
            let args = at.ok_or_else(|| broken(&format!("type {} lacks a derivational attribute", st.name)))?;
            (Expr::Call(ScalarRef(Arc::new(DerivedReference { result })), args), 0)
        }
        (RefGeneration::User | RefGeneration::Derived | RefGeneration::System, _) => {
            return Err(broken(&format!("view {} is not of its type's reference representation", view.name)));
        }
    };
    let Reader { purpose, own, user, now, .. } = reader;
    let params = Params::new();
    let cx = Context {
        purpose,
        horizon: &view.horizon,
        own,
        user,
        now,
        schema: &view.names,
        ..Context::new(db, owner_of(&*db.catalog, &view.name.auth), &params)
    };
    let bound = bind_query_expression(cx, &query, &Nesting::Query)?;
    let exprs = std::iter::once(first).chain((after..bound.columns.len()).map(Expr::Column)).collect();
    Ok(LogicalPlan::Project { exprs, input: Box::new(bound.plan) })
}

/// Whether `query`, of a view of user-defined references, has a first
/// column that casts its leaf column \[SR23s i 3\], and if so, `query` with
/// that column in place of the cast, whose value `UserReference` makes the
/// REF of.
fn uncast_first(query: &mut QueryExpression) -> bool {
    let QueryExpression::Spec(spec) = query else { return false };
    let SelectList::Values(items) = &mut spec.select_list else { return false };
    let Some(SelectSublist::Derived(item)) = items.first_mut() else { return false };
    if !matches!(item.value, ValueExpression::Cast { .. }) {
        return false;
    }
    let mut leaf = item.value.clone();
    for _ in 0..MAX_CASTS {
        let ValueExpression::Cast { operand: Some(x), .. } = leaf else { break };
        leaf = *x;
    }
    if !matches!(leaf, ValueExpression::Column(_)) {
        return false;
    }
    item.value = leaf;
    true
}

/// [ISO/IEC 9075-2:2023 §11.32 GR2b, §9.38 GR3, §7.6 GR3b] The rows of the
/// table or view `name`, whose own rows come from `source`, named with
/// ONLY if `only`, read as `reader` reads it: of a referenceable view named
/// without ONLY, its own rows and its proper subviews' own rows.
///
/// # Errors
///
/// A subview's query breaks a rule now; SQLCODE -910 if a subview lacks a
/// column of the view.
pub(super) fn hierarchical(db: &State, (name, source): (&QualifiedName, Source), (only, reader): (bool, Reader<'_>)) -> Result<Source> {
    let Some(view) = db.catalog.view(name).filter(|v| v.typed.is_some() && !only) else { return Ok(source) };
    let subviews = proper_subtables(&*db.catalog, name);
    let Source::Derived(own) = &source else { return Ok(source) };
    if subviews.is_empty() {
        return Ok(source);
    }
    let mut plan = own.clone();
    for sub in &subviews {
        let Some(sv) = db.catalog.view(sub) else { continue };
        let rows = own_rows(db, sv, reader)?;
        let position = |c: &str| sv.columns.iter().position(|s| s.name == c).map(Expr::Column);
        let exprs: Option<Vec<Expr>> = view.columns.iter().map(|c| position(&c.name)).collect();
        let exprs = exprs.ok_or_else(|| broken(&format!("subview {sub} lacks a column of {name}")))?;
        let right = LogicalPlan::Project { exprs, input: Box::new(rows) };
        plan = LogicalPlan::SetOp { op: SetOp::Union, all: true, compared: None, left: Box::new(plan), right: Box::new(right) };
    }
    Ok(Source::Derived(plan))
}

/// [ISO/IEC 9075-2:2023 §11.32 SR23j] How many columns `view` has beyond
/// those of its query: of a referenceable view of derived references, its
/// self-referencing column, IDV.
pub(super) fn beyond_query(view: &ViewDesc) -> usize {
    usize::from(view.typed.as_ref().is_some_and(|t| matches!(t.generation, RefGeneration::Derived)))
}

/// \[DESIGN\] That the table or view `name` may be the target of a data
/// change: no referenceable view, which is read-only here (RULES.md D81).
///
/// # Errors
///
/// SQLCODE -900 if it is one.
pub(super) fn writable(db: &State, name: &QualifiedName) -> Result<()> {
    if db.catalog.view(name).is_some_and(|v| v.typed.is_some()) {
        let msg =
            format!("[DESIGN] {name} is a referenceable view, read-only here: its rows are changed through its basis table (RULES.md D81)");
        return Err(SqlError::new(Code::Unsupported, msg));
    }
    Ok(())
}
