//! [ISO/IEC 9075:1992 §7.10] `<query expression>`, bound: a `<query
//! specification>` (`binder/query.rs`), a `<table value constructor>`
//! [§7.2], or a set operation on two, whose operands' columns correspond by
//! position or by name and are brought to the result's types [§9.3]. Split
//! from binder.rs (RULES.md R-SIZE-2).

use std::collections::BTreeSet;

use super::case::set_type;
use super::datetime::{stored, takes_zone};
use super::expression::dynamic_of;
use super::query::{Nesting, bind_query};
use super::statements::{INSERT_RULE, check_assignment};
use super::{Binder, Bound, Context, OutColumn, Scope, nondeterministic, rule};
use crate::ast::{Correspondence, QueryExpression, RowElement, SetOperation, SetOperator, ValueExpression, ValuesRow};
use crate::catalog::ColumnDesc;
use crate::error::{At, Result};
use crate::logical::{Expr, LogicalPlan, SetOp};
use crate::params::ParameterType;
use crate::value::{DataType, Value};

/// An object column of an INSERT [X3.135-1986 §8.7 SR4]: what NULL and
/// DEFAULT in its `<table value constructor>` are for [ISO/IEC 9075:1992
/// §7.1 SR1, GR2].
#[derive(Debug, Clone)]
pub struct ObjectColumn {
    /// The column, of the table or view named.
    pub desc: ColumnDesc,
    /// Its default value [ISO/IEC 9075:1992 §11.5].
    pub default: Value,
}

/// [ISO/IEC 9075:1992 §7.10] A `<query expression>`, bound as `nesting`
/// says.
pub(super) fn bind_query_expression(cx: Context<'_>, q: &QueryExpression, nesting: &Nesting<'_>) -> Result<Bound> {
    match q {
        QueryExpression::Spec(spec) => bind_query(cx, spec, &[], nesting),
        QueryExpression::Values(rows) => values(cx, rows, nesting),
        QueryExpression::SetOp(s) => set_operation(cx, s, nesting).or_at(s.at),
    }
}

/// [X3.135-1986 §8.7; ISO/IEC 9075:1992 §13.8] The `<query expression>` of
/// an INSERT into `columns`, the object columns. SQL-86's rule that it not
/// read the table inserted into [X3.135-1986 §8.7 SR2] is Intermediate
/// SQL's [ISO/IEC 9075:1992 §13.8, Leveling Rules]: it is evaluated before
/// any row is inserted \[GR3\].
///
/// # Errors
///
/// It breaks a rule of its query.
pub fn bind_insert_query(cx: Context<'_>, query: &QueryExpression, columns: &[ObjectColumn]) -> Result<Bound> {
    let now = cx.now;
    let bound = bind_query_expression(cx, query, &Nesting::Source { columns })?;
    // A time or timestamp without time zone, of the session's displacement
    // as a column WITH TIME ZONE takes it [§6.1 SR5].
    let wanted =
        columns.len() == bound.columns.len() && bound.columns.iter().zip(columns).any(|(c, o)| takes_zone(c.data_type, o.desc.data_type));
    if !wanted {
        return Ok(bound);
    }
    let exprs = bound
        .columns
        .iter()
        .zip(columns)
        .enumerate()
        .map(|(i, (c, o))| stored(Expr::Column(i), c.data_type, o.desc.data_type, now))
        .collect();
    Ok(Bound { plan: LogicalPlan::Project { exprs, input: Box::new(bound.plan) }, columns: bound.columns })
}

/// The type of each element of a row of a `<table value constructor>`, or
/// none for NULL or DEFAULT, which take the type of their object column.
type BoundRow = Vec<(Expr, Option<DataType>)>;

/// [ISO/IEC 9075:1992 §7.2] A `<table value constructor>`: its rows, of one
/// degree \[SR1\], each column of the type §9.3 gives its values, as the
/// UNION ALL of its rows \[GR2\]. Its columns are unnamed [§7.10 SR5c].
fn values(cx: Context<'_>, rows: &[ValuesRow], nesting: &Nesting<'_>) -> Result<Bound> {
    let (outer, objects) = match nesting {
        Nesting::Query => (None, None),
        Nesting::Source { columns } => (None, Some(*columns)),
        Nesting::Subquery { outer, .. } => (Some(*outer), None),
    };
    let binder = Binder::new(cx, outer, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
    let taken = binder.dynamic_types(rows, objects)?;
    let mut bound: Vec<BoundRow> = Vec::new();
    for row in rows {
        let elements = binder.values_row(row, (objects, &taken)).or_at(row.at)?;
        if let Some(first) = bound.first().filter(|first| first.len() != elements.len()) {
            let (n, m) = (first.len(), elements.len());
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §7.2 SR1] the rows of a <table value constructor> are of one degree, not {n} and {m}"
            ))
            .or_at(row.at));
        }
        bound.push(elements);
    }
    let degree = bound.first().map_or(0, Vec::len);
    let mut columns = Vec::new();
    for k in 0..degree {
        let mut types: Vec<DataType> = bound.iter().filter_map(|r| r.get(k).and_then(|e| e.1)).collect();
        types.extend(objects.and_then(|o| o.get(k)).map(|o| o.desc.data_type));
        let data_type = set_type(&types, "§7.2 GR2, §7.10 SR12b ii")?;
        columns.push(OutColumn { name: None, data_type, not_null: false, origin: None });
    }
    let exprs = bound.into_iter().map(|r| r.into_iter().zip(&columns).map(|((e, t), c)| conformed(e, t, c.data_type)).collect()).collect();
    Ok(Bound { plan: LogicalPlan::Values { rows: exprs, width: degree }, columns })
}

/// `e`, of type `t` if it has one, cast to `to` if that is another.
fn conformed(e: Expr, t: Option<DataType>, to: DataType) -> Expr {
    match t {
        Some(t) => super::case::brought(e, t, to),
        None => e,
    }
}

/// The first value of column `k` of `rows` that is not a `<dynamic
/// parameter specification>`, if one is: NULL and DEFAULT, of no type, are
/// passed over.
fn deciding(rows: &[ValuesRow], k: usize) -> Option<&ValueExpression> {
    for row in rows {
        if let Some(RowElement::Value(v, _)) = row.elements.get(k)
            && dynamic_of(v).is_none()
        {
            return Some(v);
        }
    }
    None
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §17.6 GR5o, p] The type a `<dynamic parameter
    /// specification>` in each column of `rows` takes: the first row's whose
    /// value there is not one; or, if each is, the column's an INSERT
    /// inserts into, of `objects`; else none \[GR4j\].
    fn dynamic_types(&self, rows: &[ValuesRow], objects: Option<&[ObjectColumn]>) -> Result<Vec<Option<DataType>>> {
        let degree = rows.iter().map(|r| r.elements.len()).max().unwrap_or(0);
        let scope = Scope::Rows("[X3.135-1986 §5.8 SR1] a set function needs an argument source");
        let mut taken = Vec::new();
        for k in 0..degree {
            // Only a column with a `?` in it needs a type for one.
            let any = rows.iter().any(|r| match r.elements.get(k) {
                Some(RowElement::Value(v, _)) => dynamic_of(v).is_some(),
                Some(RowElement::Null(_) | RowElement::Default(_)) | None => false,
            });
            if !any {
                taken.push(None);
                continue;
            }
            let by_row = deciding(rows, k).map(|v| self.value_expression(v, scope)).transpose()?.map(|(_, t)| t);
            taken.push(by_row.or_else(|| objects.and_then(|o| o.get(k)).map(|o| o.desc.data_type)));
        }
        Ok(taken)
    }

    /// [ISO/IEC 9075:1992 §7.1] A `<row value constructor>` of a `<table
    /// value constructor>`: its elements, a `<row subquery>` giving one for
    /// each of its columns. NULL and DEFAULT are only of the rows inserted
    /// into `objects` \[SR1\], each its column's default \[GR2\], and a value
    /// for an object column must be one it can take [X3.135-1986 §8.7 SR6].
    fn values_row(&self, row: &ValuesRow, (objects, taken): (Option<&[ObjectColumn]>, &[Option<DataType>])) -> Result<BoundRow> {
        let scope =
            Scope::Rows("[X3.135-1986 §5.8 SR1] a set function needs an argument source, which a <table value constructor> does not have");
        if let [RowElement::Value(v @ ValueExpression::Subquery(_), at)] = row.elements.as_slice() {
            let values = self.row(&vec![v.clone()], scope).or_at(*at)?;
            return Ok(values.into_iter().map(|(e, t)| (e, Some(t))).collect());
        }
        let mut out = Vec::new();
        for (k, element) in row.elements.iter().enumerate() {
            let object = objects.and_then(|o| o.get(k));
            let (bound, at) = match element {
                RowElement::Value(v, at) => {
                    let (e, t) = match taken.get(k).copied().flatten() {
                        Some(t) => self.typed(v, ParameterType::Of(t), scope),
                        None => self.value_expression(v, scope),
                    }
                    .or_at(*at)?;
                    if let Some(o) = object {
                        check_assignment(&o.desc, t, INSERT_RULE).or_at(*at)?;
                    }
                    ((e, Some(t)), *at)
                }
                RowElement::Null(at) => ((Expr::Const(Value::Null), None), *at),
                RowElement::Default(at) => ((Expr::Const(object.map_or(Value::Null, |o| o.default.clone())), None), *at),
            };
            if bound.1.is_none() && objects.is_none() {
                return Err(rule("[ISO/IEC 9075:1992 §7.1 SR1] NULL and DEFAULT are values only of the rows an INSERT inserts").or_at(at));
            }
            out.push(bound);
        }
        Ok(out)
    }
}

/// [ISO/IEC 9075:1992 §7.10] A set operation: its operands, each bound as
/// a query of its own, their columns corresponding \[SR9\], the result's
/// named as both operands name them, or unnamed, and of the types §9.3
/// gives them, known not null if both are \[SR10b, SR12b\]; in a
/// constraint, not possibly non-deterministic [SR15; §11.9 SR6]. SQL-86's rules
/// that each operand select only columns, of identical descriptions
/// [X3.135-1986 §8.3 SR5b], are Entry SQL's [ISO/IEC 9075:1992 §7.10,
/// Leveling Rules].
fn set_operation(cx: Context<'_>, s: &SetOperation, nesting: &Nesting<'_>) -> Result<Bound> {
    let operand = nesting.operand();
    let left = bind_query_expression(cx, &s.left, &operand)?;
    let right = bind_query_expression(cx, &s.right, &operand)?;
    let character = left.columns.iter().chain(&right.columns).any(|c| c.data_type.is_character());
    if cx.purpose.constrains() && character && !(s.op == SetOperator::Union && s.all) {
        return Err(nondeterministic("§7.10 SR15", "UNION, EXCEPT or INTERSECT, not UNION ALL, of a character string column"));
    }
    let (left, right) = match &s.corresponding {
        Correspondence::Position => {
            if left.columns.len() != right.columns.len() {
                let (n, m) = (left.columns.len(), right.columns.len());
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §7.10 SR9b] the operands of a set operation are of one degree, not {n} and {m}"
                )));
            }
            (left, right)
        }
        Correspondence::Names => corresponding(left, right, None)?,
        Correspondence::By(names) => corresponding(left, right, Some(names))?,
    };
    let clause = if s.op == SetOperator::Intersect { "§7.10 SR10b ii" } else { "§7.10 SR12b ii" };
    let mut columns = Vec::new();
    for (a, b) in left.columns.iter().zip(&right.columns) {
        let data_type = set_type(&[a.data_type, b.data_type], clause)?;
        let name = a.name.clone().filter(|_| a.name == b.name);
        columns.push(OutColumn { name, data_type, not_null: a.not_null && b.not_null, origin: None });
    }
    let op = match s.op {
        SetOperator::Union => SetOp::Union,
        SetOperator::Except => SetOp::Except,
        SetOperator::Intersect => SetOp::Intersect,
    };
    let (l, r) = (conform(left, &columns), conform(right, &columns));
    Ok(Bound { plan: LogicalPlan::SetOp { op, all: s.all, left: Box::new(l), right: Box::new(r) }, columns })
}

/// The plan of `bound`, its columns cast to the types of `columns` where
/// theirs differ.
fn conform(bound: Bound, columns: &[OutColumn]) -> LogicalPlan {
    if bound.columns.iter().zip(columns).all(|(a, b)| a.data_type == b.data_type) {
        return bound.plan;
    }
    let exprs = bound
        .columns
        .iter()
        .zip(columns)
        .enumerate()
        .map(|(i, (a, b))| conformed(Expr::Column(i), Some(a.data_type), b.data_type))
        .collect();
    LogicalPlan::Project { exprs, input: Box::new(bound.plan) }
}

/// [ISO/IEC 9075:1992 §7.10 SR9a] The operands of CORRESPONDING, each its
/// columns named `by`, in that order, or else those both name, in the
/// first's order: no operand names a column twice \[i\], and they have one
/// in common \[ii\].
fn corresponding(left: Bound, right: Bound, by: Option<&[String]>) -> Result<(Bound, Bound)> {
    for side in [&left, &right] {
        let named: Vec<&String> = side.columns.iter().filter_map(|c| c.name.as_ref()).collect();
        if let Some(twice) = named.iter().enumerate().find(|(i, n)| named.iter().skip(i.saturating_add(1)).any(|m| m == *n)) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §7.10 SR9a i] an operand of CORRESPONDING names column {} twice", twice.1)));
        }
    }
    let has = |b: &Bound, n: &str| b.columns.iter().any(|c| c.name.as_deref() == Some(n));
    let names: Vec<String> = match by {
        None => left.columns.iter().filter_map(|c| c.name.clone()).filter(|n| has(&right, n)).collect(),
        Some(list) => {
            if let Some(missing) = list.iter().find(|n| !has(&left, n) || !has(&right, n)) {
                return Err(rule(format!(
                    "[ISO/IEC 9075:1992 §7.10 SR9a iii 2] CORRESPONDING BY names {missing}, which is not a column of both operands"
                )));
            }
            list.to_vec()
        }
    };
    if names.is_empty() {
        return Err(rule("[ISO/IEC 9075:1992 §7.10 SR9a ii] the operands of CORRESPONDING have no column name in common"));
    }
    Ok((pick(left, &names), pick(right, &names)))
}

/// The columns of `bound` named `names`, in that order.
fn pick(bound: Bound, names: &[String]) -> Bound {
    let positions: Vec<usize> = names.iter().filter_map(|n| bound.columns.iter().position(|c| c.name.as_ref() == Some(n))).collect();
    let columns = positions.iter().filter_map(|&i| bound.columns.get(i)).map(|c| OutColumn { origin: None, ..c.clone() }).collect();
    let exprs = positions.into_iter().map(Expr::Column).collect();
    Bound { plan: LogicalPlan::Project { exprs, input: Box::new(bound.plan) }, columns }
}
