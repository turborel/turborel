//! \[EXTENSION\] [ISO/IEC 9075:1992 §17.9 GR5] A prepared statement's
//! kept plan (RULES.md D45) made ready for one execution: each `<dynamic
//! parameter specification>`'s slot (`Expr::Dynamic`) replaced by its
//! value, cast to the type its context gave it \[GR5f, GR5g\], in every
//! expression and condition of the plan, those of its subqueries' plans
//! among them, at any depth. The plan is then the one binding with the
//! values as constants makes, which the optimizer and the physical planner
//! see as they would see that: the indexes chosen are those its values
//! choose. Split from logical.rs (RULES.md R-SIZE-2).

use std::sync::Arc;

use super::{AggCall, Cond, Expr, LogicalPlan, Slot, Subquery};
use crate::error::{At, Code, Result, SqlError};
use crate::value::{Value, cast};

/// `plan`, each slot filled with its value of `values`, by place.
///
/// # Errors
///
/// CAST's, of a value its slot's type takes none of [§17.9 GR5f]: the
/// first met; SQLCODE -910 for a slot `values` has none for.
pub fn filled(plan: LogicalPlan, values: &[Value]) -> Result<LogicalPlan> {
    let mut filling = Filling { values, failed: None };
    let plan = filling.plan(plan);
    filling.failed.map_or(Ok(plan), Err)
}

/// `e`, each slot filled as `filled` fills a plan's.
///
/// # Errors
///
/// As `filled`.
pub fn filled_expr(e: Expr, values: &[Value]) -> Result<Expr> {
    let mut filling = Filling { values, failed: None };
    let e = filling.expr(e);
    filling.failed.map_or(Ok(e), Err)
}

/// The values slots are filled with, and the first error met.
struct Filling<'a> {
    values: &'a [Value],
    failed: Option<SqlError>,
}

impl Filling<'_> {
    /// Notes `e`, unless an error came first.
    fn fail(&mut self, e: SqlError) {
        self.failed.get_or_insert(e);
    }

    /// The constant `slot` is filled with; the slot itself if that fails.
    fn value(&mut self, slot: Slot) -> Expr {
        let Some(value) = self.values.get(slot.index).cloned() else {
            let missing = format!("[DESIGN] no value is given for <dynamic parameter specification> {}", slot.index.saturating_add(1));
            self.fail(SqlError::new(Code::Api, missing));
            return Expr::Dynamic(slot);
        };
        match cast(value, &slot.data_type).or_at(slot.at) {
            Ok(v) => Expr::Const(v),
            Err(e) => {
                self.fail(e);
                Expr::Dynamic(slot)
            }
        }
    }

    fn expr(&mut self, e: Expr) -> Expr {
        match e {
            Expr::Dynamic(slot) => self.value(slot),
            // A subquery's plan, beside its outer expressions; and a CASE's
            // conditions, with theirs.
            Expr::Subquery(s, column) => Expr::Subquery(self.subquery(&s), column),
            Expr::Case(whens, otherwise) => {
                let whens = whens.into_iter().map(|(c, result)| (self.cond(c), self.expr(result))).collect();
                Expr::Case(whens, Box::new(self.expr(*otherwise)))
            }
            other @ (Expr::Column(_)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Outer(..)
            | Expr::Cast(..)
            | Expr::Domain(..)) => other.map_children(&mut |child| self.expr(child)),
        }
    }

    fn subquery(&mut self, s: &Subquery) -> Arc<Subquery> {
        let outer = s.outer.iter().cloned().map(|e| self.expr(e)).collect();
        Arc::new(Subquery { plan: self.plan(s.plan.clone()), outer })
    }

    fn exprs(&mut self, exprs: Vec<Expr>) -> Vec<Expr> {
        exprs.into_iter().map(|e| self.expr(e)).collect()
    }

    fn cond(&mut self, c: Cond) -> Cond {
        match c {
            Cond::Const(_) => c,
            Cond::Or(a, b) => {
                let a = self.cond(*a);
                Cond::Or(Box::new(a), Box::new(self.cond(*b)))
            }
            Cond::And(a, b) => {
                let a = self.cond(*a);
                Cond::And(Box::new(a), Box::new(self.cond(*b)))
            }
            Cond::Not(a) => Cond::Not(Box::new(self.cond(*a))),
            Cond::Is(a, truth, not) => Cond::Is(Box::new(self.cond(*a)), truth, not),
            Cond::Compare(a, op, b) => {
                let a = self.expr(a);
                Cond::Compare(a, op, self.expr(b))
            }
            Cond::IsNull(e, not) => Cond::IsNull(self.expr(e), not),
            Cond::In(e, set) => Cond::In(self.expr(e), set),
            Cond::Like(e, pattern, escape) => {
                let (e, pattern) = (self.expr(e), self.expr(pattern));
                Cond::Like(e, pattern, escape.map(|x| self.expr(x)))
            }
            Cond::Exists(s) => Cond::Exists(self.subquery(&s)),
            Cond::Unique(s) => Cond::Unique(self.subquery(&s)),
            Cond::Quantified(values, op, quantifier, s) => {
                let values = self.exprs(values);
                Cond::Quantified(values, op, quantifier, self.subquery(&s))
            }
            Cond::Match { values, unique, match_type, subquery } => {
                let values = self.exprs(values);
                Cond::Match { values, unique, match_type, subquery: self.subquery(&subquery) }
            }
        }
    }

    fn plan(&mut self, plan: LogicalPlan) -> LogicalPlan {
        let node = match plan {
            LogicalPlan::Filter { cond, input } => LogicalPlan::Filter { cond: self.cond(cond), input },
            LogicalPlan::Project { exprs, input } => LogicalPlan::Project { exprs: self.exprs(exprs), input },
            LogicalPlan::Aggregate { groups, aggs, input } => {
                let aggs = aggs.into_iter().map(|a| AggCall { arg: a.arg.map(|e| self.expr(e)), ..a }).collect();
                LogicalPlan::Aggregate { groups, aggs, input }
            }
            LogicalPlan::OuterJoin { kind, cond, left, right } => LogicalPlan::OuterJoin { kind, cond: self.cond(cond), left, right },
            LogicalPlan::Values { rows, width } => LogicalPlan::Values { rows: rows.into_iter().map(|r| self.exprs(r)).collect(), width },
            other @ (LogicalPlan::Scan { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Sort { .. }) => other,
        };
        node.map_inputs(|p| self.plan(p))
    }
}

// Unit tests: every expression and condition a slot can stand in, and the
// errors of filling one (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{MatchType, QualifiedName};
    use crate::error::Code;
    use crate::logical::{InSet, OuterJoin, Quantifier, Truth};
    use crate::value::{CompOp, DataType, Decimal, Spelled};

    const INT: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

    fn slot(index: usize) -> Expr {
        Expr::Dynamic(Slot { index, data_type: INT, at: 7 })
    }

    fn scan() -> LogicalPlan {
        LogicalPlan::Scan { table: QualifiedName { auth: "A".into(), ident: "U".into() }, names: vec!["K".into()], needed: vec![0] }
    }

    /// A subquery of U whose condition and outer expression have slot 1.
    fn subquery() -> Arc<Subquery> {
        let cond = Cond::Compare(Expr::Column(0), CompOp::Eq, slot(1));
        Arc::new(Subquery { plan: LogicalPlan::Filter { cond, input: Box::new(scan()) }, outer: vec![slot(1)] })
    }

    /// Whether a slot is left anywhere in `plan`, its subqueries' too.
    fn has_slot(plan: &LogicalPlan) -> bool {
        format!("{plan:?}").contains("Dynamic")
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5f]; [DESIGN]
    /// RULES.md D45: every slot of a plan is filled with its value, cast to
    /// its type: in each kind of condition, a CASE, a subquery's plan and
    /// outer expressions, an outer join's condition and a VALUES row.
    #[test]
    fn every_slot_is_filled_with_its_value() {
        let compare = |e: Expr| Cond::Compare(Expr::Column(0), CompOp::Lt, e);
        let conds = vec![
            Cond::Or(Box::new(compare(slot(0))), Box::new(Cond::Not(Box::new(compare(slot(1)))))),
            Cond::Is(Box::new(compare(slot(0))), Truth::True, false),
            Cond::IsNull(slot(1), true),
            Cond::In(slot(0), Arc::new(InSet::of(vec![Value::Exact(Decimal::new(12, 0))]))),
            Cond::Like(slot(0), slot(1), Some(slot(0))),
            Cond::Exists(subquery()),
            Cond::Unique(subquery()),
            Cond::Quantified(vec![slot(0)], CompOp::Eq, Quantifier::Some, subquery()),
            Cond::Match { values: vec![slot(1)], unique: true, match_type: MatchType::Full, subquery: subquery() },
            Cond::Const(Truth::True),
        ];
        let cond = conds.into_iter().reduce(|a, b| Cond::And(Box::new(a), Box::new(b))).unwrap();
        let case = Expr::Case(vec![(compare(slot(0)), slot(1))], Box::new(Expr::Subquery(subquery(), 0)));
        let joined = LogicalPlan::OuterJoin {
            kind: OuterJoin::Left,
            cond: compare(slot(1)),
            left: Box::new(LogicalPlan::Values { rows: vec![vec![slot(0)]], width: 1 }),
            right: Box::new(scan()),
        };
        let plan = LogicalPlan::Project { exprs: vec![case], input: Box::new(LogicalPlan::Filter { cond, input: Box::new(joined) }) };
        assert!(has_slot(&plan));
        let filled = filled(plan, &[Value::Char(" 12 ".into()), Value::Exact(Decimal::new(3, 0))]).unwrap();
        assert!(!has_slot(&filled), "{filled:?}");
        assert!(format!("{filled:?}").contains("units: 12"), "the string cast to INTEGER: {filled:?}");
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 GR5f]; [DESIGN]
    /// RULES.md D45: a value its slot's type takes none of fails as CAST
    /// fails, at the `?`; a slot given no value is refused.
    #[test]
    fn a_slot_not_filled_is_an_error() {
        let failed = filled_expr(slot(0), &[Value::Char("ABC".into())]).unwrap_err();
        assert_eq!((failed.code, failed.pos), (Code::Data, Some(7)));
        let first = filled_expr(Expr::Neg(Box::new(slot(0))), &[Value::Char("X".into())]).unwrap_err();
        assert_eq!(first.code, Code::Data);
        assert_eq!(filled_expr(slot(2), &[]).unwrap_err().code, Code::Api);
    }
}
