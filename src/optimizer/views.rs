//! \[DESIGN\] Filters pushed into the plans of views that are not updatable
//! (RULES.md D35). Such a view is its bound plan in the query naming it
//! (`binder/views.rs`), so the query's WHERE clause is a filter over the
//! view's projection, or its DISTINCT; pushed below them, it reaches the
//! view's own filters and tables, and a filter on grouping columns goes on
//! below the grouping (`PushHavingIntoWhere`). Split from optimizer.rs
//! (RULES.md R-SIZE-2).

use super::{Rule, RuleContext};
use crate::logical::{Cond, Expr, LogicalPlan, map_cond_exprs};

/// See [`super::CONSTANT_FOLDING`].
pub const PUSH_FILTER_THROUGH_PROJECT: &str = "push_filter_through_project";
/// See [`super::CONSTANT_FOLDING`].
pub const PUSH_FILTER_THROUGH_DISTINCT: &str = "push_filter_through_distinct";

/// Filter(c, Project(e, x)) = Project(e, Filter(c', x)), c' being c with
/// each column the projection's expression for it: a projection keeps one
/// row for each row of its input, so the filter keeps the same rows. Only
/// when neither the condition nor any expression of the projection can
/// raise an error, as the expressions are then evaluated on fewer rows
/// (RULES.md D15). A function call can (`Expr::can_raise`), so none is
/// substituted, nor evaluated twice, which a nondeterministic one must not
/// be.
pub struct PushFilterThroughProject;

impl Rule for PushFilterThroughProject {
    fn name(&self) -> &'static str {
        PUSH_FILTER_THROUGH_PROJECT
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Project { exprs, input: rows } = &**input else { return None };
        if cond.can_raise() || exprs.iter().any(Expr::can_raise) {
            return None;
        }
        let pushed = map_cond_exprs(cond.clone(), &|e| substitute(e, exprs));
        let filter = LogicalPlan::Filter { cond: pushed, input: rows.clone() };
        Some(LogicalPlan::Project { exprs: exprs.clone(), input: Box::new(filter) })
    }
}

/// `e` with each column the expression `exprs` has for it.
fn substitute(e: Expr, exprs: &[Expr]) -> Expr {
    if let Expr::Column(i) = e {
        return exprs.get(i).cloned().unwrap_or(Expr::Column(i));
    }
    e.map_children(&mut |child| substitute(child, exprs))
}

/// Filter(c, Distinct(x)) = Distinct(Filter(c, x)), when c treats rows that
/// are identical [X3.135-1986 §5.11 GR7] alike, so it keeps all of a set of
/// duplicates or none, and the one DISTINCT keeps is the same: c compares
/// values, without LIKE or subqueries, which can tell 'A' from 'A ', and
/// cannot raise an error, as it is then evaluated on the duplicates too
/// (RULES.md D15); so it calls no function either (`Expr::can_raise`).
pub struct PushFilterThroughDistinct;

impl Rule for PushFilterThroughDistinct {
    fn name(&self) -> &'static str {
        PUSH_FILTER_THROUGH_DISTINCT
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Distinct { input: rows } = &**input else { return None };
        if cond.can_raise() || !alike(cond) {
            return None;
        }
        let filter = LogicalPlan::Filter { cond: cond.clone(), input: rows.clone() };
        Some(LogicalPlan::Distinct { input: Box::new(filter) })
    }
}

/// Whether `c`, which cannot raise an error, treats identical rows alike
/// (see `PushFilterThroughDistinct`): it has no LIKE and no subquery, and
/// compares columns, constants and their negations, which no CASE, CAST
/// or function can tell apart.
fn alike(c: &Cond) -> bool {
    let plain = |e: &Expr| {
        let mut plain = true;
        expr_columns_only(e, &mut plain);
        plain
    };
    match c {
        Cond::Const(_) => true,
        Cond::Compare(a, _, b) => plain(a) && plain(b),
        Cond::IsNull(e, _) => plain(e),
        Cond::Or(a, b) | Cond::And(a, b) => alike(a) && alike(b),
        Cond::Not(a) | Cond::Is(a, ..) => alike(a),
        Cond::Like(..) | Cond::Exists(_) | Cond::Quantified(..) | Cond::Unique(_) | Cond::Match { .. } => false,
    }
}

/// Clears `plain` if `e` has anything but columns, constants, outer
/// references, signs and arithmetic.
fn expr_columns_only(e: &Expr, plain: &mut bool) {
    if matches!(e, Expr::Call(..) | Expr::Case(..) | Expr::Cast(..) | Expr::Domain(..) | Expr::Subquery(..)) {
        *plain = false;
    }
    e.for_each_child(&mut |child| expr_columns_only(child, plain));
}

// Unit tests: what each rule pushes and what it leaves (RULES.md R-TEST-1,
// R-TEST-8).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::{MemoryCatalog, QualifiedName};
    use crate::db::State;
    use crate::functions::{ScalarFunction, ScalarRef};
    use crate::logical::{Quantifier, Subquery, Truth};
    use crate::value::Spelled;
    use crate::value::{ArithOp, CompOp, DataType, Decimal, Value};

    struct Identity;
    impl ScalarFunction for Identity {
        fn name(&self) -> &'static str {
            "ID"
        }
        fn return_type(&self, args: &[DataType]) -> crate::error::Result<DataType> {
            Ok(args.first().copied().unwrap_or(DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Numeric }))
        }
        fn call(&self, args: &[Value]) -> crate::error::Result<Value> {
            Ok(args.first().cloned().unwrap_or(Value::Null))
        }
    }

    fn one() -> Expr {
        Expr::Const(Value::Exact(Decimal::new(1, 0)))
    }

    fn scan() -> Box<LogicalPlan> {
        Box::new(LogicalPlan::Scan {
            table: QualifiedName { auth: "A".into(), ident: "T".into() },
            names: vec!["K".into()],
            needed: vec![0],
        })
    }

    fn equals_one(e: Expr) -> Cond {
        Cond::Compare(e, CompOp::Eq, one())
    }

    fn exists() -> Cond {
        Cond::Exists(Arc::new(Subquery { plan: *scan(), outer: vec![Expr::Column(0)] }))
    }

    /// `rule` applied to a filter of `cond` over `node` made of a scan.
    fn applied(rule: &dyn Rule, cond: Cond, node: impl Fn(Box<LogicalPlan>) -> LogicalPlan) -> Option<LogicalPlan> {
        let st = State::with_builtins(Box::new(MemoryCatalog::default()));
        let plan = LogicalPlan::Filter { cond, input: Box::new(node(scan())) };
        rule.apply(&plan, &RuleContext { db: &st })
    }

    /// Verifies: [DESIGN] RULES.md D35, `PushFilterThroughProject`: a filter
    /// over a projection is pushed below it, its columns the projection's
    /// expressions, unless the condition or an expression of the
    /// projection can raise an error (RULES.md D15); a filter over anything
    /// else is left.
    #[test]
    fn filters_go_below_projections_that_cannot_raise() {
        let project = |exprs: Vec<Expr>| move |input| LogicalPlan::Project { exprs: exprs.clone(), input };
        let pushed = applied(
            &PushFilterThroughProject,
            equals_one(Expr::Column(1)),
            project(vec![Expr::Column(0), Expr::Neg(Box::new(Expr::Column(0)))]),
        );
        let Some(LogicalPlan::Project { input, .. }) = pushed else { panic!("{pushed:?}") };
        assert!(matches!(&*input, LogicalPlan::Filter { cond: Cond::Compare(Expr::Neg(_), ..), .. }), "{input:?}");
        let arith = Expr::Arith(Box::new(Expr::Column(0)), ArithOp::Add, Box::new(one()));
        assert!(applied(&PushFilterThroughProject, equals_one(Expr::Column(0)), project(vec![arith.clone()])).is_none(), "can raise");
        assert!(applied(&PushFilterThroughProject, equals_one(arith), project(vec![Expr::Column(0)])).is_none(), "can raise");
        assert!(applied(&PushFilterThroughProject, equals_one(Expr::Column(0)), |rows| LogicalPlan::Distinct { input: rows }).is_none());
    }

    /// Verifies: [DESIGN] RULES.md D35, `PushFilterThroughDistinct`: a
    /// filter of comparisons, null tests, constants and their AND, OR and
    /// NOT is pushed below DISTINCT; one with LIKE or a subquery, which can
    /// tell identical rows apart, or that can raise, is left.
    #[test]
    fn filters_go_below_distinct_that_treat_duplicates_alike() {
        let distinct = |input| LogicalPlan::Distinct { input };
        let both = |a: Cond, b: Cond| Cond::Or(Box::new(a), Box::new(Cond::Not(Box::new(b))));
        let pushed = [
            Cond::And(Box::new(equals_one(Expr::Column(0))), Box::new(Cond::IsNull(Expr::Column(0), true))),
            both(Cond::Const(Truth::True), equals_one(Expr::Column(0))),
        ];
        for cond in pushed {
            assert!(matches!(applied(&PushFilterThroughDistinct, cond.clone(), distinct), Some(LogicalPlan::Distinct { .. })), "{cond:?}");
        }
        let like = Cond::Like(Expr::Column(0), Expr::Const(Value::Char("A%".into())), None);
        let quantified =
            Cond::Quantified(vec![Expr::Column(0)], CompOp::Eq, Quantifier::Some, Arc::new(Subquery { plan: *scan(), outer: vec![] }));
        let call = Expr::Call(ScalarRef(Arc::new(Identity)), vec![Expr::Column(0)]);
        let left = [
            like,
            both(Cond::Const(Truth::True), exists()),
            Cond::And(Box::new(quantified), Box::new(Cond::Const(Truth::True))),
            equals_one(call),
        ];
        for cond in left {
            assert!(applied(&PushFilterThroughDistinct, cond.clone(), distinct).is_none(), "{cond:?}");
        }
        assert!(
            applied(&PushFilterThroughDistinct, equals_one(Expr::Column(0)), |input| LogicalPlan::Project { exprs: vec![], input })
                .is_none()
        );
    }

    /// Verifies: [DESIGN] `substitute` replaces each column, however deep,
    /// by the projection's expression for it, and leaves a column it has
    /// none for, constants and outer references.
    #[test]
    fn columns_are_replaced_by_their_expressions() {
        let call = |args| Expr::Call(ScalarRef(Arc::new(Identity)), args);
        let e = Expr::Arith(Box::new(call(vec![Expr::Column(0), Expr::Outer(0, "X".into())])), ArithOp::Mul, Box::new(Expr::Column(5)));
        let replaced = substitute(e, &[one()]);
        assert_eq!(
            format!("{replaced:?}"),
            format!("{:?}", Expr::Arith(Box::new(call(vec![one(), Expr::Outer(0, "X".into())])), ArithOp::Mul, Box::new(Expr::Column(5))))
        );
    }
}
