//! \[DESIGN\] The plan form written and read, format 1 (`plan.rs` gives
//! its bytes). Reading is bounded by `MAX_DEPTH`, so a form nested past
//! it is refused, not followed (RULES.md R-CF-6).

use super::{Body, Column, ColumnStatistics, Expression, FORMAT, Form, JoinKind, Key, MAX_DEPTH, Node, Op, Operator, SetKind};
use crate::bytes::{Put, Reader};
use crate::error::{Result, SqlError};
use crate::plugin::errors::violation;
use crate::plugin::payload::{count, put_f64, read_f64};
use crate::plugin::values::{put_value, value};

/// The bytes of `form`.
///
/// # Errors
///
/// SQLSTATE 38T03 of a value no protocol 1 carries.
pub fn encode(form: &Form) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.put_u32(FORMAT);
    put_node(&mut out, &form.root)?;
    Ok(out)
}

/// The form `r` holds.
///
/// # Errors
///
/// SQLSTATE 38T03 if it is no plan form of format 1, or nests past
/// `MAX_DEPTH`.
pub fn decode(r: &mut Reader<'_>) -> Result<Form> {
    let format = r.u32()?;
    if format != FORMAT {
        return Err(violation(&format!("a plan form of format {format}, not {FORMAT}")));
    }
    Ok(Form { root: node(r, 0)? })
}

/// That `depth` is within `MAX_DEPTH`.
fn within(depth: usize) -> Result<usize> {
    if depth >= MAX_DEPTH { Err(violation(&format!("a plan form nested deeper than {MAX_DEPTH}"))) } else { Ok(depth.saturating_add(1)) }
}

/// Writes `n`.
fn put_node(out: &mut Vec<u8>, n: &Node) -> Result<()> {
    out.put_u8(n.op.code());
    out.put_u32(n.id);
    out.put_u8(u8::from(n.kept));
    out.put_u32(count(n.columns.len()));
    for c in &n.columns {
        out.put_str(&c.name);
        out.put_str(&c.data_type);
    }
    put_f64(out, n.rows);
    put_f64(out, n.width);
    put_body(out, &n.body)?;
    out.put_u32(count(n.inputs.len()));
    n.inputs.iter().try_for_each(|i| put_node(out, i))
}

/// Writes `b`.
fn put_body(out: &mut Vec<u8>, b: &Body) -> Result<()> {
    match b {
        Body::Kept(label) => out.put_str(label),
        Body::Scan { table, statistics } => {
            out.put_str(table);
            out.put_u32(count(statistics.len()));
            statistics.iter().try_for_each(|s| put_statistics(out, s))?;
        }
        Body::Filter(e) => put_expr(out, e)?,
        Body::Project(all) => put_exprs(out, all)?,
        Body::Join { kind, condition } => {
            out.put_u8(join_code(*kind));
            out.put_u8(u8::from(condition.is_some()));
            condition.iter().try_for_each(|c| put_expr(out, c))?;
        }
        Body::Sort(keys) => {
            out.put_u32(count(keys.len()));
            for k in keys {
                out.put_u32(k.column);
                out.put_u8(u8::from(k.descending));
                out.put_u8(u8::from(k.nulls_first));
            }
        }
        Body::Distinct => {}
        Body::Set { op, all } => {
            out.put_u8(set_code(*op));
            out.put_u8(u8::from(*all));
        }
        Body::Values(rows) => {
            out.put_u32(count(rows.len()));
            rows.iter().try_for_each(|row| put_exprs(out, row))?;
        }
    }
    Ok(())
}

/// Writes a column's statistics.
fn put_statistics(out: &mut Vec<u8>, s: &ColumnStatistics) -> Result<()> {
    put_f64(out, s.distinct);
    put_f64(out, s.nulls);
    out.put_u8(u8::from(s.bounds.is_some()));
    if let Some((low, high)) = &s.bounds {
        put_value(out, low)?;
        put_value(out, high)?;
    }
    Ok(())
}

/// The code of a join's kind.
fn join_code(k: JoinKind) -> u8 {
    match k {
        JoinKind::Cross => 0,
        JoinKind::Left => 1,
        JoinKind::Right => 2,
        JoinKind::Full => 3,
    }
}

/// The code of a set operator.
fn set_code(k: SetKind) -> u8 {
    match k {
        SetKind::Union => 1,
        SetKind::Except => 2,
        SetKind::Intersect => 3,
    }
}

/// Writes expressions, counted.
fn put_exprs(out: &mut Vec<u8>, all: &[Expression]) -> Result<()> {
    out.put_u32(count(all.len()));
    all.iter().try_for_each(|e| put_expr(out, e))
}

/// Writes `e`.
fn put_expr(out: &mut Vec<u8>, e: &Expression) -> Result<()> {
    match e {
        Expression::Column(i) => {
            out.put_u8(1);
            out.put_u32(*i);
        }
        Expression::Literal { data_type, value } => {
            out.put_u8(2);
            out.put_str(data_type);
            put_value(out, value)?;
        }
        Expression::Call { name, args } => {
            out.put_u8(3);
            out.put_str(name);
            put_exprs(out, args)?;
        }
        Expression::Operator { op, args } => {
            out.put_u8(4);
            out.put_u8(op.code());
            put_exprs(out, args)?;
        }
        Expression::Cast { data_type, arg } => {
            out.put_u8(5);
            out.put_str(data_type);
            put_expr(out, arg)?;
        }
        Expression::Opaque { id, columns } => {
            out.put_u8(6);
            out.put_u32(*id);
            out.put_u32(count(columns.len()));
            for c in columns {
                out.put_u32(*c);
            }
        }
    }
    Ok(())
}

/// A node, at `depth`.
fn node(r: &mut Reader<'_>, depth: usize) -> Result<Node> {
    let deeper = within(depth)?;
    let code = r.u8()?;
    let op = Op::of_code(code).ok_or_else(|| violation(&format!("no plan node is of operator {code}")))?;
    let (id, kept) = (r.u32()?, r.u8()? != 0);
    let columns = (0..r.u32()?).map(|_| Ok(Column { name: r.str()?, data_type: r.str()? })).collect::<Result<Vec<_>>>()?;
    let (rows, width) = (read_f64(r)?, read_f64(r)?);
    let body = if kept { Body::Kept(r.str()?) } else { body(r, op, deeper)? };
    let inputs = (0..r.u32()?).map(|_| node(r, deeper)).collect::<Result<Vec<_>>>()?;
    Ok(Node { op, id, kept, columns, rows, width, body, inputs })
}

/// The body of a node of `op` not kept.
fn body(r: &mut Reader<'_>, op: Op, depth: usize) -> Result<Body> {
    Ok(match op {
        Op::Scan => {
            let table = r.str()?;
            Body::Scan { table, statistics: (0..r.u32()?).map(|_| statistics(r)).collect::<Result<_>>()? }
        }
        Op::Filter => Body::Filter(expr(r, depth)?),
        Op::Project => Body::Project(exprs(r, depth)?),
        Op::Join => {
            let kind = match r.u8()? {
                0 => JoinKind::Cross,
                1 => JoinKind::Left,
                2 => JoinKind::Right,
                3 => JoinKind::Full,
                other => return Err(violation(&format!("no join is of kind {other}"))),
            };
            Body::Join { kind, condition: if r.u8()? == 0 { None } else { Some(expr(r, depth)?) } }
        }
        Op::Sort => Body::Sort(
            (0..r.u32()?)
                .map(|_| Ok(Key { column: r.u32()?, descending: r.u8()? != 0, nulls_first: r.u8()? != 0 }))
                .collect::<Result<_>>()?,
        ),
        Op::Distinct => Body::Distinct,
        Op::Set => Body::Set { op: set_kind(r.u8()?)?, all: r.u8()? != 0 },
        Op::Values => Body::Values((0..r.u32()?).map(|_| exprs(r, depth)).collect::<Result<_>>()?),
        Op::Aggregate
        | Op::Limit
        | Op::System
        | Op::Recursive
        | Op::WorkTable
        | Op::Transition
        | Op::Unnest
        | Op::Window
        | Op::Recognize
        | Op::Polymorphic => return Err(only_kept(op)),
    })
}

/// That a node of `op` can only be kept.
fn only_kept(op: Op) -> SqlError {
    violation(&format!("a plan node of operator {} is only kept, never made", op.code()))
}

/// The set operator of `code`.
fn set_kind(code: u8) -> Result<SetKind> {
    match code {
        1 => Ok(SetKind::Union),
        2 => Ok(SetKind::Except),
        3 => Ok(SetKind::Intersect),
        other => Err(violation(&format!("no set operator is of code {other}"))),
    }
}

/// A column's statistics.
fn statistics(r: &mut Reader<'_>) -> Result<ColumnStatistics> {
    let (distinct, nulls) = (read_f64(r)?, read_f64(r)?);
    let bounds = if r.u8()? == 0 { None } else { Some((value(r)?, value(r)?)) };
    Ok(ColumnStatistics { distinct, nulls, bounds })
}

/// Expressions, counted, at `depth`.
fn exprs(r: &mut Reader<'_>, depth: usize) -> Result<Vec<Expression>> {
    (0..r.u32()?).map(|_| expr(r, depth)).collect()
}

/// An expression, at `depth`.
fn expr(r: &mut Reader<'_>, depth: usize) -> Result<Expression> {
    let deeper = within(depth)?;
    Ok(match r.u8()? {
        1 => Expression::Column(r.u32()?),
        2 => Expression::Literal { data_type: r.str()?, value: value(r)? },
        3 => Expression::Call { name: r.str()?, args: exprs(r, deeper)? },
        4 => {
            let code = r.u8()?;
            let op = Operator::of_code(code).ok_or_else(|| violation(&format!("no operator is of code {code}")))?;
            Expression::Operator { op, args: exprs(r, deeper)? }
        }
        5 => Expression::Cast { data_type: r.str()?, arg: Box::new(expr(r, deeper)?) },
        6 => Expression::Opaque { id: r.u32()?, columns: (0..r.u32()?).map(|_| r.u32()).collect::<Result<_>>()? },
        other => return Err(violation(&format!("no expression is of code {other}"))),
    })
}

#[cfg(test)]
mod tests;
