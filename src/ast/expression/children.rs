//! The passes over a `<value expression>` and a `<search condition>` that
//! reach the expressions each is made of, stopping at a subquery, a query
//! of its own.

use super::{CaseExpression, Predicate, SearchCondition, Subquery, ValueExpression, WhenOperand};
use crate::ast::SetArg;

impl ValueExpression {
    /// Calls `f` with each value expression this one is made of: operands,
    /// arguments, a set function's argument and those its filter compares,
    /// the parts of a CASE and the values its conditions compare; not those
    /// of a subquery.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            ValueExpression::Value(_)
            | ValueExpression::Column(_)
            | ValueExpression::Subquery(_)
            | ValueExpression::Datetime(_)
            | ValueExpression::NextValue { .. }
            | ValueExpression::Classifier { .. }
            | ValueExpression::MatchNumber { .. } => {}
            ValueExpression::Navigation(n) => {
                f(&n.expr);
                n.logical.iter().filter_map(|l| l.offset.as_deref()).for_each(&mut *f);
                n.physical.iter().filter_map(|p| p.offset.as_deref()).for_each(&mut *f);
            }
            ValueExpression::Extract { source, .. } => f(source),
            ValueExpression::Difference { minuend, subtrahend, .. } => {
                f(minuend);
                f(subtrahend);
            }
            ValueExpression::AtZone { source, zone, .. } => {
                f(source);
                if let Some(z) = zone {
                    f(z);
                }
            }
            ValueExpression::Dyadic(a, _, b) => {
                f(a);
                f(b);
            }
            ValueExpression::Monadic(_, x)
            | ValueExpression::Nested(x)
            | ValueExpression::Collated(x, _)
            | ValueExpression::CollationFor(x) => f(x),
            ValueExpression::SetFunction(sf) => {
                match &sf.arg {
                    SetArg::CountStar | SetArg::Variable(..) => {}
                    SetArg::Distinct(x) | SetArg::All(x) | SetArg::Fraction(x) => f(x),
                    SetArg::Grouping(columns) | SetArg::Hypothetical(columns) => columns.iter().for_each(&mut *f),
                }
                sf.order_by.iter().for_each(|o| f(&o.key));
                sf.filter.iter().for_each(|c| c.for_each_value(f));
            }
            ValueExpression::Cast { operand, .. } => operand.iter().for_each(|x| f(x)),
            ValueExpression::Function { args, .. } | ValueExpression::Call { args, .. } => args.iter().for_each(f),
            ValueExpression::Case(case) => case.for_each_child(f),
            ValueExpression::Boolean(c) => c.for_each_value(f),
            ValueExpression::Composite(c, _) => c.for_each_child(f),
            ValueExpression::Structured(s, _) => s.for_each_child(f),
            ValueExpression::Window(w) => w.for_each_child(f),
            ValueExpression::NestedWindow(n) => n.for_each_child(f),
        }
    }

    /// Calls `f` with each subquery in it, not those nested in them: its
    /// scalar subqueries, and those of the conditions of its CASEs.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        if let ValueExpression::Subquery(s) = self {
            f(s);
        }
        if let ValueExpression::Composite(c, _) = self
            && let Some(s) = c.subquery()
        {
            f(s);
        }
        if let ValueExpression::Case(case) = self
            && let CaseExpression::Searched { whens, .. } = &**case
        {
            for (c, _) in whens {
                c.for_each_own_subquery(f);
            }
        }
        if let ValueExpression::Boolean(c) = self {
            c.for_each_own_subquery(f);
        }
        if let ValueExpression::SetFunction(sf) = self
            && let Some(c) = &sf.filter
        {
            c.for_each_own_subquery(f);
        }
        self.for_each_child(&mut |child| child.for_each_subquery(f));
    }

    /// Whether `pred` holds of this expression or of one it is made of
    /// (`for_each_child`), at any depth.
    pub fn any(&self, pred: &dyn Fn(&ValueExpression) -> bool) -> bool {
        let mut found = pred(self);
        self.for_each_child(&mut |child| found = found || child.any(pred));
        found
    }
}

impl CaseExpression {
    /// Calls `f` with each value expression of it, those its searched WHEN
    /// conditions compare among them.
    fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            CaseExpression::NullIf(a, b) => {
                f(a);
                f(b);
            }
            CaseExpression::Coalesce(values) => values.iter().for_each(f),
            CaseExpression::Simple { operand, whens, otherwise } => {
                operand.iter().for_each(&mut *f);
                for (list, r) in whens {
                    for when in list {
                        match when {
                            WhenOperand::Value(row, _) => row.iter().for_each(&mut *f),
                            WhenOperand::Condition(c) => c.for_each_value(f),
                        }
                    }
                    if let Some(x) = r {
                        f(x);
                    }
                }
                if let Some(x) = otherwise {
                    f(x);
                }
            }
            CaseExpression::Searched { whens, otherwise } => {
                for (c, r) in whens {
                    c.for_each_value(f);
                    if let Some(x) = r {
                        f(x);
                    }
                }
                if let Some(x) = otherwise {
                    f(x);
                }
            }
        }
    }
}

impl SearchCondition {
    /// Calls `f` with each value expression the condition compares, not
    /// those of its subqueries.
    pub fn for_each_value<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            SearchCondition::Or(a, b) | SearchCondition::And(a, b) => {
                a.for_each_value(f);
                b.for_each_value(f);
            }
            SearchCondition::Not(a) | SearchCondition::Is(a, ..) => a.for_each_value(f),
            SearchCondition::Predicate(p, _) => p.for_each_value(f),
            SearchCondition::Predicand(v, _) => f(v),
        }
    }

    /// Calls `f` with each subquery in it, not those nested in them: those
    /// its predicates apply to, and those in the values it compares.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        self.for_each_own_subquery(f);
        self.for_each_value(&mut |v| v.for_each_subquery(f));
    }

    /// Calls `f` with each subquery its predicates apply to.
    fn for_each_own_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        match self {
            SearchCondition::Or(a, b) | SearchCondition::And(a, b) => {
                a.for_each_own_subquery(f);
                b.for_each_own_subquery(f);
            }
            SearchCondition::Not(a) | SearchCondition::Is(a, ..) => a.for_each_own_subquery(f),
            SearchCondition::Predicate(p, _) => p.subquery().into_iter().for_each(f),
            SearchCondition::Predicand(..) => {}
        }
    }
}

impl Predicate {
    /// Calls `f` with each value expression the predicate compares, not
    /// those of its subqueries.
    pub fn for_each_value<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            Predicate::Comparison(a, _, b) | Predicate::Overlaps(a, b) | Predicate::Distinct { left: a, right: b, .. } => {
                a.iter().chain(b).for_each(f);
            }
            Predicate::Between { value, low, high, .. } => value.iter().chain(low).chain(high).for_each(f),
            Predicate::InList { value, list, .. } => value.iter().chain(list).for_each(f),
            Predicate::Like { value, pattern, escape: third, .. } | Predicate::LikeRegex { value, pattern, flag: third, .. } => {
                f(value);
                f(pattern);
                if let Some(e) = third {
                    f(e);
                }
            }
            Predicate::Member { value, multiset, .. } | Predicate::Submultiset { value, multiset, .. } => {
                value.iter().for_each(&mut *f);
                f(multiset);
            }
            Predicate::InSubquery { value, .. }
            | Predicate::Set { value, .. }
            | Predicate::Type { value, .. }
            | Predicate::Null { value, .. }
            | Predicate::Quantified { value, .. }
            | Predicate::Match { value, .. } => {
                value.iter().for_each(f);
            }
            Predicate::Exists(_) | Predicate::Unique { .. } => {}
        }
    }

    /// The `<table subquery>` it applies to, if it has one.
    pub fn subquery(&self) -> Option<&Subquery> {
        match self {
            Predicate::InSubquery { subquery, .. }
            | Predicate::Quantified { subquery, .. }
            | Predicate::Match { subquery, .. }
            | Predicate::Unique { subquery, .. }
            | Predicate::Exists(subquery) => Some(subquery),
            Predicate::Comparison(..)
            | Predicate::Overlaps(..)
            | Predicate::Between { .. }
            | Predicate::InList { .. }
            | Predicate::Like { .. }
            | Predicate::LikeRegex { .. }
            | Predicate::Distinct { .. }
            | Predicate::Member { .. }
            | Predicate::Submultiset { .. }
            | Predicate::Set { .. }
            | Predicate::Type { .. }
            | Predicate::Null { .. } => None,
        }
    }
}
