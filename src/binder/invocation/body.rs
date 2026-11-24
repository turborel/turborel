//! [ISO/IEC 9075-2:2023 §11.60, §16.2, §6.6 SR8] The SQL routine body of
//! an SQL-invoked function, bound: its `<return statement>`'s value as a
//! subquery over the row of the function's parameters (RULES.md D73).
//!
//! The body is bound as the owner of the routine's schema binds it, its
//! routine authorization identifier [§11.60 GR3u], with the names it
//! writes with no schema of the schema it was defined with, and no
//! statement parameter: its parameters are the columns of the one row of
//! a FROM clause enclosing it, named by the routine's name as correlation
//! name. So a parameter `P` is referenced as `P` or `F.P` [§6.6 SR8], and
//! a column of a table of the body's own FROM clause hides it, the most
//! local scope first [§6.6 SR4]. Each reference is an outer reference
//! of the body's subquery, whose outer expression is the parameter's
//! column of that row.
//!
//! A body that is a compound statement, or another SQL control statement,
//! is bound as a program (`binder/psm.rs`) whose frame's first slots are
//! the parameters' values, its CALLs bound with it, each procedure's body a
//! program of its own bound as the owner of the procedure's schema binds
//! it [ISO/IEC 9075-4:2006 draft §11.2].

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::from::FromItem;
use super::super::psm::{Frame, bind_program};
use super::super::subquery::Use;
use super::super::views::Source;
use super::super::{Binder, Context, Invoked, Purpose, Scope, assignable, rule};
use crate::ast::{
    self, QueryExpression, ReturnStatement, RoutineDefinition, RoutineKind, RowElement, SchemaStatement, SqlStatement, TableName, TypeName,
    ValueExpression, ValuesRow,
};
use crate::catalog::{
    ColumnDesc, DataAccess, Principal, QualifiedName, SqlRoutineDesc, TableDesc, Who, module_schema, owner_of, routine_path,
};
use crate::debugger::{Kind, Origin};
use crate::error::{Code, Result, SqlError};
use crate::logical::{LogicalPlan, Program, Subquery};
use crate::params::{Params, RoutineScope};
use crate::parser::{MAX_NESTING_DEPTH, Parser};
use crate::value::DataType;
use crate::value::precedence::in_precedence_list;

/// \[DESIGN\] What a function's body is bound as: its value, and what it
/// possibly does with SQL-data.
#[derive(Debug, Clone)]
pub struct FunctionBody {
    /// The value of its `<return statement>`: a subquery of one row and one
    /// column over the row of the function's parameters, each parameter's
    /// outer expression `Expr::Column` of its ordinal position. None of a
    /// body that is no `<return statement>`, which returns no value
    /// [§9.18 GR8b ix].
    pub body: Option<Arc<Subquery>>,
    /// [ISO/IEC 9075-4:2006 draft §11.2] Of a body that is an SQL control
    /// statement, its program.
    pub(crate) program: Option<Arc<Program>>,
    /// What the body possibly does: reads SQL-data if it reads a table or
    /// invokes a routine that possibly reads SQL-data; modifies it if it
    /// invokes one that possibly modifies it; else neither.
    pub access: DataAccess,
}

/// The `<SQL-invoked routine>` a routine descriptor keeps as `text`, read
/// again, to bind its body.
///
/// # Errors
///
/// It does not parse, or is no `<SQL-invoked routine>`.
pub fn routine_definition(text: &str, module: Option<String>) -> Result<RoutineDefinition> {
    let parser = Parser::for_stored(text)?.in_module(module);
    let SqlStatement::Schema { statement, .. } = { parser }.sql_statement()? else {
        return Err(SqlError::new(Code::Api, "[DESIGN] a routine's text is no <SQL-invoked routine>"));
    };
    if let SchemaStatement::Routine(r) = statement {
        return Ok(*r);
    }
    let SchemaStatement::Method(m) = statement else {
        return Err(SqlError::new(Code::Api, "[DESIGN] a routine's text is no <SQL-invoked routine>"));
    };
    // [ISO/IEC 9075-2:2023 §11.61] a method's body, its parameters and
    // result the descriptor's, SELF first.
    Ok(RoutineDefinition {
        kind: RoutineKind::Function,
        name: TableName { auth: None, ident: String::new(), at: m.at },
        parameters: Vec::new(),
        returns: None,
        specific: None,
        deterministic: false,
        access: DataAccess::Contains,
        null_call: false,
        new_savepoint_level: false,
        body: m.body,
        text: m.text,
        body_text: m.body_text,
        distance: None,
    })
}

/// The `<SQL-invoked routine>` of the routine `desc`, read again from its
/// text: of a module's, in the module, so MODULE.T names its declared
/// tables [ISO/IEC 9075-4:2006 draft §12.8 GR3b].
///
/// # Errors
///
/// As `routine_definition`'s.
pub fn definition_of(desc: &SqlRoutineDesc) -> Result<RoutineDefinition> {
    let module = desc.module.as_ref().map(|m| module_schema(&QualifiedName { auth: desc.name.auth.clone(), ident: m.clone() }));
    let mut definition = routine_definition(&desc.text, module)?;
    // [§16.2 SR2] RETURN NULL of a method is of its descriptor's result type.
    if desc.method.is_some() {
        definition.returns = desc.returns.clone().map(TypeName::Data);
    }
    Ok(definition)
}

/// The `<SQL procedure statement>` of the body of `definition`.
///
/// # Errors
///
/// SQLCODE -103 if it is a polymorphic table function's, which is invoked
/// in a `<PTF derived table>` alone [ISO/IEC 9075-2:2023 §9.18 SR12a].
pub fn sql_statement(definition: &RoutineDefinition) -> Result<&SqlStatement> {
    definition.body.statement().ok_or_else(|| {
        SqlError::new(
            Code::SyntaxRule,
            "[ISO/IEC 9075-2:2023 §9.18 SR12a] a polymorphic table function is invoked in a <PTF derived table> alone",
        )
    })
}

/// [ISO/IEC 9075-2:2023 §11.60, §16.2] The body `definition` holds of the
/// function `desc`, bound as the owner of its schema binds it, with the
/// database, instant and settings of `cx`; the routines it invokes noted
/// where `cx` notes them, and the columns it references where `cx` notes
/// those.
///
/// # Errors
///
/// SQLCODE -103 if its value is not assignable to the result type
/// [§16.2 SR4], or does not bind.
pub fn bind_function_body(cx: Context<'_>, desc: &SqlRoutineDesc, definition: &RoutineDefinition) -> Result<FunctionBody> {
    let (bound, invoked) = bind_body(cx, (desc, definition), chain_of(cx.invoked))?;
    if let Some(i) = cx.invoked {
        i.borrow_mut().extend(invoked);
    }
    Ok(bound)
}

/// The specific names of the functions whose bodies are being bound, the
/// outermost first: what `invoked` holds before its entry of no schema,
/// which marks where the routines a body invokes begin. None if it has
/// none, as a statement's own list has not.
pub(super) fn chain_of(invoked: Option<&Invoked>) -> Vec<QualifiedName> {
    let Some(list) = invoked.map(Invoked::borrow) else { return Vec::new() };
    let end = list.iter().position(|q| q.auth.is_empty()).unwrap_or(0);
    list.get(..end).unwrap_or_default().to_vec()
}

/// `bind_function_body`, within the bodies of the functions of `chain`;
/// with the specific names of the routines the body invokes.
pub(super) fn bind_body(
    cx: Context<'_>,
    (desc, definition): (&SqlRoutineDesc, &RoutineDefinition),
    chain: Vec<QualifiedName>,
) -> Result<(FunctionBody, Vec<QualifiedName>)> {
    let ((body, program), invoked) = within_body(cx, desc, chain, |body_cx| {
        let stmt = sql_statement(definition)?;
        if let (SqlStatement::Return(r), false) = (stmt, desc.access == DataAccess::Modifies) {
            return Ok((Some(returned(body_cx, (desc, definition), r)?), None));
        }
        // One that possibly modifies SQL-data is run by the session, its
        // RETURN a program's (`exec/invoked.rs`).
        if matches!(stmt, SqlStatement::Control(_) | SqlStatement::Signal(_) | SqlStatement::Return(_)) {
            let path = routine_path(&*body_cx.db.catalog, desc);
            return Ok((None, Some(Arc::new(bind_program(body_cx, stmt, inline_frame(desc, path))?))));
        }
        // [§9.18 GR8b ix] any other statement returns no value: raised as
        // the function is invoked, not as it is defined.
        Ok((None, None))
    })?;
    let access = access_of(cx, (body.as_deref(), program.as_deref()), &invoked);
    Ok((FunctionBody { body, program, access }, invoked))
}

/// [ISO/IEC 9075-2:2023 §16.1, §9.18] The body of the procedure `desc`,
/// which a CALL in the body of a function invokes, bound as a program in
/// `cx`, the body's, as the owner of its schema binds it; the routines it
/// invokes noted where `cx` notes them.
///
/// # Errors
///
/// SQLCODE -103 if it invokes itself through the bodies it invokes; its
/// own, bound.
pub(crate) fn bind_procedure_program(cx: Context<'_>, desc: &SqlRoutineDesc) -> Result<Arc<Program>> {
    let definition = definition_of(desc)?;
    let (program, invoked) = within_body(cx, desc, chain_of(cx.invoked), |body_cx| {
        let path = routine_path(&*body_cx.db.catalog, desc);
        bind_program(body_cx, sql_statement(&definition)?, inline_frame(desc, path))
    })?;
    if let Some(i) = cx.invoked {
        i.borrow_mut().extend(invoked);
    }
    Ok(Arc::new(program))
}

/// [ISO/IEC 9075-4:2006 draft §11.2] What the body of `desc`, an SQL control
/// statement, is bound over: its parameters, its CALLs bound with it.
fn inline_frame(desc: &SqlRoutineDesc, path: Vec<String>) -> Frame {
    let scope = RoutineScope::of(desc, path);
    let kind = if desc.function() { Kind::Function } else { Kind::Procedure };
    let origin = Some(Origin { kind, name: desc.specific.clone(), shown: desc.name.clone() });
    Frame { base: desc.parameters.len(), scope: Arc::new(scope), transitions: None, returns: desc.returns.clone(), inline: true, origin }
}

/// `f` of the context the body of `desc` is bound in, within the bodies of
/// the routines of `chain`, as the owner of its schema binds it; with the
/// specific names of the routines the body invokes; the registered XML
/// Schemas it indicates noted where `cx` notes them.
fn within_body<T>(
    cx: Context<'_>,
    desc: &SqlRoutineDesc,
    mut chain: Vec<QualifiedName>,
    f: impl FnOnce(Context<'_>) -> Result<T>,
) -> Result<(T, Vec<QualifiedName>)> {
    if chain.contains(&desc.specific) {
        return Err(rule(format!("[DESIGN] routine {} invokes itself, through the routines its body invokes", desc.name)));
    }
    if chain.len() >= MAX_NESTING_DEPTH {
        let msg = format!("[DESIGN] the bodies of routines invoking routines nest at most {MAX_NESTING_DEPTH} deep");
        return Err(SqlError::new(Code::Unsupported, msg));
    }
    chain.push(desc.specific.clone());
    let depth = chain.len();
    // The entry of no schema that ends the chain (`chain_of`).
    chain.push(QualifiedName { auth: String::new(), ident: String::new() });
    let noted = Invoked::new(chain);
    let params = Params::new().with_path(routine_path(&*cx.db.catalog, desc));
    let owner = owner_of(&*cx.db.catalog, &desc.name.auth);
    let body_cx = Context {
        auth: owner,
        role: None,
        // [ISO/IEC 9075-2:2023 §9.18 GR6l ii 1] the cell of its routine
        // authorization identifier alone; the SQL-session's user stays.
        user: Principal { cell: Who::sole(cx.db.catalog.roles(), owner), session: cx.user.session },
        schema: &desc.schema,
        params: &params,
        // [EXTENSION] A routine a mapping of a schema or the catalog
        // invokes maps none (RULES.md D106).
        purpose: if cx.purpose == Purpose::Mapped { Purpose::Mapped } else { Purpose::Query },
        collation: None,
        horizon: &desc.horizon,
        definitions: false,
        selected: None,
        referencing: None,
        draws: false,
        queries: None,
        invoked: Some(&noted),
        ..cx
    };
    let done = f(body_cx)?;
    // [ISO/IEC 9075-14:2006 draft §4.2.6] The registered XML Schemas the
    // body indicates, and the XQuery modules it imports, are its
    // definition's, as it is bound with them.
    if let Some(outer) = cx.invoked {
        let indicated = noted.take_indicated();
        indicated.xml_schemas.iter().for_each(|name| outer.indicated(name));
        indicated.xquery_modules.iter().for_each(|name| outer.imported(name));
    }
    let invoked = noted.into_inner().split_off(depth.saturating_add(1));
    Ok((done, invoked))
}

/// [ISO/IEC 9075-2:2023 §16.2] The value of the `<return statement>` `r`
/// of the body of `desc`, bound in `cx` as `(VALUES (v))`, a subquery of
/// the row of the parameters.
fn returned(cx: Context<'_>, (desc, definition): (&SqlRoutineDesc, &RoutineDefinition), r: &ReturnStatement) -> Result<Arc<Subquery>> {
    let Some(result) = &desc.returns else {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §16.2 SR1] {} is no function: it has no <returns data type>", desc.name)));
    };
    // [SR2] RETURN NULL is CAST (NULL AS RDT).
    let value = match (&r.value, &definition.returns) {
        (Some(v), _) => v.clone(),
        (None, Some(rdt)) => ValueExpression::Cast { operand: None, target: rdt.clone(), format: None, passing: None, at: r.at },
        (None, None) => return Err(rule(format!("[ISO/IEC 9075-2:2023 §16.2 SR1] {} has no <returns data type>", desc.name))),
    };
    let item = parameters_item(desc);
    let names = item.row_names();
    let visible = (0..desc.parameters.len()).map(|k| (0, k)).collect();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    let query = QueryExpression::Values(vec![ValuesRow { elements: vec![RowElement::Value(value, r.at)], at: r.at }]);
    let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §16.2] a <return value> is of one row, so it contains no set function of the parameters");
    let (body, types) = binder.subquery(&ast::Subquery { query, at: r.at }, scope, Use::Rows)?;
    // [SR4] its value is stored in an item of the result type.
    let fits = |t: &DataType| in_precedence_list(result, t) && assignable(result, t);
    match types.as_slice() {
        [t] if fits(t) => Ok(body),
        // [ISO/IEC 9075-2:2023 §9.2 GR2] by an appropriate cast.
        [t] => binder
            .assigned_query(&body, std::slice::from_ref(t), &[result])?
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §16.2 SR4] function {} returns {result}, not {t}", desc.name))),
        _ => Err(rule("[ISO/IEC 9075-2:2023 §16.2 SR3] a <return value> is one value")),
    }
}

/// [ISO/IEC 9075-2:2023 §6.6 SR8] The row of the parameters of `desc`, as
/// a table of no schema whose correlation name is the routine's name: a
/// column of each parameter, in order, of its declared type.
fn parameters_item(desc: &SqlRoutineDesc) -> FromItem {
    let columns: Vec<ColumnDesc> = desc
        .parameters
        .iter()
        .map(|p| ColumnDesc {
            name: p.name.clone(),
            data_type: p.data_type.clone(),
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        })
        .collect();
    let types = columns.iter().map(|c| c.data_type.clone()).collect();
    let name = QualifiedName { auth: String::new(), ident: desc.name.ident.clone() };
    let table = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    // Its rows are never read: the values of the parameters come as the
    // outer values of the body, as each invocation runs it.
    let source = Source::Derived(LogicalPlan::Values { rows: Vec::new(), types });
    FromItem { correlation: Some(desc.name.ident.clone()), ..FromItem::alone(name, 0, (table, source)) }
}

/// [ISO/IEC 9075-2:2023 §4.35.2] What a body of value `body`, or of the
/// program `program`, invoking the routines `invoked`, possibly does with
/// SQL-data, of the database of `cx`.
fn access_of(cx: Context<'_>, (body, program): (Option<&Subquery>, Option<&Program>), invoked: &[QualifiedName]) -> DataAccess {
    let accesses: Vec<DataAccess> = invoked.iter().filter_map(|s| cx.db.catalog.routine(s)).map(|r| r.access).collect();
    if accesses.contains(&DataAccess::Modifies) || program.is_some_and(|p| p.contained().iter().any(|c| modifies(c.statement()))) {
        return DataAccess::Modifies;
    }
    let leaves_read = program.is_some_and(|p| p.all_leaves().iter().any(|l| reads(&l.query.plan)));
    if accesses.contains(&DataAccess::Reads) || body.is_some_and(|b| reads(&b.plan)) || leaves_read {
        return DataAccess::Reads;
    }
    DataAccess::Contains
}

/// Whether the statement `s` of a program changes SQL-data.
fn modifies(s: &SqlStatement) -> bool {
    matches!(
        s,
        SqlStatement::Insert(_)
            | SqlStatement::Merge(_)
            | SqlStatement::UpdateSearched { .. }
            | SqlStatement::DeleteSearched { .. }
            | SqlStatement::UpdatePositioned { .. }
            | SqlStatement::DeletePositioned { .. }
    )
}

/// Whether `plan` reads a base table or a system table, at any node or
/// subquery. A worklist, not recursion, so no depth bounds it.
fn reads(plan: &LogicalPlan) -> bool {
    let mut work = vec![plan];
    while let Some(p) = work.pop() {
        if matches!(p, LogicalPlan::Scan { .. } | LogicalPlan::System { .. }) {
            return true;
        }
        work.extend(p.inputs());
        work.extend(p.subqueries().into_iter().map(|s| &s.plan));
    }
    false
}
