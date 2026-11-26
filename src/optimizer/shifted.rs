//! \[DESIGN\] SUM of a value and a constant, from the value's sum and
//! count (RULES.md D1216). The sum of x + c over the rows where x is not
//! null is the sum of x plus c times their number, so SUM(x + c) is
//! SUM(x) + c * COUNT(x), SUM(x - c) is SUM(x) - c * COUNT(x) and
//! SUM(c - x) is c * COUNT(x) - SUM(x). Every such SUM of one grouping
//! then reads one SUM(x) and one COUNT(x), each fed the column as it is,
//! where each evaluated its own sum of x and its constant a row.
//!
//! The two are one value only where the arithmetic agrees, so the rule
//! takes exact numbers alone. x + c is null exactly where x is, c being a
//! literal not null, so COUNT(x) counts the values summed [ISO/IEC
//! 9075-2:2023 §10.9 GR7a, GR7c], the nulls eliminated raise the same
//! warning, and of no value SUM(x) is null [GR7d ii] and so is what is
//! computed of it. A sum raises only past the range of its type [GR7d
//! vi]: where the declared types keep x + c within so few digits that no
//! sum of fewer than 2^63 of them leaves it (`total_fits`, RULES.md
//! D1060), neither SUM(x + c), SUM(x), c * COUNT(x) nor their sum or
//! difference can raise, and x + c itself is within its digits. The rule
//! takes no other SUM, so it turns no statement that raises into one that
//! answers, nor the reverse. Exact arithmetic is exact, and the result
//! has the scale of x + c, the greater of the two scales, either way.
//!
//! A filter clause goes to both shared calls, and they are shared among
//! the SUMs of one value and filter, a SUM or COUNT of it the statement
//! wrote among them. It leaves DISTINCT, whose values are other values;
//! approximate and DECFLOAT numbers, whose sums round; AVG, which
//! truncates its quotient, so AVG(x) + c is another value where the mean
//! is below zero; a value that is not columns, constants, arithmetic and
//! casts, which could differ from one evaluation to the next; and a
//! grouping that makes groups before its rows.

use super::{Rule, RuleContext};
use crate::db::State;
use crate::functions::Standard;
use crate::lexer::kw;
use crate::logical::raises::{set_function_type, total_fits};
use crate::logical::{AggCall, Expr, LogicalPlan, MAX_JUDGED_DEPTH, Types, plan_column_type, render_cond, render_expr};
use crate::value::{ArithOp, DataType, MAX_EXACT_PRECISION, Spelled, Value};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const SHARE_SHIFTED_SUMS: &str = "share_shifted_sums";

/// SUM of a value and a constant from the value's sum and count, as the
/// module says.
pub struct ShareShiftedSums;

/// How a SUM's argument is made of its value x and its constant c.
#[derive(Clone, Copy)]
enum Shift {
    /// x + c or c + x.
    Plus,
    /// x - c.
    Minus,
    /// c - x.
    From,
}

/// A SUM's argument taken apart: its value, its constant and how they
/// are joined.
struct Shifted<'a> {
    x: &'a Expr,
    c: &'a Value,
    shift: Shift,
}

impl Shifted<'_> {
    /// The SUM's value, of the columns `sum` and `count` of the grouping.
    fn value(&self, sum: usize, count: usize) -> Expr {
        let product = Box::new(Expr::Arith(Box::new(Expr::Const(self.c.clone())), ArithOp::Mul, Box::new(Expr::Column(count))));
        let sum = Box::new(Expr::Column(sum));
        match self.shift {
            Shift::Plus => Expr::Arith(sum, ArithOp::Add, product),
            Shift::Minus => Expr::Arith(sum, ArithOp::Sub, product),
            Shift::From => Expr::Arith(product, ArithOp::Sub, sum),
        }
    }
}

/// `arg` as a value and an exact constant added or subtracted, if it is
/// that, the value no constant itself.
fn taken_apart(arg: &Expr) -> Option<Shifted<'_>> {
    let Expr::Arith(left, op, right) = arg else { return None };
    let shift = |constant_last: bool| match (op, constant_last) {
        (ArithOp::Add, _) => Some(Shift::Plus),
        (ArithOp::Sub, true) => Some(Shift::Minus),
        (ArithOp::Sub, false) => Some(Shift::From),
        (ArithOp::Mul | ArithOp::Div, _) => None,
    };
    if let (x, Expr::Const(c @ Value::Exact(_)), false) = (&**left, &**right, matches!(**left, Expr::Const(_))) {
        return shift(true).map(|shift| Shifted { x, c, shift });
    }
    if let (Expr::Const(c @ Value::Exact(_)), x, false) = (&**left, &**right, matches!(**right, Expr::Const(_))) {
        return shift(false).map(|shift| Shifted { x, c, shift });
    }
    None
}

/// Whether `e` is columns and constants under arithmetic, negation and
/// casts alone, so each evaluation of it of a row gives one value.
fn recomputed_alike(e: &Expr, depth: usize) -> bool {
    let own = matches!(e, Expr::Column(_) | Expr::Const(_) | Expr::Neg(_) | Expr::Arith(..) | Expr::Cast(..));
    let mut parts = depth <= MAX_JUDGED_DEPTH;
    e.for_each_child(&mut |child| parts = parts && recomputed_alike(child, depth.saturating_add(1)));
    own && parts
}

/// What a rewrite reads beside the grouping's calls.
struct Reading<'a> {
    db: &'a State,
    types: Types<'a>,
    names: Vec<String>,
}

impl Reading<'_> {
    /// `a` as a SUM the rule takes, as the module says, and the SUM of
    /// its value alone that takes its place: of the declared types that
    /// keep both sums within an exact number's digits.
    fn shifted<'c>(&self, a: &'c AggCall) -> Option<(Shifted<'c>, AggCall)> {
        let functions = &self.db.functions;
        if functions.standard(&a.func) != Some(Standard::Sum) || a.distinct || a.distribution.is_some() {
            return None;
        }
        let found = taken_apart(a.arg.as_ref()?).filter(|s| recomputed_alike(s.x, 0))?;
        let of_value = AggCall { arg: Some(found.x.clone()), ..a.clone() };
        if !total_fits(a, self.types) || !total_fits(&of_value, self.types) {
            return None;
        }
        let DataType::Exact { scale, .. } = set_function_type(&of_value, self.types)? else { return None };
        let summed = DataType::Exact { precision: MAX_EXACT_PRECISION, scale, spelled: Spelled::Numeric };
        let result = a.func.0.result_type(Some(summed)).ok()?;
        let label = self.label(kw::SUM, &of_value);
        Some((found, AggCall { result, label, ..of_value }))
    }

    /// COUNT of the value `sum` sums, under its filter clause.
    fn count_of(&self, sum: &AggCall) -> Option<AggCall> {
        let functions = &self.db.functions;
        let func = functions.aggregate(kw::COUNT).filter(|f| functions.standard(f) == Some(Standard::Count))?;
        let result = func.0.result_type(None).ok()?;
        Some(AggCall { func, result, label: self.label(kw::COUNT, sum), ..sum.clone() })
    }

    /// The label of the set function `name` of the argument and filter
    /// clause of `a`, as the binder writes one, by which repeats of one
    /// call are found.
    fn label(&self, name: &str, a: &AggCall) -> String {
        let arg = a.arg.as_ref().map_or_else(String::new, |e| render_expr(e, &self.names));
        match &a.filter {
            Some(c) => format!("{name}({arg}) FILTER (WHERE {})", render_cond(c, &self.names)),
            None => format!("{name}({arg})"),
        }
    }

    /// Where the call `call` is among `kept`, added if no call there is
    /// the same function of the same argument and filter clause.
    fn place(&self, kept: &mut Vec<AggCall>, call: AggCall) -> usize {
        let plain = |k: &AggCall| !k.distinct && k.distribution.is_none() && k.arg.is_some();
        let same =
            |k: &AggCall| plain(k) && std::sync::Arc::ptr_eq(&k.func.0, &call.func.0) && self.label(call.func.0.name(), k) == call.label;
        if let Some(at) = kept.iter().position(same) {
            return at;
        }
        kept.push(call);
        kept.len().saturating_sub(1)
    }
}

impl Rule for ShareShiftedSums {
    fn name(&self) -> &'static str {
        SHARE_SHIFTED_SUMS
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        let catalog = &*ctx.db.catalog;
        let types = |c: usize| plan_column_type(input, c, catalog);
        let reading = Reading { db: ctx.db, types: &types, names: input.names() };
        let found: Vec<_> = aggs.iter().map(|a| reading.shifted(a).filter(|_| seeds.is_empty())).collect();
        if found.iter().all(Option::is_none) {
            return None;
        }
        let mut kept: Vec<AggCall> = aggs.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(a, _)| a.clone()).collect();
        let column = |at: usize| groups.len().saturating_add(at);
        let mut exprs: Vec<Expr> = (0..groups.len()).map(Expr::Column).collect();
        let mut left = 0_usize;
        for f in found {
            let Some((shifted, sum)) = f else {
                exprs.push(Expr::Column(column(left)));
                left = left.saturating_add(1);
                continue;
            };
            let count = reading.count_of(&sum)?;
            let (sum_at, count_at) = (reading.place(&mut kept, sum), reading.place(&mut kept, count));
            exprs.push(shifted.value(column(sum_at), column(count_at)));
        }
        let grouped = LogicalPlan::Aggregate {
            groups: groups.clone(),
            collations: collations.clone(),
            aggs: kept,
            seeds: seeds.clone(),
            input: input.clone(),
        };
        Some(LogicalPlan::Project { exprs, input: Box::new(grouped) })
    }
}
