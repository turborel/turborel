//! \[DESIGN\] (RULES.md D660) The plan of a grouping with one relation of
//! its chain, X, grouped first: X, under its own conjuncts, grouped by
//! its grouping columns (`classify.rs`), computing each pushed function's
//! part and, if a function is scaled, COUNT(*) of each early group; the
//! chain made again of the early groups in X's place; and the grouping
//! over it, by the same columns, each function computed by its role,
//! its results the grouping's in its order.

use std::sync::Arc;

use super::chain::Chain;
use super::classify::{Role, Shape};
use crate::functions::{AggregateRef, ColumnarKind, Merged, PART_TYPE, PARTIAL, Paired, Partial, ScalarRef, Scaled};
use crate::logical::{AggCall, Cond, Expr, LogicalPlan, and_all, cond_columns, expr_columns, map_cond_columns, map_expr_columns};
use crate::value::collation::CollationRef;
use crate::value::types::Field;
use crate::value::{DataType, Decimal, Value};

/// The names of the fields of a value paired with its count.
const VALUE_FIELD: &str = "VALUE";
/// See [`VALUE_FIELD`].
const ROWS_FIELD: &str = "ROWS";

/// The label of the count of each early group's rows.
const ROWS_LABEL: &str = "COUNT(*)";

/// The grouping grouped first in part.
pub(super) struct Grouping<'a> {
    pub(super) groups: &'a [usize],
    pub(super) collations: &'a [Option<CollationRef>],
    pub(super) aggs: &'a [AggCall],
}

/// The plan of `grouping` over `chain`, X its relation `x` grouped first
/// as `shape` says, COUNT(*) made by `count`, the built-in; none if a
/// column read above is not one the early groups give.
pub(super) fn rewritten(grouping: &Grouping<'_>, (chain, x): (&Chain, usize), shape: &Shape, count: &AggregateRef) -> Option<LogicalPlan> {
    let leaf = chain.leaves.get(x)?;
    let local = |c: usize| c.saturating_sub(leaf.start);
    let mut early = Vec::new();
    for (a, role) in grouping.aggs.iter().zip(&shape.roles) {
        if *role == Role::Pushed {
            early.push(part(a, &local));
        }
    }
    let parts = early.len();
    let scaled = shape.roles.iter().any(|r| matches!(r, Role::Scaled(_)));
    if scaled {
        let result = count.0.result_type(None).ok()?;
        early.push(AggCall {
            func: count.clone(),
            arg: None,
            distinct: false,
            filter: None,
            distribution: None,
            result,
            label: ROWS_LABEL.into(),
        });
    }
    let own = and_all(shape.own.iter().map(|c| map_cond_columns(c.clone(), &local)).collect());
    let input = match own {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(leaf.plan.clone()) },
        None => leaf.plan.clone(),
    };
    let width = shape.keys.len().saturating_add(early.len());
    let groups = shape.keys.iter().map(|&k| local(k)).collect();
    let grouped = LogicalPlan::Aggregate { groups, collations: Vec::new(), aggs: early, seeds: Vec::new(), input: Box::new(input) };
    let layout = Layout::new(chain, x, (&shape.keys, width));
    if !layout.answers(grouping, shape) {
        return None;
    }
    let first = layout.start_of(x)?.saturating_add(shape.keys.len());
    let aggs = finals(grouping.aggs, shape, &layout, (first, first.saturating_add(parts)));
    let mut leaves: Vec<LogicalPlan> = chain.leaves.iter().map(|l| l.plan.clone()).collect();
    *leaves.get_mut(x)? = grouped;
    let rest = shape.rest.iter().map(|c| map_cond_columns(c.clone(), &|at| layout.at(at))).collect();
    let joined = Chain::made(leaves, rest)?;
    let moved = grouping.groups.iter().map(|&g| layout.at(g)).collect();
    Some(LogicalPlan::Aggregate {
        groups: moved,
        collations: grouping.collations.to_vec(),
        aggs,
        seeds: Vec::new(),
        input: Box::new(joined),
    })
}

/// The part of `a`, of X's columns as `local` numbers them.
fn part(a: &AggCall, local: &impl Fn(usize) -> usize) -> AggCall {
    AggCall {
        func: AggregateRef(Arc::new(Partial { func: a.func.clone(), result: a.result.clone() })),
        arg: a.arg.clone().map(|e| map_expr_columns(e, local)),
        distinct: false,
        filter: a.filter.clone().map(|c| map_cond_columns(c, local)),
        distribution: None,
        result: PART_TYPE,
        label: format!("{PARTIAL} {}", a.label),
    }
}

/// The functions of the grouping above the join: each pushed one merging
/// its part, the parts at `parts` on; each plain one as it is; each
/// scaled one of its value paired with the early group's count, at
/// `rows`.
fn finals(aggs: &[AggCall], shape: &Shape, layout: &Layout, (mut parts, rows): (usize, usize)) -> Vec<AggCall> {
    let mut out = Vec::with_capacity(aggs.len());
    for (a, role) in aggs.iter().zip(&shape.roles) {
        let moved = a.clone().map_exprs(&mut |e| map_expr_columns(e, &|c| layout.at(c)));
        out.push(match role {
            Role::Pushed => {
                let func = AggregateRef(Arc::new(Merged { func: a.func.clone(), result: a.result.clone() }));
                let arg = Some(Expr::Column(parts));
                parts = parts.saturating_add(1);
                AggCall { func, arg, filter: None, distinct: false, ..a.clone() }
            }
            Role::Plain => moved,
            Role::Scaled(kind) => scaled(moved, *kind, rows),
        });
    }
    out
}

/// `a`, COUNT, SUM or AVG as `kind` says, of its value paired with the
/// count at column `rows` of the rows each stands for.
fn scaled(a: AggCall, kind: ColumnarKind, rows: usize) -> AggCall {
    let one = Expr::Const(Value::Exact(Decimal::new(1, 0)));
    let value = a.arg.clone().unwrap_or(one);
    let fields = [
        (VALUE_FIELD, a.result.clone()),
        (ROWS_FIELD, DataType::Exact { precision: crate::value::BIGINT_PRECISION, scale: 0, spelled: crate::value::Spelled::Numeric }),
    ];
    let row = DataType::row(fields.into_iter().map(|(name, data_type)| Field { name: name.into(), data_type, unnamed: false }).collect());
    let paired = Expr::Call(ScalarRef(Arc::new(Paired(row))), vec![value, Expr::Column(rows)]);
    let func = AggregateRef(Arc::new(Scaled { func: a.func.clone(), kind, result: a.result.clone() }));
    AggCall { func, arg: Some(paired), distinct: false, ..a }
}

/// Where the chain's columns are once X's are the early groups'.
struct Layout {
    /// Of each relation, its first column of the chain's and of the new
    /// chain's, and how many it has.
    starts: Vec<(usize, usize, usize)>,
    x: usize,
    keys: Vec<usize>,
}

impl Layout {
    /// The layout of `chain`, its relation `x` giving `keys` first of its
    /// `width` columns.
    fn new(chain: &Chain, x: usize, (keys, width): (&[usize], usize)) -> Layout {
        let mut next = 0_usize;
        let mut starts = Vec::with_capacity(chain.leaves.len());
        for (i, l) in chain.leaves.iter().enumerate() {
            starts.push((l.start, next, l.width));
            next = next.saturating_add(if i == x { width } else { l.width });
        }
        Layout { starts, x, keys: keys.to_vec() }
    }

    /// The new first column of relation `i`.
    fn start_of(&self, i: usize) -> Option<usize> {
        self.starts.get(i).map(|s| s.1)
    }

    /// The new place of the chain's column `c`, if it has one.
    fn place(&self, c: usize) -> Option<usize> {
        let i = self.starts.iter().position(|&(start, _, width)| (start..start.saturating_add(width)).contains(&c))?;
        let &(start, new, _) = self.starts.get(i)?;
        if i == self.x {
            return self.keys.iter().position(|&k| k == c).map(|p| new.saturating_add(p));
        }
        Some(new.saturating_add(c.saturating_sub(start)))
    }

    /// The new place of `c`, known to have one (`answers`).
    fn at(&self, c: usize) -> usize {
        self.place(c).unwrap_or(c)
    }

    /// Whether every column the grouping reads above the join has a new
    /// place: its grouping columns, the conjuncts of the rest, and the
    /// columns of the functions not pushed.
    fn answers(&self, grouping: &Grouping<'_>, shape: &Shape) -> bool {
        let mut reads: Vec<usize> = grouping.groups.to_vec();
        shape.rest.iter().for_each(|c: &Cond| cond_columns(c, &mut reads));
        for (a, role) in grouping.aggs.iter().zip(&shape.roles) {
            if *role != Role::Pushed {
                a.exprs().into_iter().for_each(|e| expr_columns(e, &mut reads));
            }
        }
        reads.into_iter().all(|c| self.place(c).is_some())
    }
}
