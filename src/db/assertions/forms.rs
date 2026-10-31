//! \[EXTENSION\] The forms of an assertion's conjuncts that a statement's
//! change checks [ISO/IEC 9075:1992 §11.34 GR2] (RULES.md D51), found in
//! its bound query; `changes.rs` checks them. Split from changes.rs
//! (RULES.md R-SIZE-2).
//!
//! A form is of the rows of one table T that pass a chain of steps
//! (`Step`), taken from T up: the filters of the query's WHERE and of a
//! view merged into it, and the projection of a view that is not
//! (`binder/views.rs`). A filter's condition is of the row alone, as
//! changes.rs says. So is each expression of a projection; a step above it
//! is rewritten over T's row, each column the expression for it, and those
//! of its expressions that can raise an error are evaluated on each row it
//! projects, as it evaluates them all (RULES.md D15). A filter above another
//! sees only the rows that one keeps (`optimizer::MergeFilters`), so a row
//! stops at the first step that does not keep it.
//!
//! A set function's argument `x` is of the row alone too. Where it is null
//! in a row passing, the whole check raises a warning, a null eliminated
//! [§6.5 GR1b]; whether it does depends on every row, so a form whose `x`
//! may be null keeps it (`Form::nulls`), and changes.rs takes the change as
//! the whole check only if a row of it shows the warning raised. `x` is
//! never null if it is a column of a NOT NULL or PRIMARY KEY constraint not
//! deferrable, whose checks come before an assertion's at a statement's
//! end; one a conjunct of a filter is not true of when null (a comparison,
//! IS NOT NULL, LIKE or IN of it, or of arithmetic of it); a constant not
//! null; or arithmetic of those.
//!
//! - `NOT EXISTS (SELECT ... FROM T ...)`: no row passes (`Kind::NoRow`).
//! - `(SELECT COUNT(*) FROM T ...) op k`, `k` a constant, either side: the
//!   count of rows passing (`Kind::Count`); and COUNT(x), `x IS NOT NULL` a
//!   step, which evaluates `x` as COUNT does.
//! - COUNT(DISTINCT x) `op k`: the count of the values of `x` of the rows
//!   passing, `x IS NOT NULL` a step (`Kind::Distinct`), values the same
//!   as DISTINCT takes them [§6.5 GR2].
//! - `MAX(x) < k` or `<= k`, `MIN(x) > k` or `>= k`: the greatest or least
//!   by the comparison rules [§6.5 GR2b iii], so false exactly when a row
//!   passing has `x op k` false: `NOT (x op k)` a step (`Kind::Bound`).
//! - `MAX(x) > k` or `>= k`, `MIN(x) < k` or `<= k`: true if a row passing
//!   has `x op k` true (`Kind::Reach`).
//! - `MAX(x) = k`: `MAX(x) <= k AND MAX(x) >= k`, the same truth, unknown
//!   both of no row [§8.2 GR1a, §8.12 GR2]; so two forms, MIN alike.
//! - `MAX(x) <> k`: true if a row passing has `x > k`; or, a row only added,
//!   each `x < k`, the greatest stays on its side of k (`Kind::Apart`).
//!   MIN alike, the sides turned.
//!
//! DISTINCT changes no greatest or least [GR2], so MAX and MIN may have it.
//! SUM and AVG have no form: their value, and whether they overflow [GR2b
//! iv], depend on every row, the running sum checked as each is added. Nor
//! do a join, a grouping, or a set function a plug-in replaced.

use crate::catalog::{Catalog, ConstraintKind, QualifiedName};
use crate::functions::{Registry, Standard};
use crate::logical::{Cond, Expr, LogicalPlan, Subquery, Truth, map_cond_exprs, split_and};
use crate::optimizer::substitute;
use crate::value::CompOp;

/// A conjunct of an assertion's condition that a statement's change
/// checks, of the rows of `table` that pass every one of `steps`.
pub(super) struct Form {
    pub(super) table: QualifiedName,
    pub(super) steps: Vec<Step>,
    pub(super) kind: Kind,
    /// A set function's argument that may be null, and how many of the
    /// steps are of the rows it is evaluated on, before its own.
    pub(super) nulls: Option<(usize, Expr)>,
}

/// What a row of the table passes, in order, to be one of a form's.
#[derive(Debug, Clone)]
pub(super) enum Step {
    /// A filter: the row passes if the condition is true of it.
    Keep(Cond),
    /// A projection's expressions that can raise an error, each evaluated;
    /// the row passes if none does.
    Compute(Vec<Expr>),
}

impl Step {
    /// The step over the row a projection of `exprs` projects.
    fn over(self, exprs: &[Expr]) -> Step {
        match self {
            Step::Keep(c) => Step::Keep(map_cond_exprs(c, &|e| substitute(e, exprs))),
            Step::Compute(computed) => Step::Compute(computed.into_iter().map(|e| substitute(e, exprs)).collect()),
        }
    }
}

/// What the conjunct says of the rows passing.
#[derive(Debug, Clone)]
pub(super) enum Kind {
    /// None does: false at the first that does, as EXISTS stops there.
    NoRow,
    /// None does, each evaluated, as a set function reads every row.
    Bound,
    /// Their count compares so with a constant: a change of `Safe`'s
    /// direction cannot make it false.
    Count(Safe),
    /// It is true if the condition is true of one of them.
    Reach(Cond),
    /// The count of the distinct values of the expression compares so
    /// with a constant, as `Count`.
    Distinct(Safe, Expr),
    /// The greatest or least is not equal to a constant: true if `beyond`
    /// is true of one of them, and kept by rows added of `within` true.
    Apart { beyond: Cond, within: Cond },
}

/// The changes of a count that cannot make its comparison false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Safe {
    /// None that adds to it: of `<` and `<=`.
    Fewer,
    /// None that takes from it: of `>` and `>=`.
    More,
    /// None that changes it: of `=` and `<>`.
    Same,
}

/// What finding the forms takes of the database: its functions, and its
/// catalog, for the constraints of a column.
pub(super) struct Known<'a> {
    pub(super) functions: &'a Registry,
    pub(super) catalog: &'a dyn Catalog,
}

/// The forms of the conjuncts of the condition of `plan`, an assertion's
/// bound query, `SELECT 1 FROM (VALUES (1)) WHERE NOT (condition)`, if
/// each has one.
pub(super) fn forms_of(plan: &LogicalPlan, known: &Known<'_>) -> Option<Vec<Form>> {
    let LogicalPlan::Project { input, .. } = plan else { return None };
    let LogicalPlan::Filter { cond: Cond::Not(condition), input: values } = &**input else { return None };
    if !matches!(**values, LogicalPlan::Values { .. }) {
        return None;
    }
    let mut conjuncts = Vec::new();
    split_and((**condition).clone(), &mut conjuncts);
    let forms: Option<Vec<Vec<Form>>> = conjuncts.iter().map(|c| form_of(c, known)).collect();
    forms.map(|f| f.into_iter().flatten().collect())
}

/// The forms of the conjunct `c`, if it has: one, or two of `MAX(x) = k`.
fn form_of(c: &Cond, known: &Known<'_>) -> Option<Vec<Form>> {
    match c {
        Cond::Not(inner) => {
            let Cond::Exists(s) = &**inner else { return None };
            exists_form(s).map(|f| vec![f])
        }
        Cond::Compare(Expr::Subquery(s, 0), op, k @ Expr::Const(_)) => aggregate_form(s, (*op, k), known),
        Cond::Compare(k @ Expr::Const(_), op, Expr::Subquery(s, 0)) => aggregate_form(s, (op.flip(), k), known),
        Cond::Const(_)
        | Cond::Or(..)
        | Cond::And(..)
        | Cond::Is(..)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::In(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(_)
        | Cond::Match { .. } => None,
    }
}

/// `NOT EXISTS (SELECT ... FROM T WHERE p)`: of `s`, the subquery.
fn exists_form(s: &Subquery) -> Option<Form> {
    let rows = if let LogicalPlan::Project { exprs, input } = &s.plan {
        // The select list of EXISTS, which raises nothing.
        exprs.iter().all(|e| matches!(e, Expr::Column(_) | Expr::Const(_))).then_some(&**input)?
    } else {
        &s.plan
    };
    let passing = passing(rows).filter(|_| s.outer.is_empty())?;
    Some(Form { table: passing.table, steps: passing.steps, kind: Kind::NoRow, nulls: None })
}

/// `(SELECT f(x) FROM T WHERE p) op k`, `f` the standard COUNT, MAX or
/// MIN: of `s`, the subquery, `op`, the comparison with it first, and `k`.
fn aggregate_form(s: &Subquery, (op, k): (CompOp, &Expr), known: &Known<'_>) -> Option<Vec<Form>> {
    let LogicalPlan::Project { exprs, input } = &s.plan else { return None };
    let LogicalPlan::Aggregate { groups, aggs, input: rows } = &**input else { return None };
    let ([Expr::Column(0)], [agg]) = (exprs.as_slice(), aggs.as_slice()) else { return None };
    let function = known.functions.standard(&agg.func).filter(|_| groups.is_empty() && s.outer.is_empty())?;
    let Passing { table, steps, columns } = passing(rows)?;
    let Some(x) = &agg.arg else {
        // COUNT(*) alone has the form (*).
        let kind = Kind::Count(safe(op));
        return (function == Standard::Count).then(|| vec![Form { table, steps, kind, nulls: None }]);
    };
    let x = Some(x).filter(|x| row_expr(x)).map(|x| columns.as_deref().map_or_else(|| x.clone(), |c| substitute(x.clone(), c)))?;
    let nulls = (!never_null(&x, (&table, &steps), known.catalog)).then(|| (steps.len(), x.clone()));
    let form = |step: Option<Cond>, kind: Kind| {
        let mut steps = steps.clone();
        steps.extend(step.map(Step::Keep));
        Form { table: table.clone(), steps, kind, nulls: nulls.clone() }
    };
    let compared = |by: CompOp| Cond::Compare(x.clone(), by, k.clone());
    let bound = |by: CompOp| form(Some(Cond::Not(Box::new(compared(by)))), Kind::Bound);
    let present = Some(Cond::IsNull(x.clone(), true));
    Some(match (function, op) {
        (Standard::Count, _) => {
            let kind = if agg.distinct { Kind::Distinct(safe(op), x.clone()) } else { Kind::Count(safe(op)) };
            vec![form(present, kind)]
        }
        (Standard::Max, CompOp::Lt | CompOp::Le) | (Standard::Min, CompOp::Gt | CompOp::Ge) => vec![bound(op)],
        (Standard::Max, CompOp::Gt | CompOp::Ge) | (Standard::Min, CompOp::Lt | CompOp::Le) => vec![form(None, Kind::Reach(compared(op)))],
        (Standard::Max, CompOp::Eq) => vec![bound(CompOp::Le), form(None, Kind::Reach(compared(CompOp::Ge)))],
        (Standard::Min, CompOp::Eq) => vec![bound(CompOp::Ge), form(None, Kind::Reach(compared(CompOp::Le)))],
        (Standard::Max, CompOp::Ne) => vec![form(None, Kind::Apart { beyond: compared(CompOp::Gt), within: compared(CompOp::Lt) })],
        (Standard::Min, CompOp::Ne) => vec![form(None, Kind::Apart { beyond: compared(CompOp::Lt), within: compared(CompOp::Gt) })],
    })
}

/// The changes of a count compared by `op`, the count first, that cannot
/// make the comparison false.
fn safe(op: CompOp) -> Safe {
    match op {
        CompOp::Lt | CompOp::Le => Safe::Fewer,
        CompOp::Gt | CompOp::Ge => Safe::More,
        CompOp::Eq | CompOp::Ne => Safe::Same,
    }
}

/// The rows of a plan, as `passing` finds them: the table, the steps a row
/// of it passes, and, below a projection, each column of the plan as an
/// expression over the table's row.
struct Passing {
    table: QualifiedName,
    steps: Vec<Step>,
    columns: Option<Vec<Expr>>,
}

/// The rows of `plan`, if it is filters of conditions and projections of
/// expressions of the row alone over a scan.
fn passing(plan: &LogicalPlan) -> Option<Passing> {
    // Taken from the top, each over the row of the node below, so turned.
    let (mut steps, mut columns) = (Vec::new(), None::<Vec<Expr>>);
    let mut node = plan;
    loop {
        match node {
            LogicalPlan::Filter { cond, input } => {
                steps.push(Step::Keep(Some(cond).filter(|c| row_cond(c))?.clone()));
                node = input;
            }
            LogicalPlan::Project { exprs, input } => {
                if !exprs.iter().all(row_expr) {
                    return None;
                }
                steps = steps.into_iter().map(|s| s.over(exprs)).collect();
                columns = Some(columns.map_or_else(|| exprs.clone(), |top| top.into_iter().map(|e| substitute(e, exprs)).collect()));
                let raising: Vec<Expr> = exprs.iter().filter(|e| e.can_raise()).cloned().collect();
                if !raising.is_empty() {
                    steps.push(Step::Compute(raising));
                }
                node = input;
            }
            LogicalPlan::Scan { table, .. } => {
                steps.reverse();
                return Some(Passing { table: table.clone(), steps, columns });
            }
            LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. } => return None,
        }
    }
}

/// Whether `x`, an expression of the row alone over `table`'s rows that
/// pass `steps`, is never null, as the module says.
fn never_null(x: &Expr, (table, steps): (&QualifiedName, &[Step]), catalog: &dyn Catalog) -> bool {
    match x {
        Expr::Column(at) => rejected(*at, steps) || declared(table, *at, catalog),
        Expr::Const(v) => !v.is_null(),
        Expr::Neg(y) => never_null(y, (table, steps), catalog),
        Expr::Arith(a, _, b) => never_null(a, (table, steps), catalog) && never_null(b, (table, steps), catalog),
        Expr::Call(..) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Case(..) | Expr::Cast(..) | Expr::Domain(..) | Expr::Subquery(..) => {
            false
        }
    }
}

/// Whether column `at` of `table` holds no null at a statement's end: a
/// NOT NULL or PRIMARY KEY constraint not deferrable is on it [ISO/IEC
/// 9075:1992 §4.10.1, §11.7 SR3a].
fn declared(table: &QualifiedName, at: usize, catalog: &dyn Catalog) -> bool {
    catalog.table(table).is_some_and(|desc| {
        desc.constraints.iter().any(|c| {
            let on = match &c.kind {
                ConstraintKind::NotNull(column) => *column == at,
                ConstraintKind::Unique { columns, primary } => *primary && columns.contains(&at),
                ConstraintKind::ForeignKey(_) | ConstraintKind::Check { .. } => false,
            };
            on && !c.attributes.deferrable
        })
    })
}

/// Whether a conjunct of a filter of `steps` is not true of a row whose
/// column `at` is null.
fn rejected(at: usize, steps: &[Step]) -> bool {
    steps.iter().any(|step| match step {
        Step::Keep(cond) => {
            let mut conjuncts = Vec::new();
            split_and(cond.clone(), &mut conjuncts);
            conjuncts.iter().any(|c| rejects(c, at))
        }
        Step::Compute(_) => false,
    })
}

/// Whether `c` is not true of a row whose column `at` is null: it is
/// unknown, or IS NOT NULL false [ISO/IEC 9075:1992 §8.2 GR1a, §8.4 SR4,
/// §8.5 GR1, GR2, §8.6 GR3].
fn rejects(c: &Cond, at: usize) -> bool {
    match c {
        Cond::IsNull(e, true) | Cond::Like(e, ..) | Cond::In(e, _) => strict(e, at),
        Cond::Compare(a, _, b) => strict(a, at) || strict(b, at),
        Cond::Is(inner, Truth::True, false) => rejects(inner, at),
        Cond::And(a, b) => rejects(a, at) || rejects(b, at),
        Cond::IsNull(_, false)
        | Cond::Is(..)
        | Cond::Const(_)
        | Cond::Or(..)
        | Cond::Not(_)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Unique(_)
        | Cond::Match { .. } => false,
    }
}

/// Whether `e` is null whenever column `at` is: the column, or arithmetic
/// of it [ISO/IEC 9075:1992 §6.12 GR1].
fn strict(e: &Expr, at: usize) -> bool {
    match e {
        Expr::Column(column) => *column == at,
        Expr::Neg(x) => strict(x, at),
        Expr::Arith(a, _, b) => strict(a, at) || strict(b, at),
        Expr::Const(_)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Subquery(..) => false,
    }
}

/// Whether `c` is a condition of the row alone, as changes.rs says.
fn row_cond(c: &Cond) -> bool {
    match c {
        Cond::Const(_) => true,
        Cond::Or(a, b) | Cond::And(a, b) => row_cond(a) && row_cond(b),
        Cond::Not(a) | Cond::Is(a, ..) => row_cond(a),
        Cond::Compare(a, _, b) => row_expr(a) && row_expr(b),
        Cond::IsNull(e, _) | Cond::In(e, _) => row_expr(e),
        Cond::Like(e, pattern, escape) => row_expr(e) && row_expr(pattern) && escape.as_ref().is_none_or(row_expr),
        Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(_) | Cond::Match { .. } => false,
    }
}

/// Whether `e` is an expression of the row alone, as changes.rs says.
fn row_expr(e: &Expr) -> bool {
    match e {
        Expr::Column(_) | Expr::Const(_) => true,
        Expr::Neg(x) => row_expr(x),
        Expr::Arith(a, _, b) => row_expr(a) && row_expr(b),
        Expr::Case(whens, otherwise) => whens.iter().all(|(c, result)| row_cond(c) && row_expr(result)) && row_expr(otherwise),
        Expr::Call(..) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Cast(..) | Expr::Domain(..) | Expr::Subquery(..) => false,
    }
}

// Unit tests: the forms found of an assertion's bound query, and the
// shapes of none (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
