//! [ISO/IEC 9075-2:2023 §6.10, §9.23] The window functions of a select
//! list, bound: each refused where the Syntax Rules refuse it, its window
//! described (`spec.rs`) and bound in the select list's scope, over the
//! rows of the table expression or, of a grouped query, over its groups
//! [§7.16 SR13, §9.20], and its value a placeholder column until the plan
//! is made. The plan appends the windows' partitioning columns and sort
//! keys to those rows, then computes the functions of each partitioning
//! and ordering in a window of its own, one after another, before the
//! select list.

mod spec;

use self::spec::{Described, Keys, described};
use super::collation::any_of;
use super::{Binder, Scope, rule};
use crate::ast::{Composite, ValueExpression, WindowDefinition, WindowFunction, WindowKind as Kind, WindowOver};
use crate::error::{Code, Result, SqlError};
use crate::functions::AggregateRef;
use crate::lexer::kw;
use crate::logical::{AggCall, Expr, LogicalPlan, Window, WindowCall, WindowKind, expr_columns, map_expr_columns, render_expr};
use crate::value::collation::CollationRef;
use crate::value::{DOUBLE_PRECISION, DataType, Spelled};

/// \[DESIGN\] The first placeholder column, of the first window function
/// bound: far past the columns of any row, so the plan's placing of them
/// alone moves them.
const PLACEHOLDER: usize = usize::MAX >> 1_u32;

/// [ISO/IEC 9075-2:2023 §6.10 SR2b] Where else a window function is.
const OUTSIDE: &str = "[ISO/IEC 9075-2:2023 §6.10 SR2b] a window function is only in the select list of a query specification";
/// [ISO/IEC 9075-2:2023 §10.9 SR7a] A window function in an aggregate's argument.
const IN_ARGUMENT: &str = "[ISO/IEC 9075-2:2023 §10.9 SR7a] the argument of a set function cannot contain a window function";
/// [ISO/IEC 9075-2:2023 §7.15 SR7] A window function in a window.
const IN_WINDOW: &str = "[ISO/IEC 9075-2:2023 §7.15 SR7] a window's partitioning, ordering and frame cannot contain a window function";

/// What the window functions of a query's select list are bound with, and
/// what they are bound to.
pub(super) struct Windowing {
    /// The definitions of its `<window clause>`, described [§7.15 GR1b i].
    named: Vec<(String, Described)>,
    /// Whether a window function may be bound now: while its select list
    /// is.
    open: bool,
    /// The rule a window function bound now breaks, if one: of a window
    /// function's argument or window being bound.
    within: Option<&'static str>,
    /// Whether the next aggregate bound is a window function's own.
    taking: bool,
    /// That aggregate, once bound.
    taken: Option<AggCall>,
    /// The expressions its windows append to the rows they are over.
    extras: Vec<Expr>,
    /// Its windows, one of each partitioning and ordering, in order.
    windows: Vec<Partitioned>,
    /// How many window functions it has bound.
    calls: usize,
}

/// The window functions of one partitioning and ordering: of the
/// expressions appended, by their places among them.
struct Partitioned {
    /// Its partitioning and ordering as text, which tells two apart.
    key: String,
    /// The places of its partitioning columns.
    partition: Vec<usize>,
    /// The places of its sort keys, each with its order.
    order: Vec<(usize, crate::value::SortOrder)>,
    /// Its functions, each with its number among the query's.
    calls: Vec<(usize, WindowCall)>,
}

impl Windowing {
    /// The windowing of a query of the `<window clause>` `definitions`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a definition breaks a Syntax Rule of §7.15.
    pub(super) fn new(definitions: &[WindowDefinition]) -> Result<Windowing> {
        let named = described(definitions)?;
        let (extras, windows) = (Vec::new(), Vec::new());
        Ok(Windowing { named, open: false, within: None, taking: false, taken: None, extras, windows, calls: 0 })
    }

    /// Whether it has bound a window function.
    fn computes(&self) -> bool {
        self.calls > 0
    }
}

/// Whether `v` contains a window function, not in a subquery.
pub(super) fn has_window_function(v: &ValueExpression) -> bool {
    v.any(&|e| matches!(e, ValueExpression::Window(_)))
}

/// Adds to `out` every column reference of `v`, those in its set
/// functions too, not those of a subquery.
fn every_column_reference<'v>(v: &'v ValueExpression, out: &mut Vec<&'v ValueExpression>) {
    let chain = if let ValueExpression::Composite(c, _) = v { matches!(**c, Composite::Chain(_)) } else { false };
    if chain || matches!(v, ValueExpression::Column(_)) {
        out.push(v);
    } else {
        v.for_each_child(&mut |child| every_column_reference(child, out));
    }
}

impl Binder<'_> {
    /// Opens its select list to window functions, fresh of any bound
    /// before.
    pub(super) fn open_windows(&self) {
        if let Some(w) = self.windowing.borrow_mut().as_mut() {
            w.open = true;
            (w.extras, w.windows, w.calls) = (Vec::new(), Vec::new(), 0);
        }
    }

    /// Closes its select list, bound in `scope`, to window functions, each
    /// definition of its `<window clause>` checked as it would be bound
    /// there, used or not [§7.15 SR4 to SR15].
    pub(super) fn close_windows(&self, scope: Scope<'_>) -> Result<()> {
        let named: Vec<Described> =
            self.windowing.borrow().as_ref().map(|w| w.named.iter().map(|(_, d)| d.clone()).collect()).unwrap_or_default();
        for d in &named {
            self.own_references(None, d)?;
            let keys = self.within(IN_WINDOW, || self.window_keys(d, scope))?;
            self.within(IN_WINDOW, || self.window_frame(d.frame.as_ref(), &keys, scope))?;
        }
        if let Some(w) = self.windowing.borrow_mut().as_mut() {
            w.open = false;
        }
        Ok(())
    }

    /// Whether its select list has a window function bound.
    pub(super) fn has_windows(&self) -> bool {
        self.windowing.borrow().as_ref().is_some_and(Windowing::computes)
    }

    /// Takes its window functions, bound, if it has any.
    pub(super) fn take_windows(&self) -> Option<Windowing> {
        self.windowing.borrow_mut().take().filter(Windowing::computes)
    }

    /// Whether the next aggregate bound is a window function's: taken, so
    /// those of its argument are not.
    pub(super) fn windowed_next(&self) -> bool {
        self.windowing.borrow_mut().as_mut().is_some_and(|w| std::mem::take(&mut w.taking))
    }

    /// [ISO/IEC 9075-2:2023 §9.23 SR8] The aggregate function `func` of a
    /// window function, of `arg`, DISTINCT if so, bound in the select
    /// list's `scope`, as it is over the rows of the window frame: kept for
    /// the window function. Its value is not a column.
    pub(super) fn window_aggregate(
        &self,
        func: AggregateRef,
        arg: Option<&ValueExpression>,
        distinct: bool,
        scope: Scope<'_>,
    ) -> Result<(Expr, DataType)> {
        let (call, _) = self.within(IN_ARGUMENT, || self.aggregate_call(func, arg, distinct, scope))?;
        let result = call.result.clone();
        if let Some(w) = self.windowing.borrow_mut().as_mut() {
            w.taken = Some(call);
        }
        Ok((Expr::Const(crate::value::Value::Null), result))
    }

    /// `bind` run with window functions refused by `rule`.
    fn within<T>(&self, rule: &'static str, bind: impl FnOnce() -> Result<T>) -> Result<T> {
        let before = self.windowing.borrow_mut().as_mut().map(|w| w.within.replace(rule));
        let bound = bind();
        if let (Some(w), Some(before)) = (self.windowing.borrow_mut().as_mut(), before) {
            w.within = before;
        }
        bound
    }

    /// [ISO/IEC 9075-2:2023 §6.10] The window function `w` in `scope`: a
    /// placeholder column for its value, and its type.
    pub(super) fn window_function(&self, w: &WindowFunction, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.window_allowed(scope)?;
        let described = self.described_window(&w.over)?;
        let aggregate = if let Kind::Aggregate(a) = &w.kind { Some(&**a) } else { None };
        self.own_references(aggregate, &described)?;
        kind_rules(&w.kind, &described)?;
        self.deterministic(&w.kind, &described)?;
        let keys = self.within(IN_WINDOW, || self.window_keys(&described, scope))?;
        let frame = self.within(IN_WINDOW, || self.window_frame(described.frame.as_ref(), &keys, scope))?;
        let (kind, t) = self.window_kind(&w.kind, &described, scope)?;
        Ok((self.windowed(keys, WindowCall { kind, frame, label: String::new() }), t))
    }

    /// That a window function may be bound in `scope` now.
    fn window_allowed(&self, scope: Scope<'_>) -> Result<()> {
        let windowing = self.windowing.borrow();
        let Some(w) = windowing.as_ref().filter(|w| w.open) else { return Err(rule(OUTSIDE)) };
        let refused = w.within.or(match scope {
            Scope::Rows(_) => Some(IN_ARGUMENT),
            Scope::Groups(..) | Scope::Select => None,
        });
        refused.map_or(Ok(()), |why| Err(rule(why)))
    }

    /// [ISO/IEC 9075-2:2023 §9.23 SR3, §7.15 SR4] That a window function
    /// of `aggregate`, if any, over `described` contains no query
    /// expression and no outer reference: its aggregate, sort keys and
    /// offsets. Its partitioning columns are its table expression's
    /// (`spec.rs`).
    fn own_references(&self, aggregate: Option<&ValueExpression>, described: &Described) -> Result<()> {
        let mut parts: Vec<&ValueExpression> = described.order.iter().map(|o| &o.key).collect();
        parts.extend(aggregate);
        if let Some(f) = &described.frame {
            for bound in std::iter::once(&f.start).chain(&f.end) {
                if let crate::ast::FrameBound::Preceding(v) | crate::ast::FrameBound::Following(v) = bound {
                    parts.push(v);
                }
            }
        }
        if parts.iter().any(|v| v.any(&|e| matches!(e, ValueExpression::Subquery(_)))) {
            return Err(rule("[ISO/IEC 9075-2:2023 §9.23 SR3] a window function cannot contain a query expression"));
        }
        let mut columns = Vec::new();
        for v in parts {
            every_column_reference(v, &mut columns);
        }
        for c in columns {
            if !self.is_local(c)? {
                return Err(rule("[ISO/IEC 9075-2:2023 §9.23 SR3] a window function cannot contain an outer reference"));
            }
        }
        Ok(())
    }

    /// The window structure descriptor `over` identifies: a definition of
    /// the `<window clause>` [§7.15 SR16], or an in-line specification
    /// described with those definitions in scope.
    fn described_window(&self, over: &WindowOver) -> Result<Described> {
        let windowing = self.windowing.borrow();
        let named = windowing.as_ref().map_or(&[][..], |w| w.named.as_slice());
        match over {
            WindowOver::Name(name, at) => named.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone()).ok_or_else(|| {
                SqlError::at(Code::SyntaxRule, *at, format!("[ISO/IEC 9075-2:2023 §7.15 SR16] no window {name} is in scope"))
            }),
            WindowOver::Specification(spec) => spec::describe(spec, named),
        }
    }

    /// [ISO/IEC 9075-2:2023 §9.16 SR1c, §11.9 CR2] Of a constraint, that
    /// the window function `kind` over `described` is no potential source
    /// of non-determinism: ROW_NUMBER, or a window of ROWS.
    fn deterministic(&self, kind: &Kind, described: &Described) -> Result<()> {
        if !self.purpose.constrains() {
            return Ok(());
        }
        let rows = described.frame.as_ref().is_some_and(|f| f.units == crate::ast::FrameUnits::Rows);
        let what = match kind {
            Kind::RowNumber => "ROW_NUMBER",
            Kind::Rank(_) | Kind::Aggregate(_) => {
                if !rows {
                    return Ok(());
                }
                "a window function over a window of ROWS"
            }
        };
        Err(super::expression::nondeterministic_source("c", what))
    }

    /// What `kind` computes over `described`, bound in `scope`, and its type.
    fn window_kind(&self, kind: &Kind, described: &Described, scope: Scope<'_>) -> Result<(WindowKind, DataType)> {
        let count = || {
            let count = self.db.functions.aggregate(kw::COUNT);
            count.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] COUNT is not registered")).and_then(|c| c.0.result_type(None))
        };
        let rank = |k| match k {
            crate::ast::RankKind::Rank => (WindowKind::Rank, count()),
            crate::ast::RankKind::DenseRank => (WindowKind::DenseRank, count()),
            // [§9.23 SR5g, SR5h] approximate numeric of the precision
            // (IV082, IV083) DOUBLE PRECISION has.
            crate::ast::RankKind::PercentRank => (WindowKind::PercentRank, Ok(double())),
            crate::ast::RankKind::CumeDist => (WindowKind::CumeDist, Ok(double())),
        };
        let (kind, t) = match kind {
            // [§9.23 SR5f, IV166] ROW_NUMBER and RANK, DENSE_RANK of
            // COUNT's type.
            Kind::RowNumber => (WindowKind::RowNumber, count()),
            Kind::Rank(k) => rank(*k),
            Kind::Aggregate(aggregate) => return self.window_aggregate_of(aggregate, described, scope),
        };
        Ok((kind, t?))
    }

    /// The aggregate function `aggregate` of a window function over
    /// `described`, bound in `scope`, and its type [§9.23 SR8b].
    fn window_aggregate_of(&self, aggregate: &ValueExpression, described: &Described, scope: Scope<'_>) -> Result<(WindowKind, DataType)> {
        spec::aggregate_rules(aggregate, described)?;
        if let Some(w) = self.windowing.borrow_mut().as_mut() {
            (w.taking, w.taken) = (true, None);
        }
        let bound = self.value_expression(aggregate, scope);
        let taken = self.windowing.borrow_mut().as_mut().and_then(|w| {
            w.taking = false;
            w.taken.take()
        });
        let t = bound?.1;
        let call =
            taken.ok_or_else(|| rule("[ISO/IEC 9075-2:2023 §6.10] OVER follows an <aggregate function> or a rank function alone"))?;
        Ok((WindowKind::Aggregate(call), t))
    }

    /// `call`, over the window of `keys`, kept: its placeholder column.
    fn windowed(&self, keys: Keys, call: WindowCall) -> Expr {
        let mut windowing = self.windowing.borrow_mut();
        let Some(w) = windowing.as_mut() else { return Expr::Const(crate::value::Value::Null) };
        let k = w.calls;
        w.calls = k.saturating_add(1);
        let mut place = |e: Expr| {
            let at = w.extras.len();
            w.extras.push(e);
            at
        };
        let partition: Vec<usize> = keys.partition.into_iter().map(&mut place).collect();
        let order: Vec<_> = keys.order.into_iter().map(|(e, o)| (place(e), o)).collect();
        let found = w.windows.iter_mut().find(|p| p.key == keys.text);
        match found {
            Some(p) => p.calls.push((k, call)),
            None => w.windows.push(Partitioned { key: keys.text, partition, order, calls: vec![(k, call)] }),
        }
        Expr::Column(PLACEHOLDER.saturating_add(k))
    }
}

/// [ISO/IEC 9075-2:2023 §9.23 SR5g, SR5h] DOUBLE PRECISION, the type of
/// PERCENT_RANK and CUME_DIST, of the precision (IV082, IV083) it has.
fn double() -> DataType {
    DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision }
}

/// [ISO/IEC 9075-2:2023 §9.23 SR5] That ROW_NUMBER and the rank functions
/// have no frame, and the rank functions an ordering.
fn kind_rules(kind: &Kind, described: &Described) -> Result<()> {
    let name = match kind {
        Kind::Aggregate(_) => return Ok(()),
        Kind::RowNumber => "ROW_NUMBER",
        Kind::Rank(_) => {
            if described.order.is_empty() {
                return Err(rule("[ISO/IEC 9075-2:2023 §9.23 SR5a] a rank function's window has an ordering"));
            }
            "a rank function"
        }
    };
    if described.frame.is_some() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.23 SR5b] the window of {name} has no frame")));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §7.15 GR1b, §9.23] `input`, the rows of a query's
/// table expression or its groups, given the select list `exprs` after the
/// window functions `windowing` holds, if any: the expressions its windows
/// append, then each window's functions, then the select list, its
/// placeholders placed.
pub(super) fn windowed(windowing: Option<Windowing>, input: LogicalPlan, exprs: Vec<Expr>) -> LogicalPlan {
    let Some(windowing) = windowing.filter(Windowing::computes) else { return LogicalPlan::Project { exprs, input: Box::new(input) } };
    let names = input.names();
    let (mut plan, extra) = with_extras(input, windowing.extras);
    let column = |c: usize| extra.get(c).copied().unwrap_or(c);
    let mut placed = vec![0_usize; windowing.calls];
    for p in windowing.windows {
        let at = plan.width();
        let partition = p.partition.iter().map(|&c| column(c)).collect();
        let order = p.order.iter().map(|&(c, o)| crate::logical::SortKey::new(column(c), o)).collect();
        let mut calls = Vec::new();
        for (n, (k, call)) in p.calls.into_iter().enumerate() {
            if let Some(slot) = placed.get_mut(k) {
                *slot = at.saturating_add(n);
            }
            calls.push(labelled(call, &names));
        }
        plan = LogicalPlan::Window(Box::new(Window { partition, order, calls, input: plan }));
    }
    let place = |c: usize| c.checked_sub(PLACEHOLDER).and_then(|k| placed.get(k).copied()).unwrap_or(c);
    let exprs = exprs.into_iter().map(|e| map_expr_columns(e, &place)).collect();
    LogicalPlan::Project { exprs, input: Box::new(plan) }
}

/// `input` with the columns of `extras` its windows read, each the column
/// of the input it is if it is one, or of an extra alike before it, else
/// appended; and the column of each.
fn with_extras(input: LogicalPlan, extras: Vec<Expr>) -> (LogicalPlan, Vec<usize>) {
    let width = input.width();
    let (mut appended, mut texts): (Vec<Expr>, Vec<String>) = (Vec::new(), Vec::new());
    let mut columns = Vec::with_capacity(extras.len());
    for e in extras {
        if let Expr::Column(c) = e
            && c < width
        {
            columns.push(c);
            continue;
        }
        let text = format!("{e:?}");
        let at = texts.iter().position(|t| *t == text).unwrap_or_else(|| {
            texts.push(text);
            appended.push(e);
            appended.len().saturating_sub(1)
        });
        columns.push(width.saturating_add(at));
    }
    if appended.is_empty() {
        return (input, columns);
    }
    let exprs = (0..width).map(Expr::Column).chain(appended).collect();
    (LogicalPlan::Project { exprs, input: Box::new(input) }, columns)
}

/// Of a grouped windowed query [ISO/IEC 9075-2:2023 §9.20], what its
/// groups are made of before its windows.
pub(super) struct Inner {
    /// DISTINCT of its select list, which §9.20 makes its groups'
    /// \[SR22\].
    pub(super) once: bool,
    /// The collation each column of the groups is compared by, if any.
    pub(super) compared: Vec<Option<CollationRef>>,
    /// The first of the groups' columns that are set functions' values.
    pub(super) aggs_from: usize,
    /// [§7.13 SR17n] Of GROUP BY DISTINCT whose rows alike are removed:
    /// the value of each column grouped by, over the groups, its collation,
    /// and the first GROUPING bit among the groups' columns.
    pub(super) sl2: Option<(Vec<Expr>, Vec<Option<CollationRef>>, usize)>,
}

/// [ISO/IEC 9075-2:2023 §9.20, §7.13 SR17n] The select list `exprs` of a
/// grouped windowed query, of its groups `groups`, after the window
/// functions `w`: the groups made the rows of §9.20's derived table, its
/// set functions and the columns its select list and windows read
/// [SR6 to SR14], their rows alike removed as GROUP BY DISTINCT and
/// DISTINCT remove them, then its windows over them.
pub(super) fn grouped_windowed(mut w: Windowing, groups: LogicalPlan, inner: Inner, exprs: Vec<Expr>) -> LogicalPlan {
    let width = groups.width();
    let mut read = Vec::new();
    for e in exprs.iter().chain(&w.extras).chain(w.windows.iter().flat_map(|p| p.calls.iter()).filter_map(|(_, c)| argument(c))) {
        expr_columns(e, &mut read);
    }
    read.retain(|&c| c < inner.aggs_from);
    read.extend(inner.aggs_from..width);
    read.sort_unstable();
    read.dedup();
    let compared: Vec<Option<CollationRef>> = read.iter().map(|&c| inner.compared.get(c).cloned().flatten()).collect();
    let projected: Vec<Expr> = read.iter().map(|&c| Expr::Column(c)).collect();
    let mut plan = match inner.sl2 {
        Some((values, mut alike, bits)) => {
            let p = values.len();
            let mut list = values;
            list.extend((0..p).map(|k| Expr::Column(bits.saturating_add(k))));
            let start = list.len();
            list.extend(projected);
            alike.extend(std::iter::repeat_n(None, p));
            alike.extend(compared.iter().cloned());
            let once = distinct_by(LogicalPlan::Project { exprs: list, input: Box::new(groups) }, alike);
            LogicalPlan::Project { exprs: (start..start.saturating_add(read.len())).map(Expr::Column).collect(), input: Box::new(once) }
        }
        None => LogicalPlan::Project { exprs: projected, input: Box::new(groups) },
    };
    if inner.once {
        plan = distinct_by(plan, compared);
    }
    let place = |c: usize| read.binary_search(&c).unwrap_or(c);
    w.extras = std::mem::take(&mut w.extras).into_iter().map(|e| map_expr_columns(e, &place)).collect();
    for (_, call) in w.windows.iter_mut().flat_map(|p| p.calls.iter_mut()) {
        if let WindowKind::Aggregate(a) = &mut call.kind {
            a.arg = a.arg.take().map(|e| map_expr_columns(e, &place));
        }
    }
    let exprs = exprs.into_iter().map(|e| map_expr_columns(e, &place)).collect();
    windowed(Some(w), plan, exprs)
}

/// The argument of `call`, if an aggregate's of one.
fn argument(call: &WindowCall) -> Option<&Expr> {
    if let WindowKind::Aggregate(a) = &call.kind { a.arg.as_ref() } else { None }
}

/// `plan` DISTINCT, each of its columns compared by its collation of
/// `compared`, if any; as `query::distinct` makes it.
fn distinct_by(plan: LogicalPlan, compared: Vec<Option<CollationRef>>) -> LogicalPlan {
    let collations = any_of(compared);
    if collations.is_empty() {
        return LogicalPlan::Distinct { input: Box::new(plan) };
    }
    LogicalPlan::Aggregate {
        groups: (0..collations.len()).collect(),
        collations,
        aggs: Vec::new(),
        seeds: Vec::new(),
        input: Box::new(plan),
    }
}

/// `call`, its label its function and argument over the columns `names`.
fn labelled(call: WindowCall, names: &[String]) -> WindowCall {
    let label = match &call.kind {
        WindowKind::RowNumber => "ROW_NUMBER()".to_owned(),
        WindowKind::Rank => "RANK()".to_owned(),
        WindowKind::DenseRank => "DENSE_RANK()".to_owned(),
        WindowKind::PercentRank => "PERCENT_RANK()".to_owned(),
        WindowKind::CumeDist => "CUME_DIST()".to_owned(),
        WindowKind::Aggregate(a) => match &a.arg {
            None => format!("{}(*)", a.func.0.name()),
            Some(e) => {
                let distinct = if a.distinct { "DISTINCT " } else { "" };
                format!("{}({distinct}{}{})", a.func.0.name(), render_expr(e, names), a.func.0.label_suffix())
            }
        },
    };
    WindowCall { label, ..call }
}
