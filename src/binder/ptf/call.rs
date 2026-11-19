//! [ISO/IEC 9075-2:2023 §9.18 SR5e, SR10d, SR12e to SR12n] An invocation
//! of a polymorphic table function, bound: its subject routine, its
//! arguments, its pre-compilation and compilation argument lists, its
//! private variables' initial values and its component procedures; and
//! the invocation compiled (`db/ptf/compile.rs`).

use std::collections::BTreeSet;

use super::super::from::Bounds;
use super::super::invocation::{
    bind_procedure_program, constraint_permits, definition_of, functions_named, generation_permits, note_specific,
};
use super::super::named::{all_or_none, arranged};
use super::super::{Binder, Candidate, Context, Purpose, Scope, assignable, bind_value, rule};
use super::Undescribed;
use super::tables::{BoundTable, copartitioned, table_argument};
use crate::ast::{PtfArgument, PtfInvocation, RoutineBody};
use crate::catalog::{GenericParameter, QualifiedName, SqlRoutineDesc, owner_of};
use crate::db::ptf::{Call, DescriptorColumns, Procedure, compile};
use crate::error::{At, Result, SqlError};
use crate::exec::{Runner, Subqueries, eval};
use crate::logical::{Cond, Expr};
use crate::params::{ParameterType, Params};
use crate::storage::Row;
use crate::value::{DataType, Value, assign};

/// An invocation, bound: the call a plan runs, its table arguments, and
/// the expressions of its pre-compilation argument list's values.
pub(super) struct Bound {
    /// The function invoked.
    pub(super) desc: SqlRoutineDesc,
    /// The call.
    pub(super) call: Call,
    /// Its table arguments.
    pub(super) tables: Vec<BoundTable>,
    /// Of each parameter, its pre-compilation argument's value [SR12f].
    pub(super) arguments: Vec<Expr>,
}

/// An exception of [ISO/IEC 9075-2:2023 §9.18] `rule_of`.
fn broken(rule_of: &str, msg: &str) -> SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §9.18 {rule_of}] {msg}"))
}

/// [§9.18 SR5d, SR5h i] The arguments of `inv` in the order of the
/// parameters of `desc`, if they fit them by name.
fn ordered<'a>(desc: &SqlRoutineDesc, inv: &'a PtfInvocation) -> Option<Vec<&'a PtfArgument>> {
    let parameters: Vec<&str> = desc.parameters.iter().map(|p| p.name.as_str()).collect();
    arranged(&parameters, &inv.arguments, &inv.names)
}

/// [§9.18 SR10d ii, iii] Whether `arguments` are of the kinds the
/// parameters of `desc` take: a table argument of each generic table
/// parameter's, a descriptor argument or NULL of each descriptor
/// parameter's, a value or NULL of each other's; of named arguments,
/// each at its parameter's place [SR5d].
fn fits(desc: &SqlRoutineDesc, inv: &PtfInvocation) -> bool {
    let Some(arguments) = ordered(desc, inv) else { return false };
    desc.parameters.len() == arguments.len()
        && desc.parameters.iter().zip(arguments).all(|(p, a)| match (p.generic, a) {
            (Some(GenericParameter::Table(_)), PtfArgument::Table(_))
            | (Some(GenericParameter::Descriptor), PtfArgument::Descriptor(..) | PtfArgument::Null(_))
            | (None, PtfArgument::Value(_) | PtfArgument::Null(_)) => true,
            (Some(GenericParameter::Table(_)), PtfArgument::Value(_) | PtfArgument::Null(_) | PtfArgument::Descriptor(..))
            | (Some(GenericParameter::Descriptor), PtfArgument::Value(_) | PtfArgument::Table(_))
            | (None, PtfArgument::Table(_) | PtfArgument::Descriptor(..)) => false,
        })
}

/// [ISO/IEC 9075-2:2023 §7.6 SR7a, §9.18 SR5e, SR6, SR10] The subject
/// routine of `inv`: of the polymorphic table functions of its name in
/// the first schema of the SQL-path that has one its arguments fit, the
/// one alone, which the statement's authorization identifier holds
/// EXECUTE on; of a definition bound again, one of its subject routines.
fn subject(cx: Context<'_>, inv: &PtfInvocation) -> Result<SqlRoutineDesc> {
    all_or_none(&inv.names)?;
    let path: Vec<String> = match &inv.schema {
        Some(s) => vec![s.clone()],
        None if !cx.params.routine_path().is_empty() => cx.params.routine_path().to_vec(),
        None => vec![cx.schema.to_owned()],
    };
    let fixed = if cx.params.horizoned() { cx.params.horizon() } else { cx.horizon }.subjects();
    let shown = inv.schema.as_ref().map_or_else(|| inv.name.clone(), |s| format!("{s}.{}", inv.name));
    let mut any = false;
    for schema in &path {
        let named = functions_named(cx.db, &QualifiedName { auth: schema.clone(), ident: inv.name.clone() });
        let ptfs: Vec<&SqlRoutineDesc> = named
            .into_iter()
            .filter_map(|c| if let Candidate::Sql(r) = c { Some(r) } else { None })
            .filter(|r| r.polymorphic() && fixed.is_none_or(|f| f.contains(&r.specific)))
            .collect();
        any = any || !ptfs.is_empty();
        let fitting: Vec<&SqlRoutineDesc> = ptfs.into_iter().filter(|r| fits(r, inv)).collect();
        let held: Vec<&SqlRoutineDesc> =
            fitting.iter().copied().filter(|r| fixed.is_some() || Candidate::Sql(r).executable(cx.db, cx.who())).collect();
        match (held.as_slice(), fitting.is_empty()) {
            ([one], _) => return Ok((*one).clone()),
            ([], true) => {}
            ([], false) => {
                let msg = format!("[ISO/IEC 9075-2:2023 §9.18 SR6] {} holds EXECUTE on no polymorphic table function {shown}", cx.auth);
                return Err(SqlError::new(crate::error::Code::Privilege, msg));
            }
            ([_, _, ..], _) => {
                return Err(broken("SR10h iv 3", &format!("more than one polymorphic table function {shown} takes these arguments")));
            }
        }
    }
    let msg = if any {
        format!("no polymorphic table function {shown} takes these {} arguments", inv.arguments.len())
    } else {
        format!("there is no polymorphic table function {shown}")
    };
    Err(broken(if any { "SR10d" } else { "SR5e" }, &msg))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12g iii] (IA062) Whether `e` is evaluated
/// as the invocation is compiled: of no column, outer reference, dynamic
/// parameter, SQL parameter or variable, subquery, invocation of an
/// SQL-invoked function nor draw of a sequence generator, so it reads no
/// SQL-data.
fn compile_time(e: &Expr) -> bool {
    let own = match e {
        Expr::Column(_) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_) => false,
        Expr::Truth(c) => compile_time_cond(c),
        Expr::Case(whens, _) => whens.iter().all(|(c, _)| compile_time_cond(c)),
        Expr::Const(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Warned(..) => true,
    };
    let mut children = true;
    e.for_each_child(&mut |x| children = children && compile_time(x));
    own && children
}

/// `compile_time`, of the condition `c`: of no subquery, its expressions
/// all evaluated as the invocation is compiled.
fn compile_time_cond(c: &Cond) -> bool {
    let own = match c {
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(..) | Cond::Match { .. } => false,
        Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Not(_)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::Distinct(..)
        | Cond::Like(..)
        | Cond::Is(..)
        | Cond::Value(_)
        | Cond::In(..) => true,
    };
    let mut exprs = true;
    c.for_each_expr(&mut |x| exprs = exprs && compile_time(x));
    own && exprs
}

/// What a pre-compilation argument and a compilation argument are of a
/// parameter: the expression of its value as the statement runs, and its
/// value as the invocation is compiled.
type Argued = (Expr, Value);

/// [§9.18 SR12e to SR12h] The argument `argument`, of a data parameter of
/// type `declared` of `desc`, bound in `b`: its value as the statement
/// runs and, if it is evaluated as the invocation is compiled, then, else
/// null.
fn data_argument(b: Bounds<'_>, (desc, name, declared): (&SqlRoutineDesc, &str, &DataType), argument: &PtfArgument) -> Result<Argued> {
    let PtfArgument::Value(value) = argument else { return Ok((Expr::Const(Value::Null), Value::Null)) };
    let binder = Binder::new(b.cx, b.outer, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §10.4] an SQL argument of a polymorphic table function has no set function");
    let (expr, found) = binder.typed(value, ParameterType::Of(declared.clone()), scope)?;
    if !assignable(declared, &found) {
        let msg = format!(
            "parameter {name} of polymorphic table function {} is of {declared}, which an argument of {found} cannot be stored in",
            desc.name
        );
        return Err(broken("SR10l iii", &msg));
    }
    let compiled = if compile_time(&expr) {
        assign(eval(&expr, &Row::new(), &Undescribed)?, declared, "ISO/IEC 9075-2:2023 §9.18 GR4c i")?
    } else {
        Value::Null
    };
    Ok((expr, compiled))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12, §9.24] The invocation `inv`, bound in
/// `b`, nested `depth` deep: its subject routine, which what it is bound
/// for may invoke; its arguments; and the invocation compiled.
///
/// # Errors
///
/// SQLCODE -103 if it has no one subject routine, an argument breaks a
/// Syntax Rule of SR10 or SR12, or its compilation does; -104 if the
/// statement's authorization identifier holds no EXECUTE on it.
pub(super) fn bound(b: Bounds<'_>, inv: &PtfInvocation, depth: usize) -> Result<Bound> {
    let cx = b.cx;
    let desc = subject(cx, inv).or_at(inv.at)?;
    match cx.purpose {
        Purpose::Query | Purpose::Mapped => {}
        Purpose::Constraint | Purpose::UnderConstraint => constraint_permits(&desc).or_at(inv.at)?,
        Purpose::Generation | Purpose::Key => generation_permits(&desc).or_at(inv.at)?,
    }
    note_specific(cx.invoked, &desc.specific);
    let mut tables = Vec::new();
    let mut descriptors = Vec::with_capacity(desc.parameters.len());
    let (mut arguments, mut compiled) = (Vec::with_capacity(desc.parameters.len()), Vec::with_capacity(desc.parameters.len()));
    for (p, a) in desc.parameters.iter().zip(ordered(&desc, inv).unwrap_or_default()) {
        let mut described = None;
        let (e, v) = match (p.generic, a) {
            (Some(GenericParameter::Table(g)), PtfArgument::Table(ta)) => {
                tables.push(table_argument(b, ta, g, depth)?);
                (Expr::Const(Value::Null), Value::Null)
            }
            (Some(GenericParameter::Descriptor), PtfArgument::Descriptor(Some(columns), _)) => {
                let mut resolved = Vec::with_capacity(columns.len());
                for c in columns {
                    let t = c
                        .data_type
                        .as_ref()
                        .map(|t| cx.db.resolve_type(t, (cx.schema, cx.who())).map(|(t, _)| t))
                        .transpose()
                        .or_at(c.at)?;
                    resolved.push((c.name.clone(), t));
                }
                described = Some(resolved);
                (Expr::Const(Value::Char(String::new())), Value::Char(String::new()))
            }
            (Some(_), _) => (Expr::Const(Value::Null), Value::Null),
            (None, a) => data_argument(b, (&desc, &p.name, &p.data_type), a)?,
        };
        descriptors.push(described);
        arguments.push(e);
        compiled.push(v);
    }
    copartitioned(&mut tables, &inv.copartition, cx.schema).or_at(inv.at)?;
    let call = called(cx, &desc, (&tables, &descriptors, &compiled)).or_at(inv.at)?;
    Ok(Bound { desc, call, tables, arguments })
}

/// [ISO/IEC 9075-2:2023 §9.24] The invocation of `desc` compiled in `cx`,
/// of its table arguments, descriptor arguments and compilation argument
/// list: its private variables' initial values found, its component
/// procedures bound, its describe component procedure run.
fn called(
    cx: Context<'_>,
    desc: &SqlRoutineDesc,
    (tables, descriptors, compiled): (&[BoundTable], &[Option<DescriptorColumns>], &[Value]),
) -> Result<Call> {
    let ptf =
        desc.ptf.as_deref().ok_or_else(|| rule("[ISO/IEC 9075-2:2023 §7.6 SR7a] the subject routine is a polymorphic table function"))?;
    let runner = cx.compile.map(|c| Runner::new(&c.db, c.tx));
    let subqueries: &dyn Subqueries = match &runner {
        Some(r) => r,
        None => &Undescribed,
    };
    let private = private_values(cx, desc, subqueries)?;
    let procedure = |specific: &QualifiedName| -> Result<Procedure> {
        let found = cx
            .db
            .catalog
            .routine(specific)
            .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §11.60 SR10h] there is no procedure {specific}")))?;
        let body_cx = Context { uses: None, selected: None, referencing: None, drawn: None, ..cx };
        let program = bind_procedure_program(body_cx, found)?;
        Ok(Procedure { program, types: found.parameter_types() })
    };
    let describe = ptf.describe.as_ref().map(procedure).transpose()?;
    let inputs: Vec<_> = tables.iter().map(|t| t.input.clone()).collect();
    let made =
        compile(desc, (&inputs, descriptors, compiled), (private, cx.db.catalog_name()), describe.as_ref().map(|p| (p, subqueries)))?;
    Ok(Call {
        name: desc.name.clone(),
        compiled: made,
        tables: inputs,
        start: ptf.start.as_ref().map(procedure).transpose()?,
        fulfill: procedure(&ptf.fulfill)?,
        finish: ptf.finish.as_ref().map(procedure).transpose()?,
    })
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2a] The initial values of the private
/// variables of `desc`: each one's default, evaluated as the owner of its
/// schema binds it where `subqueries` runs a query's subqueries, else
/// null.
fn private_values(cx: Context<'_>, desc: &SqlRoutineDesc, subqueries: &dyn Subqueries) -> Result<Vec<Value>> {
    let definition = definition_of(desc)?;
    let RoutineBody::Polymorphic(def) = &definition.body else { return Ok(Vec::new()) };
    let owner = owner_of(&*cx.db.catalog, &desc.name.auth).to_owned();
    let params = Params::default();
    let types = desc.ptf.as_ref().map(|p| p.private.iter().map(|q| q.data_type.clone()).collect::<Vec<_>>()).unwrap_or_default();
    let mut out = Vec::with_capacity(def.private.len());
    for (p, t) in def.private.iter().zip(&types) {
        let value = match &p.default {
            Some((Some(v), _)) => {
                let default_cx = Context { schema: &desc.schema, compile: cx.compile, ..Context::new(cx.db, &owner, &params) };
                let (e, _) = bind_value(default_cx, v)?;
                eval(&e, &Row::new(), subqueries)?
            }
            Some((None, _)) | None => Value::Null,
        };
        out.push(assign(value, t, "ISO/IEC 9075-2:2023 §11.60 SR10e iii, §9.2")?);
    }
    Ok(out)
}
